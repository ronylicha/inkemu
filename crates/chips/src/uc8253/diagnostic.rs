//! What the X3's controller reports: a firmware's use of the chip that the datasheet does not
//! allow, leaves undefined, or the model does not carry out (spec §7: never hide an error).

use std::fmt;

use emu_core::Cycles;

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// A command byte the controller does not have; its data are ignored.
    UnknownCommand { command: u8 },
    /// A command of the datasheet the model does not carry out (OTP programming, the automatic
    /// sequence, dual SPI); its data are accepted without effect.
    UnsupportedCommand { command: u8, name: &'static str },
    /// A data byte with no command before it since the last reset.
    DataWithoutCommand { data: u8 },
    /// More data bytes than `command` takes; reported once per command (DS79: "the chip accepts
    /// only defined parameters, and ignores undefined ones").
    ExtraData { command: u8 },
    /// `command` was followed by another command, or by a reset, before all its parameters.
    MissingData {
        command: u8,
        expected: u16,
        received: u16,
    },
    /// A command sent while `BUSY_N` is low: dropped with its data (DS79: during a refresh only
    /// REV, FLG and VV are accepted).
    WhileBusy { command: u8 },
    /// A command sent in deep sleep: dropped; only `RST_N` low wakes the chip.
    WhileAsleep { command: u8 },
    /// A command sent while `RST_N` is low or the chip initialises after it: dropped.
    WhileInReset { command: u8 },
    /// `DSLP` with a check code other than `0xA5`: the chip stays awake (DS79).
    WrongCheckCode { code: u8 },
    /// A pixel byte written past the window its command fills: nothing is written; reported once
    /// per command.
    PastTheWindow { command: u8 },
    /// A refresh the controller does not carry out the way the firmware set it: `what` names the
    /// setting (`PSR`: LUT from OTP, KWR mode, booster off). The glass does not change.
    RefreshNotModelled { what: &'static str },
    /// A refresh while the supplies are off (`DRF` before `PON`): the glass does not change.
    RefreshWithoutPower,
    /// A LUT command that ended with `bytes` bytes where the model reads 42 (the `UC8279d`'s grey
    /// banks of 49 bytes are a format no source describes): its first 42 bytes drive the refresh.
    LutLength { command: u8, bytes: u16 },
    /// A data byte read with nothing to read: the line floats.
    NothingToRead { command: u8 },
    /// A hardware reset during a refresh: the image is the one the glass had reached.
    ResetDuringRefresh,
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagnosticKind::UnknownCommand { command } => {
                write!(f, "unknown command {command:#04x}, its data ignored")
            }
            DiagnosticKind::UnsupportedCommand { command, name } => {
                write!(
                    f,
                    "command {command:#04x} ({name}) is not modelled: accepted without effect"
                )
            }
            DiagnosticKind::DataWithoutCommand { data } => {
                write!(f, "data byte {data:#04x} with no command before it")
            }
            DiagnosticKind::ExtraData { command } => write!(
                f,
                "more data bytes than command {command:#04x} takes: the extra ones are ignored"
            ),
            DiagnosticKind::MissingData {
                command,
                expected,
                received,
            } => write!(
                f,
                "command {command:#04x} takes {expected} data bytes and got {received}"
            ),
            DiagnosticKind::WhileBusy { command } => write!(
                f,
                "command {command:#04x} sent while BUSY_N is low: dropped; wait for BUSY_N high"
            ),
            DiagnosticKind::WhileAsleep { command } => write!(
                f,
                "command {command:#04x} sent in deep sleep: dropped; pulse RST_N low to wake the chip"
            ),
            DiagnosticKind::WhileInReset { command } => write!(
                f,
                "command {command:#04x} sent while RST_N is low or the chip initialises: dropped; wait for BUSY_N high after the reset"
            ),
            DiagnosticKind::WrongCheckCode { code } => write!(
                f,
                "deep sleep refused: its check code is {code:#04x}, not 0xa5"
            ),
            DiagnosticKind::PastTheWindow { command } => write!(
                f,
                "pixel bytes of command {command:#04x} past the window it fills: ignored; send as many bytes as the window holds"
            ),
            DiagnosticKind::RefreshNotModelled { what } => write!(
                f,
                "a refresh with {what} is not modelled: the glass does not change; set PSR to KW mode with the LUTs from the registers and the booster on"
            ),
            DiagnosticKind::RefreshWithoutPower => write!(
                f,
                "a refresh while the supplies are off: the glass does not change; send PON first"
            ),
            DiagnosticKind::LutLength { command, bytes } => write!(
                f,
                "LUT {command:#04x} got {bytes} bytes where KW mode reads 42: its first 42 drive the refresh"
            ),
            DiagnosticKind::NothingToRead { command } => write!(
                f,
                "read of command {command:#04x}: the controller drives nothing, the line floats"
            ),
            DiagnosticKind::ResetDuringRefresh => write!(
                f,
                "a hardware reset cut a refresh short: the glass keeps the state it had reached"
            ),
        }
    }
}

/// A diagnostic and the instant it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub at: Cycles,
    pub kind: DiagnosticKind,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at cycle {})", self.kind, self.at.get())
    }
}
