//! The super watchdog of `RTC_CNTL`: when it is armed, its feed request and its reset, and the
//! writes to `SWD_CONF`. Sources: TRM §12.3, ESP-IDF `rtc_cntl_reg.h:1706-1758`; report TW §C.4.
//! The bits of `SWD_CONF` are described on `SWD_CONF_RESET`.
use super::{FIB_SEL, INT_SWD, RtcCntl, SWD_KEY, SWD_RESET_FLAG};
use crate::clock::SECOND;
use crate::ctx::Ctx;
use crate::reset::ResetCause;
use emu_core::Cycles;

pub(super) const SWD_AUTO_FEED_EN: u32 = 1 << 31;
const SWD_DISABLE: u32 = 1 << 30;
pub(super) const SWD_FEED: u32 = 1 << 29;
pub(super) const SWD_RST_FLAG_CLR: u32 = 1 << 28;
const SWD_BYPASS_RST: u32 = 1 << 17;
const SWD_CONF_STORED: u32 = 0xCFFE_0000;
/// `FIB_SEL` bit 2, `RTC_CNTL_FIB_SUPER_WDT_RST` (`rtc_cntl_reg.h:2341`), reset value 1.
const FIB_SUPER_WDT_RST: u32 = 1 << 2;
/// ASSUMPTION: the super watchdog asks to be fed after 0.9 s and resets after 1 s. The TRM gives
/// "slightly less than one second" for the period and "about 100 ms" before it for the request
/// (§12.3; TW §C.4); the circuit is analog and no figure is published. A real chip left unfed,
/// observed with the feed-request interrupt enabled, settles it.
pub(super) const SWD_FEED_REQUEST: u64 = 9 * SECOND / 10;
const SWD_TIMEOUT: u64 = SECOND;

impl RtcCntl {
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

    /// The super watchdog at the present time; returns the instant it next needs a call.
    pub(super) fn step_super_watchdog(&mut self, ctx: &mut Ctx, now: Cycles) -> Option<Cycles> {
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

    pub(super) fn write_super_watchdog(&mut self, ctx: &mut Ctx, value: u32) {
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
