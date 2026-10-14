//! The sleep of the chip, as `RTC_CNTL` controls it: the alarm of the RTC timer (`SLP_TIMER0/1`),
//! the request (`STATE0.SLEEP_EN`), the wake sources (`WAKEUP_STATE`), the power-down of the
//! digital domain (`DIG_PWC`) and the cause of the last wake (`SLP_WAKEUP_CAUSE`). Sources: TRM
//! §9.4 to §9.7, ESP-IDF v5.5.2 `rtc_cntl_reg.h`, `rtc_sleep.c`, `rtc_cntl_ll.h`, `sleep_modes.c`;
//! report DS §A.1, §A.3, §A.4.
//!
//! Deep sleep (F1): at the write that sets `SLEEP_EN` with `DIG_PWC.DG_WRAP_PD_EN` set, the chip
//! sleeps; the machine stops the CPU and moves device time to the next call of this block. When
//! the RTC timer reaches the target of its alarm and the timer is a wake source, the block latches
//! the cause and asks for the core reset of cause 5 the chip wakes with (F2). With `DG_WRAP_PD_EN`
//! clear the request is a light sleep, refused and rejected (F4). `DIG_PWC` itself is an accepted
//! register of `RTC_CNTL` (its other power-domain switches have no effect without a power model):
//! this module reads `DG_WRAP_PD_EN` from it at the write of `SLEEP_EN`.
use super::{INT_SLP_REJECT, INT_SLP_WAKEUP, INT_TIMER_ALARM, RtcCntl, reg};
use crate::ctx::Ctx;
use crate::diag::Unsupported;
use crate::periph::RTC_CNTL_BASE;
use crate::reset::ResetCause;
use emu_core::Cycles;

const SLP_TIMER0: u32 = reg("SLP_TIMER0");
const SLP_TIMER1: u32 = reg("SLP_TIMER1");
const STATE0: u32 = reg("STATE0");
const WAKEUP_STATE: u32 = reg("WAKEUP_STATE");
const SLP_REJECT_CAUSE: u32 = reg("SLP_REJECT_CAUSE");
const SLP_WAKEUP_CAUSE: u32 = reg("SLP_WAKEUP_CAUSE");
const DIG_PWC: u32 = reg("DIG_PWC");

/// `SLP_TIMER1`: bits [15:0] `SLP_VAL_HI`, the high 16 bits of the 48-bit target, and bit 16
/// `MAIN_TIMER_ALARM_EN`, write-only (`rtc_cntl_reg.h:166-178`). `rtc_cntl_ll_set_wakeup_timer`
/// writes the target, clears the interrupt, then sets the enable (`rtc_cntl_ll.h:18-25`).
const SLP_VAL_HI: u32 = 0xFFFF;
const MAIN_TIMER_ALARM_EN: u32 = 1 << 16;
/// `STATE0` (`rtc_cntl_reg.h:222-261`): bit 31 `SLEEP_EN`, bits 30 `SLP_REJECT` and 29
/// `SLP_WAKEUP` (R/W), bit 28 `SDIO_ACTIVE_IND` (read-only, 0 here: no SDIO), bit 22
/// `APB2RTC_BRIDGE_SEL` (R/W, stored), bit 1 `SLP_REJECT_CAUSE_CLR` and bit 0 `SW_CPU_INT`
/// (write-only).
const SLEEP_EN: u32 = 1 << 31;
const SLP_REJECT: u32 = 1 << 30;
const SLP_WAKEUP: u32 = 1 << 29;
const SW_CPU_INT: u32 = 1 << 0;
const STATE0_STORED: u32 = SLEEP_EN | SLP_REJECT | SLP_WAKEUP | 1 << 22;
/// The bits of `STATE0` that would raise something the model does not have (P9 of the plan
/// names the choice; the message, the remedy).
const STATE0_SIGNALS: u32 = SLP_REJECT | SLP_WAKEUP | SW_CPU_INT;
/// `WAKEUP_STATE` bits [31:15] `WAKEUP_ENA` (`rtc_cntl_reg.h:554-560`); the rest reads 0.
const WAKEUP_ENA_SHIFT: u32 = 15;
const WAKEUP_STATE_STORED: u32 = 0xFFFF_8000;
/// Bit 3 of `WAKEUP_ENA` and of `SLP_WAKEUP_CAUSE`, the RTC timer (`RTC_TIMER_TRIG_EN`,
/// `IDF:esp_hw_support/port/esp32c3/include/soc/rtc.h:665`): the one wake source the model raises.
const TIMER_WAKE: u32 = 1 << 3;
/// `DIG_PWC` bit 31, `DG_WRAP_PD_EN` (`rtc_cntl_reg.h:1330`): `rtc_sleep_init` sets it for a deep
/// sleep and clears it for a light one (`rtc_sleep.c:211-224`; DS §A.4), and the TRM makes it the
/// switch of the digital regulator in sleep (§9.3).
const DG_WRAP_PD_EN: u32 = 1 << 31;

/// Where the chip is in its sleep, as the machine and a front end read it (F9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeepSleepState {
    /// Device time of the write of `SLEEP_EN`.
    pub since: Cycles,
    /// Device time at which the RTC timer wakes the chip, when the timer is a wake source and its
    /// alarm is enabled.
    pub timer_wake: Option<Cycles>,
    /// `WAKEUP_STATE.WAKEUP_ENA`: bit 3 the RTC timer, bit 2 GPIO (`rtc.h:664-672`).
    pub wake_sources: u32,
}

/// The sleep registers of the block, and the sleep itself.
#[derive(Clone, Copy, Debug)]
pub(super) struct Sleep {
    /// `SLP_TIMER0` and `SLP_VAL_HI`: the 48-bit target of the alarm.
    target: u64,
    /// `MAIN_TIMER_ALARM_EN` as the last write of `SLP_TIMER1` left it.
    ///
    /// ASSUMPTION: the enable, a write-only bit, holds the value of the last write of
    /// `SLP_TIMER1` until the next one, and an alarm that fired stays enabled. ESP-IDF writes
    /// `SLP_TIMER1` twice before every timed sleep, the second time with the enable
    /// (`rtc_cntl_ll.h:18-25`), so it never relies on either. A second deep sleep entered on a
    /// chip without writing `SLP_TIMER1` again, with the timer as a wake source, settles it.
    alarm: bool,
    /// Whether the alarm raised `MAIN_TIMER_INT_RAW` since its target or enable was written.
    alarm_raised: bool,
    /// `STATE0` as it reads (its stored bits).
    state0: u32,
    wakeup_state: u32,
    /// `SLP_WAKEUP_CAUSE`: the source of the last wake, as `WAKEUP_ENA` numbers it.
    wakeup_cause: u32,
    /// Device time of the entry into deep sleep, while the chip sleeps.
    asleep: Option<Cycles>,
}

impl Sleep {
    /// The registers at their reset values (`rtc_cntl_reg.h`; the SVD for `WAKEUP_STATE` and
    /// `DIG_PWC`), the chip awake.
    pub(super) fn new() -> Sleep {
        let documented = |offset: u32| {
            svd_regs::esp32c3::lookup(RTC_CNTL_BASE + offset).map_or(0, |r| r.reset_value)
        };
        Sleep {
            target: 0,
            alarm: false,
            alarm_raised: false,
            state0: 0,
            wakeup_state: documented(WAKEUP_STATE) & WAKEUP_STATE_STORED,
            wakeup_cause: 0,
            asleep: None,
        }
    }

    /// `WAKEUP_ENA`.
    const fn wake_sources(&self) -> u32 {
        self.wakeup_state >> WAKEUP_ENA_SHIFT
    }

    /// Whether the chip is in deep sleep.
    pub(super) const fn is_asleep(&self) -> bool {
        self.asleep.is_some()
    }

    /// Any reset ends the sleep: the chip is awake when the reset is over, and `SLEEP_EN` reads 0
    /// whatever ended the sleep (the alarm, or the RTC watchdog left running in it). A
    /// read-modify-write of `STATE0` after the wake, which `rtc_deep_sleep_start` does with
    /// `SLP_REJECT_CAUSE_CLR` before it computes the CRC (DS §A.1.5), would otherwise write the bit
    /// back and put the chip to sleep where the firmware never asked for it.
    pub(super) fn wake_up(&mut self) {
        self.asleep = None;
        self.state0 &= !SLEEP_EN;
    }
}

impl RtcCntl {
    /// The value of a sleep register, `None` for any other offset.
    pub(super) fn read_sleep(&self, offset: u32) -> Option<u32> {
        let sleep = &self.sleep;
        Some(match offset {
            // INVARIANT: the target holds 48 bits, so its low word and bits 32 to 47 fit in a u32.
            SLP_TIMER0 => u32::try_from(sleep.target & 0xFFFF_FFFF).unwrap_or(0),
            // `MAIN_TIMER_ALARM_EN` is write-only and reads 0.
            SLP_TIMER1 => u32::try_from(sleep.target >> 32).unwrap_or(0) & SLP_VAL_HI,
            STATE0 => sleep.state0,
            WAKEUP_STATE => sleep.wakeup_state,
            SLP_WAKEUP_CAUSE => sleep.wakeup_cause,
            // No reject source is modelled: the light sleep the model refuses is rejected without
            // one, and the cause reads 0.
            SLP_REJECT_CAUSE => 0,
            _ => return None,
        })
    }

    /// A write of a sleep register: `Some` whether the model knows the write, `None` for any
    /// other offset.
    pub(super) fn write_sleep(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> Option<bool> {
        match offset {
            SLP_TIMER0 => {
                self.sleep.target = (self.sleep.target & !0xFFFF_FFFF) | u64::from(value);
                self.sleep.alarm_raised = false;
            }
            SLP_TIMER1 => {
                self.sleep.target =
                    (self.sleep.target & 0xFFFF_FFFF) | u64::from(value & SLP_VAL_HI) << 32;
                self.sleep.alarm = value & MAIN_TIMER_ALARM_EN != 0;
                self.sleep.alarm_raised = false;
            }
            STATE0 => self.write_state0(ctx, value),
            WAKEUP_STATE => self.sleep.wakeup_state = value & WAKEUP_STATE_STORED,
            // Read-only: the write is reported as unmodelled.
            SLP_WAKEUP_CAUSE | SLP_REJECT_CAUSE => return Some(false),
            _ => return None,
        }
        // The alarm, the sleep or the rejection may have changed what is due.
        self.evaluate(ctx, false);
        Some(true)
    }

    fn write_state0(&mut self, ctx: &mut Ctx, value: u32) {
        if value & STATE0_SIGNALS != 0 {
            ctx.unsupported(Unsupported::SleepSignal { state0: value });
        }
        // `SLP_REJECT_CAUSE_CLR` (bit 1) clears a cause that always reads 0 here.
        let requested = value & SLEEP_EN != 0 && !self.sleep.is_asleep();
        self.sleep.state0 = value & STATE0_STORED;
        if requested {
            self.enter_sleep(ctx);
        }
    }

    /// `SLEEP_EN` was set: a deep sleep begins, or a light one is refused and rejected.
    fn enter_sleep(&mut self, ctx: &mut Ctx) {
        // INVARIANT: `DIG_PWC` is in `ACCEPTED`, which `config` is built from, so the read always
        // answers and the fallback is never taken.
        let dig_pwc = self.config.read(DIG_PWC).unwrap_or(0);
        if dig_pwc & DG_WRAP_PD_EN == 0 {
            // F4. ESP-IDF waits for `SLP_REJECT_INT_RAW` or `SLP_WAKEUP_INT_RAW` after `SLEEP_EN`
            // and returns `ESP_ERR_SLEEP_REJECT` on the first (`rtc_sleep.c:257-276`).
            //
            // ASSUMPTION: `SLEEP_EN` reads 0 once the request is over, rejected or woken from. No
            // code of ESP-IDF reads it back; a chip whose light sleep is rejected, read after the
            // rejection, settles it.
            ctx.unsupported(Unsupported::LightSleep { dig_pwc });
            self.int_raw |= INT_SLP_REJECT;
            self.sleep.state0 &= !SLEEP_EN;
            return;
        }
        let sources = self.sleep.wake_sources();
        if sources & !TIMER_WAKE != 0 {
            ctx.unsupported(Unsupported::SleepWakeSource {
                wakeup_ena: sources,
            });
        }
        // ASSUMPTION: a byte a UART is still sending when the chip goes to sleep leaves at the
        // wake, when the reset settles the UART (until then the UART goes no further than the
        // entry, whatever a front end calls: `Uart::power_down`), where the chip powers the
        // UART down and loses it. ESP-IDF waits for every UART to be idle before it sleeps (`flush_uarts`,
        // `sleep_modes.c:611-618`; DS §A.1.2), so only a firmware that does not would see the
        // difference: its last bytes before the sleep, received by a host on a real board, settle
        // it.
        self.sleep.asleep = Some(ctx.now());
        ctx.enter_deep_sleep();
    }

    /// The alarm of the RTC timer at the present time: raises what it has reached, wakes the chip
    /// it ends the sleep of, and returns the instant it waits for.
    pub(super) fn step_alarm(&mut self, ctx: &mut Ctx, now: Cycles) -> Option<Cycles> {
        if !self.sleep.alarm {
            return None;
        }
        if self.counts_until(self.sleep.target, now) != 0 {
            return Some(self.instant_of(self.sleep.target, now));
        }
        // `MAIN_TIMER_INT_RAW` (bit 10, `rtc_cntl_reg.h:643`), once per target.
        if !self.sleep.alarm_raised {
            self.sleep.alarm_raised = true;
            self.int_raw |= INT_TIMER_ALARM;
        }
        if self.sleep.is_asleep() && self.sleep.wake_sources() & TIMER_WAKE != 0 {
            // F2. ESP-IDF reads the cause after a reset of cause 5 (`sleep_modes.c:2275-2290`);
            // the wake sets `SLP_WAKEUP_INT_RAW` (DS §A.3.1, reasoned: not read on this path).
            self.sleep.wakeup_cause = TIMER_WAKE;
            self.int_raw |= INT_SLP_WAKEUP;
            self.sleep.state0 &= !SLEEP_EN;
            ctx.request_reset(ResetCause::DeepSleep);
        }
        None
    }

    /// The sleep, for the machine and a front end: `None` while the chip is awake.
    pub(crate) fn deep_sleep(&self, now: Cycles) -> Option<DeepSleepState> {
        let since = self.sleep.asleep?;
        let wake_sources = self.sleep.wake_sources();
        let timer_wake = (self.sleep.alarm && wake_sources & TIMER_WAKE != 0)
            .then(|| self.instant_of(self.sleep.target, now));
        Some(DeepSleepState {
            since,
            timer_wake,
            wake_sources,
        })
    }

    /// The instant of the next call the block asked for, if any: the machine moves a sleeping
    /// chip to it, since nothing else on the chip has power (F1).
    pub(crate) fn next_call(&self) -> Option<Cycles> {
        self.wake.due()
    }

    pub(crate) const fn is_in_deep_sleep(&self) -> bool {
        self.sleep.is_asleep()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{INT_CLR, INT_ENA, INT_RAW, RESET_STATE, STORE, TIME_LOW0, TIME_UPDATE};
    use super::super::{WDT_EN, WDTCONFIG0, WDTCONFIG1};
    use super::*;
    use crate::clock::SECOND;
    use crate::periph::rtc_cntl::timer::TIME_UPDATE_TRIGGERS;
    use emu_core::Peripheral;

    /// What `rtc_sleep_init` leaves in `DIG_PWC` for a deep sleep: the reset value with
    /// `DG_WRAP_PD_EN`, `WIFI_PD_EN` and `BT_PD_EN` set (DS §A.1.3).
    const DEEP: u32 = 0x0055_5010 | DG_WRAP_PD_EN | 1 << 30 | 1 << 27;
    /// The same for a light sleep: `DG_WRAP_PD_EN` clear.
    const LIGHT: u32 = DEEP & !DG_WRAP_PD_EN;
    const TIMER_ONLY: u32 = TIMER_WAKE << WAKEUP_ENA_SHIFT;
    /// Slow-clock counts in one second (136 kHz).
    const COUNTS_PER_SECOND: u64 = 136_000;

    fn rtc(ctx: &mut Ctx) -> RtcCntl {
        // Not strapped for flash: the flash-boot protection of the watchdog stays out of the way.
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        rtc
    }

    fn after(ctx: &mut Ctx, rtc: &mut RtcCntl, cycles: u64) {
        ctx.sleep_until(ctx.now() + cycles);
        crate::periph::pump(ctx, |ctx, _| rtc.wake(ctx));
    }

    /// What ESP-IDF writes for a deep sleep woken by the timer `counts` slow-clock counts from
    /// now (`timer_wakeup_prepare`, `rtc_deep_sleep_start`; DS §A.1.4, §A.1.5), up to `SLEEP_EN`.
    fn sleep_on_the_timer(ctx: &mut Ctx, rtc: &mut RtcCntl, counts: u64, dig_pwc: u32) {
        let target = rtc.rtc_time(ctx.now()) + counts;
        for (offset, value) in [
            (SLP_TIMER0, u32::try_from(target & 0xFFFF_FFFF).unwrap()),
            (SLP_TIMER1, u32::try_from(target >> 32).unwrap()),
            (
                SLP_TIMER1,
                u32::try_from(target >> 32).unwrap() | MAIN_TIMER_ALARM_EN,
            ),
            (DIG_PWC, dig_pwc),
            (WAKEUP_STATE, TIMER_ONLY),
            (STATE0, SLEEP_EN),
        ] {
            assert!(rtc.write(ctx, offset, value), "{offset:#x}");
        }
    }

    #[test]
    fn the_rtc_timer_alarm_wakes_the_chip_at_its_target_as_a_core_reset_of_cause_5() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        after(ctx, &mut rtc, SECOND);
        sleep_on_the_timer(ctx, &mut rtc, 2 * COUNTS_PER_SECOND, DEEP);
        assert!(ctx.take_deep_sleep_entered(), "the machine is told");
        let entered = ctx.now();
        let state = rtc.deep_sleep(ctx.now()).unwrap();
        assert_eq!(state.since, entered);
        // 272 000 counts at 17 every 20 000 cycles: exactly 2 s.
        assert_eq!(state.timer_wake, Some(entered + 2 * SECOND));
        assert_eq!(state.wake_sources, TIMER_WAKE);
        assert_eq!(rtc.next_call(), Some(entered + 2 * SECOND));
        after(ctx, &mut rtc, 2 * SECOND - 1);
        assert_eq!(ctx.take_reset(), None, "one cycle early");
        after(ctx, &mut rtc, 1);
        assert_eq!(ctx.take_reset(), Some(ResetCause::DeepSleep));
        assert_eq!(rtc.read(ctx, SLP_WAKEUP_CAUSE), Some(TIMER_WAKE));
        assert_eq!(
            rtc.read(ctx, INT_RAW),
            Some(INT_SLP_WAKEUP | INT_TIMER_ALARM)
        );
        assert_eq!(rtc.read(ctx, STATE0), Some(0), "SLEEP_EN is over");
        rtc.reset(ctx, ResetCause::DeepSleep, false);
        assert!(!rtc.is_in_deep_sleep());
        assert_eq!(rtc.read(ctx, RESET_STATE).map(|v| v & 0x3F), Some(5));
        assert_eq!(rtc.read(ctx, SLP_WAKEUP_CAUSE), Some(TIMER_WAKE), "kept");
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_deep_sleep_wake_keeps_the_rtc_domain() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, STORE[1], 0x003A_D2CD);
        rtc.write(ctx, STORE[7], 0x1234_5678);
        sleep_on_the_timer(ctx, &mut rtc, COUNTS_PER_SECOND, DEEP);
        after(ctx, &mut rtc, SECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::DeepSleep));
        rtc.reset(ctx, ResetCause::DeepSleep, false);
        assert_eq!(rtc.read(ctx, STORE[1]), Some(0x003A_D2CD));
        assert_eq!(rtc.read(ctx, STORE[7]), Some(0x1234_5678));
        assert_eq!(rtc.read(ctx, DIG_PWC), Some(DEEP));
        assert_eq!(rtc.read(ctx, WAKEUP_STATE), Some(TIMER_ONLY));
        rtc.write(ctx, TIME_UPDATE, 1 << 31);
        assert_eq!(
            rtc.read(ctx, TIME_LOW0),
            Some(136_000),
            "the timer counted through the sleep"
        );
    }

    #[test]
    fn an_alarm_already_reached_wakes_at_the_write_of_sleep_en() {
        // Spec §5: a target already passed at the entry wakes the chip at once.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        after(ctx, &mut rtc, SECOND);
        let behind = rtc.rtc_time(ctx.now()) - 10;
        rtc.write(ctx, SLP_TIMER0, u32::try_from(behind).unwrap());
        rtc.write(ctx, SLP_TIMER1, MAIN_TIMER_ALARM_EN);
        assert_eq!(
            rtc.read(ctx, INT_RAW),
            Some(INT_TIMER_ALARM),
            "raised at the enable"
        );
        assert_eq!(ctx.take_reset(), None, "awake: no wake");
        rtc.write(ctx, DIG_PWC, DEEP);
        rtc.write(ctx, WAKEUP_STATE, TIMER_ONLY);
        rtc.write(ctx, STATE0, SLEEP_EN);
        assert_eq!(ctx.take_reset(), Some(ResetCause::DeepSleep));
    }

    #[test]
    fn the_alarm_wraps_with_the_48_bit_timer() {
        // A target past the wrap of the 48-bit timer is in the future, not passed.
        let ctx = &mut Ctx::default();
        let rtc = rtc(ctx);
        assert_eq!(rtc.counts_until(5, ctx.now()), 5);
        assert_eq!(rtc.counts_until((1 << 48) - 1, ctx.now()), 0, "just behind");
        assert_eq!(rtc.counts_until((1 << 47) - 1, ctx.now()), (1 << 47) - 1);
        assert_eq!(rtc.counts_until(1 << 47, ctx.now()), 0, "half the range");
        assert_eq!(
            rtc.counts_until((1 << 48) + 7, ctx.now()),
            7,
            "bits 48 and up are not compared"
        );
    }

    #[test]
    fn light_sleep_is_refused_by_name_and_rejected_as_the_chip_rejects_it() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        sleep_on_the_timer(ctx, &mut rtc, COUNTS_PER_SECOND, LIGHT);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::LightSleep { dig_pwc: LIGHT })
        );
        assert!(!ctx.take_deep_sleep_entered());
        assert!(!rtc.is_in_deep_sleep());
        assert_eq!(rtc.read(ctx, INT_RAW), Some(INT_SLP_REJECT));
        assert_eq!(rtc.read(ctx, STATE0), Some(0));
        after(ctx, &mut rtc, 2 * SECOND);
        assert_eq!(ctx.take_reset(), None, "an awake chip is not reset");
        assert_eq!(
            rtc.read(ctx, INT_RAW),
            Some(INT_SLP_REJECT | INT_TIMER_ALARM)
        );
    }

    #[test]
    fn a_gpio_wake_is_named_and_the_chip_sleeps_on_the_timer_alone() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, DIG_PWC, DEEP);
        // GPIO alone: what CrossPoint arms to be woken by its power button.
        rtc.write(ctx, WAKEUP_STATE, 1 << 2 << WAKEUP_ENA_SHIFT);
        rtc.write(ctx, STATE0, SLEEP_EN);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::SleepWakeSource { wakeup_ena: 1 << 2 })
        );
        assert!(rtc.is_in_deep_sleep());
        let state = rtc.deep_sleep(ctx.now()).unwrap();
        assert_eq!((state.timer_wake, state.wake_sources), (None, 1 << 2));
        assert_eq!(rtc.next_call(), None, "nothing the model has can wake it");
    }

    #[test]
    fn a_deep_sleep_without_a_wake_source_has_no_end() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, DIG_PWC, DEEP);
        rtc.write(ctx, WAKEUP_STATE, 0);
        rtc.write(ctx, STATE0, SLEEP_EN);
        assert_eq!(ctx.take_unsupported(), None, "no source is no refusal");
        assert!(rtc.is_in_deep_sleep());
        assert_eq!(rtc.next_call(), None);
        after(ctx, &mut rtc, 3_600 * SECOND);
        assert_eq!(ctx.take_reset(), None);
    }

    #[test]
    fn the_timer_without_its_alarm_wakes_nothing() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, SLP_TIMER0, 10);
        rtc.write(ctx, SLP_TIMER1, 0);
        rtc.write(ctx, DIG_PWC, DEEP);
        rtc.write(ctx, WAKEUP_STATE, TIMER_ONLY);
        rtc.write(ctx, STATE0, SLEEP_EN);
        assert_eq!(rtc.deep_sleep(ctx.now()).unwrap().timer_wake, None);
        assert_eq!(rtc.next_call(), None);
    }

    #[test]
    fn the_rtc_watchdog_pauses_in_sleep_unless_told_otherwise() {
        // `WDTCONFIG0.PAUSE_IN_SLP` (bit 9, reset 1, `rtc_cntl_reg.h:1640`): the watchdog of the
        // bootloader, left on, does not reset a sleeping chip; without the bit it does.
        for (config0, reset) in [
            (WDT_EN | 3 << 28 | 1 << 9, None),
            (WDT_EN | 3 << 28, Some(ResetCause::RtcWatchdogCore)),
            (WDT_EN | 2 << 28, Some(ResetCause::RtcWatchdogCpu)),
        ] {
            let ctx = &mut Ctx::default();
            let mut rtc = rtc(ctx);
            rtc.write(ctx, WDTCONFIG1, 68_000);
            rtc.write(ctx, WDTCONFIG0, config0);
            rtc.write(ctx, DIG_PWC, DEEP);
            rtc.write(ctx, WAKEUP_STATE, 0);
            rtc.write(ctx, STATE0, SLEEP_EN);
            assert!(ctx.take_deep_sleep_entered());
            after(ctx, &mut rtc, 10 * SECOND);
            assert_eq!(ctx.take_reset(), reset, "{config0:#x}");
            let Some(cause) = reset else {
                continue;
            };
            // The watchdog ended the sleep: `SLEEP_EN` reads 0 after the reset, so the
            // read-modify-write of `STATE0` that sets `SLP_REJECT_CAUSE_CLR` on the way to the
            // next sleep (DS §A.1.5) does not put the chip back to sleep.
            rtc.reset(ctx, cause, false);
            assert!(!rtc.is_in_deep_sleep(), "{cause:?}");
            assert_eq!(rtc.read(ctx, STATE0), Some(0), "{cause:?}");
            let state0 = rtc.read(ctx, STATE0).unwrap();
            assert!(rtc.write(ctx, STATE0, state0 | 1 << 1));
            assert!(!ctx.take_deep_sleep_entered(), "{cause:?}");
            assert!(!rtc.is_in_deep_sleep(), "{cause:?}");
        }
    }

    #[test]
    fn the_alarm_raises_its_interrupt_once_while_the_chip_is_awake() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, INT_ENA, INT_TIMER_ALARM);
        rtc.write(ctx, SLP_TIMER0, 136_000);
        rtc.write(ctx, SLP_TIMER1, MAIN_TIMER_ALARM_EN);
        assert_eq!(
            rtc.read(ctx, SLP_TIMER1),
            Some(0),
            "the enable is write-only"
        );
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(INT_TIMER_ALARM));
        assert_eq!(ctx.sources(), 1 << 27, "the RTC_CORE source");
        rtc.write(ctx, INT_CLR, INT_TIMER_ALARM);
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, INT_RAW), Some(0), "once per target");
        assert_eq!(ctx.take_reset(), None);
    }

    #[test]
    fn the_sleep_registers_read_back_and_the_causes_are_read_only() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert_eq!(rtc.read(ctx, WAKEUP_STATE), Some(0x0006_0000), "SVD reset");
        assert_eq!(rtc.read(ctx, DIG_PWC), Some(0x0055_5010), "SVD reset");
        rtc.write(ctx, SLP_TIMER0, 0xDEAD_BEEF);
        rtc.write(ctx, SLP_TIMER1, 0xFFFF_1234);
        assert_eq!(rtc.read(ctx, SLP_TIMER0), Some(0xDEAD_BEEF));
        assert_eq!(rtc.read(ctx, SLP_TIMER1), Some(0x1234));
        rtc.write(ctx, WAKEUP_STATE, 0xFFFF_FFFF);
        assert_eq!(rtc.read(ctx, WAKEUP_STATE), Some(0xFFFF_8000));
        rtc.write(ctx, STATE0, 1 << 22 | 1 << 1);
        assert_eq!(rtc.read(ctx, STATE0), Some(1 << 22));
        assert_eq!(ctx.take_unsupported(), None);
        assert!(!rtc.write(ctx, SLP_WAKEUP_CAUSE, 8));
        assert!(!rtc.write(ctx, SLP_REJECT_CAUSE, 8));
        assert_eq!(rtc.read(ctx, SLP_REJECT_CAUSE), Some(0));
    }

    #[test]
    fn the_signals_of_state0_are_refused_by_name() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        for bit in [SW_CPU_INT, SLP_WAKEUP, SLP_REJECT] {
            assert!(rtc.write(ctx, STATE0, bit));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::SleepSignal { state0: bit })
            );
        }
        assert_eq!(rtc.read(ctx, INT_RAW), Some(0), "nothing raised");
    }

    #[test]
    fn the_capture_triggers_of_time_update_are_stored_and_refused_by_name() {
        // F7: TIMER_SYS_STALL, TIMER_XTL_OFF and TIMER_SYS_RST are R/W (`rtc_cntl_reg.h:187-204`).
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert!(rtc.write(ctx, TIME_UPDATE, 1 << 31));
        assert_eq!(ctx.take_unsupported(), None);
        for bit in [27, 28, 29] {
            assert!(rtc.write(ctx, TIME_UPDATE, 1 << bit | 1 << 31));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::RtcTimerTrigger {
                    time_update: 1 << bit | 1 << 31
                })
            );
            assert_eq!(rtc.read(ctx, TIME_UPDATE), Some(1 << bit), "bit 31 reads 0");
        }
        rtc.write(ctx, TIME_UPDATE, 0);
        assert_eq!(rtc.read(ctx, TIME_UPDATE), Some(0));
        assert_eq!(TIME_UPDATE_TRIGGERS, 0x3800_0000);
    }
}
