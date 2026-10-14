//! The comparison itself: QEMU's states, read from its trace, against the machine's, one
//! instruction at a time, under the policy of the table.

use crate::report::{Adoption, Cause, HistoryEntry, Nearest, Report};
use crate::state::{CpuState, Field};
use crate::table::{
    CPU_INT_THRESH, DIFFERENCES, Difference, Location, MEIP, MIP, Register, SPI1_CMD, SPI1_CMD_USR,
    SPI1_USER2, Treatment, absent_in_qemu,
};
use crate::trace::{QemuTrap, TraceEvent};
use crate::usage::Usage;
use cpu_rv32::{CsrOp, Decoded, Instr, LoadOp, StoreOp};
use soc_esp32c3::{CompatActs, Esp32C3, Executed, Stepped, StopReason};
use std::collections::{BTreeSet, VecDeque};

/// The instructions a report shows before the divergence, from both sides (spec §4.4).
pub const HISTORY: usize = 64;

/// The address of the first instruction of the ROM, where both sides start after any reset.
pub const RESET_PC: u32 = 0x4000_0000;

/// Identical blocks in a row the comparison drops as reprints. QEMU prints a block again, once,
/// when its instruction budget runs out at its entry (oracle §1c pitfall 1); the window of
/// `flash_hello.bin` never shows two in a row. A second identical block is compared as an
/// instruction executed: a QEMU spinning where inkemu does not then diverges.
pub const MAX_REPRINTS: u32 = 1;

/// Calls of [`Esp32C3::step`] the comparison makes to reach one boundary when the machine sleeps
/// long ([`StopReason::Asleep`] after each second of device time): an hour of device time.
pub const MAX_SLEEP_SLICES: u32 = 3_600;

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
    /// Whether QEMU's interrupt matrix would raise `mip.MEIP` in the state of the machine's
    /// controller ([`Esp32C3::external_interrupt_as_qemu`]).
    fn external_interrupt_as_qemu(&self) -> bool;
    /// What the settings of `QemuCompat` did ([`Esp32C3::compat_acts`]).
    fn compat_acts(&self) -> CompatActs;
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

    fn external_interrupt_as_qemu(&self) -> bool {
        Esp32C3::external_interrupt_as_qemu(self)
    }

    fn compat_acts(&self) -> CompatActs {
        Esp32C3::compat_acts(self)
    }
}

/// Why the comparison ended without a divergence (decision H4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowEnd {
    /// Both sides reset after the same instruction: QEMU went back to the first instruction of
    /// the ROM (it resets the whole machine) and the machine counted a reset.
    Reset,
    /// The instruction at `pc` accessed `address`, a register QEMU does not have that no row
    /// names.
    Absent { address: u32, pc: u32 },
    /// The number of instructions asked for were compared.
    Limit,
    /// QEMU's trace ended.
    TraceEnd,
}

impl std::fmt::Display for WindowEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowEnd::Reset => f.write_str("reset (QEMU and inkemu)"),
            WindowEnd::Absent { address, pc } => write!(
                f,
                "access to {address:#010x}, a register QEMU does not have, at pc {pc:#010x}"
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
    /// Blocks of the trace dropped as reprints (oracle §1c pitfall 1).
    pub duplicates: u64,
    /// Blocks dropped because an interrupt was taken at their entry ([`Lockstep::feed`]).
    pub preempted: u64,
    /// Interrupts both sides took.
    pub interrupts: u64,
}

/// One comparison, fed QEMU's trace event by event.
#[derive(Debug)]
pub struct Lockstep {
    rows: &'static [Difference],
    limit: Option<u64>,
    counts: Counts,
    usage: Usage,
    /// The last block compared: its state and fingerprint.
    last_block: Option<(CpuState, u64)>,
    /// Identical blocks dropped in a row so far.
    reprints: u32,
    /// The machine's state at the last boundary compared, and what it then executed.
    before: CpuState,
    executed: Option<Stepped>,
    /// The trap QEMU took before the next block.
    trap: Option<QemuTrap>,
    /// The last block read, not yet compared ([`Lockstep::feed`]).
    held: Option<(CpuState, u64)>,
    resets: u64,
    history: VecDeque<HistoryEntry>,
    /// The CSRs an instruction has written since power-on, for the pairs that hold until then.
    written: BTreeSet<u16>,
    /// Compared blocks left in which QEMU may still show `MEIP` after a threshold store.
    meip_lag: u8,
    /// `SPI1.USER2` as the last word store left it, and the opcode of the last user command.
    spi1_user2: u32,
    spi1_command: Option<u8>,
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
        Lockstep {
            rows,
            limit,
            counts: Counts::default(),
            usage: Usage::default(),
            last_block: None,
            reprints: 0,
            before: target.state(),
            executed: None,
            trap: None,
            held: None,
            resets: target.resets(),
            history: VecDeque::with_capacity(HISTORY),
            written: BTreeSet::new(),
            meip_lag: 0,
            spi1_user2: 0,
            spi1_command: None,
        }
    }

    /// The counts so far.
    #[must_use]
    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    /// What each row did so far, the rows of treatment (iv) with what their settings did in
    /// `target`.
    #[must_use]
    pub fn usage(&self, target: &impl Target) -> Usage {
        let mut usage = self.usage.clone();
        usage.record_compat(self.rows, &target.compat_acts());
        usage
    }

    /// Takes the next event of the trace; `Some` when the comparison ends with it.
    ///
    /// A block is held until the event after it is known: QEMU logs a block when it enters it,
    /// and an interrupt that became pending meanwhile is taken at that entry, before the
    /// instruction executes. Such a block is followed by the interrupt with `epc` its own PC: it
    /// is the state the previous instruction left, which the machine, taking the interrupt as
    /// part of that instruction's boundary, never shows. It is dropped and counted; the state it
    /// held is compared through the handler's first block, `mepc` and `mcause` included.
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
        if self.is_reprint(qemu, fingerprint, last, last_fingerprint) {
            self.reprints += 1;
            self.counts.duplicates += 1;
            return None;
        }
        self.reprints = 0;
        self.last_block = Some((qemu, fingerprint));
        let stepped = self.executed.take();
        let executed = stepped.as_ref().and_then(|s| s.executed.clone());
        let interrupt = stepped.as_ref().and_then(|s| s.interrupt);
        // QEMU resets the whole machine, back to the first instruction of the ROM; the machine
        // counts its resets. Both together end the window (H4); one alone is a divergence.
        let qemu_reset = qemu.pc() == RESET_PC;
        let inkemu_reset = target.resets() != self.resets;
        if qemu_reset && inkemu_reset {
            return Some(End::Window(WindowEnd::Reset));
        }
        if qemu_reset || inkemu_reset {
            let inkemu = target.state();
            let differing = Field::all()
                .filter(|&field| qemu.get(field) != inkemu.get(field))
                .collect();
            return Some(self.divergence(
                Cause::OneSidedReset { qemu: qemu_reset },
                qemu,
                inkemu,
                differing,
                executed,
                interrupt,
            ));
        }
        let mut adopted = None;
        if let Some(executed) = &executed {
            if let Some(end) = self.access(executed) {
                return Some(end);
            }
            adopted = self.adopt(executed, &qemu, target);
            self.note_effects(executed);
        }
        if interrupt.is_some() {
            self.counts.interrupts += 1;
        }
        let entry = HistoryEntry {
            index: self.counts.compared,
            pc: executed.as_ref().map_or(0, |e| e.pc),
            instruction: executed.as_ref().and_then(|e| e.instruction).map(|d| d.raw),
            interrupt,
            adopted,
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

    /// Whether the block is QEMU printing the last one again without having executed it: the
    /// same state and fingerprint, no trap between, an instruction that does not branch to
    /// itself from that state, and fewer than [`MAX_REPRINTS`] dropped in a row.
    fn is_reprint(
        &self,
        qemu: CpuState,
        fingerprint: u64,
        last: CpuState,
        last_fingerprint: u64,
    ) -> bool {
        if fingerprint != last_fingerprint || qemu != last || self.trap.is_some() {
            return false;
        }
        if self.reprints >= MAX_REPRINTS {
            return false;
        }
        // The instruction at that PC is the one the machine executed after the last block; its
        // bits, read from memory, are judged on QEMU's own state, not on the machine's decoder.
        let raw = self
            .executed
            .as_ref()
            .and_then(|stepped| stepped.executed.as_ref())
            .filter(|executed| executed.pc == last.pc())
            .and_then(|executed| executed.instruction)
            .map(|decoded| decoded.raw);
        !raw.is_some_and(|raw| loops_on_itself(raw, &last))
    }

    /// Executes the next instruction of the machine, through the boundaries a long sleep cuts
    /// into calls ([`Esp32C3::step`]).
    fn execute(&mut self, target: &mut impl Target) -> Option<End> {
        self.before = target.state();
        let mut stepped = target.step();
        let mut slices = 0;
        while matches!(
            stepped.stop,
            Some(StopReason::Asleep | StopReason::ConsoleOutput)
        ) && slices < MAX_SLEEP_SLICES
        {
            let more = target.step();
            stepped = Stepped {
                executed: stepped.executed.or(more.executed),
                interrupt: more.interrupt.or(stepped.interrupt),
                slept: stepped.slept || more.slept,
                stop: more.stop,
            };
            slices += 1;
        }
        if let Some(reason) = stepped.stop.clone() {
            return Some(End::Stopped {
                reason,
                pc: self.before.pc(),
            });
        }
        self.executed = Some(stepped);
        None
    }

    /// An access of `executed` to a register QEMU does not have ends the window, unless a row
    /// names that register (H4, with the correction of 2026-10-14 to the design): the row is then
    /// used, whatever it copies.
    fn access(&mut self, executed: &Executed) -> Option<End> {
        let address = data_address(executed.instruction?, &self.before)?;
        if !absent_in_qemu(address) {
            return None;
        }
        let word = address & !3;
        let Some(row) = self
            .rows
            .iter()
            .find(|row| row.registers_at(word).next().is_some())
        else {
            return Some(End::Window(WindowEnd::Absent {
                address,
                pc: executed.pc,
            }));
        };
        // A load that copies bits records its use with them ([`Lockstep::adopt`]).
        if !Self::copies(executed) {
            self.usage.record(row.number, Location::Mmio(word), 0, 0);
        }
        None
    }

    /// Whether a word load of `executed` would be handled by [`Lockstep::adopt`].
    fn copies(executed: &Executed) -> bool {
        executed.trap.is_none()
            && matches!(
                executed.instruction.map(|d| d.instr),
                Some(Instr::Load {
                    op: LoadOp::W,
                    rd: 1..,
                    ..
                })
            )
    }

    /// Treatments (i) and (ii) at a read: the register an instruction loaded from a row's
    /// register, a counter, or a CSR held to a pair takes QEMU's bits in the machine, within what
    /// the row declares. Returns what was taken.
    fn adopt(
        &mut self,
        executed: &Executed,
        qemu: &CpuState,
        target: &mut impl Target,
    ) -> Option<Adoption> {
        if executed.trap.is_some() {
            return None;
        }
        let decoded = executed.instruction?;
        let (rd, location) = match decoded.instr {
            Instr::Load {
                op: LoadOp::W, rd, ..
            } => (rd, Location::Mmio(data_address(decoded, &self.before)?)),
            Instr::Csr { rd, csr, op, .. } if op != CsrOp::Rw || rd != 0 => {
                (rd, Location::Csr(csr))
            }
            _ => return None,
        };
        if rd == 0 {
            return None;
        }
        let mine = target.state().x(rd);
        let theirs = qemu.x(rd);
        let (row, bits, gap) = self.admitted_read(location, mine, theirs)?;
        if let Location::Mmio(address) = location {
            // An admitted access to a register QEMU lacks is a use even when nothing differs.
            if bits == 0 && absent_in_qemu(address) {
                self.usage.record(row, location, 0, 0);
            }
        }
        if bits == 0 {
            return None;
        }
        target.set_x(rd, mine ^ bits);
        self.usage.record(row, location, bits, gap);
        Some(Adoption {
            row,
            register: rd,
            inkemu: mine,
            qemu: theirs,
        })
    }

    /// The row that admits the read of `location` giving `mine` in inkemu and `theirs` in QEMU,
    /// with the bits to take from QEMU and, for a counter, the gap.
    fn admitted_read(&self, location: Location, mine: u32, theirs: u32) -> Option<(u16, u32, u32)> {
        let differ = mine ^ theirs;
        for row in self.rows {
            for treatment in row.treatments {
                match (*treatment, location) {
                    (Treatment::Registers(registers), Location::Mmio(address)) => {
                        if let Some(register) = registers
                            .iter()
                            .find(|r| r.address == address && self.condition_holds(r))
                        {
                            let qemu_ok = register.qemu.is_none_or(|value| value == theirs);
                            if differ & !register.mask == 0 && qemu_ok {
                                return Some((row.number, differ, 0));
                            }
                            return None;
                        }
                    }
                    (Treatment::Counter { location: l, bound }, _) if l == location => {
                        let gap = theirs.wrapping_sub(mine).cast_signed().unsigned_abs();
                        return (gap <= bound).then_some((row.number, differ, gap));
                    }
                    (
                        Treatment::Pair {
                            field,
                            bits,
                            qemu,
                            inkemu,
                            until_written,
                        },
                        Location::Csr(csr),
                    ) if field.csr() == Some(csr)
                        && !(until_written && self.written.contains(&csr)) =>
                    {
                        let held =
                            differ & !bits == 0 && theirs & bits == qemu && mine & bits == inkemu;
                        return held.then_some((row.number, differ, 0));
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// Whether the condition a register entry carries holds now.
    fn condition_holds(&self, register: &Register) -> bool {
        register
            .after_command
            .is_none_or(|command| self.spi1_command == Some(command))
    }

    /// What an executed instruction leaves for the rows that follow it: the CSRs it wrote, the
    /// SPI1 command it started, a threshold store that QEMU's `MEIP` follows late.
    fn note_effects(&mut self, executed: &Executed) {
        let Some(decoded) = executed.instruction else {
            return;
        };
        match decoded.instr {
            Instr::Csr { op, src, csr, .. } if op == CsrOp::Rw || src != 0 => {
                self.written.insert(csr);
            }
            Instr::Store {
                op: StoreOp::W,
                rs2,
                ..
            } => {
                let Some(address) = data_address(decoded, &self.before) else {
                    return;
                };
                let value = self.before.x(rs2);
                match address {
                    SPI1_USER2 => self.spi1_user2 = value,
                    SPI1_CMD if value & SPI1_CMD_USR != 0 => {
                        self.spi1_command = Some(self.spi1_user2.to_le_bytes()[0]);
                    }
                    CPU_INT_THRESH => {
                        for row in self.rows {
                            for treatment in row.treatments {
                                if let Treatment::ExternalInterrupt { lag } = *treatment {
                                    self.meip_lag = lag;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Compares the machine's state with QEMU's under the rows of treatment (i).
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
            let admitted = if field == Field::Mip {
                self.admitted_mip(q, i, target)
            } else if q == i {
                Some(0)
            } else {
                self.admitted_pair(field, q, i)
            };
            if admitted.is_none() {
                differing.push(field);
            }
        }
        self.meip_lag = self.meip_lag.saturating_sub(1);
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
        let interrupt = entry.as_ref().and_then(|e| e.interrupt);
        Some(self.divergence(Cause::State, qemu, inkemu, differing, executed, interrupt))
    }

    /// A difference of a compared CSR that a pair admits, recorded; `None` when none does.
    fn admitted_pair(&mut self, field: Field, q: u32, i: u32) -> Option<u32> {
        let csr = field.csr()?;
        let (row, bits, _) = self.admitted_read(Location::Csr(csr), i, q)?;
        self.usage.record(row, Location::Csr(csr), bits, 0);
        Some(bits)
    }

    /// `mip`: with a row of `MEIP`, inkemu shows no `MEIP` and QEMU's is the line QEMU's matrix
    /// computes on inkemu's controller (or late after a threshold store); the other bits are
    /// compared. `None` when the two disagree.
    fn admitted_mip(&mut self, q: u32, i: u32, target: &impl Target) -> Option<u32> {
        let row = self.rows.iter().find(|row| {
            row.treatments
                .iter()
                .any(|t| matches!(t, Treatment::ExternalInterrupt { .. }))
        });
        let Some(row) = row else {
            return (q == i).then_some(0);
        };
        if (q ^ i) & !MEIP != 0 || i & MEIP != 0 {
            return None;
        }
        let shown = q & MEIP != 0;
        let expected = target.external_interrupt_as_qemu();
        let late = shown && !expected && self.meip_lag > 0;
        if shown != expected && !late {
            return None;
        }
        if shown {
            self.usage.record(row.number, Location::Csr(MIP), MEIP, 0);
        }
        Some(q ^ i)
    }

    fn divergence(
        &self,
        cause: Cause,
        qemu: CpuState,
        inkemu: CpuState,
        differing: Vec<Field>,
        executed: Option<Executed>,
        interrupt: Option<u32>,
    ) -> End {
        let nearest = self.nearest(&differing, executed.as_ref());
        End::Divergence(Box::new(Report {
            cause,
            index: self.counts.compared,
            executed,
            interrupt,
            qemu_trap: self.trap,
            before: self.before,
            qemu,
            inkemu,
            differing,
            history: self.history.iter().copied().collect(),
            nearest,
        }))
    }

    /// The row that comes nearest to applying, for the report (H8): one that names the
    /// register the instruction accessed, then one that holds the CSR it read or a quantity
    /// that differs, then one with a register in the same 4 KiB page.
    fn nearest(&self, differing: &[Field], executed: Option<&Executed>) -> Option<Nearest> {
        let decoded = executed.and_then(|e| e.instruction);
        let address = decoded
            .and_then(|decoded| data_address(decoded, &self.before))
            .map(|a| a & !3);
        let csr = decoded.and_then(|decoded| match decoded.instr {
            Instr::Csr { csr, .. } => Some(csr),
            _ => None,
        });
        let fields: Vec<Location> = differing
            .iter()
            .filter_map(|f| f.csr())
            .chain(csr)
            .map(Location::Csr)
            .collect();
        let exact = address.and_then(|a| {
            self.rows.iter().find_map(|row| {
                let masks: Vec<u32> = row.registers_at(a).map(|r| r.mask).collect();
                let counter = row.bound(Location::Mmio(a));
                if let Some(bound) = counter {
                    return Some(Nearest {
                        row: row.number,
                        why: format!("admits a gap of {bound} at {a:#010x}, which the read passed"),
                    });
                }
                (!masks.is_empty()).then(|| Nearest {
                    row: row.number,
                    why: format!(
                        "takes bits {:#010x} of {a:#010x} from QEMU (under its conditions); the \
                         read differs outside them or not as the row says",
                        masks.iter().fold(0, |m, b| m | b)
                    ),
                })
            })
        });
        let held = || {
            self.rows.iter().find_map(|row| {
                fields.iter().find_map(|&location| {
                    row.declared(location).map(|bits| Nearest {
                        row: row.number,
                        why: format!(
                            "admits bits {bits:#010x} of {location}, which differ otherwise"
                        ),
                    })
                })
            })
        };
        let page = || {
            let a = address?;
            self.rows.iter().find_map(|row| {
                row.treatments.iter().find_map(|treatment| match treatment {
                    Treatment::Registers(registers) => registers
                        .iter()
                        .find(|r| r.address & !0xFFF == a & !0xFFF)
                        .map(|r| Nearest {
                            row: row.number,
                            why: format!(
                                "names {:#010x}, in the page of {a:#010x}, which it does not name",
                                r.address
                            ),
                        }),
                    _ => None,
                })
            })
        };
        exact.or_else(held).or_else(page)
    }
}

/// Whether the instruction `raw`, executed from `state`, leaves it unchanged: a branch or jump
/// to itself whose link register, if any, already holds the return address. Judged on the bits
/// and on QEMU's state, independently of the machine's decoder.
#[must_use]
pub fn loops_on_itself(raw: u32, state: &CpuState) -> bool {
    let pc = state.pc();
    let x = |reg: u32| state.x(u8::try_from(reg & 31).unwrap_or(0));
    let links = |rd: u32, size: u32| rd == 0 || x(rd) == pc.wrapping_add(size);
    if raw & 0b11 == 0b11 {
        let rd = (raw >> 7) & 31;
        let rs1 = (raw >> 15) & 31;
        let rs2 = (raw >> 20) & 31;
        let funct3 = (raw >> 12) & 7;
        match raw & 0x7F {
            // jal rd, 0
            0x6F => raw & 0xFFFF_F000 == 0 && links(rd, 4),
            // jalr rd, imm(rs1) landing on itself
            0x67 => {
                let target = x(rs1).wrapping_add_signed(raw.cast_signed() >> 20) & !1;
                funct3 == 0 && target == pc && links(rd, 4) && (rd != rs1 || rd == 0)
            }
            // a branch with offset 0, taken from this state
            0x63 => {
                let (a, b) = (x(rs1), x(rs2));
                raw & 0xFE00_0F80 == 0
                    && match funct3 {
                        0 => a == b,
                        1 => a != b,
                        4 => a.cast_signed() < b.cast_signed(),
                        5 => a.cast_signed() >= b.cast_signed(),
                        6 => a < b,
                        7 => a >= b,
                        _ => false,
                    }
            }
            _ => false,
        }
    } else {
        let raw = raw & 0xFFFF;
        let funct3 = raw >> 13;
        match (raw & 0b11, funct3) {
            // c.j 0 and c.jal 0
            (0b01, 0b101) => raw & 0x1FFC == 0,
            (0b01, 0b001) => raw & 0x1FFC == 0 && links(1, 2),
            // c.beqz / c.bnez with offset 0
            (0b01, 0b110 | 0b111) => {
                let value = x(8 + ((raw >> 7) & 7));
                raw & 0x1C7C == 0 && ((funct3 == 0b110) == (value == 0))
            }
            // c.jr / c.jalr to itself
            (0b10, 0b100) if raw & 0x7C == 0 && (raw >> 7) & 31 != 0 => {
                let rs1 = (raw >> 7) & 31;
                let link = (raw >> 12) & 1 == 1;
                x(rs1) & !1 == pc && (!link || (rs1 != 1 && links(1, 2)))
            }
            _ => false,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state(pc: u32, regs: &[(u8, u32)]) -> CpuState {
        let mut x = [0; 32];
        for &(reg, value) in regs {
            x[usize::from(reg)] = value;
        }
        CpuState::new(pc, x, |_| Some(0))
    }

    #[test]
    fn a_jump_or_branch_to_itself_is_recognised_on_its_bits() {
        let s = state(0x4000_1000, &[(5, 3), (6, 3), (8, 0), (10, 0x4000_1000)]);
        assert!(loops_on_itself(0x0000_006F, &s), "j .");
        assert!(loops_on_itself(0x0062_8063, &s), "beq t0, t1, .");
        assert!(
            !loops_on_itself(0x0062_9063, &s),
            "bne t0, t1, . is not taken"
        );
        assert!(loops_on_itself(0xA001, &s), "c.j .");
        assert!(loops_on_itself(0xC001, &s), "c.beqz s0, .");
        assert!(!loops_on_itself(0xE001, &s), "c.bnez s0, . is not taken");
        assert!(loops_on_itself(0x0005_0067, &s), "jr a0 with a0 = pc");
        assert!(loops_on_itself(0x8502, &s), "c.jr a0 with a0 = pc");
        assert!(!loops_on_itself(0x0000_0013, &s), "nop");
        assert!(!loops_on_itself(0x0040_006F, &s), "j .+4");
        // jal ra, . leaves ra = pc + 4: it loops from its second execution on.
        assert!(!loops_on_itself(0x0000_00EF, &s));
        assert!(loops_on_itself(0x0000_00EF, &state(0x100, &[(1, 0x104)])));
    }
}
