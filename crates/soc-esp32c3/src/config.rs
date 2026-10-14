//! Run-time configuration of the virtual `SoC`.
use crate::chip::ChipRevision;
use crate::flash::FlashConfig;
use crate::periph::UsbHost;

/// Seed of the hardware random number generator model (decision D8): fixed, so two runs are identical.
/// The bytes spell "inkemu"; any non-zero value would do.
pub const DEFAULT_RNG_SEED: u64 = 0x696E_6B65_6D75_0001;

/// `GPIO_STRAP_REG` value the ROM prints as `boot:0x8 (SPI_FAST_FLASH_BOOT)`: bit 3 selects SPI
/// flash boot (report `docs/research/esp32c3-boot-chain.md`, §D.3).
pub const STRAP_SPI_FAST_FLASH_BOOT: u32 = 0x8;

/// The model that turns instructions into device time (spec §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Timing {
    /// The cost of each class of instruction in CPU cycles, at the clock the firmware selected
    /// (`DatasheetTiming`), and the 4-cycle latency of the interrupt controller.
    #[default]
    Datasheet,
    /// The time Espressif's QEMU gives with `-icount 3`: 8 ns per instruction whatever its class
    /// and the clock, and no interrupt latency (decision H2). For the lockstep against QEMU only:
    /// it describes QEMU, not the chip.
    QemuIcount,
}

impl Timing {
    /// The name a front end shows.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Timing::Datasheet => "datasheet-uncalibrated",
            Timing::QemuIcount => "qemu-icount",
        }
    }

    /// CPU cycles between the instant the interrupt controller selects a line and the instant
    /// the CPU can take it, under this model: the TRM's 4 cycles
    /// ([`crate::machine::INTERRUPT_LATENCY_CYCLES`]), or none, as in QEMU, whose interrupt
    /// matrix raises the CPU line in the same call that changes it.
    #[must_use]
    pub const fn interrupt_latency_cycles(self) -> u64 {
        match self {
            Timing::Datasheet => crate::machine::INTERRUPT_LATENCY_CYCLES,
            Timing::QemuIcount => 0,
        }
    }
}

/// Behaviours of Espressif's QEMU that the chip does not have, which the machine can take on for
/// the lockstep against QEMU and for nothing else (decision H3, treatment iv): each one is a row
/// of `docs/lockstep-differences.md`, chosen because the difference changes the program's
/// control flow and neither a mask, nor a value copied from QEMU, nor a resynchronisation keeps
/// the comparison going. All are off by default, and no front end of the emulator turns one on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QemuCompat {
    /// A SYSTIMER comparator in target mode raises no alarm while its counter is stopped: the
    /// early tick interrupt of `vSystimerSetup` is not taken (report TW §A.4, §G row 10).
    pub systimer_no_alarm_on_stopped_counter: bool,
}

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
    /// The USB host on the USB-Serial-JTAG port when the chip starts (ADR 0006): an input of
    /// the run, open by default.
    pub usb_host: UsbHost,
    /// The model that turns instructions into device time.
    pub timing: Timing,
    /// CPU cycles between the selection of an interrupt line and the instant the CPU can take
    /// it; `None` is the latency of [`SocConfig::timing`] ([`Timing::interrupt_latency_cycles`]).
    pub interrupt_latency_cycles: Option<u64>,
    /// The behaviours of QEMU the machine takes on, for the lockstep only: none by default.
    pub qemu_compat: QemuCompat,
}

impl SocConfig {
    /// The interrupt latency this configuration gives the machine, in CPU cycles.
    #[must_use]
    pub fn interrupt_latency(&self) -> u64 {
        self.interrupt_latency_cycles
            .unwrap_or(self.timing.interrupt_latency_cycles())
    }
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
            usb_host: UsbHost::Open,
            timing: Timing::Datasheet,
            interrupt_latency_cycles: None,
            qemu_compat: QemuCompat::default(),
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
        assert_eq!(config.usb_host, UsbHost::Open, "report USJ §R.2");
        assert_eq!(config.timing, Timing::Datasheet);
        assert_eq!(config.interrupt_latency(), 4, "TRM §1.5.3.1");
        assert_eq!(config.qemu_compat, QemuCompat::default());
        assert!(!config.qemu_compat.systimer_no_alarm_on_stopped_counter);
    }

    #[test]
    fn the_qemu_icount_model_has_no_interrupt_latency_unless_one_is_set() {
        let qemu = SocConfig {
            timing: Timing::QemuIcount,
            ..SocConfig::default()
        };
        assert_eq!(qemu.interrupt_latency(), 0);
        assert_eq!(qemu.timing.name(), "qemu-icount");
        let set = SocConfig {
            interrupt_latency_cycles: Some(7),
            ..qemu
        };
        assert_eq!(set.interrupt_latency(), 7);
    }
}
