//! The comparison itself: QEMU's states, read from its trace, against the machine's, one
//! instruction at a time, under the policy of the table.

use crate::report::{Adoption, Cause, HistoryEntry, Nearest, Report};
use crate::state::{CpuState, Field};
use crate::table::{
    AfterReset, CPU_INT_THRESH, Difference, Drift, Location, MEIP, MIP, MODULE_RESETS,
    PERIP_RST_EN_AT_POWER_ON, Register, SPI1_CMD, SPI1_CMD_USR, SPI1_USER2, SYSTEM_PERIP_RST_EN0,
    SYSTEM_PERIP_RST_EN1, Site, Treatment, absent_in_qemu,
};
use crate::trace::{QemuTrap, TraceEvent};
use crate::usage::{Check, Usage};
use crate::window::Window;
use cpu_rv32::{CsrOp, Decoded, Instr, LoadOp, StoreOp};
use soc_esp32c3::memmap::{PERIPH_BASE, PERIPH_SIZE};
use soc_esp32c3::{CompatActs, Esp32C3, Executed, Stepped, StopReason};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The peripheral space of the machine's address map: the only words an entry of
/// [`Treatment::RegistersAfterReset`] names, whose stores the comparison notes.
const PERIPHERALS: std::ops::Range<u32> = PERIPH_BASE..PERIPH_BASE + PERIPH_SIZE;

/// Nanoseconds in a second, for the slow clock's cycles at an instant of device time.
const NANOS_PER_SECOND: u128 = 1_000_000_000;

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
    /// Device time since power-on ([`Esp32C3::device_time_nanos`]).
    fn device_time_nanos(&self) -> u64;
    /// A CSR, compared or not ([`Esp32C3::csr`]); `None` when the core has none at `address`.
    fn csr(&self, address: u16) -> Option<u32>;
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

    fn device_time_nanos(&self) -> u64 {
        Esp32C3::device_time_nanos(self)
    }

    fn csr(&self, address: u16) -> Option<u32> {
        Esp32C3::csr(self, address)
    }
}

/// Where a counter of [`Treatment::Counter`] stands.
#[derive(Clone, Copy, Debug, Default)]
struct CounterState {
    /// The gap (QEMU's value minus inkemu's) at the origin for [`Drift::Within`], at the last
    /// read for [`Drift::Truncated`]; `None` until the first read after an origin.
    gap: Option<i64>,
    /// The updates QEMU may have made of its counter since the last read
    /// ([`Drift::Truncated`]).
    updates: u64,
}

/// A quantity [`Treatment::KeptAtReset`] holds after a reset: the row, the bits held, and the two
/// values, QEMU's from before the reset and inkemu's reset value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    row: u16,
    bits: u32,
    qemu: u32,
    inkemu: u32,
}

impl Held {
    /// Whether the two sides show the held values in the held bits, and agree on every other.
    const fn holds(self, q: u32, i: u32) -> bool {
        (q ^ self.qemu) & self.bits == 0
            && (i ^ self.inkemu) & self.bits == 0
            && (q ^ i) & !self.bits == 0
    }
}

/// How a read of a location is held, by the kind of row that names it.
#[derive(Clone, Copy)]
enum ReadRule {
    Mip,
    Counter {
        row: u16,
        drift: Drift,
    },
    SlowClock {
        row: u16,
        capture: u32,
        hz: u64,
        bound: u32,
        qemu: u32,
    },
    KeptCsr {
        row: u16,
        csr: u16,
        kept: u32,
        mask: u32,
    },
    Other,
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
    /// The number of instructions asked for were compared (`--max-instructions`): a partial
    /// window.
    Limit,
    /// The window's own bound was reached ([`Window::bound`]): a whole window.
    Bound,
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
            WindowEnd::Bound => f.write_str("the bound of the window"),
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
    /// Resets both sides took after the same instruction and the comparison went on through.
    pub resets_crossed: u32,
}

/// One comparison, fed QEMU's trace event by event.
#[derive(Debug)]
pub struct Lockstep {
    rows: &'static [Difference],
    limit: Option<u64>,
    /// The window's own bound ([`Window::bound`]).
    bound: Option<u64>,
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
    /// The resets of both sides the comparison goes on through; the next one ends the window.
    resets_to_cross: u32,
    /// The quantities the last reset left different under a row of [`Treatment::KeptAtReset`].
    kept: BTreeMap<Field, Held>,
    /// The words of memory an instruction stored a held register to, with the same hold: a word
    /// load that gives both values back holds its destination in turn.
    kept_memory: BTreeMap<u32, Held>,
    /// The words of peripherals stored to since the last reset that reached them: an entry of
    /// [`Treatment::RegistersAfterReset`] holds only until its register is stored to.
    stored_since_reset: BTreeSet<u32>,
    /// The words of peripherals stored to since the last pulse of their module, which a reset of
    /// both sides does not clear: what the machine's module reset left is gone at such a store.
    stored_since_pulse: BTreeSet<u32>,
    /// What the last word stores left in `SYSTEM.PERIP_RST_EN0` and `PERIP_RST_EN1`, which a CPU
    /// reset keeps, to see the bits a store sets.
    perip_rst_en: [u32; 2],
    /// The modules of [`MODULE_RESETS`] the machine reset at a pulse QEMU ignores.
    modules_reset: BTreeSet<usize>,
    /// inkemu's value of each CSR of [`Treatment::CsrKeptAtReset`] before the last instruction
    /// executed, and before the last reset crossed.
    csrs_before_step: BTreeMap<u16, u32>,
    csrs_before_reset: BTreeMap<u16, u32>,
    history: VecDeque<HistoryEntry>,
    /// The CSRs an instruction has written since power-on, for the pairs that hold until then.
    written: BTreeSet<u16>,
    /// The bits of each CSR instructions have written since power-on or the last reset crossed:
    /// every bit for `csrrw` and `csrrwi`, the operand for `csrrs`, `csrrc` and their immediate
    /// forms (an operand of 0 writes nothing).
    bits_written: BTreeMap<u16, u32>,
    /// Compared blocks left in which QEMU may still show `MEIP` after a threshold store.
    meip_lag: u8,
    /// `SPI1.USER2` as the last word store left it, and the opcode of the last user command.
    spi1_user2: u32,
    spi1_command: Option<u8>,
    /// The counters of the table, by location: only those the rows name.
    counters: BTreeMap<Location, CounterState>,
    /// The device time of the last capture of each slow clock, by the address of its capture
    /// register: only those the rows name.
    captures: BTreeMap<u32, u64>,
}

impl Lockstep {
    /// A comparison of a machine at power-on under `rows`, ending at the first reset of both
    /// sides, or after `limit` compared instructions when one is given: for the tests of the
    /// policy, on traces written for them. A comparison of a firmware is a window's
    /// ([`Lockstep::for_window`]): a table belongs to a window, which it serves whole.
    #[must_use]
    pub fn with_rows(
        target: &impl Target,
        rows: &'static [Difference],
        limit: Option<u64>,
    ) -> Lockstep {
        Lockstep {
            rows,
            limit,
            bound: None,
            counts: Counts::default(),
            usage: Usage::default(),
            last_block: None,
            reprints: 0,
            before: target.state(),
            executed: None,
            trap: None,
            held: None,
            resets: target.resets(),
            resets_to_cross: 0,
            kept: BTreeMap::new(),
            kept_memory: BTreeMap::new(),
            stored_since_reset: BTreeSet::new(),
            stored_since_pulse: BTreeSet::new(),
            perip_rst_en: PERIP_RST_EN_AT_POWER_ON,
            modules_reset: BTreeSet::new(),
            csrs_before_step: BTreeMap::new(),
            csrs_before_reset: BTreeMap::new(),
            history: VecDeque::with_capacity(HISTORY),
            written: BTreeSet::new(),
            bits_written: BTreeMap::new(),
            meip_lag: 0,
            spi1_user2: 0,
            spi1_command: None,
            counters: BTreeMap::new(),
            captures: BTreeMap::new(),
        }
    }

    /// A comparison of a machine at power-on in `window` (SP1b-4d, decision J3): under its rows,
    /// going on through its resets, ending at its bound; `limit` may cut it shorter.
    #[must_use]
    pub fn for_window(target: &impl Target, window: &Window, limit: Option<u64>) -> Lockstep {
        let mut lockstep = Lockstep::with_rows(target, window.rows, limit);
        lockstep.resets_to_cross = window.resets;
        lockstep.bound = window.bound;
        lockstep
    }

    /// Holds what the run did to the table (decision H3, [`Usage::check`]) for a window that
    /// ended by `end`: a window cut at an instruction limit reports what it left unused without
    /// failing on it.
    #[must_use]
    pub fn check_usage(&self, target: &impl Target, end: &WindowEnd) -> Check {
        self.usage(target)
            .check(self.rows, !matches!(end, WindowEnd::Limit))
    }

    /// The counts so far.
    #[must_use]
    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    /// The word load the machine executed and the comparison has not yet compared: its
    /// destination register and the address it read. For the test of the tool, which changes
    /// what the load gave as a fault of a model would.
    #[must_use]
    pub fn pending_load(&self) -> Option<(u8, u32)> {
        let executed = self.executed.as_ref()?.executed.as_ref()?;
        if executed.trap.is_some() {
            return None;
        }
        let decoded = executed.instruction?;
        match decoded.instr {
            Instr::Load {
                op: LoadOp::W, rd, ..
            } if rd != 0 => Some((rd, data_address(decoded, &self.before)?)),
            _ => None,
        }
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
            if self.counts.resets_crossed >= self.resets_to_cross {
                return Some(End::Window(WindowEnd::Reset));
            }
            self.cross_reset(target, &qemu, &last, executed.as_ref());
        } else if qemu_reset || inkemu_reset {
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
            // The instruction that asked for the reset read its registers before it.
            if !(qemu_reset && inkemu_reset)
                && let Some(field) = self.read_while_kept(executed)
            {
                let inkemu = target.state();
                return Some(self.divergence(
                    Cause::ReadKeptAtReset { field },
                    qemu,
                    inkemu,
                    vec![field],
                    Some(executed.clone()),
                    interrupt,
                ));
            }
            if let Some(end) = self.access(executed) {
                return Some(end);
            }
            adopted = self.adopt(executed, &last, &qemu, target);
            self.note_effects(executed, &last, target);
            self.reload_kept(executed, &qemu, target);
        }
        if interrupt.is_some() {
            self.counts.interrupts += 1;
            // An alarm QEMU raised brought its counter up to date.
            self.note_counter_update(|_| true);
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
            .bound
            .is_some_and(|bound| self.counts.compared >= bound)
        {
            return Some(End::Window(WindowEnd::Bound));
        }
        if self
            .limit
            .is_some_and(|limit| self.counts.compared >= limit)
        {
            return Some(End::Window(WindowEnd::Limit));
        }
        self.execute(target)
    }

    /// Both sides reset after the same instruction and the comparison goes on. Nothing of the
    /// machine changes: the machine resets its CPU at the reset `esp_restart` asks for, as the
    /// chip does (TRM §6.1), and QEMU resets its machine; every difference that leaves is a row
    /// of the window's table, or a divergence. What the comparison knew of QEMU starts again as
    /// at power-on: no CSR and no register has been written since, the counters' gaps are fixed
    /// again by their next read (QEMU's counters go on through its reset, inkemu's restart: a gap
    /// is refixed, nothing is assumed of its origin), no SPI1 command has run and no threshold
    /// store is pending. The device time
    /// of a slow clock's capture stays: the RTC timer of the chip, and of the machine, goes on
    /// through a CPU reset, and QEMU has none.
    ///
    /// A quantity a row of [`Treatment::KeptAtReset`] names is held when it differs in the held
    /// bits, QEMU's value being the one it had before the reset (`last`, the state the
    /// instruction that asked for it started from, which that instruction did not write) and
    /// inkemu's the reset value the row declares.
    fn cross_reset(
        &mut self,
        target: &impl Target,
        qemu: &CpuState,
        last: &CpuState,
        executed: Option<&Executed>,
    ) {
        self.resets = target.resets();
        self.counts.resets_crossed += 1;
        self.written.clear();
        self.bits_written.clear();
        self.meip_lag = 0;
        self.spi1_user2 = 0;
        self.spi1_command = None;
        for counter in self.counters.values_mut() {
            *counter = CounterState::default();
        }
        self.kept.clear();
        self.kept_memory.clear();
        self.stored_since_reset.clear();
        self.csrs_before_reset = self.csrs_before_step.clone();
        let written = executed
            .filter(|executed| executed.trap.is_none())
            .and_then(|executed| executed.instruction)
            .map(|decoded| writes(decoded.instr))
            .unwrap_or_default();
        let inkemu = target.state();
        for row in self.rows {
            for treatment in row.treatments {
                let Treatment::KeptAtReset(quantities) = treatment else {
                    continue;
                };
                for kept in *quantities {
                    let field = kept.field;
                    let (q, i) = (qemu.get(field), inkemu.get(field));
                    let bits = kept.bits;
                    let differs_as_declared = (q ^ i) & bits != 0
                        && (q ^ last.get(field)) & bits == 0
                        && (i ^ kept.reset) & bits == 0
                        && !written.contains(&field);
                    if differs_as_declared {
                        self.kept.insert(
                            field,
                            Held {
                                row: row.number,
                                bits,
                                qemu: q,
                                inkemu: i,
                            },
                        );
                    }
                }
            }
        }
    }

    /// The first quantity held by [`Treatment::KeptAtReset`] that `executed` read, and releases
    /// those it wrote: from its write on, a quantity is compared again.
    ///
    /// A word store of a held register is not a read that computes anything: it carries the two
    /// values to memory ([`Lockstep::reload_kept`]), as a function's prologue saves a register it
    /// will restore. Any other store to a word that carried a pair ends it.
    fn read_while_kept(&mut self, executed: &Executed) -> Option<Field> {
        if self.kept.is_empty() && self.kept_memory.is_empty() {
            return None;
        }
        let decoded = executed.instruction?;
        let instr = decoded.instr;
        let held = |kept: &BTreeMap<Field, Held>, reg: u8| {
            (reg != 0 && kept.contains_key(&Field::X(reg))).then_some(Field::X(reg))
        };
        if let Instr::Store { op, rs1, rs2, .. } = instr {
            if let Some(field) = held(&self.kept, rs1) {
                return Some(field);
            }
            let address = data_address(decoded, &self.before)?;
            let carried = (op == StoreOp::W && address % 4 == 0)
                .then(|| self.kept.get(&Field::X(rs2)).copied())
                .flatten();
            if carried.is_none()
                && let Some(field) = held(&self.kept, rs2)
            {
                return Some(field);
            }
            if executed.trap.is_none() {
                let last = address.wrapping_add(store_bytes(op) - 1);
                self.kept_memory.remove(&(address & !3));
                self.kept_memory.remove(&(last & !3));
                if let Some(pair) = carried {
                    self.kept_memory.insert(address, pair);
                }
            }
            return None;
        }
        if let Some(field) = reads(instr)
            .into_iter()
            .find(|field| self.kept.contains_key(field))
        {
            return Some(field);
        }
        if executed.trap.is_none() {
            for field in writes(instr) {
                self.kept.remove(&field);
            }
        }
        None
    }

    /// A word load from a word that carries a held pair: when the two sides loaded exactly that
    /// pair, its destination is held in turn, with the row that held the register stored.
    fn reload_kept(&mut self, executed: &Executed, qemu: &CpuState, target: &impl Target) {
        if self.kept_memory.is_empty() || executed.trap.is_some() {
            return;
        }
        let Some(decoded) = executed.instruction else {
            return;
        };
        let Instr::Load {
            op: LoadOp::W, rd, ..
        } = decoded.instr
        else {
            return;
        };
        let Some(address) = data_address(decoded, &self.before) else {
            return;
        };
        if let Some(&hold) = self.kept_memory.get(&address)
            && rd != 0
            && hold.holds(qemu.x(rd), target.state().x(rd))
        {
            self.kept.insert(Field::X(rd), hold);
        }
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
        for row in self.rows {
            for treatment in row.treatments {
                if let Treatment::CsrKeptAtReset(csrs) = treatment {
                    for kept in *csrs {
                        let value = target.csr(kept.csr).unwrap_or(0);
                        self.csrs_before_step.insert(kept.csr, value);
                    }
                }
            }
        }
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
        let after_reset = self.after_reset_at(word);
        let Some(row) = self
            .rows
            .iter()
            .find(|row| row.registers_at(word, after_reset).next().is_some())
        else {
            return Some(End::Window(WindowEnd::Absent {
                address,
                pc: executed.pc,
            }));
        };
        // A load that copies bits records its use with them ([`Lockstep::adopt`]).
        if !Self::copies(executed) {
            let site = self.site_of(word, row.registers_at(word, after_reset));
            self.usage.record(row.number, site, 0, 0);
        }
        None
    }

    /// The site at `address` among the register `entries` a row has there: the entry whose
    /// condition holds now, or else the first.
    fn site_of<'a>(&self, address: u32, entries: impl Iterator<Item = &'a Register>) -> Site {
        let entries: Vec<&Register> = entries.collect();
        let entry = entries
            .iter()
            .find(|register| self.condition_holds(register))
            .or_else(|| entries.first());
        Site {
            location: Location::Mmio(address),
            after_command: entry.and_then(|register| register.after_command),
        }
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
    /// the row declares. `previous` is QEMU's state the instruction started from. Returns what
    /// was taken.
    fn adopt(
        &mut self,
        executed: &Executed,
        previous: &CpuState,
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
        let differ = mine ^ theirs;
        let (row, bits) = match self.read_rule(location) {
            ReadRule::Mip => {
                let (row, bits) = self.admitted_mip_read(mine, theirs, previous)?;
                if bits != 0 {
                    self.usage.record(row, Site::at(location), bits, 0);
                }
                (row, bits)
            }
            // A counter and a clock are held at every read, equal or not: each read is a use.
            ReadRule::Counter { row, drift } => {
                let moved = self.counter_read(location, drift, mine, theirs)?;
                self.usage.record(row, Site::at(location), differ, moved);
                (row, differ)
            }
            ReadRule::SlowClock {
                row,
                capture,
                hz,
                bound,
                qemu,
            } => {
                let moved = self.clock_read(capture, hz, bound, qemu, mine, theirs)?;
                self.usage.record(row, Site::at(location), differ, moved);
                (row, differ)
            }
            ReadRule::KeptCsr {
                row,
                csr,
                kept,
                mask,
            } => {
                let bits = self.kept_csr_read(csr, kept, mask, mine, theirs)?;
                if bits != 0 {
                    self.usage.record(row, Site::at(location), bits, 0);
                }
                (row, bits)
            }
            ReadRule::Other => {
                let taken = self.admitted_read(location, mine, theirs)?;
                // An admitted access to a register QEMU lacks is a use even when nothing
                // differs: of the first row that names it.
                let absent = matches!(location, Location::Mmio(address) if absent_in_qemu(address));
                for (i, &(row, site, bits)) in taken.iter().enumerate() {
                    if bits != 0 || (absent && differ == 0 && i == 0) {
                        self.usage.record(row, site, bits, 0);
                    }
                }
                let row = taken
                    .iter()
                    .find(|(_, _, bits)| *bits != 0)
                    .or(taken.first())
                    .map_or(0, |(row, _, _)| *row);
                (row, differ)
            }
        };
        if bits == 0 {
            return None;
        }
        target.set_x(rd, mine ^ bits);
        Some(Adoption {
            row,
            register: rd,
            inkemu: mine,
            qemu: theirs,
        })
    }

    /// How the rows hold a read of `location`.
    fn read_rule(&self, location: Location) -> ReadRule {
        if location == Location::Csr(MIP) {
            return ReadRule::Mip;
        }
        for row in self.rows {
            for treatment in row.treatments {
                match *treatment {
                    Treatment::Counter {
                        location: l, drift, ..
                    } if l == location => {
                        return ReadRule::Counter {
                            row: row.number,
                            drift,
                        };
                    }
                    Treatment::SlowClock {
                        register,
                        capture,
                        hz,
                        bound,
                        qemu,
                        ..
                    } if Location::Mmio(register) == location => {
                        return ReadRule::SlowClock {
                            row: row.number,
                            capture,
                            hz,
                            bound,
                            qemu,
                        };
                    }
                    Treatment::CsrKeptAtReset(csrs) => {
                        if let Some(kept) =
                            csrs.iter().find(|kept| Location::Csr(kept.csr) == location)
                        {
                            return ReadRule::KeptCsr {
                                row: row.number,
                                csr: kept.csr,
                                kept: kept.kept,
                                mask: kept.mask,
                            };
                        }
                    }
                    _ => {}
                }
            }
        }
        ReadRule::Other
    }

    /// A read of a counter: the gap between the two values may move only as `drift` says from
    /// where the counter's origin, or its last read, left it. Returns how far it moved, `None`
    /// when that is past what the row admits.
    fn counter_read(
        &mut self,
        location: Location,
        drift: Drift,
        mine: u32,
        theirs: u32,
    ) -> Option<u32> {
        let gap = i64::from(theirs.wrapping_sub(mine).cast_signed());
        let state = self.counters.entry(location).or_default();
        let Some(held) = state.gap else {
            // The first read after an origin fixes the gap.
            *state = CounterState {
                gap: Some(gap),
                updates: 0,
            };
            return Some(0);
        };
        let moved = gap - held;
        let within = match drift {
            Drift::Within(bound) => moved.unsigned_abs() <= u64::from(bound),
            Drift::Truncated { .. } => moved <= 0 && moved.unsigned_abs() <= state.updates,
        };
        if !within {
            return None;
        }
        if matches!(drift, Drift::Truncated { .. }) {
            *state = CounterState {
                gap: Some(gap),
                updates: 0,
            };
        }
        Some(u32::try_from(moved.unsigned_abs()).unwrap_or(u32::MAX))
    }

    /// A read of a CSR of [`Treatment::CsrKeptAtReset`] giving `mine` in inkemu and `theirs` in
    /// QEMU: equal, or, after a reset of both sides, QEMU's is inkemu's with the bits of `kept`
    /// that inkemu held just before the reset set again, the difference within `mask`. Returns
    /// the bits to take.
    ///
    /// A bit an instruction has written since the reset holds what the life after it wrote, the
    /// same on both sides: it is compared. `csrrw` writes every bit; `csrrs` and `csrrc` write
    /// their operand, and leave the other bits as the read the row admitted gave them. ESP-IDF
    /// sets and clears `pmpcfg` entries this way (`RV_SET_CSR` is a `csrrs`, `RV_CLEAR_CSR` a
    /// `csrrc`: `components/riscv/include/riscv/csr.h` of v5.5.2, `PMP_ENTRY_CFG_SET` and
    /// `PMP_ENTRY_CFG_RESET`).
    fn kept_csr_read(&self, csr: u16, kept: u32, mask: u32, mine: u32, theirs: u32) -> Option<u32> {
        let differ = mine ^ theirs;
        if differ == 0 {
            return Some(0);
        }
        let written = self.bits_written.get(&csr).copied().unwrap_or(0);
        let before = *self.csrs_before_reset.get(&csr)?;
        let still_kept = before & kept & !written;
        (theirs == mine | still_kept && differ & !mask == 0).then_some(differ)
    }

    /// Whether the entries of [`Treatment::RegistersAfterReset`] at the word `address` hold: a
    /// reset left the register in different states on the two sides (a reset of both sides
    /// crossed, or a reset of its module the machine made at a pulse QEMU ignores), and no store
    /// reached the word since.
    fn after_reset_at(&self, address: u32) -> bool {
        let reset = self.counts.resets_crossed > 0
            || self
                .modules_reset
                .iter()
                .any(|&m| (MODULE_RESETS[m].first..=MODULE_RESETS[m].last).contains(&address));
        reset && !self.stored_since_reset.contains(&address)
    }

    /// Whether the side that reset the register at `address` shows the value `entry` declares,
    /// within its mask: inkemu its reset value when the machine reset the register's module
    /// since the last store to it, QEMU its reset value otherwise (a reset of both sides left
    /// inkemu's register as the life before set it). No declared value admits nothing.
    fn after_reset_value_holds(
        &self,
        address: u32,
        entry: &AfterReset,
        mine: u32,
        theirs: u32,
    ) -> bool {
        let mask = entry.register.mask;
        if self.module_reset_since_store(address) {
            entry.inkemu.is_some_and(|value| mine & mask == value)
        } else {
            entry.qemu.is_some_and(|value| theirs & mask == value)
        }
    }

    /// Whether the machine reset the module of the register at `address` (a pulse of its bit,
    /// [`MODULE_RESETS`]) since the last store to the register.
    fn module_reset_since_store(&self, address: u32) -> bool {
        !self.stored_since_pulse.contains(&address)
            && self
                .modules_reset
                .iter()
                .any(|&m| (MODULE_RESETS[m].first..=MODULE_RESETS[m].last).contains(&address))
    }

    /// A word store of `value` to `SYSTEM.PERIP_RST_EN0` or `PERIP_RST_EN1`: the modules whose
    /// bit it sets are reset in the machine (`soc_esp32c3::periph::system`, a reset at the bit
    /// going from 0 to 1) and not in QEMU, whose `SYSTEM` ignores the store; the copies after a
    /// reset hold again for their registers.
    fn note_module_resets(&mut self, address: u32, value: u32) {
        let index = usize::from(address == SYSTEM_PERIP_RST_EN1);
        let rising = value & !self.perip_rst_en[index];
        self.perip_rst_en[index] = value;
        for (m, module) in MODULE_RESETS.iter().enumerate() {
            if module.register == address && rising & module.bit != 0 {
                self.modules_reset.insert(m);
                let outside = |word: &u32| !(module.first..=module.last).contains(word);
                self.stored_since_reset.retain(outside);
                self.stored_since_pulse.retain(outside);
            }
        }
    }

    /// A read of a slow clock QEMU does not model: QEMU reads `qemu`, and inkemu the cycles of
    /// `hz` at the device time of the last capture (0 before any), within `bound`. Returns how
    /// far inkemu's value is from that, `None` when either side is off.
    fn clock_read(
        &self,
        capture: u32,
        hz: u64,
        bound: u32,
        qemu: u32,
        mine: u32,
        theirs: u32,
    ) -> Option<u32> {
        if theirs != qemu {
            return None;
        }
        let expected = self.captures.get(&capture).map_or(0, |&nanos| {
            let cycles = u128::from(nanos) * u128::from(hz) / NANOS_PER_SECOND;
            // The register is the low word of the count.
            u32::try_from(cycles & u128::from(u32::MAX)).unwrap_or(0)
        });
        let moved = mine.wrapping_sub(expected).cast_signed().unsigned_abs();
        (moved <= bound).then_some(moved)
    }

    /// One update QEMU may have made of the counters of [`Drift::Truncated`] whose block
    /// `touched` says the event reached.
    fn note_counter_update(&mut self, touched: impl Fn((u32, u32)) -> bool) {
        for row in self.rows {
            for treatment in row.treatments {
                if let Treatment::Counter {
                    location,
                    drift: Drift::Truncated { first, last },
                    ..
                } = *treatment
                    && touched((first, last))
                {
                    self.counters.entry(location).or_default().updates += 1;
                }
            }
        }
    }

    /// An origin of the counters that name `location`: their next read fixes the gap again.
    fn note_counter_origin(&mut self, location: Location) {
        for row in self.rows {
            for treatment in row.treatments {
                if let Treatment::Counter {
                    location: counter,
                    origins,
                    ..
                } = *treatment
                    && origins.contains(&location)
                {
                    self.counters.entry(counter).or_default().gap = None;
                }
            }
        }
    }

    /// The row that admits the read of `location` giving `mine` in inkemu and `theirs` in QEMU,
    /// with the site and the bits to take from QEMU.
    fn admitted_read(
        &self,
        location: Location,
        mine: u32,
        theirs: u32,
    ) -> Option<Vec<(u16, Site, u32)>> {
        let differ = mine ^ theirs;
        if let Location::Mmio(address) = location {
            return self.admitted_register_read(address, differ, theirs);
        }
        for row in self.rows {
            for treatment in row.treatments {
                match (*treatment, location) {
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
                        return held.then(|| vec![(row.number, Site::at(location), differ)]);
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// The rows that admit a word load from `address` whose two values differ by `differ`, QEMU's
    /// being `theirs`, each with the site and the bits it takes: the register entries there whose
    /// condition holds, those of [`Treatment::RegistersAfterReset`] while a reset holds them
    /// ([`Lockstep::after_reset_at`]) and only with the value the entry declares for the side
    /// that reset ([`Lockstep::after_reset_value_holds`]). Together they must cover every
    /// differing bit and agree with QEMU's value; a bit goes to the first entry that declares it,
    /// in the order of the table. `None` when no entry is there, or they do not cover the read.
    fn admitted_register_read(
        &self,
        address: u32,
        differ: u32,
        theirs: u32,
    ) -> Option<Vec<(u16, Site, u32)>> {
        let mine = theirs ^ differ;
        let after_reset = self.after_reset_at(address);
        let mut entries: Vec<(u16, &Register)> = Vec::new();
        for row in self.rows {
            entries.extend(
                row.registers_at(address, false)
                    .filter(|register| self.condition_holds(register))
                    .map(|register| (row.number, register)),
            );
            if !after_reset {
                continue;
            }
            for entry in row.after_reset_at(address) {
                if self.after_reset_value_holds(address, entry, mine, theirs) {
                    entries.push((row.number, &entry.register));
                } else if differ & entry.register.mask != 0 {
                    // The side that reset shows another value than the row declares.
                    return None;
                }
            }
        }
        if entries.is_empty() {
            return None;
        }
        let covered = entries
            .iter()
            .fold(0, |mask, (_, register)| mask | register.mask);
        let qemu_ok = entries
            .iter()
            .all(|(_, register)| register.qemu.is_none_or(|value| value == theirs));
        if differ & !covered != 0 || !qemu_ok {
            return None;
        }
        let mut left = differ;
        let mut taken = Vec::with_capacity(entries.len());
        for (row, register) in &entries {
            let bits = left & register.mask;
            left &= !register.mask;
            let site = Site {
                location: Location::Mmio(address),
                after_command: register.after_command,
            };
            taken.push((*row, site, bits));
        }
        Some(taken)
    }

    /// A read of `mip` into a register, under the row of `MEIP`: inkemu reads no `MEIP`, and the
    /// `MEIP` QEMU read is the one its state showed at the boundary the instruction started from
    /// (`previous`), which [`Lockstep::compare`] has already held to QEMU's matrix on inkemu's
    /// controller. Every other bit is compared.
    fn admitted_mip_read(&self, mine: u32, theirs: u32, previous: &CpuState) -> Option<(u16, u32)> {
        let row = self.rows.iter().find(|row| {
            row.treatments
                .iter()
                .any(|t| matches!(t, Treatment::ExternalInterrupt { .. }))
        })?;
        let differ = mine ^ theirs;
        let shown = previous.get(Field::Mip) & MEIP;
        (differ & !MEIP == 0 && mine & MEIP == 0 && theirs & MEIP == shown)
            .then_some((row.number, differ))
    }

    /// Whether the condition a register entry carries holds now.
    fn condition_holds(&self, register: &Register) -> bool {
        register
            .after_command
            .is_none_or(|command| self.spi1_command == Some(command))
    }

    /// What an executed instruction leaves for the rows that follow it: the CSRs it wrote, the
    /// origins and updates of the counters, the capture of a slow clock, the SPI1 command it
    /// started, a threshold store that QEMU's `MEIP` follows late. `previous` is QEMU's state
    /// the instruction started from. An instruction that raised an exception wrote nothing.
    fn note_effects(&mut self, executed: &Executed, previous: &CpuState, target: &impl Target) {
        let Some(decoded) = executed.instruction else {
            return;
        };
        if executed.trap.is_some() {
            return;
        }
        match decoded.instr {
            Instr::Csr {
                op, src, uimm, csr, ..
            } if op == CsrOp::Rw || src != 0 => {
                self.written.insert(csr);
                let bits = match (op, uimm) {
                    (CsrOp::Rw, _) => u32::MAX,
                    (_, true) => u32::from(src),
                    (_, false) => self.before.x(src),
                };
                *self.bits_written.entry(csr).or_default() |= bits;
                self.note_counter_origin(Location::Csr(csr));
            }
            Instr::Store { op, rs2, .. } => {
                let Some(address) = data_address(decoded, &self.before) else {
                    return;
                };
                self.note_counter_update(|(first, last)| (first..=last).contains(&address));
                self.note_counter_origin(Location::Mmio(address & !3));
                if PERIPHERALS.contains(&address) {
                    self.stored_since_reset.insert(address & !3);
                    self.stored_since_pulse.insert(address & !3);
                }
                if op != StoreOp::W {
                    return;
                }
                let value = self.before.x(rs2);
                self.note_capture(address, value, target);
                match address {
                    SYSTEM_PERIP_RST_EN0 | SYSTEM_PERIP_RST_EN1 => {
                        self.note_module_resets(address, value);
                    }
                    SPI1_USER2 => self.spi1_user2 = value,
                    SPI1_CMD if value & SPI1_CMD_USR != 0 => {
                        self.spi1_command = Some(self.spi1_user2.to_le_bytes()[0]);
                    }
                    // Only a store that brings QEMU's line down: `MEIP` was up at the boundary
                    // the store started from, and QEMU's matrix computes it down after it. The
                    // window shows QEMU's `MEIP` late after exactly those stores.
                    CPU_INT_THRESH
                        if previous.get(Field::Mip) & MEIP != 0
                            && !target.external_interrupt_as_qemu() =>
                    {
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

    /// A word store of `value` to `address`: the capture of the slow clocks whose capture
    /// register and trigger bit it is takes the device time after it. The capture is made at the
    /// store and device time moves by that one instruction, 8 ns, far less than one cycle of a
    /// slow clock.
    fn note_capture(&mut self, address: u32, value: u32, target: &impl Target) {
        for row in self.rows {
            for treatment in row.treatments {
                if let Treatment::SlowClock {
                    capture, trigger, ..
                } = *treatment
                    && capture == address
                    && value & trigger != 0
                {
                    self.captures.insert(capture, target.device_time_nanos());
                }
            }
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
            if let Some(&hold) = self.kept.get(&field) {
                if hold.holds(q, i) {
                    if let Some(location) = Location::of(field) {
                        self.usage.record(hold.row, Site::at(location), 0, 0);
                    }
                    continue;
                }
                // A trap changed it, or one side alone: compared from now on.
                self.kept.remove(&field);
            }
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
        let taken = self.admitted_read(Location::Csr(csr), i, q)?;
        for &(row, site, bits) in &taken {
            self.usage.record(row, site, bits, 0);
        }
        Some(q ^ i)
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
            self.usage
                .record(row.number, Site::at(Location::Csr(MIP)), MEIP, 0);
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
                let masks: Vec<u32> = row.registers_at(a, true).map(|r| r.mask).collect();
                let held = row
                    .sites()
                    .any(|(site, _)| site.location == Location::Mmio(a));
                if masks.is_empty() && held {
                    return Some(Nearest {
                        row: row.number,
                        why: format!(
                            "holds the read of {a:#010x} as a counter or a clock, which the read \
                             passed"
                        ),
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
                    row.declared(Site::at(location)).map(|bits| Nearest {
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

/// The compared quantities an instruction reads: its source registers, the CSR it reads, what
/// `mret` reads, and `mtvec` for an instruction that always traps. `x0` is not a quantity.
fn reads(instr: Instr) -> Vec<Field> {
    let x = |reg: u8| (reg != 0).then_some(Field::X(reg));
    match instr {
        Instr::Jalr { rs1, .. } | Instr::Load { rs1, .. } | Instr::OpImm { rs1, .. } => {
            x(rs1).into_iter().collect()
        }
        Instr::Branch { rs1, rs2, .. }
        | Instr::Store { rs1, rs2, .. }
        | Instr::Op { rs1, rs2, .. }
        | Instr::MulDiv { rs1, rs2, .. } => x(rs1).into_iter().chain(x(rs2)).collect(),
        Instr::Csr {
            op,
            rd,
            src,
            uimm,
            csr,
        } => {
            let source = (!uimm).then(|| x(src)).flatten();
            let read = (op != CsrOp::Rw || rd != 0)
                .then(|| Field::of_csr(csr))
                .flatten();
            source.into_iter().chain(read).collect()
        }
        Instr::Mret => vec![Field::Mepc, Field::Mstatus],
        Instr::Ecall | Instr::Ebreak | Instr::Illegal(_) => vec![Field::Mtvec],
        Instr::Lui { .. }
        | Instr::Auipc { .. }
        | Instr::Jal { .. }
        | Instr::Fence
        | Instr::FenceI
        | Instr::Wfi => Vec::new(),
    }
}

/// The compared quantities an instruction that does not trap writes: its destination register,
/// the CSR it writes, and `mstatus` for `mret`.
fn writes(instr: Instr) -> Vec<Field> {
    let x = |reg: u8| (reg != 0).then_some(Field::X(reg));
    match instr {
        Instr::Lui { rd, .. }
        | Instr::Auipc { rd, .. }
        | Instr::Jal { rd, .. }
        | Instr::Jalr { rd, .. }
        | Instr::Load { rd, .. }
        | Instr::OpImm { rd, .. }
        | Instr::Op { rd, .. }
        | Instr::MulDiv { rd, .. } => x(rd).into_iter().collect(),
        Instr::Csr {
            op, rd, src, csr, ..
        } => {
            let written = (op == CsrOp::Rw || src != 0)
                .then(|| Field::of_csr(csr))
                .flatten();
            x(rd).into_iter().chain(written).collect()
        }
        Instr::Mret => vec![Field::Mstatus],
        Instr::Branch { .. }
        | Instr::Store { .. }
        | Instr::Fence
        | Instr::FenceI
        | Instr::Ecall
        | Instr::Ebreak
        | Instr::Wfi
        | Instr::Illegal(_) => Vec::new(),
    }
}

/// The bytes a store writes.
const fn store_bytes(op: StoreOp) -> u32 {
    match op {
        StoreOp::B => 1,
        StoreOp::H => 2,
        StoreOp::W => 4,
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
