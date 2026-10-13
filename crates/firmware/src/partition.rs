//! ESP-IDF partition tables, bit-compatible with `gen_esp32part.py`.
//!
//! A table is written only when `gen_esp32part.py` would accept it and ESP-IDF's bootloader would
//! find its application where inkemu writes it: two application rows of one subtype, which the
//! script accepts, are refused too.
//!
//! Every error message states the cause and a correction ("<cause>; <imperative remedy>").
use crate::chip::FlashSize;
use md5::{Digest, Md5};
use thiserror::Error;

pub const PARTITION_TABLE_OFFSET: u32 = 0x8000;
pub const PARTITION_TABLE_LEN: usize = 0xC00;
const ENTRY_LEN: usize = 32;
/// First two bytes of every partition table entry.
pub(crate) const ENTRY_MAGIC: [u8; 2] = [0xAA, 0x50];
const MD5_MAGIC: [u8; 2] = [0xEB, 0xEB];
const ERASED: u8 = 0xFF;
const NAME_LEN: usize = 16;
/// Entries plus the MD5 entry must stay strictly below [`PARTITION_TABLE_LEN`]
/// (`gen_esp32part.py` `to_binary`: `len(result) >= MAX_PARTITION_LENGTH` is an error, so the
/// table is at most 95 entries long including the MD5 entry, i.e. 94 user partitions).
const MAX_ENTRIES: usize = PARTITION_TABLE_LEN / ENTRY_LEN - 2;
/// Size of the sector holding the partition table (`gen_esp32part.py` `PARTITION_TABLE_SIZE`).
const PARTITION_TABLE_SECTOR: u32 = 0x1000;
/// Lowest offset a partition may start at: the end of the partition-table sector
/// (`gen_esp32part.py` `verify`: `p.offset < offset_part_table + PARTITION_TABLE_SIZE` is an error).
pub const FIRST_PARTITION_OFFSET: u32 = PARTITION_TABLE_OFFSET + PARTITION_TABLE_SECTOR;
/// The one size of an otadata partition: two 4 KiB sectors, one per OTA entry
/// (`gen_esp32part.py`, ESP-IDF v5.5.5, `PartitionTable.verify`).
const OTADATA_SIZE: u32 = 0x2000;
const APP_ALIGN: u32 = 0x10000;
const DATA_ALIGN: u32 = 0x1000;

pub const TYPE_APP: u8 = 0x00;
pub const TYPE_DATA: u8 = 0x01;
pub const SUBTYPE_FACTORY: u8 = 0x00;
pub const SUBTYPE_OTA_0: u8 = 0x10;
pub const SUBTYPE_OTA_1: u8 = 0x11;
pub const SUBTYPE_TEST: u8 = 0x20;
/// The 16 OTA slots of an application are subtypes `0x10` (`ota_0`) to `0x1F` (`ota_15`).
const OTA_SLOTS: u8 = 16;
pub const SUBTYPE_DATA_OTA: u8 = 0x00;
pub const SUBTYPE_NVS: u8 = 0x02;
pub const SUBTYPE_COREDUMP: u8 = 0x03;
pub const SUBTYPE_SPIFFS: u8 = 0x82;

/// One entry of the partition table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    pub name: String,
    pub ptype: u8,
    pub subtype: u8,
    pub offset: u32,
    pub size: u32,
    /// Bit 0: encrypted, bit 1: read-only.
    pub flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionTable {
    pub entries: Vec<Partition>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PartitionError {
    #[error("the partition table is empty; add at least one partition")]
    Empty,
    #[error(
        "{count} partitions exceed the maximum of {MAX_ENTRIES}; remove partitions until at most {MAX_ENTRIES} remain"
    )]
    TooManyEntries { count: usize },
    #[error(
        "partition name `{name}` is longer than 15 bytes or not ASCII; shorten the name to 15 ASCII characters or fewer"
    )]
    NameTooLong { name: String },
    /// `row` counts the partitions of the table from 1.
    #[error(
        "partition {row} of the table has an empty name; give it a name of 1 to 15 ASCII characters"
    )]
    EmptyName { row: usize },
    /// The table stores a name up to its first NUL: the bootloader, and [`PartitionTable::parse`],
    /// would read a shorter name than the one written.
    #[error(
        "partition name `{}` holds a NUL byte, where the name stored in the table would end; remove the NUL from the name",
        name.escape_debug()
    )]
    NulInName { name: String },
    #[error(
        "partition `{name}` starts at {offset:#x}, which is not aligned to {required:#x}; move it to a multiple of {required:#x}"
    )]
    Misaligned {
        name: String,
        offset: u32,
        required: u32,
    },
    #[error(
        "partition `{name}` ends at {end:#x}, beyond the {flash_bytes:#x}-byte flash; shrink it or choose a larger flash size"
    )]
    BeyondFlash {
        name: String,
        end: u64,
        flash_bytes: u64,
    },
    #[error(
        "partition `{name}` starts at {offset:#x}, inside the bootloader and partition-table area; move it to {minimum:#x} or above"
    )]
    BelowTableEnd {
        name: String,
        offset: u32,
        minimum: u32,
    },
    #[error("two partitions are named `{name}`; rename one of them so every name is unique")]
    DuplicateName { name: String },
    #[error(
        "partitions `{first}` and `{second}` overlap; move one of them so their ranges no longer intersect"
    )]
    Overlap { first: String, second: String },
    /// ESP-IDF's bootloader keeps the last row of each application subtype (`bootloader_utility.c`
    /// of v5.5.1, `bootloader_utility_load_partition_table`, lines 178 and 195: `bs->factory` and
    /// `bs->ota[...]` are assigned on every row), while the application is written to the first:
    /// the chip would start an erased slot and reset for ever (measured with the bootloader inkemu
    /// ships, v5.5.2: "No bootable app partitions in the partition table" for two `ota_0`,
    /// "Factory app partition is not bootable" for two `factory`).
    #[error(
        "partitions `{first}` and `{second}` are both the {} application slot, and the bootloader keeps only the last of them; give each application partition a subtype of its own",
        app_subtype_name(*subtype)
    )]
    DuplicateAppSubtype {
        first: String,
        second: String,
        subtype: u8,
    },
    /// `gen_esp32part.py` (ESP-IDF v5.5.5, `PartitionTable.verify`, line 336) refuses a second
    /// otadata partition; the bootloader would read the last one only (`bootloader_utility.c` of
    /// v5.5.1, line 207).
    #[error(
        "partitions `{first}` and `{second}` are both otadata (type data, subtype ota), and the bootloader reads only the last of them; remove one of them"
    )]
    DuplicateOtadata { first: String, second: String },
    /// `gen_esp32part.py` (ESP-IDF v5.5.5, `PartitionTable.verify`, line 343) requires 0x2000: two
    /// 4 KiB sectors, one per OTA entry. A smaller one makes the bootloader refuse to boot
    /// (`bootloader_utility.c` of v5.5.1, `bootloader_common_read_otadata`, lines 100-103; measured
    /// with the bootloader inkemu ships, v5.5.2: "`ota_info` partition size 4096 is too small
    /// (minimum 8192 bytes)", then a reset loop).
    #[error(
        "the otadata partition `{name}` is {size:#x} bytes, not the 0x2000 bytes of its two 4 KiB entries; set its size to 0x2000"
    )]
    OtadataSize { name: String, size: u32 },
    /// The table has no slot for [`PartitionTable::application_slot`]. [`PartitionTable::to_bytes`]
    /// does not refuse it, a table of data partitions being valid; a device profile does.
    #[error(
        "the table has neither a factory application nor an ota_0 slot, and inkemu writes the application to the factory slot, else to ota_0; add an app partition of subtype factory or ota_0"
    )]
    NoApplicationSlot,
    #[error(
        "entry {index} of the partition table is not a valid entry; use a table read from offset 0x8000 of a valid flash dump"
    )]
    BadEntryMagic { index: usize },
    #[error(
        "the partition table MD5 does not match its entries, so the table is corrupted; re-download the flash dump or restore the table from a known-good copy"
    )]
    Md5Mismatch,
}

impl PartitionTable {
    fn validate(&self, flash: FlashSize) -> Result<(), PartitionError> {
        if self.entries.is_empty() {
            return Err(PartitionError::Empty);
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(PartitionError::TooManyEntries {
                count: self.entries.len(),
            });
        }
        for (index, p) in self.entries.iter().enumerate() {
            if p.name.is_empty() {
                return Err(PartitionError::EmptyName { row: index + 1 });
            }
            if p.name.contains('\0') {
                return Err(PartitionError::NulInName {
                    name: p.name.clone(),
                });
            }
            if p.name.len() >= NAME_LEN || !p.name.is_ascii() {
                return Err(PartitionError::NameTooLong {
                    name: p.name.clone(),
                });
            }
            if p.offset < FIRST_PARTITION_OFFSET {
                return Err(PartitionError::BelowTableEnd {
                    name: p.name.clone(),
                    offset: p.offset,
                    minimum: FIRST_PARTITION_OFFSET,
                });
            }
            let required = if p.ptype == TYPE_APP {
                APP_ALIGN
            } else {
                DATA_ALIGN
            };
            if p.offset % required != 0 {
                return Err(PartitionError::Misaligned {
                    name: p.name.clone(),
                    offset: p.offset,
                    required,
                });
            }
            let end = u64::from(p.offset) + u64::from(p.size);
            if end > flash.bytes() {
                return Err(PartitionError::BeyondFlash {
                    name: p.name.clone(),
                    end,
                    flash_bytes: flash.bytes(),
                });
            }
        }
        let mut names: Vec<&str> = self.entries.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(PartitionError::DuplicateName {
                name: pair[0].to_owned(),
            });
        }
        let mut sorted: Vec<&Partition> = self.entries.iter().collect();
        sorted.sort_by_key(|p| p.offset);
        for pair in sorted.windows(2) {
            if u64::from(pair[0].offset) + u64::from(pair[0].size) > u64::from(pair[1].offset) {
                return Err(PartitionError::Overlap {
                    first: pair[0].name.clone(),
                    second: pair[1].name.clone(),
                });
            }
        }
        self.validate_boot_roles()
    }

    /// The rows the bootloader looks up by subtype: one per application subtype, one otadata of
    /// 0x2000 bytes. The table has at most [`MAX_ENTRIES`] rows, which bounds the pairs compared.
    fn validate_boot_roles(&self) -> Result<(), PartitionError> {
        let is_otadata = |p: &Partition| p.ptype == TYPE_DATA && p.subtype == SUBTYPE_DATA_OTA;
        for (index, first) in self.entries.iter().enumerate() {
            let later = &self.entries[index + 1..];
            if first.ptype == TYPE_APP
                && let Some(second) = later
                    .iter()
                    .find(|p| p.ptype == TYPE_APP && p.subtype == first.subtype)
            {
                return Err(PartitionError::DuplicateAppSubtype {
                    first: first.name.clone(),
                    second: second.name.clone(),
                    subtype: first.subtype,
                });
            }
            if is_otadata(first) {
                if let Some(second) = later.iter().find(|p| is_otadata(p)) {
                    return Err(PartitionError::DuplicateOtadata {
                        first: first.name.clone(),
                        second: second.name.clone(),
                    });
                }
                if first.size != OTADATA_SIZE {
                    return Err(PartitionError::OtadataSize {
                        name: first.name.clone(),
                        size: first.size,
                    });
                }
            }
        }
        Ok(())
    }

    /// Serializes the table exactly as `gen_esp32part.py` does: entries, MD5 entry, `0xFF` padding.
    ///
    /// # Errors
    /// Any [`PartitionError`] from layout validation.
    pub fn to_bytes(&self, flash: FlashSize) -> Result<Vec<u8>, PartitionError> {
        self.validate(flash)?;
        let mut out = Vec::with_capacity(PARTITION_TABLE_LEN);
        for p in &self.entries {
            out.extend(ENTRY_MAGIC);
            out.extend([p.ptype, p.subtype]);
            out.extend(p.offset.to_le_bytes());
            out.extend(p.size.to_le_bytes());
            let mut name = [0u8; NAME_LEN];
            name[..p.name.len()].copy_from_slice(p.name.as_bytes());
            out.extend(name);
            out.extend(p.flags.to_le_bytes());
        }
        let digest = Md5::digest(&out);
        out.extend(MD5_MAGIC);
        out.extend([ERASED; 14]);
        out.extend(digest);
        out.resize(PARTITION_TABLE_LEN, ERASED);
        Ok(out)
    }

    /// The application the 2nd-stage bootloader starts when `otadata` is blank or absent: the
    /// factory application, else the OTA slot `ota_0` (ESP-IDF v5.5.2,
    /// `bootloader_utility.c`, `bootloader_utility_get_selected_boot_partition`: "Defaulting to
    /// factory image", else "No factory image, trying OTA 0").
    #[must_use]
    pub fn application_slot(&self) -> Option<&Partition> {
        let app = |subtype| {
            self.entries
                .iter()
                .find(|p| p.ptype == TYPE_APP && p.subtype == subtype)
        };
        app(SUBTYPE_FACTORY).or_else(|| app(SUBTYPE_OTA_0))
    }

    /// Reads a table back, verifying its MD5 entry when one is present.
    ///
    /// # Errors
    /// [`PartitionError::BadEntryMagic`] or [`PartitionError::Md5Mismatch`] for a corrupted
    /// table, [`PartitionError::Empty`] when no entry is found.
    pub fn parse(bytes: &[u8]) -> Result<PartitionTable, PartitionError> {
        let mut entries = Vec::new();
        for (index, e) in bytes.as_chunks::<ENTRY_LEN>().0.iter().enumerate() {
            match [e[0], e[1]] {
                ENTRY_MAGIC => entries.push(parse_entry(e)),
                MD5_MAGIC => {
                    let covered = &bytes[..index * ENTRY_LEN];
                    if Md5::digest(covered).as_slice() != &e[16..32] {
                        return Err(PartitionError::Md5Mismatch);
                    }
                    // Like the ESP-IDF bootloader, stop at the MD5 entry.
                    break;
                }
                [ERASED, ERASED] => break,
                _ => return Err(PartitionError::BadEntryMagic { index }),
            }
        }
        if entries.is_empty() {
            return Err(PartitionError::Empty);
        }
        Ok(PartitionTable { entries })
    }
}

/// How `gen_esp32part.py` writes an application subtype: `factory`, `test`, `ota_0`…`ota_15`, or
/// its value when it has no name.
fn app_subtype_name(subtype: u8) -> String {
    match subtype {
        SUBTYPE_FACTORY => "factory".to_owned(),
        SUBTYPE_TEST => "test".to_owned(),
        _ if subtype.wrapping_sub(SUBTYPE_OTA_0) < OTA_SLOTS => {
            format!("ota_{}", subtype - SUBTYPE_OTA_0)
        }
        _ => format!("{subtype:#04x}"),
    }
}

fn parse_entry(e: &[u8]) -> Partition {
    let name_end = e[12..28].iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    Partition {
        name: String::from_utf8_lossy(&e[12..12 + name_end]).into_owned(),
        ptype: e[2],
        subtype: e[3],
        offset: u32::from_le_bytes([e[4], e[5], e[6], e[7]]),
        size: u32::from_le_bytes([e[8], e[9], e[10], e[11]]),
        flags: u32::from_le_bytes([e[28], e[29], e[30], e[31]]),
    }
}

/// Stock Xteink X4 layout (hardware research §3.2; `CrossPoint` `partitions.csv`).
#[must_use]
pub fn xteink_x4_stock() -> PartitionTable {
    let p = |name: &str, ptype, subtype, offset, size| Partition {
        name: name.into(),
        ptype,
        subtype,
        offset,
        size,
        flags: 0,
    };
    PartitionTable {
        entries: vec![
            p("nvs", TYPE_DATA, SUBTYPE_NVS, 0x9000, 0x5000),
            p("otadata", TYPE_DATA, SUBTYPE_DATA_OTA, 0xE000, 0x2000),
            p("app0", TYPE_APP, SUBTYPE_OTA_0, 0x1_0000, 0x64_0000),
            p("app1", TYPE_APP, SUBTYPE_OTA_1, 0x65_0000, 0x64_0000),
            p("spiffs", TYPE_DATA, SUBTYPE_SPIFFS, 0xC9_0000, 0x36_0000),
            p("coredump", TYPE_DATA, SUBTYPE_COREDUMP, 0xFF_0000, 0x1_0000),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::FlashSize;

    // A test reads its fixture; the crate's code does no host I/O (`clippy.toml`).
    #[allow(clippy::disallowed_methods)]
    fn golden() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/partitions/x4-stock.bin"
        ))
        .unwrap()
    }

    #[test]
    fn x4_stock_matches_esp_idf_byte_for_byte() {
        assert_eq!(
            xteink_x4_stock().to_bytes(FlashSize::Mb16).unwrap(),
            golden()
        );
    }

    #[test]
    fn parses_esp_idf_output() {
        let t = PartitionTable::parse(&golden()).unwrap();
        assert_eq!(t, xteink_x4_stock());
        assert_eq!(t.entries[2].name, "app0");
        assert_eq!(
            (t.entries[2].ptype, t.entries[2].subtype),
            (TYPE_APP, SUBTYPE_OTA_0)
        );
    }

    #[test]
    fn the_application_slot_is_the_factory_one_else_ota_0() {
        assert_eq!(
            xteink_x4_stock()
                .application_slot()
                .map(|p| p.name.as_str()),
            Some("app0")
        );
        let mut with_factory = xteink_x4_stock();
        with_factory.entries[3].subtype = SUBTYPE_FACTORY;
        assert_eq!(
            with_factory.application_slot().map(|p| p.name.as_str()),
            Some("app1")
        );
        let mut ota_1_only = xteink_x4_stock();
        ota_1_only.entries.remove(2);
        assert_eq!(ota_1_only.application_slot(), None);
    }

    #[test]
    fn detects_corrupted_md5() {
        let mut b = golden();
        b[40] ^= 1; // inside entry 1
        assert_eq!(PartitionTable::parse(&b), Err(PartitionError::Md5Mismatch));
    }

    /// Rows of `(name, type, offset, size)`: an application is a factory one, a data partition a
    /// SPIFFS one, so that no row is otadata and no two rows share an application subtype unless
    /// a test sets it.
    fn table(entries: Vec<(&str, u8, u32, u32)>) -> PartitionTable {
        PartitionTable {
            entries: entries
                .into_iter()
                .map(|(n, t, o, s)| Partition {
                    name: n.into(),
                    ptype: t,
                    subtype: if t == TYPE_APP {
                        SUBTYPE_FACTORY
                    } else {
                        SUBTYPE_SPIFFS
                    },
                    offset: o,
                    size: s,
                    flags: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn rejects_invalid_layouts() {
        assert_eq!(
            PartitionTable { entries: vec![] }.to_bytes(FlashSize::Mb4),
            Err(PartitionError::Empty)
        );
        assert!(matches!(
            table(vec![("app", TYPE_APP, 0x11000, 0x10000)]).to_bytes(FlashSize::Mb4),
            Err(PartitionError::Misaligned {
                required: 0x10000,
                ..
            })
        ));
        assert!(matches!(
            table(vec![
                ("a", TYPE_DATA, 0x9000, 0x2000),
                ("b", TYPE_DATA, 0xA000, 0x1000)
            ])
            .to_bytes(FlashSize::Mb4),
            Err(PartitionError::Overlap { .. })
        ));
        assert!(matches!(
            table(vec![("big", TYPE_APP, 0x10000, 0x40_0000)]).to_bytes(FlashSize::Mb4),
            Err(PartitionError::BeyondFlash { .. })
        ));
        assert!(matches!(
            table(vec![("a_name_far_too_long", TYPE_DATA, 0x9000, 0x1000)])
                .to_bytes(FlashSize::Mb4),
            Err(PartitionError::NameTooLong { .. })
        ));
    }

    #[test]
    fn rejects_garbage_entry() {
        let mut b = golden();
        b[32] = 0x12; // magic of entry 1
        assert_eq!(
            PartitionTable::parse(&b),
            Err(PartitionError::BadEntryMagic { index: 1 })
        );
    }

    /// Imperatives that open the remedy clause of a message (`"; <verb> ..."`).
    const REMEDY_VERBS: [&str; 9] = [
        "add", "remove", "shorten", "move", "shrink", "use", "rename", "give", "set",
    ];

    #[test]
    fn every_variant_message_states_a_remedy() {
        let errors = [
            PartitionError::Empty,
            PartitionError::TooManyEntries { count: 99 },
            PartitionError::NameTooLong { name: "n".into() },
            PartitionError::EmptyName { row: 2 },
            PartitionError::NulInName { name: "n\0".into() },
            PartitionError::DuplicateAppSubtype {
                first: "a".into(),
                second: "b".into(),
                subtype: SUBTYPE_OTA_1,
            },
            PartitionError::DuplicateOtadata {
                first: "a".into(),
                second: "b".into(),
            },
            PartitionError::OtadataSize {
                name: "n".into(),
                size: 0x1000,
            },
            PartitionError::Misaligned {
                name: "n".into(),
                offset: 1,
                required: 0x1000,
            },
            PartitionError::BeyondFlash {
                name: "n".into(),
                end: 2,
                flash_bytes: 1,
            },
            PartitionError::BelowTableEnd {
                name: "n".into(),
                offset: 0,
                minimum: 0x9000,
            },
            PartitionError::DuplicateName { name: "n".into() },
            PartitionError::Overlap {
                first: "a".into(),
                second: "b".into(),
            },
            PartitionError::NoApplicationSlot,
            PartitionError::BadEntryMagic { index: 3 },
            PartitionError::Md5Mismatch,
        ];
        for err in errors {
            let msg = err.to_string();
            let has_remedy = REMEDY_VERBS
                .iter()
                .any(|verb| msg.contains(&format!("; {verb} ")))
                || msg.contains("; re-download ");
            assert!(has_remedy, "no remedy clause in: {msg}");
        }
    }

    /// `count` adjacent 4 KiB data partitions starting at the first legal offset.
    fn data_partitions(count: usize) -> PartitionTable {
        PartitionTable {
            entries: (0..count)
                .map(|i| Partition {
                    name: format!("p{i}"),
                    ptype: TYPE_DATA,
                    subtype: SUBTYPE_SPIFFS,
                    offset: FIRST_PARTITION_OFFSET + 0x1000 * u32::try_from(i).unwrap(),
                    size: 0x1000,
                    flags: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn entry_limit_matches_gen_esp32part() {
        // gen_esp32part.py to_binary rejects `len(result) >= 0xC00` with the MD5 entry included:
        // 94 partitions + MD5 = 95 * 32 = 0xBE0 bytes is the longest accepted table.
        assert_eq!(MAX_ENTRIES, 94);
        let at_limit = data_partitions(MAX_ENTRIES);
        let bytes = at_limit.to_bytes(FlashSize::Mb16).unwrap();
        assert_eq!(bytes.len(), PARTITION_TABLE_LEN);
        assert_eq!(PartitionTable::parse(&bytes).unwrap(), at_limit);
        assert_eq!(
            data_partitions(MAX_ENTRIES + 1).to_bytes(FlashSize::Mb16),
            Err(PartitionError::TooManyEntries {
                count: MAX_ENTRIES + 1
            })
        );
    }

    #[test]
    fn rejects_non_ascii_names() {
        assert!(matches!(
            table(vec![("caf\u{e9}", TYPE_DATA, 0x9000, 0x1000)]).to_bytes(FlashSize::Mb4),
            Err(PartitionError::NameTooLong { .. })
        ));
    }

    #[test]
    fn rejects_partitions_below_the_end_of_the_table_sector() {
        // gen_esp32part.py verify: offsets below 0x8000 + 0x1000 are an error.
        assert_eq!(FIRST_PARTITION_OFFSET, 0x9000);
        for offset in [0x0, 0x8000] {
            assert_eq!(
                table(vec![("early", TYPE_DATA, offset, 0x1000)]).to_bytes(FlashSize::Mb4),
                Err(PartitionError::BelowTableEnd {
                    name: "early".into(),
                    offset,
                    minimum: 0x9000
                })
            );
        }
        assert!(
            table(vec![("first", TYPE_DATA, 0x9000, 0x1000)])
                .to_bytes(FlashSize::Mb4)
                .is_ok()
        );
    }

    #[test]
    fn rejects_duplicate_names() {
        // gen_esp32part.py verify: "Partition names must be unique".
        assert_eq!(
            table(vec![
                ("nvs", TYPE_DATA, 0x9000, 0x1000),
                ("other", TYPE_DATA, 0xA000, 0x1000),
                ("nvs", TYPE_DATA, 0xB000, 0x1000),
            ])
            .to_bytes(FlashSize::Mb4),
            Err(PartitionError::DuplicateName { name: "nvs".into() })
        );
    }

    #[test]
    fn accepts_a_name_of_exactly_fifteen_characters() {
        let name = "a".repeat(NAME_LEN - 1);
        let t = table(vec![(name.as_str(), TYPE_DATA, 0x9000, 0x1000)]);
        let parsed = PartitionTable::parse(&t.to_bytes(FlashSize::Mb4).unwrap()).unwrap();
        assert_eq!(parsed.entries[0].name, name);
    }

    #[test]
    fn round_trips_flags_and_pads_to_table_length() {
        let mut t = xteink_x4_stock();
        t.entries[0].flags = 0b11;
        let bytes = t.to_bytes(FlashSize::Mb16).unwrap();
        assert_eq!(bytes.len(), PARTITION_TABLE_LEN);
        assert!(bytes[0xE0..].iter().all(|&b| b == ERASED));
        assert_eq!(PartitionTable::parse(&bytes).unwrap(), t);
    }

    #[test]
    fn two_application_rows_of_one_subtype_are_refused_by_name() {
        // ESP-IDF's bootloader keeps the last row of a subtype (`bootloader_utility.c`), the
        // application slot is the first: they would disagree on what boots.
        let mut two_ota_0 = xteink_x4_stock();
        two_ota_0.entries[3].subtype = SUBTYPE_OTA_0;
        assert_eq!(
            two_ota_0.to_bytes(FlashSize::Mb16),
            Err(PartitionError::DuplicateAppSubtype {
                first: "app0".into(),
                second: "app1".into(),
                subtype: SUBTYPE_OTA_0
            })
        );
        let mut two_factory = xteink_x4_stock();
        two_factory.entries[2].subtype = SUBTYPE_FACTORY;
        two_factory.entries[3].subtype = SUBTYPE_FACTORY;
        let err = two_factory.to_bytes(FlashSize::Mb16).unwrap_err();
        assert_eq!(
            err,
            PartitionError::DuplicateAppSubtype {
                first: "app0".into(),
                second: "app1".into(),
                subtype: SUBTYPE_FACTORY
            }
        );
        assert!(
            err.to_string()
                .starts_with("partitions `app0` and `app1` are both the factory application"),
            "{err}"
        );
        let mut two_test = xteink_x4_stock();
        two_test.entries[2].subtype = SUBTYPE_TEST;
        two_test.entries[3].subtype = SUBTYPE_TEST;
        assert!(matches!(
            two_test.to_bytes(FlashSize::Mb16),
            Err(PartitionError::DuplicateAppSubtype {
                subtype: SUBTYPE_TEST,
                ..
            })
        ));
        for (subtype, name) in [
            (SUBTYPE_OTA_0, "ota_0"),
            (0x1F, "ota_15"),
            (SUBTYPE_TEST, "test"),
            (0x30, "0x30"),
        ] {
            assert_eq!(app_subtype_name(subtype), name);
        }
        // A data row of the same subtype value is another kind of partition.
        let mut data_too = xteink_x4_stock();
        data_too.entries[2].subtype = SUBTYPE_FACTORY;
        assert_eq!(data_too.entries[1].subtype, SUBTYPE_FACTORY);
        assert!(data_too.to_bytes(FlashSize::Mb16).is_ok());
    }

    #[test]
    fn otadata_is_unique_and_two_sectors_long() {
        let mut small = xteink_x4_stock();
        small.entries[1].size = 0x1000;
        let err = small.to_bytes(FlashSize::Mb16).unwrap_err();
        assert_eq!(
            err,
            PartitionError::OtadataSize {
                name: "otadata".into(),
                size: 0x1000
            }
        );
        assert!(
            err.to_string().ends_with("; set its size to 0x2000"),
            "{err}"
        );
        let mut large = xteink_x4_stock();
        large.entries[0].size = 0x4000;
        large.entries[1].offset = 0xD000;
        large.entries[1].size = 0x3000;
        assert!(matches!(
            large.to_bytes(FlashSize::Mb16),
            Err(PartitionError::OtadataSize { size: 0x3000, .. })
        ));
        let mut two = xteink_x4_stock();
        two.entries[4].size -= 0x1_0000; // spiffs gives up its last 64 KiB
        two.entries.push(Partition {
            name: "otadata2".into(),
            ptype: TYPE_DATA,
            subtype: SUBTYPE_DATA_OTA,
            offset: 0xFE_0000,
            size: 0x2000,
            flags: 0,
        });
        assert_eq!(
            two.to_bytes(FlashSize::Mb16),
            Err(PartitionError::DuplicateOtadata {
                first: "otadata".into(),
                second: "otadata2".into()
            })
        );
    }

    #[test]
    fn a_name_is_neither_empty_nor_cut_by_a_nul() {
        assert_eq!(
            table(vec![("", TYPE_DATA, 0x9000, 0x1000)]).to_bytes(FlashSize::Mb4),
            Err(PartitionError::EmptyName { row: 1 })
        );
        let err = table(vec![
            ("nvs", TYPE_DATA, 0x9000, 0x1000),
            ("a\0b", TYPE_DATA, 0xA000, 0x1000),
        ])
        .to_bytes(FlashSize::Mb4)
        .unwrap_err();
        assert_eq!(
            err,
            PartitionError::NulInName {
                name: "a\0b".into()
            }
        );
        assert!(err.to_string().contains("`a\\0b`"), "{err}");
    }

    #[test]
    fn a_name_of_sixteen_bytes_leaves_no_room_for_its_nul() {
        let name = "a".repeat(NAME_LEN);
        assert_eq!(
            table(vec![(name.as_str(), TYPE_DATA, 0x9000, 0x1000)]).to_bytes(FlashSize::Mb4),
            Err(PartitionError::NameTooLong { name })
        );
    }

    #[test]
    fn a_partition_ends_at_the_end_of_the_flash_or_before() {
        let last_sector = 0x40_0000 - 0x1000;
        assert!(
            table(vec![("last", TYPE_DATA, last_sector, 0x1000)])
                .to_bytes(FlashSize::Mb4)
                .is_ok()
        );
        assert_eq!(
            table(vec![("last", TYPE_DATA, last_sector, 0x2000)]).to_bytes(FlashSize::Mb4),
            Err(PartitionError::BeyondFlash {
                name: "last".into(),
                end: 0x40_1000,
                flash_bytes: 0x40_0000
            })
        );
    }

    #[test]
    fn an_end_past_four_gib_is_computed_without_wrapping() {
        assert_eq!(
            table(vec![("top", TYPE_DATA, 0xFFFF_F000, 0x2000)]).to_bytes(FlashSize::Mb16),
            Err(PartitionError::BeyondFlash {
                name: "top".into(),
                end: 0x1_0000_1000,
                flash_bytes: 0x100_0000
            })
        );
    }

    #[test]
    fn a_data_partition_starts_on_a_four_kib_boundary() {
        assert_eq!(
            table(vec![("half", TYPE_DATA, 0x9800, 0x800)]).to_bytes(FlashSize::Mb4),
            Err(PartitionError::Misaligned {
                name: "half".into(),
                offset: 0x9800,
                required: 0x1000
            })
        );
    }

    #[test]
    fn parse_rejects_blank_flash_and_accepts_tables_without_md5() {
        assert_eq!(
            PartitionTable::parse(&[ERASED; PARTITION_TABLE_LEN]),
            Err(PartitionError::Empty)
        );
        let mut b = golden();
        // Entries only: erase the MD5 entry and everything after it.
        b[6 * ENTRY_LEN..].fill(ERASED);
        assert_eq!(PartitionTable::parse(&b).unwrap(), xteink_x4_stock());
    }
}
