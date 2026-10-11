//! What a peripheral reaches beyond its own registers: the one door between a device model and the
//! rest of the machine (ADR 0003).
use crate::clock::CpuClock;
use crate::diag::{Diagnostic, Diagnostics, Unsupported};
use crate::periph::INTMATRIX_BASE;
use crate::reset::ResetCause;
use emu_core::{Cycles, EventId, Scheduler};
use std::collections::VecDeque;
use svd_regs::REGISTER_BYTES;

/// The number of interrupt map registers, hence of peripheral interrupt sources. They fill the
/// start of `INTERRUPT_CORE0` (report IR §1.1) from `MAC_INTR_MAP`, at the base of the block, to
/// `CACHE_CORE0_ACS_INT_MAP`, the last one; the registers that follow (`INTR_STATUS_REG_0`, ...)
/// are not map registers. The count is read off the SVD table through that last register.
pub(crate) const MAP_REGISTERS: u32 =
    (svd_regs::esp32c3::addr("INTERRUPT_CORE0", "CACHE_CORE0_ACS_INT_MAP") - INTMATRIX_BASE)
        / REGISTER_BYTES
        + 1;

/// One of the 62 peripheral interrupt sources: source `n` is the one `INTERRUPT_CORE0` routes with
/// the map register at offset 4 × `n` (report IR §1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source(u32);

impl Source {
    /// The source routed by `INTERRUPT_CORE0.<map_register>`. Meant for a `const`, where the build
    /// fails when the SVD has no such register in `INTERRUPT_CORE0`, and when the register is one of
    /// the block that is not an interrupt map register (`INTR_STATUS_REG_0`, `CLOCK_GATE`, ...).
    ///
    /// The check is on the position: a register inside the first [`MAP_REGISTERS`] words of the
    /// block is taken to be a map register, as the SVD lays them out (a test pins that).
    ///
    /// # Panics
    /// When the register is not an interrupt map register (at compile time in a `const`).
    #[must_use]
    pub const fn of(map_register: &str) -> Source {
        let offset = svd_regs::esp32c3::addr("INTERRUPT_CORE0", map_register) - INTMATRIX_BASE;
        let index = offset / REGISTER_BYTES;
        assert!(
            index < MAP_REGISTERS,
            "the name is not an INTERRUPT_CORE0 interrupt map register"
        );
        Source(index)
    }

    /// The source number, 0 to 61.
    #[must_use]
    pub const fn number(self) -> u32 {
        self.0
    }

    const fn mask(self) -> u64 {
        1 << self.0
    }
}

/// What a device is called for, besides register accesses: at the instant it asked for (a wake-up),
/// or because another part of the machine has something to tell it.
///
/// A wake-up is a call, not an order: the device works out what is due at the time it is called.
/// A wake-up that comes early, or for a state that is gone (after a reset, a feed, a reprogrammed
/// alarm), finds nothing to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// To `EXTMEM`: a flash window was accessed through an invalid MMU entry, whose content is given.
    MmuFault { vaddr: u32, content: u32 },
    /// To `EXTMEM`: a store reached a flash window.
    FlashWindowStore { instruction_bus: bool },
    /// Wake-up of the SYSTIMER: one of its comparators may have an alarm due.
    Systimer,
    /// Wake-up of UART 0 or 1: its transmitter may have gone idle.
    Uart(u8),
    /// Wake-up of `RTC_CNTL`: the RTC watchdog may have reached a stage timeout.
    RtcWatchdog,
}

/// The wake-up a device keeps for its next deadline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Deadline {
    armed: Option<(Cycles, EventId)>,
}

impl Deadline {
    /// Makes sure the device is called with `event` at `at`, or before. A call already due earlier
    /// is kept: the device re-arms when it is called.
    pub fn arm(&mut self, ctx: &mut Ctx, at: Cycles, event: Event) {
        match self.armed {
            Some((due, _)) if due <= at => {}
            previous => {
                if let Some((_, id)) = previous {
                    ctx.cancel(id);
                }
                self.armed = Some((at, ctx.schedule(at, event)));
            }
        }
    }

    /// Forgets a wake-up whose instant has come: the device is being called for it.
    pub fn called(&mut self, now: Cycles) {
        if self.armed.is_some_and(|(due, _)| due <= now) {
            self.armed = None;
        }
    }
}

/// The context of a peripheral access. The bus owns it and hands it to every register read and
/// write; the machine reads what the peripherals left in it after each instruction.
#[derive(Debug)]
pub struct Ctx {
    /// What the run has to tell the user. Reached from outside the crate through the machine.
    pub(crate) diag: Diagnostics,
    /// PC of the instruction being executed, for diagnostics.
    pc: u32,
    /// Device time: cycles of the time base ([`crate::clock::TIME_BASE`]) since power-on. It only
    /// grows, across resets too.
    now: Cycles,
    /// The CPU and APB clocks the firmware selected.
    clock: CpuClock,
    /// A clock selection register was written since the machine last looked.
    clock_written: bool,
    /// Wake-ups the devices asked for, by device time.
    events: Scheduler<Event>,
    /// Device time of the earliest wake-up, `u64::MAX` without one: the run loop compares it with
    /// the time at every instruction.
    next_wake: u64,
    /// Events to deliver before the instruction that posted them ends, oldest first.
    signals: VecDeque<Event>,
    /// Level of each interrupt source, bit `n` for source `n`.
    sources: u64,
    /// A source level or a register of the interrupt controller changed since the machine last looked.
    interrupts_changed: bool,
    /// The reset a device asked for, if any: the first request of an instruction wins.
    reset: Option<ResetCause>,
    /// Every refusal in the order it was reported, for the tests of the device models: the
    /// diagnostics list a repeated refusal once, and a device test wants to see each one.
    #[cfg(test)]
    refused: VecDeque<Unsupported>,
}

impl Ctx {
    #[must_use]
    pub fn new(strict: bool) -> Ctx {
        Ctx {
            diag: Diagnostics::new(strict),
            pc: 0,
            now: Cycles::ZERO,
            clock: CpuClock::POWER_ON,
            clock_written: false,
            events: Scheduler::new(),
            next_wake: u64::MAX,
            signals: VecDeque::new(),
            sources: 0,
            interrupts_changed: false,
            reset: None,
            #[cfg(test)]
            refused: VecDeque::new(),
        }
    }

    /// Tells the context which instruction is running, so diagnostics can name it.
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
    }

    /// PC of the instruction being executed.
    #[must_use]
    pub const fn pc(&self) -> u32 {
        self.pc
    }

    /// Device time: cycles of the time base since power-on.
    #[must_use]
    pub const fn now(&self) -> Cycles {
        self.now
    }

    /// The CPU has run `cpu_cycles` cycles of its clock: device time moves on by as much.
    pub fn advance(&mut self, cpu_cycles: u64) {
        self.now = self.now + cpu_cycles.saturating_mul(self.clock.cpu_period);
    }

    /// Device time jumps to `instant` while the CPU sleeps. Time never goes back: an instant in the
    /// past changes nothing.
    pub fn sleep_until(&mut self, instant: Cycles) {
        self.now = self.now.max(instant);
    }

    /// The CPU and APB clocks in use.
    #[must_use]
    pub const fn clock(&self) -> CpuClock {
        self.clock
    }

    /// From now on a CPU cycle and an APB cycle last what `clock` says.
    pub fn set_clock(&mut self, clock: CpuClock) {
        self.clock = clock;
    }

    /// A device tells that a clock selection register was written.
    pub fn clock_written(&mut self) {
        self.clock_written = true;
    }

    /// Whether a clock selection register was written since the last call.
    pub fn take_clock_written(&mut self) -> bool {
        std::mem::take(&mut self.clock_written)
    }

    /// Asks for `event` to be delivered at device time `at`; an instant already past is delivered
    /// when the current instruction ends.
    pub fn schedule(&mut self, at: Cycles, event: Event) -> EventId {
        self.next_wake = self.next_wake.min(at.get());
        self.events.schedule(at, event)
    }

    /// Withdraws a wake-up; one that was already delivered or withdrawn is ignored.
    pub fn cancel(&mut self, id: EventId) {
        self.events.cancel(id);
        self.refresh_next_wake();
    }

    fn refresh_next_wake(&mut self) {
        self.next_wake = self.events.next_due().map_or(u64::MAX, Cycles::get);
    }

    /// Device time of the earliest wake-up, if any.
    #[must_use]
    pub fn next_wake(&self) -> Option<Cycles> {
        (self.next_wake != u64::MAX).then_some(Cycles(self.next_wake))
    }

    /// Whether a wake-up is due now. One comparison: the run loop asks after every instruction.
    #[must_use]
    pub const fn wake_due(&self) -> bool {
        self.next_wake <= self.now.get()
    }

    /// The oldest wake-up that is due, if any.
    pub fn take_due(&mut self) -> Option<Event> {
        let (_, event) = self.events.pop_due(self.now)?;
        self.refresh_next_wake();
        Some(event)
    }

    /// Asks for `event` to be delivered before the current instruction ends, after the events
    /// posted before it.
    pub fn signal(&mut self, event: Event) {
        self.signals.push_back(event);
    }

    /// The oldest event posted with [`Ctx::signal`] and not delivered yet.
    pub fn take_signal(&mut self) -> Option<Event> {
        self.signals.pop_front()
    }

    /// Drives the level of an interrupt source.
    ///
    /// The context carries levels, and the interrupt controller samples them when it is
    /// evaluated. A device holds its source raised until software acknowledges it, as the chip's
    /// peripherals do; a pulse shorter than one instruction (raised and lowered again before the
    /// controller is evaluated) is lost to an edge-type line (report IR §1.3, controller ruling
    /// Q13).
    pub fn irq(&mut self, source: Source, asserted: bool) {
        let levels = if asserted {
            self.sources | source.mask()
        } else {
            self.sources & !source.mask()
        };
        if levels != self.sources {
            self.sources = levels;
            self.interrupts_changed = true;
        }
    }

    /// The level of every interrupt source, bit `n` for source `n`.
    #[must_use]
    pub const fn sources(&self) -> u64 {
        self.sources
    }

    /// The interrupt controller tells that one of its registers was written.
    pub fn interrupts_changed(&mut self) {
        self.interrupts_changed = true;
    }

    /// Whether a source level or the controller changed since the last call.
    pub fn take_interrupts_changed(&mut self) -> bool {
        std::mem::take(&mut self.interrupts_changed)
    }

    /// Whether a device left something for the machine: a reset request, a clock selection write
    /// or a change of the interrupt lines.
    #[must_use]
    pub const fn is_flagged(&self) -> bool {
        self.clock_written || self.interrupts_changed || self.reset.is_some()
    }

    /// Asks for a reset. It takes place when the current instruction ends; the first request of an
    /// instruction wins.
    pub fn request_reset(&mut self, cause: ResetCause) {
        self.reset.get_or_insert(cause);
    }

    /// The reset asked for since the last call.
    pub fn take_reset(&mut self) -> Option<ResetCause> {
        self.reset.take()
    }

    /// Reports an operation the device cannot perform faithfully: it becomes a diagnostic that
    /// names the instruction, and `--strict-mmio` stops on it.
    pub fn unsupported(&mut self, what: Unsupported) {
        #[cfg(test)]
        self.refused.push_back(what);
        self.diag
            .record(Diagnostic::Unsupported { what, pc: self.pc });
    }

    /// The oldest refusal a device reported and no test took yet.
    #[cfg(test)]
    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.refused.pop_front()
    }
}

impl Default for Ctx {
    /// A context that stops on nothing: what a test of one device needs.
    fn default() -> Ctx {
        Ctx::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Source = Source::of("SYSTIMER_TARGET0_INT_MAP");
    const SOFTWARE: Source = Source::of("CPU_INTR_FROM_CPU_0_MAP");
    // Any event will do: the context does not look inside them.
    const FIRST: Event = Event::FlashWindowStore {
        instruction_bus: true,
    };
    const SECOND: Event = Event::FlashWindowStore {
        instruction_bus: false,
    };
    const THIRD: Event = Event::MmuFault {
        vaddr: 0x4200_0000,
        content: 0x100,
    };

    #[test]
    fn a_source_is_numbered_by_its_map_register() {
        // IR §3.2: SYSTIMER_TARGET0 is source 37, CPU_INTR_FROM_CPU_0 is 50, CACHE_CORE0_ACS the last, 61.
        assert_eq!(TICK.number(), 37);
        assert_eq!(SOFTWARE.number(), 50);
        assert_eq!(Source::of("MAC_INTR_MAP").number(), 0);
        assert_eq!(Source::of("CACHE_CORE0_ACS_INT_MAP").number(), 61);
    }

    #[test]
    fn the_sources_are_the_map_registers_and_the_register_after_them_is_not_one() {
        let offset = |name: &str| svd_regs::esp32c3::addr("INTERRUPT_CORE0", name) - INTMATRIX_BASE;
        // The lowest and the highest valid source, and the count the build-time check relies on.
        assert_eq!(Source::of("MAC_INTR_MAP").number(), 0);
        assert_eq!(
            Source::of("CACHE_CORE0_ACS_INT_MAP").number(),
            MAP_REGISTERS - 1
        );
        assert_eq!(MAP_REGISTERS, 62);
        // Every register of the map area is a map register by its name, and the first register
        // after it (INTR_STATUS_REG_0) is not: a neighbour never passes for a source.
        for register in svd_regs::esp32c3::registers()
            .iter()
            .filter(|r| r.peripheral == "INTERRUPT_CORE0")
        {
            let in_map_area = register.addr - INTMATRIX_BASE < MAP_REGISTERS * REGISTER_BYTES;
            assert_eq!(
                in_map_area,
                register.name.ends_with("_MAP"),
                "{}",
                register.name
            );
        }
        assert_eq!(offset("INTR_STATUS_REG_0"), MAP_REGISTERS * REGISTER_BYTES);
    }

    #[test]
    fn two_wake_ups_due_together_come_out_in_the_order_they_were_asked_for() {
        let mut ctx = Ctx::default();
        ctx.schedule(Cycles(100), SECOND);
        ctx.schedule(Cycles(100), FIRST);
        ctx.sleep_until(Cycles(100));
        assert_eq!(ctx.take_due(), Some(SECOND));
        assert_eq!(ctx.take_due(), Some(FIRST));
        assert_eq!(ctx.take_due(), None);
    }

    #[test]
    fn a_wake_up_asked_for_an_instant_already_past_is_due_at_once() {
        let mut ctx = Ctx::default();
        ctx.sleep_until(Cycles(500));
        ctx.schedule(Cycles(100), FIRST);
        assert_eq!(ctx.next_wake(), Some(Cycles(100)));
        assert!(ctx.wake_due());
        assert_eq!(ctx.take_due(), Some(FIRST));
        assert!(!ctx.wake_due());
    }

    #[test]
    fn a_source_raised_twice_and_lowered_once_is_lowered() {
        let mut ctx = Ctx::default();
        ctx.irq(TICK, true);
        ctx.irq(TICK, true);
        ctx.irq(TICK, false);
        assert_eq!(ctx.sources(), 0);
    }

    #[test]
    fn device_time_follows_the_cpu_clock_and_never_goes_back() {
        let mut ctx = Ctx::default();
        ctx.advance(10);
        assert_eq!(
            ctx.now(),
            Cycles(80),
            "20 MHz: 8 time-base cycles per CPU cycle"
        );
        ctx.set_clock(CpuClock {
            cpu_period: 1,
            apb_period: 2,
        });
        ctx.advance(10);
        assert_eq!(ctx.now(), Cycles(90));
        ctx.sleep_until(Cycles(1_000));
        assert_eq!(ctx.now(), Cycles(1_000));
        ctx.sleep_until(Cycles(500));
        assert_eq!(
            ctx.now(),
            Cycles(1_000),
            "an instant in the past changes nothing"
        );
    }

    #[test]
    fn wake_ups_come_due_in_time_order_and_a_cancelled_one_never_does() {
        let mut ctx = Ctx::default();
        assert_eq!(ctx.next_wake(), None);
        assert!(!ctx.wake_due());
        let late = ctx.schedule(Cycles(300), THIRD);
        ctx.schedule(Cycles(100), FIRST);
        ctx.schedule(Cycles(200), SECOND);
        assert_eq!(ctx.next_wake(), Some(Cycles(100)));
        assert_eq!(ctx.take_due(), None, "nothing is due at time 0");
        ctx.sleep_until(Cycles(250));
        assert!(ctx.wake_due());
        assert_eq!(ctx.take_due(), Some(FIRST));
        assert_eq!(ctx.take_due(), Some(SECOND));
        assert_eq!(ctx.take_due(), None);
        assert_eq!(ctx.next_wake(), Some(Cycles(300)));
        ctx.cancel(late);
        assert_eq!(ctx.next_wake(), None);
        ctx.sleep_until(Cycles(1_000));
        assert!(!ctx.wake_due());
    }

    #[test]
    fn a_deadline_keeps_the_earliest_call_and_replaces_a_later_one() {
        let mut ctx = Ctx::default();
        let mut deadline = Deadline::default();
        deadline.arm(&mut ctx, Cycles(500), FIRST);
        deadline.arm(&mut ctx, Cycles(900), FIRST);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(500)),
            "the earlier call is kept"
        );
        deadline.arm(&mut ctx, Cycles(200), FIRST);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(200)),
            "an earlier deadline replaces it"
        );
        ctx.sleep_until(Cycles(200));
        assert_eq!(ctx.take_due(), Some(FIRST));
        assert_eq!(ctx.take_due(), None, "the replaced call was withdrawn");
        deadline.called(ctx.now());
        deadline.arm(&mut ctx, Cycles(900), FIRST);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(900)),
            "once called, the deadline arms again"
        );
    }

    #[test]
    fn signals_are_delivered_in_posting_order() {
        let mut ctx = Ctx::default();
        ctx.signal(FIRST);
        ctx.signal(SECOND);
        assert_eq!(ctx.take_signal(), Some(FIRST));
        assert_eq!(ctx.take_signal(), Some(SECOND));
        assert_eq!(ctx.take_signal(), None);
    }

    #[test]
    fn source_levels_are_kept_and_a_change_is_flagged_once() {
        let mut ctx = Ctx::default();
        assert!(!ctx.take_interrupts_changed());
        ctx.irq(TICK, true);
        ctx.irq(SOFTWARE, true);
        assert_eq!(ctx.sources(), (1 << 37) | (1 << 50));
        assert!(ctx.take_interrupts_changed());
        ctx.irq(TICK, true);
        assert!(
            !ctx.take_interrupts_changed(),
            "the same level is no change"
        );
        ctx.irq(TICK, false);
        assert_eq!(ctx.sources(), 1 << 50);
        assert!(ctx.take_interrupts_changed());
        ctx.interrupts_changed();
        assert!(
            ctx.take_interrupts_changed(),
            "a controller register was written"
        );
    }

    #[test]
    fn the_first_reset_request_of_an_instruction_wins() {
        let mut ctx = Ctx::default();
        assert_eq!(ctx.take_reset(), None);
        ctx.request_reset(ResetCause::SoftwareCpu);
        ctx.request_reset(ResetCause::Mwdt0Core);
        assert_eq!(ctx.take_reset(), Some(ResetCause::SoftwareCpu));
        assert_eq!(ctx.take_reset(), None);
    }

    #[test]
    fn a_refusal_becomes_a_diagnostic_that_names_the_instruction() {
        let mut ctx = Ctx::new(true);
        ctx.set_pc(0x4000_1234);
        ctx.unsupported(Unsupported::ShaDma);
        assert_eq!(
            ctx.diag.take_violation(),
            Some(Diagnostic::Unsupported {
                what: Unsupported::ShaDma,
                pc: 0x4000_1234
            })
        );
        assert_eq!(ctx.take_unsupported(), Some(Unsupported::ShaDma));
    }
}
