//! Resets: how much of the chip each one returns to its reset state, and the cause it leaves in
//! `RTC_CNTL.RESET_STATE`. Sources: TRM §6.1 (figure 6.1-1, table 6.1-1); reports IR §4.2 and TW §D.
use std::fmt;

/// How much of the chip a reset returns to its reset state (TRM §6.1.3; TW §D.2).
///
/// The hardware has a fourth level, the chip reset of a power-on: in this model it is the
/// construction of the machine, and nothing asks for one while it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResetLevel {
    /// "Only resets CPU core", and with it the `SENSITIVE` permission registers. Every other
    /// peripheral register survives, the clock selection included.
    Cpu,
    /// "The whole digital system except RTC": the CPU and every digital peripheral. `RTC_CNTL`
    /// survives, with the RTC watchdog and its counter, and so does the recorded PC.
    Core,
    /// "The whole digital system, including RTC". The RTC timer keeps counting (TRM §9.3.3) and
    /// the memories keep their content (TRM §6.1.1).
    System,
}

/// The level in words, for a message: `a core reset`.
impl fmt::Display for ResetLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ResetLevel::Cpu => "a CPU reset",
            ResetLevel::Core => "a core reset",
            ResetLevel::System => "a system reset",
        })
    }
}

/// Why the chip was reset: the code `RTC_CNTL.RESET_STATE[5:0]` reads at the next boot (TRM table
/// 6.1-1), which the ROM prints as `rst:0x… (NAME)` (report TW §D.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetCause {
    /// `RTC_CNTL.OPTIONS0` bit 31, `SW_SYS_RST` (`ROM3:software_reset@0x40048b74`): `RTC_SW_SYS_RST`.
    SoftwareSystem,
    /// Main watchdog 0, stage action 3, or its flash-boot protection: `TG0WDT_SYS_RST`.
    Mwdt0Core,
    /// Main watchdog 1, stage action 3: `TG1WDT_SYS_RST`.
    Mwdt1Core,
    /// RTC watchdog, stage action 3: `RTCWDT_SYS_RST`.
    RtcWatchdogCore,
    /// Main watchdog 0, stage action 2: `TG0WDT_CPU_RST`.
    Mwdt0Cpu,
    /// `RTC_CNTL.OPTIONS0` bit 5, `SW_PROCPU_RST` (`ROM3:software_reset_cpu@0x40048b84`), what
    /// `esp_restart` ends with: `RTC_SW_CPU_RST`.
    SoftwareCpu,
    /// RTC watchdog, stage action 2: `RTCWDT_CPU_RST`.
    RtcWatchdogCpu,
    /// RTC watchdog, stage action 4, or its flash-boot protection: `RTCWDT_RTC_RST`.
    RtcWatchdogSystem,
    /// Main watchdog 1, stage action 2: `TG1WDT_CPU_RST`.
    Mwdt1Cpu,
    /// The chip woke from deep sleep: `DSLEEP` (`ROM3:boot_prepare`, the name table at
    /// `0x3ff19da8`; report DS §A.3.2). The wake is a core reset: "the whole digital system except
    /// RTC" (TRM table 6.1-1). The RTC timer is the only wake source the model raises (F2, F5).
    DeepSleep,
    /// The super watchdog was not fed: `SUPER_WDT_RST`.
    SuperWatchdog,
}

/// `RESET_STATE[5:0]` after a power-on (chip) reset: `POWERON`.
pub const CAUSE_POWER_ON: u32 = 0x01;

impl ResetCause {
    /// The code of TRM table 6.1-1 (ESP-IDF `soc/reset_reasons.h`).
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            ResetCause::SoftwareSystem => 0x03,
            ResetCause::DeepSleep => 0x05,
            ResetCause::Mwdt0Core => 0x07,
            ResetCause::Mwdt1Core => 0x08,
            ResetCause::RtcWatchdogCore => 0x09,
            ResetCause::Mwdt0Cpu => 0x0B,
            ResetCause::SoftwareCpu => 0x0C,
            ResetCause::RtcWatchdogCpu => 0x0D,
            ResetCause::RtcWatchdogSystem => 0x10,
            ResetCause::Mwdt1Cpu => 0x11,
            ResetCause::SuperWatchdog => 0x12,
        }
    }

    /// The level the cause acts at (TRM table 6.1-1).
    #[must_use]
    pub const fn level(self) -> ResetLevel {
        match self {
            ResetCause::Mwdt0Cpu
            | ResetCause::SoftwareCpu
            | ResetCause::RtcWatchdogCpu
            | ResetCause::Mwdt1Cpu => ResetLevel::Cpu,
            ResetCause::SoftwareSystem
            | ResetCause::DeepSleep
            | ResetCause::Mwdt0Core
            | ResetCause::Mwdt1Core
            | ResetCause::RtcWatchdogCore => ResetLevel::Core,
            ResetCause::RtcWatchdogSystem | ResetCause::SuperWatchdog => ResetLevel::System,
        }
    }

    /// What asked for the reset, in words.
    const fn source(self) -> &'static str {
        match self {
            ResetCause::SoftwareSystem | ResetCause::SoftwareCpu => "the firmware",
            ResetCause::Mwdt0Core | ResetCause::Mwdt0Cpu => "main watchdog 0",
            ResetCause::Mwdt1Core | ResetCause::Mwdt1Cpu => "main watchdog 1",
            ResetCause::RtcWatchdogCore
            | ResetCause::RtcWatchdogCpu
            | ResetCause::RtcWatchdogSystem => "the RTC watchdog",
            ResetCause::SuperWatchdog => "the super watchdog",
            ResetCause::DeepSleep => "the RTC timer, waking the chip from deep sleep",
        }
    }
}

/// The cause in words, for a message: what asked for the reset, then its level, as in
/// `main watchdog 0, a core reset`. The one place that names a cause for a person; the name the
/// ROM prints next to the code (`TG0WDT_SYS_RST`) is the ROM's own.
impl fmt::Display for ResetCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}, {}", self.source(), self.level())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causes_have_the_codes_and_levels_of_the_reference_manual() {
        // TRM table 6.1-1, as tabulated in TW §D.3.
        for (cause, code, level) in [
            (ResetCause::SoftwareSystem, 0x03, ResetLevel::Core),
            (ResetCause::DeepSleep, 0x05, ResetLevel::Core),
            (ResetCause::Mwdt0Core, 0x07, ResetLevel::Core),
            (ResetCause::Mwdt1Core, 0x08, ResetLevel::Core),
            (ResetCause::RtcWatchdogCore, 0x09, ResetLevel::Core),
            (ResetCause::Mwdt0Cpu, 0x0B, ResetLevel::Cpu),
            (ResetCause::SoftwareCpu, 0x0C, ResetLevel::Cpu),
            (ResetCause::RtcWatchdogCpu, 0x0D, ResetLevel::Cpu),
            (ResetCause::RtcWatchdogSystem, 0x10, ResetLevel::System),
            (ResetCause::Mwdt1Cpu, 0x11, ResetLevel::Cpu),
            (ResetCause::SuperWatchdog, 0x12, ResetLevel::System),
        ] {
            assert_eq!((cause.code(), cause.level()), (code, level), "{cause:?}");
        }
        assert!(ResetLevel::Cpu < ResetLevel::Core && ResetLevel::Core < ResetLevel::System);
    }

    #[test]
    fn every_cause_has_its_own_words_which_name_what_asked_and_how_deep_the_reset_goes() {
        let words: Vec<String> = [
            ResetCause::SoftwareSystem,
            ResetCause::Mwdt0Core,
            ResetCause::Mwdt1Core,
            ResetCause::RtcWatchdogCore,
            ResetCause::Mwdt0Cpu,
            ResetCause::SoftwareCpu,
            ResetCause::RtcWatchdogCpu,
            ResetCause::RtcWatchdogSystem,
            ResetCause::Mwdt1Cpu,
            ResetCause::SuperWatchdog,
            ResetCause::DeepSleep,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            words,
            [
                "the firmware, a core reset",
                "main watchdog 0, a core reset",
                "main watchdog 1, a core reset",
                "the RTC watchdog, a core reset",
                "main watchdog 0, a CPU reset",
                "the firmware, a CPU reset",
                "the RTC watchdog, a CPU reset",
                "the RTC watchdog, a system reset",
                "main watchdog 1, a CPU reset",
                "the super watchdog, a system reset",
                "the RTC timer, waking the chip from deep sleep, a core reset",
            ]
        );
    }
}
