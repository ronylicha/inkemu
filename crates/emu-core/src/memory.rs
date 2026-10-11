//! A linear block of RAM, little-endian, with per-page write versions.
use crate::bus::{AccessKind, AccessSize, Bus, BusError, ReadOutcome};

/// Page granularity used for code-invalidation tracking.
pub const PAGE_SHIFT: u32 = 12;

/// Widest bus access in bytes (a 32-bit word).
const WORD_BYTES: usize = 4;

/// Versions are kept per ABSOLUTE 4 KiB page (`addr >> PAGE_SHIFT`), as `Bus::page_version` requires,
/// so a base that is not page-aligned must not shift the page boundaries.
#[derive(Debug, Clone)]
pub struct Ram {
    base: u32,
    data: Vec<u8>,
    /// Absolute page number of `page_versions[0]`.
    first_page: u32,
    page_versions: Vec<u64>,
    next_version: u64,
}

impl Ram {
    #[must_use]
    pub fn new(base: u32, len: usize) -> Ram {
        let first_page = base >> PAGE_SHIFT;
        let pages = if len == 0 {
            0
        } else {
            // Wide arithmetic: `base + len - 1` may exceed `u32` for a RAM that ends at the top of the address space.
            let last_byte = u64::from(base) + len as u64 - 1;
            (last_byte >> PAGE_SHIFT) as usize - first_page as usize + 1
        };
        Ram {
            base,
            data: vec![0; len],
            first_page,
            page_versions: vec![0; pages],
            next_version: 1,
        }
    }

    fn offset(&self, addr: u32, len: u32, kind: AccessKind) -> Result<usize, BusError> {
        let unmapped = BusError::Unmapped { addr, kind };
        let start = addr.checked_sub(self.base).ok_or(unmapped)? as usize;
        let end = start.checked_add(len as usize).ok_or(unmapped)?;
        if end > self.data.len() {
            return Err(unmapped);
        }
        Ok(start)
    }

    /// Marks the pages touched by `len` bytes at RAM offset `start` as written.
    fn bump_pages(&mut self, start: usize, len: usize) {
        let first = self.page_index(self.base_plus(start));
        let last = self.page_index(self.base_plus(start + len - 1));
        for page in &mut self.page_versions[first..=last] {
            *page = self.next_version;
        }
        self.next_version += 1;
    }

    /// Absolute address of the byte at RAM offset `offset`; callers have already range-checked it.
    fn base_plus(&self, offset: usize) -> u64 {
        u64::from(self.base) + offset as u64
    }

    /// Index into `page_versions` of the absolute page containing `addr`.
    fn page_index(&self, addr: u64) -> usize {
        (addr >> PAGE_SHIFT) as usize - self.first_page as usize
    }

    /// Copies `bytes` at `addr` (used to load images); bumps page versions.
    /// # Errors
    /// [`BusError::Unmapped`] if the range does not fit.
    pub fn load(&mut self, addr: u32, bytes: &[u8]) -> Result<(), BusError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let len = u32::try_from(bytes.len()).map_err(|_| BusError::Unmapped {
            addr,
            kind: AccessKind::Store,
        })?;
        let start = self.offset(addr, len, AccessKind::Store)?;
        self.data[start..start + bytes.len()].copy_from_slice(bytes);
        self.bump_pages(start, bytes.len());
        Ok(())
    }

    /// # Errors
    /// [`BusError::Unmapped`] if the range does not fit.
    pub fn bytes(&self, addr: u32, len: usize) -> Result<&[u8], BusError> {
        let len32 = u32::try_from(len).map_err(|_| BusError::Unmapped {
            addr,
            kind: AccessKind::Load,
        })?;
        let start = self.offset(addr, len32, AccessKind::Load)?;
        Ok(&self.data[start..start + len])
    }
}

impl Bus for Ram {
    fn read(
        &mut self,
        addr: u32,
        size: AccessSize,
        kind: AccessKind,
    ) -> Result<ReadOutcome, BusError> {
        let n = size.bytes() as usize;
        let start = self.offset(addr, size.bytes(), kind)?;
        let mut word = [0u8; WORD_BYTES];
        word[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(ReadOutcome {
            value: u32::from_le_bytes(word),
            stall: 0,
        })
    }

    fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError> {
        let n = size.bytes() as usize;
        let start = self.offset(addr, size.bytes(), AccessKind::Store)?;
        self.data[start..start + n].copy_from_slice(&value.to_le_bytes()[..n]);
        self.bump_pages(start, n);
        Ok(0)
    }

    fn page_version(&self, addr: u32) -> u64 {
        ((addr >> PAGE_SHIFT) as usize)
            .checked_sub(self.first_page as usize)
            .and_then(|index| self.page_versions.get(index))
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{AccessKind, AccessSize, Bus, BusError};
    #[test]
    fn little_endian_round_trip() {
        let mut ram = Ram::new(0x1000, 16);
        ram.write(0x1000, AccessSize::Word, 0x1122_3344).unwrap();
        assert_eq!(ram.bytes(0x1000, 4).unwrap(), &[0x44, 0x33, 0x22, 0x11]);
        assert_eq!(
            ram.read(0x1002, AccessSize::Half, AccessKind::Load)
                .unwrap()
                .value,
            0x1122
        );
    }
    #[test]
    fn access_outside_range_is_unmapped() {
        let mut ram = Ram::new(0x1000, 16);
        assert_eq!(
            ram.read(0x100E, AccessSize::Word, AccessKind::Load),
            Err(BusError::Unmapped {
                addr: 0x100E,
                kind: AccessKind::Load
            })
        );
        assert_eq!(
            ram.read(0x0FFF, AccessSize::Byte, AccessKind::Fetch),
            Err(BusError::Unmapped {
                addr: 0x0FFF,
                kind: AccessKind::Fetch
            })
        );
    }
    #[test]
    fn write_bumps_page_version_only_for_touched_page() {
        let mut ram = Ram::new(0, 3 * 4096);
        let v0 = ram.page_version(0);
        let v1 = ram.page_version(4096);
        ram.write(10, AccessSize::Byte, 1).unwrap();
        assert_ne!(ram.page_version(0), v0);
        assert_eq!(ram.page_version(4096), v1);
    }
    #[test]
    fn write_spanning_two_pages_bumps_both() {
        let mut ram = Ram::new(0, 2 * 4096);
        let (a, b) = (ram.page_version(0), ram.page_version(4096));
        ram.write(4094, AccessSize::Word, 0xFFFF_FFFF).unwrap();
        assert_ne!(ram.page_version(0), a);
        assert_ne!(ram.page_version(4096), b);
    }
    #[test]
    fn page_versions_follow_absolute_4k_pages_for_an_unaligned_base() {
        // 0x1800..0x3800: the absolute page 0x2000..0x2FFF straddles the RAM-relative 4 KiB boundary at 0x2800.
        let mut ram = Ram::new(0x1800, 0x2000);
        let (low_half, high_half) = (ram.page_version(0x27FE), ram.page_version(0x2800));
        ram.write(0x2800, AccessSize::Byte, 1).unwrap();
        assert_ne!(ram.page_version(0x2800), high_half);
        assert_ne!(
            ram.page_version(0x27FE),
            low_half,
            "0x27FE and 0x2800 share the absolute page 0x2000"
        );
        assert_eq!(ram.page_version(0x27FE), ram.page_version(0x2800));
        assert_eq!(ram.page_version(0x2000), ram.page_version(0x2FFF));
    }

    #[test]
    fn unaligned_base_keeps_neighbouring_absolute_pages_independent() {
        let mut ram = Ram::new(0x1800, 0x2000);
        let (first, second, third) = (
            ram.page_version(0x1800),
            ram.page_version(0x2000),
            ram.page_version(0x3000),
        );
        ram.write(0x1FFF, AccessSize::Byte, 1).unwrap();
        assert_ne!(ram.page_version(0x1800), first);
        assert_eq!(ram.page_version(0x2000), second);
        assert_eq!(ram.page_version(0x3000), third);
        ram.write(0x37FF, AccessSize::Byte, 1).unwrap();
        assert_ne!(ram.page_version(0x3000), third);
        assert_eq!(ram.page_version(0x2000), second);
    }

    #[test]
    fn page_version_is_zero_outside_the_covered_pages() {
        let ram = Ram::new(0x1800, 0x2000);
        assert_eq!(ram.page_version(0x0FFF), 0);
        assert_eq!(ram.page_version(0x4000), 0);
        assert_eq!(Ram::new(0x1800, 0).page_version(0x1800), 0);
    }

    #[test]
    fn load_copies_bytes_and_bumps_touched_pages() {
        let mut ram = Ram::new(0x1000, 2 * 4096);
        let (a, b) = (ram.page_version(0x1000), ram.page_version(0x1000 + 4096));
        ram.load(0x1000 + 4094, &[1, 2, 3, 4]).unwrap();
        assert_eq!(ram.bytes(0x1000 + 4094, 4).unwrap(), &[1, 2, 3, 4]);
        assert_ne!(ram.page_version(0x1000), a);
        assert_ne!(ram.page_version(0x1000 + 4096), b);
    }
    #[test]
    fn load_empty_slice_is_ok_and_changes_nothing() {
        let mut ram = Ram::new(0x1000, 16);
        let v = ram.page_version(0x1000);
        assert_eq!(ram.load(0x1000, &[]), Ok(()));
        assert_eq!(ram.page_version(0x1000), v);
        assert_eq!(ram.bytes(0x1000, 16).unwrap(), &[0u8; 16]);
    }
    #[test]
    fn load_out_of_range_is_unmapped_and_leaves_ram_untouched() {
        let mut ram = Ram::new(0x1000, 16);
        assert_eq!(
            ram.load(0x100C, &[0xAA; 8]),
            Err(BusError::Unmapped {
                addr: 0x100C,
                kind: AccessKind::Store
            })
        );
        assert_eq!(
            ram.load(0x0FFF, &[0xAA; 2]),
            Err(BusError::Unmapped {
                addr: 0x0FFF,
                kind: AccessKind::Store
            })
        );
        assert_eq!(ram.bytes(0x1000, 16).unwrap(), &[0u8; 16]);
    }
    #[test]
    fn bytes_out_of_range_is_unmapped() {
        let ram = Ram::new(0x1000, 16);
        assert_eq!(
            ram.bytes(0x100E, 4),
            Err(BusError::Unmapped {
                addr: 0x100E,
                kind: AccessKind::Load
            })
        );
    }
}
