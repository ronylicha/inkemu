//! The ESP32-C3 as a machine: one hart, its bus, and the loop that runs them.
use crate::bus::SocBus;
use crate::clock;
use crate::config::SocConfig;
use crate::config::{CompatActs, Timing};
use crate::ctx::Event;
use crate::diag::{Diagnostic, Unsupported};
use crate::flash::{FlashDevice, FlashImageError, FlashWrite, NorFlash};
use crate::inputs::InputJournal;
use crate::memmap::RESET_PC;
use crate::periph::io::{BoardRefusal, MAX_PENDING_BOARD_INPUTS};
use crate::periph::{AttachError, SpiWires, UsbHost};
use crate::reset::ResetCause;
use crate::rom::RomImage;
use cpu_rv32::{
    Boundary, Core, DatasheetTiming, Decoded, Exception, Hart, HartConfig, InterruptModel,
    MisalignedPolicy, QemuIcountTiming, StepOutcome, Trap,
};
use emu_core::{
    AnalogSource, BoardInput, BoardInputError, BoardInputs, Clocked, Cycles, Frequency, GpioInput,
    GpioOutput, SpiTarget, Supply, TimingModel, TraceHash,
};

/// `x2`, the stack pointer of the RISC-V calling convention.
const STACK_POINTER: usize = 2;

/// The serial outputs of the chip: the two consoles the mask ROM prints on, and UART1, which a
/// firmware may log or talk to a peripheral on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleChannel {
    Uart0,
    UsbSerialJtag,
    Uart1,
}

impl ConsoleChannel {
    /// Every channel, for the callers that drain them all ([`Esp32C3::take_console`]).
    pub const ALL: [ConsoleChannel; 3] = [
        ConsoleChannel::Uart0,
        ConsoleChannel::UsbSerialJtag,
        ConsoleChannel::Uart1,
    ];

    /// The channel as the command line and a front end name it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ConsoleChannel::Uart0 => "uart0",
            ConsoleChannel::UsbSerialJtag => "usb-serial-jtag",
            ConsoleChannel::Uart1 => "uart1",
        }
    }

    /// The place of the channel in [`ConsoleChannel::ALL`].
    pub(crate) const fn index(self) -> usize {
        match self {
            ConsoleChannel::Uart0 => 0,
            ConsoleChannel::UsbSerialJtag => 1,
            ConsoleChannel::Uart1 => 2,
        }
    }
}

/// The most bytes of the host a channel holds that have not reached the device yet
/// ([`Esp32C3::input_pending`]): what a front end may hand over ahead of the firmware
/// ([`Esp32C3::send_console`]). A megabyte, a thousand times the 1 KiB receive buffer of the
/// ESP-IDF drivers; what has reached the device is in its receive FIFO, 128 bytes at most.
pub const MAX_PENDING_HOST_BYTES: usize = 1 << 20;
/// The most changes of the USB host still to come ([`Esp32C3::set_usb_host`]).
pub const MAX_PENDING_HOST_CHANGES: usize = 16_384;

/// Why the machine refused an input of the run ([`Esp32C3::send_console`],
/// [`Esp32C3::set_usb_host`]).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    /// The input is dated before the device time the machine has reached: what the firmware
    /// did since cannot have seen it.
    #[error(
        "the input is dated {at_nanos} ns of device time, but the run is at {now_nanos} ns already; date it at {now_nanos} ns or later"
    )]
    InThePast { at_nanos: u64, now_nanos: u64 },
    /// The channel already holds as many bytes of the host as it may.
    #[error(
        "{channel} would hold {pending} bytes of the host that have not reached the device, more than {MAX_PENDING_HOST_BYTES}; send the rest once the firmware has read some",
        channel = channel.name()
    )]
    TooMuchPending {
        channel: ConsoleChannel,
        pending: usize,
    },
    /// The input is dated after the last instant a UART times its line at.
    #[error(
        "{channel} cannot take an input dated {at_nanos} ns of device time: a UART times its line up to {last_nanos} ns; date it at {last_nanos} ns or earlier",
        channel = channel.name()
    )]
    BeyondTheLine {
        channel: ConsoleChannel,
        at_nanos: u64,
        last_nanos: u64,
    },
    /// The changes of the USB host still to come are as many as they may be.
    #[error(
        "{MAX_PENDING_HOST_CHANGES} changes of the USB host are still to come; give the next ones once the run has reached some"
    )]
    TooManyHostChanges,
    /// An input of the board, on a machine no board inputs were attached to.
    #[error("the device has no input of its board: nothing was wired to take it")]
    NoBoard,
    /// The board refused the input.
    #[error("{0}")]
    Board(#[from] BoardInputError),
    /// The inputs of the board still to act are as many as they may be.
    #[error(
        "{MAX_PENDING_BOARD_INPUTS} inputs of the board are still to come; give the next ones once the run has reached some"
    )]
    TooManyBoardInputs,
    /// An input of the board dated so late that the instant it acts at, one past its date, would
    /// be the last instant of the time base or past it: no run reaches it, and a deep sleep would
    /// jump there.
    #[error(
        "the input of the board is dated {at_nanos} ns of device time, too late for the device ever to reach the instant it acts at; date it earlier"
    )]
    BeyondTime { at_nanos: u64 },
    /// The device has no power any more: it takes no input.
    #[error("the device is off: its supply failed; power it on again")]
    PoweredOff,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The cycle limit passed to [`Esp32C3::run_within`] or [`Esp32C3::run_until`] was reached.
    CycleBudget,
    /// The device-time limit passed to [`Esp32C3::run_within`] was reached.
    TimeBudget,
    /// `--strict-mmio`: the program touched a register the emulator does not model, wrote a
    /// peripheral register with an 8- or 16-bit store, asked for an operation the emulator cannot
    /// perform faithfully, or used flash it had not mapped.
    StrictMmio(Diagnostic),
    /// The CPU waits for an interrupt and none can come: no interrupt line is pending and no
    /// device has anything scheduled. Device time stands still from here on, so no limit of
    /// [`Esp32C3::run_within`] can end such a run: this does.
    Idle,
    /// The call has slept through [`SLEEP_SLICE`] of device time, or [`MAX_FRUITLESS_WAKES`]
    /// times, and gives control back: the cycle counter stands still while the CPU sleeps, so
    /// the cycle limit alone ends a call of a firmware that sleeps late, or never when nothing
    /// interrupts its sleep. The caller calls again to go on, and bounds such a run with the
    /// device-time limit.
    ///
    /// Both bounds are counted over the whole call, the sleeps an interrupt ended included. The
    /// reason therefore says what the call did, not the state it leaves: the sleep that reaches a
    /// bound may be the one an interrupt ends, and the call in which a watchdog resets a
    /// sleeping CPU returns `Asleep` although the CPU is then awake, about to execute the first
    /// instruction of the ROM. [`Esp32C3::is_asleep`] gives the state.
    Asleep,
    /// A channel [`Esp32C3::watch_console`] watches handed bytes to the host: the call returns
    /// at the instruction boundary at which they went (at their very instant while the CPU
    /// sleeps), so that [`Esp32C3::device_time`] is the device time they reached the host. The
    /// caller takes them with [`Esp32C3::take_console`] and calls again; bytes of a watched
    /// channel it leaves untaken end the next calls too ([`Esp32C3::watch_console`]).
    ConsoleOutput,
    /// The chip is in deep sleep and nothing the model has can wake it: no wake source it can
    /// raise is enabled (the RTC timer, with its alarm) and nothing else of the RTC domain is due
    /// (F5). As with [`StopReason::Idle`], device time stands still from here on and is not moved
    /// to a limit; [`Esp32C3::deep_sleep`] says since when and on which sources it waits.
    DeepSleep,
    /// The board's supply failed (ADR 0013): the chip has no power and runs no further, whatever
    /// a caller does; [`Esp32C3::power_lost_nanos`] says when.
    PowerOff,
}

/// A chip in deep sleep, as a front end shows it (F9): read with [`Esp32C3::deep_sleep`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeepSleep {
    /// Device time of the entry, in nanoseconds since power-on: the write of `SLEEP_EN`.
    pub since_nanos: u64,
    /// Device time at which the RTC timer wakes the chip, when the timer is a wake source and
    /// its alarm is enabled; `None` when nothing the model has can wake it.
    pub timer_wake_nanos: Option<u64>,
    /// `RTC_CNTL.WAKEUP_STATE.WAKEUP_ENA` at the entry: bit 3 the RTC timer, bit 2 GPIO, the
    /// others sources the model does not raise either (`rtc.h:664-672`).
    pub wake_sources: u32,
}

impl DeepSleep {
    /// `WAKEUP_ENA` bit 3: the RTC timer.
    pub const TIMER: u32 = 1 << 3;
    /// `WAKEUP_ENA` bit 2: the wake pins, GPIO0 to GPIO5 (ADR 0013).
    pub const GPIO: u32 = 1 << 2;
}

/// Device time one call to [`Esp32C3::run_within`] sleeps through, all its sleeps added, before
/// it returns [`StopReason::Asleep`]: one second, in cycles of the time base.
pub const SLEEP_SLICE: u64 = clock::SECOND;
/// Sleeps one call to [`Esp32C3::run_within`] goes through before it returns
/// [`StopReason::Asleep`]. Every sleep counts, whether an interrupt ended it or not; the bound is
/// there for the wake-ups that wake nothing (a device alarm whose interrupt is not enabled),
/// which would otherwise keep a call from returning for as long as [`SLEEP_SLICE`] takes them.
pub const MAX_FRUITLESS_WAKES: u32 = 100_000;
/// CPU clock cycles between the instant the controller selects a line and the instant the CPU
/// can take it: "the Interrupt Controller has a fixed latency of 4 cycles" (TRM §1.5.3.1, as
/// quoted in report IR §2.7).
///
/// ASSUMPTION: only a line that becomes selected waits; a line that is masked or released
/// disappears at once. A change of the selected line is a line that becomes selected: when a
/// line of higher priority takes the place of one the CPU could already see, the CPU is shown
/// no line at all until the latency of the new one has elapsed, and the old one is not
/// presented in the meantime. No interrupt is lost by it: the sources are levels, held until
/// software clears them. The TRM gives one figure and no direction, and says of the 4 cycles
/// that follow a write to the controller that "the ordering of interrupts may not be
/// predictable" (same section). Raising a second, higher-priority source on a chip with `MIE`
/// set and recording which handler runs in the following instructions settles it. The SP1b-4
/// lockstep compares with an oracle that has no latency at all, and needs this to be
/// configurable.
pub const INTERRUPT_LATENCY_CYCLES: u64 = 4;

/// The instant, in cycles of the time base since power-on, at which a device-time limit of
/// `nanos` nanoseconds is reached: the first cycle boundary at or after that time, 4 cycles for
/// every 25 ns at 160 MHz. A whole number of microseconds is exact. A console script rounds its
/// waits with the same conversion.
///
/// It is never the `u64::MAX` at which device time saturates: the longest limit, 584 years, is
/// under a sixth of the time the base can count.
fn limit_instant(nanos: u64) -> Cycles {
    clock::TIME_BASE.nanos_to_cycles(u128::from(nanos))
}

/// Under the timing model `qemu-icount`, `ICOUNT_TIME_BASE_CYCLES` cycles of the time base last
/// as long as `ICOUNT_INSTRUCTIONS` instructions: 25 instructions of 8 ns (QEMU's `-icount 3`,
/// 2^3 ns each) are 200 ns, 32 cycles of 6.25 ns. Device time after n instructions is the whole
/// number of cycles at or below n × 8 ns: a counter of any clock that is a whole number of
/// time-base cycles (the SYSTIMER at 16 MHz, APB at 80 MHz) then reads what QEMU's, which
/// truncates n × 8 ns to its own period, reads.
const ICOUNT_TIME_BASE_CYCLES: u64 = 32;
const ICOUNT_INSTRUCTIONS: u64 = 25;
const _: () = assert!(
    clock::TIME_BASE.hz() * 8 * ICOUNT_INSTRUCTIONS == ICOUNT_TIME_BASE_CYCLES * 1_000_000_000
);

/// How the machine turns an instruction into device time ([`SocConfig::timing`]).
#[derive(Clone, Copy, Debug)]
enum Pricing {
    /// Each instruction lasts its CPU cycles at the CPU clock.
    Datasheet,
    /// Each instruction lasts 8 ns; `owed` is the fraction of a time-base cycle, in 25ths, the
    /// instructions so far have not yet moved device time by.
    QemuIcount { owed: u64 },
}

impl Pricing {
    fn model(self) -> &'static dyn TimingModel {
        match self {
            Pricing::Datasheet => &DatasheetTiming,
            Pricing::QemuIcount { .. } => &QemuIcountTiming,
        }
    }
}

/// What one turn of the run loop did.
enum Turn {
    /// Only when the turn was asked to stop at an instruction boundary: the hart is about to
    /// execute its next instruction, and nothing was done.
    Ready,
    /// The hart stepped (an instruction, an interrupt taken, or a wait), then the machine stops
    /// for `stop` when it is set.
    Stepped {
        outcome: StepOutcome,
        stop: Option<StopReason>,
    },
    /// The chip slept deeply for a while, then stops for `stop` when it is set.
    DeepSlept { stop: Option<StopReason> },
}

/// The sleeps one call has gone through, for the bounds of [`StopReason::Asleep`].
#[derive(Default)]
struct Slept {
    duration: u64,
    count: u32,
}

impl Slept {
    /// Counts one sleep of `duration` cycles; whether the call has slept enough to return.
    fn add(&mut self, duration: u64) -> bool {
        self.duration = self.duration.saturating_add(duration);
        self.count += 1;
        self.duration >= SLEEP_SLICE || self.count >= MAX_FRUITLESS_WAKES
    }
}

/// One instruction as [`Esp32C3::step`] executed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Executed {
    /// The address it was fetched from.
    pub pc: u32,
    /// The instruction as decoded; `None` when its fetch faulted.
    pub instruction: Option<Decoded>,
    /// The exception it raised, `None` when it retired.
    pub trap: Option<Trap>,
}

/// What one call of [`Esp32C3::step`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stepped {
    /// The instruction executed; `None` when the machine stopped before one.
    pub executed: Option<Executed>,
    /// The interrupt line the CPU took after it, before the next instruction.
    pub interrupt: Option<u32>,
    /// The CPU waited (`wfi`) after it, until a line ended the wait.
    pub slept: bool,
    /// The reason the machine stopped, if it did: the next instruction was not reached.
    pub stop: Option<StopReason>,
}

/// What one sleep of the CPU came to.
enum Sleep {
    /// Device time moved to the next instant something was due, by this many cycles of the time
    /// base.
    Until(u64),
    /// Nothing is due at any time.
    NothingDue,
    /// The next instant something is due is after the device-time limit of the call: device time
    /// is at the limit.
    CutAtDeadline,
}

/// An ESP32-C3: one hart, the bus with everything it reaches, and the model that prices each
/// instruction. It performs no host I/O: output is handed over through [`Esp32C3::take_console`].
#[derive(Debug)]
pub struct Esp32C3 {
    hart: Hart,
    bus: SocBus,
    pricing: Pricing,
    /// CPU cycles between the selection of an interrupt line and the instant the CPU can take it.
    interrupt_latency: u64,
    trace: Option<TraceHash>,
    /// The line the interrupt controller selects, and the device time from which the CPU sees it.
    selected: Option<(u32, Cycles)>,
    /// Resets since power-on.
    resets: u64,
    /// The channels [`Esp32C3::watch_console`] watches, by their place in [`ConsoleChannel::ALL`].
    watched: [bool; 3],
    /// The chip is in deep sleep: the CPU executes nothing until a reset (F1).
    powered_down: bool,
    /// The inputs the machine accepted since power-on (G1).
    journal: InputJournal,
    /// [`Esp32C3::step`] executed an instruction and stopped before reaching the boundary of the
    /// next one: the next call goes on to that boundary without executing anything.
    boundary_pending: bool,
    /// [`crate::QemuCompat::cycle_counter_on_time_base`]: kept here because a reset builds the
    /// hart's CSRs again, and the setting belongs to the machine.
    cycle_counter_on_time_base: bool,
    /// Reads of that counter by the CSR files a reset replaced.
    cycle_counter_reads: u64,
    /// The breakpoints of a debugger, sorted, without repetition ([`Esp32C3::set_breakpoints`]).
    breakpoints: Vec<u32>,
    /// The breakpoint the last call of [`Esp32C3::run_stopping`] stopped at, until the machine
    /// leaves it: the next call goes on past it.
    breakpoint_left: Option<u32>,
    /// What a resource monitor reads ([`Esp32C3::usage`]); nothing the run does reads it.
    counters: usage::Counters,
}

mod debug;
#[cfg(test)]
mod test_programs;
mod usage;
use usage::Asleep;

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
            core: Core::Esp32C3,
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
        let mut hart = Hart::new(RESET_PC, hart_config);
        if config.qemu_compat.cycle_counter_on_time_base {
            hart.csr.count_cycles_on_time_base();
        }
        let counters =
            usage::Counters::new(bus.ctx.clock().cpu_frequency().hz(), bus.memory_versions());
        Esp32C3 {
            hart,
            bus,
            pricing: match config.timing {
                Timing::Datasheet => Pricing::Datasheet,
                Timing::QemuIcount => Pricing::QemuIcount { owed: 0 },
            },
            interrupt_latency: config.interrupt_latency(),
            trace: config.trace_hash.then(TraceHash::new),
            selected: None,
            resets: 0,
            watched: [false; 3],
            powered_down: false,
            journal: InputJournal::new(config.usb_host),
            boundary_pending: false,
            cycle_counter_on_time_base: config.qemu_compat.cycle_counter_on_time_base,
            cycle_counter_reads: 0,
            breakpoints: Vec::new(),
            breakpoint_left: None,
            counters,
        }
    }

    /// How often each setting of [`SocConfig::qemu_compat`] acted since power-on.
    #[must_use]
    pub fn compat_acts(&self) -> CompatActs {
        let systimer = self.bus.periph.systimer.compat_acts();
        CompatActs {
            cycle_counter_reads: self.cycle_counter_reads + self.hart.csr.time_base_reads(),
            ..systimer
        }
    }

    /// Runs until the hart's cycle counter reaches `limit` (an absolute [`Esp32C3::cycles`] value) or
    /// the machine stops for another reason: [`Esp32C3::run_within`] without a device-time limit.
    pub fn run_until(&mut self, limit: Cycles) -> StopReason {
        self.run_within(limit, None)
    }

    /// Runs until the hart's cycle counter reaches `cycles` (an absolute [`Esp32C3::cycles`]
    /// value), until device time reaches `device_time_nanos` (nanoseconds since power-on, as
    /// [`Esp32C3::device_time_nanos`] counts them: they go on while the CPU sleeps and across
    /// resets; `None` is no limit), or until the machine stops for another reason.
    ///
    /// The device-time limit is an instant, the same whatever the calls a run is cut into: the
    /// number of nanoseconds rounded up to a cycle of the time base, so that a run never ends
    /// before the time asked.
    /// - While the CPU executes, the call returns [`StopReason::TimeBudget`] at the first
    ///   instruction boundary at which device time is at or past the instant. The instruction
    ///   that crosses it is executed whole, as the last one of a cycle limit is.
    /// - While the CPU sleeps, a wake-up due at or before the instant is delivered as without a
    ///   limit. When the next one is after it, device time is set to exactly the instant, nothing
    ///   is delivered, the CPU stays asleep and the call returns [`StopReason::TimeBudget`].
    /// - A CPU nothing can wake is [`StopReason::Idle`] whatever the limit, and device time is
    ///   not moved to it.
    /// - The cycle limit is tested first: at a boundary where both limits are reached the call
    ///   returns [`StopReason::CycleBudget`].
    /// - The limit is looked at before an instruction and before a sleep. A call that has reached
    ///   one of the bounds of [`StopReason::Asleep`] returns that reason even when its last sleep
    ///   ended on the instant of the limit: the next call returns [`StopReason::TimeBudget`]
    ///   without running anything. A caller goes on after `Asleep` and `CycleBudget` and passes
    ///   the same limit to every call.
    pub fn run_within(&mut self, cycles: Cycles, device_time_nanos: Option<u64>) -> StopReason {
        match self.run_loop::<debug::NoStops>(cycles, device_time_nanos) {
            Ok(reason) => reason,
            Err(never) => match never {},
        }
    }

    /// The loop of [`Esp32C3::run_within`], and of [`Esp32C3::run_stopping`] with the points of
    /// a debugger: `S` may end the call at a boundary, before or after a turn, with its own
    /// reason. It never changes what a turn does.
    fn run_loop<S: debug::Stops>(
        &mut self,
        cycles: Cycles,
        device_time_nanos: Option<u64>,
    ) -> Result<StopReason, S::Hit> {
        let deadline = device_time_nanos.map(limit_instant);
        let mut slept = Slept::default();
        // A run goes through boundaries as it meets them: a boundary a step left pending is
        // reached by its first turns, as a step would reach it.
        self.boundary_pending = false;
        S::begin(self)?;
        while self.hart.cycles.get() < cycles.get() {
            if deadline.is_some_and(|deadline| self.bus.ctx.now() >= deadline) {
                return Ok(StopReason::TimeBudget);
            }
            S::before(self)?;
            match self.turn(deadline, &mut slept, false) {
                Turn::Stepped {
                    stop: Some(reason), ..
                }
                | Turn::DeepSlept { stop: Some(reason) } => return Ok(reason),
                Turn::Ready | Turn::Stepped { stop: None, .. } | Turn::DeepSlept { stop: None } => {
                }
            }
            S::after(self)?;
        }
        Ok(StopReason::CycleBudget)
    }

    /// Executes the next instruction as [`Esp32C3::run_within`] would, then brings the machine to
    /// the boundary of the instruction after it: the interrupt the CPU takes before that one is
    /// taken, and a `wfi` is slept through until a line ends it. The state read afterwards
    /// ([`Esp32C3::pc`], [`Esp32C3::x`], [`Esp32C3::csr`]) is the state the next instruction
    /// starts from: what a debugger shows, and what a lockstep compares (decision H1).
    ///
    /// A call ends when that boundary is reached, or when the machine stops on its own: the CPU
    /// waits and nothing can wake it ([`StopReason::Idle`]), it has slept through one
    /// [`SLEEP_SLICE`] or [`MAX_FRUITLESS_WAKES`] sleeps ([`StopReason::Asleep`]), the chip is in
    /// a deep sleep nothing ends ([`StopReason::DeepSleep`]), a watched console has output
    /// ([`StopReason::ConsoleOutput`]), or `--strict-mmio` met something unmodelled. A call
    /// made away from a boundary (after [`Esp32C3::run_within`] left an interrupt to take) takes
    /// it first, as a run would. There is no limit of cycles or device time: the bounds of a
    /// sleep are the limit.
    ///
    /// A call that stops after its instruction, before the boundary (a long sleep that returns
    /// [`StopReason::Asleep`], a console output), leaves that boundary pending: the next call
    /// executes nothing, goes on to it, and reports the interrupt and the sleep met on the way
    /// with no [`Stepped::executed`]. A caller that adds the calls up until one returns no
    /// [`StopReason::Asleep`] gets what one call without the bound would have given.
    ///
    /// For a debugger: a step notes its own access to watched bytes only
    /// ([`Esp32C3::take_watch_hit`]), and the run that follows it goes past a breakpoint at the
    /// boundary it reached, as the run that follows a breakpoint stop does.
    pub fn step(&mut self) -> Stepped {
        self.bus.take_watch_hit();
        let stepped = self.step_to_boundary();
        self.breakpoint_left = Some(self.hart.pc);
        stepped
    }

    /// [`Esp32C3::step`], without what it does for a debugger.
    fn step_to_boundary(&mut self) -> Stepped {
        let mut slept = Slept::default();
        let mut stepped = Stepped {
            executed: None,
            interrupt: None,
            slept: false,
            stop: None,
        };
        if !self.boundary_pending
            && let Some(stop) = self.step_instruction(&mut slept, &mut stepped)
        {
            stepped.stop = Some(stop);
            return stepped;
        }
        // The boundary of the next instruction: a sleep, and the interrupt taken.
        loop {
            match self.turn(None, &mut slept, true) {
                Turn::Ready => {
                    self.boundary_pending = false;
                    return stepped;
                }
                Turn::Stepped { outcome, stop } => {
                    match outcome {
                        StepOutcome::Interrupted { cause } => stepped.interrupt = Some(cause),
                        StepOutcome::Waiting => stepped.slept = true,
                        StepOutcome::Retired { .. } | StepOutcome::Trapped { .. } => {}
                    }
                    if stop.is_some() {
                        stepped.stop = stop;
                        return stepped;
                    }
                }
                Turn::DeepSlept { stop } => {
                    if stop.is_some() {
                        stepped.stop = stop;
                        return stepped;
                    }
                }
            }
        }
    }

    /// The instruction of [`Esp32C3::step`], with the turns a call made away from a boundary
    /// takes before it. Returns the reason to stop, if there is one; the instruction executed,
    /// when there was one, is in `stepped`, and the boundary after it is then pending.
    fn step_instruction(&mut self, slept: &mut Slept, stepped: &mut Stepped) -> Option<StopReason> {
        loop {
            let pc = self.hart.pc;
            match self.turn(None, slept, false) {
                Turn::Stepped { outcome, stop } => {
                    let trap = match outcome {
                        StepOutcome::Retired { .. } => None,
                        StepOutcome::Trapped { trap, .. } => Some(trap),
                        StepOutcome::Interrupted { .. } | StepOutcome::Waiting => {
                            if stop.is_some() {
                                return stop;
                            }
                            continue;
                        }
                    };
                    stepped.executed = Some(Executed {
                        pc,
                        instruction: self.hart.last_instruction(),
                        trap,
                    });
                    self.boundary_pending = true;
                    return stop;
                }
                Turn::DeepSlept { stop: Some(stop) } => return Some(stop),
                Turn::DeepSlept { stop: None } | Turn::Ready => {}
            }
        }
    }

    /// One turn of the run loop: a step of the hart and what the machine does after it, or one
    /// step of a deep sleep. With `boundary_only`, the hart only acts on what stands before its
    /// next instruction ([`Hart::take_boundary`]) and the turn is [`Turn::Ready`] when nothing
    /// does.
    fn turn(&mut self, deadline: Option<Cycles>, slept: &mut Slept, boundary_only: bool) -> Turn {
        // The machine leaves the boundary a breakpoint stopped it at: from the next boundary on,
        // the breakpoints stop it again (`debug::DebugStops::before`).
        self.breakpoint_left = None;
        // The board's supply failed: nothing runs any more (ADR 0013).
        if self.bus.periph.io.power_lost().is_some() {
            return Turn::DeepSlept {
                stop: Some(StopReason::PowerOff),
            };
        }
        if self.powered_down {
            return self.deep_sleep_turn(deadline, slept);
        }
        let pc = self.hart.pc;
        self.bus.set_pc(pc);
        let outcome = if boundary_only {
            match self.hart.take_boundary() {
                Boundary::Ready => return Turn::Ready,
                Boundary::Interrupted { cause } => StepOutcome::Interrupted { cause },
                Boundary::Waiting => StepOutcome::Waiting,
            }
        } else {
            self.step_hart(pc)
        };
        match outcome {
            StepOutcome::Waiting => {
                return Turn::Stepped {
                    outcome,
                    stop: self.wait(pc, deadline, slept),
                };
            }
            StepOutcome::Interrupted { cause } => {
                self.counters.interrupt_taken(cause);
                // The CPU took the line: the controller marks the edge it had latched for it as
                // claimed (the latch stays until the firmware writes `CPU_INT_CLEAR` with
                // the bit of the line set), and the lines are evaluated again.
                self.bus.periph.intmatrix.claim(cause);
                self.bus.ctx.interrupts_changed();
            }
            StepOutcome::Retired { .. } | StepOutcome::Trapped { .. } => {}
        }
        if let Some(trace) = &mut self.trace {
            trace.mix_u32(pc);
        }
        if self.needs_service() {
            self.service(pc);
            // A watched byte leaves at an instant its device asked to be called at: the
            // service of that call is the boundary it is seen at.
            if self.watched_output() {
                let stop = match self.bus.ctx.diag.take_violation() {
                    Some(violation) => StopReason::StrictMmio(violation),
                    None => StopReason::ConsoleOutput,
                };
                return Turn::Stepped {
                    outcome,
                    stop: Some(stop),
                };
            }
        }
        Turn::Stepped {
            outcome,
            stop: self
                .bus
                .ctx
                .diag
                .take_violation()
                .map(StopReason::StrictMmio),
        }
    }

    /// One step of a deep sleep: device time jumps to the next call of the RTC domain, the one
    /// part of the chip with power, without an instruction (F1).
    fn deep_sleep_turn(&mut self, deadline: Option<Cycles>, slept: &mut Slept) -> Turn {
        let duration = match self.sleep_deeply(deadline) {
            Sleep::Until(duration) => duration,
            Sleep::NothingDue => {
                return Turn::DeepSlept {
                    stop: Some(StopReason::DeepSleep),
                };
            }
            Sleep::CutAtDeadline => {
                return Turn::DeepSlept {
                    stop: Some(StopReason::TimeBudget),
                };
            }
        };
        let enough = slept.add(duration);
        let stop = if let Some(violation) = self.bus.ctx.diag.take_violation() {
            Some(StopReason::StrictMmio(violation))
        } else if self.watched_output() {
            Some(StopReason::ConsoleOutput)
        } else {
            enough.then_some(StopReason::Asleep)
        };
        Turn::DeepSlept { stop }
    }

    /// One step of the hart at `pc`: an instruction, an interrupt taken, or a wait. An exception
    /// is reported, and device time moves on by what the step cost.
    fn step_hart(&mut self, pc: u32) -> StepOutcome {
        // The stack pointer the instruction started with, for the report of an exception. It is
        // read before the step and must stay there: after it the value would be the same only for
        // as long as a faulting instruction leaves every register untouched, which is the hart's
        // business, not this loop's.
        let sp = self.hart.x[STACK_POINTER];
        let before = self.hart.cycles.get();
        if self.cycle_counter_on_time_base {
            // The instant of an instruction is the time of the instructions before it.
            self.hart.csr.set_time_base(self.bus.ctx.now().get());
        }
        let outcome = self.hart.step(&mut self.bus, self.pricing.model());
        if let StepOutcome::Trapped { trap, .. } = outcome
            // An `ecall` is a request to the trap handler, not a fault worth reporting.
            && !trap.cause.is_environment_call()
        {
            self.bus.ctx.diag.record(Diagnostic::Exception {
                cause: trap.cause,
                tval: trap.tval,
                pc,
                sp,
            });
        }
        if !matches!(outcome, StepOutcome::Waiting) {
            self.advance_device_time(outcome, self.hart.cycles.get() - before);
        }
        outcome
    }

    /// The CPU idles: device time jumps to the next instant a device asked for, and what was due
    /// then is served. Returns the reason to stop, if there is one.
    fn wait(&mut self, pc: u32, deadline: Option<Cycles>, slept: &mut Slept) -> Option<StopReason> {
        let duration = match self.sleep(deadline) {
            Sleep::Until(duration) => duration,
            Sleep::NothingDue => return Some(StopReason::Idle),
            Sleep::CutAtDeadline => return Some(StopReason::TimeBudget),
        };
        let enough = slept.add(duration);
        self.service(pc);
        if let Some(violation) = self.bus.ctx.diag.take_violation() {
            Some(StopReason::StrictMmio(violation))
        } else if self.watched_output() {
            Some(StopReason::ConsoleOutput)
        } else {
            enough.then_some(StopReason::Asleep)
        }
    }

    /// Device time follows the CPU: the cycles of what the hart did, at the clock it ran on, or
    /// 8 ns per instruction executed under `qemu-icount`, where taking an interrupt costs none.
    ///
    /// Under `qemu-icount` an instruction whose memory access faults costs nothing either: QEMU
    /// raises such a fault through `cpu_loop_exit_restore`, whose `cpu_restore_state_from_tb`
    /// gives back to the instruction counter the instructions of the block not executed, the
    /// faulting one included (`accel/tcg/translate-all.c`, `cpu_restore_state_from_tb`, branch
    /// `esp-develop` of `espressif/qemu`), and a fetch fault happens before its block runs. An
    /// `ecall`, an `ebreak` and an illegal instruction go through `helper_raise_exception`, which
    /// restores nothing (`target/riscv/op_helper.c`): they are counted.
    fn advance_device_time(&mut self, outcome: StepOutcome, cpu_cycles: u64) {
        match &mut self.pricing {
            Pricing::Datasheet => self.bus.ctx.advance(cpu_cycles),
            Pricing::QemuIcount { owed } => {
                let counted = match outcome {
                    StepOutcome::Retired { .. } => true,
                    StepOutcome::Trapped { trap, .. } => !matches!(
                        trap.cause,
                        Exception::InstrAccessFault
                            | Exception::LoadMisaligned
                            | Exception::LoadAccessFault
                            | Exception::StoreMisaligned
                            | Exception::StoreAccessFault
                    ),
                    StepOutcome::Interrupted { .. } | StepOutcome::Waiting => false,
                };
                if counted {
                    *owed += ICOUNT_TIME_BASE_CYCLES;
                    let cycles = *owed / ICOUNT_INSTRUCTIONS;
                    *owed %= ICOUNT_INSTRUCTIONS;
                    self.bus.ctx.advance_time_base(cycles);
                }
            }
        }
    }

    /// Whether a watched channel has handed bytes to the host that no call has taken.
    fn watched_output(&self) -> bool {
        let periph = &self.bus.periph;
        let left = [
            periph.uart0.has_left(),
            periph.usb_serial_jtag.has_left(),
            periph.uart1.has_left(),
        ];
        self.watched
            .iter()
            .zip(left)
            .any(|(&watched, left)| watched && left)
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
        // One pass, bounded by the wake-ups that were due when it began: a wake-up a device arms
        // during the pass, for any instant, is not delivered by it; a later pass delivers it,
        // the next one when it is due already (`Ctx::deliver_due`).
        self.bus.deliver_due();
        if let Some(cause) = self.bus.ctx.take_reset() {
            self.reset(cause);
        }
        if self.bus.ctx.take_pad_hold_written() {
            let held = self.bus.periph.rtc_cntl.pad_hold_mask();
            self.bus.periph.io.set_hold(&mut self.bus.ctx, held);
        }
        // After the reset: a reset asked for by the same instruction ends the sleep at once.
        if self.bus.ctx.take_deep_sleep_entered() {
            self.powered_down = self.bus.periph.rtc_cntl.is_in_deep_sleep();
            if self.powered_down {
                // The console channels go no further than this instant whatever a front end
                // calls during the sleep: where a run is cut changes nothing (F10).
                self.bus.periph.power_down(&self.bus.ctx);
                // A wake pin already at its level wakes the chip at once (ADR 0013).
                self.check_gpio_wake();
                if let Some(cause) = self.bus.ctx.take_reset() {
                    self.reset(cause);
                }
            }
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
        self.counters.sources_sampled(sources);
        let line = self.bus.periph.intmatrix.resolve(sources);
        self.selected = match (line, self.selected) {
            (None, _) => None,
            // The same line as before: the instant it became visible stands.
            (Some(line), Some((previous, visible_at))) if line == previous => {
                Some((line, visible_at))
            }
            (Some(line), _) => {
                let latency = self
                    .interrupt_latency
                    .saturating_mul(self.bus.ctx.clock().cpu_period());
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
    /// the end of the latency of the selected interrupt line. When that instant is after
    /// `deadline`, the device-time limit of the call, device time moves to the deadline and no
    /// further: the caller returns without delivering anything.
    ///
    /// The cycle counter does not move while the CPU waits: the cycle event of the performance
    /// counter "does not increment during WFI mode" (TRM register 1.12, as quoted in report IR
    /// §2.6).
    fn sleep(&mut self, deadline: Option<Cycles>) -> Sleep {
        let now = self.bus.ctx.now();
        let latency_end = self.selected.map(|(_, visible_at)| visible_at);
        let wake = match (self.bus.ctx.next_wake(), latency_end) {
            (Some(a), Some(b)) => a.min(b),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => return Sleep::NothingDue,
        };
        // A wake-up already due is delivered without moving time.
        let target = wake.max(now);
        if let Some(deadline) = deadline.filter(|deadline| target > *deadline) {
            self.sleep_to(deadline, Asleep::Waiting);
            return Sleep::CutAtDeadline;
        }
        self.sleep_to(target, Asleep::Waiting);
        Sleep::Until(target.get() - now.get())
    }

    /// One step of a deep sleep: device time moves to the next call of `RTC_CNTL`, which is
    /// called alone, or to `deadline` when that call comes after it. Nothing is due when the block
    /// asked for no call: nothing the model has can wake the chip (F5).
    ///
    /// The digital devices have no power while the chip sleeps (TRM §9.4.1, table 9.4-1), so none
    /// of them is called: a main watchdog left running does not reset a chip that sleeps, and a
    /// timer does not fire. Their wake-ups that fall due meanwhile stay in the scheduler and are
    /// delivered after the core reset of the wake, to devices that reset rebuilt, which find
    /// nothing to do (the contract of [`Event`]). A reset `RTC_CNTL` asks for (the wake, or its
    /// watchdog when it is left running in sleep) ends the sleep and rebuilds the digital domain,
    /// whatever its level (`Peripherals::reset`); anything else it raises (an
    /// interrupt) waits, with the CPU, for a reset.
    ///
    /// The chips of the board have power whatever the chip does: a clocked chip (ADR 0012) whose
    /// deadline comes first is advanced at it, alone, without the pads being worked out (their
    /// interrupts belong to the digital domain), and the sleep goes on. Nothing is due only when
    /// neither the RTC domain nor a clocked chip has anything to come. A chip always due (whose
    /// deadline never moves past the instant it is advanced to) is advanced at every cycle of the
    /// time base, so a sleep with it is never [`Sleep::NothingDue`]: only `deadline` (the
    /// device-time limit of the call, `--max-time`) or the RTC domain ends it, and the chip is
    /// named once as stuck ([`Diagnostic::ClockedChipStuck`]).
    ///
    /// When a chip's deadline and the call of the RTC domain fall at one instant, the chip is
    /// advanced first, once, then the RTC domain is called (and its reset, if it asks for one,
    /// rebuilds the digital domain).
    fn sleep_deeply(&mut self, deadline: Option<Cycles>) -> Sleep {
        let now = self.bus.ctx.now();
        let call = self.bus.periph.rtc_cntl.next_call();
        let chips = self.bus.periph.io.clocked_deadline(&self.bus.ctx);
        let pins = self
            .bus
            .periph
            .rtc_cntl
            .gpio_wake_levels()
            .and_then(|wanted| self.bus.periph.io.wake_pins_change(&self.bus.ctx, &wanted));
        let board = [chips, pins].into_iter().flatten().min();
        let next = match (call, board) {
            (None, None) => return Sleep::NothingDue,
            (Some(a), Some(b)) => a.min(b),
            (Some(only), None) | (None, Some(only)) => only,
        };
        let target = next.max(now);
        if let Some(deadline) = deadline.filter(|deadline| target > *deadline) {
            self.sleep_to(deadline, Asleep::Deeply);
            return Sleep::CutAtDeadline;
        }
        self.sleep_to(target, Asleep::Deeply);
        if board.is_some_and(|board| board <= target) {
            // The chips and the board's inputs first, then the wake pins, which they may have
            // brought to their level (ADR 0013).
            self.bus.periph.io.advance_clocked(&mut self.bus.ctx);
            self.check_gpio_wake();
            if let Some(cause) = self.bus.ctx.take_reset() {
                let pc = self.hart.pc;
                self.reset(cause);
                self.service(pc);
                return Sleep::Until(target.get() - now.get());
            }
        }
        if call.is_some_and(|call| call <= target) {
            self.bus.wake_rtc_domain();
            if let Some(cause) = self.bus.ctx.take_reset() {
                let pc = self.hart.pc;
                self.reset(cause);
                // Awake: the clock of the reset, the interrupt lines, the stale wake-ups.
                self.service(pc);
            }
        }
        Sleep::Until(target.get() - now.get())
    }

    /// The pins that wake a sleeping chip and are at their wake level now wake it: `RTC_CNTL`
    /// latches the cause and asks for the reset of cause 5 (ADR 0013).
    fn check_gpio_wake(&mut self) {
        let Some(wanted) = self.bus.periph.rtc_cntl.gpio_wake_levels() else {
            return;
        };
        let pins = self.bus.periph.io.wake_levels(&self.bus.ctx, &wanted);
        self.bus
            .periph
            .rtc_cntl
            .wake_on_pins(&mut self.bus.ctx, pins);
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
        // Spec §7: a reset leaves an entry in the journal, with the PC and the stack it found,
        // and the device time that sets it apart from the other resets at the same PC.
        let at_us = self.device_time_nanos() / 1_000;
        let woken_by = if cause == ResetCause::DeepSleep {
            self.bus.periph.rtc_cntl.latched_wake()
        } else {
            None
        };
        self.bus.ctx.diag.record(Diagnostic::Reset {
            cause,
            pc,
            sp,
            at_us,
            woken_by,
        });
        self.bus.reset(cause, pc, sp);
        // A system reset clears the holds of `RTC_CNTL`; a core reset keeps them, and the held
        // pads keep their output through it.
        let held = self.bus.periph.rtc_cntl.pad_hold_mask();
        self.bus.periph.io.set_hold(&mut self.bus.ctx, held);
        // The flash-window page versions (`SocBus::page_version`) restart at 0 with the MMU and
        // the cache registers: a version can repeat with another mapping. That is safe only
        // because `Hart::reset` builds a new decode cache, at every level.
        self.cycle_counter_reads += self.hart.csr.time_base_reads();
        self.hart.reset(RESET_PC);
        if self.cycle_counter_on_time_base {
            self.hart.csr.count_cycles_on_time_base();
        }
        // A CPU reset is taken to leave the interrupt controller and the source levels as they
        // are (the assumption marked on `Peripherals::reset`, with what settles it), and the hart
        // forgot the line it was shown: the next `service` selects again. The line becomes
        // visible 4 CPU cycles after the reset, as one that has just become selected does.
        self.selected = None;
        self.bus.ctx.interrupts_changed();
        self.resets += 1;
        self.counters.reset(self.bus.memory_versions());
        // Every reset ends a deep sleep: `RTC_CNTL` ended it with the reset.
        self.powered_down = false;
    }

    /// Applies a write to the clock registers: from the end of the instruction that wrote them,
    /// cycles last what the new selection says.
    fn retune(&mut self, pc: u32) {
        let (sysclk_conf, cpu_per_conf) = self.bus.periph.system.clock_registers();
        match clock::cpu_clock(sysclk_conf, cpu_per_conf) {
            Some(selected) => {
                // The devices that time with APB_CLK come to now at the old rate first.
                self.bus.periph.clock_changing(&mut self.bus.ctx);
                self.bus.ctx.set_clock(selected);
                self.counters
                    .clock_changed(self.bus.ctx.now(), selected.cpu_frequency().hz());
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

    /// Whether the CPU is asleep: it executed `wfi` and no interrupt has ended the wait yet.
    /// [`Esp32C3::pc`] is then the instruction after the `wfi`.
    #[must_use]
    pub const fn is_asleep(&self) -> bool {
        self.hart.is_waiting()
    }

    /// The deep sleep the chip is in, if any (F9): since when, until when the RTC timer lets it
    /// sleep, and the wake sources the firmware enabled. A front end shows it; the wake comes as
    /// a reset of cause 5 in the diagnostics ([`Diagnostic::Reset`]).
    #[must_use]
    pub fn deep_sleep(&self) -> Option<DeepSleep> {
        let nanos = |instant: Cycles| clock::TIME_BASE.cycles_to_nanos(instant);
        self.bus
            .periph
            .rtc_cntl
            .deep_sleep(self.bus.ctx.now())
            .map(|state| DeepSleep {
                since_nanos: nanos(state.since),
                timer_wake_nanos: state.timer_wake.map(nanos),
                wake_sources: state.wake_sources,
            })
    }

    /// The bytes that left the device on `channel` since the last call: a UART byte counts once
    /// its last bit has gone. While the chip is in deep sleep the channels have no power and go
    /// no further than the instant it went to sleep: a byte still on its way out then leaves at
    /// the reset that ends the sleep, wherever the calls cut the run.
    ///
    /// A device keeps the bytes that left until they are taken here, and firmware decides how
    /// many there are. A caller must therefore take **every** channel ([`ConsoleChannel::ALL`]),
    /// the ones it does not show included, at least once per call of [`Esp32C3::run_within`], and
    /// keep each call short enough for what it is ready to hold: nothing else bounds the memory
    /// of a run whose firmware writes without end.
    pub fn take_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
        match channel {
            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(&mut self.bus.ctx),
            ConsoleChannel::UsbSerialJtag => {
                self.bus.periph.usb_serial_jtag.take_tx(&mut self.bus.ctx)
            }
            ConsoleChannel::Uart1 => self.bus.periph.uart1.take_tx(&mut self.bus.ctx),
        }
    }

    /// Everything the firmware wrote on `channel` and no call has taken, the bytes that have not
    /// left the device yet included. For the end of a run: without them the console would stop
    /// short of the place the run stopped at. It ends the transmission: do not go on running a
    /// machine whose console was flushed if the timing of its output matters.
    pub fn flush_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
        match channel {
            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_written(),
            ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_written(),
            ConsoleChannel::Uart1 => self.bus.periph.uart1.take_written(),
        }
    }

    /// Device time since power-on, in cycles of the time base ([`clock::TIME_BASE`]): the unit
    /// the inputs of a run are dated in.
    #[must_use]
    pub const fn device_time(&self) -> Cycles {
        self.bus.ctx.now()
    }

    /// The host sends `bytes` on `channel`, the first from device time `at`, the others after
    /// it and after what it sent before (ADR 0006): a UART receives them back to back at its
    /// line rate (report UR §5.4), USB-Serial-JTAG in packets of at most 64 bytes, no byte at all
    /// being one zero-length packet (report USJ §R.5). An input is part of the run: two runs
    /// given the same inputs at the same dates are the same run, however they are cut into
    /// calls.
    ///
    /// # Errors
    /// [`InputError::InThePast`] when `at` is before [`Esp32C3::device_time`];
    /// [`InputError::BeyondTheLine`] when a UART is given a date after
    /// [`crate::periph::uart::LAST_INPUT_DATE`], about 114 years of device time;
    /// [`InputError::TooMuchPending`] when the channel would hold more than
    /// [`MAX_PENDING_HOST_BYTES`] bytes of the host that have not reached the device
    /// ([`Esp32C3::input_pending`]), a zero-length packet of USB-Serial-JTAG counting as one.
    /// Nothing is sent then.
    pub fn send_console(
        &mut self,
        channel: ConsoleChannel,
        at: Cycles,
        bytes: &[u8],
    ) -> Result<(), InputError> {
        self.check_date(at)?;
        if channel != ConsoleChannel::UsbSerialJtag && at > crate::periph::uart::LAST_INPUT_DATE {
            return Err(InputError::BeyondTheLine {
                channel,
                at_nanos: clock::TIME_BASE.cycles_to_nanos(at),
                last_nanos: clock::TIME_BASE.cycles_to_nanos(crate::periph::uart::LAST_INPUT_DATE),
            });
        }
        // No byte at all is a packet on USB-Serial-JTAG, and nothing on a UART.
        let added = match channel {
            ConsoleChannel::UsbSerialJtag => bytes.len().max(1),
            ConsoleChannel::Uart0 | ConsoleChannel::Uart1 => bytes.len(),
        };
        let pending = self.input_pending(channel).saturating_add(added);
        if pending > MAX_PENDING_HOST_BYTES {
            return Err(InputError::TooMuchPending { channel, pending });
        }
        let (periph, ctx) = (&mut self.bus.periph, &mut self.bus.ctx);
        match channel {
            ConsoleChannel::Uart0 => periph.uart0.receive(ctx, at, bytes),
            ConsoleChannel::UsbSerialJtag => periph.usb_serial_jtag.receive(ctx, at, bytes),
            ConsoleChannel::Uart1 => periph.uart1.receive(ctx, at, bytes),
        }
        // Journaled once accepted, never before a check: a refused input is no part of the run.
        self.journal.record_console(channel, at, bytes);
        Ok(())
    }

    /// The inputs the machine accepted since power-on, in the order it accepted them, and the
    /// state of the USB host at power-on (G1): what a session records
    /// ([`InputJournal::is_complete`] says whether it holds them all).
    #[must_use]
    pub const fn input_journal(&self) -> &InputJournal {
        &self.journal
    }

    /// The machine keeps no journal of its inputs from now on, and lets go of what it holds: for
    /// a machine whose run will not be recorded, such as the replay of a session, which hands
    /// over inputs the session holds already and would otherwise hold them twice. The journal
    /// says so ([`InputJournal::is_forgone`]) and holds nothing; the inputs are accepted as
    /// before.
    pub fn forgo_input_journal(&mut self) {
        self.journal.forgo();
    }

    /// The bytes of the host on `channel` that have not reached the device yet, the same count on
    /// every channel: on a UART those still to start and the one on the line, on
    /// USB-Serial-JTAG the packets not yet in the OUT FIFO, a zero-length one counting as one
    /// byte. What reached the receive FIFO is the firmware's to read, and is not counted. A
    /// front end that ends a run says when some are left.
    #[must_use]
    pub fn input_pending(&self, channel: ConsoleChannel) -> usize {
        let periph = &self.bus.periph;
        match channel {
            ConsoleChannel::Uart0 => periph.uart0.host_pending(),
            ConsoleChannel::UsbSerialJtag => periph.usb_serial_jtag.host_pending(),
            ConsoleChannel::Uart1 => periph.uart1.host_pending(),
        }
    }

    /// From device time `at` the USB host is `state` (ADR 0006; report USJ §R.2).
    ///
    /// # Errors
    /// [`InputError::InThePast`] when `at` is before [`Esp32C3::device_time`];
    /// [`InputError::TooManyHostChanges`] when [`MAX_PENDING_HOST_CHANGES`] changes are still to
    /// come, those due by now not counted, except during a deep sleep, where the block applies
    /// them at the reset that ends the sleep and they count until then. Nothing changes then.
    pub fn set_usb_host(&mut self, at: Cycles, state: UsbHost) -> Result<(), InputError> {
        self.check_date(at)?;
        let usb = &mut self.bus.periph.usb_serial_jtag;
        if usb.host_changes_pending(&mut self.bus.ctx) >= MAX_PENDING_HOST_CHANGES {
            return Err(InputError::TooManyHostChanges);
        }
        usb.set_host(&mut self.bus.ctx, at, state);
        self.journal.record_usb_host(at, state);
        Ok(())
    }

    /// The USB host now, the changes due by now applied.
    pub fn usb_host(&mut self) -> UsbHost {
        self.bus.periph.usb_serial_jtag.host(&mut self.bus.ctx)
    }

    /// Attaches a chip that drives pin `pin` (a button, a BUSY line, a detection line): wherever
    /// the pad is an input, the `SoC` reads the level the chip gives (ADR 0012). A chip whose level
    /// changes by itself says when (`GpioInput::next_change`); one changed from outside the run
    /// is read again by [`Esp32C3::inputs_changed`].
    /// # Errors
    /// [`AttachError`] when the pin does not exist or a chip already drives it.
    pub fn attach_input(&mut self, pin: u8, chip: Box<dyn GpioInput>) -> Result<(), AttachError> {
        self.bus
            .periph
            .io
            .attach_input(&mut self.bus.ctx, pin, chip)
    }

    /// Attaches a chip that watches pin `pin` (a reset line, a power latch): it hears at once what
    /// the `SoC` drives there, then every change.
    /// # Errors
    /// [`AttachError`] when the pin does not exist.
    pub fn attach_output(&mut self, pin: u8, chip: Box<dyn GpioOutput>) -> Result<(), AttachError> {
        self.bus
            .periph
            .io
            .attach_output(&mut self.bus.ctx, pin, chip)
    }

    /// Attaches a chip to GP-SPI2 by the pins of its lines: it receives each transaction while
    /// its chip select is low, with its data/command line, at the instant the transaction ends.
    /// # Errors
    /// [`AttachError`] when one of the pins does not exist.
    pub fn attach_spi(
        &mut self,
        wires: SpiWires,
        chip: Box<dyn SpiTarget>,
    ) -> Result<(), AttachError> {
        self.bus
            .periph
            .io
            .attach_spi(&mut self.bus.ctx, wires, chip)
    }

    /// Attaches a chip that puts a voltage on pin `pin`, one of GPIO0 to GPIO5: the SAR ADC
    /// converts it.
    /// # Errors
    /// [`AttachError`] when the pin has no ADC channel or already has an analog chip.
    pub fn attach_analog(
        &mut self,
        pin: u8,
        chip: Box<dyn AnalogSource>,
    ) -> Result<(), AttachError> {
        self.bus.periph.io.attach_analog(pin, chip)
    }

    /// Attaches a chip that does work of its own over time (a display controller running a
    /// refresh): the machine advances it to each deadline it names, in device time, whether the
    /// CPU runs, waits for an interrupt, sleeps until a timer or the chip is in deep sleep (the
    /// board keeps its chips powered), and asks for its next deadline
    /// after each call to it (ADR 0012). The same chip is usually attached by its lines too,
    /// shared through `Rc<RefCell<_>>`: whatever a call on a line changes, its deadline is asked
    /// again. A chip is advanced at most once at one instant.
    pub fn attach_clocked(&mut self, chip: Box<dyn Clocked>) {
        self.bus.periph.io.attach_clocked(&mut self.bus.ctx, chip);
    }

    /// Attaches the inputs of the board's user (the keys, the power cable: ADR 0013), which
    /// [`Esp32C3::send_board_input`] then takes. A second call replaces the first.
    pub fn attach_board_inputs(&mut self, inputs: Box<dyn BoardInputs>) {
        self.bus.periph.io.attach_board_inputs(inputs);
    }

    /// Attaches the supply of the board (ADR 0013): it is asked at once, then after each change of
    /// what the chip drives on a pin and each input of the board that acts. Once it says the chip
    /// has no power, the run stops for good ([`StopReason::PowerOff`]).
    pub fn attach_supply(&mut self, supply: Box<dyn Supply>) {
        self.bus.periph.io.attach_supply(&mut self.bus.ctx, supply);
    }

    /// The user acts on the board at device time `at` (a key, the power cable): the input is
    /// journaled and acts at the next instant of the time base after `at`, the instruction under
    /// way then ended (ADR 0013). A front end gives the present time; a replay gives every input
    /// at power-on with its date.
    ///
    /// # Errors
    /// [`InputError::InThePast`] for an input dated before the present time;
    /// [`InputError::PoweredOff`] once the supply failed; [`InputError::NoBoard`] with no board
    /// inputs attached; [`InputError::Board`] when the board refuses it;
    /// [`InputError::TooManyBoardInputs`] when [`MAX_PENDING_BOARD_INPUTS`] wait already;
    /// [`InputError::BeyondTime`] for an input dated so late it would act at the last instant of
    /// the time base. Nothing is taken then.
    pub fn send_board_input(&mut self, at: Cycles, input: BoardInput) -> Result<(), InputError> {
        self.check_date(at)?;
        // It acts at `at + 1`, which must stay below the last instant of the time base.
        if at.get() >= u64::MAX - 1 {
            return Err(InputError::BeyondTime {
                at_nanos: clock::TIME_BASE.cycles_to_nanos(at),
            });
        }
        self.bus
            .periph
            .io
            .queue_board_input(&mut self.bus.ctx, at, input)
            .map_err(|refusal| match refusal {
                BoardRefusal::NoBoard => InputError::NoBoard,
                BoardRefusal::Board(error) => InputError::Board(error),
                BoardRefusal::TooMany => InputError::TooManyBoardInputs,
            })?;
        self.journal.record_board(at, input);
        Ok(())
    }

    /// The inputs of the board that have not acted yet.
    #[must_use]
    pub fn board_inputs_pending(&self) -> usize {
        self.bus.periph.io.board_inputs_pending()
    }

    /// Device time at which the board's supply failed, in nanoseconds since power-on, once it has.
    #[must_use]
    pub fn power_lost_nanos(&self) -> Option<u64> {
        self.bus
            .periph
            .io
            .power_lost()
            .map(|at| clock::TIME_BASE.cycles_to_nanos(at))
    }

    /// An attached chip changed what it drives from outside the run (a button the user pressed):
    /// the pins are read again at the present device time, and a GPIO interrupt it triggers is
    /// raised. Recording such a change in a session is the caller's (ADR 0012).
    pub fn inputs_changed(&mut self) {
        self.bus.periph.io.settle(&mut self.bus.ctx);
    }

    /// The changes of the USB host still to come, those due by now applied first; during a deep
    /// sleep, those due since its start too, which the reset that ends it applies.
    pub fn usb_host_changes_pending(&mut self) -> usize {
        self.bus
            .periph
            .usb_serial_jtag
            .host_changes_pending(&mut self.bus.ctx)
    }

    /// Watches `channel` or stops watching it. While a channel is watched, each byte it hands to
    /// the host ends the call of [`Esp32C3::run_within`] with [`StopReason::ConsoleOutput`], at
    /// the instruction boundary it went at, or at its very instant while the CPU sleeps. A front
    /// end that waits for a text then knows the device time it came at, whatever its slices.
    /// Watching changes nothing the firmware can see, nor the diagnostics of the run.
    ///
    /// The caller takes a watched channel ([`Esp32C3::take_console`]) after every call, and once
    /// before it starts watching: the stop says that bytes left that no call has taken, not that
    /// bytes left in this call, so bytes left untaken end every following call that brings a
    /// device to now, at that later instant.
    pub fn watch_console(&mut self, channel: ConsoleChannel, watched: bool) {
        self.watched[channel.index()] = watched;
        let (periph, ctx) = (&mut self.bus.periph, &mut self.bus.ctx);
        match channel {
            ConsoleChannel::Uart0 => periph.uart0.watch(ctx, watched),
            // The host takes each packet at an instant the block asked to be called at.
            ConsoleChannel::UsbSerialJtag => {}
            ConsoleChannel::Uart1 => periph.uart1.watch(ctx, watched),
        }
    }

    fn check_date(&self, at: Cycles) -> Result<(), InputError> {
        if self.bus.periph.io.power_lost().is_some() {
            return Err(InputError::PoweredOff);
        }
        let now = self.device_time();
        if at < now {
            return Err(InputError::InThePast {
                at_nanos: clock::TIME_BASE.cycles_to_nanos(at),
                now_nanos: clock::TIME_BASE.cycles_to_nanos(now),
            });
        }
        Ok(())
    }

    /// The programs, erases and status writes the flash chip performed since the last call,
    /// oldest first: the journal of what the run wrote.
    ///
    /// Its callers are a front end that shows or checks what a firmware writes, and the tests
    /// that compare the writes of a reference firmware with the ones measured on a chip
    /// (`tests/reference_boot.rs`). The command line does not take it: it keeps the content of
    /// the flash ([`Esp32C3::flash_contents`]), not the operations.
    ///
    /// The journal holds at most [`JOURNAL_CAPACITY`](crate::flash::JOURNAL_CAPACITY) entries
    /// between two calls, the first ones; the operations performed while it was full are not
    /// in it and are counted by [`Esp32C3::flash_writes_dropped`]. A caller that needs every
    /// one takes them as often as it takes the console, and reads that count when the run
    /// ends: 0 says that the entries it took are all of them. A caller that never takes the
    /// journal costs the machine those entries at most (64 bytes of data each through the
    /// controller) and loses nothing else.
    pub fn take_flash_writes(&mut self) -> Vec<FlashWrite> {
        self.bus.periph.spi1.take_flash_writes()
    }

    /// How many flash operations the journal did not keep since power-on, because nobody had
    /// taken the ones before them: what a caller of [`Esp32C3::take_flash_writes`] reads to
    /// know whether what it took is the whole of what the run wrote.
    #[must_use]
    pub const fn flash_writes_dropped(&self) -> u64 {
        self.bus.periph.spi1.flash_writes_dropped()
    }

    /// How many flash commands of `opcode` the SPI1 controller sent to the chip since power-on
    /// (`0x02` page program, `0x06` write enable, `0x04` write disable, `0x05` read status, and
    /// so on), those the chip model refused included.
    ///
    /// Its callers are the tests that compare the commands a firmware sends here with the
    /// ones it sends to a real chip (`tests/reference_boot.rs`: one write enable per write, no
    /// write disable), and a front end that wants the same numbers. The counters are 256 `u64`
    /// whatever the firmware sends.
    #[must_use]
    pub fn flash_commands_sent(&self, opcode: u8) -> u64 {
        self.bus.periph.spi1.commands_sent(opcode)
    }

    /// The whole content of the flash chip as it is now, erased bytes included: exactly
    /// [`FlashConfig::size`](crate::flash::FlashConfig::size) bytes, at most 16 MiB whatever the
    /// firmware did. It is what a caller keeps to let a later run go on from this one: given
    /// back to [`Esp32C3::with_flash`] with the same configuration, it builds a machine whose
    /// flash holds the same bytes. The machine performs no host I/O; storing the bytes is the
    /// caller's business.
    ///
    /// ```
    /// use soc_esp32c3::{Esp32C3, FlashImageError, RomImage, SocConfig};
    /// /// The machine of the next run: it starts again from the ROM, on the flash `last` left.
    /// fn next_run(
    ///     rom: RomImage,
    ///     last: &Esp32C3,
    ///     config: &SocConfig,
    /// ) -> Result<Esp32C3, FlashImageError> {
    ///     Esp32C3::with_flash(rom, last.flash_contents(), config)
    /// }
    /// ```
    #[must_use]
    pub fn flash_contents(&self) -> Vec<u8> {
        let flash = self.bus.periph.spi1.flash();
        // INVARIANT: a `u32` fits in `usize` on every target the workspace builds for (32 bits
        // and more); the fallback, an empty content, is never taken.
        let mut content = vec![0u8; usize::try_from(flash.size()).unwrap_or(0)];
        flash.read(0, &mut content);
        content
    }

    /// How many 4 KiB sectors of the flash had a byte changed by the run since the machine was
    /// built: 0 says the content is the one the machine was given. A program or an erase that
    /// left every byte as it was counts for nothing, and a reset does not start the count again:
    /// the chip is not reset with the rest. A sector that was changed and changed back counts
    /// (erased, then programmed with the bytes it held): a count above 0 says that the run wrote
    /// other bytes at some point, not that the content differs now.
    #[must_use]
    pub fn flash_modified_sectors(&self) -> u32 {
        self.bus.periph.spi1.flash().modified_sectors()
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

    /// Resets that came once [`MAX_LISTED_RESETS`](crate::diag::MAX_LISTED_RESETS) had been
    /// listed: they are counted but never listed. [`Esp32C3::resets`] counts them all.
    #[must_use]
    pub const fn unlisted_resets(&self) -> u64 {
        self.bus.ctx.diag.unlisted_resets()
    }

    /// The model that prices each instruction, so a front end can say whether timing is calibrated.
    #[must_use]
    pub fn timing_model(&self) -> &dyn TimingModel {
        self.pricing.model()
    }

    /// The 32 integer registers, `x0` first (always 0).
    #[must_use]
    pub const fn x(&self) -> [u32; 32] {
        self.hart.x
    }

    /// Writes integer register `reg` (1 to 31; `x0` and a number of 32 or more change nothing).
    /// For a lockstep that copies into the machine a value the oracle read from a device the two
    /// model differently (decision H3, treatment ii); a debugger's register write.
    pub fn set_x(&mut self, reg: usize, value: u32) {
        if (1..32).contains(&reg) {
            self.hart.x[reg] = value;
        }
    }

    /// Whether Espressif's QEMU, in the state the interrupt controller is in, would show the CPU
    /// its external interrupt (`mip.MEIP`): a bit this machine does not show in `mip`, which the
    /// lockstep compares with the `mip` QEMU prints.
    #[must_use]
    pub fn external_interrupt_as_qemu(&self) -> bool {
        self.bus
            .periph
            .intmatrix
            .asserted_as_qemu(self.bus.ctx.sources())
    }

    /// A CSR as an instruction would read it at this instant, `None` for one the CPU does not
    /// have. Reading it has no effect.
    #[must_use]
    pub fn csr(&self, address: u16) -> Option<u32> {
        self.hart
            .csr
            .read(address, self.hart.cycles, self.hart.instret)
    }

    /// Fingerprint of the executed PC sequence, when [`SocConfig::trace_hash`] is set.
    #[must_use]
    pub fn trace_hash(&self) -> Option<u64> {
        self.trace.as_ref().map(TraceHash::finish)
    }
}

#[cfg(test)]
mod board_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memmap::ROM_BASE;
    use crate::memmap::SRAM_DBUS_BASE;
    use crate::test_support::{
        HANG, T0, T1, T2, T3, TIMER_WAKE_STATE, flash_erase_sector, flash_program, flat_rom,
        load_register, print_program as print, sleep_on_the_rtc_timer, stop_watchdogs,
        store_register, unmodeled_register, within,
    };
    use cpu_rv32::asm::{addi, beq, bne, csrrs, csrrwi, jal, lbu, lui, lw, sw, wfi};
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
    fn what_a_program_writes_to_the_flash_is_in_the_journal_of_the_run_once() {
        // An erase, then the bitmap word NVS programs twice in place (SF §B.2).
        let mut program = flash_erase_sector(0x9000);
        program.extend(flash_program(0x9020, &[0xFE, 0xFF, 0xFF, 0xFF]));
        program.extend(flash_program(0x9020, &[0xFA]));
        program.push(HANG);
        let strict = SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        };
        let mut m = Esp32C3::with_flash(flat_rom(&program), vec![0u8; 0xA000], &strict).unwrap();
        assert_eq!(
            m.run_until(Cycles(400)),
            StopReason::CycleBudget,
            "a flash write is nothing a strict run stops on"
        );
        assert_eq!(
            m.take_flash_writes(),
            [
                FlashWrite::Erase {
                    start: 0x9000,
                    len: 0x1000
                },
                FlashWrite::Program {
                    address: 0x9020,
                    data: vec![0xFE, 0xFF, 0xFF, 0xFF]
                },
                FlashWrite::Program {
                    address: 0x9020,
                    data: vec![0xFA]
                },
            ]
        );
        assert_eq!(m.take_flash_writes(), [], "taken once");
        assert_eq!(m.flash_writes_dropped(), 0);
        assert_eq!(m.drain_diagnostics(), []);
    }

    /// Where two flash contents first differ, their common length when one is longer: a failed
    /// comparison says an offset, not 16 MiB of bytes.
    fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
        left.iter()
            .zip(right)
            .position(|(l, r)| l != r)
            .or_else(|| (left.len() != right.len()).then_some(left.len().min(right.len())))
    }

    #[test]
    fn the_content_of_the_flash_leaves_the_machine_whole_and_builds_the_same_flash_again() {
        // A sector erased, four bytes programmed in it, and a program in another sector that
        // changes no byte (0x00 AND 0x0F is 0x00): one sector was modified, not two.
        let mut program = flash_erase_sector(0x9000);
        program.extend(flash_program(0x9020, &[0xFE, 0xCA, 0xFF, 0x00]));
        program.extend(flash_program(0x2000, &[0x0F]));
        program.push(HANG);
        let config = SocConfig::default();
        let size = config.flash.size() as usize;
        let image = vec![0u8; 0xA000];
        let mut m = Esp32C3::with_flash(flat_rom(&program), image.clone(), &config).unwrap();
        let before = m.flash_contents();
        assert_eq!(before.len(), size, "the whole chip, not the image");
        assert_eq!(before[..image.len()], image[..]);
        assert!(before[image.len()..].iter().all(|&byte| byte == 0xFF));
        assert_eq!(m.flash_modified_sectors(), 0);

        m.run_until(Cycles(400));
        let after = m.flash_contents();
        let mut expected = before.clone();
        expected[0x9000..0xA000].fill(0xFF);
        expected[0x9020..0x9024].copy_from_slice(&[0xFE, 0xCA, 0xFF, 0x00]);
        assert_eq!(
            first_difference(&after, &expected),
            None,
            "the erase and the program, nothing else"
        );
        assert_eq!(m.flash_modified_sectors(), 1);
        m.reset(ResetCause::Mwdt0Core);
        assert_eq!(m.flash_modified_sectors(), 1, "the chip is not reset");
        assert_eq!(first_difference(&m.flash_contents(), &expected), None);

        // What left the machine is a content `with_flash` takes: the next machine starts from it.
        let next = Esp32C3::with_flash(flat_rom(&[HANG]), after, &config).unwrap();
        assert_eq!(first_difference(&next.flash_contents(), &expected), None);
        assert_eq!(
            next.flash_modified_sectors(),
            0,
            "a new run, nothing modified"
        );
    }

    #[test]
    fn a_machine_with_an_erased_flash_hands_over_a_chip_of_erased_bytes() {
        let small = SocConfig {
            flash: crate::flash::FlashConfig::new(
                1 << 20,
                crate::flash::JedecId::from_u24(0x9D_6014).unwrap(),
            )
            .unwrap(),
            ..SocConfig::default()
        };
        let m = Esp32C3::new(flat_rom(&[HANG]), &small);
        let content = m.flash_contents();
        assert_eq!(content.len(), 1 << 20, "the size of the configuration");
        assert!(content.iter().all(|&byte| byte == 0xFF));
        assert_eq!(m.flash_modified_sectors(), 0);
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
        // The host takes the packet 50 us after `WR_DONE`: 1 000 cycles at the 20 MHz of
        // power-on.
        m.run_until(Cycles(500));
        assert_eq!(m.take_console(ConsoleChannel::UsbSerialJtag), b"");
        m.run_until(Cycles(1_500));
        assert_eq!(m.take_console(ConsoleChannel::UsbSerialJtag), b"ok");
        assert_eq!(
            m.take_console(ConsoleChannel::Uart0),
            Vec::<u8>::new(),
            "nothing went to UART0"
        );
    }

    #[test]
    fn a_caller_that_drains_every_channel_leaves_no_output_held_by_a_device() {
        // UART1 with a divider of 0: a byte leaves as soon as it is written, one per turn of the
        // loop, for as long as the firmware runs.
        let mut program = store_register(svd_regs::esp32c3::addr("UART1", "CLKDIV"), 0).to_vec();
        program.extend(store_register(
            svd_regs::esp32c3::addr("UART1", "FIFO"),
            u32::from(b'u'),
        ));
        program.push(jal(0, -16));
        let mut m = machine(&program);
        let mut taken = 0;
        for slice in 1..=4u64 {
            assert_eq!(
                m.run_until(Cycles(slice * 100_000)),
                StopReason::CycleBudget
            );
            for channel in ConsoleChannel::ALL {
                let bytes = m.take_console(channel);
                if channel == ConsoleChannel::Uart1 {
                    assert!(bytes.iter().all(|byte| *byte == b'u'));
                    taken += bytes.len();
                } else {
                    assert_eq!(bytes, b"", "{channel:?}");
                }
            }
            assert_eq!(
                m.bus.periph.uart1.waiting_for_the_console(),
                0,
                "after slice {slice}, every channel drained"
            );
        }
        assert!(taken > 50_000, "one byte per turn of the loop: {taken}");
        // The end of a run takes what is still on its way out, on this channel as on UART0.
        let mut slow = print(svd_regs::esp32c3::addr("UART1", "FIFO"), b"late");
        slow.push(HANG);
        let mut m = machine(&slow);
        assert_eq!(m.run_until(Cycles(500)), StopReason::CycleBudget);
        assert_eq!(m.take_console(ConsoleChannel::Uart1), b"");
        assert_eq!(m.flush_console(ConsoleChannel::Uart1), b"late");
        assert_eq!(m.flush_console(ConsoleChannel::Uart1), b"");
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

    /// SPI1 `+0x54`, which the SVD omits and ESP-IDF v4.4 polls before each status read of the
    /// flash, is a register of the model: a strict run reads it idle and goes on.
    #[test]
    fn spi1_fsm_is_read_idle_without_a_diagnostic_under_strict_mmio() {
        // Written out: the SVD has no name for it, which is what the test is about.
        const SPI1_FSM: u32 = 0x6000_2054;
        assert!(
            svd_regs::esp32c3::lookup(SPI1_FSM).is_none(),
            "the SVD omits it"
        );
        let config = SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        };
        let [page, load] = load_register(SPI1_FSM);
        let mut m = machine_with(&[page, load, HANG], config);
        assert_eq!(m.run_until(Cycles(200)), StopReason::CycleBudget);
        assert_eq!(m.drain_diagnostics(), []);
        assert_eq!(bus_word(&mut m, SPI1_FSM), 0x200);
    }

    /// A program that echoes on UART0 every byte it receives: it polls `STATUS.RXFIFO_CNT` (its
    /// low byte), reads `FIFO`, writes the byte back.
    fn echo_uart0() -> Vec<u32> {
        vec![
            lui(T0, UART0),
            lbu(T1, T0, 0x1C),
            beq(T1, 0, -4),
            lw(T1, T0, 0),
            sw(T1, T0, 0),
            jal(0, -16),
        ]
    }

    /// A byte at the reset settings of UART0: `CLKDIV` 694 on the crystal divided by 2, 10 bit
    /// times: 55 520 cycles of the time base, 347 us.
    const RESET_BYTE: u64 = 55_520;

    #[test]
    fn an_input_dated_before_the_device_time_is_refused_and_changes_nothing() {
        let mut m = machine(&echo_uart0());
        m.run_until(Cycles(100));
        let now = m.device_time();
        assert_eq!(
            m.send_console(ConsoleChannel::Uart0, Cycles(now.get() - 1), b"x"),
            Err(InputError::InThePast {
                at_nanos: clock::TIME_BASE.cycles_to_nanos(Cycles(now.get() - 1)),
                now_nanos: m.device_time_nanos(),
            })
        );
        assert_eq!(m.input_pending(ConsoleChannel::Uart0), 0);
        assert_eq!(
            m.set_usb_host(Cycles::ZERO, UsbHost::Absent),
            Err(InputError::InThePast {
                at_nanos: 0,
                now_nanos: m.device_time_nanos(),
            })
        );
        assert_eq!(m.usb_host(), UsbHost::Open);
        assert_eq!(m.send_console(ConsoleChannel::Uart0, now, b"x"), Ok(()));
        assert_eq!(m.input_pending(ConsoleChannel::Uart0), 1);
        let message = InputError::InThePast {
            at_nanos: 5,
            now_nanos: 10,
        }
        .to_string();
        assert!(
            message.ends_with("; date it at 10 ns or later"),
            "{message}"
        );
    }

    #[test]
    fn an_input_dated_after_the_last_instant_a_uart_times_is_refused_and_changes_nothing() {
        // A UART times its line in thirty-seconds of a cycle: a later date would saturate, and
        // the byte would start before it.
        use crate::periph::uart::LAST_INPUT_DATE;
        let mut m = machine(&echo_uart0());
        let last_nanos = clock::TIME_BASE.cycles_to_nanos(LAST_INPUT_DATE);
        for channel in [ConsoleChannel::Uart0, ConsoleChannel::Uart1] {
            let too_late = Cycles(LAST_INPUT_DATE.get() + 1);
            assert_eq!(
                m.send_console(channel, too_late, b"x"),
                Err(InputError::BeyondTheLine {
                    channel,
                    at_nanos: clock::TIME_BASE.cycles_to_nanos(too_late),
                    last_nanos,
                })
            );
            assert_eq!(m.input_pending(channel), 0);
            assert_eq!(m.send_console(channel, LAST_INPUT_DATE, b"x"), Ok(()));
            assert_eq!(m.input_pending(channel), 1);
        }
        assert_eq!(
            m.send_console(ConsoleChannel::UsbSerialJtag, Cycles(u64::MAX), b"x"),
            Ok(()),
            "USB-Serial-JTAG dates in whole cycles"
        );
        let message = InputError::BeyondTheLine {
            channel: ConsoleChannel::Uart1,
            at_nanos: 5,
            last_nanos: 4,
        }
        .to_string();
        assert!(
            message.starts_with("uart1 cannot take an input dated 5 ns")
                && message.ends_with("; date it at 4 ns or earlier"),
            "{message}"
        );
    }

    #[test]
    fn a_channel_holds_a_megabyte_of_the_host_at_most() {
        let mut m = machine(&[HANG]);
        let megabyte = vec![b'.'; MAX_PENDING_HOST_BYTES];
        assert_eq!(
            m.send_console(ConsoleChannel::Uart1, Cycles::ZERO, &megabyte),
            Ok(())
        );
        assert_eq!(
            m.send_console(ConsoleChannel::Uart1, Cycles::ZERO, b"!"),
            Err(InputError::TooMuchPending {
                channel: ConsoleChannel::Uart1,
                pending: MAX_PENDING_HOST_BYTES + 1,
            })
        );
        assert_eq!(
            m.send_console(ConsoleChannel::Uart1, Cycles::ZERO, b""),
            Ok(()),
            "no byte is nothing on a UART: it adds nothing to hold"
        );
        assert_eq!(
            m.input_pending(ConsoleChannel::Uart1),
            MAX_PENDING_HOST_BYTES
        );
        assert_eq!(
            m.send_console(ConsoleChannel::UsbSerialJtag, Cycles::ZERO, &megabyte),
            Ok(())
        );
        assert_eq!(
            m.send_console(ConsoleChannel::UsbSerialJtag, Cycles::ZERO, b""),
            Err(InputError::TooMuchPending {
                channel: ConsoleChannel::UsbSerialJtag,
                pending: MAX_PENDING_HOST_BYTES + 1,
            }),
            "a zero-length packet counts"
        );
        for _ in 0..MAX_PENDING_HOST_CHANGES {
            assert_eq!(m.set_usb_host(Cycles(1_000), UsbHost::Open), Ok(()));
        }
        assert_eq!(
            m.set_usb_host(Cycles(1_000), UsbHost::Open),
            Err(InputError::TooManyHostChanges)
        );
        assert!(
            InputError::TooManyHostChanges
                .to_string()
                .contains("; give the next ones")
        );
    }

    #[test]
    fn the_input_pending_on_every_channel_is_what_has_not_reached_the_device() {
        // The firmware reads nothing: what reached a receive FIFO stays there, and is not
        // counted, on a UART as on USB-Serial-JTAG.
        let mut m = machine(&[HANG]);
        for channel in ConsoleChannel::ALL {
            m.send_console(channel, Cycles::ZERO, b"abc").unwrap();
            assert_eq!(m.input_pending(channel), 3, "{channel:?}");
        }
        // 100 us: the packet reached the OUT FIFO 50 us after it was sent, and `a` is on the
        // line of each UART.
        m.run_within(Cycles(u64::MAX), Some(100_000));
        assert_eq!(m.input_pending(ConsoleChannel::UsbSerialJtag), 0);
        let after_bytes =
            |bytes: u64| clock::TIME_BASE.cycles_to_nanos(Cycles(bytes * RESET_BYTE)) + 1_000;
        for (bytes, pending) in [(0, 3), (2, 1), (4, 0)] {
            if bytes > 0 {
                m.run_within(Cycles(u64::MAX), Some(after_bytes(bytes)));
            }
            for channel in [ConsoleChannel::Uart0, ConsoleChannel::Uart1] {
                assert_eq!(
                    m.input_pending(channel),
                    pending,
                    "{channel:?} after {bytes} byte times"
                );
            }
        }
        assert_eq!(m.input_pending(ConsoleChannel::UsbSerialJtag), 0);
    }

    #[test]
    fn a_firmware_receives_the_bytes_of_the_host_at_their_dates() {
        let mut m = machine(&echo_uart0());
        let sent_at = Cycles(1_000_000);
        m.send_console(ConsoleChannel::Uart0, sent_at, b"hi")
            .unwrap();
        let landed_ns = clock::TIME_BASE.cycles_to_nanos(sent_at + RESET_BYTE);
        m.run_within(Cycles(u64::MAX), Some(landed_ns - 1_000));
        assert_eq!(
            m.input_pending(ConsoleChannel::Uart0),
            2,
            "h is on the line"
        );
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"");
        m.run_within(Cycles(u64::MAX), Some(landed_ns + 3 * 347_000));
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"hi");
        assert_eq!(m.input_pending(ConsoleChannel::Uart0), 0);
        assert_eq!(m.drain_diagnostics(), Vec::new());
    }

    #[test]
    fn a_watched_channel_ends_the_call_at_the_instant_each_byte_reaches_the_host() {
        // The CPU sleeps after the writes: each byte is seen at its very instant. A byte that is
        // not the last written when the one before it leaves (`b`) is seen at its instant only
        // because the channel is watched; UART1 as UART0.
        for (fifo, channel) in [
            (UART0, ConsoleChannel::Uart0),
            (
                svd_regs::esp32c3::addr("UART1", "FIFO"),
                ConsoleChannel::Uart1,
            ),
        ] {
            let mut program = print(fifo, b"abc");
            program.extend([wfi(), HANG]);
            let mut m = machine(&program);
            m.watch_console(channel, true);
            assert_eq!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
            assert_eq!(m.take_console(channel), b"a", "{channel:?}");
            let first = m.device_time();
            for (byte, after) in [(b"b", 1), (b"c", 2)] {
                assert_eq!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
                assert_eq!(m.take_console(channel), byte, "{channel:?}");
                assert_eq!(
                    m.device_time(),
                    first + after * RESET_BYTE,
                    "{channel:?}: back to back, to the cycle"
                );
            }
            m.watch_console(channel, false);
            assert_ne!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
        }
    }

    #[test]
    fn a_watched_byte_ends_the_call_at_the_first_instruction_boundary_after_it_leaves() {
        // While the CPU executes: the boundary is the one a caller taking the console after
        // every instruction first finds the byte at.
        let mut program = print(UART0, b"a");
        program.push(HANG);
        let mut stepped = machine(&program);
        let seen_at = loop {
            stepped.run_until(stepped.cycles() + 1);
            if stepped.take_console(ConsoleChannel::Uart0) == b"a" {
                break stepped.device_time();
            }
        };
        let mut watched = machine(&program);
        watched.watch_console(ConsoleChannel::Uart0, true);
        assert_eq!(
            watched.run_until(Cycles(1_000_000)),
            StopReason::ConsoleOutput
        );
        assert_eq!(watched.take_console(ConsoleChannel::Uart0), b"a");
        assert_eq!(watched.device_time(), seen_at);
    }

    /// Runs `program` one CPU cycle a call, taking `channel` after each, and returns the device
    /// time `text` is first found at: the boundary a watching machine must stop at.
    fn first_seen_stepping(program: &[u32], channel: ConsoleChannel, text: &[u8]) -> Cycles {
        let mut stepped = machine(program);
        let mut seen = Vec::new();
        while stepped.cycles() < Cycles(1_000_000) {
            stepped.run_until(stepped.cycles() + 1);
            seen.extend(stepped.take_console(channel));
            if seen == text {
                return stepped.device_time();
            }
            assert!(text.starts_with(&seen), "{seen:?}");
        }
        panic!("{text:?} never left");
    }

    /// `program`, with `channel` watched and no other, stops with
    /// [`StopReason::ConsoleOutput`] at the instant a stepped run first sees `text`, which is all
    /// it takes, on that channel only.
    fn assert_watched_stop(program: &[u32], channel: ConsoleChannel, text: &[u8]) {
        let seen_at = first_seen_stepping(program, channel, text);
        let mut watched = machine(program);
        watched.watch_console(channel, true);
        assert_eq!(
            watched.run_until(Cycles(1_000_000)),
            StopReason::ConsoleOutput,
            "{channel:?}"
        );
        assert_eq!(watched.device_time(), seen_at, "{channel:?}");
        assert_eq!(watched.take_console(channel), text, "{channel:?}");
        for other in ConsoleChannel::ALL.into_iter().filter(|&c| c != channel) {
            assert_eq!(watched.take_console(other), b"", "{other:?}");
        }
    }

    #[test]
    fn a_watched_usb_packet_ends_the_call_at_the_instant_the_host_takes_it() {
        // `WR_DONE` commits `ok`; the host takes it 50 us later, both bytes at one instant.
        let mut program = print(USB, b"ok");
        program.extend([addi(T1, 0, 1), sw(T1, T0, 4), HANG]);
        assert_watched_stop(&program, ConsoleChannel::UsbSerialJtag, b"ok");
    }

    #[test]
    fn watching_a_uart_timed_on_a_clock_the_model_lacks_changes_none_of_its_diagnostics() {
        // `SCLK_SEL` = 0 names no documented clock: each byte sent and each byte read is refused
        // once, at the write or the read, however often the watched UART is brought to now.
        let clk_conf = svd_regs::esp32c3::addr("UART0", "CLK_CONF");
        let mut program = store_register(clk_conf, 0x0340_0000).to_vec();
        program.extend(print(UART0, b"ab"));
        program.extend(echo_uart0());
        let diagnostics = |watched: bool| {
            let mut m = machine(&program);
            m.watch_console(ConsoleChannel::Uart0, watched);
            m.send_console(ConsoleChannel::Uart0, Cycles(200_000), b"xy")
                .unwrap();
            let mut console = Vec::new();
            while m.run_within(m.cycles() + 997, Some(10_000_000)) != StopReason::TimeBudget {
                console.extend(m.take_console(ConsoleChannel::Uart0));
            }
            console.extend(m.take_console(ConsoleChannel::Uart0));
            let repeated = m.most_repeated_diagnostics(16);
            (console, repeated, m.drain_diagnostics())
        };
        let unwatched = diagnostics(false);
        assert_eq!(unwatched.0, b"abxy");
        assert!(!unwatched.1.is_empty(), "{unwatched:?}");
        assert_eq!(diagnostics(true), unwatched);
    }

    /// Everything a run shows of itself: its consoles, its trace hash, its device time, its
    /// cycles and instructions.
    type EchoRun = (Vec<u8>, Vec<u8>, Option<u64>, Cycles, Cycles, u64);

    /// The echo firmware with three inputs and a change of the USB host, run to 40 ms of device
    /// time in calls of `slice` cycles, watching UART0 or not.
    fn echo_run(slice: u64, watched: bool) -> EchoRun {
        let mut m = machine_with(
            &echo_uart0(),
            SocConfig {
                trace_hash: true,
                ..SocConfig::default()
            },
        );
        m.watch_console(ConsoleChannel::Uart0, watched);
        m.send_console(ConsoleChannel::Uart0, Cycles(800_000), b"ab")
            .unwrap();
        m.send_console(ConsoleChannel::Uart0, Cycles(800_001), b"c")
            .unwrap();
        m.send_console(ConsoleChannel::Uart0, Cycles(3_000_000), b"\r\n")
            .unwrap();
        m.set_usb_host(Cycles(2_000_000), UsbHost::Absent).unwrap();
        let (mut uart0, mut usb) = (Vec::new(), Vec::new());
        let limit_ns = 40_000_000;
        loop {
            let stop = m.run_within(m.cycles() + slice, Some(limit_ns));
            uart0.extend(m.take_console(ConsoleChannel::Uart0));
            usb.extend(m.take_console(ConsoleChannel::UsbSerialJtag));
            match stop {
                StopReason::TimeBudget => break,
                StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => {}
                other => panic!("{other:?}"),
            }
        }
        let trace = m.trace_hash();
        (
            uart0,
            usb,
            trace,
            m.device_time(),
            m.cycles(),
            m.instructions(),
        )
    }

    #[test]
    fn a_run_with_inputs_is_the_same_whatever_its_calls_and_whether_it_is_watched() {
        // ADR 0006: the device time of an input is fixed by the inputs alone.
        let whole = echo_run(u64::MAX, false);
        assert_eq!(whole.0, b"abc\r\n");
        assert!(whole.2.is_some());
        for (slice, watched) in [(1, false), (997, false), (1, true), (1_000_000, true)] {
            assert_eq!(
                echo_run(slice, watched),
                whole,
                "slice {slice}, watched {watched}"
            );
        }
    }

    /// UART0 counts `APB_CLK` (`SCLK_SEL` = 1, the source divider at 1): at the reset `CLKDIV`
    /// and 20 MHz a byte takes 55 520 cycles. The host sends `a` at 2 000; about 400 loops later,
    /// with the byte on the line, the firmware moves the CPU and APB to the undivided crystal,
    /// 40 MHz, then echoes what it receives. Run to 2 ms in calls of `slice` cycles.
    fn apb_echo_run(slice: u64, watched: bool) -> EchoRun {
        const T2: u8 = 7;
        let clk_conf = svd_regs::esp32c3::addr("UART0", "CLK_CONF");
        let mut program = store_register(clk_conf, 0x0350_0000).to_vec();
        program.extend([addi(T2, 0, 400), addi(T2, T2, -1), bne(T2, 0, -4)]);
        program.extend(store(SYSCLK_CONF, 0));
        program.extend(echo_uart0());
        let mut m = machine_with(
            &program,
            SocConfig {
                trace_hash: true,
                ..SocConfig::default()
            },
        );
        m.watch_console(ConsoleChannel::Uart0, watched);
        m.send_console(ConsoleChannel::Uart0, Cycles(2_000), b"a")
            .unwrap();
        let mut uart0 = Vec::new();
        loop {
            let stop = m.run_within(m.cycles() + slice, Some(2_000_000));
            uart0.extend(m.take_console(ConsoleChannel::Uart0));
            match stop {
                StopReason::TimeBudget => break,
                StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => {}
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(m.cpu_frequency().hz(), 40_000_000);
        let trace = m.trace_hash();
        (
            uart0,
            Vec::new(),
            trace,
            m.device_time(),
            m.cycles(),
            m.instructions(),
        )
    }

    #[test]
    fn a_byte_on_the_apb_clock_lands_the_same_whatever_the_calls_when_the_clock_changes_under_it() {
        // The byte is timed with the clock of its start bit, whether a call took the console
        // before the change or not (ADR 0006).
        let whole = apb_echo_run(u64::MAX, false);
        assert_eq!(whole.0, b"a");
        for (slice, watched) in [(1, false), (997, false), (7_919, true)] {
            assert_eq!(
                apb_echo_run(slice, watched),
                whole,
                "slice {slice}, watched {watched}"
            );
        }
    }

    fn rtc_cntl(name: &str) -> u32 {
        svd_regs::esp32c3::addr("RTC_CNTL", name)
    }

    /// A firmware of four lives: each counts itself in `STORE0` (in the RTC domain), prints the
    /// count on UART0 and waits for the byte to leave; lives 1 to 3 sleep 0x2000 slow-clock counts
    /// (60 ms) on the RTC timer, life 4 sleeps with no wake source.
    fn four_lives() -> Vec<u32> {
        four_lives_waiting(true)
    }

    /// [`four_lives`], each life sleeping with its byte still on the line when `flushed` is
    /// false: the byte then leaves at the wake (the assumption on `RtcCntl::enter_sleep`), and
    /// the byte of life 4 never leaves.
    fn four_lives_waiting(flushed: bool) -> Vec<u32> {
        // 8 192 turns are more than the 347 us a byte takes at the reset clock; one is not.
        let wait = if flushed {
            lui(T1, 0x2000)
        } else {
            addi(T1, 0, 1)
        };
        let store0 = rtc_cntl("STORE0");
        let mut program = stop_watchdogs();
        program.extend(load_register(store0));
        program.extend([
            addi(T2, T1, 1),
            sw(T2, T0, (store0 - rtc_cntl("OPTIONS0")).cast_signed()),
            lui(T3, UART0),
            addi(T1, T2, i32::from(b'0')),
            sw(T1, T3, 0),
            // Flushed, the byte has left before the chip sleeps, as `flush_uarts` sees to.
            wait,
            addi(T1, T1, -1),
            bne(T1, 0, -4),
        ]);
        // Life 4 skips over the timer to a sleep without a source.
        let timed = sleep_on_the_rtc_timer(0x2000, TIMER_WAKE_STATE);
        let skip = i32::try_from(4 * (timed.len() + 1)).unwrap();
        program.extend([addi(T1, 0, 4), beq(T2, T1, skip)]);
        program.extend(timed);
        program.extend(store_register(rtc_cntl("WAKEUP_STATE"), 0));
        program.extend(store_register(rtc_cntl("DIG_PWC"), 1 << 31));
        program.extend(store_register(rtc_cntl("STATE0"), 1 << 31));
        program.push(HANG);
        program
    }

    /// The console of UART0 as a run took it: with the channel watched, each take with the
    /// device time it came at; without, every byte in one entry dated 0.
    type DatedConsole = Vec<(Cycles, Vec<u8>)>;
    type DeepSleepRun = (DatedConsole, Option<u64>, Cycles, Cycles, u64);

    /// How a run of [`four_lives_waiting`] is cut into calls: each call runs at most `cycles`
    /// cycles, and to device time `step` nanoseconds after the end of the last call cut by time.
    #[derive(Clone, Copy, Debug)]
    struct Cuts {
        cycles: u64,
        step: u64,
        watched: bool,
    }

    /// The run in one call, to the first second of device time: no cut falls in a sleep.
    const WHOLE: Cuts = Cuts {
        cycles: u64::MAX,
        step: 1_000_000_000,
        watched: false,
    };

    /// [`four_lives_waiting`] cut as `cuts` says, to its sleep without end.
    fn deep_sleep_run(flushed: bool, cuts: Cuts) -> DeepSleepRun {
        let config = SocConfig {
            trace_hash: true,
            ..SocConfig::default()
        };
        let mut m = machine_with(&four_lives_waiting(flushed), config);
        m.watch_console(ConsoleChannel::Uart0, cuts.watched);
        let mut uart0: DatedConsole = Vec::new();
        let mut limit = cuts.step;
        loop {
            let stop = m.run_within(m.cycles() + cuts.cycles, Some(limit));
            let bytes = m.take_console(ConsoleChannel::Uart0);
            if !bytes.is_empty() {
                let at = if cuts.watched {
                    m.device_time()
                } else {
                    Cycles(0)
                };
                match uart0.last_mut() {
                    Some((last, taken)) if *last == at => taken.extend(bytes),
                    _ => uart0.push((at, bytes)),
                }
            }
            match stop {
                StopReason::DeepSleep => break,
                StopReason::TimeBudget => limit += cuts.step,
                StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => {}
                other => panic!("{cuts:?}: {other:?}"),
            }
        }
        (
            uart0,
            m.trace_hash(),
            m.device_time(),
            m.cycles(),
            m.resets(),
        )
    }

    #[test]
    fn a_deep_sleep_run_is_the_same_whatever_its_calls() {
        // F10: the instant of each wake is that of the RTC timer reaching its target, whatever
        // the calls the run is cut into.
        let whole = deep_sleep_run(true, WHOLE);
        assert_eq!(whole.0, [(Cycles(0), b"1234".to_vec())]);
        assert_eq!(whole.4, 3, "three wakes");
        assert!(whole.1.is_some());
        // Three sleeps of 0x2000 counts, 60.2 ms each, and a few instructions per life.
        let three_sleeps = Cycles(3 * 8_192 * 20_000 / 17);
        assert!(whole.2 > three_sleeps, "{:?}", whole.2);
        assert!(whole.2.get() < three_sleeps.get() + clock::SECOND / 100);
        for cycles in [1, 7, 997, 7_919] {
            let cuts = Cuts { cycles, ..WHOLE };
            assert_eq!(deep_sleep_run(true, cuts), whole, "{cuts:?}");
        }
    }

    /// Device-time steps that put limits inside the sleeps: whole milliseconds, a prime number of
    /// nanoseconds, and about one period of the slow clock (7 352.9 ns at 136 kHz), which is not
    /// a whole number of cycles of the time base.
    const TIME_STEPS: [u64; 4] = [1_000_000, 7_000_000, 1_000_003, 7_351];

    #[test]
    fn a_deep_sleep_cut_by_device_time_wakes_at_the_instant_of_an_uncut_run() {
        // F10 with the limits inside the sleeps: a call that ends in a sleep moves device time to
        // its limit, and the next call finds the same wake instant from there.
        let whole = deep_sleep_run(true, WHOLE);
        for step in TIME_STEPS {
            let cuts = Cuts { step, ..WHOLE };
            assert_eq!(deep_sleep_run(true, cuts), whole, "{cuts:?}");
        }
        let mixed = Cuts {
            cycles: 997,
            step: 1_000_003,
            watched: false,
        };
        assert_eq!(deep_sleep_run(true, mixed), whole, "{mixed:?}");
    }

    #[test]
    fn a_byte_on_the_line_at_a_deep_sleep_leaves_at_the_wake_wherever_the_run_is_cut() {
        // Each life sleeps with its byte still on its way out: the UART has no power, so the byte
        // leaves at the reset of the wake (the assumption on `RtcCntl::enter_sleep`), and the
        // calls of a front end during the sleep (take the console, here) do not move it earlier.
        let watched = Cuts {
            watched: true,
            ..WHOLE
        };
        let whole = deep_sleep_run(false, watched);
        let console: Vec<u8> = whole.0.iter().flat_map(|(_, b)| b.clone()).collect();
        assert_eq!(console, b"123", "the byte of life 4 never leaves");
        assert_eq!(whole.0.len(), 3, "one stop per wake: {:?}", whole.0);
        assert_eq!(whole.4, 3);
        for step in TIME_STEPS {
            let cuts = Cuts { step, ..watched };
            assert_eq!(deep_sleep_run(false, cuts), whole, "{cuts:?}");
        }
        let mixed = Cuts {
            cycles: 7_919,
            step: 7_000_000,
            watched: true,
        };
        assert_eq!(deep_sleep_run(false, mixed), whole, "{mixed:?}");
    }

    #[test]
    fn each_wake_is_a_core_reset_of_cause_5_that_keeps_the_rtc_domain() {
        let mut m = machine(&four_lives());
        while m.run_within(Cycles(u64::MAX), Some(1_000_000_000)) != StopReason::DeepSleep {}
        assert_eq!(m.resets(), 3);
        let diagnostics = m.drain_diagnostics();
        // One entry per wake (spec §7), each at its own device time.
        let wakes: Vec<u64> = diagnostics
            .iter()
            .filter_map(|d| match d {
                Diagnostic::Reset {
                    cause: ResetCause::DeepSleep,
                    at_us,
                    ..
                } => Some(*at_us),
                Diagnostic::Reset { .. } => panic!("not a wake: {d}"),
                _ => None,
            })
            .collect();
        assert_eq!(wakes.len(), 3, "{diagnostics:?}");
        assert!(wakes.is_sorted() && wakes[0] < wakes[2], "{wakes:?}");
        assert!(
            wakes
                .iter()
                .all(|at_us| *at_us <= m.device_time_nanos() / 1_000),
            "{wakes:?}"
        );
        assert_eq!(
            bus_word(&mut m, rtc_cntl("STORE0")),
            4,
            "the count survived"
        );
        assert_eq!(bus_word(&mut m, rtc_cntl("RESET_STATE")) & 0x3F, 5);
        assert_eq!(bus_word(&mut m, rtc_cntl("SLP_WAKEUP_CAUSE")), 1 << 3);
    }

    #[test]
    fn a_deep_sleep_nothing_can_end_stops_the_run_and_says_so() {
        let mut m = machine(&four_lives());
        let mut stop = StopReason::CycleBudget;
        for _ in 0..10 {
            stop = m.run_within(Cycles(u64::MAX), Some(1_000_000_000));
            if stop == StopReason::DeepSleep {
                break;
            }
        }
        assert_eq!(stop, StopReason::DeepSleep);
        let state = m.deep_sleep().unwrap();
        assert_eq!((state.timer_wake_nanos, state.wake_sources), (None, 0));
        let now = m.device_time_nanos();
        assert_eq!(
            m.run_within(Cycles(u64::MAX), Some(1_000_000_000)),
            StopReason::DeepSleep,
            "again, without moving"
        );
        assert_eq!(
            m.device_time_nanos(),
            now,
            "device time is not moved to the limit"
        );
        assert!(state.since_nanos <= now);
    }

    #[test]
    fn a_deep_sleep_cut_by_the_time_limit_is_still_asleep_and_says_when_it_wakes() {
        let mut m = machine(&four_lives());
        assert_eq!(
            m.run_within(Cycles(u64::MAX), Some(30_000_000)),
            StopReason::TimeBudget
        );
        assert_eq!(m.device_time_nanos(), 30_000_000);
        let state = m.deep_sleep().unwrap();
        assert_eq!(state.wake_sources, DeepSleep::TIMER);
        let wake = state.timer_wake_nanos.unwrap();
        // 0x2000 counts (60.2 ms) after the capture, which life 1 takes after its 1.6 ms loop.
        assert!((61_000_000..63_000_000).contains(&wake), "{wake}");
        assert!(!m.is_asleep(), "no `wfi`: the CPU is off, not waiting");
        assert_eq!(m.resets(), 0);
        // The next call wakes it at that instant; life 2 prints and sleeps within 2 ms.
        let _ = m.run_within(Cycles(u64::MAX), Some(wake + 2_000_000));
        assert_eq!(m.resets(), 1);
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"12");
    }

    #[test]
    fn a_running_main_watchdog_does_not_reset_a_chip_in_deep_sleep() {
        // The flash-boot protection of main watchdog 0 runs from every reset and is left on: the
        // digital domain has no power in deep sleep, so the only resets are the wakes. 272 000
        // counts are 2 s: 0x42000 + 0x680.
        let mut program = store_register(rtc_cntl("WDTCONFIG0"), 0).to_vec();
        let mut timed = sleep_on_the_rtc_timer(0x42000, TIMER_WAKE_STATE);
        // `lui` then `add`: the 0x680 goes in as one more `addi` after the `add`.
        timed.insert(8, addi(T1, T1, 0x680));
        program.extend(timed);
        let mut m = machine(&program);
        let mut stop = StopReason::CycleBudget;
        while m.device_time_nanos() < 5_000_000_000 && stop != StopReason::TimeBudget {
            stop = m.run_within(Cycles(u64::MAX), Some(5_000_000_000));
        }
        assert_eq!(m.resets(), 2, "woken at 2 s and 4 s");
        assert!(
            m.drain_diagnostics().iter().all(|d| !matches!(
                d,
                Diagnostic::Reset { cause, .. } if *cause != ResetCause::DeepSleep
            )),
            "no watchdog reset"
        );
    }

    #[test]
    fn a_cpu_reset_that_ends_a_deep_sleep_rebuilds_the_digital_domain() {
        // Life 1 writes UART0's `CLKDIV`, leaves the RTC watchdog running in sleep with stage 0
        // set to reset the CPU (action 2, 1 s), and sleeps with no wake source; life 2 hangs.
        let clkdiv = svd_regs::esp32c3::addr("UART0", "CLKDIV");
        let clkdiv_reset = svd_regs::esp32c3::lookup(clkdiv).unwrap().reset_value;
        assert_ne!(clkdiv_reset, 0x123);
        let store0 = rtc_cntl("STORE0");
        let mut life_1 = store_register(store0, 1).to_vec();
        life_1.extend(store_register(clkdiv, 0x123));
        life_1.extend(store_register(rtc_cntl("WDTCONFIG1"), 68_000));
        life_1.extend(store_register(rtc_cntl("WDTCONFIG0"), 1 << 31 | 2 << 28));
        life_1.extend(store_register(rtc_cntl("WAKEUP_STATE"), 0));
        life_1.extend(store_register(rtc_cntl("DIG_PWC"), 1 << 31));
        life_1.extend(store_register(rtc_cntl("STATE0"), 1 << 31));
        let mut program = stop_watchdogs();
        program.extend(load_register(store0));
        program.push(bne(T1, 0, i32::try_from(4 * (life_1.len() + 1)).unwrap()));
        program.extend(life_1);
        program.push(HANG);
        let mut m = machine(&program);
        assert_eq!(
            m.run_within(Cycles(u64::MAX), Some(1_500_000_000)),
            StopReason::TimeBudget
        );
        assert_eq!(m.resets(), 1);
        assert_eq!(m.deep_sleep(), None, "the reset ended the sleep");
        assert!(
            m.drain_diagnostics().iter().any(|d| matches!(
                d,
                Diagnostic::Reset {
                    cause: ResetCause::RtcWatchdogCpu,
                    ..
                }
            )),
            "a CPU reset"
        );
        assert_eq!(
            bus_word(&mut m, rtc_cntl("RESET_STATE")) & 0x3F,
            ResetCause::RtcWatchdogCpu.code(),
            "the cause keeps its code"
        );
        assert_eq!(
            bus_word(&mut m, clkdiv),
            clkdiv_reset,
            "the UART had no power: it starts from its reset value"
        );
        assert_eq!(bus_word(&mut m, store0), 1, "the RTC domain is kept");
    }

    #[test]
    fn light_sleep_stops_a_strict_run() {
        // F4: `SLEEP_EN` with the digital domain kept powered (the reset value of DIG_PWC).
        let mut program = store_register(rtc_cntl("STATE0"), 1 << 31).to_vec();
        program.push(HANG);
        let config = SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        };
        let mut m = machine_with(&program, config);
        assert!(matches!(
            m.run_until(Cycles(10_000)),
            StopReason::StrictMmio(Diagnostic::Unsupported {
                what: Unsupported::LightSleep { .. },
                ..
            })
        ));
        assert_eq!(m.deep_sleep(), None);
        assert_eq!(
            bus_word(&mut m, rtc_cntl("INT_RAW_RTC")),
            1 << 1,
            "rejected"
        );
    }

    #[test]
    fn the_rtc_memory_crc_unit_answers_at_once_over_the_words_asked() {
        // F3, through the bus: the unit reads the RTC fast memory, which only the bus holds.
        let config_register = svd_regs::esp32c3::addr("SYSTEM", "RTC_FASTMEM_CONFIG");
        let result = svd_regs::esp32c3::addr("SYSTEM", "RTC_FASTMEM_CRC");
        let mut m = machine(&[HANG]);
        for (i, word) in [0x3433_3231u32, 0x3837_3635, 0x0000_0039]
            .into_iter()
            .enumerate()
        {
            let at = crate::memmap::RTC_RAM_BASE + 4 * u32::try_from(i).unwrap();
            m.bus.write(at, AccessSize::Word, word).unwrap();
        }
        // ADDR 0, LEN 1: the two words "12345678".
        m.bus
            .write(config_register, AccessSize::Word, 1 << 20)
            .unwrap();
        m.bus
            .write(config_register, AccessSize::Word, 1 << 20 | 1 << 8)
            .unwrap();
        assert_eq!(
            bus_word(&mut m, config_register),
            1 << 31 | 1 << 20 | 1 << 8,
            "FINISH at once"
        );
        // CRC-32/ISO-HDLC of "12345678".
        assert_eq!(bus_word(&mut m, result), 0x9AE0_DAAF);
        assert_eq!(m.drain_diagnostics(), []);
    }

    #[test]
    fn the_usb_host_changes_at_its_date() {
        let mut m = machine(&[HANG]);
        m.set_usb_host(Cycles(1_000), UsbHost::Attached).unwrap();
        assert_eq!(m.usb_host(), UsbHost::Open);
        assert_eq!(m.usb_host_changes_pending(), 1);
        m.run_within(
            Cycles(u64::MAX),
            Some(clock::TIME_BASE.cycles_to_nanos(Cycles(1_000))),
        );
        assert_eq!(m.usb_host(), UsbHost::Attached);
        assert_eq!(m.usb_host_changes_pending(), 0);
    }

    #[test]
    fn the_channels_are_named_as_the_command_line_names_them() {
        let names: Vec<&str> = ConsoleChannel::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names, ["uart0", "usb-serial-jtag", "uart1"]);
        for (place, channel) in ConsoleChannel::ALL.into_iter().enumerate() {
            assert_eq!(channel.index(), place);
        }
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

    /// A flash chip whose content the test can change behind the machine's back, without its
    /// generation moving, which the real chip never does: the way to see that a write to a cache
    /// control register drops the decoded code by itself.
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
        ) -> Result<Option<crate::flash::FlashWrite>, crate::flash::FlashRefusal> {
            Err(crate::flash::FlashRefusal::Unmodeled {
                opcode: command.opcode,
            })
        }
        fn generation(&self) -> u64 {
            0
        }
        fn modified_sectors(&self) -> u32 {
            0
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

    /// A machine whose flash holds `li a0, 3; ret` at offset 0 and whose ROM maps flash page 0
    /// at the start of the instruction window, calls that function, stores what it returned at
    /// the start of the SRAM, runs `between`, then calls the function again and stores the result
    /// in the next word.
    fn calls_flash_code_around(between: &[u32]) -> Esp32C3 {
        use cpu_rv32::asm::jalr;
        const A0: u8 = 10;
        const RA: u8 = 1;
        const T2: u8 = 7;
        const S0: u8 = 8;
        let image: Vec<u8> = [addi(A0, 0, 3), jalr(0, RA, 0)]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let mut program = vec![
            lui(T0, crate::periph::mmu::MMU_TABLE_BASE),
            lui(T2, crate::memmap::FLASH_IBUS_BASE),
            lui(S0, SRAM_DBUS_BASE),
            sw(0, T0, 0),    // entry 0 = flash page 0
            jalr(RA, T2, 0), // decodes the function: a0 = 3
            sw(A0, S0, 0),
        ];
        program.extend_from_slice(between);
        program.extend([jalr(RA, T2, 0), sw(A0, S0, 4), HANG]);
        Esp32C3::with_flash(flat_rom(&program), image, &SocConfig::default()).unwrap()
    }

    #[test]
    fn code_decoded_from_the_flash_is_decoded_again_once_the_flash_is_programmed() {
        // A program only clears bits: `li a0, 3` becomes `li a0, 1` (immediate 0b11 to 0b01).
        const A0: u8 = 10;
        let mut m = calls_flash_code_around(&flash_program(0, &addi(A0, 0, 1).to_le_bytes()));
        m.run_until(Cycles(600));
        // The program changed code the CPU had fetched, and nothing invalidated the cache: a
        // device may run the function of before once more. The fetch is reported, at its address.
        let window = crate::memmap::FLASH_IBUS_BASE;
        assert_eq!(
            m.drain_diagnostics(),
            vec![Diagnostic::StaleFlashWindow {
                vaddr: window,
                page: 0,
                pc: window,
            }]
        );
        assert_eq!(
            (
                bus_word(&mut m, SRAM_DBUS_BASE),
                bus_word(&mut m, SRAM_DBUS_BASE + 4)
            ),
            (3, 1),
            "the second call runs what the flash holds now"
        );
    }

    #[test]
    fn code_decoded_from_the_flash_is_decoded_again_once_the_flash_is_erased() {
        // After the erase the function is `FF FF FF FF`, which is no instruction: the second
        // call must fault there. Code kept decoded would return 3 a second time.
        let mut m = calls_flash_code_around(&flash_erase_sector(0));
        m.run_until(Cycles(600));
        let exceptions: Vec<(Exception, u32)> = m
            .drain_diagnostics()
            .into_iter()
            .filter_map(|diagnostic| match diagnostic {
                Diagnostic::Exception { cause, pc, .. } => Some((cause, pc)),
                _ => None,
            })
            .collect();
        assert_eq!(
            exceptions.first(),
            Some(&(Exception::IllegalInstr, crate::memmap::FLASH_IBUS_BASE)),
            "{exceptions:?}"
        );
        assert_eq!(
            (
                bus_word(&mut m, SRAM_DBUS_BASE),
                bus_word(&mut m, SRAM_DBUS_BASE + 4)
            ),
            (3, 0),
            "the first call returned 3; the second never returned"
        );
    }

    #[test]
    fn flash_code_run_again_after_an_invalidation_counts_as_read_again_for_a_later_program() {
        use cpu_rv32::asm::jalr;
        const A0: u8 = 10;
        const RA: u8 = 1;
        const T2: u8 = 7;
        const S0: u8 = 8;
        let window = crate::memmap::FLASH_IBUS_BASE;
        let extmem = |register| svd_regs::esp32c3::addr("EXTMEM", register);
        // `Cache_Invalidate_ICache_All`: `ICACHE_SYNC_ADDR` and `ICACHE_SYNC_SIZE` hold 0 from
        // reset, so the request is for every line (CM §4.1).
        let everything = store_register(extmem("ICACHE_SYNC_CTRL"), 1).to_vec();
        // `Cache_Invalidate_Addr(0x42000000, 0x10000)`: the `0x800` lines of the one entry that
        // maps the function (CM §4.1).
        let the_entry: Vec<u32> = [
            store_register(extmem("ICACHE_SYNC_ADDR"), window),
            store_register(extmem("ICACHE_SYNC_SIZE"), 0x800),
            store_register(extmem("ICACHE_SYNC_CTRL"), 1),
        ]
        .concat();
        // A program only clears bits: `li a0, 3` becomes `li a0, 1`.
        let patch = flash_program(0, &addi(A0, 0, 1).to_le_bytes());
        let results =
            |m: &mut Esp32C3| [0, 8, 4].map(|offset| bus_word(m, SRAM_DBUS_BASE + offset));

        for (request, invalidate) in [("everything", everything), ("the entry", the_entry)] {
            // The function is called, the cache is invalidated, the function is called again,
            // then programmed, then called. On a device the second call fills the line again
            // and nothing drops it before the third: the third may run the function of before.
            // The emulator must not have run the second call from the code it kept decoded,
            // without the cache state seeing it.
            let mut between = invalidate.clone();
            between.extend([jalr(RA, T2, 0), sw(A0, S0, 8)]);
            between.extend_from_slice(&patch);
            let mut m = calls_flash_code_around(&between);
            m.run_until(Cycles(800));
            assert_eq!(
                m.drain_diagnostics(),
                vec![Diagnostic::StaleFlashWindow {
                    vaddr: window,
                    page: 0,
                    pc: window,
                }],
                "{request}"
            );
            assert_eq!(results(&mut m), [3, 3, 1], "{request}");

            // The control: without the call in between, no line holds the function when it
            // is programmed, and the call after it reads the flash on a device too.
            let mut between = invalidate;
            between.extend_from_slice(&patch);
            let mut m = calls_flash_code_around(&between);
            m.run_until(Cycles(800));
            assert_eq!(m.drain_diagnostics(), Vec::new(), "{request}");
            assert_eq!(results(&mut m), [3, 0, 1], "{request}");
        }
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
        // IR §2.7 (TRM §1.5.3.1): an interrupt "will take exactly 4 cycles before the CPU starts
        // processing" it.
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
    fn a_cache_fault_cleared_by_its_handler_is_latched_again_by_the_next_faulty_fetch() {
        // The same fault, served: the handler of line 6 counts its calls in RAM, clears the
        // status (`CACHE_ILG_INT_CLR` bit 5), notes what the status and the pending lines read
        // after that, and jumps into the unmapped window again. The second fetch must raise the
        // source a second time; the handler parks at its second call.
        const COUNT: i32 = 0x100;
        const STATUS_AFTER_CLEAR: i32 = 0x104;
        const PENDING_AFTER_CLEAR: i32 = 0x108;
        const CAUSE: i32 = 0x10C;
        const MCAUSE: u16 = 0x342;
        let (ram, count, scratch, two) = (8, 9, 11, 12);
        let mut body = vec![lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)];
        body.extend(route("CACHE_IA_INT_MAP", 6));
        body.extend(store_register(reg("EXTMEM", "CACHE_ILG_INT_ENA"), 1 << 5));
        let into_the_window = [
            lui(T1, crate::memmap::FLASH_IBUS_BASE),
            cpu_rv32::asm::jalr(0, T1, 0),
        ];
        body.extend(into_the_window);
        let mut program = with_vectors(&body);
        let table = ((VECTORS - ROM_BASE) / 4) as usize;
        // Exceptions enter at the base of the table and enable interrupts; the interrupt is
        // taken in the instructions that follow, which never run on into the entry of line 6.
        program[table] = csrrwi(0, MSTATUS, MSTATUS_MIE);
        program[table + 1..table + 5].fill(NOP);
        program[table + 5] = HANG;
        let handler = i32::try_from(4 * (program.len() - (table + 6))).unwrap();
        program[table + 6] = jal(0, handler);
        program.extend([
            lui(ram, SRAM_DBUS_BASE),
            lw(count, ram, COUNT),
            addi(count, count, 1),
            sw(count, ram, COUNT),
            csrrs(scratch, MCAUSE, 0),
            sw(scratch, ram, CAUSE),
        ]);
        program.extend(store_register(reg("EXTMEM", "CACHE_ILG_INT_CLR"), 1 << 5));
        program.extend(load_into(scratch, reg("EXTMEM", "CACHE_ILG_INT_ST")));
        program.push(sw(scratch, ram, STATUS_AFTER_CLEAR));
        program.extend(load_into(
            scratch,
            reg("INTERRUPT_CORE0", "CPU_INT_EIP_STATUS"),
        ));
        program.push(sw(scratch, ram, PENDING_AFTER_CLEAR));
        program.extend([addi(two, 0, 2), cpu_rv32::asm::beq(count, two, 12)]);
        program.extend(into_the_window);
        let park = ROM_BASE + 4 * u32::try_from(program.len()).unwrap();
        program.push(HANG);
        let mut m = machine(&program);
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        let word = |m: &mut Esp32C3, offset: i32| {
            bus_word(m, SRAM_DBUS_BASE + u32::try_from(offset).unwrap())
        };
        assert_eq!(
            word(&mut m, COUNT),
            2,
            "the handler ran twice: the second faulty fetch raised the source again"
        );
        assert_eq!(m.pc(), park);
        assert_eq!(
            word(&mut m, CAUSE),
            0x8000_0006,
            "each time as the interrupt of line 6"
        );
        assert_eq!(
            (
                word(&mut m, STATUS_AFTER_CLEAR),
                word(&mut m, PENDING_AFTER_CLEAR)
            ),
            (0, 0),
            "the clear lowered the status and the line: the second interrupt is a new fault, not the first one still standing"
        );
    }

    #[test]
    fn a_line_under_the_threshold_or_disabled_is_not_taken() {
        for (what, register, value) in [
            // What a critical section does: `CPU_INT_THRESH` above the priority of the line
            // (IR §3.3).
            ("masked by the threshold", "CPU_INT_THRESH", 2),
            ("disabled", "CPU_INT_ENABLE", 0),
        ] {
            let mut body = Vec::new();
            body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
            body.extend(route("CPU_INTR_FROM_CPU_0_MAP", 3));
            body.extend(store_register(reg("INTERRUPT_CORE0", register), value));
            body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
            body.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
            body.extend([NOP; 16]);
            body.push(HANG);
            let hang = ROM_BASE + 4 * u32::try_from(body.len() - 1).unwrap();
            let mut m = machine(&with_vectors(&body));
            assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
            assert_eq!(m.pc(), hang, "{what}");
            assert_eq!(m.hart.csr.mcause, 0, "{what}");
            assert_eq!(
                bus_word(&mut m, reg("INTERRUPT_CORE0", "CPU_INT_EIP_STATUS")),
                0,
                "{what}: the line is not pending although its source is raised"
            );
            assert_eq!(
                bus_word(&mut m, reg("SYSTEM", "CPU_INTR_FROM_CPU_0")),
                1,
                "{what}: the source"
            );
        }
    }

    /// Runs one instruction a call until the PC is `pc`, and fails if `limit` instructions do not
    /// get there.
    fn step_to(m: &mut Esp32C3, pc: u32, limit: u32) {
        for _ in 0..limit {
            if m.pc() == pc {
                return;
            }
            let next = Cycles(m.cycles().get() + 1);
            assert_eq!(m.run_until(next), StopReason::CycleBudget);
        }
        assert_eq!(
            m.pc(),
            pc,
            "pc {pc:#010x} not reached in {limit} instructions"
        );
    }

    #[test]
    fn a_higher_priority_line_blanks_the_one_the_cpu_could_see_for_the_latency() {
        // The assumption marked on `INTERRUPT_LATENCY_CYCLES`. Software interrupt 0 on line 3 at
        // priority 1 is raised with the interrupts masked and becomes visible; software interrupt
        // 1 on line 4 at priority 2 is raised next, and `MIE` is set by the instruction after.
        let mut body = vec![lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)];
        let controller = |register| reg("INTERRUPT_CORE0", register);
        for (register, value) in [
            ("CPU_INTR_FROM_CPU_0_MAP", 3),
            ("CPU_INTR_FROM_CPU_1_MAP", 4),
            ("CPU_INT_PRI_3", 1),
            ("CPU_INT_PRI_4", 2),
            ("CPU_INT_ENABLE", (1 << 3) | (1 << 4)),
        ] {
            body.extend(store_register(controller(register), value));
        }
        body.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
        body.extend([NOP; 8]);
        body.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_1"), 1));
        let second_raised = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
        body.extend([NOP; 16]);
        let mut m = machine(&with_vectors(&body));
        let period = m.bus.ctx.clock().cpu_period();
        // Before the store that raises the second source: line 3 is selected and visible.
        step_to(&mut m, second_raised - 4, 200);
        let (line, visible_at) = m.selected.unwrap();
        assert_eq!(line, 3);
        assert!(visible_at <= m.bus.ctx.now(), "its latency has elapsed");
        // After it: line 4 is selected, and visible four CPU cycles later.
        step_to(&mut m, second_raised, 1);
        assert_eq!(
            m.selected,
            Some((4, m.bus.ctx.now() + INTERRUPT_LATENCY_CYCLES * period))
        );
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        assert_eq!(
            m.hart.csr.mcause, 0x8000_0004,
            "line 3 was not taken when `MIE` was set, one cycle after the change: the CPU saw no line"
        );
        assert_eq!(
            m.hart.csr.mepc,
            second_raised + 4 * 4,
            "the instruction that sets `MIE` and three more ran before line 4 was taken"
        );
        assert_eq!(m.pc(), VECTORS + 4 * 4, "the entry of line 4 parks");
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

    const MILLISECOND_NS: u64 = 1_000_000;
    const SECOND_NS: u64 = 1_000_000_000;
    /// `SYSTIMER.CONF`: its reset value with `TARGET0_WORK_EN`, bit 24 (`systimer_ll_enable_alarm`).
    const SYSTIMER_ALARM0_ON: u32 = 0x4600_0000 | (1 << 24);
    /// More CPU cycles than any device-time test whose CPU sleeps executes: its limit is the time.
    const NO_CYCLE_LIMIT: Cycles = Cycles(1_000_000_000);
    /// The cycle limit of the device-time tests whose CPU executes without end, all of them
    /// limited to a millisecond or less: five times the 20 000 cycles a millisecond holds at the
    /// 20 MHz of the reset clock. A correct run never reaches it, and a run the time limit fails
    /// to stop ends at once on it.
    const BUSY_CYCLE_LIMIT: Cycles = Cycles(100_000);

    /// What two runs must agree on to be the same run.
    fn state(m: &Esp32C3) -> (Cycles, Cycles, u32, u64, Option<u64>, bool) {
        (
            m.cycles(),
            m.bus.ctx.now(),
            m.pc(),
            m.resets(),
            m.trace_hash(),
            m.is_asleep(),
        )
    }

    fn traced(program: &[u32]) -> Esp32C3 {
        machine_with(
            program,
            SocConfig {
                trace_hash: true,
                ..SocConfig::default()
            },
        )
    }

    #[test]
    fn a_device_time_limit_ends_a_sleep_at_exactly_its_instant() {
        // A period alarm every 16 384 counts of the 16 MHz counter (1.024 ms), its interrupt not
        // enabled: the CPU sleeps for ever under wake-ups that wake nothing. Half a second is no
        // multiple of the period: the last alarm before it is delivered, the next one is not, and
        // device time stops on the limit, not on an alarm.
        let mut body = stop_watchdogs();
        body.extend(store_register(
            reg("SYSTIMER", "TARGET0_CONF"),
            (1 << 30) | 16_384,
        ));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        body.extend(store_register(reg("SYSTIMER", "CONF"), SYSTIMER_ALARM0_ON));
        body.extend([wfi(), HANG]);
        let wfi_at = ROM_BASE + 4 * u32::try_from(body.len() - 2).unwrap();
        let mut m = machine(&body);
        // The program is asleep long before the first alarm: a first limit inside that sleep.
        assert_eq!(
            m.run_within(NO_CYCLE_LIMIT, Some(MILLISECOND_NS)),
            StopReason::TimeBudget
        );
        assert_eq!(m.device_time_nanos(), MILLISECOND_NS);
        assert!(m.is_asleep());
        assert_eq!(
            bus_word(&mut m, reg("SYSTIMER", "INT_RAW")),
            0,
            "the first alarm is after the limit: it was not delivered"
        );
        let executed = m.cycles();
        assert_eq!(
            m.run_within(NO_CYCLE_LIMIT, Some(SECOND_NS / 2)),
            StopReason::TimeBudget
        );
        assert_eq!(m.device_time_nanos(), 500_000_000);
        assert_eq!(m.cycles(), executed, "a sleeping CPU executes no cycle");
        assert!(m.is_asleep());
        assert_eq!(m.pc(), wfi_at + 4, "the instruction after the `wfi`");
        assert_eq!(
            bus_word(&mut m, reg("SYSTIMER", "INT_RAW")),
            1,
            "the alarms before the limit were delivered"
        );
        assert_eq!(
            m.run_within(NO_CYCLE_LIMIT, Some(SECOND_NS / 2)),
            StopReason::TimeBudget,
            "the limit is an instant since power-on: reached, it stays reached"
        );
        assert_eq!(m.device_time_nanos(), 500_000_000);
    }

    /// A one-shot alarm two seconds ahead whose interrupt is not enabled, then `wfi`.
    fn a_sleep_to_an_alarm_two_seconds_ahead() -> Esp32C3 {
        let mut body = stop_watchdogs();
        body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), 32_000_000));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        body.extend(store_register(reg("SYSTIMER", "CONF"), SYSTIMER_ALARM0_ON));
        body.extend([wfi(), HANG]);
        traced(&body)
    }

    #[test]
    fn a_sleep_cut_by_a_device_time_limit_goes_on_as_if_it_had_not_been() {
        let mut cut = a_sleep_to_an_alarm_two_seconds_ahead();
        assert_eq!(
            cut.run_within(NO_CYCLE_LIMIT, Some(SECOND_NS / 2)),
            StopReason::TimeBudget
        );
        assert_eq!(cut.device_time_nanos(), 500_000_000);
        assert_eq!(
            bus_word(&mut cut, reg("SYSTIMER", "INT_RAW")),
            0,
            "the alarm is 1.5 s away: nothing was delivered"
        );
        let mut whole = a_sleep_to_an_alarm_two_seconds_ahead();
        for m in [&mut cut, &mut whole] {
            // The alarm is before the limit: the sleep ends on it, as without a limit, and the
            // call gives control back for having slept more than a second.
            assert_eq!(
                m.run_within(NO_CYCLE_LIMIT, Some(3 * SECOND_NS)),
                StopReason::Asleep
            );
            assert_eq!(
                m.device_time_nanos(),
                2 * SECOND_NS,
                "the alarm at its own instant"
            );
            assert_eq!(bus_word(m, reg("SYSTIMER", "INT_RAW")), 1);
        }
        assert_eq!(state(&cut), state(&whole));
        assert!(whole.trace_hash().is_some());
        for m in [&mut cut, &mut whole] {
            assert_eq!(
                m.run_within(NO_CYCLE_LIMIT, Some(3 * SECOND_NS)),
                StopReason::Idle,
                "nothing else is scheduled: no limit is reached, the run is over"
            );
            assert_eq!(
                m.device_time_nanos(),
                2 * SECOND_NS,
                "time is not moved to the limit"
            );
        }
    }

    #[test]
    fn a_device_time_limit_stops_an_executing_cpu_at_the_first_instruction_boundary_past_it() {
        // `j .` at the 20 MHz of the reset clock: a CPU cycle is 8 cycles of the time base, and
        // one millisecond is 160 000 of those.
        const LIMIT: u64 = 160_000;
        let cost = {
            let mut one = machine(&[HANG]);
            one.run_until(Cycles(1));
            one.bus.ctx.now().get()
        };
        let mut whole = traced(&[HANG]);
        assert_eq!(
            whole.run_within(BUSY_CYCLE_LIMIT, Some(MILLISECOND_NS)),
            StopReason::TimeBudget
        );
        let now = whole.bus.ctx.now().get();
        assert!(
            (LIMIT..LIMIT + cost).contains(&now),
            "stopped at {now}: the instruction that crosses {LIMIT} is executed whole ({cost} cycles of the time base), and no other after it"
        );
        assert!(!whole.is_asleep());
        // The stop does not depend on how the run is cut into calls.
        let mut sliced = traced(&[HANG]);
        let mut calls = 0;
        loop {
            let next = Cycles(sliced.cycles().get() + 7);
            match sliced.run_within(next, Some(MILLISECOND_NS)) {
                StopReason::CycleBudget => {}
                StopReason::TimeBudget => break,
                other => panic!("{other:?}"),
            }
            calls += 1;
            assert!(calls < 100_000, "the limit is never reached");
        }
        assert_eq!(state(&sliced), state(&whole));
        assert!(whole.trace_hash().is_some());
    }

    #[test]
    fn a_cpu_nothing_can_wake_is_idle_whatever_the_device_time_limit() {
        // Strapped for another boot mode, no watchdog runs at reset: nothing is scheduled.
        let config = SocConfig {
            strapping: 0,
            ..SocConfig::default()
        };
        let mut m = machine_with(&[wfi(), HANG], config);
        assert_eq!(
            m.run_within(NO_CYCLE_LIMIT, Some(SECOND_NS)),
            StopReason::Idle,
            "a dead firmware is the more useful thing to report"
        );
        assert_eq!(
            m.device_time_nanos(),
            150,
            "device time is not moved to the limit"
        );
        assert!(m.is_asleep());
    }

    #[test]
    fn when_both_limits_are_reached_at_the_same_boundary_the_cycle_limit_is_reported() {
        let mut by_time = machine(&[HANG]);
        assert_eq!(
            by_time.run_within(BUSY_CYCLE_LIMIT, Some(MILLISECOND_NS)),
            StopReason::TimeBudget
        );
        let boundary = by_time.cycles();
        let mut both = machine(&[HANG]);
        assert_eq!(
            both.run_within(boundary, Some(MILLISECOND_NS)),
            StopReason::CycleBudget,
            "it is tested first"
        );
        assert_eq!(both.cycles(), boundary);
        assert_eq!(both.bus.ctx.now(), by_time.bus.ctx.now());
        assert_eq!(
            both.run_within(Cycles(boundary.get() + 1), Some(MILLISECOND_NS)),
            StopReason::TimeBudget,
            "one more cycle allowed: the time limit is the one that holds the run"
        );
        assert_eq!(both.cycles(), boundary, "and nothing ran");
    }

    #[test]
    fn a_device_time_limit_of_zero_stops_before_the_first_instruction() {
        let mut m = machine(&[HANG]);
        assert_eq!(
            m.run_within(BUSY_CYCLE_LIMIT, Some(0)),
            StopReason::TimeBudget
        );
        assert_eq!((m.cycles(), m.pc()), (Cycles(0), ROM_BASE));
    }

    #[test]
    fn the_limit_instant_is_rounded_up_to_a_cycle_of_the_time_base() {
        // 6.25 ns a cycle: a run never ends before the time asked.
        for (nanos, cycles) in [
            (0, 0),
            (1, 1),
            (6, 1),
            (7, 2),
            (25, 4),
            (1_000, 160),
            (SECOND_NS, clock::SECOND),
            // The longest time there is: it fits, and it is not the instant that means "never".
            (u64::MAX, 2_951_479_051_793_528_259),
        ] {
            assert_eq!(limit_instant(nanos), Cycles(cycles), "{nanos} ns");
            assert!(
                clock::TIME_BASE.cycles_to_nanos(limit_instant(nanos)) >= nanos,
                "{nanos} ns"
            );
        }
    }

    /// A short loop, then `wfi`, with the watchdogs of the reset values left on: main watchdog 0
    /// resets the core at 1.3 s (TW §D.4), and the second life does the same.
    fn a_sleep_under_the_flash_boot_watchdog() -> Esp32C3 {
        traced(&[
            addi(14, 0, 500),
            addi(14, 14, -1),
            cpu_rv32::asm::bne(14, 0, -4),
            wfi(),
            HANG,
        ])
    }

    /// Calls `run_within` with the cycle limit `next` gives until the device-time limit `nanos`
    /// ends a call.
    fn run_to_the_time_limit(m: &mut Esp32C3, nanos: u64, next: impl Fn(&Esp32C3) -> Cycles) {
        let mut calls = 0;
        loop {
            let limit = next(m);
            match m.run_within(limit, Some(nanos)) {
                StopReason::CycleBudget | StopReason::Asleep => {}
                StopReason::TimeBudget => return,
                other => panic!("{other:?}"),
            }
            calls += 1;
            assert!(calls < 100_000, "the limit of {nanos} ns is never reached");
        }
    }

    #[test]
    fn a_device_time_limit_past_a_watchdog_reset_is_reached_the_same_whole_or_in_slices() {
        const LIMIT_NS: u64 = 2 * SECOND_NS;
        /// The instant main watchdog 0 resets the chip at.
        const RESET_NS: u64 = 1_300 * MILLISECOND_NS;
        let mut whole = a_sleep_under_the_flash_boot_watchdog();
        run_to_the_time_limit(&mut whole, LIMIT_NS, |_| NO_CYCLE_LIMIT);
        assert_eq!(whole.device_time_nanos(), LIMIT_NS, "exactly the limit");
        assert_eq!(
            whole.resets(),
            1,
            "the reset at 1.3 s, not yet the one at 2.6 s"
        );
        assert!(whole.is_asleep());
        assert!(whole.trace_hash().is_some());
        // In calls of seven cycles.
        let mut sliced = a_sleep_under_the_flash_boot_watchdog();
        run_to_the_time_limit(&mut sliced, LIMIT_NS, |m| Cycles(m.cycles().get() + 7));
        assert_eq!(state(&sliced), state(&whole));
        // In steps of device time, one of them the instant of the reset itself.
        let mut stepped = a_sleep_under_the_flash_boot_watchdog();
        for nanos in [400, 800, 1_200, 1_300, 1_600, 2_000] {
            let nanos = nanos * MILLISECOND_NS;
            run_to_the_time_limit(&mut stepped, nanos, |_| NO_CYCLE_LIMIT);
            assert_eq!(stepped.device_time_nanos(), nanos);
            assert_eq!(stepped.resets(), u64::from(nanos >= 1_300 * MILLISECOND_NS));
        }
        assert_eq!(state(&stepped), state(&whole));
        // A limit on the instant of the reset, reached by one sleep of more than a second: the
        // call says it slept, and the next one that the limit is reached, without running
        // anything of the second life.
        let mut at_the_reset = a_sleep_under_the_flash_boot_watchdog();
        assert_eq!(
            at_the_reset.run_within(NO_CYCLE_LIMIT, Some(RESET_NS)),
            StopReason::Asleep
        );
        assert_eq!(
            (at_the_reset.device_time_nanos(), at_the_reset.resets()),
            (RESET_NS, 1)
        );
        let executed = at_the_reset.cycles();
        assert_eq!(
            at_the_reset.run_within(NO_CYCLE_LIMIT, Some(RESET_NS)),
            StopReason::TimeBudget
        );
        assert_eq!(
            (at_the_reset.cycles(), at_the_reset.pc()),
            (executed, ROM_BASE)
        );
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

    /// `RTC_CNTL.OPTIONS0` bit 5, `SW_PROCPU_RST`: the software reset of the CPU (IR §4.1).
    const SOFTWARE_CPU_RESET: u32 = 1 << 5;
    /// `RTC_CNTL.OPTIONS0` bit 31, `SW_SYS_RST`: the software reset of the core.
    const SOFTWARE_CORE_RESET: u32 = 1 << 31;

    /// Two passes around a CPU reset. The first routes software interrupt 0 to line 3, raises it
    /// with the interrupts masked and asks for the reset; the controller and the source levels
    /// are taken to survive a CPU reset (the assumption marked on `Peripherals::reset`), so the
    /// second pass finds the line still selected. It sets `MIE` when `mie` is true and sleeps in
    /// `wfi`.
    fn a_line_pending_across_a_cpu_reset(mie: bool) -> Esp32C3 {
        a_line_pending_across_a_reset(
            store_register(reg("RTC_CNTL", "OPTIONS0"), SOFTWARE_CPU_RESET).to_vec(),
            mie,
        )
    }

    /// The same two passes around the reset `trigger` asks for.
    fn a_line_pending_across_a_reset(trigger: Vec<u32>, mie: bool) -> Esp32C3 {
        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
        let marker = load_register(MARKER);
        let mut first = stop_watchdogs();
        first.extend(store_register(MARKER, 1));
        first.extend(route("CPU_INTR_FROM_CPU_0_MAP", 3));
        first.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
        first.extend(trigger);
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

    /// What `route("CPU_INTR_FROM_CPU_0_MAP", 3)` wrote into the interrupt controller, read
    /// back: the line enables, the map register of the source and the priority of line 3. Only
    /// the first of the two passes writes them.
    fn routing_of_line_3(m: &mut Esp32C3) -> (u32, u32, u32) {
        (
            bus_word(m, reg("INTERRUPT_CORE0", "CPU_INT_ENABLE")),
            bus_word(m, reg("INTERRUPT_CORE0", "CPU_INTR_FROM_CPU_0_MAP")),
            bus_word(m, reg("INTERRUPT_CORE0", "CPU_INT_PRI_3")),
        )
    }

    #[test]
    fn a_line_selected_before_a_cpu_reset_becomes_visible_four_cpu_cycles_after_it() {
        let mut m = a_line_pending_across_a_cpu_reset(true);
        // One instruction a call, to the boundary at which the reset has just taken place.
        for _ in 0..200 {
            if m.resets() == 1 {
                break;
            }
            let next = Cycles(m.cycles().get() + 1);
            assert_eq!(m.run_until(next), StopReason::CycleBudget);
        }
        assert_eq!((m.resets(), m.pc()), (1, ROM_BASE), "the reset, just now");
        let period = m.bus.ctx.clock().cpu_period();
        assert_eq!(
            m.selected,
            Some((3, m.bus.ctx.now() + INTERRUPT_LATENCY_CYCLES * period)),
            "the hart forgot the line: it is selected anew, with the latency of a new line"
        );
    }

    #[test]
    fn a_line_selected_before_a_cpu_reset_wakes_a_masked_wfi_after_it() {
        // A CPU reset is taken to leave the interrupt controller and the sources as they are
        // (the assumption marked on `Peripherals::reset`).
        let mut m = a_line_pending_across_a_cpu_reset(false);
        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
        assert_eq!(m.resets(), 1);
        assert_eq!(
            bus_word(&mut m, reg("INTERRUPT_CORE0", "CPU_INT_EIP_STATUS")),
            1 << 3,
            "the source is still raised"
        );
        assert_eq!(
            routing_of_line_3(&mut m),
            (1 << 3, 3, 1),
            "the controller keeps what the first pass wrote: enable, map, priority"
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
    fn a_line_pending_before_a_core_or_a_system_reset_is_gone_after_it() {
        // A core reset rebuilds the interrupt controller and the software interrupt registers
        // (TRM §6.1.3): the source is low, nothing is routed, nothing is selected. The RTC
        // watchdog's stage 0 resets the whole chip 100 cycles of the slow clock after it is
        // started (735 us), while the first pass hangs.
        let mut by_the_rtc_watchdog = store_register(reg("RTC_CNTL", "WDTCONFIG1"), 100).to_vec();
        by_the_rtc_watchdog.extend(store_register(
            reg("RTC_CNTL", "WDTCONFIG0"),
            (1 << 31) | (4 << 28),
        ));
        for (cause, trigger) in [
            (
                0x03,
                store_register(reg("RTC_CNTL", "OPTIONS0"), SOFTWARE_CORE_RESET).to_vec(),
            ),
            (0x10, by_the_rtc_watchdog),
        ] {
            let mut m = a_line_pending_across_a_reset(trigger, true);
            // A tenth of a second: long after the reset, long before the watchdogs the reset
            // values arm again.
            assert_eq!(
                m.run_within(Cycles(40_000), Some(100 * MILLISECOND_NS)),
                StopReason::TimeBudget,
                "{cause:#x}"
            );
            assert_eq!(m.resets(), 1, "{cause:#x}");
            assert_eq!(
                bus_word(&mut m, reg("RTC_CNTL", "RESET_STATE")) & 0x3F,
                cause
            );
            assert_eq!(m.selected, None, "{cause:#x}: no line is selected");
            assert_eq!(
                (
                    bus_word(&mut m, reg("SYSTEM", "CPU_INTR_FROM_CPU_0")),
                    bus_word(&mut m, reg("INTERRUPT_CORE0", "CPU_INT_EIP_STATUS")),
                ),
                (0, 0),
                "{cause:#x}: the source and the line"
            );
            assert_eq!(
                routing_of_line_3(&mut m),
                (0, 0, 0),
                "{cause:#x}: the controller is rebuilt: enable, map and priority at their reset values"
            );
            assert_eq!(
                m.hart.csr.mcause, 0,
                "{cause:#x}: the second pass set `MIE` and took nothing"
            );
            assert!(
                m.is_asleep(),
                "{cause:#x}: its `wfi` waits, where the line would have ended it"
            );
        }
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
        // A call runs to its limit or past it, so `end / 7 + 1` calls reach the end; one that
        // stops for another reason fails the test here instead of keeping the loop going.
        for _ in 0..=end / 7 {
            if sliced.cycles().get() >= end {
                break;
            }
            let next = (sliced.cycles().get() + 7).min(end);
            assert_eq!(sliced.run_until(Cycles(next)), StopReason::CycleBudget);
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
    fn a_reset_neither_mixes_into_the_trace_nor_forgets_it() {
        // Two machines execute the same instructions at the same addresses, up to and including
        // one store to `RTC_CNTL.OPTIONS0`: the first stores the request for a CPU reset, the
        // second stores 0. The trace is the sequence of executed PCs, so after that store the two
        // are equal: a trace that started again at the reset would be the empty one, and one the
        // reset had been mixed into would be different.
        let program = |value| {
            let mut body = stop_watchdogs();
            body.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), value));
            body.push(HANG);
            body
        };
        let after_the_store = ROM_BASE + 4 * u32::try_from(program(0).len() - 1).unwrap();
        let mut plain = traced(&program(0));
        step_to(&mut plain, after_the_store, 100);
        let mut reset = traced(&program(SOFTWARE_CPU_RESET));
        assert_eq!(reset.run_until(plain.cycles()), StopReason::CycleBudget);
        assert_eq!((reset.resets(), plain.resets()), (1, 0));
        assert_eq!(
            (reset.cycles(), reset.instructions()),
            (plain.cycles(), plain.instructions()),
            "the same instructions ran"
        );
        assert_eq!((reset.pc(), plain.pc()), (ROM_BASE, after_the_store));
        assert_eq!(reset.trace_hash(), plain.trace_hash());
        assert_ne!(
            reset.trace_hash(),
            traced(&program(0)).trace_hash(),
            "and it is not the trace of a machine that has executed nothing"
        );
        // The trace goes on after the reset: the next instruction of each is at another address.
        for m in [&mut reset, &mut plain] {
            let next = Cycles(m.cycles().get() + 1);
            assert_eq!(m.run_until(next), StopReason::CycleBudget);
        }
        assert_ne!(reset.trace_hash(), plain.trace_hash());
    }

    /// A firmware in miniature, with everything that can make a run depend on how it is cut into
    /// calls: a period alarm of the SYSTIMER every 400 us whose handler (line 4) raises a
    /// software interrupt of higher priority (line 3), a main loop that writes a byte to UART0
    /// and sleeps in `wfi` until the next tick, and the watchdogs of the reset values left on, so
    /// that the chip is reset at 1.3 s, 2.6 s and 2.94 s and starts again each time.
    ///
    /// The first and the third reset find the CPU asleep. The second finds it executing: the
    /// life that follows the first reset (a count of the lives, kept in RAM, tells it which one
    /// it is) stops sleeping [`TickingFirmware::BUSY_AFTER`] ticks after its start, two
    /// milliseconds before its watchdog, and counts in a loop the ticks go on interrupting.
    fn a_ticking_firmware_under_the_reset_watchdogs() -> TickingFirmware {
        const TICK: u8 = 4;
        const SOFTWARE: u8 = 3;
        // 6 400 counts of the 16 MHz counter. A byte takes 347 us to leave at the reset baud rate
        // and clock: one byte a tick, and none waits for the one before it.
        const PERIOD: u32 = 6_400;
        /// The lives before this one, kept across the resets.
        const LIVES: u32 = SRAM_DBUS_BASE + 0x100;
        let (page, value) = (28, 29);
        // 0 in the life that follows the first reset; the ticks left before it stops sleeping;
        // what its busy loop counts.
        let (not_the_second_life, ticks_left, count) = (27, 30, 31);
        let split = |addr: u32| {
            let upper = addr.wrapping_add(0x800) & 0xFFFF_F000;
            (upper, addr.wrapping_sub(upper).cast_signed())
        };
        let (clear, clear_offset) = split(reg("SYSTIMER", "INT_CLR"));
        let (software, software_offset) = split(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"));
        let (uart, uart_offset) = split(UART0);
        let (lives, lives_offset) = split(LIVES);
        let (ticks_upper, ticks_lower) = split(TickingFirmware::BUSY_AFTER);
        let mut body = vec![lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)];
        // A reset clears the registers and keeps the RAM.
        body.extend([
            lui(T0, lives),
            lw(T1, T0, lives_offset),
            addi(not_the_second_life, T1, -1),
            addi(T1, T1, 1),
            sw(T1, T0, lives_offset),
            lui(ticks_left, ticks_upper),
            addi(ticks_left, ticks_left, ticks_lower),
        ]);
        for (register, setting) in [
            ("SYSTIMER_TARGET0_INT_MAP", u32::from(TICK)),
            ("CPU_INTR_FROM_CPU_0_MAP", u32::from(SOFTWARE)),
            ("CPU_INT_PRI_3", 2),
            ("CPU_INT_PRI_4", 1),
            ("CPU_INT_ENABLE", (1 << SOFTWARE) | (1 << TICK)),
            ("CPU_INT_THRESH", 1),
        ] {
            body.extend(store_register(reg("INTERRUPT_CORE0", register), setting));
        }
        body.extend(store_register(
            reg("SYSTIMER", "TARGET0_CONF"),
            (1 << 30) | PERIOD,
        ));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        body.extend(store_register(reg("SYSTIMER", "INT_ENA"), 1));
        body.extend(store_register(reg("SYSTIMER", "CONF"), SYSTIMER_ALARM0_ON));
        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
        // The main loop: a byte, then sleep until an interrupt; back to the byte. The life that
        // follows the first reset leaves it once its ticks are counted down, for a loop that
        // only counts.
        body.extend([
            lui(10, uart),
            addi(11, 0, i32::from(b'.')),
            sw(11, 10, uart_offset),
            wfi(),
            cpu_rv32::asm::bne(not_the_second_life, 0, -12),
            cpu_rv32::asm::blt(0, ticks_left, -16),
            addi(count, count, 1),
            jal(0, -4),
        ]);
        let end = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
        let (after_the_wfi, busy_loop) = (end - 16, end - 8..end);
        let mut program = with_vectors(&body);
        let table = ((VECTORS - ROM_BASE) / 4) as usize;
        let handler_after = |program: &[u32], line: u8| {
            jal(
                0,
                4 * i32::try_from(program.len() - (table + usize::from(line))).unwrap(),
            )
        };
        // The tick: acknowledge the alarm, count it, raise the software interrupt, return.
        program[table + usize::from(TICK)] = handler_after(&program, TICK);
        program.extend([
            lui(page, clear),
            addi(value, 0, 1),
            sw(value, page, clear_offset),
            addi(ticks_left, ticks_left, -1),
            lui(page, software),
            sw(value, page, software_offset),
            cpu_rv32::asm::mret(),
        ]);
        // The software interrupt: lower it, return.
        program[table + usize::from(SOFTWARE)] = handler_after(&program, SOFTWARE);
        program.extend([
            lui(page, software),
            sw(0, page, software_offset),
            cpu_rv32::asm::mret(),
        ]);
        TickingFirmware {
            program,
            after_the_wfi,
            busy_loop,
        }
    }

    /// The program of [`a_ticking_firmware_under_the_reset_watchdogs`] and the two places a
    /// reset can find it at.
    struct TickingFirmware {
        program: Vec<u32>,
        /// Where the PC of a sleeping CPU stands: the instruction after the `wfi`.
        after_the_wfi: u32,
        /// The loop the life that follows the first reset ends in.
        busy_loop: std::ops::Range<u32>,
    }

    impl TickingFirmware {
        /// Ticks of 400 us after which the life that follows the first reset stops sleeping:
        /// 1.298 s, five ticks before the watchdog that resets it at 1.3 s.
        const BUSY_AFTER: u32 = 3_245;
    }

    /// Everything a run leaves behind that a caller can see.
    #[derive(Debug, PartialEq)]
    struct Outcome {
        trace_hash: Option<u64>,
        device_time: Cycles,
        cycles: Cycles,
        instructions: u64,
        pc: u32,
        asleep: bool,
        resets: u64,
        /// The bytes UART0 sent, every one of them the `.` of the main loop: their number is the
        /// whole console.
        console_bytes: usize,
        journal: Vec<Diagnostic>,
        repeated: Vec<(Diagnostic, u64)>,
    }

    /// Runs the ticking firmware to `total` cycles in calls of `slice` cycles, taking the console
    /// after each call as a front end does.
    fn tick_in_calls_of(slice: u64, total: u64) -> Outcome {
        let mut m = traced(&a_ticking_firmware_under_the_reset_watchdogs().program);
        let (mut console_bytes, mut journal) = (0, Vec::new());
        // A call that returns `CycleBudget` has run to its limit; one that returns `Asleep` has
        // slept through a second of device time, and the run lasts under four: 64 of those is
        // far more than there can be.
        let mut calls_left = total / slice + 64;
        while m.cycles().get() < total {
            assert!(
                calls_left > 0,
                "no progress in calls of {slice} cycles: {} cycles at {} ns of device time",
                m.cycles().get(),
                m.device_time_nanos()
            );
            calls_left -= 1;
            let limit = m.cycles().get().saturating_add(slice).min(total);
            let stop = m.run_until(Cycles(limit));
            assert!(
                matches!(stop, StopReason::CycleBudget | StopReason::Asleep),
                "{stop:?}"
            );
            for channel in ConsoleChannel::ALL {
                let bytes = m.take_console(channel);
                if channel == ConsoleChannel::Uart0 {
                    assert!(bytes.iter().all(|byte| *byte == b'.'), "{bytes:?}");
                    console_bytes += bytes.len();
                } else {
                    assert_eq!(bytes, b"", "{channel:?}");
                }
            }
            journal.extend(m.drain_diagnostics());
        }
        Outcome {
            trace_hash: m.trace_hash(),
            device_time: m.bus.ctx.now(),
            cycles: m.cycles(),
            instructions: m.instructions(),
            pc: m.pc(),
            asleep: m.is_asleep(),
            resets: m.resets(),
            console_bytes,
            journal,
            repeated: m.most_repeated_diagnostics(8),
        }
    }

    #[test]
    fn a_run_with_interrupts_sleeps_output_and_resets_is_the_same_however_it_is_cut_into_calls() {
        // This firmware sleeps most of each tick and executes about 70 000 cycles in a second of
        // device time, and 40 000 more in the two milliseconds its second life is busy: 260 000
        // cycles are a little over three seconds, past the three resets and into a fourth life.
        const TOTAL: u64 = 260_000;
        let whole = tick_in_calls_of(u64::MAX, TOTAL);
        assert_eq!(whole.resets, 3, "the two watchdogs, as on a blank chip");
        assert!(
            whole.device_time > Cycles(3 * clock::SECOND),
            "into the fourth life: {:?}",
            whole.device_time
        );
        assert!(whole.trace_hash.is_some());
        assert!(
            whole.console_bytes > 7_000,
            "a byte a tick for three seconds: {} bytes",
            whole.console_bytes
        );
        // The journal is the resets and nothing else, each with the place it found the CPU at:
        // asleep after the `wfi` at 1.3 s and at 2.94 s, executing its busy loop at 2.6 s.
        let firmware = a_ticking_firmware_under_the_reset_watchdogs();
        let resets: Vec<(u32, &str)> = whole
            .journal
            .iter()
            .map(|entry| match entry {
                Diagnostic::Reset { cause, pc, .. } if *pc == firmware.after_the_wfi => {
                    (cause.code(), "asleep")
                }
                Diagnostic::Reset { cause, pc, .. } if firmware.busy_loop.contains(pc) => {
                    (cause.code(), "executing")
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            resets,
            [(0x07, "asleep"), (0x07, "executing"), (0x10, "asleep")],
            "{:?}",
            whole.journal
        );
        assert_eq!(
            whole.repeated,
            [],
            "no reset found the CPU where another had"
        );
        for slice in [1, 7, 1_000] {
            assert_eq!(
                tick_in_calls_of(slice, TOTAL),
                whole,
                "calls of {slice} cycles"
            );
        }
    }

    #[test]
    fn a_strict_run_goes_on_across_a_reset_which_is_not_a_violation() {
        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
        let marker = load_register(MARKER);
        let mut first = stop_watchdogs();
        first.extend(store_register(MARKER, 1));
        first.push(lui(2, 0x3FC9_0000));
        first.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 5));
        first.push(HANG);
        let skip = 4 * (i32::try_from(first.len()).unwrap() + 1);
        let mut body = vec![marker[0], marker[1], cpu_rv32::asm::bne(T1, 0, skip)];
        body.extend(first);
        // The `HANG` the reset kept the first pass from reaching.
        let after_the_store = ROM_BASE + 4 * u32::try_from(body.len() - 1).unwrap();
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
        assert_eq!(
            m.drain_diagnostics(),
            [Diagnostic::Reset {
                cause: ResetCause::SoftwareCpu,
                pc: after_the_store,
                sp: 0x3FC9_0000,
                // The few dozen cycles of 20 MHz before the store: a microsecond, rounded down.
                at_us: 1,
                woken_by: None,
            }],
            "the reset is in the journal, with the PC and the stack it found, and nothing else is"
        );
    }

    #[test]
    fn a_boot_loop_is_one_entry_of_the_journal_per_reset_each_at_its_device_time() {
        // `wfi` under the watchdogs of the reset values: main watchdog 0 at 1.3 s and at 2.6 s,
        // the RTC watchdog at 2.94 s, each time with the CPU asleep after the same `wfi`.
        let mut m = machine(&[wfi(), HANG]);
        run_to_the_time_limit(&mut m, 3 * SECOND_NS, |_| NO_CYCLE_LIMIT);
        assert_eq!(m.resets(), 3);
        let by = |cause, at_us| Diagnostic::Reset {
            cause,
            pc: ROM_BASE + 4,
            sp: 0,
            at_us,
            woken_by: None,
        };
        // 2 941 176 us: 400 000 cycles of the 136 kHz slow clock, rounded down.
        assert_eq!(
            m.drain_diagnostics(),
            [
                by(ResetCause::Mwdt0Core, 1_300_000),
                by(ResetCause::Mwdt0Core, 2_600_000),
                by(ResetCause::RtcWatchdogSystem, 2_941_176),
            ],
            "spec §7: every reset is an entry, in the order they came"
        );
        assert_eq!(m.most_repeated_diagnostics(4), [], "none is a repetition");
        assert_eq!(m.unlisted_resets(), 0);
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

    #[test]
    fn the_stack_pointer_reported_is_the_one_before_a_load_into_it_that_faults() {
        // `lw sp, 0(t0)` from an address nothing answers at: the one instruction whose report
        // could show another stack pointer than the one it started with. It shows that one,
        // and the handler finds it too: a faulting instruction writes no register.
        const NOWHERE: u32 = 0x1000_0000;
        const STACK: u32 = 0x3FC9_0000;
        let mut m = machine(&[lui(2, STACK), lui(T0, NOWHERE), lw(2, T0, 0), HANG]);
        // Three instructions, the trap of the third included: the handler address is `mtvec`, 0.
        step_to(&mut m, 0, 3);
        assert_eq!(
            m.drain_diagnostics(),
            [Diagnostic::Exception {
                cause: Exception::LoadAccessFault,
                tval: NOWHERE,
                pc: ROM_BASE + 8,
                sp: STACK,
            }]
        );
        assert_eq!(
            m.hart.x[2], STACK,
            "the load did not happen: the register is as the instruction found it"
        );
    }

    #[test]
    fn a_clock_change_reaches_a_watchdog_that_counts_the_apb_clock() {
        // Main watchdog 0 counts 26 000 000 cycles of the APB clock (decision E26): 1.3 s at the
        // 20 MHz of the power-on clock. The program selects the crystal undivided (40 MHz) and
        // sleeps: the watchdog counts twice as fast from the write, and resets the chip after
        // 0.65 s plus 25 ns for each count (50 ns) it had already counted at the old rate: the
        // four instructions of the store, 100 ns. A core reset returns the clock to its power-on
        // value, so the second life does the same, and the second reset is at 1.3 s plus 200 ns.
        // Without the delivery of the clock change to the groups the watchdog would go on at
        // 20 MHz, reset at 1.3 s, and the call would return after that first reset.
        let mut body = store_register(reg("SYSTEM", "SYSCLK_CONF"), 0).to_vec();
        body.extend([wfi(), HANG]);
        let mut m = machine(&body);
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
        assert_eq!(
            m.resets(),
            2,
            "two sleeps of 0.65 s pass the second a call may sleep"
        );
        assert_eq!(m.device_time_nanos(), 1_300_000_200);
    }

    #[test]
    fn a_wake_up_at_the_end_of_device_time_does_not_hang_the_host() {
        // T0 of group 1 counts 20 MHz / 65 536 (divider field 0): one count is 2^19 cycles of the
        // time base. An alarm 2^45 - 2 counts ahead is due 2^64 - 2^20 cycles after the timer
        // started, 2^20 cycles short of the end of device time. One `wfi` sleeps there; the
        // watchdog the program starts next would fall past the end, where every instant
        // saturates to `u64::MAX` and a wake-up is due for ever. Such a wake-up never comes.
        let mut body = stop_watchdogs();
        body.extend(route("TG1_T0_INT_MAP", 4));
        body.extend(store_register(reg("TIMG1", "INT_ENA_TIMERS"), 1));
        body.extend(store_register(reg("TIMG1", "T0ALARMLO"), 0xFFFF_FFFE));
        body.extend(store_register(reg("TIMG1", "T0ALARMHI"), 0x1FFF));
        // `EN`, `INCREASE`, `ALARM_EN`.
        body.extend(store_register(reg("TIMG1", "T0CONFIG"), 0xC000_0400));
        body.push(wfi());
        // `EN` and `USE_XTAL`: 26 000 000 counts of the crystal, 104 million cycles.
        body.extend(store_register(reg("TIMG1", "WDTCONFIG0"), 0x8020_0000));
        body.push(HANG);
        // On a thread with a limit: if the delivery pass were not bounded the loop would never
        // leave one instruction, and nothing in the run could stop it. The limit only turns a
        // hang into a failure, so it is wide: the test takes about 1.3 s instrumented by
        // `cargo llvm-cov` beside a release build and 16 spinning processes on 16 cores, and the
        // limit is ten minutes, under the timeout of the CI job that runs it.
        let (calls, resets, now) = within(
            std::time::Duration::from_mins(10),
            "the run did not return: a delivery pass is not bounded",
            move || {
                let mut m = machine(&body);
                let mut calls = 0;
                // 300 000 CPU cycles are 2.4 million cycles of the time base at 20 MHz: they
                // cover the 2^20 left after the alarm.
                while m.run_until(Cycles(300_000)) != StopReason::CycleBudget && calls < 10 {
                    calls += 1;
                }
                (calls, m.resets(), m.bus.ctx.now())
            },
        );
        assert!(calls < 10, "run_until keeps returning without the budget");
        assert_eq!(resets, 0, "no watchdog fell inside device time");
        assert_eq!(
            now,
            Cycles(u64::MAX),
            "the CPU ran on to the end of device time, where it saturates"
        );
    }

    #[test]
    fn the_machine_journals_the_inputs_it_accepts_and_not_those_it_refuses() {
        use crate::inputs::AcceptedInput;
        let config = SocConfig {
            usb_host: UsbHost::Attached,
            ..SocConfig::default()
        };
        let mut m = machine_with(&[HANG], config);
        assert_eq!(m.input_journal().usb_host_at_power_on(), UsbHost::Attached);
        m.send_console(ConsoleChannel::Uart1, Cycles(40), b"hi")
            .unwrap();
        m.set_usb_host(Cycles(80), UsbHost::Open).unwrap();
        m.run_until(Cycles(100));
        let now = m.device_time();
        assert!(now > Cycles(80));
        // In the past, beyond the line of a UART, too much pending: refused, and not journaled.
        assert!(
            m.send_console(ConsoleChannel::Uart0, Cycles(0), b"x")
                .is_err()
        );
        assert!(m.set_usb_host(Cycles(0), UsbHost::Absent).is_err());
        let beyond = Cycles(crate::periph::uart::LAST_INPUT_DATE.get() + 1);
        assert!(m.send_console(ConsoleChannel::Uart0, beyond, b"x").is_err());
        let too_much = vec![0u8; MAX_PENDING_HOST_BYTES + 1];
        assert!(
            m.send_console(ConsoleChannel::UsbSerialJtag, now, &too_much)
                .is_err()
        );
        m.send_console(ConsoleChannel::UsbSerialJtag, now, b"")
            .unwrap();
        assert_eq!(
            m.input_journal().inputs(),
            [
                AcceptedInput::Console {
                    channel: ConsoleChannel::Uart1,
                    at: Cycles(40),
                    bytes: b"hi".to_vec()
                },
                AcceptedInput::UsbHost {
                    at: Cycles(80),
                    state: UsbHost::Open
                },
                AcceptedInput::Console {
                    channel: ConsoleChannel::UsbSerialJtag,
                    at: now,
                    bytes: Vec::new()
                },
            ]
        );
        assert!(m.input_journal().is_complete());
        // A machine told to keep no journal lets it go and still takes its inputs.
        m.forgo_input_journal();
        let pending = m.input_pending(ConsoleChannel::Uart1);
        m.send_console(ConsoleChannel::Uart1, now, b"!").unwrap();
        assert_eq!(m.input_pending(ConsoleChannel::Uart1), pending + 1);
        assert!(m.input_journal().is_forgone());
        assert_eq!(m.input_journal().inputs(), []);
    }

    /// The program of `a_software_interrupt_is_taken_four_cpu_cycles_after_it_becomes_pending`:
    /// software interrupt 0 routed to line 3, `MIE` set, the source raised. Returns it with the
    /// address of the store that raises the source.
    fn software_interrupt_program() -> (Vec<u32>, u32) {
        let mut body = Vec::new();
        body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
        body.extend(route("CPU_INTR_FROM_CPU_0_MAP", 3));
        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
        body.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
        let store = ROM_BASE + 4 * u32::try_from(body.len()).unwrap() - 4;
        body.extend([NOP; 16]);
        (with_vectors(&body), store)
    }

    fn qemu_icount() -> SocConfig {
        SocConfig {
            timing: Timing::QemuIcount,
            ..SocConfig::default()
        }
    }

    /// A one-shot SYSTIMER alarm `counts` ahead (16 MHz), its interrupt routed to line 4, `MIE`
    /// set, then a `wfi`; the vector table parks the handler. Returns it with the `wfi`'s address.
    fn alarm_and_wfi_program(counts: u32) -> (Vec<u32>, u32) {
        let mut body = stop_watchdogs();
        body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
        body.extend(route("SYSTIMER_TARGET0_INT_MAP", 4));
        body.extend(store_register(reg("SYSTIMER", "INT_ENA"), 1));
        body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), counts));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        body.extend(store_register(
            reg("SYSTIMER", "CONF"),
            0x4600_0000 | (1 << 24),
        ));
        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
        let at_wfi = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
        body.extend([wfi(), HANG]);
        (with_vectors(&body), at_wfi)
    }

    #[test]
    fn stepping_runs_the_same_instructions_as_a_run() {
        // Two programs (an interrupt raised by a store, an alarm slept to through a `wfi`) under
        // both timing models: a run and steps over the same instructions leave the same machine.
        // Both programs end parked in a handler, so the cycle limit falls where no interrupt is
        // left to take.
        let programs = [
            software_interrupt_program().0,
            alarm_and_wfi_program(16_000).0,
        ];
        for timing in [Timing::Datasheet, Timing::QemuIcount] {
            for program in &programs {
                let traced = SocConfig {
                    trace_hash: true,
                    timing,
                    ..SocConfig::default()
                };
                let mut run = machine_with(program, traced);
                assert_eq!(run.run_until(Cycles(2_000)), StopReason::CycleBudget);
                let mut stepped = machine_with(program, traced);
                while stepped.instructions() < run.instructions() {
                    assert_eq!(stepped.step().stop, None);
                }
                assert_eq!(
                    (
                        stepped.pc(),
                        stepped.x(),
                        stepped.cycles(),
                        stepped.device_time_nanos(),
                        stepped.trace_hash(),
                        stepped.resets(),
                        stepped.csr(0x342),
                    ),
                    (
                        run.pc(),
                        run.x(),
                        run.cycles(),
                        run.device_time_nanos(),
                        run.trace_hash(),
                        run.resets(),
                        run.csr(0x342),
                    ),
                    "{timing:?}"
                );
                assert!(
                    matches!(stepped.csr(0x342), Some(0x8000_0003 | 0x8000_0004)),
                    "the interrupt was taken"
                );
            }
        }
    }

    #[test]
    fn stepping_goes_through_deep_sleeps_and_their_wakes_as_a_run_does() {
        // Three lives woken by the RTC timer from deep sleep, then a sleep nothing ends: steps
        // over the whole of it leave the machine a run leaves, under both timing models.
        const MAX_STEPS: u32 = 1_000_000;
        let program = four_lives_waiting(true);
        for timing in [Timing::Datasheet, Timing::QemuIcount] {
            let traced = SocConfig {
                trace_hash: true,
                timing,
                ..SocConfig::default()
            };
            let mut run = machine_with(&program, traced);
            while run.run_within(Cycles(u64::MAX), Some(1_000_000_000)) != StopReason::DeepSleep {}
            let mut stepped = machine_with(&program, traced);
            let mut steps = 0;
            loop {
                match stepped.step().stop {
                    None | Some(StopReason::Asleep) => {}
                    Some(StopReason::DeepSleep) => break,
                    Some(other) => panic!("{timing:?}: {other:?}"),
                }
                steps += 1;
                assert!(
                    steps < MAX_STEPS,
                    "{timing:?}: the sleep without end was not reached"
                );
            }
            let state = |m: &Esp32C3| {
                (
                    m.pc(),
                    m.x(),
                    m.cycles(),
                    m.instructions(),
                    m.device_time_nanos(),
                    m.trace_hash(),
                    m.resets(),
                )
            };
            assert_eq!(state(&stepped), state(&run), "{timing:?}");
            assert_eq!(stepped.resets(), 3, "three wakes");
        }
    }

    #[test]
    fn a_step_cut_by_a_long_sleep_resumes_at_its_boundary() {
        // An alarm 1.5 s ahead: the sleep passes the second after which a call returns `Asleep`.
        let (program, at_wfi) = alarm_and_wfi_program(24_000_000);
        let mut m = machine_with(&program, qemu_icount());
        let first = loop {
            let stepped = m.step();
            if stepped.executed.as_ref().is_some_and(|e| e.pc == at_wfi) {
                break stepped;
            }
            assert_eq!(stepped.stop, None);
        };
        assert_eq!(first.stop, Some(StopReason::Asleep));
        assert_eq!(first.interrupt, None, "the boundary is not reached yet");
        // The next call executes nothing: it goes on to that boundary and takes the interrupt.
        let second = m.step();
        assert_eq!(second.executed, None);
        assert_eq!(second.interrupt, Some(4));
        assert_eq!(second.stop, None);
        assert_eq!(m.pc(), VECTORS + 4 * 4);
        assert!(m.device_time_nanos() >= 1_500_000_000);
        // And the one after executes the handler's first instruction.
        let third = m.step();
        assert_eq!(third.executed.map(|e| e.pc), Some(VECTORS + 4 * 4));
    }

    #[test]
    fn under_qemu_icount_a_faulting_access_costs_nothing_and_an_ecall_8_ns() {
        // `lw` from address 0, which nothing maps, then `ecall`; the trap handler is parked.
        let mut body = vec![lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)];
        body.extend([cpu_rv32::asm::lw(6, 0, 0), 0x0000_0073]);
        let mut m = machine_with(&with_vectors(&body), qemu_icount());
        m.step();
        m.step();
        let before_load = m.device_time_nanos();
        let load = m.step();
        assert!(load.executed.unwrap().trap.is_some(), "the load faults");
        assert_eq!(
            m.device_time_nanos(),
            before_load,
            "QEMU gives the faulting instruction back to its counter"
        );
        // The handler's instruction, then back: execute the `ecall` from the same state.
        let mut body = vec![lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)];
        body.push(0x0000_0073);
        let mut m = machine_with(&with_vectors(&body), qemu_icount());
        m.step();
        m.step();
        let before_ecall = m.device_time_nanos();
        let ecall = m.step();
        assert!(ecall.executed.unwrap().trap.is_some());
        assert!(m.device_time_nanos() > before_ecall, "an ecall is counted");
    }

    #[test]
    fn the_cycle_counter_counts_the_time_base_when_asked() {
        // The counter enabled (event "cycles", mode "on"), then two reads of `mpccr` (0x802) 25
        // instructions apart: on the time base it counts the 6.25 ns cycles of device time, 32
        // for 25 instructions of 8 ns; without the setting, the timing model's CPU cycles.
        let mut program = vec![
            cpu_rv32::asm::csrrwi(0, 0x800, 1),
            cpu_rv32::asm::csrrwi(0, 0x801, 1),
            cpu_rv32::asm::csrrs(6, 0x802, 0),
        ];
        program.extend([NOP; 24]);
        program.extend([cpu_rv32::asm::csrrs(7, 0x802, 0), HANG]);
        let run = |config: SocConfig| {
            let mut m = machine_with(&program, config);
            for _ in 0..28 {
                m.step();
            }
            (m.x()[7].wrapping_sub(m.x()[6]), m.compat_acts())
        };
        let on_time_base = SocConfig {
            qemu_compat: crate::QemuCompat {
                cycle_counter_on_time_base: true,
                ..crate::QemuCompat::default()
            },
            ..qemu_icount()
        };
        let (elapsed, acts) = run(on_time_base);
        assert_eq!(elapsed, 32);
        assert_eq!(acts.cycle_counter_reads, 2);
        let (elapsed, acts) = run(qemu_icount());
        assert_eq!(
            elapsed, 25,
            "one CPU cycle per instruction under qemu-icount"
        );
        assert_eq!(acts, crate::CompatActs::default());
    }

    #[test]
    fn a_step_takes_the_interrupt_due_after_its_instruction() {
        let (program, store) = software_interrupt_program();
        // `qemu-icount` has no latency: the line the store raises is taken right after it.
        let mut m = machine_with(&program, qemu_icount());
        let stepped = loop {
            let stepped = m.step();
            assert_eq!(stepped.stop, None);
            if stepped.executed.as_ref().is_some_and(|e| e.pc == store) {
                break stepped;
            }
        };
        let executed = stepped.executed.unwrap();
        assert_eq!(executed.trap, None);
        assert!(matches!(
            executed.instruction.unwrap().instr,
            cpu_rv32::Instr::Store { .. }
        ));
        assert_eq!(stepped.interrupt, Some(3));
        assert!(!stepped.slept);
        assert_eq!(
            m.pc(),
            VECTORS + 4 * 3,
            "the handler is the next instruction"
        );
        assert_eq!(m.csr(0x341), Some(store + 4), "mepc");
        // With the TRM's latency of 4 cycles, the line is not visible yet after the store.
        let mut m = machine(&program);
        let stepped = loop {
            let stepped = m.step();
            if stepped.executed.as_ref().is_some_and(|e| e.pc == store) {
                break stepped;
            }
        };
        assert_eq!(stepped.interrupt, None);
    }

    #[test]
    fn a_step_sleeps_through_a_wfi_to_the_interrupt_that_ends_it() {
        // A one-shot alarm 1 ms ahead (16 000 counts at 16 MHz), its interrupt routed to line 4.
        let mut body = stop_watchdogs();
        body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
        body.extend(route("SYSTIMER_TARGET0_INT_MAP", 4));
        body.extend(store_register(reg("SYSTIMER", "INT_ENA"), 1));
        body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), 16_000));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        body.extend(store_register(
            reg("SYSTIMER", "CONF"),
            0x4600_0000 | (1 << 24),
        ));
        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
        let at_wfi = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
        body.extend([wfi(), HANG]);
        let mut m = machine_with(&with_vectors(&body), qemu_icount());
        let stepped = loop {
            let stepped = m.step();
            assert_eq!(stepped.stop, None);
            if stepped.executed.as_ref().is_some_and(|e| e.pc == at_wfi) {
                break stepped;
            }
        };
        assert!(stepped.slept);
        assert_eq!(stepped.interrupt, Some(4));
        assert_eq!(m.pc(), VECTORS + 4 * 4);
        assert!(m.device_time_nanos() >= 1_000_000, "slept to the alarm");
        // A wfi nothing can end stops the step that executed it, once the watchdogs the reset
        // values arm are stopped.
        let mut body = stop_watchdogs();
        let at_wfi = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
        body.extend([wfi(), HANG]);
        let mut idle = machine_with(&body, qemu_icount());
        let stepped = loop {
            let stepped = idle.step();
            if stepped.stop.is_some() {
                break stepped;
            }
        };
        assert!(stepped.slept);
        assert_eq!(stepped.stop, Some(StopReason::Idle));
        assert_eq!(stepped.executed.map(|e| e.pc), Some(at_wfi));
    }

    #[test]
    fn registers_and_csrs_are_read_and_an_integer_register_written() {
        let mut m = machine(&[lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0), HANG]);
        m.step();
        m.step();
        assert_eq!(m.x()[usize::from(T0)], VECTORS);
        assert_eq!(m.csr(0x305), Some(VECTORS | 1), "mtvec, vectored");
        assert_eq!(m.csr(0x7FF), None, "no such CSR");
        m.set_x(7, 0xDEAD_BEEF);
        m.set_x(0, 1);
        m.set_x(32, 1);
        assert_eq!((m.x()[7], m.x()[0]), (0xDEAD_BEEF, 0));
    }

    #[test]
    fn qemu_icount_prices_25_instructions_at_200_ns_whatever_the_clock() {
        let to_pll_160 = [
            store_register(reg("SYSTEM", "CPU_PER_CONF"), 1),
            store_register(reg("SYSTEM", "SYSCLK_CONF"), 1 << 10),
        ]
        .concat();
        let mut program = vec![NOP; 25];
        program.extend(&to_pll_160);
        program.extend([NOP; 25]);
        program.push(HANG);
        let mut m = machine_with(&program, qemu_icount());
        assert_eq!(m.timing_model().name(), "qemu-icount");
        for _ in 0..25 {
            m.step();
        }
        assert_eq!(m.device_time_nanos(), 200, "at 20 MHz");
        assert_eq!(m.cpu_frequency().hz(), 20_000_000);
        for _ in 0..to_pll_160.len() {
            m.step();
        }
        assert_eq!(m.cpu_frequency().hz(), 160_000_000);
        let before = m.device_time_nanos();
        for _ in 0..25 {
            m.step();
        }
        assert_eq!(m.device_time_nanos() - before, 200, "at 160 MHz");
        // The datasheet model prices the same 25 instructions in cycles of the 20 MHz clock.
        let mut datasheet = machine(&program);
        for _ in 0..25 {
            datasheet.step();
        }
        assert_eq!(datasheet.device_time_nanos(), 1_250);
    }

    /// A chip that holds a pin at a level the test sets, and records what the `SoC` drives on
    /// another.
    #[derive(Debug, Default)]
    struct Pins {
        driven_low: bool,
        heard: Vec<Option<emu_core::Level>>,
    }

    impl GpioInput for Pins {
        fn level(&mut self, _at: emu_core::Nanos) -> Option<emu_core::Level> {
            Some(emu_core::Level::from_bool(!self.driven_low))
        }
    }

    impl GpioOutput for Pins {
        fn driven(&mut self, level: Option<emu_core::Level>, _at: emu_core::Nanos) {
            self.heard.push(level);
        }
    }

    #[test]
    fn chips_attached_to_the_machine_drive_and_hear_its_pins() {
        use std::cell::RefCell;
        use std::rc::Rc;
        const GPIO_PAD: u32 = (1 << 12) | (1 << 9);
        let gpio = |name| svd_regs::esp32c3::addr("GPIO", name);
        let mut m = machine(&[HANG]);
        let chip = Rc::new(RefCell::new(Pins {
            driven_low: true,
            ..Pins::default()
        }));
        m.attach_input(3, Box::new(Rc::clone(&chip))).unwrap();
        m.attach_output(4, Box::new(Rc::clone(&chip))).unwrap();
        for pad in ["GPIO3", "GPIO4"] {
            let at = svd_regs::esp32c3::addr("IO_MUX", pad);
            m.bus.write(at, AccessSize::Word, GPIO_PAD).unwrap();
        }
        assert_eq!(bus_word(&mut m, gpio("IN")) & (1 << 3), 0, "held low");
        // Any edge of GPIO3 interrupts the CPU (`INT_TYPE` 3, `INT_ENA` bit 13).
        let pin3 = gpio("PIN3");
        m.bus
            .write(pin3, AccessSize::Word, (3 << 7) | (1 << 13))
            .unwrap();
        chip.borrow_mut().driven_low = false;
        assert_eq!(
            m.bus.ctx.sources() & (1 << 16),
            0,
            "nothing has read the pins since the chip changed"
        );
        m.inputs_changed();
        assert_eq!(
            m.bus.ctx.sources() & (1 << 16),
            1 << 16,
            "the change is seen at once, its edge raises the GPIO interrupt"
        );
        assert_eq!(bus_word(&mut m, gpio("IN")) & (1 << 3), 1 << 3);
        assert_eq!(bus_word(&mut m, gpio("STATUS")) & (1 << 3), 1 << 3);
        // GPIO4 without its pull-up and not yet an output floats: nothing drives it.
        for (register, level) in [
            ("OUT_W1TC", None),
            ("ENABLE_W1TS", Some(emu_core::Level::Low)),
            ("OUT_W1TS", Some(emu_core::Level::High)),
        ] {
            m.bus
                .write(gpio(register), AccessSize::Word, 1 << 4)
                .unwrap();
            assert_eq!(chip.borrow().heard.last(), Some(&level), "{register}");
        }
        assert!(matches!(
            m.attach_input(3, Box::new(Rc::clone(&chip))),
            Err(AttachError::InputTaken { pin: 3 })
        ));
        assert!(matches!(
            m.attach_input(22, Box::new(Rc::clone(&chip))),
            Err(AttachError::NoSuchPin { pin: 22 })
        ));
        assert_eq!(m.drain_diagnostics(), []);
    }

    /// A chip that has work at the instants of `deadlines`, and records when it is advanced.
    #[derive(Debug, Default)]
    struct Refresh {
        deadlines: Vec<u64>,
        advanced: Vec<u64>,
        /// Answers `at` itself to every question: work always due, which must not hold the
        /// machine at one instant.
        always_due: bool,
    }

    impl Clocked for Refresh {
        fn next_deadline(&mut self, at: emu_core::Nanos) -> Option<emu_core::Nanos> {
            if self.always_due {
                return Some(at);
            }
            self.deadlines.first().copied().map(emu_core::Nanos)
        }

        fn advance(&mut self, at: emu_core::Nanos) {
            self.advanced.push(at.get());
            self.deadlines.retain(|&deadline| deadline > at.get());
        }
    }

    /// No watchdog at reset (another boot mode): only the chip's deadlines can wake the CPU.
    fn quiet_machine(program: &[u32]) -> Esp32C3 {
        machine_with(
            program,
            SocConfig {
                strapping: 0,
                ..SocConfig::default()
            },
        )
    }

    #[test]
    fn a_clocked_chip_is_advanced_at_its_deadlines_while_the_cpu_waits_for_an_interrupt() {
        use std::cell::RefCell;
        use std::rc::Rc;
        // Instants on cycles of the time base (6.25 ns), so that the wake-up is exactly there.
        let chip = Rc::new(RefCell::new(Refresh {
            deadlines: vec![1_000_000, 2_500_000],
            ..Refresh::default()
        }));
        let mut m = quiet_machine(&[wfi(), HANG]);
        m.attach_clocked(Box::new(Rc::clone(&chip)));
        assert_eq!(
            m.run_until(Cycles(10_000)),
            StopReason::Idle,
            "nothing left to wake the CPU once the chip has no deadline"
        );
        assert_eq!(chip.borrow().advanced, [1_000_000, 2_500_000]);
        assert_eq!(m.device_time_nanos(), 2_500_000, "time stopped at the last");
        assert_eq!(m.cycles().get(), 3, "only `wfi` was executed");
        assert_eq!(m.drain_diagnostics(), []);
    }

    /// A chip with one deadline at 1 ms, attached to `m`.
    fn refresh_at_one_millisecond(m: &mut Esp32C3) -> std::rc::Rc<std::cell::RefCell<Refresh>> {
        let chip = std::rc::Rc::new(std::cell::RefCell::new(Refresh {
            deadlines: vec![1_000_000],
            ..Refresh::default()
        }));
        m.attach_clocked(Box::new(std::rc::Rc::clone(&chip)));
        chip
    }

    /// Code that enters a deep sleep with no wake source (what `esp_deep_sleep_start` does with
    /// none enabled), then parks.
    fn sleep_for_ever() -> Vec<u32> {
        let mut program = store_register(rtc_cntl("WAKEUP_STATE"), 0).to_vec();
        program.extend(store_register(rtc_cntl("DIG_PWC"), 1 << 31));
        program.extend(store_register(rtc_cntl("STATE0"), 1 << 31));
        program.push(HANG);
        program
    }

    #[test]
    fn a_clocked_chip_is_advanced_at_its_deadline_during_a_deep_sleep_nothing_wakes() {
        let mut m = machine(&sleep_for_ever());
        let chip = refresh_at_one_millisecond(&mut m);
        let mut stop = StopReason::CycleBudget;
        for _ in 0..100 {
            stop = m.run_within(Cycles(u64::MAX), Some(1_000_000_000));
            if stop == StopReason::DeepSleep {
                break;
            }
        }
        assert_eq!(stop, StopReason::DeepSleep, "the sleep ends the run");
        assert_eq!(chip.borrow().advanced, [1_000_000], "at its deadline");
        assert_eq!(
            m.device_time_nanos(),
            1_000_000,
            "nothing is due once the chip has no deadline"
        );
        assert!(m.deep_sleep().is_some(), "the chip still sleeps");
    }

    #[test]
    fn a_clocked_chip_is_advanced_at_its_deadline_during_a_deep_sleep_the_rtc_timer_ends() {
        // 0x2000 counts of the slow clock: tens of milliseconds, far beyond the chip's 1 ms.
        let mut m = machine(&sleep_on_the_rtc_timer(0x2000, TIMER_WAKE_STATE));
        let chip = refresh_at_one_millisecond(&mut m);
        for _ in 0..100 {
            if m.device_time_nanos() >= 5_000_000 {
                break;
            }
            m.run_within(Cycles(u64::MAX), Some(5_000_000));
        }
        assert_eq!(m.device_time_nanos(), 5_000_000);
        assert!(m.deep_sleep().is_some(), "the RTC timer has not come yet");
        assert_eq!(chip.borrow().advanced, [1_000_000], "at its deadline");
        assert_eq!(m.resets(), 0);
    }

    #[test]
    fn a_clocked_chip_is_advanced_at_its_deadline_across_a_core_reset() {
        // The first pass marks SRAM and resets the core; the second waits in `wfi`.
        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
        let mut first = store_register(MARKER, 1).to_vec();
        first.extend(store_register(
            svd_regs::esp32c3::addr("RTC_CNTL", "OPTIONS0"),
            1 << 31,
        ));
        first.push(HANG);
        let skip = 4 * (i32::try_from(first.len()).unwrap() + 1);
        let marker = load_register(MARKER);
        let mut body = vec![marker[0], marker[1], bne(T1, 0, skip)];
        body.extend(first);
        body.extend([wfi(), HANG]);
        let mut m = quiet_machine(&body);
        let chip = refresh_at_one_millisecond(&mut m);
        assert_eq!(m.run_until(Cycles(100_000)), StopReason::Idle);
        assert_eq!(m.resets(), 1, "the core reset");
        assert_eq!(
            chip.borrow().advanced,
            [1_000_000],
            "the board's chip went on"
        );
        assert_eq!(m.device_time_nanos(), 1_000_000);
    }

    /// `m` run until `stop` returns true or the stop is not `Asleep`, a hundred calls at most,
    /// each to device time `limit`: the last stop.
    fn run_to(m: &mut Esp32C3, limit: u64, stop: impl Fn(&Esp32C3) -> bool) -> StopReason {
        let mut reason = StopReason::CycleBudget;
        for _ in 0..100 {
            reason = m.run_within(Cycles(u64::MAX), Some(limit));
            if stop(m) || reason != StopReason::Asleep {
                break;
            }
        }
        reason
    }

    #[test]
    fn a_deep_sleep_with_a_chip_always_due_ends_only_at_the_limit_of_device_time() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let mut m = machine(&sleep_for_ever());
        let chip = Rc::new(RefCell::new(Refresh {
            always_due: true,
            ..Refresh::default()
        }));
        m.attach_clocked(Box::new(Rc::clone(&chip)));
        assert_eq!(
            run_to(&mut m, 20_000, |_| false),
            StopReason::TimeBudget,
            "never DeepSleep: the chip always has work"
        );
        assert_eq!(m.device_time_nanos(), 20_000);
        assert!(m.deep_sleep().is_some());
        assert_eq!(chip.borrow().advanced.last(), Some(&20_000));
    }

    #[test]
    fn a_chip_always_due_is_named_once_as_stuck() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let mut m = quiet_machine(&[wfi(), HANG]);
        let chip = Rc::new(RefCell::new(Refresh {
            always_due: true,
            ..Refresh::default()
        }));
        m.attach_clocked(Box::new(Rc::clone(&chip)));
        // A thousand advances take 6.25 µs of waiting; 20 µs is three times that.
        assert_eq!(
            m.run_within(Cycles(10_000), Some(20_000)),
            StopReason::TimeBudget
        );
        assert!(chip.borrow().advanced.len() > 2 * crate::diag::STUCK_CLOCKED_INSTANTS as usize);
        assert_eq!(
            m.drain_diagnostics(),
            [Diagnostic::ClockedChipStuck { chip: 0 }],
            "once, though it stayed stuck"
        );
        // A chip that progresses is never named (the other tests of clocked chips drain none).
    }

    #[test]
    fn a_deadline_at_the_instant_of_the_rtc_call_advances_the_chip_once_then_the_wake_resets() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let mut m = machine(&sleep_on_the_rtc_timer(0x2000, TIMER_WAKE_STATE));
        run_to(&mut m, 1_000_000, |m| m.deep_sleep().is_some());
        let wake = m
            .deep_sleep()
            .and_then(|sleep| sleep.timer_wake_nanos)
            .expect("asleep, the RTC timer armed");
        let chip = Rc::new(RefCell::new(Refresh {
            deadlines: vec![wake],
            ..Refresh::default()
        }));
        m.attach_clocked(Box::new(Rc::clone(&chip)));
        run_to(&mut m, wake + 1_000_000, |m| m.resets() == 1);
        assert_eq!(m.resets(), 1, "the wake from deep sleep");
        assert_eq!(
            chip.borrow().advanced,
            [wake],
            "once, at the wake's instant"
        );
        let reset_at = m.drain_diagnostics().iter().find_map(|d| match d {
            Diagnostic::Reset { at_us, .. } => Some(*at_us),
            _ => None,
        });
        assert_eq!(
            reset_at,
            Some(wake / 1_000),
            "the reset at the same instant"
        );
    }

    #[test]
    fn a_clocked_chip_always_due_is_advanced_once_per_instant_and_again_at_the_next() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let chip = Rc::new(RefCell::new(Refresh {
            always_due: true,
            ..Refresh::default()
        }));
        let mut m = quiet_machine(&[addi(1, 0, 1), addi(1, 1, 1), wfi(), HANG]);
        m.attach_clocked(Box::new(Rc::clone(&chip)));
        // 300 ns of device time: the two `addi` take 100 ns, the `wfi` 150 (three cycles of the
        // 20 MHz clock), then the CPU waits to the end.
        assert_eq!(
            m.run_within(Cycles(10_000), Some(300)),
            StopReason::TimeBudget
        );
        let advanced = chip.borrow().advanced.clone();
        assert!(
            advanced.windows(2).all(|pair| pair[0] < pair[1]),
            "once per instant, in order of time: {advanced:?}"
        );
        // While the CPU runs, at the end of each instruction at the latest; while it waits, at
        // each cycle of the time base (6.25 ns), none skipped.
        assert_eq!(
            advanced,
            [0, 50, 100, 250, 256, 262, 268, 275, 281, 287, 293, 300]
        );
        assert_eq!(m.cycles().get(), 5, "the two `addi` and the `wfi` ran");
    }
}
