//! A hardware counter as a value at an instant. Nothing is counted cycle by cycle: a counter knows
//! what it read at one device time and how fast it runs, which gives its value at any later time
//! and the time at which it will reach a value.
use emu_core::Cycles;

/// How fast a counter runs: `counts` counts every `cycles` cycles of the time base. A rate that is
/// not a whole number of time-base cycles per count (the RC oscillators) stays exact this way.
///
/// Two rates are equal when their two fields are, which is not the same as being the same
/// speed: the fraction is not reduced (2 counts every 20 cycles is not equal to 1 every 10).
/// Nothing builds one rate in two such ways (`every(n).divided_by(k)` always has one count, and
/// the RC clocks one ratio each), and the only user of the comparison, [`Counter::retime`],
/// would merely re-anchor a counter it could have left alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    counts: u64,
    cycles: u64,
}

impl Rate {
    /// One count every `period` cycles of the time base; a period of 0 is taken as 1.
    #[must_use]
    pub const fn every(period: u64) -> Rate {
        Rate {
            counts: 1,
            cycles: if period == 0 { 1 } else { period },
        }
    }

    /// `counts` counts every `cycles` cycles of the time base.
    ///
    /// # Panics
    /// When either is 0: a rate is a constant of the model, checked where it is written.
    #[must_use]
    pub const fn ratio(counts: u64, cycles: u64) -> Rate {
        assert!(counts != 0 && cycles != 0);
        Rate { counts, cycles }
    }

    /// The same clock divided by `divider`; a divider of 0 is taken as 1.
    #[must_use]
    pub const fn divided_by(self, divider: u64) -> Rate {
        Rate {
            counts: self.counts,
            cycles: self
                .cycles
                .saturating_mul(if divider == 0 { 1 } else { divider }),
        }
    }

    /// Whole counts in `cycles` cycles of the time base.
    #[must_use]
    pub fn counts_in(self, cycles: u64) -> u64 {
        let counts = u128::from(cycles) * u128::from(self.counts) / u128::from(self.cycles);
        u64::try_from(counts).unwrap_or(u64::MAX)
    }

    /// Time-base cycles until `counts` counts have gone by, rounded up to a whole cycle.
    #[must_use]
    pub fn cycles_for(self, counts: u64) -> u64 {
        let cycles =
            (u128::from(counts) * u128::from(self.cycles)).div_ceil(u128::from(self.counts));
        u64::try_from(cycles).unwrap_or(u64::MAX)
    }
}

/// A counter that counts up at a [`Rate`] while it runs. Its value wraps at 64 bits; a narrower
/// counter is masked by its owner.
#[derive(Clone, Copy, Debug)]
pub struct Counter {
    /// The value at `since`.
    base: u64,
    since: Cycles,
    rate: Rate,
    running: bool,
}

impl Counter {
    /// A counter that reads `value` at `now`.
    #[must_use]
    pub const fn new(value: u64, now: Cycles, rate: Rate, running: bool) -> Counter {
        Counter {
            base: value,
            since: now,
            rate,
            running,
        }
    }

    /// Whether the counter counts.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    /// Counts gone by since the counter was last set, started, stopped or retimed.
    fn elapsed(&self, now: Cycles) -> u64 {
        if self.running {
            self.rate
                .counts_in(now.get().saturating_sub(self.since.get()))
        } else {
            0
        }
    }

    /// The value at `now`.
    #[must_use]
    pub fn value(&self, now: Cycles) -> u64 {
        self.base.wrapping_add(self.elapsed(now))
    }

    /// The counter reads `value` at `instant`, and counts on from there.
    pub fn set(&mut self, value: u64, instant: Cycles) {
        self.base = value;
        self.since = instant;
    }

    /// Starts or stops the counter at `now`; it keeps its value.
    pub fn run(&mut self, running: bool, now: Cycles) {
        if running == self.running {
            // Nothing changes: the anchor stays, and with it the fraction of a count gone by.
            return;
        }
        self.base = self.value(now);
        self.since = now;
        self.running = running;
    }

    /// From `now` on the counter runs at `rate`; it keeps its value.
    pub fn retime(&mut self, rate: Rate, now: Cycles) {
        if rate == self.rate {
            return;
        }
        self.base = self.value(now);
        self.since = now;
        self.rate = rate;
    }

    /// The device time at which the counter will have counted `ahead` more than it reads at `now`,
    /// `None` when it is stopped.
    #[must_use]
    pub fn instant_in(&self, ahead: u64, now: Cycles) -> Option<Cycles> {
        self.running.then(|| {
            // From the anchor, so that the fraction of a count already gone by is not counted twice.
            self.since
                + self
                    .rate
                    .cycles_for(self.elapsed(now).saturating_add(ahead))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_period_counts_exactly() {
        // A SYSTIMER counter: 16 MHz, one count every 10 cycles of the time base.
        let counter = Counter::new(0, Cycles(100), Rate::every(10), true);
        assert_eq!(counter.value(Cycles(100)), 0);
        assert_eq!(counter.value(Cycles(109)), 0);
        assert_eq!(counter.value(Cycles(110)), 1);
        assert_eq!(counter.value(Cycles(1_600_100)), 160_000);
        assert_eq!(
            counter.instant_in(160_000, Cycles(100)),
            Some(Cycles(1_600_100))
        );
        // 5 cycles into a count, the next count is 5 cycles away, not 10.
        assert_eq!(counter.instant_in(1, Cycles(105)), Some(Cycles(110)));
    }

    #[test]
    fn a_rate_that_is_not_a_whole_number_of_cycles_does_not_drift() {
        // The 136 kHz RC oscillator: 136 000 counts in 160 000 000 cycles, 17 in 20 000.
        let slow = Rate::ratio(17, 20_000);
        let counter = Counter::new(0, Cycles::ZERO, slow, true);
        assert_eq!(counter.value(Cycles(160_000_000)), 136_000, "one second");
        assert_eq!(
            counter.value(Cycles(1_600_000_000)),
            1_360_000,
            "ten seconds"
        );
        // 400 000 counts, the flash-boot timeout of the RTC watchdog: 2.941 s (TW §D.4).
        let at = counter.instant_in(400_000, Cycles::ZERO).unwrap();
        assert_eq!(at, Cycles(470_588_236));
        assert_eq!(counter.value(at), 400_000);
        assert_eq!(counter.value(Cycles(at.get() - 1)), 399_999);
    }

    #[test]
    fn stopping_retiming_and_setting_keep_or_replace_the_value() {
        let mut counter = Counter::new(5, Cycles::ZERO, Rate::every(8), true);
        counter.run(false, Cycles(80));
        assert_eq!(counter.value(Cycles(8_000)), 15, "stopped at 15");
        assert_eq!(
            counter.instant_in(1, Cycles(8_000)),
            None,
            "a stopped counter reaches nothing"
        );
        assert!(!counter.is_running());
        counter.run(true, Cycles(8_000));
        // The APB clock goes from 20 MHz to 80 MHz: 2 cycles of the time base per count.
        counter.retime(Rate::every(2), Cycles(8_080));
        assert_eq!(counter.value(Cycles(8_080)), 25);
        assert_eq!(counter.value(Cycles(8_100)), 35);
        counter.set(0, Cycles(8_100));
        assert_eq!(counter.value(Cycles(8_104)), 2);
    }

    #[test]
    fn confirming_the_state_of_a_counter_loses_no_fraction_of_a_count() {
        // A device re-states "running, at this rate" every time it is called: 9 cycles into a
        // 10-cycle count, the count must still end at cycle 10.
        let mut counter = Counter::new(0, Cycles::ZERO, Rate::every(10), true);
        counter.run(true, Cycles(9));
        counter.retime(Rate::every(10), Cycles(9));
        assert_eq!(counter.value(Cycles(10)), 1);
    }

    #[test]
    fn dividers_and_degenerate_rates_never_divide_by_zero() {
        // The interrupt watchdog: the crystal (4 cycles) divided by 20 000 is 2 kHz (TW §B.3).
        let two_khz = Rate::every(4).divided_by(20_000);
        assert_eq!(two_khz.counts_in(160_000_000), 2_000);
        assert_eq!(two_khz.cycles_for(600), 48_000_000, "600 counts are 300 ms");
        assert_eq!(Rate::every(0), Rate::every(1));
        assert_eq!(Rate::every(4).divided_by(0), Rate::every(4));
        assert_eq!(Rate::every(1).counts_in(u64::MAX), u64::MAX);
        assert_eq!(
            Rate::every(u64::MAX).cycles_for(u64::MAX),
            u64::MAX,
            "saturates"
        );
    }
}
