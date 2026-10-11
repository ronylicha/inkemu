//! Checks of a full flash image (bootloader, partition table, application) before a machine is
//! started on it: the refusals of spec §5.1.
use crate::chip::{ChipId, FlashSize, SpiMode};
use crate::error::FirmwareError;
use crate::image::{HEADER_LEN, IMAGE_MAGIC, XOTA_MAGIC, le_u16, le_u32};
use crate::partition::{PARTITION_TABLE_LEN, PARTITION_TABLE_OFFSET, PartitionTable};

/// Offsets in the image header (`esp_image_header_t`): SPI mode, flash size and speed, entry point,
/// chip id in the extended header.
const SPI_MODE_OFFSET: usize = 2;
const SIZE_FREQ_OFFSET: usize = 3;
const ENTRY_OFFSET: usize = 4;
const CHIP_ID_OFFSET: usize = 12;
const BYTES_PER_MB: u64 = 1024 * 1024;

/// What the header of the bootloader at offset 0 declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashImageInfo {
    pub chip: ChipId,
    pub spi_mode: SpiMode,
    pub flash_size: FlashSize,
    pub entry: u32,
}

/// Whether `bytes` holds a partition table at offset 0x8000: at least one whole 32-byte entry with
/// the entry magic, the entries being ended by an MD5 entry that matches them, by erased bytes, or
/// by the end of the slice.
///
/// A dump cut inside the table sector counts. A slice that ends after N whole entries is a table of
/// N entries; one that ends inside an entry is judged on the whole entries before it, so a cut
/// inside the MD5 entry is accepted **without** the MD5 check. The bootloader, which reads the
/// table from the flash when it runs, is what judges the missing part.
#[must_use]
pub fn has_partition_table(bytes: &[u8]) -> bool {
    let start = PARTITION_TABLE_OFFSET as usize;
    let end = bytes.len().min(start + PARTITION_TABLE_LEN);
    bytes
        .get(start..end)
        .is_some_and(|table| PartitionTable::parse(table).is_ok())
}

/// Checks that `bytes` can be the flash content of a device with the chip `device_chip` and
/// `device_flash_bytes` of flash. Only the header of the bootloader is read: the ROM and the
/// bootloader validate the rest when they run, as on the device.
///
/// # Errors
/// [`FirmwareError::EncryptedOtaAsFlash`], [`FirmwareError::FlashImageTooShort`],
/// [`FirmwareError::NotAFlashImage`], [`FirmwareError::BadSpiMode`], [`FirmwareError::BadFlashSize`],
/// [`FirmwareError::ChipMismatch`], [`FirmwareError::FlashTooSmall`] or [`FirmwareError::NoPartitionTable`].
pub fn check_flash_image(
    bytes: &[u8],
    device_chip: ChipId,
    device_flash_bytes: u64,
) -> Result<FlashImageInfo, FirmwareError> {
    if bytes.starts_with(XOTA_MAGIC) {
        return Err(FirmwareError::EncryptedOtaAsFlash);
    }
    if bytes.len() < HEADER_LEN {
        return Err(FirmwareError::FlashImageTooShort { len: bytes.len() });
    }
    // INVARIANT: every index below is under HEADER_LEN (24), which the length check just covered.
    if bytes[0] != IMAGE_MAGIC {
        return Err(FirmwareError::NotAFlashImage { found: bytes[0] });
    }
    let spi_mode = SpiMode::from_raw(bytes[SPI_MODE_OFFSET]).ok_or(FirmwareError::BadSpiMode {
        raw: bytes[SPI_MODE_OFFSET],
    })?;
    let size_code = bytes[SIZE_FREQ_OFFSET] >> 4;
    let flash_size =
        FlashSize::from_nibble(size_code).ok_or(FirmwareError::BadFlashSize { raw: size_code })?;
    let chip = ChipId::from_raw(le_u16(bytes, CHIP_ID_OFFSET));
    if chip != device_chip {
        return Err(FirmwareError::ChipMismatch {
            expected: device_chip,
            found: chip,
        });
    }
    if flash_size.bytes() > device_flash_bytes {
        return Err(FirmwareError::FlashTooSmall {
            needed: flash_size.megabytes(),
            available: u32::try_from(device_flash_bytes / BYTES_PER_MB).unwrap_or(u32::MAX),
        });
    }
    if !has_partition_table(bytes) {
        return Err(FirmwareError::NoPartitionTable);
    }
    Ok(FlashImageInfo {
        chip,
        spi_mode,
        flash_size,
        entry: le_u32(bytes, ENTRY_OFFSET),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{ImageSpec, build_flash_image, build_image};

    const MIB16: u64 = 16 << 20;

    fn refusal(bytes: &[u8]) -> FirmwareError {
        check_flash_image(bytes, ChipId::Esp32C3, MIB16).unwrap_err()
    }

    #[test]
    fn a_merged_image_is_accepted_and_described() {
        let info = check_flash_image(
            &build_flash_image(&ImageSpec::default()),
            ChipId::Esp32C3,
            MIB16,
        )
        .unwrap();
        assert_eq!(info.chip, ChipId::Esp32C3);
        assert_eq!(info.spi_mode, SpiMode::Dio);
        assert_eq!(info.flash_size, FlashSize::Mb16);
        assert_eq!(info.entry, ImageSpec::default().entry);
    }

    #[test]
    fn an_encrypted_ota_file_is_refused() {
        let mut bytes = b"XOTA".to_vec();
        bytes.resize(0x9000, 0);
        let err = refusal(&bytes);
        assert_eq!(err, FirmwareError::EncryptedOtaAsFlash);
        let text = err.to_string();
        // The remedy of the application path ("the plain application .bin") would be refused here.
        assert!(
            text.contains("encrypted OTA update (.xota)")
                && text.contains("; use a full flash dump")
                && text.contains("merge_bin")
                && !text.contains("application .bin"),
            "{text}"
        );
    }

    #[test]
    fn a_file_that_does_not_start_with_an_image_is_refused_with_the_byte_found() {
        let mut bytes = build_flash_image(&ImageSpec::default());
        bytes[0] = 0x7F;
        let err = refusal(&bytes);
        assert_eq!(err, FirmwareError::NotAFlashImage { found: 0x7F });
        let text = err.to_string();
        assert!(
            text.contains("0x7f") && text.contains("0xe9") && text.contains("; use"),
            "{text}"
        );
        assert_eq!(
            refusal(&[0xE9; 10]),
            FirmwareError::FlashImageTooShort { len: 10 }
        );
        assert_eq!(refusal(&[]), FirmwareError::FlashImageTooShort { len: 0 });
    }

    #[test]
    fn an_image_for_another_chip_is_refused() {
        let bytes = build_flash_image(&ImageSpec {
            chip: 9,
            ..ImageSpec::default()
        });
        assert_eq!(
            refusal(&bytes),
            FirmwareError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3
            }
        );
    }

    #[test]
    fn an_image_that_declares_more_flash_than_the_device_has_is_refused() {
        let bytes = build_flash_image(&ImageSpec {
            flash_size_nibble: 5,
            ..ImageSpec::default()
        });
        assert_eq!(
            refusal(&bytes),
            FirmwareError::FlashTooSmall {
                needed: 32,
                available: 16
            }
        );
        let fits = build_flash_image(&ImageSpec {
            flash_size_nibble: 2,
            ..ImageSpec::default()
        });
        assert!(
            check_flash_image(&fits, ChipId::Esp32C3, MIB16).is_ok(),
            "4 MB on a 16 MB device"
        );
    }

    #[test]
    fn an_application_or_bootloader_alone_is_not_a_flash_image() {
        let app = build_image(&ImageSpec::default());
        let err = refusal(&app);
        assert_eq!(err, FirmwareError::NoPartitionTable);
        let text = err.to_string();
        assert!(
            text.contains("0x8000") && text.contains("merge_bin"),
            "{text}"
        );
    }

    #[test]
    fn every_cut_of_a_full_image_is_judged_by_one_rule_and_none_panics() {
        // The table starts at 0x8000: six 32-byte entries, then the MD5 entry from 0x80C0 to 0x80E0.
        const FIRST_WHOLE_ENTRY: usize = 0x8020;
        let full = build_flash_image(&ImageSpec::default());
        assert_eq!(full[0x80C0..0x80C2], [0xEB, 0xEB], "the MD5 entry");
        for len in [
            0,
            1,
            HEADER_LEN - 1,
            HEADER_LEN, // the header alone
            HEADER_LEN + 1,
            0x7FFF,
            0x8000,
            0x8001,
            0x801F,
            FIRST_WHOLE_ENTRY,
            0x8021,
            0x80BF,
            0x80C0, // the six entries, no MD5 entry
            0x80D0, // cut inside the MD5 entry: accepted, the MD5 is not checked
            0x80E0,
            0x8BFF, // 0xBFF bytes past the start of the table
            0x8C00,
        ] {
            let cut = &full[..len];
            let table = len >= FIRST_WHOLE_ENTRY;
            assert_eq!(has_partition_table(cut), table, "{len:#x}");
            let expected = if len < HEADER_LEN {
                Err(FirmwareError::FlashImageTooShort { len })
            } else if table {
                Ok(())
            } else {
                Err(FirmwareError::NoPartitionTable)
            };
            assert_eq!(
                check_flash_image(cut, ChipId::Esp32C3, MIB16).map(|_| ()),
                expected,
                "{len:#x}"
            );
        }
        // A corrupted entry is still caught on a cut that holds it whole, MD5 entry or not.
        let mut corrupted = full.clone();
        corrupted[0x8000] = 0x00;
        for len in [FIRST_WHOLE_ENTRY, 0x80C0, 0x80D0, 0x8C00] {
            assert!(!has_partition_table(&corrupted[..len]), "{len:#x}");
        }
    }

    #[test]
    fn a_table_is_recognised_by_its_content_not_by_two_bytes() {
        let full = build_flash_image(&ImageSpec::default());
        assert!(has_partition_table(&full));
        // The two magic bytes alone, followed by something that is not an entry: SP1a's false positive.
        let mut coincidence = build_image(&ImageSpec::default());
        coincidence.resize(0x9000, 0x00);
        coincidence[0x8000] = 0xAA;
        coincidence[0x8001] = 0x50;
        assert!(!has_partition_table(&coincidence));
        // `full` is 0x8C00 bytes (0x8000 + the 0xC00 bytes `to_bytes` returns): it already ends inside
        // the table sector, which SP1a's length rule refused below 0x9000. A dump cut at 0x8100, one
        // erased entry after the six entries and their MD5 entry (which ends at 0x80E0), is
        // recognised too; the test below walks the other cuts.
        assert_eq!(full.len(), 0x8C00);
        let mut cut = full.clone();
        cut.truncate(0x8100);
        assert!(has_partition_table(&cut));
        assert!(!has_partition_table(&full[..0x8000]));
        // A table whose MD5 does not match its entries is not a table.
        let mut corrupted = full.clone();
        corrupted[0x8004] ^= 0xFF;
        assert!(!has_partition_table(&corrupted));
    }
}
