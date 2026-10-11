//! A linear block of RAM, little-endian, with per-page write versions.
use crate::bus::{AccessKind, AccessSize, Bus, BusError, ReadOutcome};

/// Page granularity used for code-invalidation tracking.
pub const PAGE_SHIFT: u32 = 12;

#[derive(Debug, Clone)]
pub struct Ram {
    base: u32,
    data: Vec<u8>,
    page_versions: Vec<u64>,
    next_version: u64,
}

impl Ram {
    #[must_use]
    pub fn new(base: u32, len: usize) -> Ram {
        let pages = len.div_ceil(1 << PAGE_SHIFT);
        Ram {
            base,
            data: vec![0; len],
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

    fn bump_pages(&mut self, start: usize, len: usize) {
        let first = start >> PAGE_SHIFT;
        let last = (start + len - 1) >> PAGE_SHIFT;
        for page in &mut self.page_versions[first..=last] {
            *page = self.next_version;
        }
        self.next_version += 1;
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
        let mut word = [0u8; 4];
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
        addr.checked_sub(self.base)
            .and_then(|off| self.page_versions.get((off >> PAGE_SHIFT) as usize))
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
}
