//! Run-time configuration of the virtual `SoC`.
use crate::chip::ChipRevision;
use crate::flash::FlashConfig;

/// Seed of the hardware random number generator model (decision D8): fixed, so two runs are identical.
/// The bytes spell "inkemu"; any non-zero value would do.
pub const DEFAULT_RNG_SEED: u64 = 0x696E_6B65_6D75_0001;

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
    /// The SPI flash chip.
    pub flash: FlashConfig,
    /// Seed of the random number generator (`APB_CTRL.RND_DATA`).
    pub rng_seed: u64,
}

impl Default for SocConfig {
    fn default() -> SocConfig {
        SocConfig {
            chip_revision: ChipRevision::V0_3,
            strapping: STRAP_SPI_FAST_FLASH_BOOT,
            strict_mmio: false,
            trace_hash: false,
            flash: FlashConfig::default(),
            rng_seed: DEFAULT_RNG_SEED,
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
        assert_eq!(config.flash, crate::flash::FlashConfig::default());
        assert_eq!(config.rng_seed, DEFAULT_RNG_SEED);
        assert_ne!(DEFAULT_RNG_SEED, 0);
    }
}
