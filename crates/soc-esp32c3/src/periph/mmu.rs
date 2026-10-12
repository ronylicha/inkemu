//! The flash MMU table: 128 entries that map 64 KiB pages of the two flash windows to flash pages.
//! Source: report CM §1 (ESP-IDF `ext_mem_defs.h`, `mmu_ll.h`; ROM `Cache_MMU_Init`).
use crate::ctx::Ctx;
use emu_core::Peripheral;

/// `DR_REG_MMU_TABLE` (ESP-IDF `reg_base.h`). The SVD does not describe this block (decision D17).
pub const MMU_TABLE_BASE: u32 = 0x600C_5000;
/// `SOC_MMU_ENTRY_NUM`.
pub const MMU_ENTRIES: usize = 128;
/// Fixed page size: `mmu_ll_get_page_size` returns 64 KiB unconditionally.
pub const MMU_PAGE_BYTES: u32 = 0x1_0000;
/// `SOC_MMU_INVALID`: bit 8 set means the entry maps nothing.
pub const MMU_ENTRY_INVALID: u32 = 1 << 8;
/// `SOC_MMU_VALID_VAL_MASK`: the flash page number.
const PAGE_NUMBER_MASK: u32 = 0xFF;
/// ASSUMPTION: an entry is 9 bits wide, as the QEMU oracle stores it (0xFFFFFFFF reads back 0x1FF).
/// `CACHE_MMU_FAULT_CONTENT` is 10 bits wide, so silicon may keep bit 9; software only uses [8:0].
const ENTRY_BITS: u32 = 0x1FF;
const ENTRY_BYTES: u32 = 4;
const PAGE_SHIFT: u32 = 16;
const PAGE_OFFSET_MASK: u32 = MMU_PAGE_BYTES - 1;

/// An access through an entry that maps nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmuMiss {
    pub entry: u32,
    pub content: u32,
}

#[derive(Clone, Debug)]
pub struct MmuTable {
    entries: [u32; MMU_ENTRIES],
    generation: u64,
}

impl MmuTable {
    /// ASSUMPTION: every entry is invalid at power-on. The content before the ROM runs is not
    /// documented; the ROM writes all 128 entries before it uses one (`Cache_MMU_Init`, FB §B.3).
    #[must_use]
    pub const fn new() -> MmuTable {
        MmuTable {
            entries: [MMU_ENTRY_INVALID; MMU_ENTRIES],
            generation: 0,
        }
    }

    #[must_use]
    pub fn entry(&self, index: usize) -> Option<u32> {
        self.entries.get(index).copied()
    }

    /// The 64 KiB flash page entry `index` maps; `None` when the entry is invalid or `index` lies
    /// past the table.
    #[must_use]
    pub fn flash_page(&self, index: usize) -> Option<u32> {
        self.entry(index)
            .filter(|content| content & MMU_ENTRY_INVALID == 0)
            .map(|content| content & PAGE_NUMBER_MASK)
    }

    /// The flash offset `window_offset` (an address minus the base of its window) maps to.
    ///
    /// # Errors
    /// [`MmuMiss`] when the entry is invalid or `window_offset` lies past the 128 pages.
    pub fn translate(&self, window_offset: u32) -> Result<u32, MmuMiss> {
        let entry = window_offset >> PAGE_SHIFT;
        let content = usize::try_from(entry)
            .ok()
            .and_then(|index| self.entry(index))
            .unwrap_or(MMU_ENTRY_INVALID);
        if content & MMU_ENTRY_INVALID != 0 {
            return Err(MmuMiss { entry, content });
        }
        Ok(((content & PAGE_NUMBER_MASK) << PAGE_SHIFT) | (window_offset & PAGE_OFFSET_MASK))
    }

    /// Changes whenever the mapping may have changed: the bus uses it as the version of the code
    /// decoded through the flash windows (decision D6).
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    fn index(offset: u32) -> Option<usize> {
        usize::try_from(offset / ENTRY_BYTES)
            .ok()
            .filter(|index| *index < MMU_ENTRIES)
    }
}

impl Default for MmuTable {
    fn default() -> MmuTable {
        MmuTable::new()
    }
}

impl Peripheral<Ctx> for MmuTable {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        Self::index(offset).and_then(|index| self.entry(index))
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        let Some(slot) = Self::index(offset).and_then(|index| self.entries.get_mut(index)) else {
            return false;
        };
        *slot = value & ENTRY_BITS;
        self.generation += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn every_entry_starts_invalid() {
        let ctx = &mut Ctx::default();
        let mut mmu = MmuTable::new();
        for offset in (0..0x200).step_by(4) {
            assert_eq!(mmu.read(ctx, offset), Some(0x100), "{offset:#x}");
        }
        assert_eq!(
            mmu.read(ctx, 0x200),
            None,
            "128 entries, 0x200 bytes (CM §1)"
        );
        assert!(!mmu.write(ctx, 0x200, 0));
        assert_eq!(
            mmu.translate(0),
            Err(MmuMiss {
                entry: 0,
                content: 0x100
            })
        );
    }

    #[test]
    fn an_entry_keeps_nine_bits_and_reads_back() {
        let ctx = &mut Ctx::default();
        let mut mmu = MmuTable::new();
        assert!(mmu.write(ctx, 4 * 85, 0xFFFF_FFFF));
        assert_eq!(
            mmu.read(ctx, 4 * 85),
            Some(0x1FF),
            "CM §1: 0x1FF kept after writing 0xFFFFFFFF"
        );
        assert_eq!(mmu.entry(85), Some(0x1FF));
        assert_eq!(mmu.entry(128), None);
    }

    #[test]
    fn the_flash_page_of_an_entry_is_its_low_byte_and_an_invalid_entry_has_none() {
        let ctx = &mut Ctx::default();
        let mut mmu = MmuTable::new();
        assert_eq!(mmu.flash_page(3), None, "invalid from power-on");
        mmu.write(ctx, 4 * 3, 0x35);
        assert_eq!(mmu.flash_page(3), Some(0x35));
        mmu.write(ctx, 4 * 3, 0x135);
        assert_eq!(mmu.flash_page(3), None, "bit 8 set: the entry maps nothing");
        mmu.write(ctx, 4 * 127, 0xFF);
        assert_eq!(mmu.flash_page(127), Some(0xFF));
        assert_eq!(mmu.flash_page(128), None);
        assert_eq!(mmu.flash_page(usize::MAX), None);
    }

    #[test]
    fn translation_uses_the_page_number_and_keeps_the_offset_in_the_page() {
        // CM §1: entry_id = (vaddr & 0x7FFFFF) >> 16; paddr = (entry & 0xFF) << 16 | (vaddr & 0xFFFF).
        let ctx = &mut Ctx::default();
        let mut mmu = MmuTable::new();
        mmu.write(ctx, 4 * 3, 1);
        assert_eq!(mmu.translate(0x03_0000), Ok(0x1_0000));
        assert_eq!(mmu.translate(0x03_FFFF), Ok(0x1_FFFF));
        assert_eq!(
            mmu.translate(0x04_0000),
            Err(MmuMiss {
                entry: 4,
                content: 0x100
            })
        );
        mmu.write(ctx, 4 * 127, 0x35);
        assert_eq!(
            mmu.translate(0x7F_0020),
            Ok(0x35_0020),
            "entry 127 is the bootloader's one-page window (BA §2.2)"
        );
    }

    #[test]
    fn offsets_and_indexes_past_the_table_miss_instead_of_wrapping() {
        /// The last word-aligned register offset a 32-bit address can form.
        const LAST_WORD: u32 = 0xFFFF_FFFC;
        let ctx = &mut Ctx::default();
        let mut mmu = MmuTable::new();
        for entry in 0..128 {
            mmu.write(ctx, 4 * entry, 0); // every entry valid: a hit would be a wrong index
        }
        assert_eq!(mmu.translate(0x7F_FFFF), Ok(0xFFFF), "the last byte");
        for window_offset in [0x80_0000, 0xFFFF_0000, u32::MAX] {
            assert_eq!(
                mmu.translate(window_offset),
                Err(MmuMiss {
                    entry: window_offset >> 16,
                    content: MMU_ENTRY_INVALID
                }),
                "{window_offset:#x}"
            );
        }
        assert_eq!(mmu.entry(127), Some(0));
        assert_eq!(mmu.entry(128), None);
        assert_eq!(mmu.entry(usize::MAX), None);
        assert_eq!(mmu.read(ctx, LAST_WORD), None);
        assert!(!mmu.write(ctx, LAST_WORD, 0));
    }

    #[test]
    fn every_write_changes_the_generation() {
        let ctx = &mut Ctx::default();
        let mut mmu = MmuTable::new();
        let first = mmu.generation();
        mmu.write(ctx, 0, 0);
        let second = mmu.generation();
        mmu.write(ctx, 0, 0);
        assert!(
            first != second && second != mmu.generation(),
            "even an identical value: the cost is one re-decode"
        );
    }
}
