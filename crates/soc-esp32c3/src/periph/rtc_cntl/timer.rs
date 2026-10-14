//! The RTC timer of `RTC_CNTL`: a 48-bit count of slow-clock cycles since power-on, which no
//! reset and no sleep stops (TRM §9.3.3; report TW §C.2). The counter is a field of [`RtcCntl`].
use super::{RtcCntl, TIME_UPDATE_TRIGGER};
use crate::ctx::Ctx;
use crate::diag::Unsupported;
use emu_core::Cycles;

/// The RTC timer is 48 bits wide: 16 bits in `TIME_HIGH0` (TRM §9.3.3).
const RTC_TIME_MASK: u64 = (1 << 48) - 1;
/// `TIME_UPDATE` bits 27 to 29, `TIMER_SYS_STALL`, `TIMER_XTL_OFF` and `TIMER_SYS_RST`: R/W, each
/// enables a capture of the timer on an event of the chip (`rtc_cntl_reg.h:187-204`; TRM §9.3.3
/// table 9.3-2). The model stores them and captures only on bit 31 (F7).
pub(super) const TIME_UPDATE_TRIGGERS: u32 = 0b111 << 27;
/// A target the timer passed less than half of its range ago counts as reached.
///
/// ASSUMPTION: the alarm of the RTC timer fires at once when it is enabled with a target the timer
/// has already passed, as the alarm of a timer group does (TRM §11.2: "within 2^53"), here over
/// half of 48 bits. The TRM only says the timer is compared with `SLP_TIMER` (§9.4); ESP-IDF never
/// sets a target in the past, but the wake must not wait 2^48 counts when the entry into sleep
/// takes longer than the time asked for (spec §5). An alarm enabled on a chip with a target
/// behind the timer, and the time of the wake, settles it.
const PASSED_WINDOW: u64 = 1 << 47;

impl RtcCntl {
    pub(super) fn rtc_time(&self, now: Cycles) -> u64 {
        self.timer.value(now) & RTC_TIME_MASK
    }

    /// Counts of the RTC timer until it reads `target` (48 bits), 0 once it has reached it.
    pub(super) fn counts_until(&self, target: u64, now: Cycles) -> u64 {
        let ahead = target.wrapping_sub(self.rtc_time(now)) & RTC_TIME_MASK;
        if ahead >= PASSED_WINDOW { 0 } else { ahead }
    }

    /// The device time at which the RTC timer reads `target`, `now` once it has reached it. One
    /// computation from the rate of the slow clock: nothing is counted cycle by cycle, however far
    /// the target is.
    pub(super) fn instant_of(&self, target: u64, now: Cycles) -> Cycles {
        match self.counts_until(target, now) {
            0 => now,
            // INVARIANT: the RTC timer always runs (`RtcCntl::new` starts it, nothing stops it),
            // so `instant_in` always has an instant to give.
            ahead => self.timer.instant_in(ahead, now).unwrap_or(now),
        }
    }

    /// A write of `TIME_UPDATE`: bit 31 captures the timer, bits 27 to 29 are stored and refused
    /// by name when set (F7).
    pub(super) fn write_time_update(&mut self, ctx: &mut Ctx, value: u32) {
        if value & TIME_UPDATE_TRIGGER != 0 {
            // "Each trigger moves the previous capture into TIME_LOW1/HIGH1" (TW §C.2).
            self.time = [self.rtc_time(ctx.now()), self.time[0]];
        }
        if value & TIME_UPDATE_TRIGGERS != 0 {
            ctx.unsupported(Unsupported::RtcTimerTrigger { time_update: value });
        }
        self.time_update = value & TIME_UPDATE_TRIGGERS;
    }
}
