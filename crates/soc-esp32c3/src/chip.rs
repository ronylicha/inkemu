//! Silicon revision of the chip, as the eFuse wafer-version fields encode it.
//! Source: `docs/research/esp32c3-boot-chain.md`, §H (ESP-IDF `efuse_ll.h`, `efuse_hal.c`).
use std::fmt;
use std::str::FromStr;

/// `WAFER_VERSION_MAJOR` is 2 bits wide, `WAFER_VERSION_MINOR` 4 (3 low bits + 1 high bit).
const MAX_MAJOR: u8 = 3;
const MAX_MINOR: u8 = 15;
/// `efuse_hal_chip_revision()` is `major * 100 + minor`.
const MAJOR_WEIGHT: u16 = 100;

/// A chip revision `v<major>.<minor>`, ordered by major then minor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChipRevision {
    major: u8,
    minor: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChipRevisionError {
    #[error(
        "chip revision v{major}.{minor} does not fit the eFuse fields (major 0..=3, minor 0..=15); use a revision Espressif shipped, such as v0.3, v0.4 or v1.1"
    )]
    OutOfRange { major: u8, minor: u8 },
    #[error("`{text}` is not a chip revision; write it as v<major>.<minor>, for example v0.3")]
    Malformed { text: String },
}

impl ChipRevision {
    /// The revision of the virtual chip Espressif's QEMU exposes (report §H).
    pub const V0_3: ChipRevision = ChipRevision { major: 0, minor: 3 };

    /// # Errors
    /// [`ChipRevisionError::OutOfRange`] when a value does not fit its eFuse field.
    pub fn new(major: u8, minor: u8) -> Result<ChipRevision, ChipRevisionError> {
        if major > MAX_MAJOR || minor > MAX_MINOR {
            return Err(ChipRevisionError::OutOfRange { major, minor });
        }
        Ok(ChipRevision { major, minor })
    }

    #[must_use]
    pub const fn major(self) -> u8 {
        self.major
    }

    #[must_use]
    pub const fn minor(self) -> u8 {
        self.minor
    }

    /// ESP-IDF's `efuse_hal_chip_revision`: `major * 100 + minor`.
    #[must_use]
    pub fn full(self) -> u16 {
        u16::from(self.major) * MAJOR_WEIGHT + u16::from(self.minor)
    }
}

impl FromStr for ChipRevision {
    type Err = ChipRevisionError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let malformed = || ChipRevisionError::Malformed {
            text: text.to_owned(),
        };
        let (major, minor) = text
            .strip_prefix('v')
            .and_then(|rest| rest.split_once('.'))
            .ok_or_else(malformed)?;
        let major = major.parse::<u8>().map_err(|_| malformed())?;
        let minor = minor.parse::<u8>().map_err(|_| malformed())?;
        ChipRevision::new(major, minor)
    }
}

impl fmt::Display for ChipRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_the_v_major_dot_minor_form() {
        let rev: ChipRevision = "v0.3".parse().unwrap();
        assert_eq!((rev.major(), rev.minor(), rev.full()), (0, 3, 3));
        assert_eq!(rev.to_string(), "v0.3");
        assert_eq!("v1.1".parse::<ChipRevision>().unwrap().full(), 101);
        assert_eq!(rev, ChipRevision::V0_3);
    }

    #[test]
    fn rejects_values_the_efuse_fields_cannot_hold() {
        assert!(ChipRevision::new(4, 0).is_err()); // WAFER_VERSION_MAJOR is 2 bits
        assert!(ChipRevision::new(0, 16).is_err()); // WAFER_VERSION_MINOR is 4 bits
        assert!(ChipRevision::new(3, 15).is_ok());
    }

    #[test]
    fn malformed_text_names_the_expected_form() {
        for text in ["0.3", "v3", "v0.x", "", "v0.3.1", "v-1.0"] {
            let err = text.parse::<ChipRevision>().unwrap_err().to_string();
            assert!(err.contains("v<major>.<minor>"), "{text:?}: {err}");
        }
    }

    #[test]
    fn revisions_order_by_major_then_minor() {
        assert!("v0.4".parse::<ChipRevision>().unwrap() < "v1.1".parse::<ChipRevision>().unwrap());
    }
}
