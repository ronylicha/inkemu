//! Every message states the cause and what the user can do about it.
use crate::chip::ChipId;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FirmwareError {
    #[error(
        "the file is empty or shorter than an image header ({len} bytes); select an ESP-IDF application .bin"
    )]
    TooShort { len: usize },
    #[error(
        "this is an encrypted OTA update (.xota); inkemu needs the plain application image — use a flash dump or a plain .bin release"
    )]
    EncryptedOta,
    #[error(
        "not an ESP-IDF application image: first byte is {found:#04x}, expected 0xe9; a merged flash image or another format was probably selected"
    )]
    BadMagic { found: u8 },
    #[error(
        "image declares {count} segments, more than the {max} an ESP-IDF image can have; the file is corrupted"
    )]
    TooManySegments { count: u8, max: u8 },
    #[error(
        "segment {index} header at file offset {offset} is cut short; the file is truncated or corrupted"
    )]
    TruncatedSegmentHeader { index: usize, offset: usize },
    #[error(
        "segment {index} at file offset {offset} declares {declared} bytes but only {available} remain; the file is truncated or corrupted"
    )]
    TruncatedSegment {
        index: usize,
        offset: usize,
        declared: u64,
        available: usize,
    },
    #[error("segment {index} length {len} is not a multiple of 4; the file is corrupted")]
    MisalignedSegment { index: usize, len: u32 },
    #[error("unknown SPI flash mode {raw}; the header is corrupted")]
    BadSpiMode { raw: u8 },
    #[error("unknown flash size code {raw}; the header is corrupted")]
    BadFlashSize { raw: u8 },
    #[error("the image ends before its checksum/hash trailer; the file is truncated")]
    TruncatedTrailer,
    #[error(
        "checksum mismatch (stored {stored:#04x}, computed {computed:#04x}); the file is corrupted"
    )]
    ChecksumMismatch { stored: u8, computed: u8 },
    #[error("SHA-256 mismatch; the file is corrupted or was modified after signing")]
    HashMismatch,
    #[error(
        "this firmware is built for {found}, but the selected device uses {expected}; choose the matching device or firmware"
    )]
    ChipMismatch { expected: ChipId, found: ChipId },
    #[error(
        "this firmware expects {needed} MB of flash, but the selected device has {available} MB"
    )]
    FlashTooSmall { needed: u32, available: u32 },
}
