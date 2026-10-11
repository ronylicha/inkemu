//! `SYSTEM`: clock selection, peripheral clock gates and resets. Source: report §E.1.
//! The clock tree itself (PLL, CPU frequency switch) arrives in SP1b-2.
use crate::periph::{Accepted, SYSTEM_BASE, Storage};
use emu_core::Peripheral;

const SYSCLK_CONF: u32 = 0x58;
/// `CPU_PERI_CLK_EN`, `CPU_PERI_RST_EN`, `PERIP_CLK_EN0/1`, `PERIP_RST_EN0/1`: gates and reset pulses the
/// ROM toggles around each peripheral it sets up. Nothing acts on them yet.
const ACCEPTED: [Accepted; 6] = [
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
/// (`ROM3:ets_get_apb_freq@0x40049866`). The Xteink boards carry a 40 MHz crystal.
const XTAL_FREQ_SHIFT: u32 = 12;
const XTAL_FREQ_MHZ: u32 = 40;

#[derive(Clone, Debug)]
pub struct System {
    sysclk_writable: u32,
    gates: Storage,
}

impl System {
    #[must_use]
    pub fn new() -> System {
        System {
            sysclk_writable: SYSCLK_RESET,
            gates: Storage::accepted(SYSTEM_BASE, &ACCEPTED),
        }
    }
}

impl Default for System {
    fn default() -> Self {
        System::new()
    }
}

impl Peripheral for System {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == SYSCLK_CONF {
            return Some(self.sysclk_writable | (XTAL_FREQ_MHZ << XTAL_FREQ_SHIFT));
        }
        self.gates.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if offset == SYSCLK_CONF {
            self.sysclk_writable = value & SYSCLK_WRITABLE;
            return true;
        }
        self.gates.write(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn sysclk_conf_reports_a_40_mhz_crystal_selected_undivided() {
        let mut sys = System::new();
        let conf = sys.read(0x58).unwrap();
        assert_eq!((conf >> 12) & 0x7F, 40, "CLK_XTAL_FREQ");
        assert_eq!((conf >> 10) & 3, 0, "SOC_CLK_SEL = XTAL");
        assert_eq!(conf & 0x3FF, 1, "PRE_DIV_CNT");
    }

    #[test]
    fn crystal_frequency_is_read_only() {
        let mut sys = System::new();
        assert!(sys.write(0x58, 0xFFFF_FFFF));
        let conf = sys.read(0x58).unwrap();
        assert_eq!((conf >> 12) & 0x7F, 40);
        assert_eq!(conf & 0xFFF, 0xFFF, "divider and clock select are writable");
        assert_eq!(conf >> 19, 0, "CLK_DIV_EN and above stay clear");
    }

    #[test]
    fn clock_gates_and_resets_are_storage_from_their_reset_values() {
        let mut sys = System::new();
        for offset in [0x00, 0x04, 0x10, 0x14, 0x18, 0x1C] {
            let reset = svd_regs::esp32c3::lookup(0x600C_0000 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(sys.read(offset), Some(reset), "{offset:#x}");
            assert!(sys.write(offset, reset | 4));
            assert_eq!(sys.read(offset), Some(reset | 4));
        }
        assert_eq!(
            sys.read(0x08),
            None,
            "CPU_PER_CONF belongs to the clock tree (SP1b-2)"
        );
    }
}
