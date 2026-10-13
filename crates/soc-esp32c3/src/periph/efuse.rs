//! eFuse read mirror of a production chip. Layout: `efuse_reg.h` (report §H).
//! The mirror answers only the read registers the SVD documents between `RD_WR_DIS` and `RD_RS_ERR1`
//! (`svd_regs::esp32c3`, peripheral `EFUSE`) and the one hole the application reads (`0x18C`, see
//! `RD_REPEAT_ERR_GAP`): the other reserved holes (such as 0x194..=0x1BC) are not registers, so they
//! stay unmodelled and surface as diagnostics (spec §5.2).
//! The content is that of a production chip (decision D9). The programming registers (`CMD`,
//! `PGM_*`) stay unmodelled and are reported by name; no plan is their heir, because no measured
//! boot touches them (report FW §K.3 lists what the community firmware reaches before its hang).
use crate::chip::ChipRevision;
use crate::ctx::Ctx;
use crate::periph::EFUSE_BASE;
use emu_core::Peripheral;

/// Offset of an `EFUSE` register relative to the eFuse base; an unknown name fails the build.
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("EFUSE", name) - EFUSE_BASE
}

/// First and last register of the read mirror: `RD_WR_DIS` to `RD_RS_ERR1`.
const RD_FIRST: u32 = reg("RD_WR_DIS");
const RD_LAST: u32 = reg("RD_RS_ERR1");
/// `RD_REPEAT_DATA1` bits [17:16]: `WDT_DELAY_SEL`.
const RD_REPEAT_DATA1: u32 = reg("RD_REPEAT_DATA1");
const WDT_DELAY_SEL_SHIFT: u32 = 16;
const WDT_DELAY_SEL_MASK: u32 = 0b11;
/// The word between `RD_REPEAT_ERR3` (`+0x188`) and `RD_REPEAT_ERR4` (`+0x190`). No register is
/// documented there, in the SVD or in ESP-IDF `efuse_reg.h`; the application reads it all the
/// same, as the fifth word of the repeat-error block, at every start
/// (`APP:esp_efuse_utility_check_errors@0x420003f8`, the load at `0x4200045c`).
///
/// ASSUMPTION: it reads 0, like the error registers around it on a chip without eFuse errors. A
/// read on a real chip settles it.
const RD_REPEAT_ERR_GAP: u32 = reg("RD_REPEAT_ERR3") + svd_regs::REGISTER_BYTES;
/// `RD_MAC_SPI_SYS_3` bits [20:18]: `WAFER_VERSION_MINOR_LO`.
const RD_MAC_SPI_SYS_3: u32 = reg("RD_MAC_SPI_SYS_3");
const WAFER_MINOR_LO_SHIFT: u32 = 18;
const WAFER_MINOR_LO_MASK: u32 = 0x7;
/// `RD_MAC_SPI_SYS_5` bit 23: `WAFER_VERSION_MINOR_HI`; bits [25:24]: `WAFER_VERSION_MAJOR`.
const RD_MAC_SPI_SYS_5: u32 = reg("RD_MAC_SPI_SYS_5");
const WAFER_MINOR_HI_SHIFT: u32 = 23;
/// The minor revision is 4 bits: its top bit lives in `RD_MAC_SPI_SYS_5`, the low three in `_3`.
const WAFER_MINOR_LO_BITS: u32 = 3;
const WAFER_MAJOR_SHIFT: u32 = 24;
/// `RD_MAC_SPI_SYS_3` bits [26:24]: `BLK_VERSION_MINOR`; `RD_SYS_PART1_DATA4` bits [1:0]:
/// `BLK_VERSION_MAJOR` (ESP-IDF v5.5.2 `hal/esp32c3/include/hal/efuse_ll.h`,
/// `efuse_ll_get_blk_version_minor`/`_major`).
const BLK_MINOR_SHIFT: u32 = 24;
const BLK_MINOR_MASK: u32 = 0x7;
const RD_SYS_PART1_DATA4: u32 = reg("RD_SYS_PART1_DATA4");
const BLK_MAJOR_MASK: u32 = 0x3;
/// `efuse_hal_blk_version()` is `major * 100 + minor`.
const BLK_MAJOR_WEIGHT: u32 = 100;
/// The wafer-version fields the profile's chip revision overrides.
const WAFER_FIELDS_SYS_3: u32 = WAFER_MINOR_LO_MASK << WAFER_MINOR_LO_SHIFT;
const WAFER_FIELDS_SYS_5: u32 = (0x3 << WAFER_MAJOR_SHIFT) | (1 << WAFER_MINOR_HI_SHIFT);

/// The eFuse content of a production ESP32-C3 v0.3: Espressif's own `espefuse` dump, quoted in
/// report BA §8 (decision D9). MAC `58:cf:79:0f:96:8c`, eFuse block version 1.2, calibration words,
/// `ERR_RST_ENABLE` set; no key, no secure boot, no flash encryption. Registers not listed read 0.
///
/// Source of the words: the `idf.py efuse-dump` sample of the ESP-IDF documentation,
/// `docs/en/api-reference/system/inc/espefuse_summary_ESP32-C3_dump.rst` (ESP-IDF v5.5.2,
/// Apache-2.0). It is the dump of one sample chip: the MAC address is that chip's, so every
/// emulated device shares it until profiles carry their own (carry-over note, sub-project 5a).
/// ASSUMPTION: the eFuse content of a real Xteink X4 is unknown; this is a real chip of the same
/// revision. An `espefuse.py summary` of a unit settles it.
const PRODUCTION_V0_3: [(u32, u32); 14] = [
    (reg("RD_REPEAT_DATA3"), 0x8000_0000),
    (reg("RD_MAC_SPI_SYS_0"), 0x790F_968C),
    (reg("RD_MAC_SPI_SYS_1"), 0x0000_58CF),
    (RD_MAC_SPI_SYS_3, 0x020C_0000),
    (reg("RD_MAC_SPI_SYS_4"), 0x7154_24E0),
    (RD_MAC_SPI_SYS_5, 0x0047_D2F2),
    (reg("RD_SYS_PART1_DATA0"), 0x9604_6025),
    (reg("RD_SYS_PART1_DATA1"), 0x6F41_FDC3),
    (reg("RD_SYS_PART1_DATA2"), 0x512C_EDBE),
    (reg("RD_SYS_PART1_DATA3"), 0x217E_E31D),
    (RD_SYS_PART1_DATA4, 0xD864_EA41),
    (reg("RD_SYS_PART1_DATA5"), 0x5ABA_3A86),
    (reg("RD_SYS_PART1_DATA6"), 0x1E26_0363),
    (reg("RD_SYS_PART1_DATA7"), 0x0000_0009),
];

#[derive(Clone, Debug)]
pub struct Efuse {
    /// `(offset, value)` of every documented read register, sorted by offset.
    read_mirror: Vec<(u32, u32)>,
}

/// Offsets, relative to the eFuse base, of the read registers the SVD documents, in ascending order.
fn documented_offsets() -> Vec<u32> {
    let mut offsets: Vec<u32> = svd_regs::esp32c3::registers()
        .iter()
        .filter(|r| r.peripheral == "EFUSE")
        .map(|r| r.addr - EFUSE_BASE)
        .filter(|offset| (RD_FIRST..=RD_LAST).contains(offset))
        .collect();
    offsets.sort_unstable();
    offsets
}

impl Efuse {
    /// The production chip of [`PRODUCTION_V0_3`], with its wafer version replaced by `revision`.
    #[must_use]
    pub fn new(revision: ChipRevision) -> Efuse {
        let (major, minor) = (u32::from(revision.major()), u32::from(revision.minor()));
        let wafer_lo = (minor & WAFER_MINOR_LO_MASK) << WAFER_MINOR_LO_SHIFT;
        let wafer_hi_major =
            ((minor >> WAFER_MINOR_LO_BITS) << WAFER_MINOR_HI_SHIFT) | (major << WAFER_MAJOR_SHIFT);
        let read_mirror = documented_offsets()
            .into_iter()
            .map(|offset| {
                let dumped = PRODUCTION_V0_3
                    .iter()
                    .find(|(o, _)| *o == offset)
                    .map_or(0, |&(_, value)| value);
                let value = match offset {
                    RD_MAC_SPI_SYS_3 => (dumped & !WAFER_FIELDS_SYS_3) | wafer_lo,
                    RD_MAC_SPI_SYS_5 => (dumped & !WAFER_FIELDS_SYS_5) | wafer_hi_major,
                    _ => dumped,
                };
                (offset, value)
            })
            .collect();
        Efuse { read_mirror }
    }

    /// The eFuse block revision the chip reports, `major * 100 + minor`, as
    /// `efuse_hal_blk_version()` computes it: what the 2nd-stage bootloader compares with the
    /// bounds of an application's descriptor (`bootloader_common_check_efuse_blk_validity`).
    #[must_use]
    pub fn block_revision(&self) -> u16 {
        let word = |at: u32| {
            self.read_mirror
                .iter()
                .find(|&&(offset, _)| offset == at)
                .map_or(0, |&(_, value)| value)
        };
        let minor = (word(RD_MAC_SPI_SYS_3) >> BLK_MINOR_SHIFT) & BLK_MINOR_MASK;
        let major = word(RD_SYS_PART1_DATA4) & BLK_MAJOR_MASK;
        // INVARIANT: 2 bits and 3 bits: at most 3 * 100 + 7, which a u16 holds.
        u16::try_from(major * BLK_MAJOR_WEIGHT + minor).unwrap_or(u16::MAX)
    }

    /// `WDT_DELAY_SEL`, `RD_REPEAT_DATA1[17:16]`: the multiplier of the stage 0 timeout of the RTC
    /// watchdog (`IDF:hal/esp32c3/include/hal/rwdt_ll.h:119`; report TW §C.3).
    #[must_use]
    pub fn wdt_delay_sel(&self) -> u32 {
        let word = self
            .read_mirror
            .iter()
            .find(|&&(offset, _)| offset == RD_REPEAT_DATA1)
            .map_or(0, |&(_, value)| value);
        (word >> WDT_DELAY_SEL_SHIFT) & WDT_DELAY_SEL_MASK
    }
}

impl Peripheral<Ctx> for Efuse {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if offset == RD_REPEAT_ERR_GAP {
            return Some(0);
        }
        self.read_mirror
            .binary_search_by_key(&offset, |&(o, _)| o)
            .ok()
            .map(|i| self.read_mirror[i].1)
    }

    fn write(&mut self, _ctx: &mut Ctx, _offset: u32, _value: u32) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    fn revision(ctx: &mut Ctx, efuse: &mut Efuse) -> (u32, u32) {
        let sys3 = efuse.read(ctx, 0x50).unwrap();
        let sys5 = efuse.read(ctx, 0x58).unwrap();
        let minor = (((sys5 >> 23) & 1) << 3) | ((sys3 >> 18) & 7);
        ((sys5 >> 24) & 3, minor)
    }

    /// Offsets of the EFUSE read registers the SVD documents in the mirror window, counted
    /// independently of the model.
    fn svd_read_offsets() -> Vec<u32> {
        svd_regs::esp32c3::registers()
            .iter()
            .filter(|r| r.peripheral == "EFUSE")
            .map(|r| r.addr - EFUSE_BASE)
            .filter(|o| (0x2C..=0x1C4).contains(o))
            .collect()
    }

    fn word(ctx: &mut Ctx, efuse: &mut Efuse, name: &str) -> u32 {
        efuse
            .read(ctx, svd_regs::esp32c3::addr("EFUSE", name) - EFUSE_BASE)
            .unwrap()
    }

    #[test]
    fn the_mirror_holds_the_dump_of_a_production_v0_3_chip() {
        // BA §8, raw words of the three blocks.
        let ctx = &mut Ctx::default();
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        let expected = [
            ("RD_WR_DIS", 0),
            ("RD_REPEAT_DATA0", 0),
            ("RD_REPEAT_DATA1", 0),
            ("RD_REPEAT_DATA2", 0),
            ("RD_REPEAT_DATA3", 0x8000_0000),
            ("RD_REPEAT_DATA4", 0),
            ("RD_MAC_SPI_SYS_0", 0x790F_968C),
            ("RD_MAC_SPI_SYS_1", 0x0000_58CF),
            ("RD_MAC_SPI_SYS_2", 0),
            ("RD_MAC_SPI_SYS_3", 0x020C_0000),
            ("RD_MAC_SPI_SYS_4", 0x7154_24E0),
            ("RD_MAC_SPI_SYS_5", 0x0047_D2F2),
            ("RD_SYS_PART1_DATA0", 0x9604_6025),
            ("RD_SYS_PART1_DATA1", 0x6F41_FDC3),
            ("RD_SYS_PART1_DATA2", 0x512C_EDBE),
            ("RD_SYS_PART1_DATA3", 0x217E_E31D),
            ("RD_SYS_PART1_DATA4", 0xD864_EA41),
            ("RD_SYS_PART1_DATA5", 0x5ABA_3A86),
            ("RD_SYS_PART1_DATA6", 0x1E26_0363),
            ("RD_SYS_PART1_DATA7", 0x0000_0009),
        ];
        for (name, value) in expected {
            assert_eq!(word(ctx, &mut efuse, name), value, "{name}");
        }
        let listed: Vec<u32> = expected
            .iter()
            .map(|(name, _)| svd_regs::esp32c3::addr("EFUSE", name) - EFUSE_BASE)
            .collect();
        for offset in svd_read_offsets()
            .into_iter()
            .filter(|o| !listed.contains(o))
        {
            assert_eq!(
                efuse.read(ctx, offset),
                Some(0),
                "{offset:#x}: keys, user data and error flags are blank"
            );
        }
    }

    #[test]
    fn the_fields_the_boot_chain_decodes_have_the_values_it_needs() {
        let ctx = &mut Ctx::default();
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(
            revision(ctx, &mut efuse),
            (0, 3),
            "chip revision v0.3: the image headers require at least v0.3"
        );
        let sys3 = word(ctx, &mut efuse, "RD_MAC_SPI_SYS_3");
        let part1_4 = word(ctx, &mut efuse, "RD_SYS_PART1_DATA4");
        assert_eq!(
            (part1_4 & 3, (sys3 >> 24) & 7),
            (1, 2),
            "eFuse block revision v1.2"
        );
        for chip in [ChipRevision::V0_3, ChipRevision::new(1, 1).unwrap()] {
            assert_eq!(
                Efuse::new(chip).block_revision(),
                102,
                "{chip}: the block revision does not follow the chip revision"
            );
        }
        let mac_low = word(ctx, &mut efuse, "RD_MAC_SPI_SYS_0").to_be_bytes();
        let mac_high = word(ctx, &mut efuse, "RD_MAC_SPI_SYS_1").to_be_bytes();
        assert_eq!(
            [
                mac_high[2],
                mac_high[3],
                mac_low[0],
                mac_low[1],
                mac_low[2],
                mac_low[3]
            ],
            [0x58, 0xCF, 0x79, 0x0F, 0x96, 0x8C]
        );
        // What the ROM decides on (FB §C rows 2 to 4): default SPI pads, no secure boot, no flash
        // power-up delay, no forced resume, direct boot not disabled.
        assert_eq!(word(ctx, &mut efuse, "RD_MAC_SPI_SYS_1") >> 16, 0);
        assert_eq!(word(ctx, &mut efuse, "RD_MAC_SPI_SYS_2"), 0);
        let data2 = word(ctx, &mut efuse, "RD_REPEAT_DATA2");
        assert_eq!((data2 >> 20) & 1, 0, "SECURE_BOOT_EN");
        assert_eq!(data2 >> 28, 0, "FLASH_TPUW");
        let data3 = word(ctx, &mut efuse, "RD_REPEAT_DATA3");
        assert_eq!(
            data3 & ((1 << 13) | (1 << 1)),
            0,
            "FORCE_SEND_RESUME, DIS_DIRECT_BOOT"
        );
        assert_eq!(data3 >> 31, 1, "ERR_RST_ENABLE (BA §11)");
        assert_eq!(
            (word(ctx, &mut efuse, "RD_REPEAT_DATA1") >> 16) & 3,
            0,
            "WDT_DELAY_SEL"
        );
    }

    #[test]
    fn the_watchdog_delay_is_bits_17_and_16_of_repeat_data_1() {
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(efuse.wdt_delay_sel(), 0, "a production chip");
        // Each word has its neighbours set the other way round from the field, so that a wrong
        // shift or mask reads another value.
        for (word, delay) in [
            (0x0000_0000, 0),
            (0x0001_0000, 1),
            (0x0002_0000, 2),
            (0x0003_0000, 3),
            (0xFFFC_FFFF, 0),
            (0xFFFD_FFFF, 1),
            (0xFFFE_FFFF, 2),
            (0xFFFF_FFFF, 3),
            (0x0000_FFFF, 0),
            (0xFFFC_0000, 0),
        ] {
            let entry = efuse
                .read_mirror
                .iter_mut()
                .find(|(offset, _)| *offset == RD_REPEAT_DATA1)
                .expect("RD_REPEAT_DATA1 is in the mirror");
            entry.1 = word;
            assert_eq!(efuse.wdt_delay_sel(), delay, "{word:#010x}");
        }
    }

    #[test]
    fn the_profile_revision_replaces_the_wafer_version_and_nothing_else() {
        let ctx = &mut Ctx::default();
        let mut efuse = Efuse::new(ChipRevision::new(1, 9).unwrap());
        assert_eq!(revision(ctx, &mut efuse), (1, 9));
        let untouched_3 = !(0x7 << 18);
        let untouched_5 = !((0x3 << 24) | (1 << 23));
        assert_eq!(
            word(ctx, &mut efuse, "RD_MAC_SPI_SYS_3") & untouched_3,
            0x020C_0000 & untouched_3
        );
        assert_eq!(
            word(ctx, &mut efuse, "RD_MAC_SPI_SYS_5") & untouched_5,
            0x0047_D2F2 & untouched_5
        );
        let mut v0_0 = Efuse::new(ChipRevision::new(0, 0).unwrap());
        assert_eq!(
            revision(ctx, &mut v0_0),
            (0, 0),
            "the rev0 ROM tests boot a v0.0 chip"
        );
    }

    #[test]
    fn the_holes_between_documented_registers_are_unmodeled_but_for_the_one_esp_idf_reads() {
        let ctx = &mut Ctx::default();
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(
            efuse.read(ctx, 0x18C),
            Some(0),
            "the hole ESP-IDF reads as a fifth repeat-error word"
        );
        assert_eq!(efuse.read(ctx, 0x194), None, "hole");
        let answered = (0x2C..=0x1C4)
            .step_by(4)
            .filter(|&o| efuse.read(ctx, o).is_some())
            .count();
        assert_eq!(
            answered,
            svd_read_offsets().len() + 1,
            "the mirror answers exactly the documented registers, and the one hole ESP-IDF reads"
        );
        assert_eq!(
            answered, 92,
            "103 words in the window minus 11 unanswered holes; a changed count means the SVD changed"
        );
    }

    #[test]
    fn programming_registers_and_writes_are_not_modelled() {
        let ctx = &mut Ctx::default();
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(efuse.read(ctx, 0x00), None, "PGM_DATA0");
        assert_eq!(efuse.read(ctx, 0x1C8), None, "CLK");
        assert_eq!(efuse.read(ctx, 0x1D4), None, "CMD");
        assert!(!efuse.write(ctx, 0x30, 1), "the read mirror is read-only");
    }
}
