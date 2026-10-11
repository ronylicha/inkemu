//! `RTC_CNTL`: reset cause, retention registers, pad and power configuration.
//! Source: report §E.2. Software reset, the RTC timer and the watchdogs arrive in SP1b-3.
use crate::periph::{RTC_CNTL_BASE, Storage};
use emu_core::Peripheral;

const RESET_STATE: u32 = 0x38;
const STORE_COUNT: usize = 8;
/// `RTC_CNTL_STORE0..7`: they survive a reset; the ROM keeps the APB frequency in STORE5.
const STORE: [u32; STORE_COUNT] = [0x50, 0x54, 0x58, 0x5C, 0xB8, 0xBC, 0xC0, 0xC4];
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
const CONFIG: [u32; 5] = [0x84, 0x8C, 0xAC, 0xD0, 0xD4];
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
            config: Storage::for_offsets(RTC_CNTL_BASE, &CONFIG),
        }
    }
}

impl Default for RtcCntl {
    fn default() -> Self {
        RtcCntl::new()
    }
}

impl Peripheral for RtcCntl {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == RESET_STATE {
            return Some(self.reset_state_other | self.reset_cause);
        }
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            return Some(self.store[i]);
        }
        self.config.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
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
        let mut rtc = RtcCntl::new();
        let state = rtc.read(0x38).unwrap();
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
        let mut rtc = RtcCntl::new();
        for (i, offset) in [0x50, 0x54, 0x58, 0x5C, 0xB8, 0xBC, 0xC0, 0xC4]
            .into_iter()
            .enumerate()
        {
            let value = 0x1000 + u32::try_from(i).unwrap();
            assert_eq!(rtc.read(offset), Some(0));
            assert!(rtc.write(offset, value));
            assert_eq!(rtc.read(offset), Some(value));
        }
    }

    #[test]
    fn pad_and_power_configuration_is_plain_storage_from_its_reset_value() {
        let mut rtc = RtcCntl::new();
        for offset in [0x84, 0x8C, 0xAC, 0xD0, 0xD4] {
            let reset = svd_regs::esp32c3::lookup(0x6000_8000 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(rtc.read(offset), Some(reset), "{offset:#x}");
            assert!(rtc.write(offset, 0x55AA));
            assert_eq!(rtc.read(offset), Some(0x55AA));
        }
        assert_eq!(
            rtc.read(0xAC).map(|v| v & 1),
            Some(0),
            "super watchdog reset flag clear"
        );
    }

    #[test]
    fn everything_else_is_left_to_the_unmodeled_path() {
        let mut rtc = RtcCntl::new();
        assert_eq!(
            rtc.read(0x00),
            None,
            "OPTIONS0: software reset arrives in SP1b-3"
        );
        assert!(!rtc.write(0x38, 0), "RESET_STATE is not writable here");
        assert!(
            !rtc.write(0x90, 0),
            "watchdog configuration arrives in SP1b-3"
        );
    }
}
