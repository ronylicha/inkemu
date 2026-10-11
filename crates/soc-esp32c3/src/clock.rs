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

/// `SYSCLK_CONF` [9:0] `PRE_DIV_CNT`, [11:10] `SOC_CLK_SEL`.
const PRE_DIV_MASK: u32 = 0x3FF;
const SOC_CLK_SEL_SHIFT: u32 = 10;
const SOC_CLK_SEL_MASK: u32 = 0x3;
const SOC_CLK_XTAL: u32 = 0;
const SOC_CLK_PLL: u32 = 1;
/// `CPU_PER_CONF` [1:0] `CPUPERIOD_SEL`: 0 is 80 MHz, 1 is 160 MHz, whichever PLL frequency is selected.
const CPUPERIOD_SEL_MASK: u32 = 0x3;

/// The time base of the machine: device time is counted in cycles of this clock (ADR 0003).
///
/// 160 MHz is the fastest clock software can select, and every clock this model counts with is a
/// whole number of its cycles: the CPU at 80 MHz (2), the crystal (4) and its divisions (4 × n), the
/// SYSTIMER counters at 16 MHz (10). One cycle lasts 6.25 ns. An event scheduled at a device time
/// therefore stays at its instant whatever the CPU clock becomes (decision E2).
pub const TIME_BASE: Frequency = megahertz(160);
/// One second of device time, in cycles of the time base.
pub const SECOND: u64 = TIME_BASE.hz();
/// Time-base cycles in one cycle of the 40 MHz crystal.
pub const XTAL_PERIOD: u64 = 4;
// The crystal is a whole number of time-base cycles, and this is that number.
const _: () = assert!(TIME_BASE.hz() == XTAL_PERIOD * XTAL.hz());
const PLL_80_PERIOD: u64 = 2;
const PLL_160_PERIOD: u64 = 1;

/// The two clocks the selection registers decide, each as its period in time-base cycles. A
/// period is never 0: the fields are private so that only [`CpuClock::new`] builds one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuClock {
    cpu_period: u64,
    apb_period: u64,
}

impl CpuClock {
    /// The clocks after a power-on reset, and after any core or system reset: the crystal divided
    /// by 2, 20 MHz (TRM §6.2.4.1; report TW §H.2: the period of the reset loop of a blank chip
    /// excludes 40 MHz). A CPU reset keeps the selection the firmware made: after `esp_restart`
    /// the ROM runs on the undivided crystal, 40 MHz (IR §4.1). The 20 MHz is an assumption, not
    /// a measured fact: it is marked, with what settles it, in the documentation of [`cpu_clock`].
    pub const POWER_ON: CpuClock = CpuClock::new(2 * XTAL_PERIOD, 2 * XTAL_PERIOD);

    /// A CPU clock and an APB clock by their periods in time-base cycles; a period of 0 is taken
    /// as 1, the shortest there is.
    #[must_use]
    pub const fn new(cpu_period: u64, apb_period: u64) -> CpuClock {
        CpuClock {
            cpu_period: if cpu_period == 0 { 1 } else { cpu_period },
            apb_period: if apb_period == 0 { 1 } else { apb_period },
        }
    }

    /// Time-base cycles in one CPU clock cycle; at least 1.
    #[must_use]
    pub const fn cpu_period(self) -> u64 {
        self.cpu_period
    }

    /// Time-base cycles in one `APB_CLK` cycle; at least 1. `APB_CLK` is 80 MHz when the CPU runs
    /// from the PLL and equals `CPU_CLK` otherwise (TRM table 6.2-4; report TW §B.2).
    #[must_use]
    pub const fn apb_period(self) -> u64 {
        self.apb_period
    }

    /// The CPU clock frequency, rounded down to a whole number of hertz.
    #[must_use]
    pub fn cpu_frequency(self) -> Frequency {
        // INVARIANT: `cpu_period` is at least 1 (`CpuClock::new`), so the division is defined. A
        // period the clock registers select is at most 4 × 1024 (the crystal divider field is 10
        // bits wide), so the quotient is at least 39 062 Hz and the fallback is never taken; a
        // period longer than a second, which only a caller of `new` could ask for, reads as the
        // time base rather than as 0 Hz.
        Frequency::from_hz(TIME_BASE.hz() / self.cpu_period).unwrap_or(TIME_BASE)
    }
}

/// The clocks the two clock registers select, or `None` for a selection this model has no
/// frequency for (the RC oscillator, or a value ESP-IDF itself aborts on: BA §2.2).
///
/// With the crystal as source the CPU runs at 40 MHz / (`PRE_DIV_CNT` + 1), the formula of ESP-IDF's
/// `clk_ll_cpu_get_divider`, hence **20 MHz after a power-on reset** (`PRE_DIV_CNT` = 1): TRM
/// §6.2.4.1 says the crystal is divided by 2 by default, and a reset-loop log of a real chip excludes
/// 40 MHz for the clock of the flash-boot watchdog (report TW §1 result 3, §H.2). The mask ROM itself
/// assumes 40 MHz (`ets_get_xtal_div` returns 1 unless `CLK_DIV_EN` is set, BA §10 item 1): it is
/// wrong about its own speed after power-on, and right after `esp_restart`, which leaves
/// `PRE_DIV_CNT` = 0 across the CPU reset (report IR §4.1).
///
/// ASSUMPTION: the lower bound measured on silicon (0.955 s between two watchdog resets, TW §H.2) is
/// compatible with 20 MHz and excludes 40 MHz, but it is one log; the SP8 calibration bench, or a
/// timestamped boot log of a real unit, settles the exact figure.
#[must_use]
pub fn cpu_clock(sysclk_conf: u32, cpu_per_conf: u32) -> Option<CpuClock> {
    match (sysclk_conf >> SOC_CLK_SEL_SHIFT) & SOC_CLK_SEL_MASK {
        SOC_CLK_XTAL => {
            let period = XTAL_PERIOD * (u64::from(sysclk_conf & PRE_DIV_MASK) + 1);
            Some(CpuClock::new(period, period))
        }
        SOC_CLK_PLL => {
            let cpu_period = match cpu_per_conf & CPUPERIOD_SEL_MASK {
                0 => PLL_80_PERIOD,
                1 => PLL_160_PERIOD,
                _ => return None,
            };
            Some(CpuClock::new(cpu_period, PLL_80_PERIOD))
        }
        _ => None,
    }
}

/// The CPU clock frequency the two clock registers select: [`cpu_clock`] as a frequency.
#[must_use]
pub fn cpu_frequency(sysclk_conf: u32, cpu_per_conf: u32) -> Option<Frequency> {
    cpu_clock(sysclk_conf, cpu_per_conf).map(CpuClock::cpu_frequency)
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
    fn every_selection_is_a_whole_number_of_time_base_cycles() {
        // The reason the time base is 160 MHz: no selectable clock needs a fraction of its cycle.
        assert_eq!(TIME_BASE.hz(), 160_000_000);
        assert_eq!(
            cpu_clock(0x2_8001, CPU_PER_RESET),
            Some(CpuClock::POWER_ON),
            "power-on: 20 MHz, 8 time-base cycles per CPU and APB cycle"
        );
        assert_eq!(CpuClock::POWER_ON.cpu_period(), 8);
        let periods = |sysclk, cpu_per| {
            let clock = cpu_clock(sysclk, cpu_per).unwrap();
            (clock.cpu_period(), clock.apb_period())
        };
        assert_eq!(periods(0x2_8000, 0xC), (4, 4));
        assert_eq!(periods(0x2_8400, 0xC), (2, 2));
        // At 160 MHz the APB clock stays at 80 MHz (TRM table 6.2-4).
        assert_eq!(periods(0x2_8400, 0xD), (1, 2));
        assert_eq!(periods(0x2_83FF, 0xC), (4 * 1024, 4 * 1024));
        let clock = |sysclk, cpu_per| cpu_clock(sysclk, cpu_per).unwrap();
        assert_eq!(
            clock(0x2_8002, 0xC).cpu_frequency().hz(),
            13_333_333,
            "40 MHz / 3, rounded down"
        );
    }

    #[test]
    fn a_clock_never_has_a_period_of_zero() {
        // No selection of the registers gives one, and a caller that asks for one gets the
        // shortest period there is: `cpu_frequency` divides by it.
        let clock = CpuClock::new(0, 0);
        assert_eq!((clock.cpu_period(), clock.apb_period()), (1, 1));
        assert_eq!(clock.cpu_frequency(), TIME_BASE);
        assert_eq!(SECOND, 160_000_000);
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
