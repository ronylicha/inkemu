//! The 2nd-stage bootloader: the descriptor it carries, and the ones inkemu ships.
//!
//! inkemu ships one bootloader per chip family and flash-mode family, built from unmodified
//! ESP-IDF sources (`third_party/esp-idf-bootloader/`, ADR 0005). A device profile selects one by
//! what it states, its chip family; the size of its flash is written into the header when the
//! flash is composed (`docs/research/shipped-bootloader-and-firmware-composition.md`, **SB** in
//! the comments of this crate, §B).
use crate::chip::{ChipId, FlashSize, SpiMode};
use sha2::{Digest, Sha256};

/// `ESP_BOOTLOADER_DESC_MAGIC_BYTE` (`esp_bootloader_desc.h`): the first byte of
/// `esp_bootloader_desc_t`, which starts the first segment of a bootloader built by ESP-IDF.
pub const BOOTLOADER_DESC_MAGIC: u8 = 80;
/// Size of `esp_bootloader_desc_t`.
const DESC_LEN: usize = 80;
const DESC_VERSION: usize = 4;
/// String fields are `(offset, field size)`, NUL-terminated within the field.
const DESC_IDF_VER: (usize, usize) = (8, 32);
const DESC_DATE_TIME: (usize, usize) = (40, 24);
/// Header byte 3: flash size in the high nibble, flash frequency in the low one.
const SIZE_FREQ_OFFSET: usize = 3;
/// Header byte 23: non-zero when a SHA-256 digest ends the image.
const HASH_APPENDED_OFFSET: usize = 23;
const SHA256_LEN: usize = 32;

/// What `esp_bootloader_desc_t` says of the build of a bootloader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootloaderDesc {
    pub version: u32,
    /// The ESP-IDF version, as `git describe` gave it: `v5.5.2`, `v5.5.2-729-g87912cd291`.
    pub idf_ver: String,
    /// Empty for a reproducible build.
    pub date_time: String,
}

fn c_string(bytes: &[u8], (at, len): (usize, usize)) -> String {
    let field = &bytes[at..at + len];
    let end = field.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

impl BootloaderDesc {
    /// Returns `None` when the segment does not start with a bootloader descriptor.
    #[must_use]
    pub fn parse(segment: &[u8]) -> Option<BootloaderDesc> {
        if segment.len() < DESC_LEN || segment[0] != BOOTLOADER_DESC_MAGIC {
            return None;
        }
        let version = u32::from_le_bytes([
            segment[DESC_VERSION],
            segment[DESC_VERSION + 1],
            segment[DESC_VERSION + 2],
            segment[DESC_VERSION + 3],
        ]);
        Some(BootloaderDesc {
            version,
            idf_ver: c_string(segment, DESC_IDF_VER),
            date_time: c_string(segment, DESC_DATE_TIME),
        })
    }
}

/// The SHA-256 of `bytes`, in lower-case hexadecimal.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(2 * SHA256_LEN), |mut text, byte| {
            text.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
            text.push(char::from(b"0123456789abcdef"[usize::from(byte & 0xF)]));
            text
        })
}

/// A bootloader inkemu carries in its binary. Its fields are what the image itself says (a test
/// holds them to the bytes); they are here so that a profile, or a front end, can select and show
/// one without reading its header.
#[derive(Debug, PartialEq, Eq)]
pub struct ShippedBootloader {
    /// A stable name: what a summary prints and a front end can store.
    pub id: &'static str,
    pub chip: ChipId,
    /// The flash mode its code drives: one binary per flash-mode family (a QIO build is other
    /// code, whose header still says DIO: SB §B).
    pub flash_mode: SpiMode,
    /// The ESP-IDF release it was built from.
    pub idf_version: &'static str,
    /// The lowest chip revision it starts on, `major * 100 + minor` (header `min_chip_rev_full`).
    /// Below it the bootloader resets the chip for ever (SB §B).
    pub min_chip_revision: u16,
    /// SHA-256 of `bytes`, in lower-case hexadecimal.
    pub sha256: &'static str,
    pub bytes: &'static [u8],
}

/// Every bootloader inkemu ships. Adding a chip family is one entry here and its directory in
/// `third_party/esp-idf-bootloader/`; a device with a chip already here needs its profile only.
pub static SHIPPED: [ShippedBootloader; 1] = [ShippedBootloader {
    id: "esp-idf-v5.5.2-esp32c3-dio",
    chip: ChipId::Esp32C3,
    flash_mode: SpiMode::Dio,
    idf_version: "v5.5.2",
    min_chip_revision: 3,
    sha256: "601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3",
    bytes: include_bytes!("../../../third_party/esp-idf-bootloader/esp32c3/bootloader.bin"),
}];

/// The bootloader inkemu ships for `chip` and the flash-mode family of `flash_mode`, if any.
#[must_use]
pub fn shipped(chip: ChipId, flash_mode: SpiMode) -> Option<&'static ShippedBootloader> {
    SHIPPED
        .iter()
        .find(|entry| entry.chip == chip && entry.flash_mode == flash_mode)
}

impl ShippedBootloader {
    /// The bytes to write at offset 0 of a flash of `size`: the shipped image with the flash size
    /// of its header set to `size` and its appended digest computed again, as `esptool.py` does
    /// to any image written at offset 0 (SB §B, `esptool/cmds.py:263-345`). The bootloader hands
    /// that size to the application, which aborts when the chip it finds is smaller. An image
    /// whose header already says `size` comes back as it is.
    #[must_use]
    pub fn for_flash(&self, size: FlashSize) -> Vec<u8> {
        let mut image = self.bytes.to_vec();
        let header = image[SIZE_FREQ_OFFSET];
        let wanted = (size.nibble() << 4) | (header & 0x0F);
        if header == wanted {
            return image;
        }
        image[SIZE_FREQ_OFFSET] = wanted;
        if image[HASH_APPENDED_OFFSET] != 0 {
            // INVARIANT: a shipped image ends with its digest (a test holds every entry to it).
            let hashed = image.len() - SHA256_LEN;
            let digest = Sha256::digest(&image[..hashed]);
            image[hashed..].copy_from_slice(&digest);
        }
        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::FirmwareImage;

    #[test]
    fn every_shipped_bootloader_is_what_its_entry_says() {
        for entry in &SHIPPED {
            assert_eq!(sha256_hex(entry.bytes), entry.sha256, "{}", entry.id);
            let image = FirmwareImage::parse(entry.bytes).unwrap();
            assert_eq!(image.chip, entry.chip, "{}", entry.id);
            assert_eq!(image.spi_mode, entry.flash_mode, "{}", entry.id);
            assert_eq!(
                image.min_chip_rev_full, entry.min_chip_revision,
                "{}",
                entry.id
            );
            assert_eq!(
                image.len,
                entry.bytes.len(),
                "{}: ends with its trailer",
                entry.id
            );
            assert!(image.hash_appended, "{}", entry.id);
            let desc = BootloaderDesc::parse(&image.segments[0].data).unwrap();
            assert_eq!(desc.idf_ver, entry.idf_version, "{}", entry.id);
            assert!(
                desc.date_time.is_empty(),
                "{}: a reproducible build",
                entry.id
            );
            assert!(
                image.app_desc.is_none(),
                "{}: a bootloader, not an application",
                entry.id
            );
        }
    }

    #[test]
    fn the_esp32c3_dio_bootloader_is_the_reference_build() {
        let entry = shipped(ChipId::Esp32C3, SpiMode::Dio).unwrap();
        assert_eq!(entry.bytes.len(), 21_040);
        assert_eq!(
            entry.sha256, "601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3",
            "tools/reference-firmware/SHA256SUMS.inputs"
        );
        assert!(shipped(ChipId::Esp32C3, SpiMode::Qio).is_none());
        assert!(shipped(ChipId::Esp32S3, SpiMode::Dio).is_none());
    }

    #[test]
    fn for_a_16_mib_flash_the_image_is_written_as_it_is() {
        let entry = shipped(ChipId::Esp32C3, SpiMode::Dio).unwrap();
        assert_eq!(entry.for_flash(FlashSize::Mb16), entry.bytes);
    }

    #[test]
    fn for_a_4_mib_flash_the_header_and_digest_are_those_of_the_4_mib_build() {
        // The reference configuration built with CONFIG_ESPTOOLPY_FLASHSIZE_4MB, measured (SB §B).
        let entry = shipped(ChipId::Esp32C3, SpiMode::Dio).unwrap();
        let image = entry.for_flash(FlashSize::Mb4);
        assert_eq!(
            sha256_hex(&image),
            "d6421484b1abfd57d3bf50cf493d5b29b35edfc6f67e0690d13db8e4f56109c5"
        );
        let parsed = FirmwareImage::parse(&image).unwrap();
        assert_eq!(parsed.flash_size, FlashSize::Mb4);
        assert_eq!(parsed.flash_freq_raw, 0xF, "80 MHz is kept");
        let changed = image
            .iter()
            .zip(entry.bytes)
            .filter(|(new, old)| new != old)
            .count();
        assert!(changed <= 33, "byte 3 and the digest only: {changed}");
    }

    #[test]
    fn a_bootloader_descriptor_is_read_where_esp_idf_puts_it() {
        let entry = shipped(ChipId::Esp32C3, SpiMode::Dio).unwrap();
        let first_segment = &entry.bytes[0x20..0x20 + 80];
        let desc = BootloaderDesc::parse(first_segment).unwrap();
        assert_eq!(
            desc,
            BootloaderDesc {
                version: 1,
                idf_ver: "v5.5.2".into(),
                date_time: String::new()
            }
        );
        assert!(BootloaderDesc::parse(&first_segment[..79]).is_none());
        let mut other = first_segment.to_vec();
        other[0] = 0x51;
        assert!(BootloaderDesc::parse(&other).is_none());
    }
}
