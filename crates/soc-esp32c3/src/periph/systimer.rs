//! `SYSTIMER`: two 52-bit counters and three comparators, the time base of ESP-IDF (`esp_timer`
//! on counter 0, the `FreeRTOS` tick on counter 1 and comparator 0). Sources: TRM chapter 10, ESP-IDF
//! `systimer_reg.h` and `systimer_ll.h`; report TW §A.
//!
//! The counters count at 16 MHz from the crystal whatever the CPU and APB clocks are (TRM §10.3;
//! TW §A.1): one count is 10 cycles of the time base. Nothing is polled: a counter is a value at an
//! instant, and a comparator asks to be woken at the device time of its next alarm.
use super::SYSTIMER_BASE;
use super::counter::{Counter, Rate};
use super::{consecutive, half_of, index_in, replace_half};
use crate::ctx::{Ctx, Deadline, Event, Source};
use emu_core::{Cycles, Peripheral};
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("SYSTIMER", name) - SYSTIMER_BASE
}

const CONF: u32 = reg("CONF");
// The first register of each block that is indexed by its offset (`index_in`, `pair_in`): the
// others follow it, which the assertion below `pair_in` checks.
const UNIT0_OP: u32 = reg("UNIT0_OP");
const UNIT0_LOAD_HI: u32 = reg("UNIT0_LOAD_HI");
const TARGET0_HI: u32 = reg("TARGET0_HI");
const TARGET0_CONF: u32 = reg("TARGET0_CONF");
const UNIT0_VALUE_HI: u32 = reg("UNIT0_VALUE_HI");
const COMP0_LOAD: u32 = reg("COMP0_LOAD");
const UNIT0_LOAD: u32 = reg("UNIT0_LOAD");
const INT_ENA: u32 = reg("INT_ENA");
const INT_RAW: u32 = reg("INT_RAW");
const INT_CLR: u32 = reg("INT_CLR");
const INT_ST: u32 = reg("INT_ST");
const DATE: u32 = reg("DATE");

/// `CNT_CLK` is the crystal divided by 2.5, 16 MHz: one count every 10 cycles of the time base
/// (TRM §10.3).
const COUNT_RATE: Rate = Rate::every(10);
const UNITS: usize = 2;
const COMPARATORS: usize = 3;
/// A counter and a target are 52 bits wide: 20 bits in the `_HI` register (TW §A.2).
const COUNT_MASK: u64 = (1 << 52) - 1;
const HI_MASK: u32 = 0x000F_FFFF;
/// "If t is already passed (0 <= count - t < 2^51 - 1) it fires immediately" (TRM table 10.4-2).
const PASSED_WINDOW: u64 = (1 << 51) - 1;

/// `CONF` (reset `0x46000000`): bit 31 `CLK_EN`, bit 30 `UNIT0_WORK_EN`, bit 29 `UNIT1_WORK_EN`,
/// bits 28..25 the stall enables, bits 24, 23, 22 `TARGET0/1/2_WORK_EN`, bit 0 `SYSTIMER_CLK_FO`.
/// `systimer_ll_enable_counter` sets bit `30 - counter` and `systimer_ll_enable_alarm` bit
/// `24 - alarm` (`IDF:hal/esp32c3/include/hal/systimer_ll.h:70-77, 163-170`). The stall bits stop a
/// counter while the CPU is halted by a debugger (TRM table 10.4-1): there is no debugger, they are
/// stored (`systimer_ll_counter_can_stall_by_cpu`, same file, lines 79-86).
const CONF_WRITABLE: u32 = 0xFFC0_0001;
const UNIT0_WORK_EN: u32 = 1 << 30;
const TARGET0_WORK_EN: u32 = 1 << 24;
/// `UNITn_OP`: bit 30 `UPDATE` latches the counter (`systimer_ll_counter_snapshot`, line 88),
/// bit 29 `VALUE_VALID` says the latch can be read (`systimer_ll_is_counter_value_valid`, line 93).
const OP_UPDATE: u32 = 1 << 30;
const OP_VALUE_VALID: u32 = 1 << 29;
/// `TARGETx_CONF`: bit 31 the counter compared (`systimer_ll_connect_alarm_counter`, line 132),
/// bit 30 `PERIOD_MODE` (`systimer_ll_enable_alarm_period`, line 142), bits `[25:0]` the period
/// (`systimer_ll_set_alarm_period`, line 147).
const TARGET_CONF_WRITABLE: u32 = 0xC3FF_FFFF;
const TARGET_UNIT_SEL: u32 = 1 << 31;
const TARGET_PERIOD_MODE: u32 = 1 << 30;
const TARGET_PERIOD_MASK: u32 = 0x03FF_FFFF;
/// Bits 2..0 of the four interrupt registers: comparators 2, 1 and 0. `INT_ENA` bit `alarm` is
/// set and cleared by `systimer_ll_enable_alarm_int` (`systimer_ll.h:174-181`), `INT_ST` is read
/// by `systimer_ll_is_alarm_int_fired` (lines 183-186) and `INT_CLR` bit `alarm` is written by
/// `systimer_ll_clear_alarm_int` (lines 188-191); `INT_RAW` is set by the hardware only.
const INT_MASK: u32 = 0b111;
/// Bit 0 of `UNITn_LOAD` and `COMPx_LOAD`, a write-1 trigger (`systimer_ll_apply_counter_value`,
/// line 114; `systimer_ll_apply_alarm_value`, line 158).
const LOAD_TRIGGER: u32 = 1;
const CONF_RESET: u32 = 0x4600_0000;
const DATE_VALUE: u32 = 0x0200_6171;

/// Comparator `x` drives interrupt source `SYSTIMER_TARGETx` (37, 38, 39: report BC §F.1).
const SOURCES: [Source; COMPARATORS] = [
    Source::of("SYSTIMER_TARGET0_INT_MAP"),
    Source::of("SYSTIMER_TARGET1_INT_MAP"),
    Source::of("SYSTIMER_TARGET2_INT_MAP"),
];

#[derive(Clone, Copy, Debug)]
struct Unit {
    counter: Counter,
    /// `UNITn_LOAD_HI/LO`.
    load: u64,
    /// `UNITn_VALUE_HI/LO`: the count at the last `UPDATE`.
    latch: u64,
    valid: bool,
}

impl Unit {
    fn new(now: Cycles, working: bool) -> Unit {
        Unit {
            counter: Counter::new(0, now, COUNT_RATE, working),
            load: 0,
            latch: 0,
            valid: false,
        }
    }

    fn count(&self, now: Cycles) -> u64 {
        self.counter.value(now) & COUNT_MASK
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Comparator {
    /// `TARGETx_HI/LO` and `TARGETx_CONF` as software reads them.
    target: u64,
    conf: u32,
    /// The target and the period the comparator works with: copies made by `COMPx_LOAD`.
    loaded_target: u64,
    loaded_period: u64,
    /// The count of the next alarm, `None` when the comparator has nothing left to signal.
    next: Option<u64>,
}

#[derive(Debug)]
pub struct Systimer {
    conf: u32,
    units: [Unit; UNITS],
    comparators: [Comparator; COMPARATORS],
    int_ena: u32,
    int_raw: u32,
    wake: Deadline,
    /// `QemuCompat::systimer_no_alarm_on_stopped_counter`: a comparator compares only a counter
    /// that runs. Kept across a reset of the block: it is a property of the machine.
    no_alarm_on_stopped_counter: bool,
}

/// Splits a register offset inside a block of `pairs` `(HI, LO)` pairs whose first register is at
/// `first` into the pair index and whether it is the `HI` half.
fn pair_in(offset: u32, first: u32, pairs: usize) -> Option<(usize, bool)> {
    let word = index_in(offset, first, 2 * pairs)?;
    Some((word / 2, word.is_multiple_of(2)))
}

// The blocks indexed by offset are consecutive registers, in the order the indexing assumes:
// `(HI, LO)` pairs per counter and per comparator, then one register per counter or comparator.
const _: () = assert!(
    consecutive(
        "SYSTIMER",
        &[
            "UNIT0_LOAD_HI",
            "UNIT0_LOAD_LO",
            "UNIT1_LOAD_HI",
            "UNIT1_LOAD_LO"
        ]
    ) && consecutive(
        "SYSTIMER",
        &[
            "TARGET0_HI",
            "TARGET0_LO",
            "TARGET1_HI",
            "TARGET1_LO",
            "TARGET2_HI",
            "TARGET2_LO"
        ]
    ) && consecutive(
        "SYSTIMER",
        &[
            "UNIT0_VALUE_HI",
            "UNIT0_VALUE_LO",
            "UNIT1_VALUE_HI",
            "UNIT1_VALUE_LO"
        ]
    ) && consecutive(
        "SYSTIMER",
        &["TARGET0_CONF", "TARGET1_CONF", "TARGET2_CONF"]
    ) && consecutive("SYSTIMER", &["COMP0_LOAD", "COMP1_LOAD", "COMP2_LOAD"])
        && consecutive("SYSTIMER", &["UNIT0_OP", "UNIT1_OP"])
        && consecutive("SYSTIMER", &["UNIT0_LOAD", "UNIT1_LOAD"])
);

impl Systimer {
    /// The block after a power-on or a peripheral reset: counter 0 counts from 0 at `now`
    /// (`UNIT0_WORK_EN` resets to 1), counter 1 is stopped, no comparator works.
    ///
    /// ASSUMPTION: a reset of the block (`SYSTEM.PERIP_RST_EN0` bit 29) returns both counters to 0.
    /// The TRM says the registers "will be restored to their default values" (§10.3) and does not
    /// name the counters (report TW §I question 6); a read of counter 0 after
    /// `esp_timer_impl_early_init` on a real chip settles it.
    #[must_use]
    pub fn new(now: Cycles) -> Systimer {
        Systimer {
            conf: CONF_RESET,
            units: [Unit::new(now, true), Unit::new(now, false)],
            comparators: [Comparator::default(); COMPARATORS],
            int_ena: 0,
            int_raw: 0,
            wake: Deadline::default(),
            no_alarm_on_stopped_counter: false,
        }
    }

    /// The block as Espressif's QEMU has it, for the lockstep (`QemuCompat`): a comparator raises
    /// no alarm while its counter is stopped.
    #[must_use]
    pub fn with_qemu_compat(mut self, no_alarm_on_stopped_counter: bool) -> Systimer {
        self.no_alarm_on_stopped_counter = no_alarm_on_stopped_counter;
        self
    }

    /// The block is reset (`SYSTEM.PERIP_RST_EN0` bit 29, or a core reset): it is built again at
    /// the present time. Its wake-up is withdrawn first, so that a firmware resetting the block over
    /// and over leaves no call behind in the scheduler.
    pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
        self.wake.cancel(ctx);
        *self = Systimer::new(ctx.now()).with_qemu_compat(self.no_alarm_on_stopped_counter);
        // The rebuilt block drives its interrupt sources low.
        self.wake(ctx);
    }

    const fn comparator_works(&self, comparator: usize) -> bool {
        self.conf & (TARGET0_WORK_EN >> comparator) != 0
    }

    fn count(&self, unit: usize, now: Cycles) -> u64 {
        self.units[unit].count(now)
    }

    fn unit_of(&self, comparator: usize) -> usize {
        usize::from(self.comparators[comparator].conf & TARGET_UNIT_SEL != 0)
    }

    /// Works out the next alarm of a comparator that was just enabled, loaded or switched to
    /// another mode: in period mode "alarms at t1 + n x period, t1 being the count at the start"
    /// (TRM §10.4.2); in target mode at the target, at once when it is already passed.
    ///
    /// ASSUMPTION: in period mode "the start" is each of the writes that call this: the write
    /// that sets `TARGETx_WORK_EN`, a `COMPx_LOAD`, and a write that changes `PERIOD_MODE` or
    /// the counter selected. `t1` is the count read at that write, so the last of them before
    /// the first alarm sets the phase of every alarm after it. The TRM does not say which event
    /// starts the period, nor whether a later one of these restarts it. Programming a period on
    /// a chip, then writing `COMPx_LOAD` again half a period later, and timing the next alarm
    /// against both writes, settles it.
    fn start(&mut self, comparator: usize, now: Cycles) {
        let count = self.count(self.unit_of(comparator), now);
        let c = &mut self.comparators[comparator];
        c.next = if c.conf & TARGET_PERIOD_MODE == 0 {
            Some(c.loaded_target)
        } else if c.loaded_period == 0 {
            // A period of 0 would be an alarm at every count: nothing ESP-IDF programs.
            None
        } else {
            Some(count.wrapping_add(c.loaded_period) & COUNT_MASK)
        };
    }

    /// Raises every alarm that is due at `now`, then asks to be woken for the next one.
    ///
    /// ASSUMPTION: a comparator in target mode compares even when its counter is stopped, so an
    /// alarm whose target the stopped counter has already reached fires at once. This is TRM table
    /// 10.4-2 read literally; with the order in which `vSystimerSetup` programs the tick
    /// (`IDF:freertos/port_systick.c:60-118`) it gives one early tick interrupt that counts no tick
    /// (report TW §A.4, §I question 6). What settles it is that interrupt: on a real chip, whether
    /// `SYSTIMER_TARGET0` (source 37) is taken once between the write of `TARGET0_WORK_EN` and the
    /// start of counter 1, before any tick period has gone by.
    fn evaluate(&mut self, ctx: &mut Ctx) {
        let now = ctx.now();
        self.wake.called(now);
        let mut earliest: Option<Cycles> = None;
        for index in 0..COMPARATORS {
            if !self.comparator_works(index) {
                continue;
            }
            let unit = self.unit_of(index);
            let count = self.count(unit, now);
            let compares =
                !self.no_alarm_on_stopped_counter || self.units[unit].counter.is_running();
            let comparator = &mut self.comparators[index];
            if compares && let Some(next) = comparator.next {
                let late = count.wrapping_sub(next) & COUNT_MASK;
                if late < PASSED_WINDOW {
                    self.int_raw |= 1 << index;
                    // In period mode the alarms that were missed are one level, not a queue: the
                    // next one is the first multiple of the period after the present count.
                    comparator.next = (comparator.conf & TARGET_PERIOD_MODE != 0
                        && comparator.loaded_period != 0)
                        .then(|| {
                            let periods = late / comparator.loaded_period + 1;
                            next.wrapping_add(periods * comparator.loaded_period) & COUNT_MASK
                        });
                }
            }
            let ahead = comparator
                .next
                .map(|next| next.wrapping_sub(count) & COUNT_MASK);
            // A stopped counter reaches nothing: the comparator waits for it to be started.
            if let Some(at) =
                ahead.and_then(|ahead| self.units[unit].counter.instant_in(ahead, now))
            {
                earliest = Some(earliest.map_or(at, |e| e.min(at)));
            }
        }
        if let Some(at) = earliest {
            self.wake.arm(ctx, at, Event::Systimer);
        }
        self.drive(ctx);
    }

    /// Each comparator drives a level interrupt: `INT_RAW & INT_ENA` (TRM §10.4.4).
    fn drive(&self, ctx: &mut Ctx) {
        for (index, source) in SOURCES.into_iter().enumerate() {
            ctx.irq(source, self.int_raw & self.int_ena & (1 << index) != 0);
        }
    }

    /// The device time of a wake-up has come.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.evaluate(ctx);
    }

    fn write_conf(&mut self, ctx: &mut Ctx, value: u32) {
        let now = ctx.now();
        let previous = self.conf;
        self.conf = value & CONF_WRITABLE;
        // A counter that is stopped or started keeps its count.
        for (unit, work_en) in (0..UNITS).zip([UNIT0_WORK_EN, UNIT0_WORK_EN >> 1]) {
            self.units[unit].counter.run(self.conf & work_en != 0, now);
        }
        for comparator in 0..COMPARATORS {
            let bit = TARGET0_WORK_EN >> comparator;
            if self.conf & bit != 0 && previous & bit == 0 {
                self.start(comparator, now);
            }
        }
        self.evaluate(ctx);
    }
}

impl Peripheral<Ctx> for Systimer {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if let Some((unit, hi)) = pair_in(offset, UNIT0_LOAD_HI, UNITS) {
            return Some(half_of(self.units[unit].load, hi));
        }
        if let Some((comparator, hi)) = pair_in(offset, TARGET0_HI, COMPARATORS) {
            return Some(half_of(self.comparators[comparator].target, hi));
        }
        if let Some((unit, hi)) = pair_in(offset, UNIT0_VALUE_HI, UNITS) {
            return Some(half_of(self.units[unit].latch, hi));
        }
        if let Some(comparator) = index_in(offset, TARGET0_CONF, COMPARATORS) {
            return Some(self.comparators[comparator].conf);
        }
        if let Some(unit) = index_in(offset, UNIT0_OP, UNITS) {
            return Some(if self.units[unit].valid {
                OP_VALUE_VALID
            } else {
                0
            });
        }
        // Write-only triggers read 0.
        if index_in(offset, COMP0_LOAD, COMPARATORS).is_some()
            || index_in(offset, UNIT0_LOAD, UNITS).is_some()
        {
            return Some(0);
        }
        Some(match offset {
            CONF => self.conf,
            INT_ENA => self.int_ena,
            INT_RAW => self.int_raw,
            INT_ST => self.int_raw & self.int_ena,
            INT_CLR => 0,
            DATE => DATE_VALUE,
            _ => return None,
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        let now = ctx.now();
        if let Some((unit, hi)) = pair_in(offset, UNIT0_LOAD_HI, UNITS) {
            self.units[unit].load = replace_half(self.units[unit].load, hi, value, HI_MASK);
            return true;
        }
        if let Some((comparator, hi)) = pair_in(offset, TARGET0_HI, COMPARATORS) {
            let target = &mut self.comparators[comparator].target;
            *target = replace_half(*target, hi, value, HI_MASK);
            return true;
        }
        if let Some(comparator) = index_in(offset, TARGET0_CONF, COMPARATORS) {
            let previous = self.comparators[comparator].conf;
            self.comparators[comparator].conf = value & TARGET_CONF_WRITABLE;
            // The mode and the counter act as soon as they are written: ESP-IDF sets
            // `PERIOD_MODE` after `COMPx_LOAD` (`systimer_hal_select_alarm_mode`). The period
            // itself waits for `COMPx_LOAD`.
            if (previous ^ value) & (TARGET_PERIOD_MODE | TARGET_UNIT_SEL) != 0 {
                self.start(comparator, now);
            }
            self.evaluate(ctx);
            return true;
        }
        if let Some(comparator) = index_in(offset, COMP0_LOAD, COMPARATORS) {
            if value & LOAD_TRIGGER != 0 {
                let c = &mut self.comparators[comparator];
                c.loaded_target = c.target;
                c.loaded_period = u64::from(c.conf & TARGET_PERIOD_MASK);
                self.start(comparator, now);
                self.evaluate(ctx);
            }
            return true;
        }
        if let Some(unit) = index_in(offset, UNIT0_OP, UNITS) {
            if value & OP_UPDATE != 0 {
                // ASSUMPTION: the latch is valid at once, so `VALUE_VALID` reads 1 from the write
                // of `UPDATE` on. The TRM gives no latency for it (TW §A.3, §I question 6), and
                // ESP-IDF polls the bit without a timeout, so a late one would only cost time.
                // Reading `UNITn_OP` in a tight loop right after the write of `UPDATE` on a chip,
                // and counting the reads that return 0, settles it.
                self.units[unit].latch = self.count(unit, now);
                self.units[unit].valid = true;
            }
            return true;
        }
        if let Some(unit) = index_in(offset, UNIT0_LOAD, UNITS) {
            if value & LOAD_TRIGGER != 0 {
                // "The current count value will be changed immediately" (TRM §10.5).
                let load = self.units[unit].load;
                self.units[unit].counter.set(load, now);
                self.evaluate(ctx);
            }
            return true;
        }
        match offset {
            CONF => self.write_conf(ctx, value),
            INT_ENA => {
                self.int_ena = value & INT_MASK;
                self.drive(ctx);
            }
            INT_CLR => {
                self.int_raw &= !value;
                self.drive(ctx);
            }
            // A write to anything else is reported as unmodelled: the read-only registers
            // (`INT_ST`, the `VALUE` latches, `DATE`), and `INT_RAW`, which the SVD gives as
            // read-write. Hardware sets its bits and software clears them through `INT_CLR`
            // (`systimer_ll_clear_alarm_int`); no ESP-IDF code writes `INT_RAW`, and what a write
            // to it does on silicon (raise an interrupt by hand, or nothing) is not known, so the
            // model refuses the write rather than give it an effect.
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT0_LOAD_LO: u32 = reg("UNIT0_LOAD_LO");
    const UNIT1_LOAD_HI: u32 = reg("UNIT1_LOAD_HI");
    const UNIT1_LOAD_LO: u32 = reg("UNIT1_LOAD_LO");
    const UNIT0_VALUE_LO: u32 = reg("UNIT0_VALUE_LO");
    const UNIT1_OP: u32 = reg("UNIT1_OP");
    const UNIT1_LOAD: u32 = reg("UNIT1_LOAD");
    const TARGET2_LO: u32 = reg("TARGET2_LO");
    const TARGET2_CONF: u32 = reg("TARGET2_CONF");
    const COMP2_LOAD: u32 = reg("COMP2_LOAD");

    /// 10 ms of device time: 160 000 counts, 1 600 000 cycles of the time base.
    const TICK_COUNTS: u32 = 160_000;
    const TICK: u64 = 1_600_000;
    const TICK_SOURCE: u64 = 1 << 37;

    fn after(ctx: &mut Ctx, cycles: u64) {
        ctx.sleep_until(ctx.now() + cycles);
    }

    /// Calls the block for every wake-up due at the present time (`periph::pump`).
    fn pump(ctx: &mut Ctx, timer: &mut Systimer) {
        crate::periph::pump(ctx, |ctx, event| {
            assert_eq!(event, Event::Systimer);
            timer.wake(ctx);
        });
    }

    fn counter(ctx: &mut Ctx, timer: &mut Systimer, unit: u32) -> u64 {
        assert!(timer.write(ctx, UNIT0_OP + 4 * unit, OP_UPDATE));
        assert_eq!(timer.read(ctx, UNIT0_OP + 4 * unit), Some(OP_VALUE_VALID));
        let lo = timer.read(ctx, UNIT0_VALUE_LO + 8 * unit).unwrap();
        let hi = timer.read(ctx, UNIT0_VALUE_HI + 8 * unit).unwrap();
        (u64::from(hi) << 32) | u64::from(lo)
    }

    /// The writes of `vSystimerSetup` (TW §A.4, values measured in trace M1).
    fn program_the_tick(ctx: &mut Ctx, timer: &mut Systimer) {
        for (offset, value) in [
            (UNIT1_LOAD_HI, 0),
            (UNIT1_LOAD_LO, 0),
            (UNIT1_LOAD, 1),
            (TARGET0_CONF, 0x8000_0000),
            (TARGET0_CONF, 0x8000_0000 | TICK_COUNTS),
            (COMP0_LOAD, 1),
            (CONF, 0xD300_0000),
            (TARGET0_CONF, 0xC000_0000 | TICK_COUNTS),
            (INT_ENA, 1),
            (CONF, 0xF700_0000),
        ] {
            assert!(timer.write(ctx, offset, value), "{offset:#x}");
        }
    }

    #[test]
    fn reset_values_are_those_of_the_header() {
        // TW §A.2.
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        assert_eq!(timer.read(ctx, CONF), Some(0x4600_0000));
        assert_eq!(timer.read(ctx, DATE), Some(0x0200_6171));
        for offset in [
            UNIT0_OP,
            UNIT1_OP,
            TARGET0_CONF,
            TARGET2_CONF,
            INT_ENA,
            INT_RAW,
            INT_ST,
        ] {
            assert_eq!(timer.read(ctx, offset), Some(0), "{offset:#x}");
        }
        assert_eq!(timer.read(ctx, DATE + 4), None, "nothing past DATE");
        assert_eq!(
            timer.read(ctx, INT_ST + 4),
            None,
            "the C3 has no REAL_TARGET registers"
        );
        assert!(!timer.write(ctx, INT_ST, 1), "read-only");
    }

    #[test]
    fn counter_0_counts_at_16_mhz_from_reset_whatever_the_cpu_clock() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        assert_eq!(
            timer.read(ctx, UNIT0_OP),
            Some(0),
            "no latch yet: not valid"
        );
        // 1 ms: 160 000 time-base cycles, 16 000 counts.
        after(ctx, 160_000);
        assert_eq!(counter(ctx, &mut timer, 0), 16_000);
        // The latch holds until the next update.
        after(ctx, 160_000);
        assert_eq!(timer.read(ctx, UNIT0_VALUE_LO), Some(16_000));
        assert_eq!(counter(ctx, &mut timer, 0), 32_000);
        assert_eq!(counter(ctx, &mut timer, 1), 0, "UNIT1_WORK_EN resets to 0");
    }

    #[test]
    fn a_stopped_counter_keeps_its_value_and_a_load_changes_it_at_once() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        after(ctx, 1_000);
        // UNIT0_WORK_EN <- 0.
        timer.write(ctx, CONF, 0x0600_0000);
        after(ctx, 5_000);
        assert_eq!(counter(ctx, &mut timer, 0), 100);
        timer.write(ctx, CONF, 0x4600_0000);
        after(ctx, 1_000);
        assert_eq!(
            counter(ctx, &mut timer, 0),
            200,
            "it resumes from where it stopped"
        );
        timer.write(ctx, UNIT0_LOAD_HI, 0xFFFF_FFFF);
        timer.write(ctx, UNIT0_LOAD_LO, 0xFFFF_FFFE);
        assert_eq!(timer.read(ctx, UNIT0_LOAD_HI), Some(0x000F_FFFF), "20 bits");
        timer.write(ctx, UNIT0_LOAD, 1);
        assert_eq!(counter(ctx, &mut timer, 0), 0x000F_FFFF_FFFF_FFFE);
        after(ctx, 30);
        assert_eq!(
            counter(ctx, &mut timer, 0),
            1,
            "the counter wraps at 52 bits"
        );
    }

    #[test]
    fn the_freertos_tick_fires_every_160_000_counts_of_counter_1() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        after(ctx, 12_345);
        program_the_tick(ctx, &mut timer);
        assert_eq!(timer.read(ctx, TARGET0_CONF), Some(0xC002_7100));
        // The early alarm of the ASSUMPTION in `evaluate`: target 0 on a counter stopped at 0.
        assert_eq!(timer.read(ctx, INT_ST), Some(1));
        assert_eq!(ctx.sources(), TICK_SOURCE);
        assert!(timer.write(ctx, INT_CLR, 1));
        assert_eq!(
            ctx.sources(),
            0,
            "the level drops when the handler clears it"
        );
        let start = ctx.now();
        assert_eq!(ctx.next_wake(), Some(start + TICK));
        for tick in 1..=3u64 {
            ctx.sleep_until(Cycles(start.get() + tick * TICK - 1));
            pump(ctx, &mut timer);
            assert_eq!(
                timer.read(ctx, INT_RAW),
                Some(0),
                "one cycle before tick {tick}"
            );
            after(ctx, 1);
            pump(ctx, &mut timer);
            assert_eq!(timer.read(ctx, INT_ST), Some(1));
            assert_eq!(ctx.sources(), TICK_SOURCE);
            // What `SysTickIsrHandler` computes: counter 1 divided by the period.
            assert_eq!(counter(ctx, &mut timer, 1) / u64::from(TICK_COUNTS), tick);
            timer.write(ctx, INT_CLR, 1);
        }
    }

    #[test]
    fn as_qemu_has_it_the_tick_set_up_raises_no_early_alarm_and_then_ticks() {
        // `QemuCompat::systimer_no_alarm_on_stopped_counter`: the target-mode comparator enabled
        // on the stopped counter 1 compares nothing; the period alarms come as on the chip.
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now()).with_qemu_compat(true);
        after(ctx, 12_345);
        program_the_tick(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0), "no early alarm");
        assert_eq!(ctx.sources(), 0);
        let start = ctx.now();
        assert_eq!(ctx.next_wake(), Some(start + TICK));
        ctx.sleep_until(start + TICK);
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_ST), Some(1), "the first tick");
        assert_eq!(counter(ctx, &mut timer, 1) / u64::from(TICK_COUNTS), 1);
        // A reset of the block keeps the setting: it belongs to the machine.
        timer.reset(ctx);
        assert!(timer.no_alarm_on_stopped_counter);
    }

    #[test]
    fn a_late_handler_finds_every_period_it_missed_in_the_counter() {
        // TW §J, third trap: the counter must have advanced by the paused time.
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        program_the_tick(ctx, &mut timer);
        timer.write(ctx, INT_CLR, 1);
        after(ctx, 3 * TICK + 500);
        pump(ctx, &mut timer);
        assert_eq!(
            timer.read(ctx, INT_RAW),
            Some(1),
            "one level, not three interrupts"
        );
        assert_eq!(counter(ctx, &mut timer, 1) / u64::from(TICK_COUNTS), 3);
        timer.write(ctx, INT_CLR, 1);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(4 * TICK)),
            "the next alarm is the fourth period"
        );
    }

    #[test]
    fn a_masked_alarm_sets_the_raw_bit_and_drives_nothing() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        program_the_tick(ctx, &mut timer);
        timer.write(ctx, INT_CLR, 1);
        timer.write(ctx, INT_ENA, 0);
        after(ctx, TICK);
        pump(ctx, &mut timer);
        assert_eq!(
            (timer.read(ctx, INT_RAW), timer.read(ctx, INT_ST)),
            (Some(1), Some(0))
        );
        assert_eq!(ctx.sources(), 0);
        timer.write(ctx, INT_ENA, 1);
        assert_eq!(ctx.sources(), TICK_SOURCE, "enabling it raises the line");
    }

    #[test]
    fn a_one_shot_alarm_fires_at_its_target_and_at_once_when_the_target_is_passed() {
        // What `esp_timer` does with comparator 2 on counter 0 (TW §A.4).
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        let program = |ctx: &mut Ctx, timer: &mut Systimer, target: u32| {
            timer.write(ctx, CONF, 0x4600_0000);
            timer.write(ctx, TARGET2_LO - 4, 0);
            timer.write(ctx, TARGET2_LO, target);
            timer.write(ctx, COMP2_LOAD, 1);
            timer.write(ctx, CONF, 0x4640_0000);
        };
        timer.write(ctx, INT_ENA, 0b100);
        program(ctx, &mut timer, 1_000);
        assert_eq!(ctx.next_wake(), Some(Cycles(10_000)));
        ctx.sleep_until(Cycles(10_000));
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b100));
        assert_eq!(ctx.sources(), 1 << 39, "SYSTIMER_TARGET2 is source 39");
        timer.write(ctx, INT_CLR, 0b100);
        after(ctx, 100_000);
        pump(ctx, &mut timer);
        assert_eq!(
            timer.read(ctx, INT_RAW),
            Some(0),
            "one alarm per programming"
        );
        // The counter reads 11 000: a target of 5 000 is already passed.
        program(ctx, &mut timer, 5_000);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b100), "fires immediately");
    }

    #[test]
    fn a_target_is_compared_modulo_the_52_bits_of_the_counter() {
        // TRM table 10.4-2: a target is passed when 0 <= count - t < 2^51 - 1, modulo 2^52.
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        timer.write(ctx, INT_ENA, 0b100);
        // Counter 0 loaded 100 counts before its wrap.
        timer.write(ctx, UNIT0_LOAD_HI, 0x000F_FFFF);
        timer.write(ctx, UNIT0_LOAD_LO, 0xFFFF_FF9C);
        timer.write(ctx, UNIT0_LOAD, 1);
        // A target of 50 is 150 counts ahead, through the wrap: it is not "already passed".
        timer.write(ctx, TARGET2_LO - 4, 0);
        timer.write(ctx, TARGET2_LO, 50);
        timer.write(ctx, COMP2_LOAD, 1);
        timer.write(ctx, CONF, 0x4640_0000);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0));
        assert_eq!(ctx.next_wake(), Some(Cycles(1_500)), "150 counts ahead");
        after(ctx, 1_499);
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0));
        after(ctx, 1);
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b100));
        assert_eq!(counter(ctx, &mut timer, 0), 50, "the counter wrapped");
        // A target 5 counts before the wrap, with the counter 50 counts after it: 55 counts
        // behind, across the wrap, so it fires as soon as the comparator works.
        timer.write(ctx, INT_CLR, 0b100);
        timer.write(ctx, CONF, 0x4600_0000);
        timer.write(ctx, TARGET2_LO - 4, 0x000F_FFFF);
        timer.write(ctx, TARGET2_LO, 0xFFFF_FFFB);
        timer.write(ctx, COMP2_LOAD, 1);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0), "not working yet");
        timer.write(ctx, CONF, 0x4640_0000);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b100), "already passed");
    }

    #[test]
    fn a_comparator_disabled_while_armed_raises_nothing_until_it_works_again() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        timer.write(ctx, INT_ENA, 0b100);
        timer.write(ctx, TARGET2_LO, 1_000);
        timer.write(ctx, COMP2_LOAD, 1);
        timer.write(ctx, CONF, 0x4640_0000);
        assert_eq!(ctx.next_wake(), Some(Cycles(10_000)));
        after(ctx, 5_000);
        // `TARGET2_WORK_EN` <- 0 with the alarm 500 counts away.
        timer.write(ctx, CONF, 0x4600_0000);
        after(ctx, 20_000);
        pump(ctx, &mut timer);
        assert_eq!(
            timer.read(ctx, INT_RAW),
            Some(0),
            "a comparator that does not work compares nothing"
        );
        assert_eq!(ctx.sources(), 0);
        assert_eq!(ctx.next_wake(), None, "and asks for no call");
        // Enabled again with its target passed meanwhile: it fires at once (TRM table 10.4-2).
        timer.write(ctx, CONF, 0x4640_0000);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b100));
        assert_eq!(ctx.sources(), 1 << 39);
    }

    #[test]
    fn a_comparator_switched_to_the_other_counter_compares_with_that_one() {
        // `TARGETx_CONF` bit 31 selects the counter a comparator works with
        // (`systimer_ll_connect_alarm_counter`): a switch acts at the write.
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        let (target1_conf, target1_lo, comp1_load) =
            (TARGET0_CONF + 4, TARGET0_HI + 12, COMP0_LOAD + 4);
        timer.write(ctx, INT_ENA, 0b010);
        // Counter 0 reads 500 when counter 1 is started from 0.
        after(ctx, 5_000);
        timer.write(ctx, CONF, 0x6600_0000);
        // Comparator 1 on counter 1, target 300.
        timer.write(ctx, target1_conf, TARGET_UNIT_SEL);
        timer.write(ctx, target1_lo, 300);
        timer.write(ctx, comp1_load, 1);
        timer.write(ctx, CONF, 0x6680_0000);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0));
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(5_000 + 3_000)),
            "300 counts of counter 1"
        );
        // Switched to counter 0, which is past 300: the alarm fires at the write.
        timer.write(ctx, target1_conf, 0);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b010));
        assert_eq!(ctx.sources(), 1 << 38, "SYSTIMER_TARGET1 is source 38");
        // And back to counter 1, which is not there yet: it waits for it.
        timer.write(ctx, INT_CLR, 0b010);
        timer.write(ctx, target1_conf, TARGET_UNIT_SEL);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0));
        after(ctx, 2_999);
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0));
        after(ctx, 1);
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b010));
        assert_eq!(counter(ctx, &mut timer, 1), 300);
    }

    #[test]
    fn a_wake_up_for_a_state_that_is_gone_finds_nothing_to_do() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        program_the_tick(ctx, &mut timer);
        timer.write(ctx, INT_CLR, 1);
        // The block is reset while its wake-up is still scheduled.
        timer = Systimer::new(ctx.now());
        after(ctx, TICK);
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0));
        assert_eq!(ctx.next_wake(), None);
    }

    #[test]
    fn a_reset_withdraws_the_wake_up_and_lowers_the_sources() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        program_the_tick(ctx, &mut timer);
        assert!(ctx.next_wake().is_some());
        timer.reset(ctx);
        assert_eq!(ctx.next_wake(), None, "no call is left for the old state");
        assert_eq!(timer.read(ctx, INT_RAW), Some(0));
        assert_eq!(timer.read(ctx, INT_ENA), Some(0));
        assert_eq!(ctx.sources(), 0);
        // Reset again and again: the scheduler holds a bounded number of withdrawn calls, not one
        // per reset (without the withdrawal it would hold 1 000 live ones).
        for _ in 0..1_000 {
            program_the_tick(ctx, &mut timer);
            timer.reset(ctx);
        }
        assert!(ctx.scheduled_entries() <= 40, "{}", ctx.scheduled_entries());
    }
}
