//! Builds valid ESP-IDF application images for tests (feature `test-support`).
// Segment counts and lengths are bounded by the caller's test fixtures (far below
// u8/u32::MAX), so the narrowing casts cannot truncate in practice.
#![allow(clippy::cast_possible_truncation)]
use crate::app_desc::APP_DESC_MAGIC;
use crate::chip::FlashSize;
use crate::image::{CHECKSUM_SEED, IMAGE_MAGIC, TRAILER_ALIGN};
use crate::partition::{PARTITION_TABLE_OFFSET, xteink_x4_stock};
use sha2::{Digest, Sha256};

/// Write-protect pin value meaning "disabled" in ESP-IDF headers.
const WP_PIN_DISABLED: u8 = 0xEE;
/// Flash frequency nibble for 80 MHz in ESP-IDF headers.
const FLASH_FREQ_80M: u8 = 0xF;

#[derive(Debug, Clone)]
pub struct ImageSpec {
    pub chip: u16,
    pub entry: u32,
    pub flash_size_nibble: u8,
    pub spi_mode: u8,
    pub segments: Vec<(u32, Vec<u8>)>,
    pub hash_appended: bool,
}

impl Default for ImageSpec {
    fn default() -> Self {
        ImageSpec {
            chip: 5,
            entry: 0x4038_086E,
            flash_size_nibble: 4,
            spi_mode: 2,
            segments: vec![
                (0x3C00_0020, app_desc_bytes("1.0.0", "demo", "v5.5.2")),
                (0x4038_0000, vec![0x13, 0, 0, 0]),
            ],
            hash_appended: true,
        }
    }
}

#[must_use]
pub fn build_image(spec: &ImageSpec) -> Vec<u8> {
    let mut out = vec![
        IMAGE_MAGIC,
        spec.segments.len() as u8,
        spec.spi_mode,
        (spec.flash_size_nibble << 4) | FLASH_FREQ_80M,
    ];
    out.extend(spec.entry.to_le_bytes());
    out.extend([WP_PIN_DISABLED, 0, 0, 0]);
    out.extend(spec.chip.to_le_bytes());
    out.push(0); // min_chip_rev (legacy)
    out.extend(0u16.to_le_bytes()); // min_chip_rev_full
    out.extend(0xFFFFu16.to_le_bytes()); // max_chip_rev_full
    out.extend([0u8; 4]); // reserved
    out.push(u8::from(spec.hash_appended));
    let mut checksum = CHECKSUM_SEED;
    for (addr, data) in &spec.segments {
        out.extend(addr.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        checksum = data.iter().fold(checksum, |c, b| c ^ b);
    }
    while out.len() % TRAILER_ALIGN != TRAILER_ALIGN - 1 {
        out.push(0);
    }
    out.push(checksum);
    if spec.hash_appended {
        let digest = Sha256::digest(&out);
        out.extend(digest);
    }
    out
}

/// Recomputes the SHA-256 appended to `image`, over every byte before it: a test that changes a
/// header byte (the digest covers the header) keeps the image valid with this.
///
/// # Panics
/// When `image` is shorter than a digest.
pub fn refresh_digest(image: &mut [u8]) {
    let hashed = image.len() - 32;
    let digest = Sha256::digest(&image[..hashed]);
    image[hashed..].copy_from_slice(&digest);
}

/// An erased flash byte, which fills the gap between the image and the partition table.
const ERASED: u8 = 0xFF;

/// A full flash image: the image of `spec` at offset 0, where the bootloader goes, erased bytes up
/// to the partition table at `0x8000`, then the stock Xteink X4 table for a 16 MB flash. The result
/// is `0x8C00` bytes long; what a real flash holds after the table reads erased.
///
/// # Panics
/// When the image of `spec` does not fit below the partition table.
#[must_use]
pub fn build_flash_image(spec: &ImageSpec) -> Vec<u8> {
    let table_offset = PARTITION_TABLE_OFFSET as usize;
    let mut flash = build_image(spec);
    assert!(
        flash.len() <= table_offset,
        "the image is {} bytes and would overlap the partition table at {table_offset:#x}; use smaller segments",
        flash.len()
    );
    flash.resize(table_offset, ERASED);
    let Ok(table) = xteink_x4_stock().to_bytes(FlashSize::Mb16) else {
        // INVARIANT: the stock table is a fixed layout that is valid for a 16 MB flash.
        unreachable!("the stock partition table is valid for a 16 MB flash")
    };
    flash.extend(table);
    flash
}

/// A 256-byte `esp_app_desc_t` with the given strings (truncated to their field sizes).
#[must_use]
pub fn app_desc_bytes(version: &str, project: &str, idf: &str) -> Vec<u8> {
    let mut d = vec![0u8; 256];
    d[0..4].copy_from_slice(&APP_DESC_MAGIC.to_le_bytes());
    let put = |d: &mut Vec<u8>, at: usize, len: usize, s: &str| {
        let b = s.as_bytes();
        let n = b.len().min(len - 1);
        d[at..at + n].copy_from_slice(&b[..n]);
    };
    put(&mut d, 16, 32, version);
    put(&mut d, 48, 32, project);
    put(&mut d, 80, 16, "12:00:00");
    put(&mut d, 96, 16, "Oct 11 2026");
    put(&mut d, 112, 32, idf);
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{PARTITION_TABLE_LEN, PartitionTable};

    #[test]
    fn a_flash_image_is_the_image_then_erased_bytes_then_the_stock_table() {
        let spec = ImageSpec::default();
        let image = build_image(&spec);
        let flash = build_flash_image(&spec);
        assert_eq!(flash.len(), 0x8000 + PARTITION_TABLE_LEN);
        assert_eq!(flash[..image.len()], image[..], "the image is at offset 0");
        assert!(
            flash[image.len()..0x8000].iter().all(|&byte| byte == 0xFF),
            "erased up to the table"
        );
        assert_eq!(
            PartitionTable::parse(&flash[0x8000..]),
            Ok(xteink_x4_stock())
        );
    }

    #[test]
    fn a_header_byte_changed_and_the_digest_refreshed_is_a_valid_image() {
        use crate::image::FirmwareImage;
        let mut image = build_image(&ImageSpec::default());
        image[15..17].copy_from_slice(&4u16.to_le_bytes());
        assert!(
            FirmwareImage::parse(&image).is_err(),
            "the digest covers the header"
        );
        refresh_digest(&mut image);
        assert_eq!(FirmwareImage::parse(&image).unwrap().min_chip_rev_full, 4);
    }

    #[test]
    #[should_panic(expected = "would overlap the partition table")]
    fn an_image_that_reaches_the_partition_table_is_refused() {
        let spec = ImageSpec {
            segments: vec![(0x4038_0000, vec![0; 0x8000])],
            ..ImageSpec::default()
        };
        let _ = build_flash_image(&spec);
    }
}
