//! What the controller reports: a firmware's use of the chip that the datasheet does not allow,
//! leaves undefined, or the model does not carry out (spec §7: never hide an error).

use std::fmt;

use emu_core::Cycles;

use super::config::{DisplayMode, Temperature};

/// A stage of an operation that needs the clock or the analog supplies (`0x22`, `0x14`, `0x15`,
/// `0x28`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    AnalogOn,
    TemperatureLoad,
    LutLoad,
    Display,
    HvReadyDetection,
    VciDetection,
    VcomSense,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stage::AnalogOn => "starting the analog supplies",
            Stage::TemperatureLoad => "the temperature load",
            Stage::LutLoad => "the LUT load",
            Stage::Display => "the display",
            Stage::HvReadyDetection => "the HV ready detection",
            Stage::VciDetection => "the VCI detection",
            Stage::VcomSense => "the VCOM sensing",
        })
    }
}

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// A command byte the SSD1677 does not have (the X4-family probe sends `0x70` and `0x71`).
    UnknownCommand { command: u8 },
    /// `0x3A` or `0x3B`, which the datasheet reserves.
    ReservedCommand { command: u8 },
    /// A command of the datasheet whose effect the model does not carry out (OTP programming,
    /// CRC, initial code, external sensor command, dithering); its data are accepted.
    UnsupportedCommand { command: u8, name: &'static str },
    /// A data byte with no command before it since the last reset.
    DataWithoutCommand { data: u8 },
    /// More data bytes than `command` takes; reported once per command.
    ExtraData { command: u8 },
    /// `command` was followed by another command, or by a reset, before all its parameters.
    MissingData {
        command: u8,
        expected: u16,
        received: u16,
    },
    /// A command sent while BUSY is high: dropped with its data.
    WhileBusy { command: u8 },
    /// A command sent in deep sleep: dropped with its data; only a hardware reset wakes the chip.
    WhileAsleep { command: u8 },
    /// A command sent while RES# is low: dropped with its data.
    WhileInReset { command: u8 },
    /// A parameter of `command` the datasheet reserves or does not list.
    ReservedValue { command: u8, value: u8 },
    /// A RAM byte written, or read, where the counter lies past the RAM (X beyond 959 or Y beyond
    /// 679): nothing is written; reported once per command.
    RamOutOfRange { command: u8, x: u16, y: u16 },
    /// A RAM write or read starting with the counter outside the window ("undesirable image",
    /// datasheet §8.5).
    CounterOutsideWindow { command: u8, x: u16, y: u16 },
    /// A stage skipped because the clock is off.
    WithoutClock { stage: Stage },
    /// A stage skipped because the analog supplies are off.
    WithoutAnalog { stage: Stage },
    /// No waveform of the OTP matches: the display does not take place (datasheet §6.9).
    NoWaveform {
        mode: DisplayMode,
        temperature: Temperature,
    },
    /// A temperature load from the external sensor with nothing connected: the register keeps its
    /// value.
    NoExternalSensor,
    /// The RAM ping-pong of display mode 2 (`0x37`), not modelled.
    RamPingPong,
    /// A data byte read with nothing to read: the line floats, `read_data` gives `None`.
    NothingToRead { command: u8 },
    /// A hardware reset during the display: the image is the one the glass had reached.
    ResetDuringDisplay,
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagnosticKind::UnknownCommand { command } => {
                write!(f, "unknown command {command:#04x}, its data ignored")
            }
            DiagnosticKind::ReservedCommand { command } => {
                write!(f, "reserved command {command:#04x}, its data ignored")
            }
            DiagnosticKind::UnsupportedCommand { command, name } => {
                write!(f, "command {command:#04x} ({name}) is not modelled: accepted without effect")
            }
            DiagnosticKind::DataWithoutCommand { data } => {
                write!(f, "data byte {data:#04x} with no command before it")
            }
            DiagnosticKind::ExtraData { command } => {
                write!(f, "more data bytes than command {command:#04x} takes")
            }
            DiagnosticKind::MissingData {
                command,
                expected,
                received,
            } => write!(
                f,
                "command {command:#04x} takes {expected} data bytes and got {received}"
            ),
            DiagnosticKind::WhileBusy { command } => {
                write!(f, "command {command:#04x} sent while BUSY is high: dropped")
            }
            DiagnosticKind::WhileAsleep { command } => write!(
                f,
                "command {command:#04x} sent in deep sleep: dropped (only a hardware reset wakes the chip)"
            ),
            DiagnosticKind::WhileInReset { command } => {
                write!(f, "command {command:#04x} sent while RES# is low: dropped")
            }
            DiagnosticKind::ReservedValue { command, value } => write!(
                f,
                "command {command:#04x}: value {value:#04x} is reserved or not listed by the datasheet"
            ),
            DiagnosticKind::RamOutOfRange { command, x, y } => write!(
                f,
                "command {command:#04x}: the RAM counter ({x}, {y}) lies past the RAM; nothing written"
            ),
            DiagnosticKind::CounterOutsideWindow { command, x, y } => write!(
                f,
                "command {command:#04x}: the RAM counter ({x}, {y}) starts outside the window"
            ),
            DiagnosticKind::WithoutClock { stage } => {
                write!(f, "{stage} needs the clock, which is off: skipped")
            }
            DiagnosticKind::WithoutAnalog { stage } => {
                write!(f, "{stage} needs the analog supplies, which are off: skipped")
            }
            DiagnosticKind::NoWaveform { mode, temperature } => write!(
                f,
                "no waveform of the OTP for display mode {} at {} sixteenths of a degree: no display",
                match mode {
                    DisplayMode::One => 1,
                    DisplayMode::Two => 2,
                },
                temperature.sixteenths()
            ),
            DiagnosticKind::NoExternalSensor => f.write_str(
                "temperature load from the external sensor, and none is connected: the register keeps its value",
            ),
            DiagnosticKind::RamPingPong => {
                f.write_str("the RAM ping-pong of display mode 2 is not modelled")
            }
            DiagnosticKind::NothingToRead { command } => {
                write!(f, "a data byte read after command {command:#04x}, which has nothing to read")
            }
            DiagnosticKind::ResetDuringDisplay => f.write_str(
                "hardware reset during the display: the glass keeps the state it had reached",
            ),
        }
    }
}

/// A diagnostic and the instant of device time it happened at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub at: Cycles,
    pub kind: DiagnosticKind,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SSD1677 at cycle {}: {}", self.at.get(), self.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_diagnostic_names_what_happened() {
        let kinds = [
            DiagnosticKind::UnknownCommand { command: 0x70 },
            DiagnosticKind::ReservedCommand { command: 0x3A },
            DiagnosticKind::UnsupportedCommand {
                command: 0x30,
                name: "Program WS OTP",
            },
            DiagnosticKind::DataWithoutCommand { data: 1 },
            DiagnosticKind::ExtraData { command: 0x11 },
            DiagnosticKind::MissingData {
                command: 0x01,
                expected: 3,
                received: 1,
            },
            DiagnosticKind::WhileBusy { command: 0x24 },
            DiagnosticKind::WhileAsleep { command: 0x24 },
            DiagnosticKind::WhileInReset { command: 0x24 },
            DiagnosticKind::ReservedValue {
                command: 0x01,
                value: 3,
            },
            DiagnosticKind::RamOutOfRange {
                command: 0x24,
                x: 960,
                y: 0,
            },
            DiagnosticKind::CounterOutsideWindow {
                command: 0x24,
                x: 0,
                y: 9,
            },
            DiagnosticKind::WithoutClock {
                stage: Stage::LutLoad,
            },
            DiagnosticKind::WithoutAnalog {
                stage: Stage::Display,
            },
            DiagnosticKind::NoWaveform {
                mode: DisplayMode::Two,
                temperature: Temperature::from_celsius(25),
            },
            DiagnosticKind::NoExternalSensor,
            DiagnosticKind::RamPingPong,
            DiagnosticKind::NothingToRead { command: 0x24 },
            DiagnosticKind::ResetDuringDisplay,
        ];
        let texts: Vec<String> = kinds.iter().map(ToString::to_string).collect();
        for (index, text) in texts.iter().enumerate() {
            assert_ne!(text, "");
            assert!(
                !texts[..index].contains(text),
                "two diagnostics read the same: {text}"
            );
        }
        assert_eq!(texts[0], "unknown command 0x70, its data ignored");
        assert_eq!(
            texts[14],
            "no waveform of the OTP for display mode 2 at 400 sixteenths of a degree: no display"
        );
        let diagnostic = Diagnostic {
            at: Cycles(16),
            kind: DiagnosticKind::RamPingPong,
        };
        assert_eq!(
            diagnostic.to_string(),
            "SSD1677 at cycle 16: the RAM ping-pong of display mode 2 is not modelled"
        );
        assert_eq!(
            [
                Stage::AnalogOn,
                Stage::TemperatureLoad,
                Stage::HvReadyDetection,
                Stage::VciDetection,
                Stage::VcomSense,
            ]
            .map(|stage| stage.to_string()),
            [
                "starting the analog supplies",
                "the temperature load",
                "the HV ready detection",
                "the VCI detection",
                "the VCOM sensing"
            ]
        );
    }
}
