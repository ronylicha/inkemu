//! `RTC_CNTL`: reset cause, retention registers, pad, clock, analog and power configuration.
//! Source: report §E.2. The watchdogs are stored and never expire (decision D11); the RTC timer and
//! resets arrive in SP1b-3.
use crate::ctx::Ctx;
use crate::diag::Unsupported;
use crate::periph::{Accepted, RTC_CNTL_BASE, Storage};
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const OPTIONS0: u32 = addr("RTC_CNTL", "OPTIONS0") - RTC_CNTL_BASE;
/// `OPTIONS0` bit 31 `SW_SYS_RST` and bit 5 `SW_PROCPU_RST` (report BC §E.2).
const SW_SYS_RST: u32 = 1 << 31;
const SW_PROCPU_RST: u32 = 1 << 5;
const RESET_STATE: u32 = addr("RTC_CNTL", "RESET_STATE") - RTC_CNTL_BASE;
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
/// Configuration the ROM reads back and modifies before its banner: `PWC`, `DIG_ISO`,
/// `SWD_CONF`, `PAD_HOLD`, `DIG_PAD_HOLD`. Nothing acts on it yet.
///
/// `SWD_CONF` is kept as plain storage although on silicon it mixes a read-only reset flag and
/// write-only strobes. Its bits (ESP-IDF `rtc_cntl_reg.h`, `RTC_CNTL_SWD_CONF_REG`; report §E.2;
/// same names in the SVD): `SWD_RESET_FLAG` [0] and `SWD_FEED_INT` [1] read-only; `SWD_BYPASS_RST` [17],
/// `SWD_SIGNAL_WIDTH` [27:18], `SWD_DISABLE` [30] and `SWD_AUTO_FEED_EN` [31] read/write;
/// `SWD_RST_FLAG_CLR` [28] and `SWD_FEED` [29] write-only. Storing them is harmless for the boot,
/// which only reads bit 0 (it stays clear after a power-on reset); the super watchdog itself is
/// modelled in SP1b-3.
const ACCEPTED: [Accepted; 26] = [
    Accepted::svd(
        "RTC_CNTL",
        "OPTIONS0",
        "BBPLL, crystal and I2C power forcing (software reset bits are refused)",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "TIMER1",
        "clock and PLL wake-up delays",
        "SP1b-3",
    ),
    Accepted::svd("RTC_CNTL", "TIMER3", "Wi-Fi/BT power-up timing", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "TIMER4", "digital power-up timing", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "TIMER5", "minimum sleep time", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "TIMER6", "peripheral power-up timing", "SP1b-3"),
    Accepted::svd(
        "RTC_CNTL",
        "ANA_CONF",
        "analog configuration, glitch reset",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "INT_ENA_RTC",
        "RTC interrupt enables: nothing is raised",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "INT_CLR_RTC",
        "RTC interrupt clear: nothing is pending",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "CLK_CONF",
        "RTC fast and slow clock sources and dividers",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "SLOW_CLK_CONF",
        "RTC slow clock divider",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "RTC_CNTL",
        "RTC regulator and digital bias",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "DIG_PWC",
        "digital power-domain control",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "WDTCONFIG0",
        "RTC watchdog enable and stages: it never expires (D11)",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "WDTCONFIG1",
        "RTC watchdog stage 0 timeout",
        "SP1b-3",
    ),
    Accepted::svd("RTC_CNTL", "WDTFEED", "RTC watchdog feed", "SP1b-3"),
    Accepted::svd(
        "RTC_CNTL",
        "WDTWPROTECT",
        "RTC watchdog write protection: writes are never locked out",
        "SP1b-3",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "SWD_WPROTECT",
        "super watchdog write protection",
        "SP1b-3",
    ),
    Accepted::svd("RTC_CNTL", "BROWN_OUT", "brown-out detector", "SP1b-3"),
    Accepted::svd(
        "RTC_CNTL",
        "FIB_SEL",
        "brown-out and glitch reset selection",
        "SP1b-3",
    ),
    Accepted::svd("RTC_CNTL", "SENSOR_CTRL", "SAR ADC power forcing", "SP2"),
    Accepted::svd("RTC_CNTL", "PWC", "RTC power-domain control", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "DIG_ISO", "digital pad isolation", "SP1b-3"),
    Accepted::svd(
        "RTC_CNTL",
        "SWD_CONF",
        "super watchdog: auto-feed, reset flag, strobes",
        "SP1b-3",
    ),
    Accepted::svd("RTC_CNTL", "PAD_HOLD", "pad hold", "SP2"),
    Accepted::svd("RTC_CNTL", "DIG_PAD_HOLD", "digital pad hold", "SP2"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// `RESET_CAUSE_PROCPU`, bits [5:0] of `RESET_STATE` (`soc/reset_reasons.h`).
const RESET_CAUSE_MASK: u32 = 0x3F;
const RESET_CAUSE_POWERON: u32 = 0x01;

#[derive(Clone, Debug)]
pub struct RtcCntl {
    reset_cause: u32,
    /// The bits of `RESET_STATE` other than the cause, at their documented reset value.
    reset_state_other: u32,
    store: [u32; STORE_COUNT],
    config: Storage,
}

impl RtcCntl {
    #[must_use]
    pub fn new() -> RtcCntl {
        let documented =
            svd_regs::esp32c3::lookup(RTC_CNTL_BASE + RESET_STATE).map_or(0, |r| r.reset_value);
        RtcCntl {
            reset_cause: RESET_CAUSE_POWERON,
            reset_state_other: documented & !RESET_CAUSE_MASK,
            store: [0; STORE_COUNT],
            config: Storage::accepted(RTC_CNTL_BASE, &ACCEPTED),
        }
    }
}

impl Default for RtcCntl {
    fn default() -> Self {
        RtcCntl::new()
    }
}

impl Peripheral<Ctx> for RtcCntl {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if offset == RESET_STATE {
            return Some(self.reset_state_other | self.reset_cause);
        }
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            return Some(self.store[i]);
        }
        self.config.read(offset)
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        // Decision D27: a software reset is not emulated yet (SP1b-3). Storing the bit would be a
        // reset that silently never happens: the request is reported and dropped instead.
        if offset == OPTIONS0 && value & (SW_SYS_RST | SW_PROCPU_RST) != 0 {
            ctx.unsupported(Unsupported::SoftwareReset { options0: value });
            return true;
        }
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            self.store[i] = value;
            return true;
        }
        self.config.write(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn power_on_reset_cause_is_reported() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new();
        let state = rtc.read(ctx, 0x38).unwrap();
        assert_eq!(state & 0x3F, 1, "RESET_CAUSE_PROCPU = POWERON");
        assert_eq!(state & (1 << 20), 0, "no JTAG reset flag");
        assert_eq!(
            state & !0x3F,
            0x3000,
            "the other bits keep their documented reset value"
        );
    }

    #[test]
    fn store_registers_keep_what_the_rom_writes() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new();
        for (i, offset) in [0x50, 0x54, 0x58, 0x5C, 0xB8, 0xBC, 0xC0, 0xC4]
            .into_iter()
            .enumerate()
        {
            let value = 0x1000 + u32::try_from(i).unwrap();
            assert_eq!(rtc.read(ctx, offset), Some(0));
            assert!(rtc.write(ctx, offset, value));
            assert_eq!(rtc.read(ctx, offset), Some(value));
        }
    }

    #[test]
    fn pad_and_power_configuration_is_plain_storage_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new();
        for offset in [0x84, 0x8C, 0xAC, 0xD0, 0xD4] {
            let reset = svd_regs::esp32c3::lookup(0x6000_8000 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(rtc.read(ctx, offset), Some(reset), "{offset:#x}");
            assert!(rtc.write(ctx, offset, 0x55AA));
            assert_eq!(rtc.read(ctx, offset), Some(0x55AA));
        }
        assert_eq!(
            rtc.read(ctx, 0xAC).map(|v| v & 1),
            Some(0),
            "super watchdog reset flag clear"
        );
    }

    fn reg(name: &str) -> u32 {
        svd_regs::esp32c3::addr("RTC_CNTL", name) - RTC_CNTL_BASE
    }

    #[test]
    fn clock_analog_and_watchdog_configuration_is_stored_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new();
        for (name, reset) in [
            ("OPTIONS0", 0x1C00_A000),
            ("TIMER1", 0x2814_0403),
            ("ANA_CONF", 0x00C4_0000),
            ("CLK_CONF", 0x1158_3218),
            ("RTC_CNTL", 0xA000_0000),
            ("DIG_PWC", 0x0055_5010),
            ("WDTCONFIG0", 0x0001_3214),
            ("WDTCONFIG1", 0x0003_0D40),
            ("BROWN_OUT", 0x43FF_0010),
            ("FIB_SEL", 0x0000_0007),
        ] {
            assert_eq!(rtc.read(ctx, reg(name)), Some(reset), "{name}");
            assert!(rtc.write(ctx, reg(name), 0x0000_0140));
            assert_eq!(rtc.read(ctx, reg(name)), Some(0x0000_0140), "{name}");
        }
        for name in [
            "TIMER3",
            "TIMER4",
            "TIMER5",
            "TIMER6",
            "SLOW_CLK_CONF",
            "INT_ENA_RTC",
            "INT_CLR_RTC",
            "WDTFEED",
            "WDTWPROTECT",
            "SWD_WPROTECT",
            "SENSOR_CTRL",
        ] {
            assert!(rtc.write(ctx, reg(name), 0x8F1D_312A), "{name}");
            assert_eq!(rtc.read(ctx, reg(name)), Some(0x8F1D_312A), "{name}");
        }
    }

    #[test]
    fn the_bootloader_watchdog_sequence_is_stored_as_written_and_changes_no_reset_cause() {
        // BA §6: unlock, clear FLASHBOOT_MOD_EN, stage 0 = 0x956A0, feed, enable, lock.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new();
        rtc.write(ctx, reg("WDTWPROTECT"), 0x50D8_3AA1);
        rtc.write(ctx, reg("WDTCONFIG1"), 0x0009_56A0);
        rtc.write(ctx, reg("WDTFEED"), 1 << 31);
        rtc.write(ctx, reg("WDTCONFIG0"), 0xC007_EA00);
        rtc.write(ctx, reg("WDTWPROTECT"), 0);
        assert_eq!(rtc.read(ctx, reg("WDTCONFIG0")), Some(0xC007_EA00));
        assert_eq!(rtc.read(ctx, reg("WDTCONFIG1")), Some(0x0009_56A0));
        assert_eq!(
            rtc.read(ctx, RESET_STATE).map(|v| v & 0x3F),
            Some(1),
            "still a power-on reset: decision D11"
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_software_reset_request_is_reported_and_not_stored() {
        // BC §E.2: OPTIONS0 bit 31 SW_SYS_RST, bit 5 SW_PROCPU_RST.
        let ctx = &mut Ctx::default();
        for value in [1u32 << 31, 1 << 5, 0x1C00_A000 | (1 << 5)] {
            let mut rtc = RtcCntl::new();
            assert!(rtc.write(ctx, reg("OPTIONS0"), value));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::SoftwareReset { options0: value })
            );
            assert_eq!(ctx.take_unsupported(), None);
            assert_eq!(
                rtc.read(ctx, reg("OPTIONS0")),
                Some(0x1C00_A000),
                "not stored: decision D27"
            );
        }
        let mut rtc = RtcCntl::new();
        rtc.write(ctx, reg("OPTIONS0"), 0x1C00_A000 & !(1 << 13));
        assert_eq!(
            ctx.take_unsupported(),
            None,
            "BBPLL power bits are plain configuration (BA §5 step 9)"
        );
    }

    #[test]
    fn two_software_reset_requests_before_a_relay_are_both_reported_in_order() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new();
        rtc.write(ctx, reg("OPTIONS0"), 1 << 31);
        rtc.write(ctx, reg("OPTIONS0"), 1 << 5);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::SoftwareReset { options0: 1 << 31 })
        );
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::SoftwareReset { options0: 1 << 5 })
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn everything_else_is_left_to_the_unmodeled_path() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new();
        assert!(
            !rtc.write(ctx, RESET_STATE, 0),
            "RESET_STATE is not writable here"
        );
        for name in [
            "TIME_UPDATE",
            "TIME_LOW0",
            "SLP_TIMER0",
            "STATE0",
            "WDTCONFIG2",
        ] {
            assert_eq!(
                rtc.read(ctx, reg(name)),
                None,
                "{name}: the RTC timer, sleep and watchdog stages are SP1b-3"
            );
        }
    }
}
