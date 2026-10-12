//! The device a flash is composed for, as the checks of a bootloader and of an application see it.
use crate::chip::{ChipId, FlashSize};

/// `efuse_hal_chip_revision()` and the image headers count a revision `major * 100 + minor`.
const MAJOR_WEIGHT: u16 = 100;

/// What a bootloader and an application are checked against before a flash is composed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub chip: ChipId,
    /// The chip revision, `major * 100 + minor`.
    pub chip_revision: u16,
    /// The eFuse block revision the chip reports, `major * 100 + minor`.
    pub efuse_block_revision: u16,
    pub flash_size: FlashSize,
}

/// `v<major>.<minor>` of a revision counted `major * 100 + minor`, as ESP-IDF prints it.
#[must_use]
pub fn revision(full: u16) -> String {
    format!("v{}.{}", full / MAJOR_WEIGHT, full % MAJOR_WEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revision_prints_as_esp_idf_prints_it() {
        assert_eq!(revision(3), "v0.3");
        assert_eq!(revision(101), "v1.1");
        assert_eq!(revision(199), "v1.99");
    }
}
