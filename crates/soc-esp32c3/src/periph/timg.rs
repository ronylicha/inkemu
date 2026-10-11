//! `TIMG0` and `TIMG1`: each timer group has a general-purpose timer T0, a main watchdog (MWDT)
//! and the registers of the slow-clock calibration. Sources: TRM chapters 11 and 12, ESP-IDF
//! `timer_group_reg.h`, `timer_ll.h`, `mwdt_ll.h`, `rtc_time.c`; report TW §B and §D.
use super::counter::{Counter, Rate};
use super::{TIMG0_BASE, TIMG1_BASE};
use super::{half_of, replace_half};
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event, Group, Source};
use crate::reset::ResetCause;
use emu_core::{Cycles, Peripheral};
use svd_regs::esp32c3::addr;

/// The two groups have the same registers at the same offsets: those of `TIMG0` name them.
const fn reg(name: &str) -> u32 {
    addr("TIMG0", name) - TIMG0_BASE
}
const _: () = assert!(addr("TIMG1", "REGCLK") - TIMG1_BASE == reg("REGCLK"));

const T0CONFIG: u32 = reg("T0CONFIG");
const T0LO: u32 = reg("T0LO");
const T0HI: u32 = reg("T0HI");
const T0UPDATE: u32 = reg("T0UPDATE");
const T0ALARMLO: u32 = reg("T0ALARMLO");
const T0ALARMHI: u32 = reg("T0ALARMHI");
const T0LOADLO: u32 = reg("T0LOADLO");
const T0LOADHI: u32 = reg("T0LOADHI");
const T0LOAD: u32 = reg("T0LOAD");
const WDTCONFIG0: u32 = reg("WDTCONFIG0");
const WDTCONFIG1: u32 = reg("WDTCONFIG1");
const WDTCONFIG2: u32 = reg("WDTCONFIG2");
const WDTCONFIG5: u32 = reg("WDTCONFIG5");
const WDTFEED: u32 = reg("WDTFEED");
const WDTWPROTECT: u32 = reg("WDTWPROTECT");
const RTCCALICFG: u32 = reg("RTCCALICFG");
const RTCCALICFG1: u32 = reg("RTCCALICFG1");
const INT_ENA: u32 = reg("INT_ENA_TIMERS");
const INT_RAW: u32 = reg("INT_RAW_TIMERS");
const INT_ST: u32 = reg("INT_ST_TIMERS");
const INT_CLR: u32 = reg("INT_CLR_TIMERS");
const RTCCALICFG2: u32 = reg("RTCCALICFG2");
const NTIMG_DATE: u32 = reg("NTIMG_DATE");
const REGCLK: u32 = reg("REGCLK");

// Reset values: `timer_group_reg.h`, tabulated in TW §B.1.
const T0CONFIG_RESET: u32 = 0x6000_2000;
/// `FLASHBOOT_MOD_EN` set, both reset lengths 1: the flash-boot protection is armed.
const WDTCONFIG0_RESET: u32 = 0x0004_C000;
const WDTCONFIG1_RESET: u32 = 0x0001_0000;
/// Stage timeouts 0 to 3: 26 000 000, 134 217 727, 1 048 575, 1 048 575 counts.
const STAGE_TIMEOUT_RESET: [u32; STAGES] = [26_000_000, 0x07FF_FFFF, 0x000F_FFFF, 0x000F_FFFF];
const RTCCALICFG_RESET: u32 = 0x0001_3000;
const RTCCALICFG2_RESET: u32 = 0xFFFF_FF98;
const NTIMG_DATE_VALUE: u32 = 0x0200_6191;
const REGCLK_RESET: u32 = 0x6000_0000;

/// `T0CONFIG`: bit 31 `EN` (`timer_ll_enable_counter`, `IDF:hal/esp32c3/include/hal/timer_ll.h:175`),
/// bit 30 `INCREASE` (`timer_ll_set_count_direction`, line 161), bit 29 `AUTORELOAD`
/// (`timer_ll_enable_auto_reload`, line 149), bits [28:13] `DIVIDER` and bit 12 `DIVCNT_RST`, a
/// write-1 trigger (`timer_ll_set_clock_prescale`, lines 130-137), bit 10 `ALARM_EN`
/// (`timer_ll_enable_alarm`, line 118), bit 9 `USE_XTAL` (`timer_ll_set_clock_source`, line 79).
const T0_EN: u32 = 1 << 31;
const T0_INCREASE: u32 = 1 << 30;
const T0_AUTORELOAD: u32 = 1 << 29;
const T0_DIVIDER_SHIFT: u32 = 13;
const T0_DIVIDER_MASK: u32 = 0xFFFF;
const T0_ALARM_EN: u32 = 1 << 10;
const T0_DIVCNT_RST: u32 = 1 << 12;
const T0_USE_XTAL: u32 = 1 << 9;
const T0CONFIG_STORED: u32 = 0xFFFF_E600;
/// The counter, its alarm and its reload value are 54 bits wide: 22 bits in the `HI` registers.
const T0_MASK: u64 = (1 << 54) - 1;
const T0_HI_MASK: u32 = 0x003F_FFFF;
/// "An alarm value already passed (within 2^53) fires immediately" (TRM §11.2; TW §B.2).
const T0_PASSED_WINDOW: u64 = 1 << 53;

/// `WDTCONFIG0`: bit 31 `EN` (`mwdt_ll_enable`, `IDF:hal/esp32c3/include/hal/mwdt_ll.h:65`), two
/// bits per stage from [30:29] for stage 0 to [24:23] for stage 3 (`mwdt_ll_config_stage`, line
/// 102), bit 22 `CONF_UPDATE_EN`, a write-1 trigger that reads 0 (same function), bit 21 `USE_XTAL`
/// (`mwdt_ll_set_clock_source`, line 277), bit 14 `FLASHBOOT_MOD_EN` (`mwdt_ll_set_flashboot_en`,
/// line 194). The reset lengths and the two CPU reset enables are stored.
const WDT_EN: u32 = 1 << 31;
const WDT_STAGE0_SHIFT: u32 = 29;
const WDT_STAGE_BITS: u32 = 2;
const WDT_STAGE_MASK: u32 = 0b11;
const WDT_CONF_UPDATE_EN: u32 = 1 << 22;
const WDT_USE_XTAL: u32 = 1 << 21;
const WDT_FLASHBOOT_MOD_EN: u32 = 1 << 14;
const WDTCONFIG0_STORED: u32 = 0xFFBF_F000;
/// `WDTCONFIG1` bits [31:16] `CLK_PRESCALE` (`mwdt_ll_set_prescaler`, line 207); bit 0 is a
/// write-1 trigger that restarts the prescaler.
const WDT_PRESCALE_SHIFT: u32 = 16;
const WDTCONFIG1_STORED: u32 = 0xFFFF_0000;
/// The key that unlocks every watchdog register (`mwdt_ll_write_protect_disable`, line 245); it
/// is also the reset value of `WDTWPROTECT`: the watchdog is unlocked out of reset (TW §B.3).
const WDT_KEY: u32 = 0x50D8_3AA1;
const STAGES: usize = 4;
/// The most iterations one evaluation of the stage walk takes: four expiries, the reduction of
/// the count modulo a turn of the four stages, at most four more expiries, and the wait.
const WALK_STEPS: usize = 2 * STAGES + 1;
/// Stage actions (`IDF:hal/include/hal/wdt_types.h`; TW §D.5).
const ACTION_INTERRUPT: u32 = 1;
const ACTION_RESET_CPU: u32 = 2;
const ACTION_RESET_SYSTEM: u32 = 3;

/// Bit 0 of the interrupt registers is T0 (`timer_ll_enable_intr`, `timer_ll.h:272`), bit 1 the
/// watchdog (`mwdt_ll_set_intr_enable`, `mwdt_ll.h:266`).
const INT_T0: u32 = 1 << 0;
const INT_WDT: u32 = 1 << 1;
const INT_MASK: u32 = INT_T0 | INT_WDT;

/// `REGCLK`: bit 31 `CLK_EN` (`timer_ll_enable_register_clock_always_on`, `timer_ll.h:313`), bit 30
/// `TIMER_CLK_IS_ACTIVE`, bit 29 `WDT_CLK_IS_ACTIVE` (`mwdt_ll_enable_clock`, `mwdt_ll.h:299`): the
/// last two gate the clock of T0 and of the watchdog (TW §B.2, §B.3).
const REGCLK_STORED: u32 = 0xE000_0000;
const TIMER_CLK_IS_ACTIVE: u32 = 1 << 30;
const WDT_CLK_IS_ACTIVE: u32 = 1 << 29;

/// `RTCCALICFG`: bit 31 `START`, bits [30:16] `MAX`, bit 15 `RDY` (read-only), bits [14:13]
/// `CLK_SEL`, bit 12 `START_CYCLING`. `rtc_clk_cal_internal` writes each of them
/// (`IDF:esp_hw_support/port/esp32c3/rtc_time.c:36-128`; TW §B.4).
const CALI_START: u32 = 1 << 31;
const CALI_MAX_SHIFT: u32 = 16;
const CALI_MAX_MASK: u32 = 0x7FFF;
const CALI_RDY: u32 = 1 << 15;
const CALI_CLK_SEL_SHIFT: u32 = 13;
const CALI_CLK_SEL_MASK: u32 = 0b11;
const CALI_START_CYCLING: u32 = 1 << 12;
const RTCCALICFG_STORED: u32 = 0xFFFF_7000;
/// `RTCCALICFG1` bits [31:7] `VALUE`: the crystal cycles counted.
const CALI_VALUE_SHIFT: u32 = 7;
/// `RTCCALICFG2`: bits [31:7] `TIMEOUT_THRES`, bits [6:3] `TIMEOUT_RST_CNT`, bit 0 `TIMEOUT`
/// (read-only).
const CALI_THRES_SHIFT: u32 = 7;
const CALI_TIMEOUT: u32 = 1;
const RTCCALICFG2_STORED: u32 = 0xFFFF_FFF8;

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
struct Measurement {
    /// Crystal cycles counted during `MAX` slow-clock cycles.
    value: u32,
    /// Device time from which `RDY` reads 1, if the measurement completes.
    ready_at: Option<Cycles>,
    /// Device time from which `TIMEOUT` reads 1, if it gives up.
    timeout_at: Option<Cycles>,
}

#[derive(Clone, Copy, Debug)]
struct Calibration {
    config: u32,
    config2: u32,
    measurement: Measurement,
}

impl Calibration {
    const fn new() -> Calibration {
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

    fn read_config(&self, now: Cycles) -> u32 {
        let ready = self.measurement.ready_at.is_some_and(|at| at <= now);
        self.config | if ready { CALI_RDY } else { 0 }
    }

    fn read_config2(&self, now: Cycles) -> u32 {
        let timed_out = self.measurement.timeout_at.is_some_and(|at| at <= now);
        self.config2 | if timed_out { CALI_TIMEOUT } else { 0 }
    }

    fn write_config(&mut self, value: u32, now: Cycles) {
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
    fn write_config2(&mut self, value: u32, now: Cycles) {
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

/// The earlier of two optional instants.
fn earlier(a: Option<Cycles>, b: Option<Cycles>) -> Option<Cycles> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[derive(Clone, Debug)]
pub struct Timg {
    group: Group,
    /// Whether the chip boots from flash: the strapping the flash-boot protection depends on.
    flash_boot: bool,
    t0_config: u32,
    t0: Counter,
    t0_latch: u64,
    t0_alarm: u64,
    /// The instant T0 reaches its alarm, as the last look worked it out and nothing has changed
    /// since: where an auto-reload restarts the counter when the wake-up comes late.
    t0_alarm_at: Option<Cycles>,
    t0_load: u64,
    wdt_config0: u32,
    wdt_config1: u32,
    wdt_timeout: [u32; STAGES],
    wdt_key: u32,
    /// Counts since the last feed or the start of the current stage.
    wdt: Counter,
    wdt_stage: usize,
    /// Whether `EN` or the flash-boot protection had the watchdog on at the last look.
    wdt_enabled: bool,
    calibration: Calibration,
    int_ena: u32,
    int_raw: u32,
    regclk: u32,
    wake: Deadline,
}

impl Timg {
    /// The group after a power-on, a core reset or a peripheral reset. `flash_boot` says whether
    /// the chip was strapped to boot from flash.
    ///
    /// The owner calls [`Timg::wake`] once afterwards: the reset values arm the flash-boot
    /// protection of group 0, which needs its first deadline.
    #[must_use]
    pub fn new(group: Group, flash_boot: bool, ctx: &Ctx) -> Timg {
        let now = ctx.now();
        let mut timg = Timg {
            group,
            flash_boot,
            t0_config: T0CONFIG_RESET,
            t0: Counter::new(0, now, Rate::every(1), false),
            t0_latch: 0,
            t0_alarm: 0,
            t0_alarm_at: None,
            t0_load: 0,
            wdt_config0: WDTCONFIG0_RESET,
            wdt_config1: WDTCONFIG1_RESET,
            wdt_timeout: STAGE_TIMEOUT_RESET,
            wdt_key: WDT_KEY,
            wdt: Counter::new(0, now, Rate::every(1), false),
            wdt_stage: 0,
            wdt_enabled: false,
            calibration: Calibration::new(),
            int_ena: 0,
            int_raw: 0,
            regclk: REGCLK_RESET,
            wake: Deadline::default(),
        };
        timg.retime(ctx);
        timg
    }

    /// The group is reset (`SYSTEM.PERIP_RST_EN0` bit 13 or 15, or a core reset): it is built
    /// again at the present time. Its wake-up is withdrawn first, so that a firmware resetting the
    /// group over and over leaves no call behind in the scheduler, and the rebuilt group drives its
    /// interrupt sources low and arms the flash-boot protection again.
    pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
        self.wake.cancel(ctx);
        *self = Timg::new(self.group, self.flash_boot, ctx);
        self.wake(ctx);
    }

    const fn event(&self) -> Event {
        Event::Timg(self.group)
    }

    const fn sources(&self) -> (Source, Source) {
        match self.group {
            Group::Timg0 => (Source::of("TG_T0_INT_MAP"), Source::of("TG_WDT_INT_MAP")),
            Group::Timg1 => (Source::of("TG1_T0_INT_MAP"), Source::of("TG1_WDT_INT_MAP")),
        }
    }

    /// T0 counts `APB_CLK` or the crystal, divided by `DIVIDER`: 2 to 65 535 divide by that value,
    /// 0 divides by 65 536 and 1 by 2 (TRM §11.2; TW §B.2).
    fn t0_rate(&self, ctx: &Ctx) -> Rate {
        let source = if self.t0_config & T0_USE_XTAL == 0 {
            ctx.clock().apb_period
        } else {
            XTAL_PERIOD
        };
        let divider = match (self.t0_config >> T0_DIVIDER_SHIFT) & T0_DIVIDER_MASK {
            0 => 65_536,
            1 => 2,
            divider => u64::from(divider),
        };
        Rate::every(source).divided_by(divider)
    }

    /// The watchdog counts `APB_CLK` or the crystal, divided by `CLK_PRESCALE` (TRM §12.2.2.1).
    ///
    /// ASSUMPTION: a prescaler of 0 divides by 65 536, as the 16-bit divider of T0 does. No source
    /// says; ESP-IDF never writes 0. A probe on a real chip settles it.
    fn wdt_rate(&self, ctx: &Ctx) -> Rate {
        let source = if self.wdt_config0 & WDT_USE_XTAL == 0 {
            ctx.clock().apb_period
        } else {
            XTAL_PERIOD
        };
        let prescale = match self.wdt_config1 >> WDT_PRESCALE_SHIFT {
            0 => 65_536,
            prescale => u64::from(prescale),
        };
        Rate::every(source).divided_by(prescale)
    }

    /// Whether the flash-boot protection runs the watchdog: only that of group 0 has one, and only
    /// when the chip boots from flash (TRM §12.2.2.4; TW §D.4).
    ///
    /// ASSUMPTION: the protection is off when the chip is strapped for another boot mode. The ROM
    /// waits in its download modes for ever and has no code that could stop a watchdog (TW §D.4,
    /// §I question 4): the condition is inferred, not documented. A boot of a chip strapped for
    /// download, with the main watchdog left running, settles it.
    fn flash_boot_protection(&self) -> bool {
        self.group == Group::Timg0
            && self.flash_boot
            && self.wdt_config0 & WDT_FLASHBOOT_MOD_EN != 0
    }

    fn wdt_enabled(&self) -> bool {
        self.wdt_config0 & WDT_EN != 0 || self.flash_boot_protection()
    }

    fn t0_runs(&self) -> bool {
        self.regclk & TIMER_CLK_IS_ACTIVE != 0 && self.t0_config & T0_EN != 0
    }

    /// The action of a stage: the flash-boot protection makes stage 0 a core reset "whatever EN
    /// and STG0 contain" (TW §B.3).
    fn stage_action(&self, stage: usize) -> u32 {
        if stage == 0 && self.flash_boot_protection() {
            return ACTION_RESET_SYSTEM;
        }
        // INVARIANT: `stage` is below `STAGES` (4), so the shift stays inside the register.
        let shift = WDT_STAGE0_SHIFT - WDT_STAGE_BITS * u32::try_from(stage).unwrap_or(0);
        (self.wdt_config0 >> shift) & WDT_STAGE_MASK
    }

    /// The timeout of a stage, in watchdog counts.
    ///
    /// ASSUMPTION: a stage lasts at least one count, so a timeout of 0 counts as 1: what a stage
    /// timeout of 0 does on silicon is not documented. Programming 0 on a chip and timing the
    /// reset settles it.
    fn stage_timeout(&self, stage: usize) -> u64 {
        u64::from(self.wdt_timeout[stage]).max(1)
    }

    /// Brings both counters to the clocks and the enables the registers now describe. They keep
    /// their value: only what happens from now on changes.
    fn retime(&mut self, ctx: &Ctx) {
        let now = ctx.now();
        self.t0.retime(self.t0_rate(ctx), now);
        self.t0.run(self.t0_runs(), now);
        self.wdt.retime(self.wdt_rate(ctx), now);
        let enabled = self.wdt_enabled();
        if enabled && !self.wdt_enabled {
            // ASSUMPTION: a watchdog that is switched on counts its first stage from 0. ESP-IDF
            // feeds before it enables (`wdt_hal_enable`, `IDF:hal/wdt_hal_iram.c:146-155`), so the
            // firmware does not tell (TW §I question 5). A real chip whose watchdog is enabled
            // after a long run, left unfed, settles it.
            self.wdt.set(0, now);
            self.wdt_stage = 0;
        }
        self.wdt_enabled = enabled;
        // The clock gate freezes the counter without clearing it.
        self.wdt
            .run(enabled && self.regclk & WDT_CLK_IS_ACTIVE != 0, now);
    }

    fn t0_count(&self, now: Cycles) -> u64 {
        let counted = self.t0.value(now);
        if self.t0_config & T0_INCREASE == 0 {
            // Counting down: the counter holds the value it was set to, minus what went by.
            // `set_t0` stores the negation, so that the same up-counter serves both directions.
            counted.wrapping_neg() & T0_MASK
        } else {
            counted & T0_MASK
        }
    }

    fn set_t0(&mut self, value: u64, instant: Cycles) {
        let stored = if self.t0_config & T0_INCREASE == 0 {
            value.wrapping_neg()
        } else {
            value
        };
        self.t0.set(stored & T0_MASK, instant);
    }

    /// Counts until T0 reaches its alarm value in its direction, 0 when it is there or past it.
    fn t0_counts_to_alarm(&self, now: Cycles) -> u64 {
        let count = self.t0_count(now);
        let ahead = if self.t0_config & T0_INCREASE == 0 {
            count.wrapping_sub(self.t0_alarm)
        } else {
            self.t0_alarm.wrapping_sub(count)
        } & T0_MASK;
        if ahead >= T0_PASSED_WINDOW { 0 } else { ahead }
    }

    /// Raises what is due at the present time, then asks to be woken for the next deadline.
    /// `from_register_write` says that a write to a watchdog register called.
    fn evaluate(&mut self, ctx: &mut Ctx, from_register_write: bool) {
        let now = ctx.now();
        // The deadline is worked out again at every call and no earlier wake-up is kept: a register
        // write can move the deadline later, and `Deadline::arm` keeps an earlier one. The
        // instant `next_wake` gives the sleeping CPU must be the true one. The scheduler bounds
        // the cancelled entries this leaves.
        self.wake.cancel(ctx);
        let mut earliest = self.step_t0(now);
        if self.wdt.is_running() {
            earliest = earlier(earliest, self.walk_stages(ctx, now, from_register_write));
        }
        if let Some(at) = earliest {
            self.wake.arm(ctx, at, self.event());
        }
        self.drive(ctx);
    }

    /// T0 at the present time; returns the instant of its alarm, if one is waited for.
    fn step_t0(&mut self, now: Cycles) -> Option<Cycles> {
        if self.t0_config & T0_ALARM_EN == 0 || !self.t0.is_running() {
            self.t0_alarm_at = None;
            return None;
        }
        let ahead = self.t0_counts_to_alarm(now);
        if ahead != 0 {
            self.t0_alarm_at = self.t0.instant_in(ahead, now);
            return self.t0_alarm_at;
        }
        // At the alarm the interrupt is raised, `ALARM_EN` is cleared by hardware and, with
        // `AUTORELOAD`, the counter restarts from the reload value (TW §B.2).
        self.int_raw |= INT_T0;
        self.t0_config &= !T0_ALARM_EN;
        if self.t0_config & T0_AUTORELOAD != 0 {
            // The counter restarts at the instant of the alarm, not at the call: a wake-up
            // delivered late finds the counter already `now - alarm` further on, and the phase of
            // its count is kept (the counter is anchored at an instant, to the cycle). An alarm
            // that was already past when it was enabled has no such instant: it reloads now.
            let reached = self.t0_alarm_at.map_or(now, |at| at.min(now));
            self.set_t0(self.t0_load, reached);
        }
        self.t0_alarm_at = None;
        None
    }

    /// Lets the main watchdog go through the stages whose timeout the count has reached, and
    /// returns the instant of the next expiry. The walk follows these rules:
    ///
    /// 1. A stage lasts at least one count (see [`Timg::stage_timeout`]).
    /// 2. A write that leaves the count at or past the timeout of the current stage expires that
    ///    stage at the write, and the next stage counts from 0 (TRM §12.2.2.2; TW §B.3: "the
    ///    counter returns to 0 and the next stage becomes current"): the first expiry of such a
    ///    call carries nothing.
    /// 3. An expiry found because time passed (a wake-up, a clock change, a reset, the construction)
    ///    carries the counts elapsed since the expiry into the next stage (`count - timeout`). The
    ///    fraction of a count is lost at each transition: the counter is anchored at the present
    ///    time with a whole number of counts, so the loss is less than one count per transition,
    ///    and deterministic.
    /// 4. The walk is bounded whatever firmware wrote: after four expiries in one call every stage
    ///    has acted once, so the remaining count is reduced modulo the sum of the four timeouts
    ///    (whole turns change neither the stage nor which actions fired), which leaves at most four
    ///    more expiries. One call costs at most [`WALK_STEPS`] iterations, with no trip count that
    ///    depends on a register value.
    /// 5. The walk goes on after a reset request: the reset it asks for does not stop the
    ///    watchdog, and [`Ctx::request_reset`] keeps the deepest request, the first between equal
    ///    levels.
    fn walk_stages(
        &mut self,
        ctx: &mut Ctx,
        now: Cycles,
        from_register_write: bool,
    ) -> Option<Cycles> {
        let timeouts: [u64; STAGES] = std::array::from_fn(|stage| self.stage_timeout(stage));
        let turn: u64 = timeouts.iter().sum();
        let mut count = self.wdt.value(now);
        let mut carries = !from_register_write;
        let mut expiries = 0;
        for _ in 0..WALK_STEPS {
            let timeout = timeouts[self.wdt_stage];
            // ASSUMPTION: a stage expires when the counter is at or past its timeout. The hardware
            // may compare for equality, which would let a timeout lowered below the present count
            // never expire until the next feed; QEMU's model says so, no document does (TW §G row
            // 7, §I question 5). At-or-past makes such a watchdog fire, which is visible. Lowering
            // a timeout below the count on a chip, and seeing whether the stage fires, settles it.
            if count < timeout {
                return self.wdt.instant_in(timeout - count, now);
            }
            let action = self.stage_action(self.wdt_stage);
            // The counter returns to 0 (or keeps the counts beyond the timeout, rule 3) and the
            // next stage becomes current; stages loop 0 to 3 (TRM §12.2.2.2).
            count = if carries { count - timeout } else { 0 };
            carries = true;
            self.wdt_stage = (self.wdt_stage + 1) % STAGES;
            expiries += 1;
            if expiries == STAGES {
                count %= turn;
            }
            self.wdt.set(count, now);
            match (action, self.group) {
                (ACTION_INTERRUPT, _) => self.int_raw |= INT_WDT,
                // ASSUMPTION: action 2 resets the CPU whatever `PROCPU_RESET_EN` (`WDTCONFIG0`
                // bit 13) holds, as for the RTC watchdog (TW §D.5, §I question 5). Clearing the
                // bit on a chip whose stage resets the CPU, and seeing whether the CPU still
                // resets, settles it.
                (ACTION_RESET_CPU, Group::Timg0) => ctx.request_reset(ResetCause::Mwdt0Cpu),
                (ACTION_RESET_CPU, Group::Timg1) => ctx.request_reset(ResetCause::Mwdt1Cpu),
                (ACTION_RESET_SYSTEM, Group::Timg0) => ctx.request_reset(ResetCause::Mwdt0Core),
                (ACTION_RESET_SYSTEM, Group::Timg1) => ctx.request_reset(ResetCause::Mwdt1Core),
                // ASSUMPTION: a stage whose action is "off" is waited for like the others and
                // does nothing when it ends (QEMU's reading; TW §B.3, §I question 5). A chip with
                // stage 0 off and stage 1 resetting, timed, settles it.
                _ => {}
            }
        }
        // INVARIANT: unreachable. After the reduction the count is below the sum of the four
        // timeouts, so at most four more stages expire and the walk returns above. A wake-up one
        // count away keeps the watchdog alive if that were ever wrong.
        self.wdt.instant_in(1, now)
    }

    /// Both interrupts are levels: `INT_RAW & INT_ENA` (TW §B.1).
    fn drive(&self, ctx: &mut Ctx) {
        let (t0, wdt) = self.sources();
        let status = self.int_raw & self.int_ena;
        ctx.irq(t0, status & INT_T0 != 0);
        ctx.irq(wdt, status & INT_WDT != 0);
    }

    /// The device time of a wake-up has come, or the group was just built.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.retime(ctx);
        self.evaluate(ctx, false);
    }

    /// The APB clock changed: the counters keep their value and count on at the new rate.
    pub(crate) fn clock_changed(&mut self, ctx: &mut Ctx) {
        // The instant of the alarm was worked out at the old rate.
        self.t0_alarm_at = None;
        self.retime(ctx);
        self.evaluate(ctx, false);
    }

    /// A write to a watchdog register other than the key: dropped unless the key is in place,
    /// "feeding or changing its configuration" alike (TRM §12.2.2.3).
    fn write_watchdog(&mut self, ctx: &mut Ctx, offset: u32, value: u32) {
        if self.wdt_key != WDT_KEY {
            return;
        }
        let now = ctx.now();
        match offset {
            // ASSUMPTION: a configuration register acts as soon as it is written, without waiting
            // for `CONF_UPDATE_EN`. The TRM describes a synchronisation on that bit (§12.2; TW
            // §B.3), but `_timer_ll_reset_register` clears `FLASHBOOT_MOD_EN` right after a
            // peripheral reset without writing it, "to prevent the system from being reset
            // unexpectedly" (`IDF:hal/esp32c3/include/hal/timer_ll.h:50-66`): that code only works
            // if the bit acts at once. A register read-back on a real chip settles the others.
            WDTCONFIG0 => self.wdt_config0 = value & WDTCONFIG0_STORED & !WDT_CONF_UPDATE_EN,
            WDTCONFIG1 => self.wdt_config1 = value & WDTCONFIG1_STORED,
            WDTFEED => {
                // Any write feeds: the counter returns to 0 and stage 0 becomes current (TW §B.3).
                self.wdt.set(0, now);
                self.wdt_stage = 0;
            }
            _ => self.wdt_timeout[((offset - WDTCONFIG2) / 4) as usize] = value,
        }
        self.retime(ctx);
        self.evaluate(ctx, true);
    }
}

impl Peripheral<Ctx> for Timg {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        let now = ctx.now();
        Some(match offset {
            T0CONFIG => self.t0_config,
            T0LO => half_of(self.t0_latch, false),
            T0HI => half_of(self.t0_latch, true),
            // "Cleared by hardware when the latch is done" (TW §B.1): the latch is immediate.
            T0UPDATE | T0LOAD | WDTFEED | INT_CLR => 0,
            T0ALARMLO => half_of(self.t0_alarm, false),
            T0ALARMHI => half_of(self.t0_alarm, true),
            T0LOADLO => half_of(self.t0_load, false),
            T0LOADHI => half_of(self.t0_load, true),
            WDTCONFIG0 => self.wdt_config0,
            WDTCONFIG1 => self.wdt_config1,
            WDTWPROTECT => self.wdt_key,
            RTCCALICFG => self.calibration.read_config(now),
            RTCCALICFG1 => {
                if self
                    .calibration
                    .measurement
                    .ready_at
                    .is_some_and(|at| at <= now)
                {
                    self.calibration.measurement.value << CALI_VALUE_SHIFT
                } else {
                    0
                }
            }
            RTCCALICFG2 => self.calibration.read_config2(now),
            INT_ENA => self.int_ena,
            INT_RAW => self.int_raw,
            INT_ST => self.int_raw & self.int_ena,
            NTIMG_DATE => NTIMG_DATE_VALUE,
            REGCLK => self.regclk,
            offset if (WDTCONFIG2..=WDTCONFIG5).contains(&offset) => {
                self.wdt_timeout[((offset - WDTCONFIG2) / 4) as usize]
            }
            _ => return None,
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        let now = ctx.now();
        match offset {
            T0CONFIG => {
                // A write re-anchors the counter only for what changes its counting, so that
                // the fraction of a count it has counted is not lost: a driver rewrites this
                // register at every alarm (`timer_ll_enable_alarm`, `timer_ll.h:118`).
                // - `EN`: `retime` starts or stops the counter, which keeps its value. A start
                //   (0 to 1) resumes from the kept value with a fresh phase: the count in
                //   progress is counted from the write.
                // - `USE_XTAL`: the clock source changes the rate, which `retime` applies by
                //   re-anchoring the counter at the write, like a divider change.
                // - `INCREASE`: the value is kept and the direction changes from now.
                // - `DIVIDER`: ASSUMPTION: a change takes effect at once and keeps the count
                //   (`retime`). The header says the divider is changed with the timer stopped
                //   and that `DIVCNT_RST` restarts the divider counter; what a running change
                //   does is not documented. Changing the divider of a running timer on a chip
                //   and reading the counter settles it.
                // - `DIVCNT_RST` written 1: the divider counter restarts, which drops the
                //   fraction of a count (`timer_group_reg.h:36`; written at `timer_ll.h:137`).
                // A write that changes none of these changes nothing in the counter.
                let count = self.t0_count(now);
                let flipped = (self.t0_config ^ value) & T0_INCREASE != 0;
                self.t0_config = value & T0CONFIG_STORED;
                if flipped {
                    self.set_t0(count, now);
                }
                if value & T0_DIVCNT_RST != 0 {
                    self.t0.set(self.t0.value(now), now);
                }
                self.t0_alarm_at = None;
                self.retime(ctx);
                self.evaluate(ctx, false);
            }
            // Any write latches the counter (`timer_ll_trigger_soft_capture`, `timer_ll.h:187`).
            T0UPDATE => self.t0_latch = self.t0_count(now),
            T0ALARMLO | T0ALARMHI => {
                self.t0_alarm = replace_half(self.t0_alarm, offset == T0ALARMHI, value, T0_HI_MASK);
                self.t0_alarm_at = None;
                self.evaluate(ctx, false);
            }
            T0LOADLO | T0LOADHI => {
                self.t0_load = replace_half(self.t0_load, offset == T0LOADHI, value, T0_HI_MASK);
            }
            // Any write reloads (`timer_ll_trigger_soft_reload`, `timer_ll.h:258`).
            T0LOAD => {
                self.set_t0(self.t0_load, now);
                self.t0_alarm_at = None;
                self.evaluate(ctx, false);
            }
            WDTWPROTECT => self.wdt_key = value,
            WDTCONFIG0 | WDTCONFIG1 | WDTFEED => self.write_watchdog(ctx, offset, value),
            offset if (WDTCONFIG2..=WDTCONFIG5).contains(&offset) => {
                self.write_watchdog(ctx, offset, value);
            }
            RTCCALICFG => self.calibration.write_config(value, now),
            RTCCALICFG2 => self.calibration.write_config2(value, now),
            INT_ENA => {
                self.int_ena = value & INT_MASK;
                self.drive(ctx);
            }
            INT_CLR => {
                self.int_raw &= !value;
                self.drive(ctx);
            }
            REGCLK => {
                self.regclk = value & REGCLK_STORED;
                self.t0_alarm_at = None;
                self.retime(ctx);
                self.evaluate(ctx, false);
            }
            // `INT_ST`, `T0LO`/`T0HI` and `RTCCALICFG1` are read-only; `INT_RAW` is set by the
            // hardware and cleared through `INT_CLR` (R/SS/WTC, `timer_group_reg.h:440`);
            // `NTIMG_DATE` is R/W (:524) but the model keeps its version value. A write to any of
            // them is reported as unmodelled.
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::CpuClock;

    /// One second of device time.
    const SECOND: u64 = 160_000_000;
    const MILLISECOND: u64 = SECOND / 1_000;

    fn group(ctx: &mut Ctx, group: Group) -> Timg {
        let mut timg = Timg::new(group, true, ctx);
        timg.wake(ctx);
        timg
    }

    /// Drains every wake-up due at the present time, those a call arms for the present time
    /// included (the machine delivers those one pass later: `Ctx::deliver_due`), and fails the
    /// test after a thousand calls.
    fn pump(ctx: &mut Ctx, timg: &mut Timg) {
        let mut calls = 0;
        while let Some(event) = ctx.take_due() {
            calls += 1;
            assert!(
                calls <= 1_000,
                "the device keeps arming itself at the present time"
            );
            assert_eq!(event, timg.event());
            timg.wake(ctx);
        }
    }

    fn after(ctx: &mut Ctx, timg: &mut Timg, cycles: u64) {
        ctx.sleep_until(ctx.now() + cycles);
        pump(ctx, timg);
    }

    /// What `wdt_hal_init`, `wdt_hal_config_stage` and `wdt_hal_enable` write for the interrupt
    /// watchdog (TW §B.3; values measured in trace M1, as hardware reads them back).
    fn start_the_interrupt_watchdog(ctx: &mut Ctx, timg: &mut Timg) {
        for (offset, value) in [
            (WDTWPROTECT, WDT_KEY),
            (WDTCONFIG0, 0x003F_8000 | WDT_CONF_UPDATE_EN),
            (INT_CLR, INT_WDT),
            (INT_ENA, INT_WDT),
            (REGCLK, 0x6000_0000),
            (WDTCONFIG1, 20_000 << 16),
            (WDTCONFIG0, 0x203F_8000 | WDT_CONF_UPDATE_EN),
            (WDTCONFIG2, 600),
            (WDTCONFIG0, 0x383F_8000 | WDT_CONF_UPDATE_EN),
            (WDTCONFIG2 + 4, 1_200),
            (WDTFEED, 1),
            (WDTCONFIG0, 0xB83F_8000 | WDT_CONF_UPDATE_EN),
            (WDTWPROTECT, 0),
        ] {
            assert!(timg.write(ctx, offset, value), "{offset:#x}");
        }
    }

    #[test]
    fn reset_values_are_those_of_the_header() {
        // TW §B.1.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        for (offset, value) in [
            (T0CONFIG, 0x6000_2000),
            (WDTCONFIG0, 0x0004_C000),
            (WDTCONFIG1, 0x0001_0000),
            (WDTCONFIG2, 26_000_000),
            (WDTCONFIG2 + 4, 134_217_727),
            (WDTCONFIG2 + 8, 1_048_575),
            (WDTCONFIG5, 1_048_575),
            (WDTWPROTECT, 0x50D8_3AA1),
            (RTCCALICFG, 0x0001_3000),
            (RTCCALICFG1, 0),
            (RTCCALICFG2, 0xFFFF_FF98),
            (INT_ENA, 0),
            (INT_RAW, 0),
            (NTIMG_DATE, 0x0200_6191),
            (REGCLK, 0x6000_0000),
        ] {
            assert_eq!(timg.read(ctx, offset), Some(value), "{offset:#x}");
        }
        assert_eq!(
            timg.read(ctx, T0LOAD + 4),
            None,
            "no second timer on the C3"
        );
        assert!(!timg.write(ctx, INT_ST, 1), "read-only");
        assert_eq!(
            ctx.next_wake(),
            None,
            "group 1 has no flash-boot protection"
        );
    }

    #[test]
    fn the_flash_boot_protection_of_group_0_resets_the_core_after_26_million_apb_cycles() {
        // TW §D.4: 26 000 000 cycles of APB_CLK at 20 MHz, 1.3 s; cause 0x07.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        assert_eq!(ctx.next_wake(), Some(Cycles(13 * SECOND / 10)));
        after(ctx, &mut timg, 13 * SECOND / 10 - 1);
        assert_eq!(ctx.take_reset(), None);
        after(ctx, &mut timg, 1);
        assert_eq!(ctx.take_reset(), Some(ResetCause::Mwdt0Core));
    }

    #[test]
    fn the_protection_follows_the_apb_clock_and_stops_when_its_bit_is_cleared() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        // Half of the timeout at 20 MHz, then APB at 40 MHz as after `esp_restart` (TW §D.4).
        after(ctx, &mut timg, 65 * SECOND / 100);
        ctx.set_clock(CpuClock {
            cpu_period: 4,
            apb_period: 4,
        });
        timg.clock_changed(ctx);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(975 * SECOND / 1_000)),
            "0.65 s + 0.325 s"
        );
        // What `bootloader_config_wdt` writes: unlock, clear the bit, lock (TW §B.3).
        timg.write(ctx, WDTWPROTECT, WDT_KEY);
        timg.write(ctx, WDTCONFIG0, 0x0004_8000 | WDT_CONF_UPDATE_EN);
        timg.write(ctx, WDTWPROTECT, 0);
        assert_eq!(
            timg.read(ctx, WDTCONFIG0),
            Some(0x0004_8000),
            "CONF_UPDATE_EN reads 0"
        );
        after(ctx, &mut timg, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None, "stopped");
    }

    #[test]
    fn the_bit_cleared_without_the_update_strobe_stops_the_protection_too() {
        // `_timer_ll_reset_register`: a plain read-modify-write right after a peripheral reset.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        timg.write(ctx, WDTCONFIG0, 0x0004_8000);
        after(ctx, &mut timg, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None);
    }

    #[test]
    fn a_chip_that_does_not_boot_from_flash_has_no_protection() {
        let ctx = &mut Ctx::default();
        let mut timg = Timg::new(Group::Timg0, false, ctx);
        timg.wake(ctx);
        assert_eq!(ctx.next_wake(), None);
    }

    #[test]
    fn a_locked_watchdog_ignores_feeds_and_configuration() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        timg.write(ctx, WDTWPROTECT, 0);
        assert!(timg.write(ctx, WDTCONFIG0, 0), "accepted and dropped");
        assert!(timg.write(ctx, WDTFEED, 1));
        assert!(timg.write(ctx, WDTCONFIG2, 5));
        assert_eq!(timg.read(ctx, WDTCONFIG0), Some(0x0004_C000));
        assert_eq!(timg.read(ctx, WDTCONFIG2), Some(26_000_000));
        after(ctx, &mut timg, 13 * SECOND / 10);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::Mwdt0Core),
            "still armed, never fed"
        );
    }

    #[test]
    fn the_interrupt_watchdog_interrupts_after_300_ms_and_resets_the_core_600_ms_later() {
        // TW §B.3, §D.5: stage 0 = interrupt after 600 counts of 500 us, stage 1 = core reset
        // after 1 200 more. Source TG1_WDT is 35.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        start_the_interrupt_watchdog(ctx, &mut timg);
        assert_eq!(timg.read(ctx, WDTCONFIG0), Some(0xB83F_8000));
        after(ctx, &mut timg, 300 * MILLISECOND - 1);
        assert_eq!((timg.read(ctx, INT_RAW), ctx.sources()), (Some(0), 0));
        after(ctx, &mut timg, 1);
        assert_eq!(timg.read(ctx, INT_ST), Some(INT_WDT));
        assert_eq!(ctx.sources(), 1 << 35);
        assert_eq!(ctx.take_reset(), None);
        after(ctx, &mut timg, 600 * MILLISECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::Mwdt1Core));
    }

    #[test]
    fn a_watchdog_fed_at_every_tick_never_expires() {
        // The tick hook: unlock, both stages rewritten, feed, lock, every 10 ms (TW §F.3).
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        start_the_interrupt_watchdog(ctx, &mut timg);
        for _ in 0..200 {
            after(ctx, &mut timg, 10 * MILLISECOND);
            for (offset, value) in [
                (WDTWPROTECT, WDT_KEY),
                (WDTCONFIG2, 600),
                (WDTCONFIG0, 0xB83F_8000 | WDT_CONF_UPDATE_EN),
                (WDTCONFIG2 + 4, 1_200),
                (WDTCONFIG0, 0xB83F_8000 | WDT_CONF_UPDATE_EN),
                (WDTFEED, 1),
                (WDTWPROTECT, 0),
            ] {
                timg.write(ctx, offset, value);
            }
        }
        assert_eq!(timg.read(ctx, INT_RAW), Some(0));
        assert_eq!(ctx.take_reset(), None);
        // The ticks stop: the interrupt comes 300 ms after the last feed.
        after(ctx, &mut timg, 300 * MILLISECOND);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_WDT));
    }

    #[test]
    fn a_sleep_that_covers_two_stages_runs_both_in_order() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        start_the_interrupt_watchdog(ctx, &mut timg);
        ctx.sleep_until(ctx.now() + 2 * SECOND);
        pump(ctx, &mut timg);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_WDT), "stage 0");
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::Mwdt1Core),
            "then stage 1"
        );
    }

    #[test]
    fn a_cpu_reset_stage_and_an_off_stage_do_what_their_action_says() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        // Stage 0 off (10 counts), stage 1 CPU reset (10 counts), crystal divided by 40 000: 1 ms.
        for (offset, value) in [
            (WDTCONFIG1, 40_000 << 16),
            (WDTCONFIG2, 10),
            (WDTCONFIG2 + 4, 10),
            (WDTFEED, 1),
            (WDTCONFIG0, WDT_EN | WDT_USE_XTAL | (ACTION_RESET_CPU << 27)),
        ] {
            timg.write(ctx, offset, value);
        }
        after(ctx, &mut timg, 10 * MILLISECOND);
        assert_eq!(
            (timg.read(ctx, INT_RAW), ctx.take_reset()),
            (Some(0), None),
            "off"
        );
        after(ctx, &mut timg, 10 * MILLISECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::Mwdt0Cpu));
    }

    #[test]
    fn the_watchdog_clock_gate_stops_the_counter() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        after(ctx, &mut timg, SECOND);
        timg.write(ctx, REGCLK, 0x4000_0000);
        after(ctx, &mut timg, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None, "gated");
        timg.write(ctx, REGCLK, 0x6000_0000);
        after(ctx, &mut timg, 3 * SECOND / 10);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::Mwdt0Core),
            "1 s + 0.3 s of counting"
        );
    }

    /// Device time of one watchdog count with the crystal as source and a prescaler of 20 000:
    /// 4 x 20 000 cycles of the time base (2 kHz, 500 us).
    const COUNT: u64 = XTAL_PERIOD * 20_000;

    /// Writes the prescaler, the four stage timeouts and `WDTCONFIG0` as `wdt_hal_init`,
    /// `wdt_hal_config_stage` and `wdt_hal_enable` do, on the crystal clock at 2 kHz. `stages` are
    /// the actions of stages 0 and 1.
    fn program_watchdog(ctx: &mut Ctx, timg: &mut Timg, timeouts: [u32; 4], stages: [u32; 2]) {
        assert!(timg.write(ctx, WDTCONFIG1, 20_000 << 16));
        for (i, timeout) in timeouts.into_iter().enumerate() {
            assert!(timg.write(ctx, WDTCONFIG2 + 4 * u32::try_from(i).unwrap(), timeout));
        }
        let actions = (stages[0] << WDT_STAGE0_SHIFT) | (stages[1] << (WDT_STAGE0_SHIFT - 2));
        assert!(timg.write(ctx, WDTCONFIG0, WDT_EN | WDT_USE_XTAL | actions));
    }

    #[test]
    fn stage_timeouts_of_zero_last_one_count_and_the_stage_walk_terminates() {
        // ASSUMPTION (rule 1): a stage lasts at least one count. All four timeouts at 0, enabled.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        program_watchdog(ctx, &mut timg, [0; 4], [0; 2]);
        assert_eq!(timg.wdt_stage, 0);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(COUNT)),
            "1 count: 80 000 cycles"
        );
        after(ctx, &mut timg, COUNT);
        assert_eq!(
            timg.wdt_stage, 1,
            "one stage per count, the fraction of a count is lost"
        );
        assert_eq!(ctx.next_wake(), Some(Cycles(2 * COUNT)));
        // A very late call: one hour is 7 200 000 counts, which is 4 x 1 800 000 whole turns of
        // the four one-count stages. Four expiries leave 7 199 996 counts, the reduction modulo
        // the turn (4) leaves 0, and the walk waits in the stage it is in: 1 + 4 = 5 = 1.
        after(ctx, &mut timg, 3_600 * SECOND);
        assert_eq!(timg.wdt_stage, 1);
        assert_eq!(ctx.next_wake(), Some(ctx.now() + COUNT));
    }

    #[test]
    fn lowering_the_timeouts_after_a_long_count_expires_one_stage_at_the_write() {
        // Stage 0 counts up to 0xFFFF_FFFF counts (24.9 days at 2 kHz); one hour of device time is
        // 3 600 * 2 000 = 7 200 000 counts. The write of stage 0's timeout (1 count) leaves the
        // count past it: stage 0 expires at that write, with its interrupt, and stage 1 counts from
        // 0 (TW §B.3). The other writes find the count at 0, below 1: nothing more happens.
        // Stage 1 is current, its wake-up one count after the writes.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        program_watchdog(
            ctx,
            &mut timg,
            [0xFFFF_FFFF, 1_200, 1_200, 1_200],
            [ACTION_INTERRUPT, 0],
        );
        after(ctx, &mut timg, 3_600 * SECOND);
        assert_eq!(timg.read(ctx, INT_RAW), Some(0));
        for (i, offset) in [WDTCONFIG2, WDTCONFIG2 + 4, WDTCONFIG2 + 8, WDTCONFIG5]
            .into_iter()
            .enumerate()
        {
            assert!(timg.write(ctx, offset, 1), "write {i}");
        }
        assert_eq!(timg.wdt_stage, 1);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_WDT));
        assert_eq!(ctx.take_reset(), None);
        assert_eq!(ctx.next_wake(), Some(ctx.now() + COUNT));
    }

    #[test]
    fn a_long_sleep_over_tiny_stages_is_walked_in_bounded_steps() {
        // Four one-count stages, a turn of 4 counts; stage 0 raises the interrupt. The wake-up due
        // after 1 count is delivered an hour late, plus 40 000 cycles (half a count, which no
        // whole count sees): the count is 7 200 000. Four expiries take 1 each and leave
        // 7 199 996, which is 0 modulo 4. Whole turns change neither the stage nor which actions
        // fired, so the walk goes on from stage 0 with a count of 0: the interrupt of stage 0 was
        // raised, and the next wake-up is one count away.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        program_watchdog(ctx, &mut timg, [1; 4], [ACTION_INTERRUPT, 0]);
        after(ctx, &mut timg, 3_600 * SECOND + COUNT / 2);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_WDT));
        assert_eq!(timg.wdt_stage, 0);
        assert_eq!(ctx.next_wake(), Some(ctx.now() + COUNT));
    }

    #[test]
    fn a_timeout_lowered_below_the_count_fires_at_the_write_and_the_next_stage_starts_from_0() {
        // Stage 0: interrupt after 2 000 counts (1 s), stage 1: reset the CPU after 1 600 (0.8 s).
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        program_watchdog(
            ctx,
            &mut timg,
            [2_000, 1_600, 1_048_575, 1_048_575],
            [ACTION_INTERRUPT, ACTION_RESET_CPU],
        );
        // Half a second: 1 000 counts. Stage 0 is lowered to 500, strictly below the count: it
        // expires at the write, and stage 1 counts from 0.
        after(ctx, &mut timg, SECOND / 2);
        assert_eq!(timg.read(ctx, INT_RAW), Some(0));
        timg.write(ctx, WDTCONFIG2, 500);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_WDT));
        assert_eq!(timg.wdt_stage, 1);
        assert_eq!(ctx.take_reset(), None);
        let started = ctx.now();
        assert_eq!(ctx.next_wake(), Some(started + 1_600 * COUNT), "from 0");
        // 125 counts into stage 1, its timeout goes from 1 600 to 800: still above the count,
        // nothing fires, and the wake-up moves up to 800 counts from the start.
        after(ctx, &mut timg, 125 * COUNT);
        timg.write(ctx, WDTCONFIG2 + 4, 800);
        assert_eq!(ctx.take_reset(), None);
        assert_eq!(timg.wdt_stage, 1);
        assert_eq!(ctx.next_wake(), Some(started + 800 * COUNT));
        // And it moves later when the timeout is raised: the wake-up must be the true one.
        timg.write(ctx, WDTCONFIG2 + 4, 3_200);
        assert_eq!(ctx.next_wake(), Some(started + 3_200 * COUNT));
    }

    #[test]
    fn an_expiry_delivered_late_carries_the_counts_since_the_deadline_into_the_next_stage() {
        // Stage 0 lasts 2 000 counts (160 000 000 cycles), stage 1 1 600. The wake-up is
        // delivered 400 001 cycles late: 5 whole counts (5.0000125), the 0.0000125 of a count
        // (one cycle) is lost. Stage 1 has 5 counts behind it when it starts at the delivery, and
        // its wake-up is 1 595 counts after that.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        program_watchdog(ctx, &mut timg, [2_000, 1_600, 1_048_575, 1_048_575], [0; 2]);
        assert_eq!(ctx.next_wake(), Some(Cycles(SECOND)));
        after(ctx, &mut timg, SECOND + 400_001);
        assert_eq!(timg.wdt_stage, 1);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(SECOND + 400_001 + 1_595 * COUNT))
        );
    }

    #[test]
    fn a_stopped_watchdog_leaves_no_wake_up_behind() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        assert_eq!(ctx.next_wake(), None, "nothing runs out of reset here");
        timg.write(ctx, WDTCONFIG0, WDT_EN);
        assert!(ctx.next_wake().is_some());
        timg.write(ctx, WDTCONFIG0, 0);
        assert_eq!(ctx.next_wake(), None);
    }

    #[test]
    fn a_firmware_that_feeds_or_toggles_the_watchdog_forever_does_not_grow_the_scheduler() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        program_watchdog(ctx, &mut timg, [0xFFFF_FFFF, 1_200, 1_200, 1_200], [0; 2]);
        // Each write cancels a wake-up hours ahead and schedules another.
        // One live wake-up, and the scheduler tolerates its slack of 32 cancelled ones.
        let bound = 40;
        for _ in 0..10_000 {
            timg.write(ctx, WDTFEED, 1);
            assert!(
                ctx.scheduled_entries() <= bound,
                "{}",
                ctx.scheduled_entries()
            );
        }
        for round in 0..10_000u64 {
            ctx.sleep_until(ctx.now() + 1 + round % 3);
            timg.write(ctx, WDTFEED, 1);
            assert!(
                ctx.scheduled_entries() <= bound,
                "{}",
                ctx.scheduled_entries()
            );
        }
        for _ in 0..10_000 {
            timg.write(ctx, WDTCONFIG0, 0);
            timg.write(ctx, WDTCONFIG0, WDT_EN | WDT_USE_XTAL);
            assert!(
                ctx.scheduled_entries() <= bound,
                "{}",
                ctx.scheduled_entries()
            );
        }
        assert!(ctx.next_wake().is_some(), "and the watchdog still runs");
    }

    #[test]
    fn a_group_that_is_reset_over_and_over_leaves_no_wake_up_behind() {
        // A peripheral reset or a core reset rebuilds the group: the wake-up of the group it
        // replaces is withdrawn, and the rebuilt group arms the flash-boot protection again.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        for _ in 0..10_000 {
            timg.reset(ctx);
            assert!(ctx.scheduled_entries() <= 40, "{}", ctx.scheduled_entries());
        }
        assert_eq!(ctx.next_wake(), Some(Cycles(13 * SECOND / 10)));
    }

    /// The one-shot measurement of `rtc_clk_cal_internal` (TW §B.4), `cycles` slow-clock cycles of
    /// the clock `clk_sel`.
    fn calibrate(ctx: &mut Ctx, timg: &mut Timg, clk_sel: u32, cycles: u32, shift: u32) {
        // Step 1: the cycling mode is on at reset; a threshold of 1 ends it by a timeout.
        assert_eq!(timg.read(ctx, RTCCALICFG), Some(0x0001_3000));
        assert!(timg.write(ctx, RTCCALICFG2, 0x0000_0098));
        assert_eq!(timg.read(ctx, RTCCALICFG2), Some(0x0000_0099), "TIMEOUT");
        // Steps 2 to 4.
        let config = (cycles << CALI_MAX_SHIFT) | (clk_sel << CALI_CLK_SEL_SHIFT);
        timg.write(ctx, RTCCALICFG, config);
        timg.write(
            ctx,
            RTCCALICFG2,
            ((cycles << shift) << CALI_THRES_SHIFT) | 0x18,
        );
        timg.write(ctx, RTCCALICFG, config);
        timg.write(ctx, RTCCALICFG, config | CALI_START);
    }

    #[test]
    fn the_rc_slow_calibration_is_ready_after_7_53_ms_with_301_176_crystal_cycles() {
        // TW §B.4: 1 024 cycles of the 136 kHz RC; `RTCCALICFG1` reads 0x024C3C00.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        calibrate(ctx, &mut timg, 0, 1_024, 10);
        assert_eq!(timg.read(ctx, RTCCALICFG).map(|v| v & CALI_RDY), Some(0));
        assert_eq!(
            timg.read(ctx, RTCCALICFG1),
            Some(0),
            "no value before it is ready"
        );
        // 301 176 crystal cycles: 7.5294 ms.
        ctx.sleep_until(Cycles(ctx.now().get() + 301_176 * 4 - 1));
        assert_eq!(timg.read(ctx, RTCCALICFG).map(|v| v & CALI_RDY), Some(0));
        ctx.sleep_until(ctx.now() + 1);
        assert_eq!(timg.read(ctx, RTCCALICFG), Some(0x8400_8000));
        assert_eq!(timg.read(ctx, RTCCALICFG1), Some(0x024C_3C00));
        assert_eq!(
            timg.read(ctx, RTCCALICFG2).map(|v| v & CALI_TIMEOUT),
            Some(0)
        );
        // Step 7: `START` is cleared; the result stays until the next start.
        timg.write(ctx, RTCCALICFG, 0x0400_8000);
        assert_eq!(timg.read(ctx, RTCCALICFG), Some(0x0400_8000));
    }

    #[test]
    fn the_rc_fast_calibration_counts_599_186_crystal_cycles() {
        // TW §B.4: 17.5 MHz / 256; `RTCCALICFG1` reads 0x04924900.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        calibrate(ctx, &mut timg, 1, 1_024, 12);
        ctx.sleep_until(ctx.now() + 599_186 * 4);
        assert_eq!(timg.read(ctx, RTCCALICFG1), Some(0x0492_4900));
    }

    #[test]
    fn a_calibration_of_the_missing_32_khz_crystal_times_out_without_a_result() {
        // Pins the ASSUMPTION of `Calibration::start`.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        calibrate(ctx, &mut timg, 2, 1_024, 12);
        assert_eq!(
            timg.read(ctx, RTCCALICFG2).map(|v| v & CALI_TIMEOUT),
            Some(0)
        );
        // The threshold: 1 024 << 12 crystal cycles, 104.9 ms.
        ctx.sleep_until(ctx.now() + (1_024 << 12) * 4);
        assert_eq!(
            timg.read(ctx, RTCCALICFG2).map(|v| v & CALI_TIMEOUT),
            Some(1)
        );
        assert_eq!(timg.read(ctx, RTCCALICFG).map(|v| v & CALI_RDY), Some(0));
        assert_eq!(timg.read(ctx, RTCCALICFG1), Some(0));
    }

    fn t0_value(ctx: &mut Ctx, timg: &mut Timg) -> u64 {
        assert!(timg.write(ctx, T0UPDATE, 1));
        assert_eq!(timg.read(ctx, T0UPDATE), Some(0), "the capture is done");
        (u64::from(timg.read(ctx, T0HI).unwrap()) << 32) | u64::from(timg.read(ctx, T0LO).unwrap())
    }

    #[test]
    fn t0_counts_the_apb_clock_divided_and_keeps_its_value_when_stopped() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        // EN, counting up, divider 20: 20 MHz / 20, one count per microsecond.
        timg.write(
            ctx,
            T0CONFIG,
            T0_EN | T0_INCREASE | (20 << T0_DIVIDER_SHIFT),
        );
        after(ctx, &mut timg, MILLISECOND);
        assert_eq!(t0_value(ctx, &mut timg), 1_000);
        timg.write(ctx, T0CONFIG, T0_INCREASE | (20 << T0_DIVIDER_SHIFT));
        after(ctx, &mut timg, MILLISECOND);
        assert_eq!(t0_value(ctx, &mut timg), 1_000, "frozen");
        // The reset divider, 1, divides by 2; the crystal as source: 20 MHz.
        timg.write(
            ctx,
            T0CONFIG,
            T0_EN | T0_INCREASE | (1 << T0_DIVIDER_SHIFT) | T0_USE_XTAL,
        );
        after(ctx, &mut timg, MILLISECOND);
        assert_eq!(t0_value(ctx, &mut timg), 21_000);
    }

    #[test]
    fn t0_counts_down_and_reloads() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0LOADLO, 5_000);
        timg.write(ctx, T0LOADHI, 0);
        timg.write(ctx, T0LOAD, 1);
        timg.write(ctx, T0CONFIG, T0_EN | (20 << T0_DIVIDER_SHIFT));
        after(ctx, &mut timg, MILLISECOND);
        assert_eq!(t0_value(ctx, &mut timg), 4_000);
        timg.write(ctx, T0LOAD, 0);
        assert_eq!(
            t0_value(ctx, &mut timg),
            5_000,
            "any write to T0LOAD reloads"
        );
    }

    #[test]
    fn the_t0_alarm_interrupts_clears_its_enable_and_reloads_the_counter() {
        // TW §B.2. Source TG_T0 is 32.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        timg.write(ctx, WDTCONFIG0, 0);
        timg.write(ctx, INT_ENA, INT_T0);
        timg.write(ctx, T0ALARMLO, 1_000);
        timg.write(ctx, T0ALARMHI, 0);
        let config = T0_EN | T0_INCREASE | T0_AUTORELOAD | (20 << T0_DIVIDER_SHIFT);
        timg.write(ctx, T0CONFIG, config | T0_ALARM_EN);
        after(ctx, &mut timg, MILLISECOND - 1);
        assert_eq!(timg.read(ctx, INT_RAW), Some(0));
        after(ctx, &mut timg, 1);
        assert_eq!(timg.read(ctx, INT_ST), Some(INT_T0));
        assert_eq!(ctx.sources(), 1 << 32);
        assert_eq!(
            timg.read(ctx, T0CONFIG),
            Some(config),
            "ALARM_EN cleared by hardware"
        );
        assert_eq!(t0_value(ctx, &mut timg), 0, "reloaded from T0LOAD");
        timg.write(ctx, INT_CLR, INT_T0);
        assert_eq!(ctx.sources(), 0);
        // An alarm value already passed fires at once when the alarm is enabled.
        after(ctx, &mut timg, 2 * MILLISECOND);
        timg.write(ctx, T0CONFIG, config | T0_ALARM_EN);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_T0));
    }

    /// Time-base cycles of one count of T0 with the APB clock at 20 MHz and a divider of 20.
    const T0_COUNT: u64 = 160;
    const T0_COUNTING: u32 = T0_EN | T0_INCREASE | (20 << T0_DIVIDER_SHIFT);

    #[test]
    fn t0config_rewritten_with_the_same_value_does_not_lose_the_fraction_of_a_count() {
        // A count is 160 cycles and the register is rewritten every 159: after 1 000 rounds
        // 159 000 cycles have gone by, which is 993 whole counts (993.75). A rewrite that
        // re-anchored the counter would lose the 159 cycles each time and read 0.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0CONFIG, T0_COUNTING);
        for _ in 0..1_000 {
            ctx.sleep_until(ctx.now() + 159);
            timg.write(ctx, T0CONFIG, T0_COUNTING);
        }
        assert_eq!(t0_value(ctx, &mut timg), 993);
    }

    #[test]
    fn a_periodic_alarm_that_is_enabled_again_late_falls_at_the_same_period_every_time() {
        // Alarm after 1 000 counts (160 000 cycles), reload from 0. The driver enables the alarm
        // again 100.5 counts after each interrupt (the hardware cleared `ALARM_EN`), and the
        // wake-up is delivered 90 cycles late (0.56 count). The counter restarts at the instant of
        // the alarm and keeps its phase, so the next alarm is due exactly one period after the
        // last: the Nth alarm falls at N periods, with no tolerance.
        const PERIOD: u64 = 1_000 * T0_COUNT;
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0ALARMLO, 1_000);
        timg.write(ctx, T0CONFIG, T0_COUNTING | T0_AUTORELOAD | T0_ALARM_EN);
        for n in 1..=1_000u64 {
            assert_eq!(ctx.next_wake(), Some(Cycles(n * PERIOD)), "alarm {n}");
            ctx.sleep_until(Cycles(n * PERIOD + 90));
            pump(ctx, &mut timg);
            assert_eq!(timg.read(ctx, INT_RAW), Some(INT_T0), "alarm {n}");
            timg.write(ctx, INT_CLR, INT_T0);
            ctx.sleep_until(Cycles(n * PERIOD + 16_080));
            timg.write(ctx, T0CONFIG, T0_COUNTING | T0_AUTORELOAD | T0_ALARM_EN);
        }
        assert_eq!(ctx.next_wake(), Some(Cycles(1_001 * PERIOD)));
    }

    #[test]
    fn divcnt_rst_drops_the_fraction_of_a_count() {
        // `DIVCNT_RST` restarts the divider counter (`timer_group_reg.h`, written by
        // `timer_ll_set_clock_prescale`). 159 cycles into a count the trigger is written: the
        // count that was to come 1 cycle later comes 160 cycles after the trigger.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0CONFIG, T0_COUNTING);
        ctx.sleep_until(Cycles(159));
        timg.write(ctx, T0CONFIG, T0_COUNTING | (1 << 12));
        assert_eq!(timg.read(ctx, T0CONFIG), Some(T0_COUNTING), "a trigger");
        ctx.sleep_until(Cycles(160));
        assert_eq!(t0_value(ctx, &mut timg), 0);
        ctx.sleep_until(Cycles(318));
        assert_eq!(t0_value(ctx, &mut timg), 0);
        ctx.sleep_until(Cycles(319));
        assert_eq!(t0_value(ctx, &mut timg), 1);
    }

    #[test]
    fn flipping_the_direction_keeps_the_value() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0CONFIG, T0_COUNTING);
        ctx.sleep_until(Cycles(1_000 * T0_COUNT));
        timg.write(ctx, T0CONFIG, T0_COUNTING & !T0_INCREASE);
        assert_eq!(t0_value(ctx, &mut timg), 1_000, "kept");
        ctx.sleep_until(ctx.now() + 400 * T0_COUNT);
        assert_eq!(t0_value(ctx, &mut timg), 600, "counting down from now");
        timg.write(ctx, T0CONFIG, T0_COUNTING);
        ctx.sleep_until(ctx.now() + 100 * T0_COUNT);
        assert_eq!(t0_value(ctx, &mut timg), 700, "and up again");
    }

    #[test]
    fn a_divider_changed_while_counting_takes_effect_at_once_and_keeps_the_count() {
        // ASSUMPTION of `Timg::write` (T0CONFIG): see its comment.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0CONFIG, T0_COUNTING);
        ctx.sleep_until(Cycles(1_000 * T0_COUNT));
        timg.write(
            ctx,
            T0CONFIG,
            T0_EN | T0_INCREASE | (40 << T0_DIVIDER_SHIFT),
        );
        assert_eq!(t0_value(ctx, &mut timg), 1_000);
        ctx.sleep_until(ctx.now() + 1_000 * T0_COUNT);
        assert_eq!(t0_value(ctx, &mut timg), 1_500, "one count per 320 cycles");
    }

    #[test]
    fn the_t0_divider_field_0_divides_by_65_536_and_1_by_2() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0CONFIG, T0_EN | T0_INCREASE);
        ctx.sleep_until(Cycles(8 * 65_536 - 1));
        assert_eq!(t0_value(ctx, &mut timg), 0);
        ctx.sleep_until(Cycles(8 * 65_536));
        assert_eq!(t0_value(ctx, &mut timg), 1, "field 0: 65 536 APB cycles");
        timg.write(ctx, T0LOAD, 1);
        timg.write(ctx, T0CONFIG, T0_EN | T0_INCREASE | (1 << T0_DIVIDER_SHIFT));
        assert_eq!(t0_value(ctx, &mut timg), 0);
        ctx.sleep_until(ctx.now() + 16 * 100);
        assert_eq!(t0_value(ctx, &mut timg), 100, "field 1: 2 APB cycles");
    }

    #[test]
    fn a_watchdog_prescaler_of_0_divides_by_65_536() {
        // Pins the ASSUMPTION of `Timg::wdt_rate`: one count is 65 536 crystal cycles.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, WDTCONFIG1, 0);
        timg.write(ctx, WDTCONFIG2, 1);
        timg.write(ctx, WDTCONFIG0, WDT_EN | WDT_USE_XTAL);
        assert_eq!(ctx.next_wake(), Some(Cycles(XTAL_PERIOD * 65_536)));
    }

    #[test]
    fn a_down_counting_timer_reaches_its_alarm_going_down() {
        // From 5 000 down to the alarm at 4 000: 1 000 counts, 1 ms.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0LOADLO, 5_000);
        timg.write(ctx, T0LOAD, 1);
        timg.write(ctx, T0ALARMLO, 4_000);
        timg.write(
            ctx,
            T0CONFIG,
            T0_EN | (20 << T0_DIVIDER_SHIFT) | T0_ALARM_EN,
        );
        assert_eq!(ctx.next_wake(), Some(Cycles(1_000 * T0_COUNT)));
        after(ctx, &mut timg, 1_000 * T0_COUNT - 1);
        assert_eq!(timg.read(ctx, INT_RAW), Some(0));
        after(ctx, &mut timg, 1);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_T0));
        assert_eq!(t0_value(ctx, &mut timg), 4_000);
    }

    #[test]
    fn an_alarm_without_auto_reload_fires_once_and_the_counter_goes_on() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0ALARMLO, 1_000);
        timg.write(ctx, T0CONFIG, T0_COUNTING | T0_ALARM_EN);
        after(ctx, &mut timg, 1_000 * T0_COUNT);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_T0));
        assert_eq!(
            timg.read(ctx, T0CONFIG),
            Some(T0_COUNTING),
            "ALARM_EN cleared"
        );
        timg.write(ctx, INT_CLR, INT_T0);
        after(ctx, &mut timg, 1_000 * T0_COUNT);
        assert_eq!(timg.read(ctx, INT_RAW), Some(0), "once");
        assert_eq!(t0_value(ctx, &mut timg), 2_000, "and not reloaded");
        assert_eq!(ctx.next_wake(), None);
    }
}
