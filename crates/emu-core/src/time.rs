//! Virtual time, measured in clock cycles.
use std::num::NonZeroU64;
use std::ops::Add;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A count of clock cycles: a point in virtual time, or a duration.
///
/// Which clock is counted is said where the value lives. A machine keeps its device time in cycles
/// of one fixed time-base clock, which every clock of the chip divides, so an event stays at its
/// instant when the CPU clock is switched or stopped; a hart counts the cycles of its own clock.
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

impl Frequency {
    /// A frequency known at compile time.
    #[must_use]
    pub const fn new(hz: NonZeroU64) -> Frequency {
        Frequency(hz)
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

    #[test]
    fn a_frequency_can_be_a_constant() {
        const ONE: Frequency = Frequency::new(std::num::NonZeroU64::MIN);
        assert_eq!(ONE.hz(), 1);
    }
}
