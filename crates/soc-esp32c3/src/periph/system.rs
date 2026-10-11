//! `SYSTEM`: clock selection, peripheral clock gates and resets. Source: report §E.1.
//! The CPU clock selection is read by `crate::clock`; gates and resets are stored.
use crate::ctx::Ctx;
use crate::periph::{Accepted, SYSTEM_BASE, Storage};
use emu_core::Peripheral;

const SYSCLK_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "SYSCLK_CONF") - SYSTEM_BASE;
const CPU_PER_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;
/// `CPU_PERI_CLK_EN`, `CPU_PERI_RST_EN`, `PERIP_CLK_EN0/1`, `PERIP_RST_EN0/1`: gates and reset pulses the
/// ROM toggles around each peripheral it sets up. Nothing acts on them yet. `CPU_PER_CONF` is stored
/// as software wrote it; only its `CPUPERIOD_SEL` field acts, through `crate::clock`. `MEM_PD_MASK`
/// is read by `rtc_init`.
const ACCEPTED: [Accepted; 9] = [
    Accepted::svd(
        "SYSTEM",
        "CPU_PERI_CLK_EN",
        "clock gates of the CPU peripherals",
        "SP1b-3",
    ),
    Accepted::svd(
        "SYSTEM",
        "CPU_PERI_RST_EN",
        "resets of the CPU peripherals",
        "SP1b-3",
    ),
    Accepted::svd(
        "SYSTEM",
        "PERIP_CLK_EN0",
        "peripheral clock gates",
        "SP1b-3",
    ),
    Accepted::svd(
        "SYSTEM",
        "PERIP_CLK_EN1",
        "peripheral clock gates",
        "SP1b-3",
    ),
    Accepted::svd(
        "SYSTEM",
        "PERIP_RST_EN0",
        "peripheral reset pulses",
        "SP1b-3",
    ),
    Accepted::svd(
        "SYSTEM",
        "PERIP_RST_EN1",
        "peripheral reset pulses",
        "SP1b-3",
    ),
    Accepted::svd(
        "SYSTEM",
        "CACHE_CONTROL",
        "cache clock enable and reset",
        "SP1b-4",
    ),
    Accepted::svd(
        "SYSTEM",
        "CPU_PER_CONF",
        "PLL_FREQ_SEL and the CPU wait-mode bits (CPUPERIOD_SEL is used by the clock model)",
        "SP8",
    ),
    Accepted::svd(
        "SYSTEM",
        "MEM_PD_MASK",
        "memory power-down mask in light sleep",
        "SP1b-3",
    ),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// `SYSCLK_CONF`: bits [9:0] `PRE_DIV_CNT` and [11:10] `SOC_CLK_SEL` are writable; a write keeps
/// bits [11:0] only, because `CLK_XTAL_FREQ` [18:12] and `CLK_DIV_EN` [19] are read-only (report §E.1).
const SYSCLK_WRITABLE: u32 = 0xFFF;
/// Reset: `PRE_DIV_CNT` = 1, `SOC_CLK_SEL` = 0 (the crystal drives the CPU).
const SYSCLK_RESET: u32 = 1;
/// `CLK_XTAL_FREQ`, bits [18:12], read-only: the ROM derives its UART divider from it
/// (`ROM3:ets_get_apb_freq@0x40049866`). Its value is the crystal of the clock tree, in MHz.
const XTAL_FREQ_SHIFT: u32 = 12;
const XTAL_FREQ_MHZ: u32 = crate::clock::XTAL_MHZ;

#[derive(Clone, Debug)]
pub struct System {
    sysclk_writable: u32,
    registers: Storage,
    clock_changed: bool,
}

impl System {
    #[must_use]
    pub fn new() -> System {
        System {
            sysclk_writable: SYSCLK_RESET,
            registers: Storage::accepted(SYSTEM_BASE, &ACCEPTED),
            clock_changed: false,
        }
    }

    /// `SYSCLK_CONF` and `CPU_PER_CONF` as software reads them: the inputs of [`crate::clock::cpu_frequency`].
    #[must_use]
    pub fn clock_registers(&self) -> (u32, u32) {
        (
            self.sysclk_writable | (XTAL_FREQ_MHZ << XTAL_FREQ_SHIFT),
            // INVARIANT: `CPU_PER_CONF` is in `ACCEPTED`, which `registers` is built from, so the
            // read always answers and the fallback is never taken.
            self.registers.read(CPU_PER_CONF).unwrap_or(0),
        )
    }

    /// Whether a clock register was written since the last call.
    pub(crate) fn take_clock_change(&mut self) -> bool {
        std::mem::take(&mut self.clock_changed)
    }
}

impl Default for System {
    fn default() -> Self {
        System::new()
    }
}

impl Peripheral<Ctx> for System {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if offset == SYSCLK_CONF {
            return Some(self.clock_registers().0);
        }
        self.registers.read(offset)
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        match offset {
            SYSCLK_CONF => {
                self.sysclk_writable = value & SYSCLK_WRITABLE;
                self.clock_changed = true;
                true
            }
            CPU_PER_CONF => {
                self.clock_changed = true;
                self.registers.write(offset, value)
            }
            _ => self.registers.write(offset, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn sysclk_conf_reports_a_40_mhz_crystal_selected_undivided() {
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        let conf = sys.read(ctx, 0x58).unwrap();
        assert_eq!((conf >> 12) & 0x7F, 40, "CLK_XTAL_FREQ");
        assert_eq!((conf >> 10) & 3, 0, "SOC_CLK_SEL = XTAL");
        assert_eq!(conf & 0x3FF, 1, "PRE_DIV_CNT");
    }

    #[test]
    fn cache_control_is_storage_from_its_reset_value() {
        // CM §2.1: reset 5; ROM_Boot_Cache_Init does |=1, |=2, &=~2.
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        let offset = svd_regs::esp32c3::addr("SYSTEM", "CACHE_CONTROL") - SYSTEM_BASE;
        assert_eq!(sys.read(ctx, offset), Some(5));
        assert!(sys.write(ctx, offset, 7));
        assert_eq!(sys.read(ctx, offset), Some(7));
    }

    #[test]
    fn crystal_frequency_is_read_only() {
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        assert!(sys.write(ctx, 0x58, 0xFFFF_FFFF));
        let conf = sys.read(ctx, 0x58).unwrap();
        assert_eq!((conf >> 12) & 0x7F, 40);
        assert_eq!(conf & 0xFFF, 0xFFF, "divider and clock select are writable");
        assert_eq!(conf >> 19, 0, "CLK_DIV_EN and above stay clear");
    }

    #[test]
    fn clock_gates_and_resets_are_storage_from_their_reset_values() {
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        for offset in [0x00, 0x04, 0x10, 0x14, 0x18, 0x1C] {
            let reset = svd_regs::esp32c3::lookup(0x600C_0000 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(sys.read(ctx, offset), Some(reset), "{offset:#x}");
            assert!(sys.write(ctx, offset, reset | 4));
            assert_eq!(sys.read(ctx, offset), Some(reset | 4));
        }
        assert_eq!(sys.read(ctx, 0x08), Some(0xC), "CPU_PER_CONF reset");
    }

    #[test]
    fn clock_writes_are_flagged_once_and_the_registers_read_as_software_sees_them() {
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        assert_eq!(sys.clock_registers(), (0x2_8001, 0xC), "reset: BA §5");
        assert!(!sys.take_clock_change());
        let cpu_per_conf = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;
        assert!(sys.write(ctx, cpu_per_conf, 0xD));
        assert!(sys.take_clock_change());
        assert!(!sys.take_clock_change(), "taken once");
        sys.write(ctx, SYSCLK_CONF, 0x400);
        assert!(sys.take_clock_change());
        assert_eq!(sys.clock_registers(), (0x2_8400, 0xD));
        sys.write(
            ctx,
            svd_regs::esp32c3::addr("SYSTEM", "PERIP_CLK_EN0") - SYSTEM_BASE,
            0,
        );
        assert!(
            !sys.take_clock_change(),
            "a clock gate is not the CPU clock"
        );
    }
}
