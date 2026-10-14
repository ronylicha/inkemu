//! The RTC watchdog of `RTC_CNTL`: its stages, their actions and timeouts, its flash-boot
//! protection and the writes to its registers. Sources: TRM §12.2, ESP-IDF `rwdt_ll.h`; report TW
//! §C.3, §D.4, §D.5. The registers and the counter are fields of [`RtcCntl`].
use super::{INT_WDT, RtcCntl, WDT_KEY, WDTCONFIG0, WDTCONFIG1, WDTFEED};
use crate::ctx::Ctx;
use crate::periph::index_in;
use crate::periph::stages::{self, STAGES as WDT_STAGES};
use crate::reset::ResetCause;
use emu_core::Cycles;

// `WDTCONFIG0` (see `WDT_EN`): three bits per stage from [30:28] for stage 0 to [21:19] for
// stage 3 (`rwdt_ll_config_stage`, line 113), bit 12 `FLASHBOOT_MOD_EN` (`rwdt_ll_set_flashboot_en`,
// line 196).
const WDT_STAGE0_SHIFT: u32 = 28;
const WDT_STAGE_BITS: u32 = 3;
const WDT_STAGE_MASK: u32 = 0b111;
const WDT_FLASHBOOT_MOD_EN: u32 = 1 << 12;
/// `WDTFEED` bit 31 feeds (`rwdt_ll_feed`, line 266).
pub(super) const WDT_FEED: u32 = 1 << 31;
/// Stage actions (`IDF:hal/include/hal/wdt_types.h`; TW §D.5).
pub(super) const ACTION_INTERRUPT: u32 = 1;
pub(super) const ACTION_RESET_CPU: u32 = 2;
pub(super) const ACTION_RESET_SYSTEM: u32 = 3;
pub(super) const ACTION_RESET_RTC: u32 = 4;

impl RtcCntl {
    /// Whether the flash-boot protection runs the watchdog (TRM §12.2.2.4; TW §D.4).
    ///
    /// ASSUMPTION: as for the main watchdog (`Timg::flash_boot_protection`), the protection is off
    /// when the chip is not strapped to boot from flash (TW §I question 4). A boot of a chip
    /// strapped for download, with the RTC watchdog counting, settles it.
    pub(super) const fn flash_boot_protection(&self) -> bool {
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

    /// Lets the RTC watchdog go through the stages whose timeout the count has reached, and returns
    /// the instant of the next expiry. The walk and its rules are those of every watchdog of the
    /// chip ([`stages::walk`]); what is this device's own is the action of a stage that ended.
    pub(super) fn walk_stages(
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

    /// A write to a watchdog register other than the key: dropped unless the key is in place.
    pub(super) fn write_watchdog(&mut self, ctx: &mut Ctx, offset: u32, value: u32) {
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
}
