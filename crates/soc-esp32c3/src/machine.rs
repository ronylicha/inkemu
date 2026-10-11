//! The ESP32-C3 as a machine: one hart, its bus, and the loop that runs them.
use crate::bus::SocBus;
use crate::clock;
use crate::config::SocConfig;
use crate::diag::{Diagnostic, Unsupported};
use crate::flash::{FlashDevice, FlashImageError, NorFlash};
use crate::memmap::RESET_PC;
use crate::rom::RomImage;
use cpu_rv32::{
    DatasheetTiming, Exception, Hart, HartConfig, MisalignedPolicy, StepOutcome, VendorCsrs,
};
use emu_core::{Cycles, Frequency, TimingModel, TraceHash};

/// The two serial consoles the mask ROM prints on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleChannel {
    Uart0,
    UsbSerialJtag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The cycle limit passed to [`Esp32C3::run_until`] was reached.
    CycleBudget,
    /// `--strict-mmio`: the program touched a register the emulator does not model, wrote a
    /// peripheral register with an 8- or 16-bit store, asked for an operation the emulator cannot
    /// perform faithfully, or used flash it had not mapped.
    StrictMmio(Diagnostic),
    /// The CPU is in `wfi` and nothing can raise an interrupt yet (interrupt sources arrive in SP1b-3).
    Idle,
}

/// An ESP32-C3: one hart, the bus with everything it reaches, and the model that prices each
/// instruction. It performs no host I/O: output is handed over through [`Esp32C3::take_console`].
#[derive(Debug)]
pub struct Esp32C3 {
    hart: Hart,
    bus: SocBus,
    timing: DatasheetTiming,
    trace: Option<TraceHash>,
}

impl Esp32C3 {
    /// A machine in its power-on reset state with an erased flash, about to execute the first ROM
    /// instruction.
    #[must_use]
    pub fn new(rom: RomImage, config: &SocConfig) -> Esp32C3 {
        Esp32C3::build(rom, Box::new(NorFlash::blank(config.flash)), config)
    }

    /// A machine whose flash holds `image` from offset 0; what the image does not cover is erased.
    ///
    /// # Errors
    /// [`FlashImageError`] when the image is longer than the flash.
    pub fn with_flash(
        rom: RomImage,
        image: Vec<u8>,
        config: &SocConfig,
    ) -> Result<Esp32C3, FlashImageError> {
        Ok(Esp32C3::build(
            rom,
            Box::new(NorFlash::with_image(image, config.flash)?),
            config,
        ))
    }

    fn build(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) -> Esp32C3 {
        // Misaligned loads and stores are performed, not trapped. Evidence: the rev0 mask ROM's `_init`
        // executes `lw t0,0(a5)` at pc 0x40001e00 from address 0x40058c3e (2 mod 4), the first byte
        // after `.wifi0iram.3` where the linker placed the initial `.data` image, so real silicon
        // performs misaligned accesses; ESP-IDF treats them as functional but slow
        // (`docs/research/esp32c3-boot-chain.md` §B.1).
        // ASSUMPTION: a misaligned access costs no extra cycle (a misaligned access to a peripheral
        // register is a bus error and raises an access fault). Silicon may charge extra cycles; a
        // silicon trace or the SP1b-4 lockstep settles it.
        let hart_config = HartConfig {
            misaligned: MisalignedPolicy::Allow,
            vendor_csrs: VendorCsrs::Esp32C3,
        };
        let mut bus = SocBus::new(rom, flash, config);
        let (sysclk_conf, cpu_per_conf) = bus.periph.system.clock_registers();
        // INVARIANT: the reset values of the two clock registers select the crystal with a divider
        // of 2 (decisions D21 and E8), which is a selection `cpu_clock` knows: the fallback is never
        // taken, and it is that same power-on clock.
        bus.ctx.set_clock(
            clock::cpu_clock(sysclk_conf, cpu_per_conf).unwrap_or(clock::CpuClock::POWER_ON),
        );
        Esp32C3 {
            hart: Hart::new(RESET_PC, hart_config),
            bus,
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
            let before = self.hart.cycles.get();
            match self.hart.step(&mut self.bus, &self.timing) {
                StepOutcome::Waiting => return StopReason::Idle,
                // An `ecall` is a request to the trap handler, not a fault worth reporting.
                StepOutcome::Trapped { trap, .. } if trap.cause != Exception::EcallM => {
                    self.bus.ctx.diag.record(Diagnostic::Exception {
                        cause: trap.cause,
                        tval: trap.tval,
                        pc,
                    });
                }
                _ => {}
            }
            // Device time follows the CPU: the cycles of this instruction, at the clock it ran on.
            self.bus.ctx.advance(self.hart.cycles.get() - before);
            if self.bus.ctx.take_clock_written() {
                self.retune(pc);
            }
            if let Some(trace) = &mut self.trace {
                trace.mix_u32(pc);
            }
            if let Some(violation) = self.bus.ctx.diag.take_violation() {
                return StopReason::StrictMmio(violation);
            }
        }
        StopReason::CycleBudget
    }

    /// Applies a write to the clock registers: from the end of the instruction that wrote them,
    /// cycles last what the new selection says.
    fn retune(&mut self, pc: u32) {
        let (sysclk_conf, cpu_per_conf) = self.bus.periph.system.clock_registers();
        match clock::cpu_clock(sysclk_conf, cpu_per_conf) {
            Some(selected) => self.bus.ctx.set_clock(selected),
            // Decision D22: the previous frequency stays, and the selection is reported.
            None => self.bus.ctx.diag.record(Diagnostic::Unsupported {
                what: Unsupported::ClockSource {
                    sysclk_conf,
                    cpu_per_conf,
                },
                pc,
            }),
        }
    }

    /// The CPU clock frequency the firmware selected.
    #[must_use]
    pub fn cpu_frequency(&self) -> Frequency {
        self.bus.ctx.clock().cpu_frequency()
    }

    /// Time elapsed on the device since power-on, following every CPU clock switch.
    #[must_use]
    pub fn device_time_nanos(&self) -> u64 {
        clock::TIME_BASE.cycles_to_nanos(self.bus.ctx.now())
    }

    /// CPU clock cycles elapsed since reset.
    #[must_use]
    pub const fn cycles(&self) -> Cycles {
        self.hart.cycles
    }

    /// Instructions retired since reset.
    #[must_use]
    pub const fn instructions(&self) -> u64 {
        self.hart.instret
    }

    /// The program counter: the address of the next instruction to execute.
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
        self.bus.ctx.diag.drain()
    }

    /// At most `limit` diagnostics that occurred more than once, the most frequent first, with their
    /// number of occurrences: what the firmware keeps doing, such as polling one register.
    #[must_use]
    pub fn most_repeated_diagnostics(&self, limit: usize) -> Vec<(Diagnostic, u64)> {
        self.bus.ctx.diag.most_repeated(limit)
    }

    /// Occurrences dropped because more than [`MAX_DISTINCT_DIAGNOSTICS`](crate::diag::MAX_DISTINCT_DIAGNOSTICS) distinct diagnostics were
    /// known: they are counted but never listed.
    #[must_use]
    pub const fn suppressed_diagnostics(&self) -> u64 {
        self.bus.ctx.diag.suppressed()
    }

    /// The model that prices each instruction, so a front end can say whether timing is calibrated.
    #[must_use]
    pub fn timing_model(&self) -> &dyn TimingModel {
        &self.timing
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
    use crate::test_support::{
        HANG, T0, T1, flat_rom, load_register, print_program as print, unmodeled_register,
    };
    use cpu_rv32::asm::{addi, csrrs, csrrwi, jal, lui, lw, sw, wfi};
    use cpu_rv32::csr_esp32c3::{PCCR_USER, PCER_USER, PCMR_USER};
    use emu_core::{AccessKind, AccessSize, Bus};

    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    const USB: u32 = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");

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

    #[test]
    fn timing_model_is_exposed_and_says_it_is_not_calibrated() {
        let m = machine(&[HANG]);
        assert!(!m.timing_model().is_calibrated());
        assert_eq!(m.timing_model().name(), "datasheet-uncalibrated");
    }

    #[test]
    fn a_machine_can_be_built_around_a_flash_image_and_refuses_one_that_is_too_long() {
        let config = SocConfig::default();
        assert!(Esp32C3::with_flash(flat_rom(&[HANG]), vec![0xE9; 64], &config).is_ok());
        let too_long = vec![0u8; config.flash.size() as usize + 1];
        let err = Esp32C3::with_flash(flat_rom(&[HANG]), too_long, &config).unwrap_err();
        assert!(
            matches!(err, crate::flash::FlashImageError::TooLong { .. }),
            "{err}"
        );
    }

    #[test]
    fn hang_instruction_is_what_the_assembler_produces() {
        assert_eq!(jal(0, 0), HANG);
    }

    #[test]
    fn starts_at_the_reset_vector_and_prints_through_uart0() {
        let mut program = print(UART0, b"Hi");
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
        let mut program = print(USB, b"ok");
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
        // What ets_delay_us relies on: enable the counter, then watch it advance between two reads.
        const SCRATCH: u8 = 7;
        let mut m = machine(&[
            csrrwi(0, PCER_USER, 1),
            csrrwi(0, PCMR_USER, 1),
            csrrs(T0, PCCR_USER, 0),
            csrrs(T1, PCCR_USER, 0),
            lui(SCRATCH, SRAM_DBUS_BASE),
            sw(T0, SCRATCH, 0),
            sw(T1, SCRATCH, 4),
            HANG,
        ]);
        m.run_until(Cycles(200));
        assert!(
            m.drain_diagnostics().is_empty(),
            "no illegal-instruction exception"
        );
        let (first, second) = (
            bus_word(&mut m, SRAM_DBUS_BASE),
            bus_word(&mut m, SRAM_DBUS_BASE + 4),
        );
        assert!(
            second > first,
            "the counter advanced: {first} then {second}"
        );
    }

    #[test]
    fn a_misaligned_word_access_to_a_peripheral_register_raises_an_access_fault() {
        // Under the machine's `Allow` policy the access reaches the bus, which refuses it: the fault
        // is reported with the faulting address as `tval` (IO_MUX.GPIO20 is at 0x6000_9054).
        const MISALIGNED: u32 = 0x6000_9055;
        for (access, expected) in [
            (lw(T1, T0, 0x55), Exception::LoadAccessFault),
            (sw(T1, T0, 0x55), Exception::StoreAccessFault),
        ] {
            let mut m = machine(&[lui(T0, 0x6000_9000), access, HANG]);
            m.run_until(Cycles(100));
            let first = m
                .drain_diagnostics()
                .into_iter()
                .find(|d| matches!(d, Diagnostic::Exception { .. }));
            assert_eq!(
                first,
                Some(Diagnostic::Exception {
                    cause: expected,
                    tval: MISALIGNED,
                    pc: ROM_BASE + 4,
                })
            );
        }
    }

    #[test]
    fn unmodeled_access_is_reported_with_the_pc_of_the_instruction() {
        let [page, load] = load_register(unmodeled_register().addr);
        let mut m = machine(&[page, load, HANG]);
        assert_eq!(m.run_until(Cycles(200)), StopReason::CycleBudget);
        let listed = m.drain_diagnostics();
        assert!(
            matches!(listed[..], [Diagnostic::UnmodeledMmio { pc, .. }] if pc == ROM_BASE + 4),
            "{listed:?}"
        );
    }

    #[test]
    fn strict_mode_stops_at_the_unmodeled_access() {
        let config = SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        };
        let [page, load] = load_register(unmodeled_register().addr);
        let mut m = machine_with(&[page, load, HANG], config);
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
        let a = print(UART0, b"A");
        let b = print(UART0, b"AB");
        assert_eq!(run(&a), run(&a));
        assert_ne!(run(&a).0, run(&b).0);
        assert_eq!(machine(&a).trace_hash(), None, "off unless asked for");
    }

    /// Three instructions that store `value` (a 12-bit signed immediate) at `addr`.
    fn store(addr: u32, value: i32) -> [u32; 3] {
        let page = addr.wrapping_add(0x800) & 0xFFFF_F000;
        [
            lui(T0, page),
            addi(T1, 0, value),
            sw(T1, T0, addr.wrapping_sub(page).cast_signed()),
        ]
    }
    const SYSCLK_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "SYSCLK_CONF");
    const CPU_PER_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF");

    #[test]
    fn device_time_follows_the_cpu_clock_the_firmware_selects() {
        let mut program = Vec::new();
        program.extend(store(SYSCLK_CONF, 0x400)); // PLL, CPUPERIOD_SEL = 0: 80 MHz
        program.extend(store(CPU_PER_CONF, 0xD)); // CPUPERIOD_SEL = 1: 160 MHz
        program.push(HANG);
        let mut m = machine(&program);
        assert_eq!(m.cpu_frequency().hz(), 20_000_000, "decision D21");
        assert_eq!(m.device_time_nanos(), 0);
        m.run_until(Cycles(1_000));
        assert_eq!(m.cpu_frequency().hz(), 160_000_000);
        assert_eq!(m.drain_diagnostics(), Vec::new());
        // Fewer than 20 cycles ran before the switch to 160 MHz, each of at most 50 ns.
        let nanos = m.device_time_nanos();
        let all_fast = m.cycles().get() * 1_000 / 160;
        assert!(
            nanos >= all_fast && nanos <= all_fast + 20 * 50,
            "{nanos} vs {all_fast}"
        );
    }

    #[test]
    fn device_time_at_the_reset_clock_is_50_ns_per_cycle() {
        let mut m = machine(&[HANG]);
        m.run_until(Cycles(2_000));
        assert_eq!(m.device_time_nanos(), m.cycles().get() * 50);
    }

    #[test]
    fn a_clock_selection_without_a_known_frequency_is_reported_and_keeps_the_previous_one() {
        // -1024 = 0x...C00: SOC_CLK_SEL = 3 once masked to the 12 writable bits.
        let mut program = store(SYSCLK_CONF, -1024).to_vec();
        program.push(HANG);
        let mut m = machine_with(
            &program,
            SocConfig {
                strict_mmio: true,
                ..SocConfig::default()
            },
        );
        let stop = m.run_until(Cycles(1_000));
        let StopReason::StrictMmio(Diagnostic::Unsupported { what, .. }) = &stop else {
            panic!("expected a strict stop, got {stop:?}");
        };
        assert_eq!(
            *what,
            crate::diag::Unsupported::ClockSource {
                sysclk_conf: 0x2_8C00,
                cpu_per_conf: 0xC
            }
        );
        assert_eq!(m.cpu_frequency().hz(), 20_000_000);
    }

    /// A flash chip whose content the test can change behind the machine's back, which the real
    /// read-only chip never does: the only way to see decoded code being dropped.
    #[derive(Debug)]
    struct SharedFlash(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);

    impl FlashDevice for SharedFlash {
        fn size(&self) -> u32 {
            SocConfig::default().flash.size()
        }
        fn read(&self, offset: u32, out: &mut [u8]) {
            let content = self.0.borrow();
            for (byte, index) in out.iter_mut().zip(offset as usize..) {
                *byte = content.get(index).copied().unwrap_or(crate::flash::ERASED);
            }
        }
        fn execute(
            &mut self,
            command: &crate::flash::FlashCommand<'_>,
            _data_in: &mut [u8],
        ) -> Result<(), crate::flash::UnmodeledCommand> {
            Err(crate::flash::UnmodeledCommand {
                opcode: command.opcode,
            })
        }
    }

    #[test]
    fn a_write_to_either_cache_control_register_drops_the_code_decoded_from_the_flash_window() {
        use cpu_rv32::asm::jalr;
        const A0: u8 = 10;
        const RA: u8 = 1;
        const T2: u8 = 7;
        const S0: u8 = 8;
        // The flash holds `li a0, <value>; ret` at offset 0.
        let function = |value: i32| -> Vec<u8> {
            [addi(A0, 0, value), jalr(0, RA, 0)]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect()
        };
        for register in ["ICACHE_CTRL", "ICACHE_CTRL1"] {
            let cache_control = svd_regs::esp32c3::addr("EXTMEM", register);
            let mut program = vec![
                lui(T0, crate::periph::mmu::MMU_TABLE_BASE),
                lui(T2, crate::memmap::FLASH_IBUS_BASE),
                lui(S0, SRAM_DBUS_BASE),
                sw(0, T0, 0),    // entry 0 = flash page 0
                jalr(RA, T2, 0), // decodes the function: a0 = 1
                sw(A0, S0, 0),
                HANG,
            ];
            let second_part = u32::try_from(4 * program.len()).unwrap();
            program.extend([jalr(RA, T2, 0), sw(A0, S0, 4)]);
            program.extend(store(cache_control, 1));
            program.extend([jalr(RA, T2, 0), sw(A0, S0, 8), HANG]);

            let content = std::rc::Rc::new(std::cell::RefCell::new(function(1)));
            let flash = SharedFlash(std::rc::Rc::clone(&content));
            let mut m = Esp32C3::build(flat_rom(&program), Box::new(flash), &SocConfig::default());
            m.run_until(Cycles(200));
            // The content changes without the bus knowing; the machine resumes after the first part.
            *content.borrow_mut() = function(2);
            m.hart.pc = ROM_BASE + second_part;
            m.run_until(Cycles(m.cycles().get() + 200));
            assert_eq!(m.drain_diagnostics(), Vec::new(), "{register}");
            assert_eq!(
                (
                    bus_word(&mut m, SRAM_DBUS_BASE),
                    // The control of the experiment: nothing has invalidated the decoded code yet.
                    bus_word(&mut m, SRAM_DBUS_BASE + 4),
                    bus_word(&mut m, SRAM_DBUS_BASE + 8)
                ),
                (1, 1, 2),
                "the write to {register} must make the next call decode the flash again"
            );
        }
    }

    #[test]
    fn code_executed_from_the_flash_window_follows_a_remap() {
        use cpu_rv32::asm::jalr;
        const A0: u8 = 10;
        const RA: u8 = 1;
        const T2: u8 = 7;
        const S0: u8 = 8;
        // Flash page 0: `li a0, 1; ret`. Flash page 1: `li a0, 2; ret`.
        let mut image = vec![0u8; 0x1_0008];
        for (page, value) in [(0usize, 1), (1, 2)] {
            for (i, word) in [addi(A0, 0, value), jalr(0, RA, 0)].into_iter().enumerate() {
                let at = page * 0x1_0000 + 4 * i;
                image[at..at + 4].copy_from_slice(&word.to_le_bytes());
            }
        }
        let program = [
            lui(T0, crate::periph::mmu::MMU_TABLE_BASE),
            lui(T2, crate::memmap::FLASH_IBUS_BASE),
            lui(S0, SRAM_DBUS_BASE),
            sw(0, T0, 0),    // entry 0 = flash page 0
            jalr(RA, T2, 0), // a0 = 1
            sw(A0, S0, 0),
            addi(T1, 0, 1),
            sw(T1, T0, 0),   // entry 0 = flash page 1: same address, new code
            jalr(RA, T2, 0), // a0 = 2 only if the decoded code was dropped
            sw(A0, S0, 4),
            HANG,
        ];
        let mut m = Esp32C3::with_flash(flat_rom(&program), image, &SocConfig::default()).unwrap();
        m.run_until(Cycles(400));
        assert_eq!(m.drain_diagnostics(), Vec::new());
        assert_eq!(
            (
                bus_word(&mut m, SRAM_DBUS_BASE),
                bus_word(&mut m, SRAM_DBUS_BASE + 4)
            ),
            (1, 2)
        );
    }
}
