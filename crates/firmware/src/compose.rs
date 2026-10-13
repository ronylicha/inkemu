//! A flash composed from an application image: the bootloader at offset 0, the partition table
//! at 0x8000, the application in the slot the bootloader starts, every other byte erased (spec
//! §5.1; `docs/research/shipped-bootloader-and-firmware-composition.md`, **SB** below, §B, §F).
//! Bytes in, bytes out: no file, no host. The same inputs give the same flash, byte for byte.
use crate::app_desc::AppDesc;
use crate::application::{ApplicationError, check_application};
use crate::bootloader::{
    BootloaderError, BootloaderIdentity, BootloaderWarning, ShippedBootloader, check_bootloader,
};
use crate::device::{Device, revision};
use crate::partition::{PARTITION_TABLE_OFFSET, Partition, PartitionError, PartitionTable};

/// Where the 2nd-stage bootloader goes: 0 on the ESP32-C3 and the ESP32-S3 (esptool's
/// `BOOTLOADER_FLASH_OFFSET`; SB §E).
pub const BOOTLOADER_OFFSET: usize = 0;
/// An erased flash byte.
const ERASED: u8 = 0xFF;

/// The bootloader of a composed flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bootloader<'a> {
    /// One inkemu ships: its header is set to the flash size of the device.
    Shipped(&'static ShippedBootloader),
    /// One the user gives: checked, never rewritten.
    Given(&'a [u8]),
}

/// A flash ready to boot, and what it was made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedFlash {
    /// Exactly the flash size of the device.
    pub bytes: Vec<u8>,
    pub bootloader: BootloaderIdentity,
    pub application: AppDesc,
    /// The partition the application was written to.
    pub slot: Partition,
    pub warnings: Vec<BootloaderWarning>,
}

/// Why no flash can be composed. Each message states the cause, then the remedy.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ComposeError {
    #[error("the partition table of the device is refused: {0}")]
    Table(PartitionError),
    #[error(
        "the partition table of the device has neither a factory application nor an ota_0 slot, and inkemu writes the application to the factory slot, else to ota_0; choose a device whose table has one"
    )]
    NoApplicationSlot,
    #[error(transparent)]
    Application(ApplicationError),
    #[error(transparent)]
    Bootloader(BootloaderError),
    #[error(
        "the bootloader inkemu ships for this device ({id}) requires chip revision {} or later and the device is {}: it would reset the chip for ever; give a bootloader built for {} (`--bootloader`), or choose a device of revision {} or later",
        revision(*required), revision(*device), revision(*device), revision(*required)
    )]
    ShippedNeedsLaterChip {
        id: &'static str,
        required: u16,
        device: u16,
    },
}

/// Composes the flash of `device` from its partition table, an application and a bootloader.
///
/// The application goes to the slot the bootloader starts on a blank `otadata`
/// ([`PartitionTable::application_slot`]); `otadata` and everything else stay erased, so the
/// first boot is the one of a freshly flashed device.
///
/// # Errors
/// [`ComposeError`]: the table, the application or the bootloader is refused.
pub fn compose(
    device: &Device,
    table: &PartitionTable,
    application: &[u8],
    bootloader: Bootloader<'_>,
) -> Result<ComposedFlash, ComposeError> {
    let table_bytes = table
        .to_bytes(device.flash_size)
        .map_err(ComposeError::Table)?;
    let slot = table
        .application_slot()
        .ok_or(ComposeError::NoApplicationSlot)?
        .clone();
    let report =
        check_application(application, device, &slot).map_err(ComposeError::Application)?;
    let (boot, identity, warnings) = match bootloader {
        Bootloader::Shipped(entry) => {
            if entry.chip != device.chip {
                return Err(ComposeError::Bootloader(BootloaderError::ChipMismatch {
                    expected: device.chip,
                    found: entry.chip,
                }));
            }
            if entry.min_chip_revision > device.chip_revision {
                return Err(ComposeError::ShippedNeedsLaterChip {
                    id: entry.id,
                    required: entry.min_chip_revision,
                    device: device.chip_revision,
                });
            }
            let bytes = entry.for_flash(device.flash_size);
            (bytes, BootloaderIdentity::Shipped(entry), Vec::new())
        }
        Bootloader::Given(bytes) => {
            let checked = check_bootloader(bytes, device).map_err(ComposeError::Bootloader)?;
            (bytes.to_vec(), checked.identity, checked.warnings)
        }
    };
    let Ok(size) = usize::try_from(device.flash_size.bytes()) else {
        // INVARIANT: a flash size is at most 128 MiB, which a usize of 32 bits holds.
        unreachable!("a flash size fits a usize")
    };
    let mut flash = vec![ERASED; size];
    // The bootloader is at most 0x8000 bytes (checked, or shipped), the table 0xC00 bytes at
    // 0x8000, and the application fits its slot, which the table holds inside the flash.
    flash[BOOTLOADER_OFFSET..BOOTLOADER_OFFSET + boot.len()].copy_from_slice(&boot);
    let table_at = PARTITION_TABLE_OFFSET as usize;
    flash[table_at..table_at + table_bytes.len()].copy_from_slice(&table_bytes);
    let app_at = slot.offset as usize;
    flash[app_at..app_at + application.len()].copy_from_slice(application);
    Ok(ComposedFlash {
        bytes: flash,
        bootloader: identity,
        application: report.desc,
        slot,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootloader::{SHIPPED, sha256_hex};
    use crate::builder::{ImageSpec, bootloader_spec, build_image};
    use crate::chip::{ChipId, FlashSize};
    use crate::partition::{SUBTYPE_FACTORY, TYPE_APP, xteink_x4_stock};

    const X4: Device = Device {
        chip: ChipId::Esp32C3,
        chip_revision: 3,
        efuse_block_revision: 102,
        flash_size: FlashSize::Mb16,
    };

    fn application() -> Vec<u8> {
        build_image(&ImageSpec::default())
    }

    fn shipped() -> Bootloader<'static> {
        Bootloader::Shipped(&SHIPPED[0])
    }

    #[test]
    fn the_flash_holds_the_bootloader_the_table_and_the_application_and_nothing_else() {
        let app = application();
        let flash = compose(&X4, &xteink_x4_stock(), &app, shipped()).unwrap();
        let bytes = &flash.bytes;
        assert_eq!(bytes.len(), 16 << 20);
        let boot = SHIPPED[0].bytes;
        assert_eq!(&bytes[..boot.len()], boot);
        let table = xteink_x4_stock().to_bytes(FlashSize::Mb16).unwrap();
        assert_eq!(&bytes[0x8000..0x8000 + table.len()], &table[..]);
        assert_eq!(&bytes[0x1_0000..0x1_0000 + app.len()], &app[..]);
        let written = |at: usize| {
            at < boot.len()
                || (0x8000..0x8000 + table.len()).contains(&at)
                || (0x1_0000..0x1_0000 + app.len()).contains(&at)
        };
        assert!(
            bytes
                .iter()
                .enumerate()
                .all(|(at, &byte)| written(at) || byte == 0xFF),
            "every other byte is erased, otadata included"
        );
        assert_eq!(flash.slot.name, "app0");
        assert_eq!(flash.application.project_name, "demo");
        assert_eq!(flash.bootloader, BootloaderIdentity::Shipped(&SHIPPED[0]));
    }

    #[test]
    fn the_same_inputs_compose_the_same_flash() {
        let app = application();
        let one = compose(&X4, &xteink_x4_stock(), &app, shipped()).unwrap();
        let two = compose(&X4, &xteink_x4_stock(), &app, shipped()).unwrap();
        assert_eq!(sha256_hex(&one.bytes), sha256_hex(&two.bytes));
    }

    #[test]
    fn a_smaller_flash_gets_the_shipped_bootloader_written_for_its_size() {
        let device = Device {
            flash_size: FlashSize::Mb4,
            ..X4
        };
        // nvs, otadata and a 1 MiB app0: what fits in 4 MiB of the stock layout.
        let mut table = xteink_x4_stock();
        table.entries.truncate(3);
        table.entries[2].size = 0x10_0000;
        let app = build_image(&ImageSpec {
            flash_size_nibble: FlashSize::Mb4.nibble(),
            ..ImageSpec::default()
        });
        let flash = compose(&device, &table, &app, shipped()).unwrap();
        assert_eq!(flash.bytes.len(), 4 << 20);
        assert_eq!(
            flash.bytes[..SHIPPED[0].bytes.len()],
            SHIPPED[0].for_flash(FlashSize::Mb4)[..]
        );
        assert_eq!(flash.bytes[3] >> 4, FlashSize::Mb4.nibble());
    }

    #[test]
    fn a_table_that_does_not_fit_the_flash_or_starts_nothing_is_refused() {
        let device = Device {
            flash_size: FlashSize::Mb4,
            ..X4
        };
        assert!(matches!(
            compose(&device, &xteink_x4_stock(), &application(), shipped()),
            Err(ComposeError::Table(PartitionError::BeyondFlash { .. }))
        ));
        let mut no_app = xteink_x4_stock();
        no_app.entries.retain(|p| p.ptype != TYPE_APP);
        let err = compose(&X4, &no_app, &application(), shipped()).unwrap_err();
        assert_eq!(err, ComposeError::NoApplicationSlot);
        assert!(
            err.to_string().contains(
                "and inkemu writes the application to the factory slot, else to ota_0; choose"
            ),
            "{err}"
        );
        // The prefix of a refused table states no cause of its own: the table's error does.
        let mut empty = xteink_x4_stock();
        empty.entries.clear();
        assert_eq!(
            compose(&X4, &empty, &application(), shipped())
                .unwrap_err()
                .to_string(),
            "the partition table of the device is refused: the partition table is empty; add at least one partition"
        );
    }

    #[test]
    fn a_factory_slot_is_where_the_application_goes_when_there_is_one() {
        let mut table = xteink_x4_stock();
        table.entries[3].subtype = SUBTYPE_FACTORY; // app1 becomes the factory application
        let app = application();
        let flash = compose(&X4, &table, &app, shipped()).unwrap();
        assert_eq!(flash.slot.name, "app1");
        assert_eq!(&flash.bytes[0x65_0000..0x65_0000 + app.len()], &app[..]);
        assert!(flash.bytes[0x1_0000..0x2_0000].iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn the_shipped_bootloader_is_refused_on_a_chip_it_would_reset_for_ever() {
        let device = Device {
            chip_revision: 2,
            ..X4
        };
        let mut app = application();
        app[15..17].copy_from_slice(&0u16.to_le_bytes());
        let err = compose(&device, &xteink_x4_stock(), &app, shipped()).unwrap_err();
        assert_eq!(
            err,
            ComposeError::ShippedNeedsLaterChip {
                id: "esp-idf-v5.5.2-esp32c3-dio",
                required: 3,
                device: 2
            }
        );
        let text = err.to_string();
        assert!(
            text.contains("v0.3 or later and the device is v0.2")
                && text.contains("; give a bootloader built for v0.2 (`--bootloader`)"),
            "{text}"
        );
        // The same device boots with a bootloader that accepts it.
        let given = build_image(&bootloader_spec());
        assert!(compose(&device, &xteink_x4_stock(), &app, Bootloader::Given(&given)).is_ok());
    }

    #[test]
    fn a_given_bootloader_is_written_as_it_is_with_its_warnings() {
        let mut given = build_image(&bootloader_spec());
        let last = given.len() - 1;
        given[last] ^= 1;
        let flash = compose(
            &X4,
            &xteink_x4_stock(),
            &application(),
            Bootloader::Given(&given),
        )
        .unwrap();
        assert_eq!(&flash.bytes[..given.len()], &given[..]);
        assert_eq!(flash.warnings, [BootloaderWarning::DigestMismatch]);
        assert!(matches!(
            flash.bootloader,
            BootloaderIdentity::Described { .. }
        ));
    }

    #[test]
    fn the_application_and_the_bootloader_are_judged_with_their_own_errors() {
        assert_eq!(
            compose(&X4, &xteink_x4_stock(), &[0u8; 64], shipped()),
            Err(ComposeError::Application(ApplicationError::NotAnImage {
                found: 0
            }))
        );
        let app = application();
        assert_eq!(
            compose(&X4, &xteink_x4_stock(), &app, Bootloader::Given(&app)),
            Err(ComposeError::Bootloader(BootloaderError::Application))
        );
        let s3 = Device {
            chip: ChipId::Esp32S3,
            ..X4
        };
        let mut s3_app = application();
        s3_app[12] = 9;
        crate::builder::refresh_digest(&mut s3_app);
        assert_eq!(
            compose(&s3, &xteink_x4_stock(), &s3_app, shipped()),
            Err(ComposeError::Bootloader(BootloaderError::ChipMismatch {
                expected: ChipId::Esp32S3,
                found: ChipId::Esp32C3
            }))
        );
    }
}
