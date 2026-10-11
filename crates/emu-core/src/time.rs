//! Virtual time, measured in `SoC` clock cycles.
use std::num::NonZeroU64;
use std::ops::Add;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A point in virtual time, counted in cycles of the emulated `SoC` clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cycles(pub u64);

impl Cycles {
    pub const ZERO: Cycles = Cycles(0);

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Add<u64> for Cycles {
    type Output = Cycles;
    fn add(self, rhs: u64) -> Cycles {
        Cycles(self.0.saturating_add(rhs))
    }
}

/// A clock frequency; never zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frequency(NonZeroU64);

impl Frequency {
    #[must_use]
    pub fn from_hz(hz: u64) -> Option<Frequency> {
        NonZeroU64::new(hz).map(Frequency)
    }

    #[must_use]
    pub fn hz(&self) -> u64 {
        self.0.get()
    }

    /// Converts a cycle count to nanoseconds, saturating at `u64::MAX`.
    #[must_use]
    pub fn cycles_to_nanos(&self, cycles: Cycles) -> u64 {
        let nanos = u128::from(cycles.0) * NANOS_PER_SECOND / u128::from(self.hz());
        u64::try_from(nanos).unwrap_or(u64::MAX)
    }
}

const ATTOS_PER_SECOND: u128 = 1_000_000_000_000_000_000;
const ATTOS_PER_NANO: u128 = 1_000_000_000;

impl Frequency {
    /// A frequency known at compile time.
    #[must_use]
    pub const fn new(hz: NonZeroU64) -> Frequency {
        Frequency(hz)
    }
}

/// Converts the cycle counter of a CPU whose clock frequency changes into elapsed time.
///
/// Time is kept in attoseconds, so a cycle that is not a whole number of nanoseconds (6.25 ns at
/// 160 MHz) accumulates without drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualClock {
    frequency: Frequency,
    /// Cycle counter at the last frequency change.
    since: Cycles,
    /// Time elapsed at `since`.
    elapsed_attos: u128,
}

impl VirtualClock {
    #[must_use]
    pub const fn new(frequency: Frequency) -> VirtualClock {
        VirtualClock {
            frequency,
            since: Cycles::ZERO,
            elapsed_attos: 0,
        }
    }

    #[must_use]
    pub const fn frequency(&self) -> Frequency {
        self.frequency
    }

    fn attos_at(&self, now: Cycles) -> u128 {
        // `u64::MAX` cycles times 1e18 stays below `u128::MAX` (1.8e37 < 3.4e38).
        let cycles = u128::from(now.0.saturating_sub(self.since.0));
        self.elapsed_attos
            .saturating_add(cycles * ATTOS_PER_SECOND / u128::from(self.frequency.hz()))
    }

    /// From cycle `now` on, a cycle lasts `1 / frequency`.
    ///
    /// The cycle counter only grows, so `now` is never before the previous change. Should a caller
    /// pass an earlier one, it is taken as the previous change itself: moving the anchor back would
    /// count the cycles in between a second time.
    pub fn set_frequency(&mut self, now: Cycles, frequency: Frequency) {
        let now = Cycles(now.0.max(self.since.0));
        self.elapsed_attos = self.attos_at(now);
        self.since = now;
        self.frequency = frequency;
    }

    /// Nanoseconds elapsed when the cycle counter reads `now`, saturating at `u64::MAX`.
    #[must_use]
    pub fn nanos_at(&self, now: Cycles) -> u64 {
        u64::try_from(self.attos_at(now) / ATTOS_PER_NANO).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cycles_add_saturates_at_u64_max() {
        assert_eq!(Cycles(3) + 4, Cycles(7));
        assert_eq!(Cycles(u64::MAX - 1) + 5, Cycles(u64::MAX));
        assert_eq!(Cycles(u64::MAX) + u64::MAX, Cycles(u64::MAX));
    }
    #[test]
    fn zero_frequency_is_rejected() {
        assert!(Frequency::from_hz(0).is_none());
    }
    #[test]
    fn cycles_convert_to_nanos_without_overflow() {
        let f = Frequency::from_hz(160_000_000).unwrap();
        assert_eq!(f.cycles_to_nanos(Cycles(160)), 1_000);
        // u64::MAX cycles at 160 MHz ≈ 1.15e20 ns, beyond u64: the conversion saturates.
        assert_eq!(f.cycles_to_nanos(Cycles(u64::MAX)), u64::MAX);
    }

    fn mhz(value: u64) -> Frequency {
        Frequency::from_hz(value * 1_000_000).unwrap()
    }

    #[test]
    fn a_virtual_clock_adds_up_time_across_frequency_switches() {
        let mut clock = VirtualClock::new(mhz(20));
        assert_eq!(
            clock.nanos_at(Cycles(20_000_000)),
            1_000_000_000,
            "20 M cycles at 20 MHz"
        );
        clock.set_frequency(Cycles(20_000_000), mhz(80));
        assert_eq!(clock.frequency(), mhz(80));
        assert_eq!(
            clock.nanos_at(Cycles(100_000_000)),
            2_000_000_000,
            "+80 M cycles at 80 MHz"
        );
        clock.set_frequency(Cycles(100_000_000), mhz(160));
        assert_eq!(
            clock.nanos_at(Cycles(260_000_000)),
            3_000_000_000,
            "+160 M cycles at 160 MHz"
        );
    }

    #[test]
    fn a_cycle_that_is_not_a_whole_nanosecond_does_not_drift() {
        // 6.25 ns per cycle at 160 MHz: switching on an odd cycle must not lose the fraction.
        let mut clock = VirtualClock::new(mhz(160));
        clock.set_frequency(Cycles(1), mhz(160));
        clock.set_frequency(Cycles(3), mhz(160));
        assert_eq!(clock.nanos_at(Cycles(4)), 25);
        // Asked about a cycle before the last switch (cycle 3, 18.75 ns), the clock answers the time
        // of that switch: it keeps no history, and it never goes back past it.
        assert_eq!(
            clock.nanos_at(Cycles(1)),
            18,
            "time does not run backwards before the last switch"
        );
    }

    #[test]
    fn a_frequency_change_dated_before_the_last_one_does_not_count_cycles_twice() {
        // 1 µs per cycle up to cycle 100, then 2 MHz. A change dated at cycle 50 cannot move the
        // anchor back: the 50 cycles from 50 to 100 are already in the elapsed time.
        let mut clock = VirtualClock::new(mhz(1));
        clock.set_frequency(Cycles(100), mhz(2));
        clock.set_frequency(Cycles(50), mhz(1));
        assert_eq!(clock.frequency(), mhz(1), "the new frequency applies");
        assert_eq!(clock.nanos_at(Cycles(100)), 100_000, "from the last change");
        assert_eq!(clock.nanos_at(Cycles(110)), 110_000);
    }

    #[test]
    fn a_period_that_is_not_a_whole_number_of_attoseconds_loses_less_than_one_per_switch() {
        // 40 MHz / 1024, the slowest crystal division, rounded down to 39 062 Hz: one cycle lasts
        // 25 600 327 684 194.357… attoseconds. Each switch floors the time elapsed so far, so 1 000
        // switches lose less than 1 000 attoseconds: far below a nanosecond after a whole second.
        let slow = Frequency::from_hz(39_062).unwrap();
        let mut clock = VirtualClock::new(slow);
        for cycle in 1..=1_000 {
            clock.set_frequency(Cycles(cycle), slow);
        }
        let nanos = clock.nanos_at(Cycles(39_062));
        assert!(
            (999_999_999..=1_000_000_000).contains(&nanos),
            "one second of cycles: {nanos} ns"
        );
        assert_eq!(
            VirtualClock::new(slow).nanos_at(Cycles(39_062)),
            1_000_000_000,
            "without a switch the second is exact"
        );
    }

    #[test]
    fn a_virtual_clock_saturates_instead_of_overflowing() {
        let clock = VirtualClock::new(Frequency::from_hz(1).unwrap());
        assert_eq!(clock.nanos_at(Cycles(u64::MAX)), u64::MAX);
    }

    #[test]
    fn a_frequency_can_be_a_constant() {
        const ONE: Frequency = Frequency::new(std::num::NonZeroU64::MIN);
        assert_eq!(ONE.hz(), 1);
    }
}
