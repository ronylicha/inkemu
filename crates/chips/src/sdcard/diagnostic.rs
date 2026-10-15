//! What the card reports: a host's use of it that the specification refuses, leaves undefined,
//! or the model does not carry out (spec §7: never hide an error; design `SP2e`, 2e-D5, 2e-D10).
use std::fmt;

use emu_core::Cycles;

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdDiagnosticKind {
    /// A command other than CMD0 before the card entered SPI mode: ignored, MISO left alone.
    CommandInSdMode { index: u8 },
    /// CMD0 with a CRC other than `95h` before the card entered SPI mode: ignored.
    Cmd0CrcInSdMode { crc: u8 },
    /// A command whose CRC7 is wrong while the card checks it (CMD8 always, everything after
    /// CMD59): answered with the CRC error bit, not executed.
    CommandCrc { index: u8, crc: u8, expected: u8 },
    /// A command the idle state does not take (only CMD0, CMD8, CMD55, ACMD41, CMD58, CMD59).
    IllegalInIdle { index: u8, app: bool },
    /// An index the SPI mode does not have: answered "illegal command".
    UnknownCommand { index: u8, app: bool },
    /// A command of the specification the model does not carry out: answered "illegal command".
    UnsupportedCommand {
        index: u8,
        app: bool,
        name: &'static str,
    },
    /// CMD1 before any ACMD41 since power-on: illegal for a thin card (PLS §7.2.7).
    Cmd1BeforeAcmd41,
    /// A high-capacity card was asked to initialize without CMD8 before, or with HCS 0: it
    /// stays in the idle state (PLS Figure 7-1, note 1).
    HighCapacityWithoutHcs { cmd8: bool },
    /// A command while the card is busy: ignored.
    CommandWhileBusy { index: u8 },
    /// A command while the card sends blocks or waits for the next written block, other than
    /// CMD0 and CMD12: refused.
    CommandDuringTransfer { index: u8 },
    /// The card was deselected in the middle of a command: the command is lost.
    CommandCut,
    /// The card was deselected in the middle of a data block: the block is abandoned.
    TransferCut,
    /// A byte that starts no command where the card waits for one.
    StrayByte { byte: u8 },
    /// A token that does not fit the write in progress.
    UnexpectedToken { token: u8 },
    /// An address past the end of the card: parameter error.
    AddressOutOfRange { address: u32 },
    /// A standard-capacity block that crosses a 512-byte block, or a write at an address not on
    /// one: address error.
    MisalignedAddress { address: u32, length: u32 },
    /// A block length outside 1 to 512, or a write with a length other than 512.
    BlockLength { length: u32 },
    /// A multiple-block read went past the last block: the data error token "out of range".
    ReadPastEnd { block: u64 },
    /// A written block whose CRC16 is wrong while the card checks it: rejected.
    DataCrc { block: u64, crc: u16, expected: u16 },
    /// A write to a write-protected card: refused.
    WriteProtected { block: u64 },
    /// An erase of a write-protected card: nothing erased.
    EraseProtected,
    /// CMD38 without both addresses, or an address command after the erase started.
    EraseSequence,
    /// The storage behind the card failed.
    StorageFailed { block: u64, detail: String },
}

impl fmt::Display for SdDiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |index: &u8, app: &bool| format!("{}{index}", if *app { "ACMD" } else { "CMD" });
        match self {
            SdDiagnosticKind::CommandInSdMode { index } => write!(
                f,
                "CMD{index} before CMD0 put the card in SPI mode: ignored"
            ),
            SdDiagnosticKind::Cmd0CrcInSdMode { crc } => write!(
                f,
                "CMD0 with CRC {crc:#04x} instead of 0x95 before the card is in SPI mode: ignored"
            ),
            SdDiagnosticKind::CommandCrc {
                index,
                crc,
                expected,
            } => write!(
                f,
                "CMD{index} with CRC {crc:#04x} instead of {expected:#04x}: answered with a CRC error, not executed"
            ),
            SdDiagnosticKind::IllegalInIdle { index, app } => write!(
                f,
                "{} in the idle state, which takes only CMD0, CMD8, CMD55, ACMD41, CMD58 and CMD59: illegal command",
                name(index, app)
            ),
            SdDiagnosticKind::UnknownCommand { index, app } => write!(
                f,
                "{} is no command of the SPI mode: illegal command",
                name(index, app)
            ),
            SdDiagnosticKind::UnsupportedCommand { index, app, name: what } => write!(
                f,
                "{} ({what}) is not modelled: illegal command",
                name(index, app)
            ),
            SdDiagnosticKind::Cmd1BeforeAcmd41 => {
                f.write_str("CMD1 before any ACMD41 since power-on: illegal command")
            }
            SdDiagnosticKind::HighCapacityWithoutHcs { cmd8 } => write!(
                f,
                "a high-capacity card asked to initialize {}: it stays idle",
                if *cmd8 { "with HCS 0" } else { "without CMD8 before" }
            ),
            SdDiagnosticKind::CommandWhileBusy { index } => {
                write!(f, "CMD{index} while the card is busy: ignored")
            }
            SdDiagnosticKind::CommandDuringTransfer { index } => write!(
                f,
                "CMD{index} during a data transfer, where only CMD0 and CMD12 are taken: refused"
            ),
            SdDiagnosticKind::CommandCut => {
                f.write_str("the card was deselected in the middle of a command: it is lost")
            }
            SdDiagnosticKind::TransferCut => f.write_str(
                "the card was deselected in the middle of a data block: the block is abandoned",
            ),
            SdDiagnosticKind::StrayByte { byte } => {
                write!(f, "byte {byte:#04x} where a command was awaited: ignored")
            }
            SdDiagnosticKind::UnexpectedToken { token } => {
                write!(f, "token {token:#04x} does not fit the write in progress: ignored")
            }
            SdDiagnosticKind::AddressOutOfRange { address } => {
                write!(f, "address {address:#010x} lies past the end of the card: parameter error")
            }
            SdDiagnosticKind::MisalignedAddress { address, length } => write!(
                f,
                "{length} bytes at address {address:#010x} do not fit in one 512-byte block: address error"
            ),
            SdDiagnosticKind::BlockLength { length } => write!(
                f,
                "block length {length} is outside what the card takes: parameter error"
            ),
            SdDiagnosticKind::ReadPastEnd { block } => write!(
                f,
                "a multiple-block read reached block {block}, past the end: error token \"out of range\""
            ),
            SdDiagnosticKind::DataCrc {
                block,
                crc,
                expected,
            } => write!(
                f,
                "block {block} written with CRC {crc:#06x} instead of {expected:#06x}: rejected"
            ),
            SdDiagnosticKind::WriteProtected { block } => {
                write!(f, "write of block {block} to a write-protected card: refused")
            }
            SdDiagnosticKind::EraseProtected => {
                f.write_str("erase of a write-protected card: nothing erased")
            }
            SdDiagnosticKind::EraseSequence => f.write_str(
                "erase out of sequence (CMD38 without CMD32 and CMD33 before it): erase sequence error",
            ),
            SdDiagnosticKind::StorageFailed { block, detail } => {
                write!(f, "the storage failed at block {block}: {detail}")
            }
        }
    }
}

/// A diagnostic and the instant of device time it happened at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdDiagnostic {
    pub at: Cycles,
    pub kind: SdDiagnosticKind,
}

impl fmt::Display for SdDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SD card at cycle {}: {}", self.at.get(), self.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_diagnostic_names_what_happened_and_no_two_read_the_same() {
        let kinds = [
            SdDiagnosticKind::CommandInSdMode { index: 8 },
            SdDiagnosticKind::Cmd0CrcInSdMode { crc: 0x87 },
            SdDiagnosticKind::CommandCrc {
                index: 8,
                crc: 0x01,
                expected: 0x87,
            },
            SdDiagnosticKind::IllegalInIdle {
                index: 17,
                app: false,
            },
            SdDiagnosticKind::UnknownCommand {
                index: 63,
                app: false,
            },
            SdDiagnosticKind::UnsupportedCommand {
                index: 42,
                app: false,
                name: "LOCK_UNLOCK",
            },
            SdDiagnosticKind::Cmd1BeforeAcmd41,
            SdDiagnosticKind::HighCapacityWithoutHcs { cmd8: false },
            SdDiagnosticKind::HighCapacityWithoutHcs { cmd8: true },
            SdDiagnosticKind::CommandWhileBusy { index: 17 },
            SdDiagnosticKind::CommandDuringTransfer { index: 17 },
            SdDiagnosticKind::CommandCut,
            SdDiagnosticKind::TransferCut,
            SdDiagnosticKind::StrayByte { byte: 0x12 },
            SdDiagnosticKind::UnexpectedToken { token: 0xFE },
            SdDiagnosticKind::AddressOutOfRange { address: 0x100 },
            SdDiagnosticKind::MisalignedAddress {
                address: 0x100,
                length: 512,
            },
            SdDiagnosticKind::BlockLength { length: 1024 },
            SdDiagnosticKind::ReadPastEnd { block: 9 },
            SdDiagnosticKind::DataCrc {
                block: 1,
                crc: 0xFFFF,
                expected: 0x7FA1,
            },
            SdDiagnosticKind::WriteProtected { block: 3 },
            SdDiagnosticKind::EraseProtected,
            SdDiagnosticKind::EraseSequence,
            SdDiagnosticKind::StorageFailed {
                block: 4,
                detail: "Input/output error".into(),
            },
        ];
        let texts: Vec<String> = kinds.iter().map(ToString::to_string).collect();
        for (index, text) in texts.iter().enumerate() {
            assert_ne!(text, "");
            assert!(
                !texts[..index].contains(text),
                "two diagnostics read the same: {text}"
            );
        }
        assert_eq!(
            texts[5],
            "CMD42 (LOCK_UNLOCK) is not modelled: illegal command"
        );
        let diagnostic = SdDiagnostic {
            at: Cycles(16),
            kind: SdDiagnosticKind::CommandCut,
        };
        assert_eq!(
            diagnostic.to_string(),
            "SD card at cycle 16: the card was deselected in the middle of a command: it is lost"
        );
    }
}
