//! A single machine-mode RV32IMC hart.
use crate::csr::{CsrFile, MSTATUS_MIE, MSTATUS_MPIE};
use crate::decode::decode;
use crate::trap::{Exception, Trap};
use emu_core::{AccessKind, AccessSize, Bus, Cycles, InstrClass, TimingModel};

const MCAUSE_INTERRUPT: u32 = 1 << 31;
const MTVEC_MODE_MASK: u32 = 0b11;
const MTVEC_VECTORED: u32 = 1;
const COMPRESSED_MASK: u32 = 0b11;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct HartConfig {
    pub misaligned: MisalignedPolicy,
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
    pub(crate) waiting: bool,
}

impl Hart {
    #[must_use]
    pub fn new(reset_pc: u32, config: HartConfig) -> Hart {
        Hart {
            x: [0; 32],
            pc: reset_pc,
            csr: CsrFile::default(),
            cycles: Cycles::ZERO,
            instret: 0,
            config,
            waiting: false,
        }
    }

    /// Drives one `mip` bit; `line` is taken modulo 32.
    pub fn set_interrupt_pending(&mut self, line: u32, pending: bool) {
        let bit = 1u32 << (line & 31);
        if pending {
            self.csr.mip |= bit;
        } else {
            self.csr.mip &= !bit;
        }
    }

    /// Highest-numbered line that is pending and enabled (`ESP32-C3` priorities are applied by the `SoC` in `SP1b`).
    fn pending_line(&self) -> Option<u32> {
        let ready = self.csr.mip & self.csr.mie;
        ready.checked_ilog2()
    }

    /// Executes one instruction, or takes one pending interrupt, or reports that the hart is waiting.
    pub fn step(&mut self, bus: &mut dyn Bus, timing: &dyn TimingModel) -> StepOutcome {
        if self.waiting {
            if self.csr.mip & self.csr.mie == 0 {
                return StepOutcome::Waiting;
            }
            // wfi resumes on any enabled pending interrupt, even when mstatus.MIE masks it.
            self.waiting = false;
        }
        if self.csr.mstatus & MSTATUS_MIE != 0
            && let Some(line) = self.pending_line()
        {
            self.enter_trap(MCAUSE_INTERRUPT | line, 0);
            return StepOutcome::Interrupted { cause: line };
        }
        let (raw, fetch_stall) = match self.fetch(bus) {
            Ok(fetched) => fetched,
            Err(trap) => return self.trap(trap, 1),
        };
        let decoded = decode(raw);
        match self.execute(decoded, bus) {
            Ok(done) => {
                let cycles = timing.instr_cycles(done.class) + fetch_stall + done.stall;
                self.cycles = self.cycles + u64::from(cycles);
                self.instret += 1;
                self.pc = done.next_pc;
                self.waiting = done.wait;
                StepOutcome::Retired { cycles }
            }
            Err(trap) => self.trap(trap, timing.instr_cycles(InstrClass::System) + fetch_stall),
        }
    }

    /// Fetches one parcel or two, so a 32-bit instruction may straddle a page or region boundary.
    fn fetch(&self, bus: &mut dyn Bus) -> Result<(u32, u32), Trap> {
        let fault = |_| Trap {
            cause: Exception::InstrAccessFault,
            tval: self.pc,
        };
        let lo = bus
            .read(self.pc, AccessSize::Half, AccessKind::Fetch)
            .map_err(fault)?;
        if lo.value & COMPRESSED_MASK != COMPRESSED_MASK {
            return Ok((lo.value, lo.stall));
        }
        let hi = bus
            .read(self.pc.wrapping_add(2), AccessSize::Half, AccessKind::Fetch)
            .map_err(fault)?;
        Ok((lo.value | (hi.value << 16), lo.stall + hi.stall))
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
        let base = csr.mtvec & !MTVEC_MODE_MASK;
        let vectored =
            csr.mtvec & MTVEC_MODE_MASK == MTVEC_VECTORED && mcause & MCAUSE_INTERRUPT != 0;
        self.pc = if vectored {
            base.wrapping_add(VECTOR_ENTRY_BYTES * (mcause & !MCAUSE_INTERRUPT))
        } else {
            base
        };
    }

    /// `mret`: restores `MIE` from `MPIE`, sets `MPIE`, and returns the address to resume at.
    pub(crate) fn return_from_trap(&mut self) -> u32 {
        let csr = &mut self.csr;
        let mpie = csr.mstatus & MSTATUS_MPIE != 0;
        csr.mstatus =
            (csr.mstatus & !MSTATUS_MIE) | MSTATUS_MPIE | if mpie { MSTATUS_MIE } else { 0 };
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
        let mut hart = Hart::new(0, HartConfig { misaligned: policy });
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
}
