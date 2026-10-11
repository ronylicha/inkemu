//! `RTC_CNTL`: the reset cause and the software resets, the retention registers, the RTC timer,
//! the RTC watchdog and the super watchdog, the RTC interrupts, and pad, clock, analog and power
//! configuration. Sources: TRM §6.1, §9.3, §12.2, §12.3, ESP-IDF `rtc_cntl_reg.h`, `rwdt_ll.h`,
//! `rtc_cntl_ll.h`; reports BC §E.2, IR §4, TW §C and §D.
//!
//! This block is in the RTC power domain: a CPU reset and a core reset leave it as it is, RTC
//! watchdog counter included; a system reset returns its registers to their reset values; nothing
//! but a power-on stops the RTC timer (TRM §9.3.3; TW §D.2).
use crate::clock::SECOND;
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::Unsupported;
use crate::periph::counter::{Counter, Rate};
use crate::periph::stages::{self, STAGES as WDT_STAGES};
use crate::periph::{Accepted, RTC_CNTL_BASE, Storage, consecutive, half_of, index_in};
use crate::reset::{CAUSE_POWER_ON, ResetCause};
use emu_core::{Cycles, Peripheral};
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("RTC_CNTL", name) - RTC_CNTL_BASE
}

const OPTIONS0: u32 = reg("OPTIONS0");
/// `OPTIONS0` bit 31 `SW_SYS_RST` (`ROM3:software_reset@0x40048b74`) and bit 5 `SW_PROCPU_RST`
/// (`ROM3:software_reset_cpu@0x40048b84`, the last store of `esp_restart`): each resets when it is
/// written 1 and reads 0 (report IR §4.1, §8 item 14).
const SW_SYS_RST: u32 = 1 << 31;
const SW_PROCPU_RST: u32 = 1 << 5;
const TIME_UPDATE: u32 = reg("TIME_UPDATE");
const TIME_LOW0: u32 = reg("TIME_LOW0");
const TIME_HIGH0: u32 = reg("TIME_HIGH0");
const TIME_LOW1: u32 = reg("TIME_LOW1");
const TIME_HIGH1: u32 = reg("TIME_HIGH1");
/// `TIME_UPDATE` bit 31, write-only: captures the RTC timer (
/// `rtc_cntl_ll_get_rtc_time`, `IDF:hal/esp32c3/include/hal/rtc_cntl_ll.h:74-80`).
const TIME_UPDATE_TRIGGER: u32 = 1 << 31;
/// The RTC timer is 48 bits wide: 16 bits in `TIME_HIGH0` (TRM §9.3.3).
const RTC_TIME_MASK: u64 = (1 << 48) - 1;
const RESET_STATE: u32 = reg("RESET_STATE");
const INT_ENA: u32 = reg("INT_ENA_RTC");
const INT_RAW: u32 = reg("INT_RAW_RTC");
const INT_ST: u32 = reg("INT_ST_RTC");
const INT_CLR: u32 = reg("INT_CLR_RTC");
const INT_ENA_W1TS: u32 = reg("INT_ENA_RTC_W1TS");
const INT_ENA_W1TC: u32 = reg("INT_ENA_RTC_W1TC");
/// `INT_RAW_RTC` bit 3: the RTC watchdog (`rwdt_ll_set_intr_enable`,
/// `IDF:hal/esp32c3/include/hal/rwdt_ll.h:297`); bit 15: the feed request of the super watchdog
/// (`RTC_CNTL_SWD_INT_RAW`, `rtc_cntl_reg.h:637`).
const INT_WDT: u32 = 1 << 3;
const INT_SWD: u32 = 1 << 15;
const CLK_CONF: u32 = reg("CLK_CONF");
/// `CLK_CONF` bits [31:30] `ANA_CLK_RTC_SEL`: the slow clock, 0 = the RC oscillator
/// (`rtc_cntl_reg.h:971`; `clk_ll_rtc_slow_set_src`).
const ANA_CLK_RTC_SEL_SHIFT: u32 = 30;
const WDTCONFIG0: u32 = reg("WDTCONFIG0");
const WDTCONFIG1: u32 = reg("WDTCONFIG1");
#[cfg(test)]
const WDTCONFIG4: u32 = reg("WDTCONFIG4");
// The four stage timeout registers follow one another: they are indexed by their offset.
const _: () = assert!(consecutive(
    "RTC_CNTL",
    &["WDTCONFIG1", "WDTCONFIG2", "WDTCONFIG3", "WDTCONFIG4"]
));
const WDTFEED: u32 = reg("WDTFEED");
const WDTWPROTECT: u32 = reg("WDTWPROTECT");
const SWD_CONF: u32 = reg("SWD_CONF");
const SWD_WPROTECT: u32 = reg("SWD_WPROTECT");
const FIB_SEL: u32 = reg("FIB_SEL");

/// The RTC slow clock: the 136 kHz RC oscillator, 17 cycles every 20 000 cycles of the time base.
///
/// ASSUMPTION: the oscillator runs at the nominal frequency ESP-IDF computes every timeout from
/// (`SOC_CLK_RC_SLOW_FREQ_APPROX`, `IDF:soc/esp32c3/include/soc/clk_tree_defs.h`), from power-on. A
/// real chip deviates from it, and runs at another, undocumented frequency until the bootloader
/// has written `SCK_DCAP` (TW §C.1, §I question 2). A measurement on a real unit settles it.
const SLOW_CLOCK: Rate = Rate::ratio(17, 20_000);
// 136 000 cycles of the slow clock in one second of the time base.
const _: () = assert!(SECOND * 17 == 136_000 * 20_000);

// Reset values of the RTC watchdog registers (`rtc_cntl_reg.h:1565-1705`; TW §C.3).
/// `FLASHBOOT_MOD_EN` and `PAUSE_IN_SLP` set, reset lengths 1, chip reset width 20.
const WDTCONFIG0_RESET: u32 = 0x0001_3214;
/// Stage timeouts 0 to 3: 200 000, 80 000, 0xFFF, 0xFFF cycles of the slow clock.
const WDT_TIMEOUT_RESET: [u32; WDT_STAGES] = [200_000, 80_000, 0xFFF, 0xFFF];
/// The key that unlocks the watchdog registers, and the reset value of `WDTWPROTECT`
/// (`RTC_CNTL_WDT_WKEY`, default `0x50d83aa1`, `rtc_cntl_reg.h:1700`): unlocked out of reset. The
/// SVD gives 0 for this register and is wrong on this point.
const WDT_KEY: u32 = 0x50D8_3AA1;
/// `EFUSE_WDT_DELAY_SEL` is two bits wide.
const WDT_DELAY_SEL_MASK: u32 = 0b11;
/// `WDTCONFIG0`: bit 31 `EN` (`rwdt_ll_enable`, `rwdt_ll.h:66`), three bits per stage from
/// [30:28] for stage 0 to [21:19] for stage 3 (`rwdt_ll_config_stage`, line 113), bit 12
/// `FLASHBOOT_MOD_EN` (`rwdt_ll_set_flashboot_en`, line 196). The rest is stored.
const WDT_EN: u32 = 1 << 31;
const WDT_STAGE0_SHIFT: u32 = 28;
const WDT_STAGE_BITS: u32 = 3;
const WDT_STAGE_MASK: u32 = 0b111;
const WDT_FLASHBOOT_MOD_EN: u32 = 1 << 12;
/// `WDTFEED` bit 31 feeds (`rwdt_ll_feed`, line 266).
const WDT_FEED: u32 = 1 << 31;
/// Stage actions (`IDF:hal/include/hal/wdt_types.h`; TW §D.5).
const ACTION_INTERRUPT: u32 = 1;
const ACTION_RESET_CPU: u32 = 2;
const ACTION_RESET_SYSTEM: u32 = 3;
const ACTION_RESET_RTC: u32 = 4;

/// `SWD_CONF` (reset `0x04B00000`): bit 31 `AUTO_FEED_EN`, bit 30 `DISABLE`, bit 29 `FEED` and bit
/// 28 `RST_FLAG_CLR` (write-only), bits [27:18] `SIGNAL_WIDTH`, bit 17 `BYPASS_RST`, bit 1
/// `FEED_INT` and bit 0 `RESET_FLAG` (read-only) (`rtc_cntl_reg.h:1706-1757`). The bootloader
/// clears `BYPASS_RST` and sets `AUTO_FEED_EN` (`IDF:bootloader_support/src/esp32c3/bootloader_soc.c:10-19`,
/// `bootloader_esp32c3.c:79-84`); the ROM reads `RESET_FLAG` and clears it with `RST_FLAG_CLR`
/// (`ROM3:analog_super_wdt_reset_happened@0x40048a26`, `clear_super_wdt_reset_flag@0x40048b58`).
const SWD_CONF_RESET: u32 = 0x04B0_0000;
const SWD_AUTO_FEED_EN: u32 = 1 << 31;
const SWD_DISABLE: u32 = 1 << 30;
const SWD_FEED: u32 = 1 << 29;
const SWD_RST_FLAG_CLR: u32 = 1 << 28;
const SWD_BYPASS_RST: u32 = 1 << 17;
const SWD_FEED_INT: u32 = 1 << 1;
const SWD_RESET_FLAG: u32 = 1 << 0;
const SWD_CONF_STORED: u32 = 0xCFFE_0000;
/// The key of the super watchdog, and the reset value of `SWD_WPROTECT` (`RTC_CNTL_SWD_WKEY`,
/// default `0x8f1d312a`, `rtc_cntl_reg.h:1758`). The SVD gives 0 and is wrong on this point too.
const SWD_KEY: u32 = 0x8F1D_312A;
/// `FIB_SEL` bit 2, `RTC_CNTL_FIB_SUPER_WDT_RST` (`rtc_cntl_reg.h:2341`), reset value 1.
const FIB_SUPER_WDT_RST: u32 = 1 << 2;
/// ASSUMPTION: the super watchdog asks to be fed after 0.9 s and resets after 1 s. The TRM gives
/// "slightly less than one second" for the period and "about 100 ms" before it for the request
/// (§12.3; TW §C.4); the circuit is analog and no figure is published. A real chip left unfed,
/// observed with the feed-request interrupt enabled, settles it.
const SWD_FEED_REQUEST: u64 = 9 * SECOND / 10;
const SWD_TIMEOUT: u64 = SECOND;

const STORE_COUNT: usize = 8;
/// `RTC_CNTL_STORE0..7`: they survive a reset; the ROM keeps the APB frequency in STORE5.
const STORE: [u32; STORE_COUNT] = [
    addr("RTC_CNTL", "STORE0") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE1") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE2") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE3") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE4") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE5") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE6") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE7") - RTC_CNTL_BASE,
];
/// Configuration the ROM, the bootloader and the application write and read back. Nothing acts on
/// it: there is no analog part, no power domain and no sleep in this model.
const ACCEPTED: [Accepted; 18] = [
    Accepted::svd(
        "RTC_CNTL",
        "OPTIONS0",
        "BBPLL, crystal and I2C power forcing, CPU stall (the two software reset bits act)",
        "SP1b-3b",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "TIMER1",
        "clock and PLL wake-up delays",
        "SP1b-3b",
    ),
    Accepted::svd("RTC_CNTL", "TIMER3", "Wi-Fi/BT power-up timing", "SP1b-3b"),
    Accepted::svd("RTC_CNTL", "TIMER4", "digital power-up timing", "SP1b-3b"),
    Accepted::svd("RTC_CNTL", "TIMER5", "minimum sleep time", "SP1b-3b"),
    Accepted::svd(
        "RTC_CNTL",
        "TIMER6",
        "peripheral power-up timing",
        "SP1b-3b",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "ANA_CONF",
        "analog configuration, glitch reset",
        "SP8",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "CLK_CONF",
        "RTC fast clock source and dividers (a slow clock other than the RC oscillator is refused)",
        "SP8",
    ),
    Accepted::svd("RTC_CNTL", "SLOW_CLK_CONF", "RTC slow clock divider", "SP8"),
    Accepted::svd(
        "RTC_CNTL",
        "RTC_CNTL",
        "RTC regulator and digital bias",
        "SP8",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "DIG_PWC",
        "digital power-domain control",
        "SP1b-3b",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "BROWN_OUT",
        "brown-out detector: the supply is always good",
        "SP2",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "FIB_SEL",
        "brown-out and glitch reset selection (the super watchdog bit acts)",
        "SP2",
    ),
    Accepted::svd("RTC_CNTL", "SENSOR_CTRL", "SAR ADC power forcing", "SP2"),
    Accepted::svd("RTC_CNTL", "PWC", "RTC power-domain control", "SP1b-3b"),
    Accepted::svd("RTC_CNTL", "DIG_ISO", "digital pad isolation", "SP1b-3b"),
    Accepted::svd("RTC_CNTL", "PAD_HOLD", "pad hold", "SP2"),
    Accepted::svd("RTC_CNTL", "DIG_PAD_HOLD", "digital pad hold", "SP2"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// `RESET_CAUSE_PROCPU`, bits [5:0] of `RESET_STATE` (`soc/reset_reasons.h`).
const RESET_CAUSE_MASK: u32 = 0x3F;

#[derive(Clone, Debug)]
pub struct RtcCntl {
    /// Whether the chip boots from flash: the strapping the flash-boot protection depends on.
    flash_boot: bool,
    /// `EFUSE_WDT_DELAY_SEL`: stage 0 of the watchdog lasts `WDTCONFIG1 << (1 + this)` cycles.
    wdt_delay_sel: u32,
    reset_cause: u32,
    /// The bits of `RESET_STATE` other than the cause, at their documented reset value.
    reset_state_other: u32,
    store: [u32; STORE_COUNT],
    config: Storage,
    /// The RTC timer: slow-clock cycles since power-on.
    timer: Counter,
    /// The last capture, and the one before it.
    time: [u64; 2],
    int_ena: u32,
    int_raw: u32,
    wdt_config0: u32,
    wdt_timeout: [u32; WDT_STAGES],
    wdt_key: u32,
    /// Slow-clock cycles since the last feed or the start of the current stage.
    wdt: Counter,
    wdt_stage: usize,
    wdt_enabled: bool,
    swd_conf: u32,
    swd_key: u32,
    /// Device time of the last feed of the super watchdog, while it is armed.
    swd_fed_at: Option<Cycles>,
    wake: Deadline,
}

/// The RTC interrupts share one source, `RTC_CORE` (27: report IR §3.2).
const RTC_CORE: Source = Source::of("RTC_CORE_INTR_MAP");

impl RtcCntl {
    /// The block after a power-on. `flash_boot` says whether the chip was strapped to boot from
    /// flash, `wdt_delay_sel` is the eFuse field of that name.
    ///
    /// The owner calls [`RtcCntl::wake`] once afterwards: the reset values arm the flash-boot
    /// protection of the RTC watchdog, which needs its first deadline.
    #[must_use]
    pub fn new(flash_boot: bool, wdt_delay_sel: u32, now: Cycles) -> RtcCntl {
        let mut rtc = RtcCntl {
            flash_boot,
            wdt_delay_sel: wdt_delay_sel & WDT_DELAY_SEL_MASK,
            reset_cause: CAUSE_POWER_ON,
            reset_state_other: 0,
            store: [0; STORE_COUNT],
            config: Storage::accepted(RTC_CNTL_BASE, &ACCEPTED),
            timer: Counter::new(0, now, SLOW_CLOCK, true),
            time: [0; 2],
            int_ena: 0,
            int_raw: 0,
            wdt_config0: 0,
            wdt_timeout: WDT_TIMEOUT_RESET,
            wdt_key: 0,
            wdt: Counter::new(0, now, SLOW_CLOCK, false),
            wdt_stage: 0,
            wdt_enabled: false,
            swd_conf: 0,
            swd_key: 0,
            swd_fed_at: None,
            wake: Deadline::default(),
        };
        rtc.reset_registers();
        rtc
    }

    /// Returns every register to its reset value. The RTC timer and the reset cause are no
    /// registers of this kind: they stay.
    fn reset_registers(&mut self) {
        let documented =
            svd_regs::esp32c3::lookup(RTC_CNTL_BASE + RESET_STATE).map_or(0, |r| r.reset_value);
        self.reset_state_other = documented & !RESET_CAUSE_MASK;
        // ASSUMPTION: a system reset clears `STORE0..7`. The TRM says it resets "the whole digital
        // system, including RTC" and does not list the retention registers (TW §I question 9); the
        // registers read after a `rst:0x10` on a real chip settle it.
        self.store = [0; STORE_COUNT];
        self.config = Storage::accepted(RTC_CNTL_BASE, &ACCEPTED);
        self.time = [0; 2];
        self.int_ena = 0;
        self.int_raw = 0;
        self.wdt_config0 = WDTCONFIG0_RESET;
        self.wdt_timeout = WDT_TIMEOUT_RESET;
        self.wdt_key = WDT_KEY;
        self.wdt_stage = 0;
        self.wdt_enabled = false;
        self.swd_conf = SWD_CONF_RESET;
        self.swd_key = SWD_KEY;
        self.swd_fed_at = None;
    }

    /// A reset took place: the next boot reads its cause in `RESET_STATE`. A system reset also
    /// returns the registers of this block to their reset values (TW §D.2).
    pub fn reset(&mut self, ctx: &mut Ctx, cause: ResetCause, system: bool) {
        if system {
            let swd_reset = self.swd_conf & SWD_RESET_FLAG;
            self.reset_registers();
            // The flag that says the super watchdog caused the reset is what the next boot reads.
            self.swd_conf |= swd_reset;
        }
        self.reset_cause = cause.code();
        self.wake(ctx);
    }

    fn rtc_time(&self, now: Cycles) -> u64 {
        self.timer.value(now) & RTC_TIME_MASK
    }

    /// Whether the flash-boot protection runs the watchdog (TRM §12.2.2.4; TW §D.4).
    ///
    /// ASSUMPTION: as for the main watchdog (`Timg::flash_boot_protection`), the protection is off
    /// when the chip is not strapped to boot from flash (TW §I question 4). A boot of a chip
    /// strapped for download, with the RTC watchdog counting, settles it.
    const fn flash_boot_protection(&self) -> bool {
        self.flash_boot && self.wdt_config0 & WDT_FLASHBOOT_MOD_EN != 0
    }

    /// The action of a stage.
    ///
    /// ASSUMPTION: under the flash-boot protection stage 0 is a system reset after the timeout
    /// `WDTCONFIG1` holds, whatever `STG0` says. The TRM gives the action (§12.2.2.4); that the
    /// timeout is the programmed one matters after `esp_restart`, which programs 1 s and sets the
    /// protection bit without `EN` (`IDF:esp_system/port/soc/esp32c3/system_internal.c:71-80`; TW
    /// §C.3, §I question 3). A slow second boot on a real chip settles it.
    fn stage_action(&self, stage: usize) -> u32 {
        if stage == 0 && self.flash_boot_protection() {
            return ACTION_RESET_RTC;
        }
        // INVARIANT: `stage` is below `WDT_STAGES` (4), so the shift stays inside the register.
        let shift = WDT_STAGE0_SHIFT - WDT_STAGE_BITS * u32::try_from(stage).unwrap_or(0);
        (self.wdt_config0 >> shift) & WDT_STAGE_MASK
    }

    /// The timeout of a stage, in slow-clock cycles. Stage 0 lasts `WDTCONFIG1 << (1 + delay)`,
    /// which is why `rwdt_ll_config_stage` writes its value shifted the other way (`rwdt_ll.h:119`;
    /// TRM §12.2.2.2; TW §C.3).
    ///
    /// ASSUMPTION: a stage lasts at least one count, so a timeout of 0 counts as 1: what a stage
    /// timeout of 0 does on silicon is not documented. Programming 0 on a chip and timing the
    /// reset settles it.
    fn stage_timeout(&self, stage: usize) -> u64 {
        let written = u64::from(self.wdt_timeout[stage]);
        let counted = if stage == 0 {
            written << (1 + self.wdt_delay_sel)
        } else {
            written
        };
        counted.max(1)
    }

    /// Whether the super watchdog can reset the chip.
    ///
    /// ASSUMPTION: its reset is out of software's hands, and inactive, until `FIB_SEL` bit 2 is
    /// cleared; `BYPASS_RST` and `DISABLE` switch it off, and `AUTO_FEED_EN` makes the controller
    /// feed it for ever. This reading comes from the bootloader code that hands the reset to
    /// software (`bootloader_soc.c:10-19`) and from the absence of any `rst:0x12` in the logs of
    /// chips that never reach the bootloader (TW §C.4, §I question 11). A chip running the ROM
    /// with the three bits as they are out of reset, observed for several seconds, settles it.
    fn swd_armed(&self) -> bool {
        self.config.read(FIB_SEL).unwrap_or(FIB_SUPER_WDT_RST) & FIB_SUPER_WDT_RST == 0
            && self.swd_conf & (SWD_AUTO_FEED_EN | SWD_DISABLE | SWD_BYPASS_RST) == 0
    }

    /// Raises what is due at the present time, then asks to be woken for the next deadline.
    fn evaluate(&mut self, ctx: &mut Ctx, from_register_write: bool) {
        let now = ctx.now();
        // The deadline is worked out again at every call and no earlier wake-up is kept: a register
        // write can move the deadline later, and the instant `next_wake` gives the sleeping CPU
        // must be the true one. The scheduler bounds the cancelled entries this leaves.
        self.wake.cancel(ctx);

        let enabled = self.wdt_config0 & WDT_EN != 0 || self.flash_boot_protection();
        if enabled && !self.wdt_enabled {
            // ASSUMPTION: a watchdog that is switched on counts its first stage from 0. A real
            // chip whose watchdog is enabled after a long run, left unfed, settles it.
            self.wdt.set(0, now);
            self.wdt_stage = 0;
        }
        self.wdt_enabled = enabled;
        self.wdt.run(enabled, now);
        let mut earliest = if self.wdt.is_running() {
            self.walk_stages(ctx, now, from_register_write)
        } else {
            None
        };
        if let Some(at) = self.step_super_watchdog(ctx, now) {
            earliest = Some(earliest.map_or(at, |e| e.min(at)));
        }
        if let Some(at) = earliest {
            self.wake.arm(ctx, at, Event::RtcWatchdog);
        }
        self.drive(ctx);
    }

    /// Lets the RTC watchdog go through the stages whose timeout the count has reached, and returns
    /// the instant of the next expiry. The walk and its rules are those of every watchdog of the
    /// chip ([`stages::walk`]); what is this device's own is the action of a stage that ended.
    fn walk_stages(
        &mut self,
        ctx: &mut Ctx,
        now: Cycles,
        from_register_write: bool,
    ) -> Option<Cycles> {
        let timeouts: [u64; WDT_STAGES] = std::array::from_fn(|stage| self.stage_timeout(stage));
        // The actions as they are when the walk begins: no expiry changes a register they are
        // read from.
        let actions: [u32; WDT_STAGES] = std::array::from_fn(|stage| self.stage_action(stage));
        let int_raw = &mut self.int_raw;
        stages::walk(
            &mut self.wdt,
            &mut self.wdt_stage,
            timeouts,
            now,
            from_register_write,
            |ended| match actions[ended] {
                ACTION_INTERRUPT => *int_raw |= INT_WDT,
                // ASSUMPTION: action 2 resets the CPU whatever `PROCPU_RESET_EN` (`WDTCONFIG0` bit
                // 11) holds (TW §D.5, §I question 5). Clearing the bit on a chip whose stage
                // resets the CPU, and seeing whether the CPU still resets, settles it.
                ACTION_RESET_CPU => ctx.request_reset(ResetCause::RtcWatchdogCpu),
                ACTION_RESET_SYSTEM => ctx.request_reset(ResetCause::RtcWatchdogCore),
                ACTION_RESET_RTC => ctx.request_reset(ResetCause::RtcWatchdogSystem),
                // An "off" stage was waited for and does nothing when it ends: the assumption is
                // marked on the walk.
                _ => {}
            },
        )
    }

    /// The super watchdog at the present time; returns the instant it next needs a call.
    fn step_super_watchdog(&mut self, ctx: &mut Ctx, now: Cycles) -> Option<Cycles> {
        if !self.swd_armed() {
            self.swd_fed_at = None;
            return None;
        }
        let fed_at = *self.swd_fed_at.get_or_insert(now);
        let waited = now.get().saturating_sub(fed_at.get());
        if waited >= SWD_TIMEOUT {
            self.swd_conf |= SWD_RESET_FLAG;
            self.swd_fed_at = None;
            ctx.request_reset(ResetCause::SuperWatchdog);
            None
        } else if waited >= SWD_FEED_REQUEST {
            self.int_raw |= INT_SWD;
            Some(fed_at + SWD_TIMEOUT)
        } else {
            Some(fed_at + SWD_FEED_REQUEST)
        }
    }

    /// Drives the `RTC_CORE` source to the level of the enabled RTC interrupts.
    fn drive(&self, ctx: &mut Ctx) {
        ctx.irq(RTC_CORE, self.int_raw & self.int_ena != 0);
    }

    /// The device time of a wake-up has come, or the block was just built or reset.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.evaluate(ctx, false);
    }

    /// A write to a watchdog register other than the key: dropped unless the key is in place.
    fn write_watchdog(&mut self, ctx: &mut Ctx, offset: u32, value: u32) {
        if self.wdt_key != WDT_KEY {
            return;
        }
        match offset {
            WDTCONFIG0 => self.wdt_config0 = value,
            WDTFEED => {
                if value & WDT_FEED != 0 {
                    self.wdt.set(0, ctx.now());
                    self.wdt_stage = 0;
                }
            }
            _ => {
                // INVARIANT: the caller passes one of the four stage timeout registers besides
                // the two above; any other offset changes nothing.
                if let Some(stage) = index_in(offset, WDTCONFIG1, WDT_STAGES) {
                    self.wdt_timeout[stage] = value;
                }
            }
        }
        self.evaluate(ctx, true);
    }

    fn write_super_watchdog(&mut self, ctx: &mut Ctx, value: u32) {
        if self.swd_key != SWD_KEY {
            return;
        }
        self.swd_conf = (self.swd_conf & SWD_RESET_FLAG) | (value & SWD_CONF_STORED);
        if value & SWD_RST_FLAG_CLR != 0 {
            self.swd_conf &= !SWD_RESET_FLAG;
        }
        if value & SWD_FEED != 0 {
            self.swd_fed_at = Some(ctx.now());
            self.int_raw &= !INT_SWD;
        }
        self.evaluate(ctx, false);
    }
}

impl Peripheral<Ctx> for RtcCntl {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            return Some(self.store[i]);
        }
        Some(match offset {
            RESET_STATE => self.reset_state_other | self.reset_cause,
            TIME_LOW0 => half_of(self.time[0], false),
            TIME_HIGH0 => half_of(self.time[0], true),
            TIME_LOW1 => half_of(self.time[1], false),
            TIME_HIGH1 => half_of(self.time[1], true),
            INT_ENA => self.int_ena,
            INT_RAW => self.int_raw,
            INT_ST => self.int_raw & self.int_ena,
            // Write-only registers read 0.
            TIME_UPDATE | INT_CLR | INT_ENA_W1TS | INT_ENA_W1TC | WDTFEED => 0,
            WDTCONFIG0 => self.wdt_config0,
            WDTWPROTECT => self.wdt_key,
            SWD_CONF => {
                // The feed request of the super watchdog reads in `SWD_CONF` while its interrupt
                // is raw.
                let feed_request = if self.int_raw & INT_SWD == 0 {
                    0
                } else {
                    SWD_FEED_INT
                };
                self.swd_conf | feed_request
            }
            SWD_WPROTECT => self.swd_key,
            _ => {
                return index_in(offset, WDTCONFIG1, WDT_STAGES)
                    .map(|stage| self.wdt_timeout[stage])
                    .or_else(|| self.config.read(offset));
            }
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            self.store[i] = value;
            return true;
        }
        match offset {
            OPTIONS0 => {
                // The system reset is the wider of the two: it wins when both bits are written.
                if value & SW_SYS_RST != 0 {
                    ctx.request_reset(ResetCause::SoftwareSystem);
                } else if value & SW_PROCPU_RST != 0 {
                    ctx.request_reset(ResetCause::SoftwareCpu);
                }
                // The two bits read 0; the rest of the register is stored.
                return self
                    .config
                    .write(offset, value & !(SW_SYS_RST | SW_PROCPU_RST));
            }
            TIME_UPDATE => {
                if value & TIME_UPDATE_TRIGGER != 0 {
                    // "Each trigger moves the previous capture into TIME_LOW1/HIGH1" (TW §C.2).
                    self.time = [self.rtc_time(ctx.now()), self.time[0]];
                }
            }
            INT_ENA => self.int_ena = value,
            INT_ENA_W1TS => self.int_ena |= value,
            INT_ENA_W1TC => self.int_ena &= !value,
            INT_CLR => self.int_raw &= !value,
            WDTWPROTECT => self.wdt_key = value,
            WDTCONFIG0 | WDTFEED => {
                self.write_watchdog(ctx, offset, value);
                return true;
            }
            offset if index_in(offset, WDTCONFIG1, WDT_STAGES).is_some() => {
                self.write_watchdog(ctx, offset, value);
                return true;
            }
            SWD_WPROTECT => self.swd_key = value,
            SWD_CONF => {
                self.write_super_watchdog(ctx, value);
                return true;
            }
            CLK_CONF => {
                // The slow clock of this model is the RC oscillator, whatever is selected: a
                // selection of another source is reported, not silently mistimed.
                if value >> ANA_CLK_RTC_SEL_SHIFT != 0 {
                    ctx.unsupported(Unsupported::SlowClock { clk_conf: value });
                }
                return self.config.write(offset, value);
            }
            FIB_SEL => {
                let known = self.config.write(offset, value);
                self.evaluate(ctx, false);
                return known;
            }
            // Read-only: the write is reported as unmodelled.
            RESET_STATE | TIME_LOW0 | TIME_HIGH0 | TIME_LOW1 | TIME_HIGH1 | INT_RAW | INT_ST => {
                return false;
            }
            _ => return self.config.write(offset, value),
        }
        self.drive(ctx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    const MILLISECOND: u64 = SECOND / 1_000;

    fn rtc(ctx: &mut Ctx) -> RtcCntl {
        let mut rtc = RtcCntl::new(true, 0, ctx.now());
        rtc.wake(ctx);
        rtc
    }

    /// Calls the block for every wake-up due at the present time (`periph::pump`).
    fn pump(ctx: &mut Ctx, rtc: &mut RtcCntl) {
        crate::periph::pump(ctx, |ctx, event| {
            assert_eq!(event, Event::RtcWatchdog);
            rtc.wake(ctx);
        });
    }

    fn after(ctx: &mut Ctx, rtc: &mut RtcCntl, cycles: u64) {
        ctx.sleep_until(ctx.now() + cycles);
        pump(ctx, rtc);
    }

    /// What `bootloader_config_wdt` writes (TW §C.3): the flash-boot protection off, one stage, a
    /// system reset after 9 s (612 000 cycles written, 1 224 000 counted).
    fn start_the_bootloader_watchdog(ctx: &mut Ctx, rtc: &mut RtcCntl) {
        for (offset, value) in [
            (WDTWPROTECT, WDT_KEY),
            (WDTCONFIG0, 0x0001_2214),
            (WDTCONFIG0, 0x0007_EA00),
            (WDTCONFIG0, 0x4007_EA00),
            (WDTCONFIG1, 0x0009_56A0),
            (WDTFEED, 0x8000_0000),
            (WDTCONFIG0, 0xC007_EA00),
            (WDTWPROTECT, 0),
        ] {
            assert!(rtc.write(ctx, offset, value), "{offset:#x}");
        }
    }

    #[test]
    fn power_on_reset_cause_is_reported() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        let state = rtc.read(ctx, RESET_STATE).unwrap();
        assert_eq!(state & 0x3F, 1, "RESET_CAUSE_PROCPU = POWERON");
        assert_eq!(state & (1 << 20), 0, "no JTAG reset flag");
        assert_eq!(
            state & !0x3F,
            0x3000,
            "the other bits keep their documented reset value"
        );
        assert!(!rtc.write(ctx, RESET_STATE, 0), "read-only");
    }

    #[test]
    fn store_registers_keep_what_the_rom_writes() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        for (i, offset) in STORE.into_iter().enumerate() {
            let value = 0x1000 + u32::try_from(i).unwrap();
            assert_eq!(rtc.read(ctx, offset), Some(0));
            assert!(rtc.write(ctx, offset, value));
            assert_eq!(rtc.read(ctx, offset), Some(value));
        }
    }

    #[test]
    fn configuration_is_stored_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        for (name, reset) in [
            ("OPTIONS0", 0x1C00_A000),
            ("TIMER1", 0x2814_0403),
            ("ANA_CONF", 0x00C4_0000),
            ("CLK_CONF", 0x1158_3218),
            ("RTC_CNTL", 0xA000_0000),
            ("DIG_PWC", 0x0055_5010),
            ("BROWN_OUT", 0x43FF_0010),
        ] {
            assert_eq!(rtc.read(ctx, reg(name)), Some(reset), "{name}");
            assert!(rtc.write(ctx, reg(name), 0x0000_0140));
            assert_eq!(rtc.read(ctx, reg(name)), Some(0x0000_0140), "{name}");
        }
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(
            rtc.read(ctx, reg("SLP_TIMER0")),
            None,
            "sleep is not modelled"
        );
        assert_eq!(rtc.read(ctx, reg("STATE0")), None);
    }

    #[test]
    fn a_software_reset_bit_asks_for_its_reset_and_the_rest_of_the_write_is_stored() {
        // IR §4.1: `software_reset_cpu` writes `OPTIONS0 | 0x20`; the other bits must survive.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert!(rtc.write(ctx, OPTIONS0, 0x1C00_8000 | SW_PROCPU_RST));
        assert_eq!(ctx.take_reset(), Some(ResetCause::SoftwareCpu));
        assert_eq!(rtc.read(ctx, OPTIONS0), Some(0x1C00_8000), "bit 5 reads 0");
        assert!(rtc.write(ctx, OPTIONS0, 0x1C00_A000 | SW_SYS_RST));
        assert_eq!(ctx.take_reset(), Some(ResetCause::SoftwareSystem));
        assert_eq!(rtc.read(ctx, OPTIONS0), Some(0x1C00_A000));
        rtc.write(ctx, OPTIONS0, SW_SYS_RST | SW_PROCPU_RST);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::SoftwareSystem),
            "the wider one"
        );
        rtc.write(ctx, OPTIONS0, 0x1C00_A000 & !(1 << 13));
        assert_eq!(
            ctx.take_reset(),
            None,
            "BBPLL power bits are plain configuration"
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn the_cause_of_a_reset_is_what_the_next_boot_reads() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, STORE[4], 0x0028_0028);
        rtc.reset(ctx, ResetCause::SoftwareCpu, false);
        assert_eq!(rtc.read(ctx, RESET_STATE), Some(0x300C));
        assert_eq!(
            rtc.read(ctx, STORE[4]),
            Some(0x0028_0028),
            "kept by a CPU reset"
        );
        rtc.reset(ctx, ResetCause::RtcWatchdogSystem, true);
        assert_eq!(rtc.read(ctx, RESET_STATE), Some(0x3010));
        assert_eq!(
            rtc.read(ctx, STORE[4]),
            Some(0),
            "cleared by a system reset"
        );
    }

    #[test]
    fn the_rtc_timer_counts_slow_clock_cycles_and_keeps_two_captures() {
        // TW §C.2: 48 bits, 136 kHz, captured by TIME_UPDATE bit 31.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert_eq!(rtc.read(ctx, TIME_LOW0), Some(0));
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, TIME_LOW0), Some(0), "no capture yet");
        assert!(rtc.write(ctx, TIME_UPDATE, 0x8000_0000));
        assert_eq!(rtc.read(ctx, TIME_UPDATE), Some(0), "write-only");
        assert_eq!(rtc.read(ctx, TIME_LOW0), Some(136_000));
        assert_eq!(rtc.read(ctx, TIME_HIGH0), Some(0));
        // 40 000 s more: past 32 bits of slow-clock cycles.
        after(ctx, &mut rtc, 40_000 * SECOND);
        rtc.write(ctx, TIME_UPDATE, 0x8000_0000);
        let cycles = 40_001u64 * 136_000;
        assert_eq!(
            rtc.read(ctx, TIME_LOW0),
            Some((cycles & 0xFFFF_FFFF) as u32)
        );
        assert_eq!(rtc.read(ctx, TIME_HIGH0), Some((cycles >> 32) as u32));
        assert_eq!(
            rtc.read(ctx, TIME_LOW1),
            Some(136_000),
            "the capture before"
        );
        assert_eq!(rtc.read(ctx, TIME_HIGH1), Some(0));
    }

    #[test]
    fn the_rtc_timer_survives_every_reset() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, WDTWPROTECT, WDT_KEY);
        rtc.write(ctx, WDTCONFIG0, 0);
        after(ctx, &mut rtc, SECOND);
        rtc.reset(ctx, ResetCause::RtcWatchdogSystem, true);
        rtc.write(ctx, TIME_UPDATE, 0x8000_0000);
        assert_eq!(rtc.read(ctx, TIME_LOW0), Some(136_000));
    }

    #[test]
    fn the_flash_boot_protection_resets_the_system_after_400_000_slow_cycles() {
        // TW §D.4: `WDTCONFIG1` = 200 000, doubled by the eFuse rule; 2.941 s at 136 kHz; cause 0x10.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert_eq!(rtc.read(ctx, WDTCONFIG0), Some(0x0001_3214));
        assert_eq!(rtc.read(ctx, WDTCONFIG1), Some(200_000));
        assert_eq!(
            rtc.read(ctx, WDTWPROTECT),
            Some(0x50D8_3AA1),
            "unlocked out of reset"
        );
        assert_eq!(ctx.next_wake(), Some(Cycles(470_588_236)));
        after(ctx, &mut rtc, 470_588_235);
        assert_eq!(ctx.take_reset(), None);
        after(ctx, &mut rtc, 1);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogSystem));
    }

    #[test]
    fn a_core_reset_does_not_restart_the_watchdog() {
        // TW §D.2: the RTC watchdog keeps counting across a core reset of the main watchdog.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        after(ctx, &mut rtc, 2 * SECOND);
        rtc.reset(ctx, ResetCause::Mwdt0Core, false);
        after(ctx, &mut rtc, SECOND);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::RtcWatchdogSystem),
            "2.94 s from power-on"
        );
    }

    #[test]
    fn the_bootloader_watchdog_resets_after_9_s_unless_it_is_fed() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        start_the_bootloader_watchdog(ctx, &mut rtc);
        assert_eq!(rtc.read(ctx, WDTCONFIG0), Some(0xC007_EA00), "TW §C.3");
        after(ctx, &mut rtc, 8 * SECOND);
        // `esp_clk_init`: unlock, feed, stage 0 <- 1.6 s (0x1A900), lock.
        rtc.write(ctx, WDTWPROTECT, WDT_KEY);
        rtc.write(ctx, WDTFEED, 0x8000_0000);
        rtc.write(ctx, WDTCONFIG1, 0x0001_A900);
        rtc.write(ctx, WDTWPROTECT, 0);
        after(ctx, &mut rtc, 1_599 * MILLISECOND);
        assert_eq!(ctx.take_reset(), None);
        after(ctx, &mut rtc, 2 * MILLISECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogSystem));
    }

    #[test]
    fn a_locked_watchdog_ignores_feeds_and_a_feed_needs_bit_31() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        start_the_bootloader_watchdog(ctx, &mut rtc);
        after(ctx, &mut rtc, 5 * SECOND);
        rtc.write(ctx, WDTFEED, 0x8000_0000);
        rtc.write(ctx, WDTCONFIG0, 0);
        assert_eq!(rtc.read(ctx, WDTCONFIG0), Some(0xC007_EA00), "locked");
        rtc.write(ctx, WDTWPROTECT, WDT_KEY);
        rtc.write(ctx, WDTFEED, 1);
        after(ctx, &mut rtc, 4 * SECOND);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::RtcWatchdogSystem),
            "never fed"
        );
    }

    #[test]
    fn the_stages_esp_restart_programs_reset_the_core_then_the_system() {
        // IR §4.1: stage 0 action 3 after 68 000 << 1 cycles (1 s), stage 1 action 4 after 136 000.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        for (offset, value) in [
            (
                WDTCONFIG0,
                (ACTION_RESET_SYSTEM << 28) | (ACTION_RESET_RTC << 25),
            ),
            (WDTCONFIG1, 0x0001_09A0),
            (WDTCONFIG1 + 4, 0x0002_1340),
            (WDTFEED, 0x8000_0000),
            (
                WDTCONFIG0,
                WDT_EN | (ACTION_RESET_SYSTEM << 28) | (ACTION_RESET_RTC << 25),
            ),
        ] {
            rtc.write(ctx, offset, value);
        }
        after(ctx, &mut rtc, SECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogCore));
        after(ctx, &mut rtc, SECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogSystem));
    }

    #[test]
    fn an_interrupt_stage_raises_the_rtc_core_source_until_it_is_cleared() {
        // Source RTC_CORE is 27; the watchdog is bit 3 of the RTC interrupt registers.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        rtc.write(ctx, INT_ENA_W1TS, INT_WDT);
        rtc.write(ctx, WDTCONFIG1, 68_000);
        rtc.write(
            ctx,
            WDTCONFIG0,
            WDT_EN | (ACTION_INTERRUPT << 28) | (ACTION_RESET_CPU << 25),
        );
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, INT_ST), Some(INT_WDT));
        assert_eq!(ctx.sources(), 1 << 27);
        rtc.write(ctx, INT_CLR, INT_WDT);
        assert_eq!(ctx.sources(), 0);
        rtc.write(ctx, INT_ENA_W1TC, INT_WDT);
        assert_eq!(rtc.read(ctx, INT_ENA), Some(0));
        // Stage 1: 80 000 cycles, a CPU reset.
        after(ctx, &mut rtc, SECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogCpu));
    }

    #[test]
    fn the_super_watchdog_never_fires_before_software_takes_it_over_nor_once_it_is_auto_fed() {
        // TW §C.4: the ROM phase shows no `rst:0x12`; the bootloader sets AUTO_FEED_EN.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        assert_eq!(rtc.read(ctx, SWD_CONF), Some(0x04B0_0000));
        assert_eq!(rtc.read(ctx, SWD_WPROTECT), Some(0x8F1D_312A));
        after(ctx, &mut rtc, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None);
        // The bootloader's order: `bootloader_ana_super_wdt_reset_config(true)` hands the reset to
        // software (`bootloader_esp32c3.c:99`: `BYPASS_RST` clear, `FIB_SEL` bit 2 clear), and
        // `bootloader_super_wdt_auto_feed` sets `AUTO_FEED_EN` later (`:133`; TW §C.4).
        rtc.write(ctx, FIB_SEL, 0b011);
        assert_eq!(
            ctx.next_wake(),
            Some(ctx.now() + SWD_FEED_REQUEST),
            "armed from the first write"
        );
        // The window between the two: the super watchdog counts, and has not asked yet.
        after(ctx, &mut rtc, SECOND / 2);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(0), "no feed request yet");
        assert_eq!(ctx.take_reset(), None);
        rtc.write(ctx, SWD_WPROTECT, SWD_KEY);
        rtc.write(ctx, SWD_CONF, 0x04B0_0000 | SWD_AUTO_FEED_EN);
        rtc.write(ctx, SWD_WPROTECT, 0);
        assert_eq!(ctx.next_wake(), None, "auto-fed: nothing left to wait for");
        after(ctx, &mut rtc, 10 * SECOND);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(0), "no feed request after");
        assert_eq!(ctx.take_reset(), None);
        rtc.write(ctx, SWD_CONF, 0x04B0_0000);
        assert_eq!(
            rtc.read(ctx, SWD_CONF),
            Some(0x04B0_0000 | SWD_AUTO_FEED_EN),
            "locked: the write is dropped"
        );
    }

    #[test]
    fn an_unfed_super_watchdog_asks_for_a_feed_then_resets_the_system() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        rtc.write(ctx, INT_ENA, INT_SWD);
        rtc.write(ctx, FIB_SEL, 0b011);
        after(ctx, &mut rtc, 900 * MILLISECOND);
        assert_eq!(
            rtc.read(ctx, SWD_CONF).map(|v| v & SWD_FEED_INT),
            Some(SWD_FEED_INT)
        );
        assert_eq!(ctx.sources(), 1 << 27);
        rtc.write(ctx, SWD_CONF, 0x04B0_0000 | SWD_FEED);
        assert_eq!(
            rtc.read(ctx, SWD_CONF),
            Some(0x04B0_0000),
            "FEED is write-only"
        );
        assert_eq!(ctx.sources(), 0);
        after(ctx, &mut rtc, 999 * MILLISECOND);
        assert_eq!(ctx.take_reset(), None, "fed 999 ms ago");
        after(ctx, &mut rtc, MILLISECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::SuperWatchdog));
        // The next boot reads the flag, and clears it (`clear_super_wdt_reset_flag`).
        rtc.reset(ctx, ResetCause::SuperWatchdog, true);
        assert_eq!(rtc.read(ctx, RESET_STATE), Some(0x3012));
        assert_eq!(rtc.read(ctx, SWD_CONF), Some(0x04B0_0000 | SWD_RESET_FLAG));
        rtc.write(ctx, SWD_CONF, 0x04B0_0000 | SWD_RST_FLAG_CLR);
        assert_eq!(rtc.read(ctx, SWD_CONF), Some(0x04B0_0000));
    }

    /// Writes the four stage timeouts as `WDTCONFIG1..4` take them (stage 0 is halved by the
    /// eFuse rule when it is counted), then `WDTCONFIG0`.
    fn program_watchdog(ctx: &mut Ctx, rtc: &mut RtcCntl, timeouts: [u32; 4], config0: u32) {
        for (i, timeout) in timeouts.into_iter().enumerate() {
            assert!(rtc.write(ctx, WDTCONFIG1 + 4 * u32::try_from(i).unwrap(), timeout));
        }
        assert!(rtc.write(ctx, WDTCONFIG0, config0));
    }

    /// Cycles of the time base one stage-timeout of `counts` slow-clock cycles lasts, as the model
    /// counts them: 20 000 / 17 cycles per count, rounded up.
    fn cycles_of(counts: u64) -> u64 {
        (counts * 20_000).div_ceil(17)
    }

    #[test]
    fn stage_timeouts_of_zero_last_one_count_and_the_stage_walk_terminates() {
        // ASSUMPTION (rule 1): a stage lasts at least one count. All four timeouts at 0, enabled.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        program_watchdog(ctx, &mut rtc, [0; 4], WDT_EN);
        assert_eq!(rtc.wdt_stage, 0);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(cycles_of(1))),
            "1 count: 1177 cycles"
        );
        after(ctx, &mut rtc, cycles_of(1));
        assert_eq!(
            rtc.wdt_stage, 1,
            "one stage per count, the fraction of a count is lost"
        );
        assert_eq!(ctx.next_wake(), Some(Cycles(2 * cycles_of(1))));
        // A very late call walks the four stages at most twice, whatever the registers hold.
        after(ctx, &mut rtc, 3_600 * SECOND);
        assert!(rtc.wdt_stage < WDT_STAGES);
        assert!(ctx.next_wake().is_some());
    }

    #[test]
    fn lowering_the_timeouts_after_a_long_count_expires_one_stage_at_the_write() {
        // Stage 0 counts up to 0xFFFF_FFFF << 1 = 8 589 934 590 counts (17.5 h at 136 kHz); one
        // hour of device time is 3 600 * 136 000 = 489 600 000 counts. The write of stage 0's
        // timeout (1 << 1 = 2 counts) leaves the count past it: stage 0 expires at that write and
        // stage 1 counts from 0 (TW §B.3). The other writes find the count at 0, below 1: nothing
        // more happens. Stage 1 is current, its wake-up one count after the writes.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        program_watchdog(ctx, &mut rtc, [0xFFFF_FFFF, 80_000, 0xFFF, 0xFFF], WDT_EN);
        after(ctx, &mut rtc, 3_600 * SECOND);
        for (i, offset) in [WDTCONFIG1, WDTCONFIG1 + 4, WDTCONFIG1 + 8, WDTCONFIG4]
            .into_iter()
            .enumerate()
        {
            assert!(rtc.write(ctx, offset, 1), "write {i}");
        }
        assert_eq!(rtc.wdt_stage, 1);
        assert_eq!(ctx.next_wake(), Some(ctx.now() + cycles_of(1)));
    }

    #[test]
    fn a_long_sleep_over_tiny_stages_is_walked_in_bounded_steps() {
        // Stage 0 lasts 1 << 1 = 2 counts, stages 1 to 3 one count each: a turn is 5 counts.
        // The wake-up due after 2 counts is delivered an hour late, plus 2 000 cycles (1.7
        // counts): the count is 489 600 000 + 1 = 489 600 001. Four expiries take 2 + 1 + 1 + 1,
        // leaving 489 599 996, which is 1 modulo 5. Whole turns change nothing, so the walk goes on
        // from stage 0 with a count of 1: the interrupt of stage 0 is raised, and the next
        // wake-up is one count (1 177 cycles) away.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        program_watchdog(ctx, &mut rtc, [1; 4], WDT_EN | (ACTION_INTERRUPT << 28));
        after(ctx, &mut rtc, 3_600 * SECOND + 2_000);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(INT_WDT));
        assert_eq!(rtc.wdt_stage, 0);
        assert_eq!(ctx.next_wake(), Some(ctx.now() + cycles_of(1)));
    }

    #[test]
    fn a_timeout_lowered_below_the_count_fires_at_the_write_and_the_next_stage_starts_from_0() {
        // Stage 0: interrupt after 136 000 counts (1 s), stage 1: reset the CPU after 80 000.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        program_watchdog(
            ctx,
            &mut rtc,
            [68_000, 80_000, 0xFFF, 0xFFF],
            WDT_EN | (ACTION_INTERRUPT << 28) | (ACTION_RESET_CPU << 25),
        );
        // Half a second: 68 000 counts. Stage 0 is lowered to 34 000 << 1 = 68 000 counts: at or
        // past the timeout, so it expires at the write (decision E27).
        after(ctx, &mut rtc, SECOND / 2);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(0));
        rtc.write(ctx, WDTCONFIG1, 34_000);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(INT_WDT));
        assert_eq!(rtc.wdt_stage, 1);
        assert_eq!(ctx.take_reset(), None);
        let started = ctx.now();
        assert_eq!(ctx.next_wake(), Some(started + cycles_of(80_000)), "from 0");
        // 8 500 counts into stage 1, its timeout goes from 80 000 to 40 000: still above the
        // count, nothing fires, and the wake-up moves up to 40 000 counts from the start.
        after(ctx, &mut rtc, 10_000_000);
        rtc.write(ctx, WDTCONFIG1 + 4, 40_000);
        assert_eq!(ctx.take_reset(), None);
        assert_eq!(rtc.wdt_stage, 1);
        assert_eq!(ctx.next_wake(), Some(started + cycles_of(40_000)));
    }

    #[test]
    fn the_stage_walk_goes_0_1_2_3_0_and_an_off_stage_is_waited_for_and_does_nothing() {
        // Stage 0 interrupt, stage 1 off, stage 2 reset the CPU, stage 3 reset the RTC; each lasts
        // 136 000 counts (1 s: stage 0 is written halved).
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        let config0 =
            WDT_EN | (ACTION_INTERRUPT << 28) | (ACTION_RESET_CPU << 22) | (ACTION_RESET_RTC << 19);
        program_watchdog(ctx, &mut rtc, [68_000, 136_000, 136_000, 136_000], config0);
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(INT_WDT));
        assert_eq!((rtc.wdt_stage, ctx.take_reset()), (1, None));
        rtc.write(ctx, INT_CLR, INT_WDT);
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(0), "the off stage did nothing");
        assert_eq!((rtc.wdt_stage, ctx.take_reset()), (2, None));
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.wdt_stage, 3);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogCpu));
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.wdt_stage, 0, "and round again");
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogSystem));
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(INT_WDT));
        assert_eq!(rtc.wdt_stage, 1);
    }

    #[test]
    fn an_expiry_delivered_late_carries_the_counts_since_the_deadline_into_the_next_stage() {
        // Stage 0 lasts 136 000 counts (160 000 000 cycles), stage 1 80 000. The wake-up is
        // delivered 5 883 cycles late: 5 counts (5 883 * 17 / 20 000 = 5.0006). Stage 1 has 5
        // counts behind it when it starts, and its wake-up is that much earlier than 80 000 counts
        // (94 117 648 cycles) away: 79 995 counts, 94 111 765 cycles.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        program_watchdog(ctx, &mut rtc, [68_000, 80_000, 0xFFF, 0xFFF], WDT_EN);
        assert_eq!(ctx.next_wake(), Some(Cycles(SECOND)));
        after(ctx, &mut rtc, SECOND + 5_883);
        assert_eq!(rtc.wdt_stage, 1);
        assert_eq!(cycles_of(80_000), 94_117_648);
        assert_eq!(cycles_of(79_995), 94_111_765);
        assert_eq!(ctx.next_wake(), Some(Cycles(SECOND + 5_883 + 94_111_765)));
    }

    #[test]
    fn stage_0_is_stretched_by_the_efuse_delay_and_the_delay_is_masked_to_its_two_bits() {
        // The flash-boot protection counts `WDTCONFIG1` (200 000) << (1 + delay) slow-clock
        // cycles: delay 1 is 800 000 counts, 800 000 * 20 000 / 17 = 941 176 470.6 -> 941 176 471
        // cycles; delay 3 is 3 200 000 counts -> 3 764 705 882.4 -> 3 764 705 883 cycles. A
        // delay with higher bits set is read through its two low bits.
        for (delay, cycles) in [
            (1, 941_176_471),
            (3, 3_764_705_883),
            (u32::MAX, 3_764_705_883),
        ] {
            let ctx = &mut Ctx::default();
            let mut rtc = RtcCntl::new(true, delay, ctx.now());
            rtc.wake(ctx);
            assert_eq!(ctx.next_wake(), Some(Cycles(cycles)), "delay {delay:#x}");
        }
    }

    #[test]
    fn a_stopped_watchdog_leaves_no_wake_up_behind() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        assert_eq!(ctx.next_wake(), None, "nothing runs out of reset here");
        rtc.write(ctx, WDTCONFIG0, WDT_EN);
        assert!(ctx.next_wake().is_some());
        rtc.write(ctx, WDTCONFIG0, 0);
        assert_eq!(ctx.next_wake(), None);
    }

    #[test]
    fn a_firmware_that_feeds_or_toggles_the_watchdog_forever_does_not_grow_the_scheduler() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        program_watchdog(ctx, &mut rtc, [0xFFFF_FFFF, 80_000, 0xFFF, 0xFFF], WDT_EN);
        // Each write cancels a wake-up hours ahead and schedules another.
        // One live wake-up, and the scheduler tolerates its slack of 32 cancelled ones.
        let bound = 40;
        for _ in 0..10_000 {
            rtc.write(ctx, WDTFEED, WDT_FEED);
            assert!(
                ctx.scheduled_entries() <= bound,
                "{}",
                ctx.scheduled_entries()
            );
        }
        for round in 0..10_000u64 {
            ctx.sleep_until(ctx.now() + 1 + round % 3);
            rtc.write(ctx, WDTFEED, WDT_FEED);
            assert!(
                ctx.scheduled_entries() <= bound,
                "{}",
                ctx.scheduled_entries()
            );
        }
        for _ in 0..10_000 {
            rtc.write(ctx, WDTCONFIG0, 0);
            rtc.write(ctx, WDTCONFIG0, WDT_EN);
            assert!(
                ctx.scheduled_entries() <= bound,
                "{}",
                ctx.scheduled_entries()
            );
        }
        assert!(ctx.next_wake().is_some(), "and the watchdog still runs");
    }

    #[test]
    fn a_slow_clock_other_than_the_rc_oscillator_is_refused_by_name() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert!(rtc.write(ctx, CLK_CONF, 0x1158_3218));
        assert_eq!(ctx.take_unsupported(), None);
        assert!(rtc.write(ctx, CLK_CONF, 0x5158_3218));
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::SlowClock {
                clk_conf: 0x5158_3218
            })
        );
        assert_eq!(
            rtc.read(ctx, CLK_CONF),
            Some(0x5158_3218),
            "stored all the same"
        );
    }
}
