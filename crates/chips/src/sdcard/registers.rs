//! The registers a card answers with (PLS 9.10 §5, research `sp2e-sd-card.md` §1 item 12): the
//! capacity and the CSD that tells it, the CID, the OCR, the SCR and the status of CMD6.
use super::crc::crc7_byte;

/// The largest standard-capacity card: what the CSD version 1.0 tells at most.
pub const SDSC_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// The unit of the capacity of a high-capacity card (CSD version 2.0).
pub const SDHC_UNIT_BYTES: u64 = 512 * 1024;
/// The largest `C_SIZE` of the CSD version 2.0: an SDXC card of 2 TiB − 128 MiB.
pub const MAX_C_SIZE: u32 = 0x3F_FEFF;
/// The largest card the SPI mode addresses.
pub const MAX_CAPACITY_BYTES: u64 = (MAX_C_SIZE as u64 + 1) * SDHC_UNIT_BYTES;

/// The card command classes the CSD declares: 0 (basic), 2 (block read), 4 (block write), 5
/// (erase), 8 (application specific), 10 (switch). Not 7 (lock), which the model does not carry
/// out (design 2e-D5).
pub const CARD_COMMAND_CLASSES: u16 = 0x535;

/// What the CSD gives as the access time and transfer rate: 1 ms (`0Eh`, fixed for SDHC) and
/// 25 MHz (`32h`). ASSUMPTION: the standard-capacity card gives the same access time.
const TAAC: u8 = 0x0E;
const TRAN_SPEED: u8 = 0x32;

/// The 2.7–3.6 V window of the OCR (bits 15 to 23).
const OCR_VOLTAGES: u32 = 0x00FF_8000;
const OCR_CCS: u32 = 1 << 30;
const OCR_POWERED_UP: u32 = 1 << 31;

/// The bytes of a block, as a `u64`.
const BLOCK: u64 = 512;

/// The kind of card a capacity makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CardKind {
    /// Standard capacity, up to 2 GiB: CSD version 1.0, addresses in bytes.
    Sdsc,
    /// High or extended capacity: CSD version 2.0, addresses in blocks of 512 bytes.
    Sdhc,
}

impl CardKind {
    /// What a card of this kind is called.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            CardKind::Sdsc => "SDSC",
            CardKind::Sdhc => "SDHC/SDXC",
        }
    }
}

/// Why a size cannot be the capacity of a card.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CapacityError {
    #[error("a card of 0 bytes holds nothing")]
    Empty,
    #[error("{bytes} bytes are not a whole number of 512-byte blocks")]
    NotWholeBlocks { bytes: u64 },
    #[error(
        "{bytes} bytes cannot be told by the CSD of a standard-capacity card (up to 2 GiB); a whole number of MiB always can"
    )]
    NotCodableSdsc { bytes: u64 },
    #[error(
        "{bytes} bytes cannot be told by the CSD of a high-capacity card (above 2 GiB); give a whole number of 512 KiB"
    )]
    NotCodableSdhc { bytes: u64 },
    #[error("{bytes} bytes are more than a card holds in SPI mode, {MAX_CAPACITY_BYTES} bytes")]
    TooLarge { bytes: u64 },
}

/// How the CSD tells a capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Size {
    /// CSD 1.0: (`c_size` + 1) × 2^(`c_size_mult` + 2) × 2^`read_bl_len` bytes.
    V1 {
        c_size: u16,
        c_size_mult: u8,
        read_bl_len: u8,
    },
    /// CSD 2.0: (`c_size` + 1) × 512 KiB.
    V2 { c_size: u32 },
}

/// The capacity of a card, and how its CSD tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Capacity {
    bytes: u64,
    size: Size,
}

impl Capacity {
    /// The capacity of a card of `bytes` bytes: standard up to 2 GiB, high above (design 2e-D6).
    ///
    /// # Errors
    /// [`CapacityError`] when the size is no card's: empty, not whole blocks, not told by the
    /// CSD, or too large.
    pub fn new(bytes: u64) -> Result<Capacity, CapacityError> {
        if bytes == 0 {
            return Err(CapacityError::Empty);
        }
        if !bytes.is_multiple_of(BLOCK) {
            return Err(CapacityError::NotWholeBlocks { bytes });
        }
        if bytes > MAX_CAPACITY_BYTES {
            return Err(CapacityError::TooLarge { bytes });
        }
        if bytes <= SDSC_MAX_BYTES {
            return Self::v1(bytes)
                .map(|size| Capacity { bytes, size })
                .ok_or(CapacityError::NotCodableSdsc { bytes });
        }
        if !bytes.is_multiple_of(SDHC_UNIT_BYTES) {
            return Err(CapacityError::NotCodableSdhc { bytes });
        }
        let c_size = u32::try_from(bytes / SDHC_UNIT_BYTES - 1)
            .map_err(|_| CapacityError::TooLarge { bytes })?;
        Ok(Capacity {
            bytes,
            size: Size::V2 { c_size },
        })
    }

    /// The fields of the CSD 1.0 that tell `bytes`, with the smallest block length and
    /// multiplier that do.
    fn v1(bytes: u64) -> Option<Size> {
        for read_bl_len in [9u8, 10] {
            let block = 1u64 << read_bl_len;
            if !bytes.is_multiple_of(block) {
                continue;
            }
            let blocks = bytes / block;
            for c_size_mult in 0u8..8 {
                let mult = 1u64 << (c_size_mult + 2);
                if blocks.is_multiple_of(mult) && (1..=4096).contains(&(blocks / mult)) {
                    let c_size = u16::try_from(blocks / mult - 1).ok()?;
                    return Some(Size::V1 {
                        c_size,
                        c_size_mult,
                        read_bl_len,
                    });
                }
            }
        }
        None
    }

    /// The capacity in bytes.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The capacity in blocks of 512 bytes.
    #[must_use]
    pub const fn blocks(&self) -> u64 {
        self.bytes / BLOCK
    }

    /// The kind of card.
    #[must_use]
    pub const fn kind(&self) -> CardKind {
        match self.size {
            Size::V1 { .. } => CardKind::Sdsc,
            Size::V2 { .. } => CardKind::Sdhc,
        }
    }

    /// The CSD of the card (PLS Tables 5-4 and 5-16), its CRC7 included.
    #[must_use]
    pub fn csd(&self, write_protected: bool) -> [u8; 16] {
        let mut csd = [0u8; 16];
        set(&mut csd, 119, 112, TAAC.into());
        set(&mut csd, 103, 96, TRAN_SPEED.into());
        set(&mut csd, 95, 84, CARD_COMMAND_CLASSES.into());
        set(&mut csd, 46, 46, 1); // ERASE_BLK_EN
        set(&mut csd, 45, 39, 0x7F); // SECTOR_SIZE
        set(&mut csd, 28, 26, 0b010); // R2W_FACTOR
        set(&mut csd, 12, 12, u64::from(write_protected)); // TMP_WRITE_PROTECT
        match self.size {
            Size::V1 {
                c_size,
                c_size_mult,
                read_bl_len,
            } => {
                set(&mut csd, 83, 80, read_bl_len.into());
                set(&mut csd, 79, 79, 1); // READ_BL_PARTIAL
                set(&mut csd, 73, 62, c_size.into());
                // VDD_R_CURR_MIN, VDD_R_CURR_MAX, VDD_W_CURR_MIN, VDD_W_CURR_MAX.
                // ASSUMPTION: 100 mA and 80 mA, as the cards of the time declared; nothing reads them.
                set(&mut csd, 61, 50, 0b111_110_111_110);
                set(&mut csd, 49, 47, c_size_mult.into());
                set(&mut csd, 25, 22, read_bl_len.into()); // WRITE_BL_LEN equals READ_BL_LEN
            }
            Size::V2 { c_size } => {
                set(&mut csd, 127, 126, 0b01); // CSD_STRUCTURE 2.0
                set(&mut csd, 83, 80, 9);
                set(&mut csd, 69, 48, c_size.into());
                set(&mut csd, 25, 22, 9);
            }
        }
        seal(&mut csd);
        csd
    }

    /// The capacity a CSD tells, by the formulas of PLS §5.3.2 and §5.3.3.
    #[must_use]
    pub fn told_by(csd: &[u8; 16]) -> u64 {
        if get(csd, 127, 126) == 1 {
            return (get(csd, 69, 48) + 1) * SDHC_UNIT_BYTES;
        }
        (get(csd, 73, 62) + 1) << (get(csd, 49, 47) + 2) << get(csd, 83, 80)
    }
}

/// Sets bits `high` down to `low` of a register, bit 127 the most significant of its first
/// byte, to the low bits of `value`.
fn set(register: &mut [u8], high: u32, low: u32, value: u64) {
    let top = register.len() * 8;
    for bit in low..=high {
        let Ok(position) = usize::try_from(bit) else {
            continue;
        };
        if position >= top {
            continue;
        }
        let index = register.len() - 1 - position / 8;
        let mask = 1u8 << (position % 8);
        if let Some(byte) = register.get_mut(index) {
            if value.checked_shr(bit - low).unwrap_or(0) & 1 == 1 {
                *byte |= mask;
            } else {
                *byte &= !mask;
            }
        }
    }
}

/// Bits `high` down to `low` of a register, bit 127 (for a 16-byte register) the most
/// significant bit of its first byte, as the specification numbers them.
#[must_use]
pub fn get(register: &[u8], high: u32, low: u32) -> u64 {
    (low..=high).rev().fold(0, |value, bit| {
        let byte = usize::try_from(bit / 8)
            .ok()
            .and_then(|from_end| register.len().checked_sub(1 + from_end))
            .and_then(|index| register.get(index));
        value << 1 | byte.map_or(0, |byte| u64::from(byte >> (bit % 8) & 1))
    })
}

/// Writes the CRC7 and the end bit in the last byte of a 16-byte register.
fn seal(register: &mut [u8; 16]) {
    let (body, last) = register.split_at_mut(15);
    if let Some(last) = last.first_mut() {
        *last = crc7_byte(body);
    }
}

/// The identity of a card (CID, PLS Table 5-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cid {
    /// MID, assigned by the SD-3C.
    pub manufacturer: u8,
    /// OID, two ASCII characters.
    pub oem: [u8; 2],
    /// PNM, five ASCII characters.
    pub product: [u8; 5],
    /// PRV, two BCD digits `n.m`.
    pub revision: u8,
    /// PSN.
    pub serial: u32,
    /// The year of manufacture, 2000 to 2255.
    pub year: u16,
    /// The month of manufacture, 1 to 12.
    pub month: u8,
}

impl Default for Cid {
    /// ASSUMPTION: no manufacturer is imitated (research §5): MID 0, OID `IE`, PNM `INKEM`,
    /// revision 1.0, serial 1, made in October 2026. A fixed identity keeps runs deterministic.
    fn default() -> Cid {
        Cid {
            manufacturer: 0,
            oem: *b"IE",
            product: *b"INKEM",
            revision: 0x10,
            serial: 1,
            year: 2026,
            month: 10,
        }
    }
}

impl Cid {
    /// The register, its CRC7 included. A year outside 2000–2255 is clamped into it, and a month
    /// keeps its low four bits.
    #[must_use]
    pub fn register(&self) -> [u8; 16] {
        let mut cid = [0u8; 16];
        set(&mut cid, 127, 120, self.manufacturer.into());
        set(&mut cid, 119, 104, u16::from_be_bytes(self.oem).into());
        let product = self
            .product
            .iter()
            .fold(0u64, |value, &byte| value << 8 | u64::from(byte));
        set(&mut cid, 103, 64, product);
        set(&mut cid, 63, 56, self.revision.into());
        set(&mut cid, 55, 24, self.serial.into());
        let year = self.year.clamp(2000, 2255) - 2000;
        set(&mut cid, 19, 12, year.into());
        set(&mut cid, 11, 8, (self.month & 0x0F).into());
        seal(&mut cid);
        cid
    }
}

/// The OCR (PLS Table 5-1): the 2.7–3.6 V window, and once powered up, the capacity status.
#[must_use]
pub const fn ocr(powered_up: bool, kind: CardKind) -> u32 {
    let mut ocr = OCR_VOLTAGES;
    if powered_up {
        ocr |= OCR_POWERED_UP;
        if matches!(kind, CardKind::Sdhc) {
            ocr |= OCR_CCS;
        }
    }
    ocr
}

/// The SCR (PLS Table 5-17): structure 1.0, specification 3.0x (`SD_SPEC` 2 and `SD_SPEC3`),
/// zeros after an erase, no CPRM (ASSUMPTION: not modelled), one and four data lines.
pub const SCR: [u8; 8] = [0x02, 0x05, 0x80, 0, 0, 0, 0, 0];

/// The bytes of the SD status (ACMD13): ASSUMPTION, all zero (a one-line bus, a regular card,
/// no speed class); nothing on the X4 reads it.
pub const SD_STATUS: [u8; 64] = [0; 64];

/// The status CMD6 sends (PLS §4.3.10, Table 4-13) for `argument`: a card with the default
/// function only, in every group. A group asked for function 0 or "no change" (`Fh`) gives 0;
/// any other function gives `Fh`, which says it cannot be switched to. Mode 1 switches nothing.
#[must_use]
pub fn switch_status(argument: u32) -> [u8; 64] {
    let mut status = [0u8; 64];
    // ASSUMPTION: 100 mA of maximum current.
    set(&mut status, 511, 496, 100);
    // Functions 15 and 0 of each of the six groups are always "supported".
    for group in 0..6u32 {
        let low = 400 + group * 16;
        set(&mut status, low + 15, low, 0x8001);
        let asked = (argument >> (group * 4)) & 0xF;
        let given = if asked == 0 || asked == 0xF { 0 } else { 0xF };
        let low = 376 + group * 4;
        set(&mut status, low + 3, low, given);
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdcard::crc::crc7;

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    #[test]
    fn a_capacity_up_to_two_gibibytes_is_a_standard_one_its_csd_tells_back() {
        for bytes in [
            32 * 1024,
            64 * MIB,
            100 * MIB,
            GIB,
            GIB + 512 * MIB,
            2 * GIB,
        ] {
            let capacity = Capacity::new(bytes).unwrap();
            assert_eq!(capacity.kind(), CardKind::Sdsc, "{bytes}");
            assert_eq!(capacity.bytes(), bytes);
            assert_eq!(capacity.blocks(), bytes / 512);
            let csd = capacity.csd(false);
            assert_eq!(get(&csd, 127, 126), 0, "CSD 1.0");
            assert_eq!(Capacity::told_by(&csd), bytes, "{bytes}");
            assert_eq!(get(&csd, 79, 79), 1, "READ_BL_PARTIAL");
            assert_eq!(get(&csd, 21, 21), 0, "WRITE_BL_PARTIAL");
            assert_eq!(get(&csd, 95, 84), 0x535);
            assert_eq!(csd[15], crc7_byte(&csd[..15]));
        }
        assert_eq!(get(&Capacity::new(2 * GIB).unwrap().csd(false), 83, 80), 10);
        assert_eq!(get(&Capacity::new(GIB).unwrap().csd(false), 83, 80), 9);
    }

    #[test]
    fn a_capacity_above_two_gibibytes_is_a_high_one_its_csd_tells_back() {
        for bytes in [2 * GIB + 512 * 1024, 4 * GIB, 32 * GIB, MAX_CAPACITY_BYTES] {
            let capacity = Capacity::new(bytes).unwrap();
            assert_eq!(capacity.kind(), CardKind::Sdhc);
            let csd = capacity.csd(true);
            assert_eq!(get(&csd, 127, 126), 1, "CSD 2.0");
            assert_eq!(Capacity::told_by(&csd), bytes);
            assert_eq!(get(&csd, 119, 112), 0x0E, "TAAC fixed");
            assert_eq!(get(&csd, 83, 80), 9);
            assert_eq!(get(&csd, 46, 39), 0xFF, "ERASE_BLK_EN and SECTOR_SIZE");
            assert_eq!(get(&csd, 12, 12), 1, "TMP_WRITE_PROTECT");
            assert_eq!(csd[15] & 1, 1);
            assert_eq!(csd[15] >> 1, crc7(&csd[..15]));
        }
        assert_eq!(get(&Capacity::new(4 * GIB).unwrap().csd(false), 12, 12), 0);
    }

    #[test]
    fn a_size_no_card_has_is_refused_with_its_reason() {
        assert_eq!(Capacity::new(0).unwrap_err(), CapacityError::Empty);
        assert_eq!(
            Capacity::new(1000).unwrap_err(),
            CapacityError::NotWholeBlocks { bytes: 1000 }
        );
        // 4097 blocks of 512: a prime count of blocks, which no multiplier divides.
        assert_eq!(
            Capacity::new(4097 * 512).unwrap_err(),
            CapacityError::NotCodableSdsc { bytes: 4097 * 512 }
        );
        assert_eq!(
            Capacity::new(3 * GIB + 512).unwrap_err(),
            CapacityError::NotCodableSdhc {
                bytes: 3 * GIB + 512
            }
        );
        assert_eq!(
            Capacity::new(MAX_CAPACITY_BYTES + SDHC_UNIT_BYTES).unwrap_err(),
            CapacityError::TooLarge {
                bytes: MAX_CAPACITY_BYTES + SDHC_UNIT_BYTES
            }
        );
        assert_eq!(
            CapacityError::NotCodableSdhc { bytes: 7 }.to_string(),
            "7 bytes cannot be told by the CSD of a high-capacity card (above 2 GiB); give a whole number of 512 KiB"
        );
        assert_eq!(CardKind::Sdsc.name(), "SDSC");
        assert_eq!(CardKind::Sdhc.name(), "SDHC/SDXC");
    }

    #[test]
    fn every_whole_number_of_mebibytes_up_to_two_gibibytes_is_codable() {
        for mebibytes in 1..=2048 {
            Capacity::new(mebibytes * MIB).unwrap();
        }
    }

    #[test]
    fn the_cid_carries_its_fields_and_its_crc() {
        let cid = Cid::default().register();
        assert_eq!(get(&cid, 127, 120), 0);
        assert_eq!(&cid[1..3], b"IE");
        assert_eq!(&cid[3..8], b"INKEM");
        assert_eq!(cid[8], 0x10);
        assert_eq!(get(&cid, 55, 24), 1);
        assert_eq!(get(&cid, 19, 12), 26);
        assert_eq!(get(&cid, 11, 8), 10);
        assert_eq!(cid[15], crc7_byte(&cid[..15]));
        let old = Cid {
            year: 1990,
            ..Cid::default()
        };
        assert_eq!(get(&old.register(), 19, 12), 0, "clamped to 2000");
    }

    #[test]
    fn the_ocr_says_power_up_and_capacity_once_initialized() {
        assert_eq!(ocr(false, CardKind::Sdhc), 0x00FF_8000);
        assert_eq!(ocr(true, CardKind::Sdsc), 0x80FF_8000);
        assert_eq!(ocr(true, CardKind::Sdhc), 0xC0FF_8000);
    }

    #[test]
    fn the_scr_tells_specification_three_and_two_bus_widths() {
        assert_eq!(get(&SCR, 63, 60), 0, "SCR_STRUCTURE");
        assert_eq!(get(&SCR, 59, 56), 2, "SD_SPEC");
        assert_eq!(get(&SCR, 55, 55), 0, "DATA_STAT_AFTER_ERASE");
        assert_eq!(get(&SCR, 54, 52), 0, "SD_SECURITY");
        assert_eq!(get(&SCR, 51, 48), 0b0101, "SD_BUS_WIDTHS");
        assert_eq!(get(&SCR, 47, 47), 1, "SD_SPEC3");
    }

    #[test]
    fn the_switch_status_offers_the_default_function_only() {
        let status = switch_status(0x00FF_FFF1);
        assert_eq!(get(&status, 511, 496), 100);
        for group in 0..6 {
            assert_eq!(get(&status, 415 + group * 16, 400 + group * 16), 0x8001);
        }
        assert_eq!(get(&status, 379, 376), 0xF, "group 1 asked for high speed");
        assert_eq!(get(&status, 383, 380), 0, "group 2 asked for no change");
        assert_eq!(get(&status, 375, 368), 0, "data structure version 0");
        assert_eq!(get(&switch_status(0), 399, 376), 0);
    }
}
