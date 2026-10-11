//! Run-time configuration of the virtual `SoC`.
use crate::chip::ChipRevision;

/// `GPIO_STRAP_REG` value the ROM prints as `boot:0x8 (SPI_FAST_FLASH_BOOT)`: bit 3 selects SPI
/// flash boot (report `docs/research/esp32c3-boot-chain.md`, §D.3).
pub const STRAP_SPI_FAST_FLASH_BOOT: u32 = 0x8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocConfig {
    /// The silicon revision the eFuses report.
    pub chip_revision: ChipRevision,
    /// Value read from `GPIO_STRAP_REG`.
    pub strapping: u32,
    /// Stop the machine at the first access to an unmodelled register (spec §5.2, `--strict-mmio`).
    pub strict_mmio: bool,
    /// Maintain the execution trace hash.
    pub trace_hash: bool,
}

impl Default for SocConfig {
    fn default() -> SocConfig {
        SocConfig {
            chip_revision: ChipRevision::V0_3,
            strapping: STRAP_SPI_FAST_FLASH_BOOT,
            strict_mmio: false,
            trace_hash: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_boots_from_spi_flash_on_revision_0_3_without_strictness() {
        let config = SocConfig::default();
        assert_eq!(config.chip_revision, ChipRevision::V0_3);
        assert_eq!(config.strapping, STRAP_SPI_FAST_FLASH_BOOT);
        assert!(!config.strict_mmio && !config.trace_hash);
    }
}
