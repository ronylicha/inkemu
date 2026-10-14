//! The slow-clock calibration of a timer group: `RTCCALICFG`, `RTCCALICFG1` and `RTCCALICFG2`,
//! which count crystal cycles during a number of cycles of the RTC slow clock. Sources: TRM
//! chapter 11, ESP-IDF `rtc_time.c`; report TW §B.4.
use crate::clock::XTAL_PERIOD;
use emu_core::Cycles;

/// `RTCCALICFG`: bit 31 `START`, bits [30:16] `MAX`, bit 15 `RDY` (read-only), bits [14:13]
/// `CLK_SEL`, bit 12 `START_CYCLING`. `rtc_clk_cal_internal` writes each of them
/// (`IDF:esp_hw_support/port/esp32c3/rtc_time.c:36-128`; TW §B.4).
pub(super) const CALI_START: u32 = 1 << 31;
pub(super) const CALI_MAX_SHIFT: u32 = 16;
const CALI_MAX_MASK: u32 = 0x7FFF;
pub(super) const CALI_RDY: u32 = 1 << 15;
pub(super) const CALI_CLK_SEL_SHIFT: u32 = 13;
const CALI_CLK_SEL_MASK: u32 = 0b11;
const CALI_START_CYCLING: u32 = 1 << 12;
const RTCCALICFG_STORED: u32 = 0xFFFF_7000;
/// `RTCCALICFG1` bits [31:7] `VALUE`: the crystal cycles counted.
pub(super) const CALI_VALUE_SHIFT: u32 = 7;
/// `RTCCALICFG2`: bits [31:7] `TIMEOUT_THRES`, bits [6:3] `TIMEOUT_RST_CNT`, bit 0 `TIMEOUT`
/// (read-only).
pub(super) const CALI_THRES_SHIFT: u32 = 7;
pub(super) const CALI_TIMEOUT: u32 = 1;
const RTCCALICFG2_STORED: u32 = 0xFFFF_FFF8;
const RTCCALICFG_RESET: u32 = 0x0001_3000;
const RTCCALICFG2_RESET: u32 = 0xFFFF_FF98;

/// The clock a calibration measures, as crystal cycles per slow-clock cycle (a fraction): the
/// nominal frequencies of `IDF:soc/esp32c3/include/soc/clk_tree_defs.h:39-42` (TW §B.4).
///
/// ASSUMPTION: the RC oscillators run at their nominal frequency. A real chip deviates from it, by
/// an amount no source gives (TW §I question 2); the calibration value of a real unit
/// (`RTC_CNTL.STORE1` after boot) settles it, and belongs in the device profile then.
const fn crystal_cycles_per_slow_cycle(clk_sel: u32) -> Option<(u64, u64)> {
    match clk_sel {
        // RC_SLOW, 136 kHz: 40 000 000 / 136 000 = 5 000 / 17.
        0 => Some((5_000, 17)),
        // RC_FAST divided by 256, 17.5 MHz / 256: 40 000 000 * 256 / 17 500 000 = 4 096 / 7.
        //
        // ASSUMPTION: this clock is measured whatever `RTC_CNTL.CLK_CONF` says about its enable
        // (`rtc_clk_cal_internal` switches it on first: `IDF:esp_hw_support/port/esp32c3/rtc_time.c:62`;
        // TW §B.4). With the oscillator off, a chip may count nothing and time out. Calibrating
        // RC_FAST with its enable cleared, and reading `RDY` and `TIMEOUT`, settles it.
        1 => Some((4_096, 7)),
        // ASSUMPTION: the 32 kHz crystal (selection 2), which no board of this plan fits (see
        // `Calibration::start`), and selection 3, which `rtc_cal_sel_t` does not define, both
        // measure nothing and end in `TIMEOUT`. A calibration of selection 3 on a chip, and of
        // selection 2 on a board with and one without the crystal, settles it.
        _ => None,
    }
}

/// One measurement of the slow clock against the crystal.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Measurement {
    /// Crystal cycles counted during `MAX` slow-clock cycles.
    pub(super) value: u32,
    /// Device time from which `RDY` reads 1, if the measurement completes.
    pub(super) ready_at: Option<Cycles>,
    /// Device time from which `TIMEOUT` reads 1, if it gives up.
    timeout_at: Option<Cycles>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Calibration {
    config: u32,
    config2: u32,
    pub(super) measurement: Measurement,
}

impl Calibration {
    pub(super) const fn new() -> Calibration {
        Calibration {
            config: RTCCALICFG_RESET,
            config2: RTCCALICFG2_RESET,
            measurement: Measurement {
                value: 0,
                ready_at: None,
                timeout_at: None,
            },
        }
    }

    fn max(&self) -> u64 {
        u64::from((self.config >> CALI_MAX_SHIFT) & CALI_MAX_MASK)
    }

    fn threshold(&self) -> u64 {
        u64::from(self.config2 >> CALI_THRES_SHIFT)
    }

    /// Crystal cycles the selected clock takes for `MAX` of its cycles, when the clock exists.
    fn crystal_cycles(&self) -> Option<u64> {
        let (numerator, denominator) =
            crystal_cycles_per_slow_cycle((self.config >> CALI_CLK_SEL_SHIFT) & CALI_CLK_SEL_MASK)?;
        Some(self.max() * numerator / denominator)
    }

    /// `START` went from 0 to 1: a one-shot measurement begins (TW §B.4 step 4).
    ///
    /// It counts crystal cycles during `MAX` cycles of the selected clock: the result is known
    /// from the two frequencies, and it is ready when those `MAX` cycles have gone by. A count
    /// that would pass `TIMEOUT_THRES` first ends in `TIMEOUT` instead, `RDY` staying clear.
    ///
    /// ASSUMPTION: `TIMEOUT_THRES` is in crystal cycles, and no 32 kHz crystal is fitted, so a
    /// measurement of that clock always ends in `TIMEOUT` (ESP-IDF then falls back to the RC
    /// oscillator and says so: `IDF:esp_system/port/soc/esp32c3/clk.c:140-199`). Neither is
    /// documented (TW §I question 7); the Xteink boards carry no such crystal as far as the
    /// hardware report knows. A board that has one needs a profile entry.
    fn start(&mut self, now: Cycles) {
        let threshold = self.threshold();
        self.measurement = match self.crystal_cycles() {
            Some(cycles) if cycles <= threshold => Measurement {
                value: u32::try_from(cycles).unwrap_or(u32::MAX),
                ready_at: Some(now + cycles.saturating_mul(XTAL_PERIOD)),
                timeout_at: None,
            },
            _ => Measurement {
                value: 0,
                ready_at: None,
                timeout_at: Some(now + threshold.saturating_mul(XTAL_PERIOD)),
            },
        };
    }

    pub(super) fn read_config(&self, now: Cycles) -> u32 {
        let ready = self.measurement.ready_at.is_some_and(|at| at <= now);
        self.config | if ready { CALI_RDY } else { 0 }
    }

    pub(super) fn read_config2(&self, now: Cycles) -> u32 {
        let timed_out = self.measurement.timeout_at.is_some_and(|at| at <= now);
        self.config2 | if timed_out { CALI_TIMEOUT } else { 0 }
    }

    pub(super) fn write_config(&mut self, value: u32, now: Cycles) {
        let started = value & CALI_START != 0 && self.config & CALI_START == 0;
        self.config = value & RTCCALICFG_STORED;
        if started {
            self.start(now);
        }
    }

    /// ASSUMPTION: while the cycling mode is on (it is at reset), lowering `TIMEOUT_THRES` below
    /// what the running measurement needs makes `TIMEOUT` read 1 at once. `rtc_clk_cal_internal`
    /// does exactly that to get out of the cycling mode, with a threshold of 1, and then waits
    /// for `RDY` or `TIMEOUT` without a time limit (`rtc_time.c`, comment "accelerate the
    /// generation of timeout"; TW §B.4 step 1, §I question 7). What the cycling mode itself
    /// measures is not modelled: `RTCCALICFG1.CYCLING_DATA_VLD` reads 0.
    pub(super) fn write_config2(&mut self, value: u32, now: Cycles) {
        self.config2 = value & RTCCALICFG2_STORED;
        let cycling = self.config & CALI_START_CYCLING != 0;
        if cycling
            && self
                .crystal_cycles()
                .is_none_or(|cycles| cycles > self.threshold())
        {
            self.measurement.ready_at = None;
            self.measurement.timeout_at = Some(now);
        }
    }
}
