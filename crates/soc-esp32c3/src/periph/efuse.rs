//! eFuse read mirror of a blank chip. Layout: `efuse_reg.h` (report §H).
//! Programming, the MAC address and the block version arrive when the bootloader needs them (SP1b-2).
use crate::chip::ChipRevision;
use emu_core::Peripheral;

/// First and last register of the read mirror: `RD_WR_DIS` to `RD_RS_ERR1`.
const RD_FIRST: u32 = 0x2C;
const RD_LAST: u32 = 0x1C4;
const RD_WORDS: usize = ((RD_LAST - RD_FIRST) / 4 + 1) as usize;
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
    read_mirror: [u32; RD_WORDS],
}

/// Index in the read mirror of the register at `offset`, which must lie in `RD_FIRST..=RD_LAST`.
const fn word_index(offset: u32) -> usize {
    ((offset - RD_FIRST) / 4) as usize
}

impl Efuse {
    /// A chip with every fuse blank except its wafer version: no secure boot, no flash encryption,
    /// both ROM consoles enabled, default SPI pads.
    #[must_use]
    pub fn new(revision: ChipRevision) -> Efuse {
        let (major, minor) = (u32::from(revision.major()), u32::from(revision.minor()));
        let mut read_mirror = [0; RD_WORDS];
        read_mirror[word_index(RD_MAC_SPI_SYS_3)] =
            (minor & WAFER_MINOR_LO_MASK) << WAFER_MINOR_LO_SHIFT;
        read_mirror[word_index(RD_MAC_SPI_SYS_5)] =
            ((minor >> WAFER_MINOR_LO_BITS) << WAFER_MINOR_HI_SHIFT) | (major << WAFER_MAJOR_SHIFT);
        Efuse { read_mirror }
    }
}

impl Peripheral for Efuse {
    fn read(&mut self, offset: u32) -> Option<u32> {
        (RD_FIRST..=RD_LAST)
            .contains(&offset)
            .then(|| self.read_mirror[word_index(offset)])
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

    #[test]
    fn blank_chip_reads_zero_everywhere_except_the_wafer_version() {
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(revision(&mut efuse), (0, 3));
        for offset in (0x2C..=0x1C4)
            .step_by(4)
            .filter(|o| ![0x50, 0x58].contains(o))
        {
            assert_eq!(efuse.read(offset), Some(0), "{offset:#x}");
        }
        assert_eq!(efuse.read(0x50), Some(3 << 18));
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
