//! The ESP32-C3 as a machine: one hart, its bus, and the loop that runs them.
use crate::bus::SocBus;
use crate::clock;
use crate::config::SocConfig;
use crate::ctx::Event;
use crate::diag::{Diagnostic, Unsupported};
use crate::flash::{FlashDevice, FlashImageError, NorFlash};
use crate::memmap::RESET_PC;
use crate::reset::ResetCause;
use crate::rom::RomImage;
use cpu_rv32::{
    DatasheetTiming, Exception, Hart, HartConfig, InterruptModel, MisalignedPolicy, StepOutcome,
    VendorCsrs,
};
use emu_core::{Cycles, Frequency, TimingModel, TraceHash};

/// `x2`, the stack pointer of the RISC-V calling convention.
const STACK_POINTER: usize = 2;

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
    /// The CPU waits for an interrupt and none can come: no interrupt line is pending and no
    /// device has anything scheduled.
    Idle,
    /// The CPU has been waiting for an interrupt through [`SLEEP_SLICE`] of device time, or was
    /// called [`MAX_FRUITLESS_WAKES`] times without one, since the call began. Device time goes
    /// on while the CPU sleeps and its cycle counter does not: a caller that only watches the
    /// cycle limit gets control back here, and calls again to go on.
    Asleep,
}

/// Device time a call to [`Esp32C3::run_until`] spends asleep before it returns
/// [`StopReason::Asleep`]: one second, in cycles of the time base.
pub const SLEEP_SLICE: u64 = 160_000_000;
/// Wake-ups without an interrupt one call goes through before it returns [`StopReason::Asleep`].
pub const MAX_FRUITLESS_WAKES: u32 = 100_000;
/// CPU clock cycles between the instant the controller selects a line and the instant the CPU
/// can take it: "the interrupt controller ... takes 4 cycles" (TRM §1.6.1; report IR §2.7).
///
/// ASSUMPTION: only a line that becomes selectable waits; a line that is masked or released
/// disappears at once. The TRM gives one figure and no direction; the SP1b-4 lockstep compares
/// with an oracle that has no latency at all, and needs this to be configurable.
pub const INTERRUPT_LATENCY_CYCLES: u64 = 4;

/// An ESP32-C3: one hart, the bus with everything it reaches, and the model that prices each
/// instruction. It performs no host I/O: output is handed over through [`Esp32C3::take_console`].
#[derive(Debug)]
pub struct Esp32C3 {
    hart: Hart,
    bus: SocBus,
    timing: DatasheetTiming,
    trace: Option<TraceHash>,
    /// The line the interrupt controller selects, and the device time from which the CPU sees it.
    selected: Option<(u32, Cycles)>,
    /// Resets since power-on.
    resets: u64,
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
            interrupts: InterruptModel::Controller,
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
            selected: None,
            resets: 0,
        }
    }

    /// Runs until the hart's cycle counter reaches `limit` (an absolute [`Esp32C3::cycles`] value) or
    /// the machine stops for another reason.
    pub fn run_until(&mut self, limit: Cycles) -> StopReason {
        let mut slept: u64 = 0;
        let mut fruitless_wakes: u32 = 0;
        while self.hart.cycles.get() < limit.get() {
            let pc = self.hart.pc;
            self.bus.set_pc(pc);
            let before = self.hart.cycles.get();
            // The stack pointer the instruction started with, for the report of an exception.
            let sp = self.hart.x[STACK_POINTER];
            match self.hart.step(&mut self.bus, &self.timing) {
                StepOutcome::Waiting => {
                    // The CPU idles: device time jumps to the next instant a device asked for.
                    let Some(duration) = self.sleep() else {
                        return StopReason::Idle;
                    };
                    slept = slept.saturating_add(duration);
                    fruitless_wakes += 1;
                    self.service(pc);
                    if let Some(violation) = self.bus.ctx.diag.take_violation() {
                        return StopReason::StrictMmio(violation);
                    }
                    if slept >= SLEEP_SLICE || fruitless_wakes >= MAX_FRUITLESS_WAKES {
                        return StopReason::Asleep;
                    }
                    continue;
                }
                StepOutcome::Interrupted { cause } => {
                    // The CPU took the line: the controller marks the edge it had latched for it as
                    // claimed (the latch stays until the firmware's `CPU_INT_CLEAR` toggle
                    // releases it), and the lines are evaluated again.
                    self.bus.periph.intmatrix.claim(cause);
                    self.bus.ctx.interrupts_changed();
                }
                // An `ecall` is a request to the trap handler, not a fault worth reporting.
                StepOutcome::Trapped { trap, .. } if trap.cause != Exception::EcallM => {
                    self.bus.ctx.diag.record(Diagnostic::Exception {
                        cause: trap.cause,
                        tval: trap.tval,
                        pc,
                        sp,
                    });
                }
                _ => {}
            }
            // Device time follows the CPU: the cycles of this instruction, at the clock it ran on.
            self.bus.ctx.advance(self.hart.cycles.get() - before);
            if let Some(trace) = &mut self.trace {
                trace.mix_u32(pc);
            }
            if self.needs_service() {
                self.service(pc);
            }
            if let Some(violation) = self.bus.ctx.diag.take_violation() {
                return StopReason::StrictMmio(violation);
            }
        }
        StopReason::CycleBudget
    }

    /// Whether anything happened that the loop has to act on before the next instruction.
    fn needs_service(&self) -> bool {
        let ctx = &self.bus.ctx;
        ctx.wake_due()
            || ctx.is_flagged()
            || self
                .selected
                .is_some_and(|(_, visible_at)| visible_at <= ctx.now())
    }

    /// Acts on what the last instruction, or the passing of time, left in the context: wake-ups
    /// that are due, a reset, a clock change, a change of the interrupt lines.
    fn service(&mut self, pc: u32) {
        while let Some(event) = self.bus.ctx.take_due() {
            self.bus.deliver(event);
        }
        if let Some(cause) = self.bus.ctx.take_reset() {
            self.reset(cause);
        }
        if self.bus.ctx.take_clock_written() {
            self.retune(pc);
        }
        if self.bus.ctx.take_interrupts_changed() {
            self.select_interrupt();
        }
        self.present_interrupt();
    }

    /// Asks the interrupt controller which line it selects now.
    fn select_interrupt(&mut self) {
        let sources = self.bus.ctx.sources();
        let line = self.bus.periph.intmatrix.resolve(sources);
        self.selected = match (line, self.selected) {
            (None, _) => None,
            // The same line as before: the instant it became visible stands.
            (Some(line), Some((previous, visible_at))) if line == previous => {
                Some((line, visible_at))
            }
            (Some(line), _) => {
                let latency = INTERRUPT_LATENCY_CYCLES * self.bus.ctx.clock().cpu_period;
                Some((line, self.bus.ctx.now() + latency))
            }
        };
    }

    /// Shows the CPU the selected line, once its latency has elapsed.
    fn present_interrupt(&mut self) {
        let now = self.bus.ctx.now();
        let visible = self
            .selected
            .filter(|&(_, visible_at)| visible_at <= now)
            .map(|(line, _)| line);
        self.hart.set_external_interrupt(visible);
    }

    /// Moves device time to the next instant something is due: a wake-up a device asked for, or
    /// the end of the latency of the selected interrupt line. Returns the time skipped, `None`
    /// when nothing is due at any time.
    ///
    /// The CPU clock is gated while the CPU waits: its cycle counter does not move (TRM register
    /// 1.12 `mpccr`, "the counter stops when the CPU is in WFI"; report TW §H.3).
    fn sleep(&mut self) -> Option<u64> {
        let now = self.bus.ctx.now();
        let latency_end = self.selected.map(|(_, visible_at)| visible_at);
        let wake = match (self.bus.ctx.next_wake(), latency_end) {
            (Some(a), Some(b)) => a.min(b),
            (a, b) => a.or(b)?,
        };
        // A wake-up already due is delivered without moving time.
        let target = wake.max(now);
        self.bus.ctx.sleep_until(target);
        Some(target.get() - now.get())
    }

    /// Applies a reset (TRM §6.1): the peripherals of its level return to their reset state, the
    /// CPU restarts in the ROM. The memories keep their content and device time goes on.
    ///
    /// The bound that makes a reset loop safe: a reset request posted while a reset is performed
    /// stays in the context and is taken at the end of the next instruction. That is one reset per
    /// instruction at most, and every instruction advances the cycle count, so a run still
    /// reaches its cycle limit.
    fn reset(&mut self, cause: ResetCause) {
        let (pc, sp) = (self.hart.pc, self.hart.x[STACK_POINTER]);
        self.bus.reset(cause, pc, sp);
        // The flash-window page versions (`SocBus::page_version`) restart at 0 with the MMU and
        // the cache registers: a version can repeat with another mapping. That is safe only
        // because `Hart::reset` builds a new decode cache, at every level.
        self.hart.reset(RESET_PC);
        // A CPU reset keeps the interrupt controller and the source levels (TRM §6.1.3), and the
        // hart forgot the line it was shown: the next `service` selects again. The line becomes
        // visible 4 CPU cycles after the reset, as one that has just become selectable does.
        self.selected = None;
        self.bus.ctx.interrupts_changed();
        self.resets += 1;
    }

    /// Applies a write to the clock registers: from the end of the instruction that wrote them,
    /// cycles last what the new selection says.
    fn retune(&mut self, pc: u32) {
        let (sysclk_conf, cpu_per_conf) = self.bus.periph.system.clock_registers();
        match clock::cpu_clock(sysclk_conf, cpu_per_conf) {
            Some(selected) => {
                self.bus.ctx.set_clock(selected);
                // The devices that count APB cycles go on at the new rate.
                self.bus.deliver(Event::ClockChanged);
            }
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

    /// Resets since power-on: software resets and watchdog resets alike.
    #[must_use]
    pub const fn resets(&self) -> u64 {
        self.resets
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

    /// The bytes that left the device on `channel` since the last call: a UART byte counts once
    /// its last bit has gone.
    pub fn take_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
        match channel {
            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(self.bus.ctx.now()),
            ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_tx(),
        }
    }

    /// Everything the firmware wrote on `channel` and no call has taken, the bytes that have not
    /// left the device yet included. For the end of a run: without them the console would stop
    /// short of the place the run stopped at. It ends the transmission: do not go on running a
    /// machine whose console was flushed if the timing of its output matters.
    pub fn flush_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
        match channel {
            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_written(),
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
        HANG, T0, T1, flat_rom, load_register, print_program as print, stop_watchdogs,
        store_register, unmodeled_register,
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
        assert_eq!(
            m.take_console(ConsoleChannel::Uart0),
            b"",
            "a byte takes 6 940 CPU cycles to leave at the reset baud rate and clock"
        );
        assert_eq!(m.run_until(Cycles(15_000)), StopReason::CycleBudget);
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
                    sp: 0,
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
        // Strapped for another boot mode, no watchdog runs at reset: no device has anything
        // scheduled.
        let config = SocConfig {
            strapping: 0,
            ..SocConfig::default()
        };
        let mut m = machine_with(&[wfi(), HANG], config);
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Idle);
        assert_eq!(
            m.device_time_nanos(),
            150,
            "`wfi` alone: 3 cycles at 20 MHz, no jump in time"
        );
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

    /// Where the tests that take traps put their vector table: a page boundary, which `lui` can
    /// load (`mtvec.BASE` only has to be aligned to 256 bytes).
    const VECTORS: u32 = ROM_BASE + 0x1000;
    const MTVEC: u16 = 0x305;

    #[test]
    fn an_instruction_fetched_through_an_unmapped_flash_page_is_fetched_again_each_time() {
        // Carried over from SP1b-2: decoded once and kept, such an instruction would be reported
        // and latched once however often it runs. The handler jumps back into the window.
        let window = crate::memmap::FLASH_IBUS_BASE;
        let mut body = vec![
            lui(T0, VECTORS),
            cpu_rv32::asm::csrrw(0, MTVEC, T0),
            lui(T1, window),
            cpu_rv32::asm::jalr(0, T1, 0),
        ];
        body.resize(((VECTORS - ROM_BASE) / 4) as usize, HANG);
        // Exceptions enter at the base of the table.
        body.push(cpu_rv32::asm::jalr(0, T1, 0));
        let mut m = machine(&body);
        assert_eq!(m.run_until(Cycles(200)), StopReason::CycleBudget);
        let faults = m
            .most_repeated_diagnostics(8)
            .into_iter()
            .find(|(d, _)| matches!(d, Diagnostic::FlashMapping { vaddr, .. } if *vaddr == window));
        let Some((_, count)) = faults else {
            panic!("the fetch through the invalid entry was not reported more than once");
        };
        assert!(count > 5, "one report per execution: {count}");
    }

    /// `addi x0, x0, 0`: one cycle, no effect.
    const NOP: u32 = 0x0000_0013;
    const MSTATUS: u16 = 0x300;
    const MSTATUS_MIE: u8 = 1 << 3;

    fn reg(peripheral: &str, register: &str) -> u32 {
        svd_regs::esp32c3::addr(peripheral, register)
    }

    /// `body` at the reset vector, then the vector table at [`VECTORS`]: every entry parks.
    fn with_vectors(body: &[u32]) -> Vec<u32> {
        let table_index = ((VECTORS - ROM_BASE) / 4) as usize;
        assert!(
            body.len() <= table_index,
            "the body overlaps the vector table"
        );
        let mut program = body.to_vec();
        program.resize(table_index, HANG);
        program.extend([HANG; 32]);
        program
    }

    /// Routes interrupt source `map_register` to CPU `line` at priority 1 and enables the line,
    /// as `esp_intr_alloc` does (IR §3.2).
    fn route(map_register: &str, line: u32) -> Vec<u32> {
        let mut program = Vec::new();
        program.extend(store_register(reg("INTERRUPT_CORE0", map_register), line));
        program.extend(store_register(
            reg("INTERRUPT_CORE0", "CPU_INT_PRI_0") + 4 * line,
            1,
        ));
        program.extend(store_register(
            reg("INTERRUPT_CORE0", "CPU_INT_ENABLE"),
            1 << line,
        ));
        program
    }

    #[test]
    fn a_software_interrupt_is_taken_four_cpu_cycles_after_it_becomes_pending() {
        // IR §3.3: the application raises `CPU_INTR_FROM_CPU_0` (source 50) routed to line 3;
        // IR §2.7: "the interrupt controller ... takes 4 cycles".
        let mut body = Vec::new();
        body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
        body.extend(route("CPU_INTR_FROM_CPU_0_MAP", 3));
        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
        body.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
        let raised_at = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
        body.extend([NOP; 16]);
        let mut m = machine(&with_vectors(&body));
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        assert_eq!(m.hart.csr.mcause, 0x8000_0003);
        assert_eq!(
            m.hart.csr.mepc,
            raised_at + 4 * 4,
            "four one-cycle instructions ran between the store and the interrupt"
        );
        assert_eq!(m.pc(), VECTORS + 4 * 3, "the entry of line 3 parks");
        assert_eq!(
            bus_word(&mut m, reg("INTERRUPT_CORE0", "CPU_INT_EIP_STATUS")),
            1 << 3,
            "the source is a level: the line stays pending until the handler clears it"
        );
    }

    #[test]
    fn a_level_raised_while_an_instruction_is_fetched_is_evaluated_before_the_next_one() {
        // A fetch through an invalid MMU entry raises the cache error source (`EXTMEM`, source 36)
        // in the middle of `step`: no store to a register of the controller follows, so only the
        // flag the context keeps makes the machine ask the controller again. The exception
        // handler enables interrupts; the interrupt on line 6 must be taken inside it.
        let mut body = Vec::new();
        body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
        body.extend(route("CACHE_IA_INT_MAP", 6));
        body.extend(store_register(reg("EXTMEM", "CACHE_ILG_INT_ENA"), 1 << 5));
        body.extend([
            lui(T1, crate::memmap::FLASH_IBUS_BASE),
            cpu_rv32::asm::jalr(0, T1, 0),
        ]);
        let mut program = with_vectors(&body);
        let table = ((VECTORS - ROM_BASE) / 4) as usize;
        // Exceptions enter at the base of the table; the entry of line 6 is `table + 6`.
        program[table] = csrrwi(0, MSTATUS, MSTATUS_MIE);
        program[table + 1..table + 6].fill(NOP);
        let mut m = machine(&program);
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        assert_eq!(
            m.hart.csr.mcause, 0x8000_0006,
            "the interrupt, not the instruction access fault (1), was the last trap"
        );
        assert_eq!(m.pc(), VECTORS + 4 * 6, "the entry of line 6 parks");
    }

    #[test]
    fn a_line_under_the_threshold_or_disabled_is_not_taken() {
        let mut body = Vec::new();
        body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
        body.extend(route("CPU_INTR_FROM_CPU_0_MAP", 3));
        // What a critical section does: `CPU_INT_THRESH` above the priority of the line (IR §3.3).
        body.extend(store_register(reg("INTERRUPT_CORE0", "CPU_INT_THRESH"), 2));
        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
        body.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
        body.extend([NOP; 16]);
        body.push(HANG);
        let hang = ROM_BASE + 4 * u32::try_from(body.len() - 1).unwrap();
        let mut m = machine(&with_vectors(&body));
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        assert_eq!(m.pc(), hang, "masked by the threshold");
        assert_eq!(m.hart.csr.mcause, 0);
    }

    #[test]
    fn wfi_sleeps_to_the_alarm_and_resumes_without_a_trap_when_interrupts_are_masked() {
        // A one-shot alarm of the SYSTIMER 16 384 counts ahead (1.024 ms at 16 MHz), on line 4.
        let mut body = Vec::new();
        body.extend(route("SYSTIMER_TARGET0_INT_MAP", 4));
        body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), 16_384));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        body.extend(store_register(reg("SYSTIMER", "INT_ENA"), 1));
        // `CONF`: its reset value with `TARGET0_WORK_EN`, bit 24 (`systimer_ll_enable_alarm`).
        body.extend(store_register(
            reg("SYSTIMER", "CONF"),
            0x4600_0000 | (1 << 24),
        ));
        body.extend([wfi(), addi(1, 0, 1), HANG]);
        let mut m = machine(&body);
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        assert_eq!(
            m.hart.x[1], 1,
            "`mstatus.MIE` is 0: execution goes on after `wfi`"
        );
        assert_eq!(m.hart.csr.mcause, 0, "no trap");
        // Awake, a cycle lasts 50 ns; the rest of the time went by in `wfi`, from a few
        // microseconds after reset to the alarm, 1.024 ms after the counter started.
        let slept_ns = m.device_time_nanos() - 50 * m.cycles().get();
        assert!(
            (1_020_000..=1_024_200).contains(&slept_ns),
            "slept {slept_ns} ns"
        );
        assert!(
            (2_000..2_003).contains(&m.cycles().get()),
            "the cycle counter stands still in `wfi`: {}",
            m.cycles().get()
        );
    }

    #[test]
    fn a_cpu_that_sleeps_while_a_device_counts_gives_control_back_after_a_second() {
        // A one-shot alarm two seconds ahead (32 000 000 counts at 16 MHz), its interrupt not
        // enabled: nothing will wake the CPU, and time must still pass. The watchdogs the reset
        // values arm are stopped first: they would reset the chip at 1.3 s. A watchdog that is
        // stopped withdraws its wake-up, so the first thing scheduled is the alarm.
        let mut body = stop_watchdogs();
        body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), 32_000_000));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        body.extend(store_register(
            reg("SYSTIMER", "CONF"),
            0x4600_0000 | (1 << 24),
        ));
        body.extend([wfi(), HANG]);
        let mut m = machine(&body);
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
        assert_eq!(
            m.device_time_nanos(),
            2_000_000_000,
            "slept to the alarm in one jump"
        );
        assert_eq!(
            m.run_until(Cycles(10_000)),
            StopReason::Idle,
            "the alarm has fired and nothing else is scheduled"
        );
    }

    #[test]
    fn a_storm_of_wake_ups_without_an_interrupt_gives_control_back() {
        // A period alarm at every count of the 16 MHz counter, its interrupt not enabled
        // (`TARGET0_CONF`: bit 30 `PERIOD_MODE`, period 1): the SYSTIMER is called 100 000 times
        // in 6.25 ms of device time and the CPU never wakes.
        let mut body = Vec::new();
        body.extend(store_register(
            reg("SYSTIMER", "TARGET0_CONF"),
            (1 << 30) | 1,
        ));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        body.extend(store_register(
            reg("SYSTIMER", "CONF"),
            0x4600_0000 | (1 << 24),
        ));
        body.extend([wfi(), HANG]);
        let mut m = machine(&body);
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
        let first = m.device_time_nanos();
        assert!(
            (6_250_000..6_300_000).contains(&first),
            "MAX_FRUITLESS_WAKES counts of 62.5 ns: {first} ns"
        );
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep, "and again");
        assert!(m.device_time_nanos() > first);
    }

    #[test]
    fn a_software_cpu_reset_restarts_at_the_reset_vector_and_keeps_memory_and_time() {
        // What `esp_restart` ends with: `RTC_CNTL.OPTIONS0` bit 5 (IR §4.1). The second pass
        // finds the cause and parks.
        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
        let marker = load_register(MARKER);
        // The RAM is zero after power-on: the first pass skips the park of the second.
        let mut body = vec![marker[0], marker[1], cpu_rv32::asm::beq(T1, 0, 8), HANG];
        body.extend(stop_watchdogs());
        body.extend(store_register(MARKER, 0x5AFE_C0DE));
        body.extend(store_register(reg("ASSIST_DEBUG", "CORE_0_RCD_EN"), 3));
        body.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 5));
        let after_store = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
        body.push(HANG);
        let mut m = machine(&body);
        assert_eq!(m.run_until(Cycles(1_000)), StopReason::CycleBudget);
        assert_eq!(m.resets(), 1);
        assert_eq!(m.pc(), ROM_BASE + 4 * 3, "the second pass parked");
        assert_eq!(
            bus_word(&mut m, reg("RTC_CNTL", "RESET_STATE")) & 0x3F,
            0x0C,
            "RTC_SW_CPU_RST"
        );
        assert_eq!(
            bus_word(&mut m, MARKER),
            0x5AFE_C0DE,
            "the RAM keeps its content"
        );
        assert_eq!(
            bus_word(&mut m, reg("ASSIST_DEBUG", "CORE_0_RCD_PDEBUGPC")),
            after_store,
            "the PC the reset kept from executing"
        );
        assert_eq!(
            bus_word(&mut m, reg("TIMG0", "WDTCONFIG0")),
            0,
            "a CPU reset leaves the peripherals as they are"
        );
        assert_eq!(
            m.cycles().get(),
            1_000,
            "the cycle total goes on across the reset"
        );
        assert!(
            m.device_time_nanos() >= 1_000 * 50,
            "and so does device time"
        );
    }

    #[test]
    fn the_flash_boot_watchdog_resets_a_cpu_that_only_sleeps() {
        // The reset values arm main watchdog 0 (TW §D.4): 26 000 000 cycles of the 20 MHz APB
        // clock, 1.3 s. Its deadline is the first thing scheduled.
        let mut m = machine(&[wfi(), HANG]);
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
        assert_eq!(
            m.device_time_nanos(),
            1_300_000_000,
            "slept to the watchdog deadline"
        );
        assert_eq!(m.resets(), 1, "which reset the chip");
        assert_eq!(
            bus_word(&mut m, svd_regs::esp32c3::addr("RTC_CNTL", "RESET_STATE")) & 0x3F,
            0x07,
            "TG0WDT_SYS_RST"
        );
        assert_eq!(m.cycles().get(), 3, "only `wfi` was executed");
    }

    /// `rd` <- the word at `addr`, through `x10`.
    fn load_into(rd: u8, addr: u32) -> [u32; 2] {
        let page = addr.wrapping_add(0x800) & 0xFFFF_F000;
        [
            lui(10, page),
            lw(rd, 10, addr.wrapping_sub(page).cast_signed()),
        ]
    }

    /// Two passes around a CPU reset. The first routes software interrupt 0 to line 3, raises it
    /// with the interrupts masked and asks for the reset; the controller and the source levels
    /// survive a CPU reset, so the second pass finds the line still selected. It sets `MIE` when
    /// `mie` is true and sleeps in `wfi`.
    fn a_line_pending_across_a_cpu_reset(mie: bool) -> Esp32C3 {
        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
        let marker = load_register(MARKER);
        let mut first = stop_watchdogs();
        first.extend(store_register(MARKER, 1));
        first.extend(route("CPU_INTR_FROM_CPU_0_MAP", 3));
        first.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
        first.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 5));
        first.push(HANG);
        let mut second = vec![lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)];
        if mie {
            second.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
        }
        second.extend([wfi(), addi(1, 0, 1), HANG]);
        let skip = 4 * (i32::try_from(first.len()).unwrap() + 1);
        let mut body = vec![marker[0], marker[1], cpu_rv32::asm::bne(T1, 0, skip)];
        body.extend(first);
        body.extend(second);
        machine(&with_vectors(&body))
    }

    #[test]
    fn a_line_selected_before_a_cpu_reset_wakes_a_masked_wfi_after_it() {
        // TRM §6.1.3: a CPU reset leaves the interrupt controller and the sources as they are.
        let mut m = a_line_pending_across_a_cpu_reset(false);
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        assert_eq!(m.resets(), 1);
        assert_eq!(
            bus_word(&mut m, reg("INTERRUPT_CORE0", "CPU_INT_EIP_STATUS")),
            1 << 3,
            "the source is still raised"
        );
        assert_eq!(
            m.hart.x[1], 1,
            "`mstatus.MIE` is 0: `wfi` ends, the code after it runs"
        );
    }

    #[test]
    fn a_line_selected_before_a_cpu_reset_is_taken_after_it() {
        let mut m = a_line_pending_across_a_cpu_reset(true);
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        assert_eq!(m.resets(), 1);
        assert_eq!(m.hart.csr.mcause, 0x8000_0003);
        assert_eq!(m.pc(), VECTORS + 4 * 3, "the entry of line 3 parks");
    }

    #[test]
    fn the_rtc_watchdog_resets_a_sleeping_hart_at_each_level_in_turn() {
        // Stage 0 resets the CPU, stage 1 the core, stage 2 the whole chip (TW §D.2). Every pass
        // appends a record to a log in RAM before it sleeps: the reset cause, `STORE0`, and the
        // address of its first instruction. The log is the proof that the RAM is kept.
        const NEXT: u32 = SRAM_DBUS_BASE + 0x100;
        const LOG: u32 = SRAM_DBUS_BASE + 0x200;
        // Registers: the base of the RAM, the log pointer (0 on the first pass), a copy of it, the
        // value being logged, the address of the first instruction.
        let (base, ptr, old, value, pc) = (8, 9, 13, 11, 12);
        let mut body = vec![
            cpu_rv32::asm::auipc(pc, 0),
            lui(base, SRAM_DBUS_BASE),
            lw(ptr, base, 0x100),
            addi(old, ptr, 0),
            cpu_rv32::asm::bne(ptr, 0, 8),
            addi(ptr, base, 0x200),
        ];
        for (offset, register) in [(0, "RESET_STATE"), (4, "STORE0")] {
            body.extend(load_into(value, reg("RTC_CNTL", register)));
            body.push(sw(value, ptr, offset));
        }
        body.extend([sw(pc, ptr, 8), addi(ptr, ptr, 16), sw(ptr, base, 0x100)]);
        // Only the first pass sets the retention register and arms the watchdog.
        let mut first = store_register(reg("TIMG0", "WDTCONFIG0"), 0).to_vec();
        first.extend(store_register(reg("RTC_CNTL", "STORE0"), 0xA5A5));
        for (i, timeout) in [1_000, 2_000, 3_000, 4_000].into_iter().enumerate() {
            first.extend(store_register(
                reg("RTC_CNTL", "WDTCONFIG1") + 4 * u32::try_from(i).unwrap(),
                timeout,
            ));
        }
        first.extend(store_register(
            reg("RTC_CNTL", "WDTCONFIG0"),
            (1 << 31) | (2 << 28) | (3 << 25) | (4 << 22),
        ));
        body.push(cpu_rv32::asm::bne(
            old,
            0,
            4 * (i32::try_from(first.len()).unwrap() + 1),
        ));
        body.extend(first);
        body.extend([wfi(), HANG]);
        let mut m = machine(&body);
        assert_eq!(m.run_until(Cycles(100_000)), StopReason::Asleep);
        assert!(m.resets() >= 3, "{} resets", m.resets());
        let logged = (bus_word(&mut m, NEXT) - LOG) / 16;
        assert!(
            u64::from(logged) >= m.resets(),
            "{logged} passes logged, {} resets",
            m.resets()
        );
        let record = |m: &mut Esp32C3, pass: u32| {
            let at = LOG + 16 * pass;
            (
                bus_word(m, at) & 0x3F,
                bus_word(m, at + 4),
                bus_word(m, at + 8),
            )
        };
        for (pass, cause, store0) in [
            (0, 0x01, 0),
            (1, 0x0D, 0xA5A5),
            (2, 0x09, 0xA5A5),
            (3, 0x10, 0),
        ] {
            assert_eq!(
                record(&mut m, pass),
                (cause, store0, ROM_BASE),
                "pass {pass}: cause, STORE0, and a start at the reset vector"
            );
        }
    }

    /// Two passes around the reset a `trigger` asks for. The first selects the PLL at 160 MHz and
    /// runs `trigger`; the second parks.
    fn a_machine_reset_after_selecting_the_pll(trigger: Vec<u32>) -> Esp32C3 {
        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
        let marker = load_register(MARKER);
        let mut first = stop_watchdogs();
        first.extend(store_register(MARKER, 1));
        first.extend(store_register(SYSCLK_CONF, 0x400));
        first.extend(store_register(CPU_PER_CONF, 0xD));
        first.extend(trigger);
        first.push(HANG);
        let skip = 4 * (i32::try_from(first.len()).unwrap() + 1);
        let mut body = vec![marker[0], marker[1], cpu_rv32::asm::bne(T1, 0, skip)];
        body.extend(first);
        body.push(HANG);
        let mut m = machine(&body);
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::CycleBudget);
        assert_eq!(m.resets(), 1);
        m
    }

    #[test]
    fn the_cpu_clock_selection_survives_a_cpu_reset_and_not_a_core_or_system_reset() {
        // Decision E8: the crystal divided by 2 after a reset of the clock registers, 20 MHz.
        let mut cpu = a_machine_reset_after_selecting_the_pll(
            store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 5).to_vec(),
        );
        assert_eq!(
            bus_word(&mut cpu, reg("RTC_CNTL", "RESET_STATE")) & 0x3F,
            0x0C
        );
        assert_eq!(cpu.cpu_frequency().hz(), 160_000_000, "kept by a CPU reset");
        let mut core = a_machine_reset_after_selecting_the_pll(
            store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 31).to_vec(),
        );
        assert_eq!(
            bus_word(&mut core, reg("RTC_CNTL", "RESET_STATE")) & 0x3F,
            0x03
        );
        assert_eq!(
            core.cpu_frequency().hz(),
            20_000_000,
            "reset by a core reset"
        );
        // The RTC watchdog's last stage resets the whole chip while the hart sleeps.
        let mut watchdog = store_register(reg("RTC_CNTL", "WDTCONFIG1"), 100).to_vec();
        watchdog.extend(store_register(
            reg("RTC_CNTL", "WDTCONFIG0"),
            (1 << 31) | (4 << 28),
        ));
        watchdog.push(wfi());
        let mut system = a_machine_reset_after_selecting_the_pll(watchdog);
        assert_eq!(
            bus_word(&mut system, reg("RTC_CNTL", "RESET_STATE")) & 0x3F,
            0x10
        );
        assert_eq!(
            system.cpu_frequency().hz(),
            20_000_000,
            "reset by a system reset"
        );
    }

    /// A core reset in the middle of a console line: `AB` leaves the UART during a delay, `CD`
    /// is still in the FIFO when the reset comes, and the second pass prints `Z`.
    fn a_console_line_cut_by_a_core_reset() -> Esp32C3 {
        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
        let marker = load_register(MARKER);
        let mut first = stop_watchdogs();
        first.extend(store_register(MARKER, 1));
        first.extend(print(UART0, b"AB"));
        // About 1 ms at 20 MHz: more than the two bytes take at the reset baud rate.
        first.extend([
            lui(14, 0x3000),
            addi(14, 14, -1),
            cpu_rv32::asm::bne(14, 0, -4),
        ]);
        first.extend(print(UART0, b"CD"));
        first.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 31));
        first.push(HANG);
        let skip = 4 * (i32::try_from(first.len()).unwrap() + 1);
        let mut body = vec![marker[0], marker[1], cpu_rv32::asm::bne(T1, 0, skip)];
        body.extend(first);
        body.extend(print(UART0, b"Z"));
        body.push(HANG);
        machine_with(
            &body,
            SocConfig {
                trace_hash: true,
                ..SocConfig::default()
            },
        )
    }

    #[test]
    fn a_core_reset_keeps_the_console_bytes_that_left_and_drops_those_in_the_fifo() {
        let mut m = a_console_line_cut_by_a_core_reset();
        assert_eq!(m.run_until(Cycles(200_000)), StopReason::CycleBudget);
        assert_eq!(m.resets(), 1);
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"ABZ");
    }

    #[test]
    fn a_run_across_a_core_reset_is_the_same_whole_or_in_slices_of_seven_cycles() {
        let mut whole = a_console_line_cut_by_a_core_reset();
        whole.run_until(Cycles(200_000));
        let end = whole.cycles().get();
        let mut sliced = a_console_line_cut_by_a_core_reset();
        let mut console = Vec::new();
        while sliced.cycles().get() < end {
            let next = (sliced.cycles().get() + 7).min(end);
            sliced.run_until(Cycles(next));
            console.extend(sliced.take_console(ConsoleChannel::Uart0));
        }
        assert_eq!(sliced.cycles(), whole.cycles());
        assert_eq!(sliced.resets(), whole.resets());
        assert_eq!(sliced.device_time_nanos(), whole.device_time_nanos());
        assert_eq!(sliced.trace_hash(), whole.trace_hash());
        assert!(whole.trace_hash().is_some());
        assert_eq!(console, whole.take_console(ConsoleChannel::Uart0));
    }

    #[test]
    fn a_strict_run_goes_on_across_a_reset_which_is_not_a_violation() {
        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
        let marker = load_register(MARKER);
        let mut first = stop_watchdogs();
        first.extend(store_register(MARKER, 1));
        first.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 5));
        first.push(HANG);
        let skip = 4 * (i32::try_from(first.len()).unwrap() + 1);
        let mut body = vec![marker[0], marker[1], cpu_rv32::asm::bne(T1, 0, skip)];
        body.extend(first);
        body.push(HANG);
        let mut m = machine_with(
            &body,
            SocConfig {
                strict_mmio: true,
                ..SocConfig::default()
            },
        );
        assert_eq!(m.run_until(Cycles(1_000)), StopReason::CycleBudget);
        assert_eq!(m.resets(), 1);
        assert_eq!(m.drain_diagnostics(), Vec::new());
    }

    #[test]
    fn an_exception_is_reported_with_the_stack_pointer_of_the_faulting_instruction() {
        // Decision E12: one line per cause and PC, with what a developer needs to find the frame.
        let mut m = machine(&[lui(2, 0x3FC9_0000), 0, HANG]);
        m.run_until(Cycles(20));
        let exception = m
            .drain_diagnostics()
            .into_iter()
            .find(|d| matches!(d, Diagnostic::Exception { .. }));
        assert_eq!(
            exception,
            Some(Diagnostic::Exception {
                cause: Exception::IllegalInstr,
                tval: 0,
                pc: ROM_BASE + 4,
                sp: 0x3FC9_0000,
            })
        );
    }
}
