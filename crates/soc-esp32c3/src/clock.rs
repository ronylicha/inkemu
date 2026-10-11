//! The CPU clock tree, reduced to what software can select. Sources: reports BA §5 and BC §E.1
//! (ESP-IDF `system_reg.h`, `clk_tree_ll.h`).
use emu_core::Frequency;
use std::num::NonZeroU64;

const fn megahertz(mhz: u32) -> Frequency {
    // A widening cast: `u64::from` is not callable in a `const fn`.
    match NonZeroU64::new(mhz as u64 * 1_000_000) {
        Some(hz) => Frequency::new(hz),
        // INVARIANT: only called below with non-zero constants, at compile time.
        None => panic!("a clock frequency is never zero"),
    }
}

/// The Xteink boards carry a 40 MHz crystal. This is the one definition of it: the frequency
/// below, and the value `SYSCLK_CONF.CLK_XTAL_FREQ` reads (`periph/system.rs`), both come from it.
pub const XTAL_MHZ: u32 = 40;
pub const XTAL: Frequency = megahertz(XTAL_MHZ);
const PLL_CPU_80: Frequency = megahertz(80);
const PLL_CPU_160: Frequency = megahertz(160);

/// `SYSCLK_CONF` [9:0] `PRE_DIV_CNT`, [11:10] `SOC_CLK_SEL`.
const PRE_DIV_MASK: u32 = 0x3FF;
const SOC_CLK_SEL_SHIFT: u32 = 10;
const SOC_CLK_SEL_MASK: u32 = 0x3;
const SOC_CLK_XTAL: u32 = 0;
const SOC_CLK_PLL: u32 = 1;
/// `CPU_PER_CONF` [1:0] `CPUPERIOD_SEL`: 0 is 80 MHz, 1 is 160 MHz, whichever PLL frequency is selected.
const CPUPERIOD_SEL_MASK: u32 = 0x3;

/// The CPU clock frequency the two clock registers select, or `None` for a selection this model
/// has no frequency for (the RC oscillator, or a value ESP-IDF itself aborts on: BA §2.2).
///
/// ASSUMPTION: (decision D21) with the crystal as source the CPU runs at 40 MHz / (`PRE_DIV_CNT` + 1),
/// the formula of ESP-IDF's `clk_ll_cpu_get_divider`, hence **20 MHz at reset** (`PRE_DIV_CNT` = 1).
/// The mask ROM assumes 40 MHz (`ets_get_xtal_div` returns 1 unless `CLK_DIV_EN` is set): which one
/// silicon does is not established (BA §10 item 1). The firmware-visible cycle counter is the same
/// either way; only this conversion to time depends on it. A timestamped log of a real unit, or the
/// SP8 calibration bench, settles it.
#[must_use]
pub fn cpu_frequency(sysclk_conf: u32, cpu_per_conf: u32) -> Option<Frequency> {
    match (sysclk_conf >> SOC_CLK_SEL_SHIFT) & SOC_CLK_SEL_MASK {
        SOC_CLK_XTAL => {
            let divider = u64::from(sysclk_conf & PRE_DIV_MASK) + 1;
            Frequency::from_hz(XTAL.hz() / divider)
        }
        SOC_CLK_PLL => match cpu_per_conf & CPUPERIOD_SEL_MASK {
            0 => Some(PLL_CPU_80),
            1 => Some(PLL_CPU_160),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPU_PER_RESET: u32 = 0xC;
    fn hz(sysclk_conf: u32, cpu_per_conf: u32) -> Option<u64> {
        cpu_frequency(sysclk_conf, cpu_per_conf).map(|f| f.hz())
    }

    #[test]
    fn the_reset_state_runs_the_cpu_from_the_crystal_divided_by_two() {
        // Decision D21: PRE_DIV_CNT = 1 at reset, divider = PRE_DIV_CNT + 1.
        assert_eq!(hz(0x2_8001, CPU_PER_RESET), Some(20_000_000));
    }

    #[test]
    fn the_bootloader_sequence_ends_at_80_mhz_and_the_application_at_160() {
        assert_eq!(
            hz(0x2_8000, CPU_PER_RESET),
            Some(40_000_000),
            "PRE_DIV_CNT <- 0"
        );
        assert_eq!(
            hz(0x2_8400, CPU_PER_RESET),
            Some(80_000_000),
            "SOC_CLK_SEL <- 1 (BA §5 step 11)"
        );
        assert_eq!(
            hz(0x2_8400, CPU_PER_RESET | 1),
            Some(160_000_000),
            "CPUPERIOD_SEL <- 1"
        );
        assert_eq!(
            hz(0x2_8400, 0x1),
            Some(160_000_000),
            "the PLL at 320 MHz gives the same CPU clocks"
        );
    }

    #[test]
    fn the_crystal_divider_covers_its_whole_field() {
        assert_eq!(hz(0x2_83FF, CPU_PER_RESET), Some(40_000_000 / 1024));
    }

    #[test]
    fn selections_without_a_known_frequency_are_refused() {
        assert_eq!(hz(0x2_8800, CPU_PER_RESET), None, "SOC_CLK_SEL = 2");
        assert_eq!(
            hz(0x2_8C00, CPU_PER_RESET),
            None,
            "SOC_CLK_SEL = 3: ESP-IDF aborts"
        );
        assert_eq!(
            hz(0x2_8400, CPU_PER_RESET | 2),
            None,
            "PLL with CPUPERIOD_SEL >= 2"
        );
    }
}
