//! The comparison itself: QEMU's states, read from its trace, against the machine's, one
//! instruction at a time, under the policy of the table.

use crate::report::{HistoryEntry, Nearest, Report};
use crate::state::{CpuState, Field};
use crate::table::{DIFFERENCES, Difference, Treatment, Unemulated, unemulated};
use crate::trace::{QemuTrap, TraceEvent};
use cpu_rv32::{Decoded, Instr};
use soc_esp32c3::{Esp32C3, Executed, Stepped, StopReason};
use std::collections::{BTreeMap, VecDeque};

/// The instructions a report shows before the divergence, from both sides (spec §4.4).
pub const HISTORY: usize = 64;

/// The address of the first instruction of the ROM, where both sides start after any reset.
pub const RESET_PC: u32 = 0x4000_0000;

/// The side the lockstep drives: the machine, through its stepping API (decision H1).
pub trait Target {
    /// The compared state at the current instruction boundary.
    fn state(&self) -> CpuState;
    /// Executes one instruction and moves to the next boundary ([`Esp32C3::step`]).
    fn step(&mut self) -> Stepped;
    /// Writes an integer register.
    fn set_x(&mut self, reg: u8, value: u32);
    /// Resets since power-on.
    fn resets(&self) -> u64;
}

impl Target for Esp32C3 {
    fn state(&self) -> CpuState {
        CpuState::new(self.pc(), self.x(), |address| self.csr(address))
    }

    fn step(&mut self) -> Stepped {
        Esp32C3::step(self)
    }

    fn set_x(&mut self, reg: u8, value: u32) {
        Esp32C3::set_x(self, usize::from(reg), value);
    }

    fn resets(&self) -> u64 {
        Esp32C3::resets(self)
    }
}

/// Why the comparison ended without a divergence (decision H4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowEnd {
    /// A reset after the instruction: QEMU went back to the first instruction of the ROM (it
    /// resets the whole machine), the machine counted a reset, or both.
    Reset { qemu: bool, inkemu: bool },
    /// The instruction at `pc` accessed `address`, in a block QEMU does not emulate.
    Unemulated {
        block: &'static str,
        address: u32,
        pc: u32,
    },
    /// The number of instructions asked for were compared.
    Limit,
    /// QEMU's trace ended.
    TraceEnd,
}

impl std::fmt::Display for WindowEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowEnd::Reset {
                qemu: true,
                inkemu: true,
            } => f.write_str("reset (QEMU and inkemu)"),
            WindowEnd::Reset { qemu: true, .. } => f.write_str("reset (QEMU only)"),
            WindowEnd::Reset { .. } => f.write_str("reset (inkemu only)"),
            WindowEnd::Unemulated { block, address, pc } => write!(
                f,
                "unemulated peripheral {block} (address {address:#010x}, pc {pc:#010x})"
            ),
            WindowEnd::Limit => f.write_str("instruction limit"),
            WindowEnd::TraceEnd => f.write_str("end of the QEMU trace"),
        }
    }
}

/// How a comparison ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The window ended with no divergence outside the table.
    Window(WindowEnd),
    /// The two sides diverged.
    Divergence(Box<Report>),
    /// The machine stopped on its own before the next instruction (a sleep nothing ends, a
    /// strict-mode violation): the two can no longer be compared.
    Stopped { reason: StopReason, pc: u32 },
}

/// The counts a run of the lockstep ends with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Instructions both sides executed and whose following states were compared.
    pub compared: u64,
    /// Blocks of the trace dropped as duplicates (oracle §1c pitfall 1).
    pub duplicates: u64,
    /// Blocks dropped because an interrupt was taken at their entry ([`Lockstep::feed`]).
    pub preempted: u64,
    /// Interrupts both sides took.
    pub interrupts: u64,
    /// How many times each row of the table was used, by number.
    pub rows: BTreeMap<u16, u64>,
}

/// One comparison, fed QEMU's trace event by event.
#[derive(Debug)]
pub struct Lockstep {
    rows: &'static [Difference],
    limit: Option<u64>,
    counts: Counts,
    /// The last block compared: its state and fingerprint.
    last_block: Option<(CpuState, u64)>,
    /// The machine's state at the last boundary compared, and the instruction it then executed.
    before: CpuState,
    executed: Option<Stepped>,
    /// The trap QEMU took before the next block.
    trap: Option<QemuTrap>,
    /// The last block read, not yet compared ([`Lockstep::feed`]).
    held: Option<(CpuState, u64)>,
    resets: u64,
    history: VecDeque<HistoryEntry>,
}

impl Lockstep {
    /// A comparison of a machine at power-on under the table of this crate, ending after `limit`
    /// compared instructions when one is given.
    #[must_use]
    pub fn new(target: &impl Target, limit: Option<u64>) -> Lockstep {
        Lockstep::with_rows(target, DIFFERENCES, limit)
    }

    /// The same under another table: for the tests of the policy.
    #[must_use]
    pub fn with_rows(
        target: &impl Target,
        rows: &'static [Difference],
        limit: Option<u64>,
    ) -> Lockstep {
        // A row (iv) is a setting the machine was built with ([`crate::table::configure`]): it
        // is used once, from the start.
        let mut counts = Counts::default();
        for row in rows {
            if row
                .treatments
                .iter()
                .any(|treatment| matches!(treatment, Treatment::Compat(_)))
            {
                counts.rows.insert(row.number, 1);
            }
        }
        Lockstep {
            rows,
            limit,
            counts,
            last_block: None,
            before: target.state(),
            executed: None,
            trap: None,
            held: None,
            resets: target.resets(),
            history: VecDeque::with_capacity(HISTORY),
        }
    }

    /// The counts so far.
    #[must_use]
    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    /// Takes the next event of the trace; `Some` when the comparison ends with it.
    ///
    /// A block is held until the event after it is known: QEMU logs a block when it enters it,
    /// and an interrupt that became pending meanwhile is taken at that entry, before the
    /// instruction executes. Such a block is followed by the interrupt with `epc` its own PC: it
    /// is the state the previous instruction left, which the machine, taking the interrupt as
    /// part of that instruction's boundary, never shows. It is dropped and counted.
    pub fn feed(&mut self, event: TraceEvent, target: &mut impl Target) -> Option<End> {
        match event {
            TraceEvent::Trap(trap) => {
                match self.held.take() {
                    Some((state, _)) if trap.asynchronous && state.pc() == trap.epc => {
                        self.counts.preempted += 1;
                    }
                    Some((state, fingerprint)) => {
                        if let Some(end) = self.block(state, fingerprint, target) {
                            return Some(end);
                        }
                    }
                    None => {}
                }
                self.trap = Some(trap);
                None
            }
            TraceEvent::State { state, fingerprint } => {
                let held = self.held.replace((state, fingerprint));
                held.and_then(|(state, fingerprint)| self.block(state, fingerprint, target))
            }
        }
    }

    /// QEMU's trace ended: the block held is compared, and the window ends with the trace.
    pub fn finish(&mut self, target: &mut impl Target) -> End {
        if let Some((state, fingerprint)) = self.held.take()
            && let Some(end) = self.block(state, fingerprint, target)
        {
            return end;
        }
        End::Window(WindowEnd::TraceEnd)
    }

    fn block(&mut self, qemu: CpuState, fingerprint: u64, target: &mut impl Target) -> Option<End> {
        let Some((last, last_fingerprint)) = self.last_block else {
            // The power-on state, before the first instruction of either side.
            self.last_block = Some((qemu, fingerprint));
            if let Some(end) = self.compare(qemu, target, None) {
                return Some(end);
            }
            return self.execute(target);
        };
        // A block printed again without the instruction having executed: QEMU logs a block on
        // every entry, and re-enters one when its instruction budget runs out (oracle §1c
        // pitfall 1). Only a branch to itself executes and leaves the state unchanged.
        if fingerprint == last_fingerprint && qemu == last && self.trap.is_none() {
            let self_loop = self
                .executed
                .as_ref()
                .and_then(|stepped| stepped.executed.as_ref())
                .and_then(|executed| executed.instruction)
                .is_some_and(branches_to_itself);
            if !self_loop {
                self.counts.duplicates += 1;
                return None;
            }
        }
        self.last_block = Some((qemu, fingerprint));
        // QEMU resets the whole machine, back to the first instruction of the ROM; the machine
        // counts its resets. Either ends the window (H4).
        let qemu_reset = qemu.pc() == RESET_PC;
        let inkemu_reset = target.resets() != self.resets;
        if qemu_reset || inkemu_reset {
            return Some(End::Window(WindowEnd::Reset {
                qemu: qemu_reset,
                inkemu: inkemu_reset,
            }));
        }
        let stepped = self.executed.take();
        let executed = stepped.as_ref().and_then(|s| s.executed.clone());
        if let Some(executed) = &executed
            && let Some((block, address)) = self.unemulated_access(executed)
        {
            return Some(End::Window(WindowEnd::Unemulated {
                block: block.name,
                address,
                pc: executed.pc,
            }));
        }
        let injected = executed
            .as_ref()
            .and_then(|executed| self.inject(executed, &qemu, target));
        let interrupt = stepped.as_ref().and_then(|s| s.interrupt);
        if interrupt.is_some() {
            self.counts.interrupts += 1;
        }
        let entry = HistoryEntry {
            index: self.counts.compared,
            pc: executed.as_ref().map_or(0, |e| e.pc),
            instruction: executed.as_ref().and_then(|e| e.instruction).map(|d| d.raw),
            interrupt,
            injected,
        };
        if let Some(end) = self.compare(qemu, target, Some((entry, executed))) {
            return Some(end);
        }
        self.counts.compared += 1;
        if self
            .limit
            .is_some_and(|limit| self.counts.compared >= limit)
        {
            return Some(End::Window(WindowEnd::Limit));
        }
        self.execute(target)
    }

    /// Executes the next instruction of the machine.
    fn execute(&mut self, target: &mut impl Target) -> Option<End> {
        self.before = target.state();
        let stepped = target.step();
        if let Some(reason) = stepped.stop.clone() {
            return Some(End::Stopped {
                reason,
                pc: self.before.pc(),
            });
        }
        self.executed = Some(stepped);
        None
    }

    /// The block the data access of `executed` reached, when it is one QEMU does not emulate and
    /// no row covers the address (R6 of the plan: such a row is counted instead).
    fn unemulated_access(&mut self, executed: &Executed) -> Option<(&'static Unemulated, u32)> {
        let address = data_address(executed.instruction?, &self.before)?;
        let block = unemulated(address)?;
        match self.covering(address) {
            Some(row) => {
                *self.counts.rows.entry(row).or_default() += 1;
                None
            }
            None => Some((block, address)),
        }
    }

    /// Treatment (ii): the register a load from a row's range, or a read of a row's CSR, wrote
    /// takes in the machine the value it has in QEMU. Returns the row and the register when a
    /// value was copied.
    fn inject(
        &mut self,
        executed: &Executed,
        qemu: &CpuState,
        target: &mut impl Target,
    ) -> Option<(u16, u8, u32)> {
        if executed.trap.is_some() {
            return None;
        }
        let decoded = executed.instruction?;
        let (rd, number, bits) = match decoded.instr {
            Instr::Load { rd, .. } => {
                let address = data_address(decoded, &self.before)?;
                self.rows.iter().find_map(|row| {
                    row.treatments
                        .iter()
                        .find_map(|treatment| match *treatment {
                            Treatment::Inject { first, last, bits }
                                if (first..=last).contains(&address) =>
                            {
                                Some((rd, row.number, bits))
                            }
                            _ => None,
                        })
                })?
            }
            Instr::Csr { rd, csr, .. } => self.rows.iter().find_map(|row| {
                row.treatments
                    .iter()
                    .find_map(|treatment| match *treatment {
                        Treatment::InjectCsr { csrs, bits } if csrs.contains(&csr) => {
                            Some((rd, row.number, bits))
                        }
                        _ => None,
                    })
            })?,
            _ => return None,
        };
        if rd == 0 {
            return None;
        }
        let mine = target.state().x(rd);
        let value = (mine & !bits) | (qemu.x(rd) & bits);
        if mine == value {
            return None;
        }
        target.set_x(rd, value);
        *self.counts.rows.entry(number).or_default() += 1;
        Some((number, rd, value))
    }

    /// The row of treatment (ii) that covers `address`, if one does.
    fn covering(&self, address: u32) -> Option<u16> {
        self.rows.iter().find_map(|row| {
            row.treatments
                .iter()
                .any(|treatment| {
                    matches!(*treatment, Treatment::Inject { first, last, .. } if (first..=last).contains(&address))
                })
                .then_some(row.number)
        })
    }

    /// Compares the machine's state with QEMU's under the masks of the table.
    fn compare(
        &mut self,
        qemu: CpuState,
        target: &impl Target,
        step: Option<(HistoryEntry, Option<Executed>)>,
    ) -> Option<End> {
        let inkemu = target.state();
        let mut differing = Vec::new();
        for field in Field::all() {
            let (q, i) = (qemu.get(field), inkemu.get(field));
            if q == i {
                continue;
            }
            let mut masked = 0;
            for row in self.rows {
                for treatment in row.treatments {
                    if let Treatment::Mask { field: f, bits } = *treatment
                        && f == field
                        && (q ^ i) & bits != 0
                    {
                        masked |= bits;
                        *self.counts.rows.entry(row.number).or_default() += 1;
                    }
                }
            }
            if (q ^ i) & !masked != 0 {
                differing.push(field);
            }
        }
        let (entry, executed) = match step {
            Some((entry, executed)) => (Some(entry), executed),
            None => (None, None),
        };
        if differing.is_empty() {
            if let Some(entry) = entry {
                if self.history.len() == HISTORY {
                    self.history.pop_front();
                }
                self.history.push_back(entry);
            }
            self.trap = None;
            return None;
        }
        let nearest = self.nearest(&differing, executed.as_ref());
        Some(End::Divergence(Box::new(Report {
            index: self.counts.compared,
            executed,
            interrupt: entry.as_ref().and_then(|e| e.interrupt),
            qemu_trap: self.trap,
            before: self.before,
            qemu,
            inkemu,
            differing,
            history: self.history.iter().copied().collect(),
            nearest,
        })))
    }

    /// The row that comes nearest to applying, for the report (H8): a mask of a quantity that
    /// differs, or an injection whose range is in the same 4 KiB page as the load.
    fn nearest(&self, differing: &[Field], executed: Option<&Executed>) -> Option<Nearest> {
        let address = executed
            .and_then(|e| e.instruction)
            .and_then(|decoded| data_address(decoded, &self.before));
        let csr = executed
            .and_then(|e| e.instruction)
            .and_then(|decoded| match decoded.instr {
                Instr::Csr { csr, .. } => Some(csr),
                _ => None,
            });
        self.rows.iter().find_map(|row| {
            row.treatments.iter().find_map(|treatment| match *treatment {
                Treatment::Mask { field, bits } if differing.contains(&field) => Some(Nearest {
                    row: row.number,
                    why: format!("masks bits {bits:#010x} of {field}, which differ outside them"),
                }),
                Treatment::Inject { first, last, bits } => address
                    .filter(|a| (first & !0xFFF..=last | 0xFFF).contains(a))
                    .map(|a| Nearest {
                        row: row.number,
                        why: format!(
                            "copies bits {bits:#010x} of loads from {first:#010x}..={last:#010x}; the instruction accessed {a:#010x}"
                        ),
                    }),
                Treatment::InjectCsr { csrs, bits } if csr.is_some_and(|c| csrs.contains(&c)) => {
                    Some(Nearest {
                        row: row.number,
                        why: format!("copies bits {bits:#010x} of reads of this CSR, which differ outside them"),
                    })
                }
                _ => None,
            })
        })
    }
}

/// Whether the instruction is a branch or jump to itself, the one instruction that leaves the
/// state it found.
fn branches_to_itself(decoded: Decoded) -> bool {
    matches!(
        decoded.instr,
        Instr::Jal { rd: 0, offset: 0 } | Instr::Branch { offset: 0, .. }
    )
}

/// The address a load or store accessed, from the registers it started with.
fn data_address(decoded: Decoded, before: &CpuState) -> Option<u32> {
    match decoded.instr {
        Instr::Load { rs1, offset, .. } | Instr::Store { rs1, offset, .. } => {
            Some(before.x(rs1).wrapping_add_signed(offset))
        }
        _ => None,
    }
}
