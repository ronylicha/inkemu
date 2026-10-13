//! The 2nd-stage bootloader: the descriptor it carries, and the ones inkemu ships.
//!
//! inkemu ships one bootloader per chip family and flash-mode family, built from unmodified
//! ESP-IDF sources (`third_party/esp-idf-bootloader/`, ADR 0005). A device profile selects one by
//! what it states, its chip family; the size of its flash is written into the header when the
//! flash is composed (`docs/research/shipped-bootloader-and-firmware-composition.md`, **SB** in
//! the comments of this crate, §B).
use crate::app_desc::AppDesc;
use crate::chip::{ChipId, FlashSize, SpiMode};
use crate::device::{Device, revision};
use crate::error::FirmwareError;
use crate::image::{
    FirmwareImage, HEADER_LEN, IMAGE_MAGIC, Segment, XOTA_MAGIC, first_segment_data,
};
use crate::partition::PARTITION_TABLE_OFFSET;
use sha2::{Digest, Sha256};
use std::fmt;

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
/// Header byte 2: the SPI mode. A bootloader runs in QIO, QOUT, DIO or DOUT (0 to 3).
const SPI_MODE_OFFSET: usize = 2;
const LAST_BOOTLOADER_MODE: u8 = 3;

/// Where the mask ROM of a chip family loads a 2nd-stage bootloader, and what it refuses: the
/// rules a bootloader given for a device of that family is checked against. Every range is
/// `(start, end)`, `end` excluded.
#[derive(Debug, PartialEq, Eq)]
pub struct MemoryMap {
    pub chip: ChipId,
    /// Internal SRAM, through its data bus (DRAM) and its instruction bus (IRAM): where the ROM
    /// loads a bootloader's segments.
    pub ram: &'static [(u32, u32)],
    /// The windows the cache maps the flash to: where an application's code and read-only data
    /// are, and never a bootloader's.
    pub flash_windows: &'static [(u32, u32)],
    /// The ROM's stack and data, where it refuses to load a segment.
    pub rom_reserved: (u32, u32),
    /// The speed codes the ROM accepts in the low nibble of header byte 3.
    pub rom_speeds: &'static [u8],
    /// The Kconfig option that sets the lowest chip revision a build starts on, which a refusal
    /// of a build for a later chip names.
    pub revision_option: &'static str,
}

/// The memory map of every chip family inkemu models. Adding a chip family is one entry here, one
/// in [`SHIPPED`], and its directory in `third_party/esp-idf-bootloader/`; a chip without an
/// entry is refused before its segments are looked at.
pub static MEMORY_MAPS: [MemoryMap; 1] = [MemoryMap {
    chip: ChipId::Esp32C3,
    // ESP32-C3 Technical Reference Manual, "System and Memory": DRAM, then IRAM.
    ram: &[(0x3FC8_0000, 0x3FCE_0000), (0x4037_C000, 0x403E_0000)],
    // The same chapter: the data bus window, then the instruction bus window, of the flash.
    flash_windows: &[(0x3C00_0000, 0x3C80_0000), (0x4200_0000, 0x4280_0000)],
    // `docs/research/esp32c3-rom-flash-boot.md` (**RFB** below), §F step 5.
    rom_reserved: (0x3FCD_E160, 0x3FCD_F664),
    // RFB §F step 1: 0 to 2, and 15.
    rom_speeds: &[0, 1, 2, 0xF],
    // The `choice ESP32C3_REV_MIN` of `components/esp_hw_support/port/esp32c3/Kconfig.hw_support`
    // (ESP-IDF v5.5).
    revision_option: "CONFIG_ESP32C3_REV_MIN",
}];

/// The memory map of `chip`, if inkemu models it.
#[must_use]
pub fn memory_map(chip: ChipId) -> Option<&'static MemoryMap> {
    MEMORY_MAPS.iter().find(|map| map.chip == chip)
}

impl MemoryMap {
    /// Whether `len` bytes at `addr` fall in a window of the flash.
    fn in_flash_window(&self, addr: u32, len: usize) -> bool {
        self.flash_windows
            .iter()
            .any(|&window| overlaps(window, addr, len))
    }

    /// Whether `len` bytes at `addr` lie whole in internal RAM.
    fn in_ram(&self, addr: u32, len: usize) -> bool {
        self.ram.iter().any(|&ram| within(ram, addr, len))
    }

    /// Whether `segments` are laid out as a bootloader's: at least one, and every one in internal
    /// RAM, so none in a window of the flash (the two never overlap; a test holds every map to it).
    pub(crate) fn loads_only_into_ram(&self, segments: &[Segment]) -> bool {
        !segments.is_empty()
            && segments
                .iter()
                .all(|segment| self.in_ram(segment.load_addr, segment.data.len()))
    }

    /// Whether the ROM loads `len` bytes at `addr`: word-aligned, in internal RAM, clear of its
    /// own stack and data. The ROM also refuses a segment longer than 1 MiB
    /// (`docs/research/esp32c3-rom-flash-boot.md`, §F step 4), which no range of
    /// [`MemoryMap::ram`] holds, so lying in RAM covers it.
    fn loads(&self, addr: u32, len: usize) -> bool {
        addr.is_multiple_of(4) && self.in_ram(addr, len) && !overlaps(self.rom_reserved, addr, len)
    }
}

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

/// Every bootloader inkemu ships. Adding a chip family is one entry here, its [`MemoryMap`] in
/// [`MEMORY_MAPS`], and its directory in `third_party/esp-idf-bootloader/`; a device with a chip
/// already here needs its profile only.
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

/// A bootloader inkemu recognises by its SHA-256, and neither ships nor fetches
/// (`docs/research/flash-writes-and-crosspoint-boot.md`, §N).
#[derive(Debug, PartialEq, Eq)]
pub struct KnownBootloader {
    pub name: &'static str,
    /// SHA-256 of the image, from its header to the end of its trailer, in lower-case
    /// hexadecimal: of the whole file for a bootloader.bin, which ends with its trailer.
    pub sha256: &'static str,
}

/// Bootloaders a user is likely to give, named in the summary of a run.
pub static KNOWN: [KnownBootloader; 1] = [KnownBootloader {
    name: "the Arduino-ESP32 prebuilt bootloader of the CrossPoint web flasher (ESP-IDF v5.5.2-729-g87912cd291)",
    sha256: "6e6b0d386095f0783e9f0513ea29ea2ce413790e7f7153a06a2b04e2b5f59fb6",
}];

/// What a bootloader is known as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootloaderIdentity {
    /// One inkemu ships.
    Shipped(&'static ShippedBootloader),
    /// One of [`KNOWN`].
    Known(&'static KnownBootloader),
    /// Another, as its descriptor describes it.
    Described {
        idf_ver: String,
        date_time: String,
        sha256: String,
    },
    /// Another, without a descriptor (ESP-IDF before v5.1, or not ESP-IDF).
    Unknown { sha256: String },
}

impl fmt::Display for BootloaderIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BootloaderIdentity::Shipped(entry) => write!(
                f,
                "the ESP-IDF {} bootloader inkemu ships ({})",
                entry.idf_version, entry.id
            ),
            BootloaderIdentity::Known(known) => f.write_str(known.name),
            BootloaderIdentity::Described {
                idf_ver,
                date_time,
                sha256,
            } if date_time.is_empty() => {
                write!(f, "a bootloader of ESP-IDF {idf_ver}, sha256 {sha256}")
            }
            BootloaderIdentity::Described {
                idf_ver,
                date_time,
                sha256,
            } => write!(
                f,
                "a bootloader of ESP-IDF {idf_ver} built {date_time}, sha256 {sha256}"
            ),
            BootloaderIdentity::Unknown { sha256 } => {
                write!(f, "a bootloader without a descriptor, sha256 {sha256}")
            }
        }
    }
}

/// What may not go as the user expects, and does not stop the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BootloaderWarning {
    #[error(
        "the SHA-256 appended to the bootloader does not match it; the mask ROM boots it anyway, as a chip without secure boot does, and prints four lines that say so; rebuild the bootloader if it was not changed on purpose"
    )]
    DigestMismatch,
    #[error(
        "the header of the bootloader says {header} MB of flash and the device has {device} MB; the application is told {header} MB and uses no more; rebuild the bootloader for {device} MB to use the whole flash"
    )]
    SmallerFlash { header: u32, device: u32 },
}

/// What the check of a bootloader found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootloaderReport {
    pub identity: BootloaderIdentity,
    pub warnings: Vec<BootloaderWarning>,
}

/// Imperatives that open the remedy clause of a [`BootloaderError`] (`"; <verb> ..."`).
pub const BOOTLOADER_REMEDY_VERBS: [&str; 4] = ["use", "give", "rebuild", "choose"];

/// Why a file cannot be the bootloader of a device. The messages name the option of `inkemu run`
/// that takes what the file is.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BootloaderError {
    #[error(
        "the bootloader is an encrypted OTA update (.xota), not a bootloader; give the bootloader.bin of a build, or no bootloader to use the one inkemu ships"
    )]
    EncryptedOta,
    #[error(
        "the bootloader is a whole flash image (a partition table follows it at 0x8000), not a bootloader alone; use the file as the flash (`inkemu run --flash`)"
    )]
    FlashImage,
    #[error(
        "the bootloader is {len} bytes, more than the 32768 bytes below the partition table; give the bootloader.bin of a build, or no bootloader to use the one inkemu ships"
    )]
    TooLong { len: usize },
    #[error(
        "the bootloader is empty or shorter than an image header ({len} bytes); give the bootloader.bin of a build, or no bootloader to use the one inkemu ships"
    )]
    TooShort { len: usize },
    #[error(
        "the bootloader is not an ESP-IDF image: its first byte is {found:#04x}, expected 0xe9; give the bootloader.bin of a build, or no bootloader to use the one inkemu ships"
    )]
    NotAnImage { found: u8 },
    #[error(
        "the bootloader is an application image (it carries an application descriptor at 0x20); use it as the firmware (`--firmware`), and give no bootloader or the bootloader.bin of its build"
    )]
    Application,
    #[error(
        "the bootloader declares SPI mode {raw}, which no bootloader runs in (0 to 3: QIO, QOUT, DIO, DOUT); rebuild it, or use the one inkemu ships"
    )]
    BadMode { raw: u8 },
    #[error(
        "the bootloader declares flash speed code {raw}, which the mask ROM refuses ({}); rebuild it, or use the one inkemu ships",
        either(accepted)
    )]
    BadSpeed {
        raw: u8,
        /// The codes the ROM of the chip accepts ([`MemoryMap::rom_speeds`]).
        accepted: &'static [u8],
    },
    #[error("the bootloader is damaged: {0}")]
    Damaged(FirmwareError),
    #[error(
        "segment {index} of the bootloader is loaded at {addr:#010x}, in a window of the flash, as an application's code is; use the file as the firmware (`--firmware`), with the bootloader of its build"
    )]
    FlashMapped { index: usize, addr: u32 },
    #[error(
        "segment {index} of the bootloader ({len} bytes at {addr:#010x}) is not where the mask ROM loads a bootloader (word-aligned, at most 1 MiB, in internal RAM, clear of {:#x}-{:#x}); rebuild it, or use the one inkemu ships",
        rom_reserved.0, rom_reserved.1 - 1
    )]
    Unloadable {
        index: usize,
        addr: u32,
        len: usize,
        /// The ROM's stack and data on the chip of the device ([`MemoryMap::rom_reserved`]).
        rom_reserved: (u32, u32),
    },
    #[error(
        "the bootloader is built for {found}, and the device uses {expected}; give a bootloader built for {expected}, or no bootloader to use the one inkemu ships"
    )]
    ChipMismatch { expected: ChipId, found: ChipId },
    #[error(
        "the device uses {chip}, and inkemu does not know where the mask ROM of {chip} loads a bootloader; choose a device whose chip inkemu models"
    )]
    UnmodelledChip { chip: ChipId },
    #[error(
        "the bootloader requires chip revision {} or later and the device is {}: it would reset the chip for ever; give a bootloader built for {} ({option}), or choose a device of revision {} or later",
        revision(*required), revision(*device), revision(*device), revision(*required)
    )]
    RevisionTooHigh {
        required: u16,
        device: u16,
        /// The Kconfig option of the chip ([`MemoryMap::revision_option`]).
        option: &'static str,
    },
    #[error(
        "the header of the bootloader says {header} MB of flash and the device has {device} MB: the application would abort when it finds the smaller chip; give a bootloader built for {device} MB, or no bootloader to use the one inkemu ships"
    )]
    LargerFlash { header: u32, device: u32 },
}

/// `0, 1, 2 or 15`: the codes of `codes`, as a sentence lists them.
fn either(codes: &[u8]) -> String {
    match codes.split_last() {
        None => String::new(),
        Some((final_code, [])) => final_code.to_string(),
        Some((final_code, others)) => {
            let others: Vec<String> = others.iter().map(ToString::to_string).collect();
            format!("{} or {final_code}", others.join(", "))
        }
    }
}

fn within(range: (u32, u32), addr: u32, len: usize) -> bool {
    let end = u64::from(addr) + len as u64;
    u64::from(range.0) <= u64::from(addr) && end <= u64::from(range.1)
}

fn overlaps(range: (u32, u32), addr: u32, len: usize) -> bool {
    let end = u64::from(addr) + len as u64;
    u64::from(addr) < u64::from(range.1) && end > u64::from(range.0)
}

/// What the bootloader `image`, read from `bytes`, is known as among `shipped` and `known`, or
/// as its descriptor describes it. The hash is that of the image, from its header to the end of
/// its trailer: the first 0x8000 bytes of a dump hold a bootloader followed by erased bytes.
fn identify_in(
    bytes: &[u8],
    image: &FirmwareImage,
    shipped: &'static [ShippedBootloader],
    known: &'static [KnownBootloader],
) -> BootloaderIdentity {
    // `image` was parsed from `bytes`, so its trailer ends within them: `get` never falls back.
    let sha256 = sha256_hex(bytes.get(..image.len).unwrap_or(bytes));
    if let Some(entry) = shipped.iter().find(|entry| entry.sha256 == sha256) {
        return BootloaderIdentity::Shipped(entry);
    }
    if let Some(known) = known.iter().find(|known| known.sha256 == sha256) {
        return BootloaderIdentity::Known(known);
    }
    match image
        .segments
        .first()
        .and_then(|s| BootloaderDesc::parse(&s.data))
    {
        Some(desc) => BootloaderIdentity::Described {
            idf_ver: desc.idf_ver,
            date_time: desc.date_time,
            sha256,
        },
        None => BootloaderIdentity::Unknown { sha256 },
    }
}

/// Checks a bootloader given by the user before it is written at offset 0 of the flash of
/// `device`: what the mask ROM would refuse, what would make the bootloader reset the chip or
/// the application abort, and a file that is something else (SB §G). It is never rewritten.
///
/// # Errors
/// [`BootloaderError`]: the cause, then what to give instead.
pub fn check_bootloader(
    bytes: &[u8],
    device: &Device,
) -> Result<BootloaderReport, BootloaderError> {
    if bytes.starts_with(XOTA_MAGIC) {
        return Err(BootloaderError::EncryptedOta);
    }
    if crate::flash_image::has_partition_table(bytes) {
        return Err(BootloaderError::FlashImage);
    }
    if bytes.len() > PARTITION_TABLE_OFFSET as usize {
        // An application is far longer than a bootloader can be: its descriptor, read in place,
        // names it before its length is refused.
        if first_segment_data(bytes).and_then(AppDesc::parse).is_some() {
            return Err(BootloaderError::Application);
        }
        return Err(BootloaderError::TooLong { len: bytes.len() });
    }
    if bytes.len() < HEADER_LEN {
        return Err(BootloaderError::TooShort { len: bytes.len() });
    }
    // INVARIANT: every index below is under HEADER_LEN, which the length check just covered.
    if bytes[0] != IMAGE_MAGIC {
        return Err(BootloaderError::NotAnImage { found: bytes[0] });
    }
    if bytes[SPI_MODE_OFFSET] > LAST_BOOTLOADER_MODE {
        return Err(BootloaderError::BadMode {
            raw: bytes[SPI_MODE_OFFSET],
        });
    }
    let (image, digest_matches) =
        FirmwareImage::parse_reporting_digest(bytes).map_err(BootloaderError::Damaged)?;
    if image.app_desc.is_some() {
        return Err(BootloaderError::Application);
    }
    // The ROM checks the chip before it looks at a segment (RFB §F steps 3 to 5); where it loads
    // one depends on the chip.
    if image.chip != device.chip {
        return Err(BootloaderError::ChipMismatch {
            expected: device.chip,
            found: image.chip,
        });
    }
    let map =
        memory_map(device.chip).ok_or(BootloaderError::UnmodelledChip { chip: device.chip })?;
    let speed = bytes[SIZE_FREQ_OFFSET] & 0x0F;
    if !map.rom_speeds.contains(&speed) {
        return Err(BootloaderError::BadSpeed {
            raw: speed,
            accepted: map.rom_speeds,
        });
    }
    for (index, segment) in image.segments.iter().enumerate() {
        let (addr, len) = (segment.load_addr, segment.data.len());
        if map.in_flash_window(addr, len) {
            return Err(BootloaderError::FlashMapped { index, addr });
        }
        if !map.loads(addr, len) {
            return Err(BootloaderError::Unloadable {
                index,
                addr,
                len,
                rom_reserved: map.rom_reserved,
            });
        }
    }
    if image.min_chip_rev_full > device.chip_revision {
        return Err(BootloaderError::RevisionTooHigh {
            required: image.min_chip_rev_full,
            device: device.chip_revision,
            option: map.revision_option,
        });
    }
    let (header, flash) = (image.flash_size.megabytes(), device.flash_size.megabytes());
    if header > flash {
        return Err(BootloaderError::LargerFlash {
            header,
            device: flash,
        });
    }
    let mut warnings = Vec::new();
    if header < flash {
        warnings.push(BootloaderWarning::SmallerFlash {
            header,
            device: flash,
        });
    }
    if !digest_matches {
        warnings.push(BootloaderWarning::DigestMismatch);
    }
    Ok(BootloaderReport {
        identity: identify_in(bytes, &image, &SHIPPED, &KNOWN),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{
        ImageSpec, app_desc_bytes, bootloader_spec, build_flash_image, build_image, refresh_digest,
    };
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

    const X4: Device = Device {
        chip: ChipId::Esp32C3,
        chip_revision: 3,
        efuse_block_revision: 102,
        flash_size: FlashSize::Mb16,
    };

    fn shipped_bytes() -> &'static [u8] {
        SHIPPED[0].bytes
    }

    fn refusal(bytes: &[u8]) -> BootloaderError {
        check_bootloader(bytes, &X4).unwrap_err()
    }

    #[test]
    fn the_shipped_bootloader_passes_and_is_named() {
        let report = check_bootloader(shipped_bytes(), &X4).unwrap();
        assert_eq!(report.identity, BootloaderIdentity::Shipped(&SHIPPED[0]));
        assert_eq!(report.warnings, []);
        assert_eq!(
            report.identity.to_string(),
            "the ESP-IDF v5.5.2 bootloader inkemu ships (esp-idf-v5.5.2-esp32c3-dio)"
        );
    }

    #[test]
    fn another_bootloader_is_named_by_its_descriptor_or_its_hash() {
        let built = build_image(&bootloader_spec());
        let report = check_bootloader(&built, &X4).unwrap();
        let sha256 = sha256_hex(&built);
        assert_eq!(
            report.identity,
            BootloaderIdentity::Described {
                idf_ver: "v5.5.2-test".into(),
                date_time: "Oct 13 2026 10:00:00".into(),
                sha256: sha256.clone()
            }
        );
        assert_eq!(
            report.identity.to_string(),
            format!(
                "a bootloader of ESP-IDF v5.5.2-test built Oct 13 2026 10:00:00, sha256 {sha256}"
            )
        );
        let mut spec = bootloader_spec();
        spec.segments[0].1[0] = 0; // no descriptor
        let bare = build_image(&spec);
        assert_eq!(
            check_bootloader(&bare, &X4).unwrap().identity,
            BootloaderIdentity::Unknown {
                sha256: sha256_hex(&bare)
            }
        );
    }

    #[test]
    fn a_whole_flash_image_an_ota_update_or_a_long_file_is_not_a_bootloader() {
        assert_eq!(
            refusal(&build_flash_image(&bootloader_spec())),
            BootloaderError::FlashImage
        );
        let mut xota = b"XOTA".to_vec();
        xota.resize(512, 0);
        assert_eq!(refusal(&xota), BootloaderError::EncryptedOta);
        let mut long = build_image(&bootloader_spec());
        long.resize(0x8001, 0xFF);
        assert_eq!(refusal(&long), BootloaderError::TooLong { len: 0x8001 });
        long.truncate(0x8000);
        assert!(
            check_bootloader(&long, &X4).is_ok(),
            "erased bytes up to the table"
        );
        assert_eq!(refusal(&[]), BootloaderError::TooShort { len: 0 });
        assert_eq!(refusal(&[0xE9; 23]), BootloaderError::TooShort { len: 23 });
        assert_eq!(
            refusal(&[0x7F; 64]),
            BootloaderError::NotAnImage { found: 0x7F }
        );
    }

    #[test]
    fn an_application_longer_than_a_bootloader_can_be_is_named_as_one() {
        // A real application is far longer than the 0x8000 bytes below the partition table.
        let mut spec = ImageSpec::default();
        spec.segments.push((0x3C01_0000, vec![0; 0x9000]));
        let application = build_image(&spec);
        assert!(application.len() > 0x8000);
        let err = refusal(&application);
        assert_eq!(err, BootloaderError::Application);
        assert!(
            err.to_string()
                .contains("; use it as the firmware (`--firmware`)"),
            "{err}"
        );
        // Without its descriptor, or cut inside it, its length is all that is known.
        let mut cut = application.clone();
        cut[0x20] ^= 1;
        assert_eq!(
            refusal(&cut),
            BootloaderError::TooLong {
                len: application.len()
            }
        );
        let mut short_segment = application;
        short_segment[28..32].copy_from_slice(&255u32.to_le_bytes());
        assert_eq!(
            refusal(&short_segment),
            BootloaderError::TooLong {
                len: short_segment.len()
            }
        );
        // A dump whose table does not parse starts with a bootloader, not an application.
        let mut dump = build_flash_image(&bootloader_spec());
        dump[0x8010] ^= 1;
        assert_eq!(refusal(&dump), BootloaderError::TooLong { len: dump.len() });
    }

    #[test]
    fn an_application_given_as_the_bootloader_is_named_as_one() {
        assert_eq!(
            refusal(&build_image(&ImageSpec::default())),
            BootloaderError::Application
        );
        // Without its descriptor, its flash-mapped segment still gives it away.
        let spec = ImageSpec {
            segments: vec![(0x4200_0020, vec![0; 8])],
            ..ImageSpec::default()
        };
        assert_eq!(
            refusal(&build_image(&spec)),
            BootloaderError::FlashMapped {
                index: 0,
                addr: 0x4200_0020
            }
        );
    }

    #[test]
    fn a_segment_the_rom_would_not_load_is_refused() {
        for (addr, len) in [
            (0x3FCD_E15C, 8),    // ends inside the ROM's stack
            (0x3FCD_F660, 8),    // starts inside it
            (0x5000_0000, 8),    // RTC memory
            (0x3FCD_FFFC, 0x8),  // runs past the end of DRAM
            (0x403D_FFF8, 0x10), // runs past the end of IRAM
            (0x3FC8_0002, 8),    // not word-aligned
            (0x3FC7_FFFC, 8),    // starts below DRAM
            (0x4037_BFFC, 8),    // starts below IRAM
        ] {
            let mut spec = bootloader_spec();
            spec.segments.push((addr, vec![0; len]));
            let index = spec.segments.len() - 1;
            assert_eq!(
                refusal(&build_image(&spec)),
                BootloaderError::Unloadable {
                    index,
                    addr,
                    len,
                    rom_reserved: (0x3FCD_E160, 0x3FCD_F664)
                },
                "{addr:#x}+{len:#x}"
            );
        }
        let mut spec = bootloader_spec();
        spec.segments.push((0x5000_0000, vec![0; 8]));
        assert_eq!(
            refusal(&build_image(&spec)).to_string(),
            "segment 2 of the bootloader (8 bytes at 0x50000000) is not where the mask ROM loads a bootloader (word-aligned, at most 1 MiB, in internal RAM, clear of 0x3fcde160-0x3fcdf663); rebuild it, or use the one inkemu ships"
        );
    }

    #[test]
    fn a_segment_at_either_end_of_ram_is_loaded() {
        for (addr, len) in [
            (0x3FC8_0000, 8), // the start of DRAM
            (0x3FCD_FFF8, 8), // the end of DRAM
            (0x4037_C000, 8), // the start of IRAM
            (0x403D_FFF8, 8), // the end of IRAM
        ] {
            let mut spec = bootloader_spec();
            spec.segments.push((addr, vec![0; len]));
            assert!(
                check_bootloader(&build_image(&spec), &X4).is_ok(),
                "{addr:#x}+{len:#x}"
            );
        }
    }

    #[test]
    fn a_segment_in_either_window_of_the_flash_is_an_applications() {
        for addr in [0x3C00_0020, 0x4200_0020] {
            let mut spec = bootloader_spec();
            spec.segments.push((addr, vec![0; 8]));
            assert_eq!(
                refusal(&build_image(&spec)),
                BootloaderError::FlashMapped { index: 2, addr },
                "{addr:#x}"
            );
        }
    }

    #[test]
    fn every_speed_code_the_rom_accepts_is_accepted() {
        for speed in [0, 1, 2, 0xF] {
            let mut bytes = build_image(&bootloader_spec());
            bytes[3] = (bytes[3] & 0xF0) | speed;
            refresh_digest(&mut bytes);
            assert!(check_bootloader(&bytes, &X4).is_ok(), "speed {speed}");
        }
    }

    #[test]
    fn a_header_the_rom_or_the_device_would_refuse_is_refused() {
        let with = |at: usize, value: u8| {
            let mut bytes = build_image(&bootloader_spec());
            bytes[at] = value;
            refresh_digest(&mut bytes);
            bytes
        };
        assert_eq!(refusal(&with(2, 4)), BootloaderError::BadMode { raw: 4 });
        let err = refusal(&with(3, 0x43));
        assert_eq!(
            err,
            BootloaderError::BadSpeed {
                raw: 3,
                accepted: &[0, 1, 2, 0xF]
            }
        );
        assert_eq!(
            err.to_string(),
            "the bootloader declares flash speed code 3, which the mask ROM refuses (0, 1, 2 or 15); rebuild it, or use the one inkemu ships"
        );
        assert!(matches!(
            refusal(&with(1, 17)),
            BootloaderError::Damaged(FirmwareError::TooManySegments { .. })
        ));
        assert_eq!(
            refusal(&with(12, 9)),
            BootloaderError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3
            }
        );
        let err = refusal(&with(3, 0x5F));
        assert_eq!(
            err,
            BootloaderError::LargerFlash {
                header: 32,
                device: 16
            }
        );
        assert_eq!(
            err.to_string(),
            "the header of the bootloader says 32 MB of flash and the device has 16 MB: the application would abort when it finds the smaller chip; give a bootloader built for 16 MB, or no bootloader to use the one inkemu ships"
        );
    }

    /// A bootloader laid out as ESP-IDF v5.5 links one for the ESP32-S3: its descriptor at
    /// 0x3FCE2700 (`bootloader_usable_dram_end` 0x3FCE9700 in its `bootloader.ld`), its code at
    /// `iram_loader_seg_start` 0x403CB700. Both lie outside the RAM of an ESP32-C3.
    fn esp32s3_bootloader() -> Vec<u8> {
        build_image(&ImageSpec {
            chip: 9,
            entry: 0x403C_B700,
            segments: vec![
                (
                    0x3FCE_2700,
                    crate::builder::bootloader_desc_bytes("v5.5.2", ""),
                ),
                (0x403C_B700, vec![0x6F, 0, 0, 0]),
            ],
            ..ImageSpec::default()
        })
    }

    #[test]
    fn a_bootloader_for_another_chip_is_refused_for_its_chip_not_for_its_addresses() {
        let err = refusal(&esp32s3_bootloader());
        assert_eq!(
            err,
            BootloaderError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3
            }
        );
        assert_eq!(
            err.to_string(),
            "the bootloader is built for ESP32-S3, and the device uses ESP32-C3; give a bootloader built for ESP32-C3, or no bootloader to use the one inkemu ships"
        );
    }

    #[test]
    fn a_bootloader_for_a_chip_without_a_memory_map_is_refused_for_its_chip() {
        let esp32s3 = Device {
            chip: ChipId::Esp32S3,
            ..X4
        };
        assert!(memory_map(ChipId::Esp32S3).is_none());
        let err = check_bootloader(&esp32s3_bootloader(), &esp32s3).unwrap_err();
        assert_eq!(
            err,
            BootloaderError::UnmodelledChip {
                chip: ChipId::Esp32S3
            }
        );
        assert_eq!(
            err.to_string(),
            "the device uses ESP32-S3, and inkemu does not know where the mask ROM of ESP32-S3 loads a bootloader; choose a device whose chip inkemu models"
        );
    }

    #[test]
    fn every_modelled_chip_has_one_memory_map() {
        for map in &MEMORY_MAPS {
            assert_eq!(memory_map(map.chip), Some(map), "{}", map.chip);
            // `MemoryMap::loads` relies on it for the ROM's 1 MiB limit on a segment.
            assert!(
                map.ram.iter().all(|&(start, end)| end - start <= 0x10_0000),
                "{}",
                map.chip
            );
            // `MemoryMap::loads_only_into_ram` relies on it: RAM is never a window of the flash.
            for &(start, end) in map.ram {
                assert!(
                    !map.in_flash_window(start, (end - start) as usize),
                    "{}",
                    map.chip
                );
            }
        }
        for entry in &SHIPPED {
            assert!(memory_map(entry.chip).is_some(), "{}", entry.id);
        }
    }

    #[test]
    fn a_bootloader_that_requires_a_later_chip_is_refused_before_it_resets_for_ever() {
        let mut bytes = build_image(&bootloader_spec());
        bytes[15..17].copy_from_slice(&4u16.to_le_bytes());
        refresh_digest(&mut bytes);
        let err = refusal(&bytes);
        assert_eq!(
            err,
            BootloaderError::RevisionTooHigh {
                required: 4,
                device: 3,
                option: "CONFIG_ESP32C3_REV_MIN"
            }
        );
        assert_eq!(
            err.to_string(),
            "the bootloader requires chip revision v0.4 or later and the device is v0.3: it would reset the chip for ever; give a bootloader built for v0.3 (CONFIG_ESP32C3_REV_MIN), or choose a device of revision v0.4 or later"
        );
        bytes[15..17].copy_from_slice(&3u16.to_le_bytes());
        refresh_digest(&mut bytes);
        assert!(check_bootloader(&bytes, &X4).is_ok(), "v0.3 on v0.3");
    }

    #[test]
    fn a_smaller_flash_and_a_wrong_digest_are_warnings() {
        let mut bytes = build_image(&bootloader_spec());
        bytes[3] = 0x2F;
        refresh_digest(&mut bytes);
        assert_eq!(
            check_bootloader(&bytes, &X4).unwrap().warnings,
            [BootloaderWarning::SmallerFlash {
                header: 4,
                device: 16
            }]
        );
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(
            check_bootloader(&bytes, &X4).unwrap().warnings,
            [
                BootloaderWarning::SmallerFlash {
                    header: 4,
                    device: 16
                },
                BootloaderWarning::DigestMismatch
            ]
        );
        // A checksum is never a warning: the ROM refuses the image.
        let spec = ImageSpec {
            hash_appended: false,
            ..bootloader_spec()
        };
        let mut bytes = build_image(&spec);
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            refusal(&bytes),
            BootloaderError::Damaged(FirmwareError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn the_shipped_bootloader_read_from_the_start_of_a_dump_is_named() {
        // The first 0x8000 bytes of a flash dump: the bootloader, then erased bytes.
        let mut padded = shipped_bytes().to_vec();
        padded.resize(0x8000, 0xFF);
        let report = check_bootloader(&padded, &X4).unwrap();
        assert_eq!(report.identity, BootloaderIdentity::Shipped(&SHIPPED[0]));
        // Another bootloader is named by the hash of its image, not of the file.
        let built = build_image(&bootloader_spec());
        let mut padded = built.clone();
        padded.resize(0x8000, 0xFF);
        assert!(matches!(
            check_bootloader(&padded, &X4).unwrap().identity,
            BootloaderIdentity::Described { sha256, .. } if sha256 == sha256_hex(&built)
        ));
    }

    #[test]
    fn a_known_bootloader_is_named() {
        assert_eq!(
            KNOWN[0].sha256,
            "6e6b0d386095f0783e9f0513ea29ea2ce413790e7f7153a06a2b04e2b5f59fb6"
        );
        let identity = BootloaderIdentity::Known(&KNOWN[0]);
        assert!(identity.to_string().contains("Arduino-ESP32"), "{identity}");
        // A bootloader whose hash is in the table of known ones is named by it, padded or not.
        let built = build_image(&bootloader_spec());
        let known: &'static [KnownBootloader] = Box::leak(Box::new([KnownBootloader {
            name: "a test bootloader",
            sha256: Box::leak(sha256_hex(&built).into_boxed_str()),
        }]));
        let mut padded = built.clone();
        padded.resize(0x8000, 0xFF);
        for bytes in [&built, &padded] {
            let image = FirmwareImage::parse(bytes).unwrap();
            assert_eq!(
                identify_in(bytes, &image, &SHIPPED, known),
                BootloaderIdentity::Known(&known[0])
            );
            assert!(matches!(
                identify_in(bytes, &image, &SHIPPED, &KNOWN),
                BootloaderIdentity::Described { .. }
            ));
        }
        // A descriptor with an application's magic is an application, whatever else it holds.
        let mut spec = bootloader_spec();
        spec.segments[0].1 = app_desc_bytes("1", "demo", "v5.5.2");
        assert_eq!(refusal(&build_image(&spec)), BootloaderError::Application);
    }

    #[test]
    fn every_refusal_and_warning_states_a_remedy() {
        let errors = [
            BootloaderError::EncryptedOta,
            BootloaderError::FlashImage,
            BootloaderError::TooLong { len: 1 },
            BootloaderError::TooShort { len: 1 },
            BootloaderError::NotAnImage { found: 1 },
            BootloaderError::Application,
            BootloaderError::BadMode { raw: 4 },
            BootloaderError::BadSpeed {
                raw: 3,
                accepted: &[0, 1, 2, 0xF],
            },
            BootloaderError::Damaged(FirmwareError::HashMismatch),
            BootloaderError::FlashMapped { index: 0, addr: 0 },
            BootloaderError::Unloadable {
                index: 0,
                addr: 0,
                len: 0,
                rom_reserved: (0x3FCD_E160, 0x3FCD_F664),
            },
            BootloaderError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3,
            },
            BootloaderError::UnmodelledChip {
                chip: ChipId::Esp32S3,
            },
            BootloaderError::RevisionTooHigh {
                required: 4,
                device: 3,
                option: "CONFIG_ESP32C3_REV_MIN",
            },
            BootloaderError::LargerFlash {
                header: 32,
                device: 16,
            },
        ];
        let verbs: Vec<&str> = BOOTLOADER_REMEDY_VERBS
            .iter()
            .chain(crate::error::REMEDY_VERBS.iter())
            .copied()
            .collect();
        for message in errors.iter().map(ToString::to_string).chain(
            [
                BootloaderWarning::DigestMismatch,
                BootloaderWarning::SmallerFlash {
                    header: 4,
                    device: 16,
                },
            ]
            .iter()
            .map(ToString::to_string),
        ) {
            assert!(
                verbs
                    .iter()
                    .any(|verb| message.contains(&format!("; {verb} "))),
                "no remedy in: {message}"
            );
        }
    }
}
