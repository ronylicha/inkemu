//! Builds valid ESP-IDF application images for tests (feature `test-support`).
// Segment counts and lengths are bounded by the caller's test fixtures (far below
// u8/u32::MAX), so the narrowing casts cannot truncate in practice.
#![allow(clippy::cast_possible_truncation)]
use crate::app_desc::APP_DESC_MAGIC;
use crate::image::{CHECKSUM_SEED, IMAGE_MAGIC, TRAILER_ALIGN};
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
