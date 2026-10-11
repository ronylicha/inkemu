//! ESP-IDF partition tables, bit-compatible with `gen_esp32part.py`.
//!
//! Every error message states the cause and a correction ("<cause>; <imperative remedy>").
use crate::chip::FlashSize;
use md5::{Digest, Md5};
use thiserror::Error;

pub const PARTITION_TABLE_OFFSET: u32 = 0x8000;
pub const PARTITION_TABLE_LEN: usize = 0xC00;
const ENTRY_LEN: usize = 32;
const ENTRY_MAGIC: [u8; 2] = [0xAA, 0x50];
const MD5_MAGIC: [u8; 2] = [0xEB, 0xEB];
const ERASED: u8 = 0xFF;
const NAME_LEN: usize = 16;
/// One slot is reserved for the MD5 entry.
const MAX_ENTRIES: usize = PARTITION_TABLE_LEN / ENTRY_LEN - 1;
const APP_ALIGN: u32 = 0x10000;
const DATA_ALIGN: u32 = 0x1000;

pub const TYPE_APP: u8 = 0x00;
pub const TYPE_DATA: u8 = 0x01;
pub const SUBTYPE_FACTORY: u8 = 0x00;
pub const SUBTYPE_OTA_0: u8 = 0x10;
pub const SUBTYPE_OTA_1: u8 = 0x11;
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

#[derive(Debug, Error, PartialEq, Eq)]
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
        "partitions `{first}` and `{second}` overlap; move one of them so their ranges no longer intersect"
    )]
    Overlap { first: String, second: String },
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
        for p in &self.entries {
            if p.name.len() >= NAME_LEN || !p.name.is_ascii() {
                return Err(PartitionError::NameTooLong {
                    name: p.name.clone(),
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
    fn detects_corrupted_md5() {
        let mut b = golden();
        b[40] ^= 1; // inside entry 1
        assert_eq!(PartitionTable::parse(&b), Err(PartitionError::Md5Mismatch));
    }

    fn table(entries: Vec<(&str, u8, u32, u32)>) -> PartitionTable {
        PartitionTable {
            entries: entries
                .into_iter()
                .map(|(n, t, o, s)| Partition {
                    name: n.into(),
                    ptype: t,
                    subtype: 0,
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
    const REMEDY_VERBS: [&str; 6] = ["add", "remove", "shorten", "move", "shrink", "use"];

    #[test]
    fn every_variant_message_states_a_remedy() {
        let errors = [
            PartitionError::Empty,
            PartitionError::TooManyEntries { count: 99 },
            PartitionError::NameTooLong { name: "n".into() },
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
            PartitionError::Overlap {
                first: "a".into(),
                second: "b".into(),
            },
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

    #[test]
    fn rejects_too_many_entries_and_non_ascii_names() {
        let many = PartitionTable {
            entries: (0..=MAX_ENTRIES)
                .map(|i| Partition {
                    name: format!("p{i}"),
                    ptype: TYPE_DATA,
                    subtype: 0,
                    offset: 0x1000 * u32::try_from(i).unwrap(),
                    size: 0x1000,
                    flags: 0,
                })
                .collect(),
        };
        assert_eq!(
            many.to_bytes(FlashSize::Mb16),
            Err(PartitionError::TooManyEntries {
                count: MAX_ENTRIES + 1
            })
        );
        assert!(matches!(
            table(vec![("caf\u{e9}", TYPE_DATA, 0x9000, 0x1000)]).to_bytes(FlashSize::Mb4),
            Err(PartitionError::NameTooLong { .. })
        ));
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
