//! The RTC timer of `RTC_CNTL`: a 48-bit count of slow-clock cycles since power-on, which no
//! reset and no sleep stops (TRM §9.3.3; report TW §C.2). The counter is a field of [`RtcCntl`].
use super::RtcCntl;
use emu_core::Cycles;

/// The RTC timer is 48 bits wide: 16 bits in `TIME_HIGH0` (TRM §9.3.3).
const RTC_TIME_MASK: u64 = (1 << 48) - 1;

impl RtcCntl {
    pub(super) fn rtc_time(&self, now: Cycles) -> u64 {
        self.timer.value(now) & RTC_TIME_MASK
    }
}
