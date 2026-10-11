//! The ESP32-C3 as a machine: one hart, its bus, and the loop that runs them.
use crate::bus::SocBus;
use crate::config::SocConfig;
use crate::diag::Diagnostic;
use crate::memmap::RESET_PC;
use crate::rom::RomImage;
use cpu_rv32::{
    DatasheetTiming, Exception, Hart, HartConfig, MisalignedPolicy, StepOutcome, VendorCsrs,
};
use emu_core::{Cycles, TraceHash};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleChannel {
    Uart0,
    UsbSerialJtag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The cycle limit passed to [`Esp32C3::run_until`] was reached.
    CycleBudget,
    /// `--strict-mmio`: the program touched a register the emulator does not model.
    StrictMmio(Diagnostic),
    /// The CPU is in `wfi` and nothing can raise an interrupt yet (interrupt sources arrive in SP1b-3).
    Idle,
}

#[derive(Debug)]
pub struct Esp32C3 {
    hart: Hart,
    bus: SocBus,
    timing: DatasheetTiming,
    trace: Option<TraceHash>,
}

impl Esp32C3 {
    /// A machine in its power-on reset state, about to execute the first ROM instruction.
    #[must_use]
    pub fn new(rom: RomImage, config: &SocConfig) -> Esp32C3 {
        // Misaligned loads and stores are performed, not trapped. Evidence: the rev0 mask ROM's `_init`
        // executes `lw t0,0(a5)` at pc 0x40001e00 from address 0x40058c3e (2 mod 4), the first byte
        // after `.wifi0iram.3` where the linker placed the initial `.data` image, so real silicon
        // performs misaligned accesses; ESP-IDF treats them as functional but slow
        // (`docs/research/esp32c3-boot-chain.md` §B.1).
        // Known gap: the extra cycles a misaligned access costs on silicon are not modelled yet, and a
        // misaligned access to a peripheral register is still a bus error.
        let hart_config = HartConfig {
            misaligned: MisalignedPolicy::Allow,
            vendor_csrs: VendorCsrs::Esp32C3,
        };
        Esp32C3 {
            hart: Hart::new(RESET_PC, hart_config),
            bus: SocBus::new(rom, config),
            timing: DatasheetTiming,
            trace: config.trace_hash.then(TraceHash::new),
        }
    }

    /// Runs until the hart's cycle counter reaches `limit` (an absolute [`Esp32C3::cycles`] value) or
    /// the machine stops for another reason.
    pub fn run_until(&mut self, limit: Cycles) -> StopReason {
        while self.hart.cycles.get() < limit.get() {
            let pc = self.hart.pc;
            self.bus.set_pc(pc);
            match self.hart.step(&mut self.bus, &self.timing) {
                StepOutcome::Waiting => return StopReason::Idle,
                // An `ecall` is a request to the trap handler, not a fault worth reporting.
                StepOutcome::Trapped { trap, .. } if trap.cause != Exception::EcallM => {
                    self.bus.diag.record(Diagnostic::Exception {
                        cause: trap.cause,
                        tval: trap.tval,
                        pc,
                    });
                }
                _ => {}
            }
            if let Some(trace) = &mut self.trace {
                trace.mix_u32(pc);
            }
            if let Some(violation) = self.bus.diag.take_violation() {
                return StopReason::StrictMmio(violation);
            }
        }
        StopReason::CycleBudget
    }

    /// CPU clock cycles elapsed since reset.
    #[must_use]
    pub const fn cycles(&self) -> Cycles {
        self.hart.cycles
    }

    #[must_use]
    pub const fn instructions(&self) -> u64 {
        self.hart.instret
    }

    #[must_use]
    pub const fn pc(&self) -> u32 {
        self.hart.pc
    }

    /// The bytes the firmware sent on `channel` since the last call.
    pub fn take_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
        match channel {
            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(),
            ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_tx(),
        }
    }

    /// Diagnostics seen for the first time since the last call.
    pub fn drain_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.bus.diag.drain()
    }

    /// Fingerprint of the executed PC sequence, when [`SocConfig::trace_hash`] is set.
    #[must_use]
    pub fn trace_hash(&self) -> Option<u64> {
        self.trace.as_ref().map(TraceHash::finish)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memmap::ROM_BASE;
    use crate::memmap::SRAM_DBUS_BASE;
    use crate::test_support::flat_rom;
    use cpu_rv32::asm::{addi, csrrs, csrrwi, jal, lui, lw, sw, wfi};
    use emu_core::{AccessKind, AccessSize, Bus};

    const T0: u8 = 5;
    const T1: u8 = 6;
    /// `j .`: the program parks itself.
    const HANG: u32 = 0x0000_006f;

    fn machine_with(program: &[u32], config: SocConfig) -> Esp32C3 {
        Esp32C3::new(flat_rom(program), &config)
    }
    fn machine(program: &[u32]) -> Esp32C3 {
        machine_with(program, SocConfig::default())
    }
    /// An aligned word read straight off the bus, as a later aligned load would see it.
    fn bus_word(m: &mut Esp32C3, addr: u32) -> u32 {
        m.bus
            .read(addr, AccessSize::Word, AccessKind::Load)
            .unwrap()
            .value
    }
    fn print(base: u32, text: &[u8]) -> Vec<u32> {
        let mut program = vec![lui(T0, base)];
        for &byte in text {
            program.extend([addi(T1, 0, i32::from(byte)), sw(T1, T0, 0)]);
        }
        program
    }

    #[test]
    fn hang_instruction_is_what_the_assembler_produces() {
        assert_eq!(jal(0, 0), HANG);
    }

    #[test]
    fn starts_at_the_reset_vector_and_prints_through_uart0() {
        let mut program = print(0x6000_0000, b"Hi");
        program.push(HANG);
        let mut m = machine(&program);
        assert_eq!(m.pc(), ROM_BASE);
        assert_eq!(m.run_until(Cycles(500)), StopReason::CycleBudget);
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"Hi");
        assert_eq!(
            m.take_console(ConsoleChannel::Uart0),
            Vec::<u8>::new(),
            "taking the console empties it"
        );
        assert!(m.cycles().get() >= 500);
        assert_eq!(m.drain_diagnostics(), Vec::new());
    }

    #[test]
    fn usb_serial_jtag_console_is_a_separate_channel() {
        let mut program = print(0x6004_3000, b"ok");
        program.extend([addi(T1, 0, 1), sw(T1, T0, 4), HANG]);
        let mut m = machine(&program);
        m.run_until(Cycles(500));
        assert_eq!(m.take_console(ConsoleChannel::UsbSerialJtag), b"ok");
        assert_eq!(
            m.take_console(ConsoleChannel::Uart0),
            Vec::<u8>::new(),
            "nothing went to UART0"
        );
    }

    #[test]
    fn misaligned_word_load_from_the_rom_returns_the_little_endian_value() {
        // Bytes 11 22 33 44 sit at ROM offset 0x1006 (2 mod 4), read through the instruction bus: the
        // access the rev0 ROM's `_init` performs.
        const DATA_OFFSET: u32 = 0x1006;
        let mut program = vec![
            lui(T0, ROM_BASE + DATA_OFFSET),
            lw(T1, T0, 6),
            lui(7, SRAM_DBUS_BASE),
            sw(T1, 7, 0),
            HANG,
        ];
        program.resize(0x1004 / 4, 0);
        program.extend([0x2211_0000, 0x0000_4433]);
        let mut m = machine(&program);
        m.run_until(Cycles(200));
        assert_eq!(m.drain_diagnostics(), Vec::new());
        assert_eq!(bus_word(&mut m, SRAM_DBUS_BASE), 0x4433_2211);
    }

    #[test]
    fn misaligned_word_store_to_sram_lands_little_endian_across_two_words() {
        let mut m = machine(&[
            lui(T0, SRAM_DBUS_BASE),
            lui(T1, 0xAABB_C000),
            addi(T1, T1, 0xDD),
            sw(T1, T0, 2),
            HANG,
        ]);
        m.run_until(Cycles(300));
        assert_eq!(m.drain_diagnostics(), Vec::new());
        // 0xAABBC0DD stored at byte offset 2: DD C0 at bytes 2..4, BB AA at bytes 4..6.
        assert_eq!(bus_word(&mut m, SRAM_DBUS_BASE), 0xC0DD_0000, "first word");
        assert_eq!(
            bus_word(&mut m, SRAM_DBUS_BASE + 4),
            0x0000_AABB,
            "second word"
        );
    }

    #[test]
    fn budget_is_absolute_so_a_run_can_be_resumed() {
        let mut m = machine(&[HANG]);
        assert_eq!(m.run_until(Cycles(100)), StopReason::CycleBudget);
        let first = m.cycles().get();
        assert_eq!(
            m.run_until(Cycles(100)),
            StopReason::CycleBudget,
            "already past the limit: nothing runs"
        );
        assert_eq!(m.cycles().get(), first);
        m.run_until(Cycles(300));
        assert!(m.cycles().get() >= 300);
    }

    #[test]
    fn the_rom_delay_counter_is_available() {
        // What ets_delay_us relies on: enable through 0x800/0x801, then watch 0x802 advance.
        let mut m = machine(&[
            csrrwi(0, 0x800, 1),
            csrrwi(0, 0x801, 1),
            csrrs(T0, 0x802, 0),
            csrrs(T1, 0x802, 0),
            HANG,
        ]);
        m.run_until(Cycles(200));
        assert!(
            m.drain_diagnostics().is_empty(),
            "no illegal-instruction exception"
        );
    }

    #[test]
    fn unmodeled_access_is_reported_with_the_pc_of_the_instruction() {
        let ledc = svd_regs::esp32c3::registers()
            .iter()
            .find(|r| r.peripheral == "LEDC")
            .unwrap();
        let mut m = machine(&[lui(T0, ledc.addr), lw(T1, T0, 0), HANG]);
        assert_eq!(m.run_until(Cycles(200)), StopReason::CycleBudget);
        let listed = m.drain_diagnostics();
        assert!(
            matches!(listed[..], [Diagnostic::UnmodeledMmio { pc, .. }] if pc == ROM_BASE + 4),
            "{listed:?}"
        );
    }

    #[test]
    fn strict_mode_stops_at_the_unmodeled_access() {
        let ledc = svd_regs::esp32c3::registers()
            .iter()
            .find(|r| r.peripheral == "LEDC")
            .unwrap();
        let config = SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        };
        let mut m = machine_with(&[lui(T0, ledc.addr), lw(T1, T0, 0), HANG], config);
        let StopReason::StrictMmio(diagnostic) = m.run_until(Cycles(10_000)) else {
            panic!("expected a strict stop")
        };
        assert!(diagnostic.to_string().contains("LEDC"), "{diagnostic}");
        assert!(m.cycles().get() < 100, "stopped right after the access");
    }

    #[test]
    fn wfi_with_nothing_to_wake_it_stops_as_idle() {
        let mut m = machine(&[wfi(), HANG]);
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Idle);
    }

    #[test]
    fn exception_loop_is_reported_once_per_site() {
        // An all-zero word is an illegal instruction; mtvec is 0, so the handler address is unmapped
        // and the hart then faults on every fetch at pc 0.
        let mut m = machine(&[0]);
        assert_eq!(m.run_until(Cycles(5_000)), StopReason::CycleBudget);
        let exceptions: Vec<_> = m
            .drain_diagnostics()
            .into_iter()
            .filter(|d| matches!(d, Diagnostic::Exception { .. }))
            .collect();
        assert_eq!(exceptions.len(), 2, "{exceptions:?}");
        assert!(
            matches!(exceptions[0], Diagnostic::Exception { cause: Exception::IllegalInstr, pc, .. } if pc == ROM_BASE)
        );
        assert!(matches!(
            exceptions[1],
            Diagnostic::Exception {
                cause: Exception::InstrAccessFault,
                pc: 0,
                ..
            }
        ));
    }

    #[test]
    fn same_program_gives_the_same_trace_hash_and_another_program_a_different_one() {
        let config = SocConfig {
            trace_hash: true,
            ..SocConfig::default()
        };
        let run = |program: &[u32]| {
            let mut m = machine_with(program, config);
            m.run_until(Cycles(2_000));
            (m.trace_hash().unwrap(), m.instructions(), m.cycles())
        };
        let a = print(0x6000_0000, b"A");
        let b = print(0x6000_0000, b"AB");
        assert_eq!(run(&a), run(&a));
        assert_ne!(run(&a).0, run(&b).0);
        assert_eq!(machine(&a).trace_hash(), None, "off unless asked for");
    }
}
