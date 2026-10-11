//! ESP-IDF application image parsing and validation (spec §5.1).
use crate::app_desc::AppDesc;
use crate::chip::{ChipId, FlashSize, SpiMode};
use crate::error::FirmwareError;
use sha2::{Digest, Sha256};

/// Header byte 0 of every ESP-IDF application image.
pub const IMAGE_MAGIC: u8 = 0xE9;
/// Prefix of an encrypted OTA update, which is not a plain application image.
pub(crate) const XOTA_MAGIC: &[u8; 4] = b"XOTA";
/// Common header (8 bytes) followed by the extended header (16 bytes).
pub const HEADER_LEN: usize = 24;
/// Segment header: `load_addr` (u32) then `data_len` (u32).
const SEGMENT_HEADER_LEN: usize = 8;
/// `ESP_IMAGE_MAX_SEGMENTS` in ESP-IDF.
pub const MAX_SEGMENTS: u8 = 16;
/// Seed of the XOR checksum over all segment data (ESP-IDF `ESP_ROM_CHECKSUM_INITIAL`).
pub const CHECKSUM_SEED: u8 = 0xEF;
/// The checksum byte is the last byte of a 16-byte block (`position % 16 == 15`).
pub const TRAILER_ALIGN: usize = 16;
const SHA256_LEN: usize = 32;
/// Segment data is written in whole 32-bit words.
const SEGMENT_ALIGN: u32 = 4;
/// Header byte 23: non-zero when a SHA-256 digest follows the checksum.
const HASH_APPENDED_OFFSET: usize = 23;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub load_addr: u32,
    pub data: Vec<u8>,
    pub file_offset: usize,
}

/// A validated ESP-IDF application image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareImage {
    pub chip: ChipId,
    pub entry: u32,
    pub spi_mode: SpiMode,
    pub flash_size: FlashSize,
    pub flash_freq_raw: u8,
    pub min_chip_rev_full: u16,
    pub max_chip_rev_full: u16,
    pub segments: Vec<Segment>,
    pub hash_appended: bool,
    pub app_desc: Option<AppDesc>,
}

pub(crate) fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

pub(crate) fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl FirmwareImage {
    /// Parses and fully verifies an image: header, every segment, checksum and hash.
    ///
    /// # Errors
    /// Any [`FirmwareError`] describing why the file is not a usable application image.
    pub fn parse(bytes: &[u8]) -> Result<FirmwareImage, FirmwareError> {
        if bytes.starts_with(XOTA_MAGIC) {
            return Err(FirmwareError::EncryptedOta);
        }
        // A flash dump or merged image holds a partition table at 0x8000: the bytes at offset 0 are
        // then the bootloader, which parses as a valid-looking image and must not be mistaken for
        // the application.
        if crate::flash_image::has_partition_table(bytes) {
            return Err(FirmwareError::FlashImage);
        }
        if bytes.len() < HEADER_LEN {
            return Err(FirmwareError::TooShort { len: bytes.len() });
        }
        if bytes[0] != IMAGE_MAGIC {
            return Err(FirmwareError::BadMagic { found: bytes[0] });
        }
        let count = bytes[1];
        if count > MAX_SEGMENTS {
            return Err(FirmwareError::TooManySegments {
                count,
                max: MAX_SEGMENTS,
            });
        }
        let spi_mode =
            SpiMode::from_raw(bytes[2]).ok_or(FirmwareError::BadSpiMode { raw: bytes[2] })?;
        let size_code = bytes[3] >> 4;
        let flash_size = FlashSize::from_nibble(size_code)
            .ok_or(FirmwareError::BadFlashSize { raw: size_code })?;
        let (segments, checksum, end) = Self::read_segments(bytes, usize::from(count))?;
        let hash_appended = bytes[HASH_APPENDED_OFFSET] != 0;
        Self::verify_trailer(bytes, end, checksum, hash_appended)?;
        let app_desc = segments.first().and_then(|s| AppDesc::parse(&s.data));
        Ok(FirmwareImage {
            chip: ChipId::from_raw(le_u16(bytes, 12)),
            entry: le_u32(bytes, 4),
            spi_mode,
            flash_size,
            flash_freq_raw: bytes[3] & 0x0F,
            min_chip_rev_full: le_u16(bytes, 15),
            max_chip_rev_full: le_u16(bytes, 17),
            segments,
            hash_appended,
            app_desc,
        })
    }

    /// Reads `count` segments; returns them, the XOR checksum of their data, and the
    /// offset just past the last segment.
    #[allow(clippy::cast_possible_truncation)] // INVARIANT: `len` is checked against `available` (a usize) before the cast.
    fn read_segments(
        bytes: &[u8],
        count: usize,
    ) -> Result<(Vec<Segment>, u8, usize), FirmwareError> {
        let mut segments = Vec::with_capacity(count);
        let mut checksum = CHECKSUM_SEED;
        let mut pos = HEADER_LEN;
        for index in 0..count {
            if bytes.len() < pos + SEGMENT_HEADER_LEN {
                return Err(FirmwareError::TruncatedSegmentHeader { index, offset: pos });
            }
            let available = bytes.len() - pos - SEGMENT_HEADER_LEN;
            let load_addr = le_u32(bytes, pos);
            let len = le_u32(bytes, pos + 4);
            if u64::from(len) > available as u64 {
                return Err(FirmwareError::TruncatedSegment {
                    index,
                    offset: pos,
                    declared: u64::from(len),
                    available,
                });
            }
            if !len.is_multiple_of(SEGMENT_ALIGN) {
                return Err(FirmwareError::MisalignedSegment { index, len });
            }
            let start = pos + SEGMENT_HEADER_LEN;
            let data = bytes[start..start + len as usize].to_vec();
            checksum = data.iter().fold(checksum, |c, b| c ^ b);
            segments.push(Segment {
                load_addr,
                data,
                file_offset: start,
            });
            pos = start + len as usize;
        }
        Ok((segments, checksum, pos))
    }

    /// Checks the checksum byte (at the next `position % 16 == 15`) and, when announced,
    /// the SHA-256 digest that follows it.
    fn verify_trailer(
        bytes: &[u8],
        end: usize,
        checksum: u8,
        hash_appended: bool,
    ) -> Result<(), FirmwareError> {
        let checksum_pos = end + (TRAILER_ALIGN - 1 - end % TRAILER_ALIGN);
        let stored = *bytes
            .get(checksum_pos)
            .ok_or(FirmwareError::TruncatedTrailer)?;
        if stored != checksum {
            return Err(FirmwareError::ChecksumMismatch {
                stored,
                computed: checksum,
            });
        }
        if hash_appended {
            let hashed_len = checksum_pos + 1;
            let stored_hash = bytes
                .get(hashed_len..hashed_len + SHA256_LEN)
                .ok_or(FirmwareError::TruncatedTrailer)?;
            if Sha256::digest(&bytes[..hashed_len]).as_slice() != stored_hash {
                return Err(FirmwareError::HashMismatch);
            }
        }
        Ok(())
    }

    /// # Errors
    /// [`FirmwareError::ChipMismatch`] when the image targets another `SoC`.
    pub fn require_chip(&self, expected: ChipId) -> Result<(), FirmwareError> {
        if self.chip == expected {
            Ok(())
        } else {
            Err(FirmwareError::ChipMismatch {
                expected,
                found: self.chip,
            })
        }
    }

    /// # Errors
    /// [`FirmwareError::FlashTooSmall`] when the image header requires more flash than the device has.
    pub fn require_flash(&self, available: FlashSize) -> Result<(), FirmwareError> {
        if self.flash_size <= available {
            Ok(())
        } else {
            Err(FirmwareError::FlashTooSmall {
                needed: self.flash_size.megabytes(),
                available: available.megabytes(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{ImageSpec, build_flash_image, build_image};

    fn with<F: FnOnce(&mut Vec<u8>)>(f: F) -> Vec<u8> {
        let mut b = build_image(&ImageSpec::default());
        f(&mut b);
        b
    }

    #[test]
    fn parses_valid_image() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec::default())).unwrap();
        assert_eq!(img.chip, ChipId::Esp32C3);
        assert_eq!(img.entry, 0x4038_086E);
        assert_eq!(
            (img.spi_mode, img.flash_size),
            (SpiMode::Dio, FlashSize::Mb16)
        );
        assert_eq!(img.segments.len(), 2);
        assert_eq!(img.segments[1].load_addr, 0x4038_0000);
        let desc = img.app_desc.unwrap();
        assert_eq!(
            (
                desc.version.as_str(),
                desc.project_name.as_str(),
                desc.idf_ver.as_str()
            ),
            ("1.0.0", "demo", "v5.5.2")
        );
    }

    #[test]
    fn accepts_image_without_hash() {
        let spec = ImageSpec {
            hash_appended: false,
            ..ImageSpec::default()
        };
        assert!(FirmwareImage::parse(&build_image(&spec)).is_ok());
    }

    #[test]
    fn rejects_empty_and_short_files() {
        assert_eq!(
            FirmwareImage::parse(&[]),
            Err(FirmwareError::TooShort { len: 0 })
        );
        assert_eq!(
            FirmwareImage::parse(&[0xE9; 10]),
            Err(FirmwareError::TooShort { len: 10 })
        );
    }

    #[test]
    fn rejects_encrypted_xota() {
        let mut b = b"XOTA".to_vec();
        b.resize(512, 0);
        let err = FirmwareImage::parse(&b).unwrap_err();
        assert_eq!(err, FirmwareError::EncryptedOta);
        let msg = err.to_string();
        assert!(msg.contains("encrypted"), "{msg}");
        assert!(msg.contains("plain application .bin"), "{msg}");
        assert!(!msg.contains("flash dump"), "{msg}");
    }

    /// A bootloader-first flash image: here the application image stands in for the bootloader
    /// at offset 0, padded with erased flash up to the partition table at 0x8000.
    fn flash_image(len: usize) -> Vec<u8> {
        let mut b = build_flash_image(&ImageSpec::default());
        b.resize(len, 0xFF);
        b
    }

    #[test]
    fn refuses_a_full_flash_image_instead_of_describing_its_bootloader() {
        let err = FirmwareImage::parse(&flash_image(0x9000)).unwrap_err();
        assert_eq!(err, FirmwareError::FlashImage);
        let msg = err.to_string();
        assert!(msg.contains("full flash image"), "{msg}");
        assert!(msg.contains("; select the application .bin"), "{msg}");
    }

    #[test]
    fn flash_image_detection_needs_a_table_not_a_file_length() {
        assert_eq!(
            FirmwareImage::parse(&flash_image(0x8C00)),
            Err(FirmwareError::FlashImage),
            "a dump cut in the table sector"
        );
        let mut coincidence = build_image(&ImageSpec::default());
        let len = coincidence.len();
        coincidence.resize(0x9000, 0);
        coincidence[0x8000] = 0xAA;
        coincidence[0x8001] = 0x50;
        // Not a table: the file is parsed as the application it starts with (its trailing bytes are ignored).
        assert!(
            FirmwareImage::parse(&coincidence).is_ok(),
            "image of {len} bytes with AA 50 at 0x8000"
        );
    }

    #[test]
    fn a_large_application_image_without_a_partition_table_magic_is_unaffected() {
        let mut b = flash_image(0x9000);
        b[0x8000] = 0xFF;
        b[0x8001] = 0xFF;
        assert!(FirmwareImage::parse(&b).is_ok());
    }

    #[test]
    fn rejects_bad_magic() {
        let err = FirmwareImage::parse(&with(|b| b[0] = 0x00)).unwrap_err();
        assert_eq!(err, FirmwareError::BadMagic { found: 0 });
        let msg = err.to_string();
        assert!(msg.contains("0xe9") && msg.contains("0x00"), "{msg}");
    }

    #[test]
    fn inflated_segment_count_is_refused_cleanly() {
        // Count 16 is the legal maximum, but the file holds only 2 segments: the parser
        // must refuse at the first segment that does not fit, never panic or over-read.
        // The third segment header sits at 300 and declares a length read from the hash.
        assert!(matches!(
            FirmwareImage::parse(&with(|b| b[1] = 16)),
            Err(FirmwareError::TruncatedSegment {
                index: 2,
                offset: 300,
                ..
            })
        ));
    }

    #[test]
    fn rejects_too_many_segments() {
        assert_eq!(
            FirmwareImage::parse(&with(|b| b[1] = 200)),
            Err(FirmwareError::TooManySegments {
                count: 200,
                max: 16
            })
        );
    }

    #[test]
    fn rejects_unknown_spi_mode() {
        assert_eq!(
            FirmwareImage::parse(&with(|b| b[2] = 9)),
            Err(FirmwareError::BadSpiMode { raw: 9 })
        );
    }

    #[test]
    fn rejects_unknown_flash_size() {
        // High nibble 8 has no defined flash size (codes are 0..=7).
        assert_eq!(
            FirmwareImage::parse(&with(|b| b[3] = 0x8F)),
            Err(FirmwareError::BadFlashSize { raw: 8 })
        );
    }

    #[test]
    fn rejects_truncated_segment() {
        let full = build_image(&ImageSpec::default());
        let cut = &full[..40]; // inside the first segment's data
        assert!(matches!(
            FirmwareImage::parse(cut),
            Err(FirmwareError::TruncatedSegment {
                index: 0,
                offset: 24,
                ..
            })
        ));
    }

    #[test]
    fn rejects_segment_header_cut_short() {
        // Second segment header starts at 288 and needs 8 bytes; cut after 4 of them.
        let full = build_image(&ImageSpec::default());
        assert_eq!(
            FirmwareImage::parse(&full[..292]),
            Err(FirmwareError::TruncatedSegmentHeader {
                index: 1,
                offset: 288
            })
        );
    }

    #[test]
    fn rejects_absurd_segment_length() {
        // data_len of segment 0 lives at bytes 28..32
        let b = with(|b| b[28..32].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes()));
        assert!(matches!(
            FirmwareImage::parse(&b),
            Err(FirmwareError::TruncatedSegment {
                index: 0,
                declared: 0xFFFF_FFF0,
                ..
            })
        ));
    }

    #[test]
    fn rejects_misaligned_segment_length() {
        let spec = ImageSpec {
            segments: vec![(0x4038_0000, vec![1, 2, 3])],
            ..ImageSpec::default()
        };
        assert_eq!(
            FirmwareImage::parse(&build_image(&spec)),
            Err(FirmwareError::MisalignedSegment { index: 0, len: 3 })
        );
    }

    #[test]
    fn rejects_bad_checksum_and_bad_hash() {
        let spec = ImageSpec {
            hash_appended: false,
            ..ImageSpec::default()
        };
        let mut b = build_image(&spec);
        let last = b.len() - 1;
        b[last] ^= 0xFF;
        assert!(matches!(
            FirmwareImage::parse(&b),
            Err(FirmwareError::ChecksumMismatch { .. })
        ));
        let mut b = build_image(&ImageSpec::default());
        let last = b.len() - 1;
        b[last] ^= 0xFF; // flips a byte of the SHA-256
        assert_eq!(FirmwareImage::parse(&b), Err(FirmwareError::HashMismatch));
    }

    #[test]
    fn rejects_truncated_trailer() {
        let full = build_image(&ImageSpec::default());
        assert_eq!(
            FirmwareImage::parse(&full[..full.len() - 5]),
            Err(FirmwareError::TruncatedTrailer)
        );
    }

    #[test]
    fn rejects_image_cut_before_checksum() {
        let spec = ImageSpec {
            hash_appended: false,
            ..ImageSpec::default()
        };
        let full = build_image(&spec);
        assert_eq!(
            FirmwareImage::parse(&full[..full.len() - 1]),
            Err(FirmwareError::TruncatedTrailer)
        );
    }

    #[test]
    fn unknown_chip_is_kept_and_reported() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec {
            chip: 99,
            ..ImageSpec::default()
        }))
        .unwrap();
        assert_eq!(img.chip, ChipId::Unknown(99));
    }

    #[test]
    fn chip_mismatch_names_both_chips() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec {
            chip: 9,
            ..ImageSpec::default()
        }))
        .unwrap();
        let err = img.require_chip(ChipId::Esp32C3).unwrap_err();
        assert_eq!(
            err,
            FirmwareError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3
            }
        );
        let msg = err.to_string();
        assert!(
            msg.contains("ESP32-S3") && msg.contains("ESP32-C3"),
            "{msg}"
        );
    }

    #[test]
    fn matching_chip_is_accepted() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec::default())).unwrap();
        assert_eq!(img.require_chip(ChipId::Esp32C3), Ok(()));
    }

    #[test]
    fn flash_size_must_fit_device() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec::default())).unwrap();
        let err = img.require_flash(FlashSize::Mb4).unwrap_err();
        assert_eq!(
            err,
            FirmwareError::FlashTooSmall {
                needed: 16,
                available: 4
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("16 MB") && msg.contains("4 MB"), "{msg}");
        assert!(msg.contains("choose a device"), "{msg}");
        assert_eq!(img.require_flash(FlashSize::Mb16), Ok(()));
    }

    #[test]
    fn image_without_app_desc_parses_with_none() {
        let spec = ImageSpec {
            segments: vec![(0x4038_0000, vec![0; 8])],
            ..ImageSpec::default()
        };
        assert!(
            FirmwareImage::parse(&build_image(&spec))
                .unwrap()
                .app_desc
                .is_none()
        );
    }
}
