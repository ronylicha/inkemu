//! A single RV32IMC hart: machine mode, and user mode with a PMP on the cores that have them.
use crate::csr::{Core, CsrFile, MSTATUS_MIE, MSTATUS_MPIE, Privilege};
use crate::dcache::DecodeCache;
use crate::decode::decode;
use crate::instr::Decoded;
use crate::trap::{Exception, Trap};
use emu_core::{AccessKind, AccessSize, Bus, Cycles, InstrClass, TimingModel};

const MCAUSE_INTERRUPT: u32 = 1 << 31;
const MTVEC_MODE_MASK: u32 = 0b11;
const MTVEC_VECTORED: u32 = 1;
const COMPRESSED_MASK: u32 = 0b11;
/// One 16-bit instruction parcel.
const PARCEL_MASK: u32 = 0xffff;
const PARCEL_BYTES: u32 = 2;
/// Bytes between two entries of a vectored trap table (one `jal`/`j` per entry).
const VECTOR_ENTRY_BYTES: u32 = 4;

/// What the core does on a load/store whose address is not aligned to its size.
/// The real `ESP32-C3` behaviour is arbitrated by the QEMU lockstep in `SP1b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MisalignedPolicy {
    /// Raise a load/store address-misaligned exception (the default).
    #[default]
    Trap,
    /// Forward the access to the bus as is.
    Allow,
}

/// Where the hart's interrupts come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InterruptModel {
    /// The privileged specification: a line is taken when its bit is set in both `mip` and `mie`
    /// (the default).
    #[default]
    Standard,
    /// An interrupt controller outside the hart selects at most one line and presents it with
    /// [`Hart::set_external_interrupt`]. `mie` and `mip` are plain read/write CSRs that gate
    /// nothing; `mstatus.MIE` still does.
    ///
    /// ASSUMPTION: on the ESP32-C3, `mie` and `mip` are plain registers, and `wfi` ends when a
    /// line is presented even with `mstatus.MIE` clear (decisions E3 and E4). Neither is
    /// documented (report IR §7, questions 4 and 5); a probe on a real chip settles both.
    Controller,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct HartConfig {
    pub misaligned: MisalignedPolicy,
    /// The core the hart models: its privilege modes, its PMP, its vendor's CSRs.
    pub core: Core,
    /// Where interrupts come from: `mip` and `mie` (the default), or a line presented by a
    /// controller outside the hart ([`Hart::set_external_interrupt`]).
    pub interrupts: InterruptModel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    /// One instruction retired, costing `cycles`.
    Retired { cycles: u32 },
    /// A synchronous exception was taken; the faulting instruction did not retire.
    Trapped { trap: Trap, cycles: u32 },
    /// An enabled interrupt was taken before fetching; `cause` is the interrupt line.
    Interrupted { cause: u32 },
    /// The hart is parked in `wfi` and no enabled interrupt is pending.
    Waiting,
}

#[derive(Clone, Debug)]
pub struct Hart {
    pub x: [u32; 32],
    pub pc: u32,
    pub csr: CsrFile,
    pub cycles: Cycles,
    pub instret: u64,
    pub config: HartConfig,
    /// The mode the hart executes in ([`Hart::privilege`]).
    pub(crate) privilege: Privilege,
    pub(crate) waiting: bool,
    /// The line an outside controller presents ([`InterruptModel::Controller`]).
    pub(crate) external: Option<u32>,
    pub(crate) dcache: DecodeCache,
    /// The last instruction fetched and decoded ([`Hart::last_instruction`]).
    last: Option<Decoded>,
}

/// What [`Hart::take_boundary`] did before the next instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    /// Nothing stands in the way: the next [`Hart::step`] executes the instruction at `pc`.
    Ready,
    /// The hart took the interrupt on this line: `pc` is its handler.
    Interrupted { cause: u32 },
    /// The hart waits in `wfi` and no line is pending.
    Waiting,
}

impl Hart {
    #[must_use]
    pub fn new(reset_pc: u32, config: HartConfig) -> Hart {
        Hart {
            x: [0; 32],
            pc: reset_pc,
            csr: CsrFile::new(config.core),
            cycles: Cycles::ZERO,
            instret: 0,
            config,
            privilege: Privilege::Machine,
            waiting: false,
            external: None,
            dcache: DecodeCache::new(),
            last: None,
        }
    }

    /// Resets the hart: registers, CSRs (the PMP entries and their lock bits with them), the mode
    /// (machine) and decoded instructions are those of a new hart about to execute `reset_pc`; a
    /// reset is the only way to clear a lock bit (TRM §1.8.3: "it can only be cleared through CPU
    /// reset"). The cycle and
    /// instruction totals ([`Hart::cycles`], [`Hart::instret`]) are measures of the run, not
    /// state of the core: they go on. The decode cache is rebuilt, so its statistics
    /// ([`Hart::decode_cache_stats`]) restart at zero too.
    ///
    /// ASSUMPTION: what software reads of the counters restarts with the core: `mcycle` and
    /// `minstret` read 0 after a reset, as on a new hart, and so does the vendor's performance
    /// counter `mpccr`. Nothing establishes it. The architecture leaves the reset value of
    /// `mcycle` and `minstret` arbitrary, and the TRM does not list the two among the CSRs of
    /// this core (§1.4.1, as quoted in report IR §2.1); the reset of `mpccr` with the core is
    /// "not established; ASSUMED" (report IR §4.2), on the strength of log timestamps that are
    /// small after a restart in every log seen (report IR §7 question 7). Reading `mpccr` at the
    /// first instruction after a CPU reset on a real chip settles it.
    ///
    /// ASSUMPTION: a reset clears the integer registers. The architecture leaves them undefined;
    /// the mask ROM writes every register it reads, so no measured boot tells the difference. A
    /// register dump taken at the first instruction after a reset, on a chip with a debugger
    /// attached, settles it.
    pub fn reset(&mut self, reset_pc: u32) {
        self.x = [0; 32];
        self.pc = reset_pc;
        self.csr = CsrFile::new(self.config.core);
        self.csr.restart_counters(self.cycles, self.instret);
        self.privilege = Privilege::Machine;
        self.waiting = false;
        self.external = None;
        self.dcache = DecodeCache::new();
    }

    /// Presents the line an outside interrupt controller selected, `None` when it selects none.
    /// Only used with [`InterruptModel::Controller`]. The hart has lines 0 to 31 (the cause is
    /// five bits wide and indexes the vector table): a line of 32 or more does not exist and is
    /// taken as `None`.
    pub fn set_external_interrupt(&mut self, line: Option<u32>) {
        self.external = line.filter(|line| *line < 32);
    }

    /// Whether the hart is parked in `wfi`.
    #[must_use]
    pub const fn is_waiting(&self) -> bool {
        self.waiting
    }

    /// The mode the hart executes in: machine mode from a reset and in every trap handler, user
    /// mode after an `mret` with `MPP` = 0 on a core that has it.
    #[must_use]
    pub const fn privilege(&self) -> Privilege {
        self.privilege
    }

    /// `(hits, misses)` of the decoded-instruction cache, for performance reporting.
    #[must_use]
    pub fn decode_cache_stats(&self) -> (u64, u64) {
        self.dcache.stats()
    }

    /// Drives one `mip` bit; `line` must be lower than 32.
    pub fn set_interrupt_pending(&mut self, line: u32, pending: bool) {
        debug_assert!(line < 32, "interrupt line {line} out of range");
        let bit = 1u32 << (line & 31);
        if pending {
            self.csr.mip |= bit;
        } else {
            self.csr.mip &= !bit;
        }
    }

    /// The line to take: the highest-numbered one that is pending and enabled in the CSRs, or the
    /// one an outside controller presents.
    fn pending_line(&self) -> Option<u32> {
        match self.config.interrupts {
            InterruptModel::Standard => (self.csr.mip & self.csr.mie).checked_ilog2(),
            InterruptModel::Controller => self.external,
        }
    }

    /// What stands between the hart and its next instruction, acted on: a `wfi` that a pending
    /// line ends, then the interrupt the hart takes. [`Hart::step`] does this first; a caller that
    /// wants the state the next instruction starts from (a lockstep, a debugger) calls it alone
    /// until it returns [`Boundary::Ready`].
    pub fn take_boundary(&mut self) -> Boundary {
        let pending = self.pending_line();
        if self.waiting {
            if pending.is_none() {
                return Boundary::Waiting;
            }
            // `wfi` ends on any pending line, even when `mstatus.MIE` keeps it from being taken:
            // one set in both `mip` and `mie` (the standard model), or the one a controller
            // presents (the instruction after `wfi` then runs: `InterruptModel::Controller`).
            self.waiting = false;
        }
        // A machine interrupt is always enabled below machine mode, whatever `mstatus.MIE` says
        // (privileged specification, "Machine Status Register").
        let enabled = self.privilege == Privilege::User || self.csr.mstatus & MSTATUS_MIE != 0;
        if enabled && let Some(line) = pending {
            self.enter_trap(MCAUSE_INTERRUPT | line, 0);
            return Boundary::Interrupted { cause: line };
        }
        Boundary::Ready
    }

    /// The last instruction the hart fetched and decoded, whether it retired or raised an
    /// exception; `None` before the first one and after a fetch that faulted.
    #[must_use]
    pub const fn last_instruction(&self) -> Option<Decoded> {
        self.last
    }

    /// Executes one instruction, or takes one pending interrupt, or reports that the hart is waiting.
    pub fn step(&mut self, bus: &mut dyn Bus, timing: &dyn TimingModel) -> StepOutcome {
        match self.take_boundary() {
            Boundary::Waiting => return StepOutcome::Waiting,
            Boundary::Interrupted { cause } => return StepOutcome::Interrupted { cause },
            Boundary::Ready => {}
        }
        let (decoded, fetch_stall) = match self.fetch_decoded(bus) {
            Ok(fetched) => fetched,
            Err(trap) => {
                self.last = None;
                return self.trap(trap, timing.instr_cycles(InstrClass::System));
            }
        };
        self.last = Some(decoded);
        match self.execute(decoded, bus) {
            Ok(done) => {
                let cycles = timing.instr_cycles(done.class) + fetch_stall + done.stall;
                self.cycles = self.cycles + u64::from(cycles);
                self.instret += 1;
                self.csr.on_instruction_retired(cycles);
                self.pc = done.next_pc;
                self.waiting = done.wait;
                StepOutcome::Retired { cycles }
            }
            Err(trap) => self.trap(trap, timing.instr_cycles(InstrClass::System) + fetch_stall),
        }
    }

    /// Serves the instruction at `pc` from the decode cache or the bus; a hit still pays the fetch
    /// timing, and is still checked by the PMP parcel by parcel: the cache holds decodings, not
    /// permissions, so a write to the PMP takes effect at the next fetch without invalidating it.
    fn fetch_decoded(&mut self, bus: &mut dyn Bus) -> Result<(Decoded, u32), Trap> {
        if let Some(decoded) = self.dcache.lookup(self.pc, &*bus) {
            self.check_fetch(self.pc)?;
            if u32::from(decoded.len) > PARCEL_BYTES {
                self.check_fetch(self.pc.wrapping_add(PARCEL_BYTES))?;
            }
            let stall = bus.fetch_stall(self.pc, u32::from(decoded.len));
            return Ok((decoded, stall));
        }
        let (raw, stall) = self.fetch(bus)?;
        let decoded = decode(raw);
        if bus.fetch_is_memory(self.pc, u32::from(decoded.len)) {
            self.dcache.insert(self.pc, decoded, &*bus);
        }
        Ok((decoded, stall))
    }

    /// Fetches one parcel or two, so a 32-bit instruction may straddle a page or region boundary.
    /// A fault on the second parcel reports that parcel's address in `tval`, as the privileged spec
    /// requires; the PMP checks each parcel before the bus is read, as Sail does (report
    /// `docs/research/esp32c3-pmp-and-user-mode.md` §4, §6).
    fn fetch(&mut self, bus: &mut dyn Bus) -> Result<(u32, u32), Trap> {
        let fault = |addr: u32| Trap {
            cause: Exception::InstrAccessFault,
            tval: addr,
        };
        self.check_fetch(self.pc)?;
        let lo = bus
            .read(self.pc, AccessSize::Half, AccessKind::Fetch)
            .map_err(|_| fault(self.pc))?;
        // Each parcel is masked to 16 bits: the bus contract says so, but a peripheral may not comply.
        let lo_parcel = lo.value & PARCEL_MASK;
        if lo_parcel & COMPRESSED_MASK != COMPRESSED_MASK {
            return Ok((lo_parcel, lo.stall));
        }
        let hi_addr = self.pc.wrapping_add(PARCEL_BYTES);
        self.check_fetch(hi_addr)?;
        let hi = bus
            .read(hi_addr, AccessSize::Half, AccessKind::Fetch)
            .map_err(|_| fault(hi_addr))?;
        Ok((
            lo_parcel | ((hi.value & PARCEL_MASK) << 16),
            lo.stall + hi.stall,
        ))
    }

    /// The PMP's verdict on fetching the parcel at `addr` in the current mode.
    fn check_fetch(&mut self, addr: u32) -> Result<(), Trap> {
        if self
            .csr
            .pmp_allows(addr, PARCEL_BYTES, AccessKind::Fetch, self.privilege)
        {
            Ok(())
        } else {
            Err(Trap {
                cause: Exception::InstrAccessFault,
                tval: addr,
            })
        }
    }

    fn trap(&mut self, trap: Trap, cycles: u32) -> StepOutcome {
        self.enter_trap(trap.cause.code(), trap.tval);
        self.cycles = self.cycles + u64::from(cycles);
        StepOutcome::Trapped { trap, cycles }
    }

    fn enter_trap(&mut self, mcause: u32, tval: u32) {
        let csr = &mut self.csr;
        csr.mepc = self.pc;
        csr.mcause = mcause;
        csr.mtval = tval;
        let mie_was_set = csr.mstatus & MSTATUS_MIE != 0;
        csr.mstatus &= !(MSTATUS_MIE | MSTATUS_MPIE);
        if mie_was_set {
            csr.mstatus |= MSTATUS_MPIE;
        }
        // Every trap is taken in machine mode: nothing delegates one to another mode.
        csr.save_privilege_on_trap(self.privilege);
        self.privilege = Privilege::Machine;
        let base = csr.mtvec & !MTVEC_MODE_MASK;
        let vectored =
            csr.mtvec & MTVEC_MODE_MASK == MTVEC_VECTORED && mcause & MCAUSE_INTERRUPT != 0;
        self.pc = if vectored {
            base.wrapping_add(VECTOR_ENTRY_BYTES * (mcause & !MCAUSE_INTERRUPT))
        } else {
            base
        };
    }

    /// `mret`: restores `MIE` from `MPIE`, sets `MPIE`, goes on in the mode `MPP` held, sets `MPP`
    /// to the least privileged mode of the core, and returns the address to resume at.
    ///
    /// ASSUMPTION: `mret` sets `MPIE`, the rule of the RISC-V privileged specification (and what
    /// QEMU does). The ESP32-C3 TRM says `MPIE` is cleared (report IR §2.3, §7 question 6). The
    /// difference shows only in a read of `mstatus` outside a handler, which no measured code
    /// does. Reading `mstatus` right after an `mret` on a chip settles it.
    pub(crate) fn return_from_trap(&mut self) -> u32 {
        let csr = &mut self.csr;
        let mpie = csr.mstatus & MSTATUS_MPIE != 0;
        csr.mstatus =
            (csr.mstatus & !MSTATUS_MIE) | MSTATUS_MPIE | if mpie { MSTATUS_MIE } else { 0 };
        self.privilege = csr.return_privilege();
        csr.mepc
    }
}

#[cfg(test)]
// The arithmetic test reinterprets a register as signed to compare against -10.
#[allow(clippy::cast_possible_wrap)]
mod tests {
    use super::*;
    use crate::asm::*;
    use crate::csr::*;
    use crate::csr_esp32c3::{PCCR_USER, PCER_USER, PCMR_USER};
    use emu_core::{InstrClass, Ram, TimingModel};

    #[derive(Debug)]
    struct OneCycle;
    impl TimingModel for OneCycle {
        fn name(&self) -> &'static str {
            "one-cycle"
        }
        fn is_calibrated(&self) -> bool {
            false
        }
        fn instr_cycles(&self, _: InstrClass) -> u32 {
            1
        }
    }

    const TRAP_VECTOR: u32 = 0x1000;

    fn machine(program: &[u32], policy: MisalignedPolicy) -> (Hart, Ram) {
        let mut ram = Ram::new(0, 64 * 1024);
        let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
        ram.load(0, &bytes).unwrap();
        let mut hart = Hart::new(
            0,
            HartConfig {
                misaligned: policy,
                ..HartConfig::default()
            },
        );
        assert!(hart.csr.write(MTVEC, TRAP_VECTOR));
        (hart, ram)
    }

    fn run(hart: &mut Hart, ram: &mut Ram, steps: usize) -> Vec<StepOutcome> {
        (0..steps).map(|_| hart.step(ram, &OneCycle)).collect()
    }

    #[test]
    fn arithmetic_and_x0_hardwired() {
        let (mut h, mut m) = machine(
            &[
                addi(1, 0, 7),
                addi(2, 0, -3),
                add(3, 1, 2),
                sub(4, 2, 1),
                addi(0, 0, 9),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 5);
        assert_eq!((h.x[3], h.x[4] as i32, h.x[0]), (4, -10, 0));
        assert_eq!((h.pc, h.instret), (20, 5));
    }

    #[test]
    fn lui_auipc_and_jumps_link() {
        let (mut h, mut m) = machine(
            &[
                lui(5, 0x1234_5000),
                auipc(6, 0x1000),
                jal(1, 8),
                addi(7, 0, 1),
                jalr(2, 1, 0),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 4);
        assert_eq!(h.x[5], 0x1234_5000);
        assert_eq!(h.x[6], 0x1004); // auipc at pc=4
        assert_eq!(h.x[1], 12); // jal at pc=8 links to 12, jumps to 16
        assert_eq!(h.x[7], 0); // skipped
        assert_eq!((h.x[2], h.pc), (20, 12)); // jalr at 16 returns to x1=12
    }

    #[test]
    fn branches_report_taken_and_not_taken() {
        let (mut h, mut m) = machine(
            &[
                addi(1, 0, 1),
                beq(1, 0, 8),
                bne(1, 0, 8),
                addi(2, 0, 1),
                blt(0, 1, -4),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 3);
        assert_eq!(h.pc, 16); // beq not taken (4→8), bne taken (8→16)
        assert_eq!(h.x[2], 0);
    }

    #[test]
    fn loads_sign_and_zero_extend() {
        let (mut h, mut m) = machine(
            &[
                addi(1, 0, 0x200),
                addi(2, 0, -1),
                sb(2, 1, 0),
                lb(3, 1, 0),
                lbu(4, 1, 0),
                lh(5, 1, 0),
                sw(2, 1, 4),
                lw(6, 1, 4),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 8);
        assert_eq!(
            (h.x[3], h.x[4], h.x[5], h.x[6]),
            (0xffff_ffff, 0xff, 0x0000_00ff, 0xffff_ffff)
        );
    }

    /// A bus whose sub-word reads leave garbage above the access size, as a peripheral
    /// that answers a byte read with its whole register word would. Only reads of the
    /// given kind are dirtied, so each test isolates one consumer of the contract.
    #[derive(Debug)]
    struct DirtyBus(Ram, AccessKind);
    impl Bus for DirtyBus {
        fn read(
            &mut self,
            a: u32,
            s: AccessSize,
            k: AccessKind,
        ) -> Result<emu_core::ReadOutcome, emu_core::BusError> {
            let mut o = self.0.read(a, s, k)?;
            if k == self.1 {
                o.value |= match s {
                    AccessSize::Byte => 0xDEAD_BE00,
                    AccessSize::Half => 0xDEAD_0000,
                    AccessSize::Word => 0,
                };
            }
            Ok(o)
        }
        fn write(&mut self, a: u32, s: AccessSize, v: u32) -> Result<u32, emu_core::BusError> {
            self.0.write(a, s, v)
        }
        fn page_version(&self, a: u32) -> u64 {
            self.0.page_version(a)
        }
    }

    #[test]
    fn sub_word_loads_ignore_bits_above_the_access_size() {
        let (mut h, mut m) = machine(
            &[
                addi(1, 0, 0x200),
                addi(2, 0, -0x80), // 0xffff_ff80
                sw(2, 1, 0),       // memory holds 0x80 0xff 0xff 0xff
                lb(3, 1, 0),
                lbu(4, 1, 0),
                lh(5, 1, 0),
                lhu(6, 1, 0),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 3);
        let mut bus = DirtyBus(m, AccessKind::Load);
        for _ in 0..4 {
            h.step(&mut bus, &OneCycle);
        }
        assert_eq!(h.x[3], 0xffff_ff80, "lb sign-extends the low byte");
        assert_eq!(h.x[4], 0x0000_0080, "lbu zero-extends the low byte");
        assert_eq!(h.x[5], 0xffff_ff80, "lh sign-extends the low half");
        assert_eq!(h.x[6], 0x0000_ff80, "lhu zero-extends the low half");
    }

    #[test]
    fn a_32_bit_instruction_fetched_as_two_halves_ignores_dirty_upper_bits() {
        let (mut h, m) = machine(&[addi(5, 0, 7)], MisalignedPolicy::Trap);
        let mut bus = DirtyBus(m, AccessKind::Fetch);
        let out = h.step(&mut bus, &OneCycle);
        assert_eq!(out, StepOutcome::Retired { cycles: 1 });
        assert_eq!((h.x[5], h.pc), (7, 4));
    }

    #[test]
    fn mul_div_edge_cases_follow_the_spec() {
        let (mut h, mut m) = machine(
            &[
                addi(1, 0, -1),
                lui(2, 0x8000_0000),
                addi(3, 0, 0),
                mulh(10, 1, 1),
                mulhu(11, 1, 1),
                mulhsu(12, 1, 1),
                mul(13, 1, 1),
                div(14, 1, 3),
                divu(15, 1, 3),
                rem(16, 1, 3),
                remu(17, 2, 3),
                div(18, 2, 1),
                rem(19, 2, 1),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 13);
        assert_eq!(h.x[10], 0); // (-1)*(-1) = 1, high word 0
        assert_eq!(h.x[11], 0xffff_fffe); // 0xffffffff^2 high word
        assert_eq!(h.x[12], 0xffff_ffff); // -1 * 0xffffffff, high word
        assert_eq!(h.x[13], 1);
        assert_eq!((h.x[14], h.x[15]), (0xffff_ffff, 0xffff_ffff)); // division by zero
        assert_eq!((h.x[16], h.x[17]), (0xffff_ffff, 0x8000_0000)); // remainder by zero = dividend
        assert_eq!((h.x[18], h.x[19]), (0x8000_0000, 0)); // INT_MIN / -1 overflow
    }

    #[test]
    fn unmapped_load_traps() {
        let (mut h, mut m) = machine(&[lui(1, 0x8000_0000), lw(2, 1, 0)], MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 2);
        assert_eq!(
            out[1],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::LoadAccessFault,
                    tval: 0x8000_0000
                },
                cycles: 1
            }
        );
        assert_eq!(h.pc, TRAP_VECTOR);
        assert_eq!(h.csr.read(MEPC, h.cycles, h.instret), Some(4));
        assert_eq!(h.csr.read(MCAUSE, h.cycles, h.instret), Some(5));
        assert_eq!(h.csr.read(MTVAL, h.cycles, h.instret), Some(0x8000_0000));
        assert_eq!(h.instret, 1); // the faulting load does not retire
    }

    #[test]
    fn misaligned_load_traps_or_succeeds_by_policy() {
        let prog = [addi(1, 0, 0x201), lw(2, 1, 0)];
        let (mut h, mut m) = machine(&prog, MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 2);
        assert_eq!(
            out[1],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::LoadMisaligned,
                    tval: 0x201
                },
                cycles: 1
            }
        );
        let (mut h, mut m) = machine(&prog, MisalignedPolicy::Allow);
        let out = run(&mut h, &mut m, 2);
        assert!(matches!(out[1], StepOutcome::Retired { .. }));
    }

    #[test]
    fn illegal_instruction_reports_raw_word() {
        let (mut h, mut m) = machine(&[0xffff_ffff], MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 1);
        assert_eq!(
            out[0],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::IllegalInstr,
                    tval: 0xffff_ffff
                },
                cycles: 1
            }
        );
    }

    #[test]
    fn ecall_traps_and_mret_returns() {
        let mut prog = vec![ecall(), addi(5, 0, 1)];
        prog.resize((TRAP_VECTOR / 4) as usize, 0x0000_0013); // nops
        prog.extend([csrrs(6, MEPC, 0), addi(6, 6, 4), csrrw(0, MEPC, 6), mret()]);
        let (mut h, mut m) = machine(&prog, MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 6);
        assert!(matches!(
            out[0],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::EcallM,
                    tval: 0
                },
                ..
            }
        ));
        assert_eq!(h.x[5], 1); // resumed after the ecall
    }

    #[test]
    fn read_only_csr_write_is_illegal_and_unknown_csr_is_illegal() {
        let (mut h, mut m) = machine(
            &[
                csrrw(1, MHARTID, 2),
                csrrs(1, 0x7ff, 0),
                csrrs(3, MHARTID, 0),
            ],
            MisalignedPolicy::Trap,
        );
        let out = run(&mut h, &mut m, 1);
        assert!(matches!(
            out[0],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::IllegalInstr,
                    ..
                },
                ..
            }
        ));
        h.pc = 4;
        let out = run(&mut h, &mut m, 2);
        assert!(matches!(
            out[0],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::IllegalInstr,
                    ..
                },
                ..
            }
        ));
        h.pc = 8;
        let out = run(&mut h, &mut m, 1);
        assert!(matches!(out[0], StepOutcome::Retired { .. })); // csrrs x3, mhartid, x0 does not write
        assert_eq!(h.x[3], 0);
    }

    #[test]
    fn wfi_waits_then_vectored_interrupt_is_taken() {
        const LINE: u32 = 7;
        let (mut h, mut m) = machine(
            &[csrrwi(0, MIE, 0), wfi(), addi(1, 0, 1)],
            MisalignedPolicy::Trap,
        );
        assert!(h.csr.write(MTVEC, TRAP_VECTOR | 1)); // vectored mode
        assert!(h.csr.write(MIE, 1 << LINE));
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        h.pc = 4; // skip the mie reset
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Retired { .. }
        )); // wfi retires
        assert_eq!(h.step(&mut m, &OneCycle), StepOutcome::Waiting);
        h.set_interrupt_pending(LINE, true);
        assert_eq!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Interrupted { cause: LINE }
        );
        assert_eq!(h.pc, TRAP_VECTOR + 4 * LINE);
        assert_eq!(
            h.csr.read(MCAUSE, h.cycles, h.instret),
            Some(0x8000_0000 | LINE)
        );
        assert_eq!(h.csr.read(MEPC, h.cycles, h.instret), Some(8));
        assert_eq!(
            h.csr
                .read(MSTATUS, h.cycles, h.instret)
                .map(|s| s & MSTATUS_MIE),
            Some(0)
        );
    }

    #[test]
    fn counters_track_cycles_and_instret() {
        let (mut h, mut m) = machine(
            &[addi(1, 0, 1), csrrs(2, MINSTRET, 0), csrrs(3, MCYCLE, 0)],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 3);
        assert_eq!((h.x[2], h.x[3]), (1, 2));
    }
    #[test]
    fn minstret_write_suppresses_the_writing_instructions_own_increment() {
        // Privileged spec: a CSR write to minstret takes precedence over the increment
        // that the writing instruction itself would cause (riscv-tests rv32mi-p-instret_overflow).
        let (mut h, mut m) = machine(
            &[
                csrrwi(0, MINSTRET, 0),
                csrrs(1, MINSTRET, 0),
                csrrs(2, MINSTRET, 0),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 3);
        assert_eq!((h.x[1], h.x[2]), (0, 1));
    }

    #[test]
    fn minstreth_write_suppresses_the_writing_instructions_own_increment() {
        let (mut h, mut m) = machine(
            &[
                addi(5, 0, -1),
                csrrw(0, MINSTRET, 5),
                csrrw(0, MINSTRETH, 5),
                addi(0, 0, 0),
                csrrs(1, MINSTRET, 0),
                csrrs(2, MINSTRETH, 0),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 6);
        // Both writes are suppressed, so minstret holds u64::MAX when the nop retires and
        // wraps to 0 there; the first csrrs then reads 0, and the second reads the high word 0.
        assert_eq!((h.x[1], h.x[2]), (0, 0));
    }

    #[test]
    fn illegal_csr_access_reports_instruction_word() {
        let word = csrrw(1, MHARTID, 2);
        let (mut h, mut m) = machine(&[word], MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 1);
        assert_eq!(
            out[0],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::IllegalInstr,
                    tval: word
                },
                cycles: 1
            }
        );
    }

    #[test]
    fn ebreak_traps_with_its_own_address_as_tval() {
        let (mut h, mut m) = machine(&[addi(0, 0, 0), ebreak()], MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 2);
        assert_eq!(
            out[1],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::Breakpoint,
                    tval: 4
                },
                cycles: 1
            }
        );
        assert_eq!(h.csr.read(MEPC, h.cycles, h.instret), Some(4));
    }

    #[test]
    fn compressed_instructions_retire_with_two_byte_steps() {
        // c.li x10, 5 ; c.nop packed in one word.
        let (mut h, mut m) = machine(&[0x0001_4515], MisalignedPolicy::Trap);
        run(&mut h, &mut m, 2);
        assert_eq!((h.x[10], h.pc, h.instret), (5, 4, 2));
    }

    #[test]
    fn fetch_from_unmapped_memory_raises_instruction_access_fault() {
        let (mut h, mut m) = machine(&[], MisalignedPolicy::Trap);
        h.pc = 0x8000_0000;
        let out = run(&mut h, &mut m, 1);
        assert_eq!(
            out[0],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::InstrAccessFault,
                    tval: 0x8000_0000
                },
                cycles: 1
            }
        );
        assert_eq!(h.csr.read(MEPC, h.cycles, h.instret), Some(0x8000_0000));
    }

    #[test]
    fn second_parcel_fetch_fault_reports_the_faulting_parcel_in_mtval() {
        // A 32-bit instruction whose low half is the last mapped halfword of RAM.
        let mut ram = Ram::new(0, 4);
        ram.load(2, &0x0093u16.to_le_bytes()).unwrap();
        let mut h = Hart::new(2, HartConfig::default());
        let out = h.step(&mut ram, &OneCycle);
        assert_eq!(
            out,
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::InstrAccessFault,
                    tval: 4
                },
                cycles: 1
            }
        );
        assert_eq!(h.csr.read(MEPC, h.cycles, h.instret), Some(2));
    }

    #[test]
    fn fetch_fault_costs_the_system_instruction_cycles() {
        #[derive(Debug)]
        struct CostlySystem;
        impl TimingModel for CostlySystem {
            fn name(&self) -> &'static str {
                "costly-system"
            }
            fn is_calibrated(&self) -> bool {
                false
            }
            fn instr_cycles(&self, class: InstrClass) -> u32 {
                if class == InstrClass::System { 5 } else { 1 }
            }
        }
        let (mut h, mut m) = machine(&[], MisalignedPolicy::Trap);
        h.pc = 0x8000_0000;
        let out = h.step(&mut m, &CostlySystem);
        assert!(
            matches!(out, StepOutcome::Trapped { cycles: 5, .. }),
            "{out:?}"
        );
        assert_eq!(h.cycles, Cycles::ZERO + 5);
    }

    #[test]
    fn store_into_decoded_page_invalidates() {
        // The program runs the instruction at 16 once (caching it), overwrites it with
        // `addi x5,x0,7`, then jumps back to it. Execution order: 0,4,8,12,16,20,24,16 = 8 steps.
        let patched = addi(5, 0, 7);
        let (mut h, mut m) = machine(
            &[
                addi(5, 0, 1),                                      // 0
                lui(2, patched.wrapping_add(0x800) & 0xffff_f000),  // 4
                addi(2, 2, ((patched & 0xfff) as i32) << 20 >> 20), // 8: x2 = patched word
                jal(0, 4),                                          // 12: skip to 16
                addi(5, 0, 1), // 16: original instruction (executed first)
                sw(2, 0, 16),  // 20: overwrite instruction at 16
                jal(0, -8),    // 24: back to 16
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut h, &mut m, 8);
        assert_eq!(
            h.x[5], 7,
            "the rewritten instruction must run, not the cached one"
        );
    }

    #[test]
    fn store_into_the_second_half_of_a_cached_instruction_invalidates_it_with_an_unaligned_ram_base()
     {
        // RAM at 0x1800..0x3800: the 4-byte `addi x5,x0,1` at 0x27FE sits inside the absolute
        // page 0x2000 but straddles the RAM-relative page boundary at 0x2800. Its high parcel
        // is rewritten with `sh` from code in another absolute page, so nothing else refreshes
        // the decode cache of page 0x2000: only `page_version` can invalidate the stale entry.
        let words =
            |program: &[u32]| -> Vec<u8> { program.iter().flat_map(|w| w.to_le_bytes()).collect() };
        let mut ram = Ram::new(0x1800, 0x2000);
        ram.load(
            0x1800,
            &words(&[
                lui(2, 0x2000),    // x2 = 0x2000
                addi(2, 2, 0x7fe), // x2 = 0x27fe
                jalr(0, 2, 0),     // run the straddling instruction
            ]),
        )
        .unwrap();
        ram.load(
            0x1810,
            &words(&[
                addi(3, 0, 0x70), // high parcel of `addi x5,x0,7` is 0x0070
                sh(3, 2, 2),      // rewrite the parcel at 0x2800
                jalr(0, 2, 0),    // run the rewritten instruction
            ]),
        )
        .unwrap();
        ram.load(0x2802, &words(&[jal(0, 0x1810 - 0x2802)]))
            .unwrap();
        // Loaded last: one write spanning both RAM-relative pages gives them the same version,
        // so the decode cache cannot be refreshed by accident when it first runs from 0x2802.
        ram.load(0x27fe, &addi(5, 0, 1).to_le_bytes()).unwrap();
        let mut h = Hart::new(0x1800, HartConfig::default());
        // 3 entry + straddling addi + jal + 3 patch + straddling addi again.
        run(&mut h, &mut ram, 9);
        assert_eq!(
            h.x[5], 7,
            "the rewritten instruction must run, not the cached one"
        );
        assert_eq!(h.pc, 0x2802);
    }

    #[test]
    fn cache_hit_still_pays_fetch_stall() {
        #[derive(Debug)]
        struct SlowFetch(Ram);
        impl Bus for SlowFetch {
            fn read(
                &mut self,
                a: u32,
                s: AccessSize,
                k: AccessKind,
            ) -> Result<emu_core::ReadOutcome, emu_core::BusError> {
                let mut o = self.0.read(a, s, k)?;
                if k == AccessKind::Fetch {
                    o.stall = 3;
                }
                Ok(o)
            }
            fn write(&mut self, a: u32, s: AccessSize, v: u32) -> Result<u32, emu_core::BusError> {
                self.0.write(a, s, v)
            }
            fn page_version(&self, a: u32) -> u64 {
                self.0.page_version(a)
            }
            fn fetch_stall(&mut self, _a: u32, _len: u32) -> u32 {
                3
            }
        }
        let (mut h, m) = machine(&[jal(0, 0)], MisalignedPolicy::Trap); // infinite self-loop
        let mut bus = SlowFetch(m);
        let first = h.step(&mut bus, &OneCycle);
        let second = h.step(&mut bus, &OneCycle);
        assert_eq!(first, StepOutcome::Retired { cycles: 1 + 6 }); // two 16-bit fetch reads at 3 each
        assert_eq!(second, StepOutcome::Retired { cycles: 1 + 3 }); // cached: one fetch_stall call
        assert_eq!(h.dcache.stats(), (1, 1));
    }

    #[test]
    fn store_faults_report_the_target_address() {
        let (mut h, mut m) = machine(
            &[
                lui(1, 0x8000_0000),
                sw(0, 1, 0),
                addi(2, 0, 0x202),
                sw(0, 2, 0),
            ],
            MisalignedPolicy::Trap,
        );
        let out = run(&mut h, &mut m, 2);
        assert!(matches!(
            out[1],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::StoreAccessFault,
                    tval: 0x8000_0000
                },
                ..
            }
        ));
        h.pc = 8;
        let out = run(&mut h, &mut m, 2);
        assert!(matches!(
            out[1],
            StepOutcome::Trapped {
                trap: Trap {
                    cause: Exception::StoreMisaligned,
                    tval: 0x202
                },
                ..
            }
        ));
    }

    #[test]
    fn trap_entry_and_mret_swap_mie_and_mpie() {
        let mut prog = vec![ecall()];
        prog.resize((TRAP_VECTOR / 4) as usize, 0x0000_0013);
        prog.push(mret());
        let (mut h, mut m) = machine(&prog, MisalignedPolicy::Trap);
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        run(&mut h, &mut m, 1);
        let status = h.csr.read(MSTATUS, h.cycles, h.instret).unwrap();
        assert_eq!(
            (status & MSTATUS_MIE, status & MSTATUS_MPIE),
            (0, MSTATUS_MPIE)
        );
        h.pc = TRAP_VECTOR;
        run(&mut h, &mut m, 1);
        let status = h.csr.read(MSTATUS, h.cycles, h.instret).unwrap();
        assert_eq!(status & MSTATUS_MIE, MSTATUS_MIE);
        assert_eq!(h.pc, 0);
    }

    #[test]
    fn wfi_resumes_without_trap_when_global_interrupts_are_masked() {
        let (mut h, mut m) = machine(&[wfi(), addi(1, 0, 1)], MisalignedPolicy::Trap);
        assert!(h.csr.write(MIE, 1 << 3));
        run(&mut h, &mut m, 1);
        assert_eq!(h.step(&mut m, &OneCycle), StepOutcome::Waiting);
        h.set_interrupt_pending(3, true);
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Retired { .. }
        ));
        assert_eq!(h.x[1], 1);
        h.set_interrupt_pending(3, false);
        assert_eq!(h.csr.mip, 0);
    }

    #[test]
    fn the_boundary_takes_a_pending_interrupt_without_executing_the_next_instruction() {
        let (mut h, mut m) = machine(&[addi(1, 0, 1), addi(2, 0, 2)], MisalignedPolicy::Trap);
        assert_eq!(h.take_boundary(), Boundary::Ready);
        run(&mut h, &mut m, 1);
        assert!(h.csr.write(MIE, 1 << 7));
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        h.set_interrupt_pending(7, true);
        assert_eq!(h.take_boundary(), Boundary::Interrupted { cause: 7 });
        assert_eq!(h.pc, TRAP_VECTOR, "the handler is next");
        assert_eq!(h.csr.mepc, 4, "the instruction it interrupted");
        assert_eq!((h.x[2], h.instret), (0, 1), "nothing else executed");
        // `MIE` is clear in the handler: nothing more stands before its first instruction.
        assert_eq!(h.take_boundary(), Boundary::Ready);
    }

    #[test]
    fn the_boundary_ends_a_wfi_on_a_line_even_with_interrupts_masked() {
        let (mut h, mut m) = machine(&[wfi(), addi(1, 0, 1)], MisalignedPolicy::Trap);
        assert!(h.csr.write(MIE, 1 << 3));
        run(&mut h, &mut m, 1);
        assert_eq!(h.take_boundary(), Boundary::Waiting);
        h.set_interrupt_pending(3, true);
        assert_eq!(h.take_boundary(), Boundary::Ready);
        assert!(!h.is_waiting());
        assert_eq!(
            (h.pc, h.x[1]),
            (4, 0),
            "the instruction after wfi has not run"
        );
    }

    #[test]
    fn the_last_instruction_is_the_one_just_executed_and_none_after_a_fetch_fault() {
        let (mut h, mut m) = machine(&[addi(1, 0, 1), jal(0, 0x2_0000)], MisalignedPolicy::Trap);
        assert_eq!(h.last_instruction(), None);
        run(&mut h, &mut m, 1);
        assert_eq!(h.last_instruction().map(|d| d.raw), Some(addi(1, 0, 1)));
        run(&mut h, &mut m, 1);
        assert_eq!(h.last_instruction().map(|d| d.raw), Some(jal(0, 0x2_0000)));
        // The jump left the 64 KiB of RAM: the next fetch faults.
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Trapped { .. }
        ));
        assert_eq!(h.last_instruction(), None);
    }

    fn c3_machine(program: &[u32]) -> (Hart, Ram) {
        let mut ram = Ram::new(0, 0x1000);
        for (i, word) in program.iter().enumerate() {
            ram.load(4 * u32::try_from(i).unwrap(), &word.to_le_bytes())
                .unwrap();
        }
        let config = HartConfig {
            core: Core::Esp32C3,
            ..HartConfig::default()
        };
        (Hart::new(0, config), ram)
    }

    #[test]
    fn esp32c3_cycle_counter_advances_between_two_reads() {
        // The ROM's ets_delay_us enables the counter through 0x800/0x801 and polls 0x802.
        let (mut hart, mut ram) = c3_machine(&[
            csrrwi(0, PCER_USER, 1),
            csrrwi(0, PCMR_USER, 1),
            csrrs(5, PCCR_USER, 0),
            addi(0, 0, 0),
            csrrs(6, PCCR_USER, 0),
        ]);
        run(&mut hart, &mut ram, 5);
        assert_eq!(hart.x[6] - hart.x[5], 2);
    }

    #[test]
    fn esp32c3_mpp_is_user_at_reset_machine_in_a_trap_and_user_after_mret() {
        // TRM §1.4.2: `mstatus` resets to 0; register 1.5: `MPP` is 0 (user) or 3 (machine).
        let mut program = vec![csrrs(5, MSTATUS, 0), ecall(), csrrs(8, MSTATUS, 0)];
        program.resize(0x100 / 4, addi(0, 0, 0));
        program.extend([
            csrrs(6, MSTATUS, 0),
            csrrs(7, MEPC, 0),
            addi(7, 7, 4),
            csrrw(0, MEPC, 7),
            mret(),
        ]);
        let (mut hart, mut ram) = c3_machine(&program);
        assert!(hart.csr.write(MTVEC, 0x100));
        run(&mut hart, &mut ram, 8);
        assert_eq!(hart.x[5], 0, "mstatus at reset");
        assert_eq!(hart.x[6] & MSTATUS_MPP_M, MSTATUS_MPP_M, "in the trap");
        assert_eq!(hart.x[8] & MSTATUS_MPP_M, 0, "after mret");
        assert_eq!(hart.pc, 12);
    }

    #[test]
    fn a_plain_hart_keeps_mpp_at_machine_after_mret() {
        let mut program = vec![ecall(), csrrs(8, MSTATUS, 0)];
        program.resize((TRAP_VECTOR / 4) as usize, addi(0, 0, 0));
        program.extend([csrrs(7, MEPC, 0), addi(7, 7, 4), csrrw(0, MEPC, 7), mret()]);
        let (mut hart, mut ram) = machine(&program, MisalignedPolicy::Trap);
        run(&mut hart, &mut ram, 6);
        assert_eq!(hart.x[8] & MSTATUS_MPP_M, MSTATUS_MPP_M);
    }

    /// A hart whose interrupts come from an outside controller, as on the ESP32-C3.
    fn controller_machine(program: &[u32]) -> (Hart, Ram) {
        let (_, ram) = c3_machine(program);
        let config = HartConfig {
            core: Core::Esp32C3,
            interrupts: InterruptModel::Controller,
            ..HartConfig::default()
        };
        (Hart::new(0, config), ram)
    }

    #[test]
    fn a_line_presented_by_the_controller_is_taken_whatever_mie_and_mip_say() {
        // IR §2.4: on the ESP32-C3 `mie` gates nothing; §3.3: the first interrupt of the
        // reference application is line 3, vector `mtvec.BASE + 12`, `mcause` 0x80000003.
        let (mut h, mut m) = controller_machine(&[addi(1, 0, 1), addi(1, 1, 1)]);
        assert!(h.csr.write(MTVEC, 0x800));
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        assert_eq!(h.csr.mie, 0, "no line is enabled in `mie`");
        run(&mut h, &mut m, 1);
        h.set_external_interrupt(Some(3));
        assert_eq!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Interrupted { cause: 3 }
        );
        assert_eq!(h.pc, 0x800 + 4 * 3);
        assert_eq!(h.csr.mcause, 0x8000_0003);
        assert_eq!(h.csr.mepc, 4);
        assert_eq!(h.csr.mip, 0, "`mip` is not driven by the controller");
        // `mstatus.MIE` is cleared on entry: the line stays presented and is not taken again.
        h.pc = 4;
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Retired { .. }
        ));
        // Bits of `mip` and `mie` alone raise nothing.
        h.set_external_interrupt(None);
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        assert!(h.csr.write(MIE, 1 << 7));
        h.set_interrupt_pending(7, true);
        h.pc = 0;
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Retired { .. }
        ));
    }

    #[test]
    fn wfi_ends_when_the_controller_presents_a_line_even_with_interrupts_masked() {
        // Decision E4; IR §2.6: the instruction after `wfi` runs, no trap is taken.
        let (mut h, mut m) = controller_machine(&[wfi(), addi(1, 0, 1)]);
        run(&mut h, &mut m, 1);
        assert!(h.is_waiting());
        assert_eq!(h.step(&mut m, &OneCycle), StepOutcome::Waiting);
        h.set_external_interrupt(Some(4));
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Retired { .. }
        ));
        assert_eq!(h.x[1], 1);
        assert!(!h.is_waiting());
    }

    #[test]
    fn the_standard_model_ignores_a_line_presented_by_a_controller() {
        let (mut h, mut m) = machine(&[addi(1, 0, 1)], MisalignedPolicy::Trap);
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        h.set_external_interrupt(Some(3));
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Retired { .. }
        ));
    }

    #[test]
    fn a_reset_returns_the_hart_to_its_reset_state_and_keeps_the_totals() {
        const PMPCFG0: u16 = 0x3A0;
        let (mut h, mut m) = controller_machine(&[addi(1, 0, 1), wfi()]);
        assert!(h.csr.write(MTVEC, 0x800));
        assert!(h.csr.write(PMPCFG0, 0x9F), "an entry with its lock bit");
        run(&mut h, &mut m, 2);
        assert!(h.is_waiting());
        h.set_external_interrupt(Some(5));
        let (cycles, instret) = (h.cycles, h.instret);
        assert_eq!((cycles.get(), instret), (2, 2));
        // What the reset has to undo is there before it: the locked entry (a write to it is
        // ignored), `mtvec`, a non-zero register, counters that read the run so far.
        assert!(h.csr.write(PMPCFG0, 0));
        assert_eq!(
            h.csr.read(PMPCFG0, h.cycles, h.instret),
            Some(0x9F),
            "the entry is locked: only a reset clears it"
        );
        assert_eq!(h.csr.mtvec, 0x801);
        assert_eq!(h.x[1], 1);
        assert_eq!(h.csr.read(MCYCLE, h.cycles, h.instret), Some(2));
        assert_ne!(h.decode_cache_stats(), (0, 0));
        h.reset(0x40);
        assert_eq!(h.pc, 0x40);
        assert_eq!(h.x, [0; 32]);
        assert!(!h.is_waiting());
        assert_eq!(
            h.csr.read(PMPCFG0, h.cycles, h.instret),
            Some(0),
            "the PMP entries and their locks are cleared"
        );
        assert_eq!(h.csr.mtvec, 1, "the ESP32-C3 reset value");
        assert_eq!(
            (h.cycles, h.instret),
            (cycles, instret),
            "totals of the run"
        );
        assert_eq!(
            (
                h.csr.read(MCYCLE, h.cycles, h.instret),
                h.csr.read(MINSTRET, h.cycles, h.instret)
            ),
            (Some(0), Some(0)),
            "software reads the counters of a new hart, not the age of the run"
        );
        assert_eq!(
            h.decode_cache_stats(),
            (0, 0),
            "the decode cache is new, its statistics with it"
        );
        // The presented line is forgotten: the controller presents it again if it still selects it.
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        h.pc = 0;
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Retired { .. }
        ));
        assert_eq!(
            (
                h.csr.read(MCYCLE, h.cycles, h.instret),
                h.csr.read(MINSTRET, h.cycles, h.instret),
                h.csr.read(MCYCLEH, h.cycles, h.instret),
            ),
            (Some(1), Some(1), Some(0)),
            "and they count on from the reset"
        );
    }

    #[test]
    fn a_line_the_hart_does_not_have_is_no_line() {
        // The cause is five bits wide and indexes the vector table: line 32 does not exist.
        let (mut h, mut m) = controller_machine(&[addi(1, 0, 1), addi(1, 1, 1)]);
        assert!(h.csr.write(MTVEC, 0x800));
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        for line in [32, 33, u32::MAX] {
            h.set_external_interrupt(Some(line));
            assert!(
                matches!(h.step(&mut m, &OneCycle), StepOutcome::Retired { .. }),
                "line {line}"
            );
            h.pc = 0;
        }
        h.set_external_interrupt(Some(31));
        assert_eq!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Interrupted { cause: 31 }
        );
        assert_eq!(h.pc, 0x800 + 4 * 31, "the last entry of the vector table");
    }

    #[test]
    fn a_line_still_presented_is_taken_again_right_after_mret() {
        // A level interrupt whose handler returns without acknowledging its source: `mret`
        // restores `MIE` and the line, which the controller still presents, is taken before the
        // instruction `mepc` points at. The handler sits at the vector of line 3.
        let mut program = vec![addi(1, 0, 1), addi(1, 1, 1)];
        program.resize(0x100 / 4 + 3, addi(0, 0, 0));
        program.push(mret());
        let (mut h, mut m) = controller_machine(&program);
        assert!(h.csr.write(MTVEC, 0x100));
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        h.set_external_interrupt(Some(3));
        for round in 0..3 {
            assert_eq!(
                h.step(&mut m, &OneCycle),
                StepOutcome::Interrupted { cause: 3 },
                "round {round}"
            );
            assert_eq!((h.pc, h.csr.mepc), (0x100 + 4 * 3, 0));
            assert_eq!(h.csr.mstatus & MSTATUS_MIE, 0, "masked in the handler");
            assert!(matches!(
                h.step(&mut m, &OneCycle),
                StepOutcome::Retired { .. }
            ));
            assert_eq!(h.pc, 0, "`mret` returned to the interrupted instruction");
            assert_ne!(h.csr.mstatus & MSTATUS_MIE, 0);
        }
        assert_eq!(
            h.x[1], 0,
            "which never ran: the line was taken first each time"
        );
        // Once the controller withdraws the line the program goes on.
        h.set_external_interrupt(None);
        assert!(matches!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Retired { .. }
        ));
        assert_eq!(h.x[1], 1);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "out of range")]
    fn an_interrupt_line_beyond_31_is_refused() {
        let (mut hart, _) = machine(&[], MisalignedPolicy::Trap);
        hart.set_interrupt_pending(32, true);
    }

    #[test]
    fn vendor_csrs_do_not_exist_on_a_generic_hart() {
        let (mut hart, mut ram) = machine(&[csrrs(5, PCCR_USER, 0)], MisalignedPolicy::Trap);
        let outcome = run(&mut hart, &mut ram, 1);
        assert!(
            matches!(outcome[0], StepOutcome::Trapped { trap, .. } if trap.cause == Exception::IllegalInstr)
        );
    }

    use crate::pmp::{PMPADDR0, PMPCFG0};
    /// `mstatus.TW` (TRM register 1.5).
    const MSTATUS_TW: u32 = 1 << 21;
    const USER_CODE: u32 = 0x40;
    const C3_TRAP_VECTOR: u32 = 0x100;
    /// `pmpcfg` byte: NAPOT, R, W, X, unlocked.
    const NAPOT_RWX: u32 = 0x1F;

    /// An ESP32-C3 hart about to `mret` into user mode at [`USER_CODE`], with an entry that
    /// grants user mode the whole address space and its trap vector at [`C3_TRAP_VECTOR`].
    fn c3_user(user_code: &[u32]) -> (Hart, Ram) {
        let mut program = vec![mret()];
        program.resize((USER_CODE / 4) as usize, addi(0, 0, 0));
        program.extend_from_slice(user_code);
        let (mut h, ram) = c3_machine(&program);
        assert!(h.csr.write(MTVEC, C3_TRAP_VECTOR));
        assert!(h.csr.write(MEPC, USER_CODE));
        assert!(h.csr.write(PMPADDR0, u32::MAX));
        assert!(h.csr.write(PMPCFG0, NAPOT_RWX));
        (h, ram)
    }

    fn trapped(outcome: StepOutcome) -> Trap {
        match outcome {
            StepOutcome::Trapped { trap, .. } => trap,
            other => panic!("expected a trap, got {other:?}"),
        }
    }

    #[test]
    fn mret_with_mpp_0_enters_user_mode_where_a_machine_csr_is_an_illegal_instruction() {
        let (mut h, mut m) = c3_user(&[csrrs(6, MSTATUS, 0)]);
        assert_eq!(
            h.privilege(),
            Privilege::Machine,
            "a reset starts in machine mode"
        );
        run(&mut h, &mut m, 1);
        assert_eq!((h.privilege(), h.pc), (Privilege::User, USER_CODE));
        let trap = trapped(h.step(&mut m, &OneCycle));
        assert_eq!(trap.cause, Exception::IllegalInstr);
        assert_eq!(trap.tval, csrrs(6, MSTATUS, 0));
        assert_eq!(h.x[6], 0, "nothing was read");
        assert_eq!(h.privilege(), Privilege::Machine);
        assert_eq!((h.pc, h.csr.mepc), (C3_TRAP_VECTOR, USER_CODE));
        assert_eq!(
            h.csr.mstatus & MSTATUS_MPP_M,
            0,
            "MPP says the trap came from user mode"
        );
    }

    #[test]
    fn ecall_in_user_mode_is_cause_8_and_a_user_csr_is_reachable() {
        let (mut h, mut m) = c3_user(&[csrrwi(0, PCER_USER, 1), ecall()]);
        run(&mut h, &mut m, 2);
        let trap = trapped(h.step(&mut m, &OneCycle));
        assert_eq!(trap.cause, Exception::EcallU);
        assert_eq!(h.csr.mcause, 8);
        assert_eq!(h.csr.read(PCER_USER, h.cycles, h.instret), Some(1));
    }

    #[test]
    fn mret_in_user_mode_is_an_illegal_instruction() {
        let (mut h, mut m) = c3_user(&[mret()]);
        run(&mut h, &mut m, 1);
        let trap = trapped(h.step(&mut m, &OneCycle));
        assert_eq!((trap.cause, trap.tval), (Exception::IllegalInstr, mret()));
    }

    #[test]
    fn wfi_in_user_mode_waits_unless_tw_makes_it_an_illegal_instruction() {
        let (mut h, mut m) = c3_user(&[wfi()]);
        run(&mut h, &mut m, 2);
        assert!(h.is_waiting(), "TW clear");
        let (mut h, mut m) = c3_user(&[wfi()]);
        assert!(h.csr.write(MSTATUS, MSTATUS_TW));
        run(&mut h, &mut m, 1);
        let trap = trapped(h.step(&mut m, &OneCycle));
        assert_eq!((trap.cause, trap.tval), (Exception::IllegalInstr, wfi()));
    }

    #[test]
    fn a_line_is_taken_in_user_mode_whatever_mstatus_mie_says() {
        let mut program = vec![mret()];
        program.resize((USER_CODE / 4) as usize, addi(0, 0, 0));
        program.extend([addi(1, 0, 1), addi(1, 1, 1)]);
        let (mut h, mut m) = controller_machine(&program);
        assert!(h.csr.write(MTVEC, C3_TRAP_VECTOR));
        assert!(h.csr.write(MEPC, USER_CODE));
        assert!(h.csr.write(PMPADDR0, u32::MAX));
        assert!(h.csr.write(PMPCFG0, NAPOT_RWX));
        run(&mut h, &mut m, 2);
        assert_eq!(h.csr.mstatus & MSTATUS_MIE, 0, "MPIE was clear at the mret");
        h.set_external_interrupt(Some(3));
        assert_eq!(
            h.step(&mut m, &OneCycle),
            StepOutcome::Interrupted { cause: 3 }
        );
        assert_eq!(h.privilege(), Privilege::Machine);
        assert_eq!(h.csr.mstatus & MSTATUS_MPP_M, 0);
        assert_eq!(h.x[1], 1);
    }

    #[test]
    fn user_mode_cannot_fetch_where_no_entry_grants_it() {
        let (mut h, mut m) = c3_user(&[addi(1, 0, 1)]);
        assert!(h.csr.write(PMPCFG0, 0));
        run(&mut h, &mut m, 1);
        let trap = trapped(h.step(&mut m, &OneCycle));
        assert_eq!(
            (trap.cause, trap.tval),
            (Exception::InstrAccessFault, USER_CODE)
        );
        assert_eq!(h.x[1], 0);
        assert_eq!(h.last_instruction(), None, "nothing was fetched");
    }

    #[test]
    fn a_locked_entry_refuses_a_store_of_machine_mode_and_the_memory_keeps_its_value() {
        // [0x800, 0x1000) read-only for every mode; the program below it runs unchecked.
        let (mut h, mut m) = c3_machine(&[
            addi(5, 0, 0x7FF),
            addi(5, 5, 1),
            addi(6, 0, 0x55),
            sw(6, 5, 0),
            lw(7, 5, 0),
        ]);
        assert!(h.csr.write(MTVEC, C3_TRAP_VECTOR));
        m.write(0x800, AccessSize::Word, 0x1234).unwrap();
        assert!(h.csr.write(PMPADDR0, 0x800 >> 2));
        assert!(h.csr.write(PMPADDR0 + 1, 0x1000 >> 2));
        assert!(h.csr.write(PMPCFG0, (0x80 | 0x08 | 0x01) << 8));
        run(&mut h, &mut m, 3);
        let trap = trapped(h.step(&mut m, &OneCycle));
        assert_eq!(
            (trap.cause, trap.tval),
            (Exception::StoreAccessFault, 0x800)
        );
        assert_eq!(h.csr.mcause, 7);
        h.pc = 16;
        run(&mut h, &mut m, 1);
        assert_eq!(
            h.x[7], 0x1234,
            "the load is granted, the store wrote nothing"
        );
    }

    #[test]
    fn a_cached_decoding_does_not_bypass_a_pmp_written_after_it() {
        let (mut h, mut m) = c3_machine(&[addi(1, 1, 1)]);
        assert!(h.csr.write(MTVEC, C3_TRAP_VECTOR));
        run(&mut h, &mut m, 1);
        h.pc = 0;
        run(&mut h, &mut m, 1);
        assert_eq!(h.x[1], 2);
        let (hits, _) = h.decode_cache_stats();
        assert!(hits > 0, "the instruction is cached");
        // Lock the word at 0 without X: machine mode may no longer fetch it.
        assert!(h.csr.write(PMPCFG0, 0x80 | 0x10 | 0x01));
        h.pc = 0;
        let trap = trapped(h.step(&mut m, &OneCycle));
        assert_eq!((trap.cause, trap.tval), (Exception::InstrAccessFault, 0));
        assert_eq!(h.x[1], 2);
    }

    #[test]
    fn the_second_parcel_of_an_instruction_is_checked_on_its_own_cached_or_not() {
        // `c.nop` at 0, a 32-bit `addi` at 2 whose second parcel is the word at 4, which a
        // locked NA4 entry keeps machine mode from fetching.
        let mut ram = Ram::new(0, 0x1000);
        let mut bytes = vec![0x01, 0x00];
        bytes.extend(addi(1, 1, 1).to_le_bytes());
        ram.load(0, &bytes).unwrap();
        let mut h = Hart::new(
            2,
            HartConfig {
                core: Core::Esp32C3,
                ..HartConfig::default()
            },
        );
        assert!(h.csr.write(MTVEC, C3_TRAP_VECTOR));
        run(&mut h, &mut ram, 1);
        assert_eq!(h.x[1], 1, "decoded and cached before the PMP changes");
        assert!(h.csr.write(PMPADDR0, 1));
        assert!(h.csr.write(PMPCFG0, 0x80 | 0x10 | 0x01));
        h.pc = 2;
        let trap = trapped(h.step(&mut ram, &OneCycle));
        assert_eq!(
            (trap.cause, trap.tval),
            (Exception::InstrAccessFault, 4),
            "cached"
        );
        let mut fresh = Hart::new(2, h.config);
        assert!(fresh.csr.write(MTVEC, C3_TRAP_VECTOR));
        assert!(fresh.csr.write(PMPADDR0, 1));
        assert!(fresh.csr.write(PMPCFG0, 0x80 | 0x10 | 0x01));
        let trap = trapped(fresh.step(&mut ram, &OneCycle));
        assert_eq!(
            (trap.cause, trap.tval),
            (Exception::InstrAccessFault, 4),
            "not cached"
        );
    }

    #[test]
    fn on_the_bench_mprv_checks_a_load_of_machine_mode_as_mpps_mode() {
        let mut ram = Ram::new(0, 0x1000);
        ram.load(0, &lw(7, 0, 0x400).to_le_bytes()).unwrap();
        let mut h = Hart::new(
            0,
            HartConfig {
                core: Core::MachineUser,
                ..HartConfig::default()
            },
        );
        assert!(h.csr.write(MTVEC, TRAP_VECTOR));
        // MPRV with MPP = user: the load is a user-mode access, and no entry grants it.
        assert!(h.csr.write(MSTATUS, 1 << 17));
        let trap = trapped(h.step(&mut ram, &OneCycle));
        assert_eq!((trap.cause, trap.tval), (Exception::LoadAccessFault, 0x400));
        // MPRV with MPP = machine: machine mode, which no entry restricts.
        h.pc = 0;
        assert!(h.csr.write(MSTATUS, (1 << 17) | MSTATUS_MPP_M));
        assert!(matches!(
            h.step(&mut ram, &OneCycle),
            StepOutcome::Retired { .. }
        ));
    }

    #[test]
    fn a_reset_puts_the_hart_back_in_machine_mode() {
        let (mut h, mut m) = c3_user(&[addi(1, 0, 1)]);
        run(&mut h, &mut m, 1);
        assert_eq!(h.privilege(), Privilege::User);
        h.reset(0);
        assert_eq!(h.privilege(), Privilege::Machine);
        assert_eq!(h.csr.read(PMPCFG0, h.cycles, h.instret), Some(0));
    }

    #[test]
    fn mcycle_write_takes_precedence_over_the_cycles_of_the_writing_instruction() {
        let (mut hart, mut ram) = machine(
            &[
                addi(5, 0, 100),
                csrrw(0, MCYCLE, 5),
                csrrs(6, MCYCLE, 0),
                csrrs(7, MCYCLE, 0),
            ],
            MisalignedPolicy::Trap,
        );
        run(&mut hart, &mut ram, 4);
        assert_eq!(hart.x[6], 100);
        assert_eq!(hart.x[7], 101);
    }
}
