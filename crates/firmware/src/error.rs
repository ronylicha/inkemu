//! Every message states the cause and what the user can do about it.
//!
//! Remedy convention (checked by `every_variant_message_states_a_remedy`): each message
//! ends its cause with `"; "` followed by an imperative from [`REMEDY_VERBS`].
use crate::chip::ChipId;
use thiserror::Error;

/// Imperatives that open the remedy clause of a message (`"; <verb> ..."`).
pub const REMEDY_VERBS: [&str; 4] = ["select", "choose", "use", "re-download"];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FirmwareError {
    #[error(
        "the file is empty or shorter than an image header ({len} bytes); select an ESP-IDF application .bin"
    )]
    TooShort { len: usize },
    #[error(
        "this is an encrypted OTA update (.xota), which inkemu cannot read; use the plain application .bin of a release, not the encrypted OTA file"
    )]
    EncryptedOta,
    /// The same file given where a full flash image is expected: an application `.bin` is not
    /// accepted there either, so the remedy differs from [`FirmwareError::EncryptedOta`].
    #[error(
        "this is an encrypted OTA update (.xota), which inkemu cannot read and which is not a flash image; use a full flash dump (`esptool.py read_flash`), or an image merged with `esptool.py merge_bin`"
    )]
    EncryptedOtaAsFlash,
    #[error(
        "this is a full flash image (bootloader + partition table + application), not an application image; select the application .bin to inspect it, or boot the full image with `inkemu run --flash`"
    )]
    FlashImage,
    #[error(
        "not an ESP-IDF application image: first byte is {found:#04x}, expected 0xe9; select the application .bin from the build directory, not a merged flash image or another format"
    )]
    BadMagic { found: u8 },
    #[error(
        "image declares {count} segments, more than the {max} an ESP-IDF image can have; re-download the file or rebuild the firmware"
    )]
    TooManySegments { count: u8, max: u8 },
    #[error(
        "segment {index} header at file offset {offset} is cut short; re-download the file or rebuild the firmware"
    )]
    TruncatedSegmentHeader { index: usize, offset: usize },
    #[error(
        "segment {index} at file offset {offset} declares {declared} bytes but only {available} remain; re-download the file or rebuild the firmware"
    )]
    TruncatedSegment {
        index: usize,
        offset: usize,
        declared: u64,
        available: usize,
    },
    #[error(
        "segment {index} length {len} is not a multiple of 4; re-download the file or rebuild the firmware"
    )]
    MisalignedSegment { index: usize, len: u32 },
    #[error("unknown SPI flash mode {raw}; re-download the file or rebuild the firmware")]
    BadSpiMode { raw: u8 },
    #[error(
        "unknown flash size code {raw} (inkemu knows 0 to 7, that is 1 MB to 128 MB): the header is damaged, or the image targets a larger flash; re-download the file, or use an image built for a flash of at most 128 MB"
    )]
    BadFlashSize { raw: u8 },
    #[error(
        "the image ends before its checksum/hash trailer; re-download the file or rebuild the firmware"
    )]
    TruncatedTrailer,
    #[error(
        "checksum mismatch (stored {stored:#04x}, computed {computed:#04x}); re-download the file or rebuild the firmware"
    )]
    ChecksumMismatch { stored: u8, computed: u8 },
    #[error(
        "SHA-256 mismatch: the file is corrupted or was modified after signing; re-download the file or rebuild the firmware"
    )]
    HashMismatch,
    #[error(
        "this firmware is built for {found}, but the selected device uses {expected}; choose the matching device or firmware"
    )]
    ChipMismatch { expected: ChipId, found: ChipId },
    #[error(
        "this firmware expects {needed} MB of flash, but the selected device has {available} MB; choose a device with at least {needed} MB of flash, or rebuild the firmware for a {available} MB flash"
    )]
    FlashTooSmall { needed: u32, available: u32 },
    #[error(
        "the file does not start with a bootloader image: its first byte is {found:#04x}, expected 0xe9; use a full flash dump, or an image merged with `esptool.py merge_bin` (bootloader at offset 0)"
    )]
    NotAFlashImage { found: u8 },
    #[error(
        "the file is empty or shorter than an image header ({len} bytes); use a full flash dump, or an image merged with `esptool.py merge_bin`"
    )]
    FlashImageTooShort { len: usize },
    #[error(
        "the file starts with an image but holds no partition table at offset 0x8000, so it is an application or a bootloader alone, not a full flash image; use an application with --firmware, or give --flash a flash dump or the output of `esptool.py merge_bin` (bootloader, partition table and application)"
    )]
    NoPartitionTable,
}

impl FirmwareError {
    /// The cause the message states, without its remedy: for a message about another role, whose
    /// remedy differs (the remedies of these messages speak of an application).
    #[must_use]
    pub fn cause(&self) -> String {
        let message = self.to_string();
        match message.split_once("; ") {
            Some((cause, _)) => cause.to_owned(),
            None => message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::ChipId;

    #[test]
    fn the_cause_of_every_message_is_what_comes_before_its_one_remedy() {
        for err in one_of_each() {
            let message = err.to_string();
            assert_eq!(message.matches("; ").count(), 1, "{message}");
            let cause = err.cause();
            assert!(
                message.starts_with(&format!("{cause}; ")) && !cause.is_empty(),
                "{message}"
            );
        }
    }

    /// One instance of every variant. The exhaustive `match` in `variant_index` makes
    /// adding a variant without adding it here a compile error.
    fn one_of_each() -> Vec<FirmwareError> {
        let all = vec![
            FirmwareError::TooShort { len: 0 },
            FirmwareError::EncryptedOta,
            FirmwareError::EncryptedOtaAsFlash,
            FirmwareError::FlashImage,
            FirmwareError::BadMagic { found: 0 },
            FirmwareError::TooManySegments { count: 17, max: 16 },
            FirmwareError::TruncatedSegmentHeader {
                index: 1,
                offset: 288,
            },
            FirmwareError::TruncatedSegment {
                index: 0,
                offset: 24,
                declared: 256,
                available: 8,
            },
            FirmwareError::MisalignedSegment { index: 0, len: 3 },
            FirmwareError::BadSpiMode { raw: 9 },
            FirmwareError::BadFlashSize { raw: 8 },
            FirmwareError::TruncatedTrailer,
            FirmwareError::ChecksumMismatch {
                stored: 1,
                computed: 2,
            },
            FirmwareError::HashMismatch,
            FirmwareError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3,
            },
            FirmwareError::FlashTooSmall {
                needed: 16,
                available: 4,
            },
            FirmwareError::NotAFlashImage { found: 0x7F },
            FirmwareError::FlashImageTooShort { len: 10 },
            FirmwareError::NoPartitionTable,
        ];
        let mut seen = [false; 19];
        for e in &all {
            seen[variant_index(e)] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "one_of_each must cover every variant"
        );
        all
    }

    fn variant_index(e: &FirmwareError) -> usize {
        match e {
            FirmwareError::TooShort { .. } => 0,
            FirmwareError::EncryptedOta => 1,
            FirmwareError::FlashImage => 2,
            FirmwareError::BadMagic { .. } => 3,
            FirmwareError::TooManySegments { .. } => 4,
            FirmwareError::TruncatedSegmentHeader { .. } => 5,
            FirmwareError::TruncatedSegment { .. } => 6,
            FirmwareError::MisalignedSegment { .. } => 7,
            FirmwareError::BadSpiMode { .. } => 8,
            FirmwareError::BadFlashSize { .. } => 9,
            FirmwareError::TruncatedTrailer => 10,
            FirmwareError::ChecksumMismatch { .. } => 11,
            FirmwareError::HashMismatch => 12,
            FirmwareError::ChipMismatch { .. } => 13,
            FirmwareError::FlashTooSmall { .. } => 14,
            FirmwareError::NotAFlashImage { .. } => 15,
            FirmwareError::FlashImageTooShort { .. } => 16,
            FirmwareError::NoPartitionTable => 17,
            FirmwareError::EncryptedOtaAsFlash => 18,
        }
    }

    #[test]
    fn an_unknown_flash_size_code_says_what_is_known_and_what_to_do() {
        let text = FirmwareError::BadFlashSize { raw: 9 }.to_string();
        assert!(
            text.contains("code 9") && text.contains("0 to 7") && text.contains("128 MB"),
            "{text}"
        );
        assert!(
            text.contains(
                "; re-download the file, or use an image built for a flash of at most 128 MB"
            ),
            "{text}"
        );
    }

    #[test]
    fn every_variant_message_states_a_remedy() {
        for err in one_of_each() {
            let msg = err.to_string();
            let has_remedy = REMEDY_VERBS
                .iter()
                .any(|verb| msg.contains(&format!("; {verb} ")));
            assert!(
                has_remedy,
                "no remedy clause (\"; <{REMEDY_VERBS:?}> ...\") in: {msg}"
            );
        }
    }
}
