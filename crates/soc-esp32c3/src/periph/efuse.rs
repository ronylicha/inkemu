//! eFuse read mirror of a blank chip. Layout: `efuse_reg.h` (report §H).
//! The mirror answers only the read registers the SVD documents between `RD_WR_DIS` and `RD_RS_ERR1`
//! (`svd_regs::esp32c3`, peripheral `EFUSE`): the reserved holes between them (such as 0x18C and
//! 0x194..=0x1BC) are not registers, so they stay unmodelled and surface as diagnostics (spec §5.2).
//! Programming, the MAC address and the block version arrive when the bootloader needs them (SP1b-2).
use crate::chip::ChipRevision;
use crate::periph::EFUSE_BASE;
use emu_core::Peripheral;

/// First and last register of the read mirror: `RD_WR_DIS` to `RD_RS_ERR1`.
const RD_FIRST: u32 = 0x2C;
const RD_LAST: u32 = 0x1C4;
/// `RD_MAC_SPI_SYS_3` bits [20:18]: `WAFER_VERSION_MINOR_LO`.
const RD_MAC_SPI_SYS_3: u32 = 0x50;
const WAFER_MINOR_LO_SHIFT: u32 = 18;
const WAFER_MINOR_LO_MASK: u32 = 0x7;
/// `RD_MAC_SPI_SYS_5` bit 23: `WAFER_VERSION_MINOR_HI`; bits [25:24]: `WAFER_VERSION_MAJOR`.
const RD_MAC_SPI_SYS_5: u32 = 0x58;
const WAFER_MINOR_HI_SHIFT: u32 = 23;
/// The minor revision is 4 bits: its top bit lives in `RD_MAC_SPI_SYS_5`, the low three in `_3`.
const WAFER_MINOR_LO_BITS: u32 = 3;
const WAFER_MAJOR_SHIFT: u32 = 24;

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
    /// A chip with every fuse blank except its wafer version: no secure boot, no flash encryption,
    /// both ROM consoles enabled, default SPI pads.
    #[must_use]
    pub fn new(revision: ChipRevision) -> Efuse {
        let (major, minor) = (u32::from(revision.major()), u32::from(revision.minor()));
        let wafer_lo = (minor & WAFER_MINOR_LO_MASK) << WAFER_MINOR_LO_SHIFT;
        let wafer_hi_major =
            ((minor >> WAFER_MINOR_LO_BITS) << WAFER_MINOR_HI_SHIFT) | (major << WAFER_MAJOR_SHIFT);
        let read_mirror = documented_offsets()
            .into_iter()
            .map(|offset| {
                let value = match offset {
                    RD_MAC_SPI_SYS_3 => wafer_lo,
                    RD_MAC_SPI_SYS_5 => wafer_hi_major,
                    _ => 0,
                };
                (offset, value)
            })
            .collect();
        Efuse { read_mirror }
    }
}

impl Peripheral for Efuse {
    fn read(&mut self, offset: u32) -> Option<u32> {
        self.read_mirror
            .binary_search_by_key(&offset, |&(o, _)| o)
            .ok()
            .map(|i| self.read_mirror[i].1)
    }

    fn write(&mut self, _offset: u32, _value: u32) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    fn revision(efuse: &mut Efuse) -> (u32, u32) {
        let sys3 = efuse.read(0x50).unwrap();
        let sys5 = efuse.read(0x58).unwrap();
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

    #[test]
    fn blank_chip_reads_zero_everywhere_except_the_wafer_version() {
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(revision(&mut efuse), (0, 3));
        let documented = svd_read_offsets();
        for &offset in documented.iter().filter(|o| ![0x50, 0x58].contains(*o)) {
            assert_eq!(efuse.read(offset), Some(0), "{offset:#x}");
        }
        assert_eq!(efuse.read(0x50), Some(3 << 18));
        assert_eq!(documented.first(), Some(&0x2C));
        assert_eq!(documented.last(), Some(&0x1C4));
        assert_eq!(efuse.read(0x2C), Some(0), "first documented register");
        assert_eq!(efuse.read(0x1C4), Some(0), "last documented register");
    }

    #[test]
    fn reserved_holes_between_documented_registers_are_unmodeled() {
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(efuse.read(0x18C), None, "hole");
        assert_eq!(efuse.read(0x194), None, "hole");
        let answered = (0x2C..=0x1C4)
            .step_by(4)
            .filter(|&o| efuse.read(o).is_some())
            .count();
        assert_eq!(
            answered,
            svd_read_offsets().len(),
            "the mirror answers exactly the documented registers"
        );
        assert_eq!(
            answered, 91,
            "103 words in the window minus 12 reserved holes; a changed count means the SVD changed"
        );
    }

    #[test]
    fn minor_revision_is_split_across_two_words() {
        let mut efuse = Efuse::new(ChipRevision::new(1, 9).unwrap());
        assert_eq!(revision(&mut efuse), (1, 9));
        assert_eq!(efuse.read(0x58), Some((1 << 24) | (1 << 23)));
    }

    #[test]
    fn programming_registers_and_writes_are_not_modelled() {
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(efuse.read(0x00), None, "PGM_DATA0");
        assert_eq!(efuse.read(0x1C8), None, "CLK");
        assert_eq!(efuse.read(0x1D4), None, "CMD");
        assert!(!efuse.write(0x30, 1), "the read mirror is read-only");
    }
}
