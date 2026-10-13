//! Checks of an application image before it is written into a flash: what the 2nd-stage
//! bootloader would refuse, what does not fit, and a file that is something else (spec §5.1;
//! `docs/research/flash-writes-and-crosspoint-boot.md`, **FW** below, §M;
//! `docs/research/shipped-bootloader-and-firmware-composition.md`, **SB** below, §C).
use crate::app_desc::AppDesc;
use crate::bootloader::{BootloaderDesc, memory_map};
use crate::chip::ChipId;
use crate::device::{Device, revision};
use crate::error::FirmwareError;
use crate::image::{FirmwareImage, HEADER_LEN, IMAGE_MAGIC, XOTA_MAGIC};
use crate::partition::Partition;

/// The bootloader ignores a revision bound of 0 or 65535 (`IS_FIELD_SET`,
/// `bootloader_common_loader.c` of ESP-IDF v5.5.2).
const fn is_set(bound: u16) -> bool {
    bound != 0 && bound != u16::MAX
}

/// ` (CONFIG_…)`: the Kconfig option a remedy names, when it is known.
fn named(option: Option<&str>) -> String {
    option.map_or_else(String::new, |option| format!(" ({option})"))
}

/// What the check of an application found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationReport {
    pub desc: AppDesc,
    /// Bytes of the image; the file may hold more (a partition dump holds the rest of its slot).
    pub image_len: usize,
}

/// Imperatives that open the remedy clause of an [`ApplicationError`] (`"; <verb> ..."`).
pub const APPLICATION_REMEDY_VERBS: [&str; 3] = ["use", "give", "choose"];

/// Why a file cannot be the application of a device. The messages name the option of
/// `inkemu run` that takes what the file is.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApplicationError {
    #[error(
        "the firmware is an encrypted OTA update (.xota), which inkemu cannot read; use the plain application .bin of a release, not the encrypted OTA file"
    )]
    EncryptedOta,
    #[error(
        "the firmware is a whole flash image (a dump, or the output of `esptool.py merge_bin`: a partition table at 0x8000), not an application; use it as the flash (`--flash`)"
    )]
    FlashImage,
    #[error(
        "the firmware is empty or shorter than an image header ({len} bytes); use the application .bin of an ESP-IDF build or release"
    )]
    TooShort { len: usize },
    #[error(
        "the firmware is not an ESP-IDF image: its first byte is {found:#04x}, expected 0xe9 (an encrypted or compressed update, or another format); use the plain application .bin of an ESP-IDF build or release"
    )]
    NotAnImage { found: u8 },
    #[error("the firmware is damaged: {0}")]
    Damaged(FirmwareError),
    #[error(
        "the firmware is a 2nd-stage bootloader (it carries a bootloader descriptor at 0x20), not an application; give it as the bootloader (`--bootloader`), and the application .bin as the firmware"
    )]
    Bootloader,
    #[error(
        "the firmware has no application descriptor at 0x20 (esp_app_desc_t), which every ESP-IDF application carries and the bootloader reads; use the application .bin of an ESP-IDF build"
    )]
    NoDescriptor,
    #[error(
        "the firmware is built for {found} and the device uses {expected}; choose a device with that chip, or use the firmware built for {expected}"
    )]
    ChipMismatch { expected: ChipId, found: ChipId },
    #[error(
        "the firmware is {len} bytes and its slot, {slot}, holds {size}; use an application that fits, or choose a device whose partition table has a larger slot"
    )]
    TooLarge { len: usize, slot: String, size: u32 },
    #[error(
        "the header of the firmware says {header} MB of flash and the device has {device} MB; use the firmware built for {device} MB, or choose a device with at least {header} MB"
    )]
    LargerFlash { header: u32, device: u32 },
    #[error(
        "the firmware requires chip revision {} or later and the device is {}: the bootloader would refuse to start it; use a build for {}{}, or choose a device of revision {} or later",
        revision(*required), revision(*device), revision(*device), named(*option), revision(*required)
    )]
    ChipTooOld {
        required: u16,
        device: u16,
        /// The Kconfig option that sets the revision, when inkemu models the chip
        /// ([`crate::bootloader::MemoryMap::revision_option`]).
        option: Option<&'static str>,
    },
    #[error(
        "the firmware accepts chip revisions up to {} and the device is {}: the bootloader would refuse to start it; use a build that accepts {}, or choose a device of revision {} or earlier",
        revision(*accepted), revision(*device), revision(*device), revision(*accepted)
    )]
    ChipTooNew { accepted: u16, device: u16 },
    #[error(
        "the firmware requires eFuse block revision {} or later and the chip has {}: the bootloader would refuse to start it; use a build that accepts {}",
        revision(*required), revision(*device), revision(*device)
    )]
    EfuseBlockTooOld { required: u16, device: u16 },
    #[error(
        "the firmware accepts eFuse block revisions up to {} and the chip has {}: the bootloader would refuse to start it; use a build that accepts {}",
        revision(*accepted), revision(*device), revision(*device)
    )]
    EfuseBlockTooNew { accepted: u16, device: u16 },
}

/// Checks the application `bytes` for `device`, to be written into `slot`. The application is
/// never rewritten: what the bootloader would refuse is refused here, before boot.
///
/// # Errors
/// [`ApplicationError`]: the cause, then what to give instead.
pub fn check_application(
    bytes: &[u8],
    device: &Device,
    slot: &Partition,
) -> Result<ApplicationReport, ApplicationError> {
    if bytes.starts_with(XOTA_MAGIC) {
        return Err(ApplicationError::EncryptedOta);
    }
    if crate::flash_image::has_partition_table(bytes) {
        return Err(ApplicationError::FlashImage);
    }
    if bytes.len() < HEADER_LEN {
        return Err(ApplicationError::TooShort { len: bytes.len() });
    }
    if bytes[0] != IMAGE_MAGIC {
        return Err(ApplicationError::NotAnImage { found: bytes[0] });
    }
    let image = FirmwareImage::parse(bytes).map_err(ApplicationError::Damaged)?;
    let first = image.segments.first().map(|s| s.data.as_slice());
    let Some(desc) = image.app_desc.clone() else {
        return Err(if first.and_then(BootloaderDesc::parse).is_some() {
            ApplicationError::Bootloader
        } else {
            ApplicationError::NoDescriptor
        });
    };
    if image.chip != device.chip {
        return Err(ApplicationError::ChipMismatch {
            expected: device.chip,
            found: image.chip,
        });
    }
    if bytes.len() > slot.size as usize {
        return Err(ApplicationError::TooLarge {
            len: bytes.len(),
            slot: slot.name.clone(),
            size: slot.size,
        });
    }
    let (header, flash) = (image.flash_size.megabytes(), device.flash_size.megabytes());
    if header > flash {
        return Err(ApplicationError::LargerFlash {
            header,
            device: flash,
        });
    }
    check_revisions(&image, &desc, device)?;
    Ok(ApplicationReport {
        desc,
        image_len: image.len,
    })
}

/// The revision bounds the bootloader checks before it starts an application: the chip's
/// (`bootloader_common_check_chip_revision_validity`) and the eFuse block's
/// (`bootloader_common_check_efuse_blk_validity`, SB §C).
fn check_revisions(
    image: &FirmwareImage,
    desc: &AppDesc,
    device: &Device,
) -> Result<(), ApplicationError> {
    let chip = device.chip_revision;
    if image.min_chip_rev_full > chip {
        return Err(ApplicationError::ChipTooOld {
            required: image.min_chip_rev_full,
            device: chip,
            option: memory_map(device.chip).map(|map| map.revision_option),
        });
    }
    if is_set(image.max_chip_rev_full) && chip > image.max_chip_rev_full {
        return Err(ApplicationError::ChipTooNew {
            accepted: image.max_chip_rev_full,
            device: chip,
        });
    }
    let block = device.efuse_block_revision;
    if is_set(desc.min_efuse_block_revision) && desc.min_efuse_block_revision > block {
        return Err(ApplicationError::EfuseBlockTooOld {
            required: desc.min_efuse_block_revision,
            device: block,
        });
    }
    if is_set(desc.max_efuse_block_revision) && block > desc.max_efuse_block_revision {
        return Err(ApplicationError::EfuseBlockTooNew {
            accepted: desc.max_efuse_block_revision,
            device: block,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{
        ImageSpec, app_desc_bytes, bootloader_spec, build_flash_image, build_image, refresh_digest,
    };
    use crate::chip::FlashSize;
    use crate::partition::{SUBTYPE_OTA_0, TYPE_APP};

    const X4: Device = Device {
        chip: ChipId::Esp32C3,
        chip_revision: 3,
        efuse_block_revision: 102,
        flash_size: FlashSize::Mb16,
    };

    fn slot(size: u32) -> Partition {
        Partition {
            name: "app0".into(),
            ptype: TYPE_APP,
            subtype: SUBTYPE_OTA_0,
            offset: 0x1_0000,
            size,
            flags: 0,
        }
    }

    fn refusal(bytes: &[u8]) -> ApplicationError {
        check_application(bytes, &X4, &slot(0x64_0000)).unwrap_err()
    }

    /// The default test application with its header bytes `at..` set to `value` (LE), digest
    /// refreshed.
    fn with_header(at: usize, value: u16) -> Vec<u8> {
        let mut bytes = build_image(&ImageSpec::default());
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        refresh_digest(&mut bytes);
        bytes
    }

    /// The default test application whose descriptor accepts eFuse blocks `min..=max`.
    fn with_efuse_blocks(min: u16, max: u16) -> Vec<u8> {
        let mut desc = app_desc_bytes("1.0.0", "demo", "v5.5.2");
        desc[176..178].copy_from_slice(&min.to_le_bytes());
        desc[178..180].copy_from_slice(&max.to_le_bytes());
        let mut spec = ImageSpec::default();
        spec.segments[0].1 = desc;
        build_image(&spec)
    }

    #[test]
    fn an_application_for_the_device_passes_with_its_descriptor() {
        let bytes = build_image(&ImageSpec::default());
        let report = check_application(&bytes, &X4, &slot(0x64_0000)).unwrap();
        assert_eq!(report.desc.project_name, "demo");
        assert_eq!(report.image_len, bytes.len());
    }

    #[test]
    fn what_is_not_an_application_is_named_for_what_it_is() {
        let mut xota = b"XOTA".to_vec();
        xota.resize(512, 0);
        assert_eq!(refusal(&xota), ApplicationError::EncryptedOta);
        assert_eq!(
            refusal(&build_flash_image(&ImageSpec::default())),
            ApplicationError::FlashImage
        );
        assert_eq!(
            refusal(&build_image(&bootloader_spec())),
            ApplicationError::Bootloader
        );
        let spec = ImageSpec {
            segments: vec![(0x4038_0000, vec![0; 8])],
            ..ImageSpec::default()
        };
        assert_eq!(refusal(&build_image(&spec)), ApplicationError::NoDescriptor);
        assert_eq!(refusal(&[0xE9; 10]), ApplicationError::TooShort { len: 10 });
        assert_eq!(
            refusal(&[0x00; 64]),
            ApplicationError::NotAnImage { found: 0 }
        );
    }

    #[test]
    fn a_damaged_image_is_refused_with_what_is_wrong() {
        let mut bytes = build_image(&ImageSpec::default());
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(
            refusal(&bytes),
            ApplicationError::Damaged(FirmwareError::HashMismatch)
        );
        let mut lying = build_image(&ImageSpec::default());
        lying[28..32].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(matches!(
            refusal(&lying),
            ApplicationError::Damaged(FirmwareError::TruncatedSegment { .. })
        ));
    }

    #[test]
    fn an_application_for_another_chip_is_refused_with_both_chips() {
        let err = refusal(&with_header(12, 9));
        assert_eq!(
            err,
            ApplicationError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3
            }
        );
        let text = err.to_string();
        assert!(
            text.contains("ESP32-S3") && text.contains("ESP32-C3"),
            "{text}"
        );
    }

    #[test]
    fn an_application_larger_than_its_slot_is_refused() {
        let bytes = build_image(&ImageSpec::default());
        let len = bytes.len();
        let fits = u32::try_from(len).unwrap();
        assert!(
            check_application(&bytes, &X4, &slot(fits)).is_ok(),
            "exactly the slot"
        );
        assert_eq!(
            check_application(&bytes, &X4, &slot(fits - 1)).unwrap_err(),
            ApplicationError::TooLarge {
                len,
                slot: "app0".into(),
                size: fits - 1
            }
        );
    }

    #[test]
    fn an_application_declaring_more_flash_than_the_device_is_refused() {
        let mut bytes = build_image(&ImageSpec::default());
        bytes[3] = 0x5F;
        refresh_digest(&mut bytes);
        assert_eq!(
            refusal(&bytes),
            ApplicationError::LargerFlash {
                header: 32,
                device: 16
            }
        );
    }

    #[test]
    fn the_chip_revision_bounds_are_those_the_bootloader_checks() {
        // Header bytes 15..17: min_chip_rev_full; 17..19: max_chip_rev_full.
        assert_eq!(
            refusal(&with_header(15, 4)),
            ApplicationError::ChipTooOld {
                required: 4,
                device: 3,
                option: Some("CONFIG_ESP32C3_REV_MIN")
            }
        );
        assert!(check_application(&with_header(15, 3), &X4, &slot(0x64_0000)).is_ok());
        assert_eq!(
            refusal(&with_header(17, 2)),
            ApplicationError::ChipTooNew {
                accepted: 2,
                device: 3
            }
        );
        for max in [0, u16::MAX, 3] {
            assert!(
                check_application(&with_header(17, max), &X4, &slot(0x64_0000)).is_ok(),
                "max {max}"
            );
        }
    }

    #[test]
    fn the_efuse_block_bounds_are_those_the_bootloader_checks() {
        assert_eq!(
            refusal(&with_efuse_blocks(103, 199)),
            ApplicationError::EfuseBlockTooOld {
                required: 103,
                device: 102
            }
        );
        assert_eq!(
            refusal(&with_efuse_blocks(0, 101)),
            ApplicationError::EfuseBlockTooNew {
                accepted: 101,
                device: 102
            }
        );
        for (min, max) in [(0, 0), (u16::MAX, u16::MAX), (102, 102), (0, 199)] {
            assert!(
                check_application(&with_efuse_blocks(min, max), &X4, &slot(0x64_0000)).is_ok(),
                "{min}..={max}"
            );
        }
    }

    #[test]
    fn every_refusal_states_a_remedy() {
        let errors = [
            ApplicationError::EncryptedOta,
            ApplicationError::FlashImage,
            ApplicationError::TooShort { len: 1 },
            ApplicationError::NotAnImage { found: 1 },
            ApplicationError::Damaged(FirmwareError::HashMismatch),
            ApplicationError::Bootloader,
            ApplicationError::NoDescriptor,
            ApplicationError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3,
            },
            ApplicationError::TooLarge {
                len: 2,
                slot: "app0".into(),
                size: 1,
            },
            ApplicationError::LargerFlash {
                header: 32,
                device: 16,
            },
            ApplicationError::ChipTooOld {
                required: 4,
                device: 3,
                option: Some("CONFIG_ESP32C3_REV_MIN"),
            },
            ApplicationError::ChipTooOld {
                required: 4,
                device: 3,
                option: None,
            },
            ApplicationError::ChipTooNew {
                accepted: 2,
                device: 3,
            },
            ApplicationError::EfuseBlockTooOld {
                required: 103,
                device: 102,
            },
            ApplicationError::EfuseBlockTooNew {
                accepted: 101,
                device: 102,
            },
        ];
        let verbs: Vec<&str> = APPLICATION_REMEDY_VERBS
            .iter()
            .chain(crate::error::REMEDY_VERBS.iter())
            .copied()
            .collect();
        for err in errors {
            let message = err.to_string();
            assert!(
                verbs
                    .iter()
                    .any(|verb| message.contains(&format!("; {verb} "))),
                "no remedy in: {message}"
            );
        }
    }
}
