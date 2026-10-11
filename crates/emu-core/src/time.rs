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

#[cfg(test)]
mod tests {
    use super::*;
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
}
