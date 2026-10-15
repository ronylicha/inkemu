//! The SD card of a session (design of sub-project 2e, §7; ADR 0014): what the card held when the
//! run started, kept as its blocks that are not all zeros, and the fingerprint of what the last
//! life changed on it.
//!
//! A card of 1 GiB that a firmware formatted holds a few hundred kilobytes that are not zeros: a
//! session keeps those, in canonical ranges, and a replay rebuilds the card over them
//! ([`SdCardStart::source`]) without reading the image of the host. A card that holds more than
//! [`MAX_SD_NONZERO_BYTES`] is refused before the run is recorded.
use chips::sdcard::{
    BLOCK_BYTES, Block, BlockSource, Capacity, CapacityError, Changes, SparseBlocks, StorageError,
};
use sha2::{Digest as _, Sha256};
use std::fmt;

/// The most bytes of blocks that are not all zeros a session holds of a card: 64 MiB.
pub const MAX_SD_NONZERO_BYTES: usize = 64 << 20;
/// The most ranges of such blocks: one block each, at most.
pub const MAX_SD_RANGES: usize = MAX_SD_NONZERO_BYTES / BLOCK_BYTES;
/// The longest deflated data of a card, with the margin of [`crate::MAX_DEFLATED_FLASH_BYTES`].
pub const MAX_DEFLATED_SD_BYTES: usize = MAX_SD_NONZERO_BYTES + MAX_SD_NONZERO_BYTES / 1024 + 4096;

/// Blocks `first` to `first + count - 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockRange {
    pub first: u64,
    pub count: u32,
}

impl BlockRange {
    /// The block past the last.
    #[must_use]
    pub fn end(self) -> u64 {
        self.first.saturating_add(u64::from(self.count))
    }
}

/// Why the card a run starts with cannot be recorded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SdCardError {
    #[error(
        "the SD card holds {found} bytes in blocks that are not all zeros, more than the {max} a session holds; record the run with a card that holds less (a new image, or one with fewer files)"
    )]
    TooFull { found: u64, max: u64 },
    #[error("the SD card cannot be read: {0}")]
    Storage(StorageError),
    #[error("the SD card cannot be recorded: {0}")]
    Capacity(CapacityError),
}

/// The card a run started with: its size, whether it is write-protected, and its blocks that are
/// not all zeros, in ranges sorted, disjoint, not adjacent, not empty and within the card.
#[derive(Clone, PartialEq, Eq)]
pub struct SdCardStart {
    pub(crate) blocks: u64,
    pub(crate) write_protected: bool,
    pub(crate) ranges: Vec<BlockRange>,
    /// The blocks of the ranges, in order: `BLOCK_BYTES` times the sum of their counts.
    pub(crate) data: Vec<u8>,
}

impl fmt::Debug for SdCardStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SdCardStart")
            .field("blocks", &self.blocks)
            .field("write_protected", &self.write_protected)
            .field("ranges", &self.ranges.len())
            .field("data_bytes", &self.data.len())
            .finish()
    }
}

fn is_zero(block: &[u8]) -> bool {
    block.iter().all(|&byte| byte == 0)
}

impl SdCardStart {
    /// The card that `source` holds, read whole, for a card `write_protected` or not.
    ///
    /// # Errors
    /// [`SdCardError::Capacity`] for a size no card has, [`SdCardError::Storage`] when a block
    /// cannot be read, [`SdCardError::TooFull`] past [`MAX_SD_NONZERO_BYTES`], with the size
    /// found.
    pub fn read(
        source: &mut dyn BlockSource,
        write_protected: bool,
    ) -> Result<SdCardStart, SdCardError> {
        let blocks = source.block_count();
        Capacity::new(blocks.saturating_mul(BLOCK_BYTES as u64)).map_err(SdCardError::Capacity)?;
        let mut ranges: Vec<BlockRange> = Vec::new();
        let mut data = Vec::new();
        let mut found: u64 = 0;
        let mut block: Block = [0; BLOCK_BYTES];
        for lba in 0..blocks {
            source
                .read_block(lba, &mut block)
                .map_err(SdCardError::Storage)?;
            if is_zero(&block) {
                continue;
            }
            found += BLOCK_BYTES as u64;
            // Past the bound the blocks are counted, not kept: the refusal says how much the card
            // holds.
            if found > MAX_SD_NONZERO_BYTES as u64 {
                continue;
            }
            match ranges.last_mut() {
                Some(range) if range.end() == lba => range.count += 1,
                _ => ranges.push(BlockRange {
                    first: lba,
                    count: 1,
                }),
            }
            data.extend_from_slice(&block);
        }
        if found > MAX_SD_NONZERO_BYTES as u64 {
            return Err(SdCardError::TooFull {
                found,
                max: MAX_SD_NONZERO_BYTES as u64,
            });
        }
        Ok(SdCardStart {
            blocks,
            write_protected,
            ranges,
            data,
        })
    }

    /// The size of the card, in blocks of 512 bytes.
    #[must_use]
    pub fn blocks(&self) -> u64 {
        self.blocks
    }

    /// Whether the card refuses writes.
    #[must_use]
    pub fn write_protected(&self) -> bool {
        self.write_protected
    }

    /// The ranges of blocks that are not all zeros.
    #[must_use]
    pub fn ranges(&self) -> &[BlockRange] {
        &self.ranges
    }

    /// The bytes of those blocks, in order.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The SHA-256 of the card: its capacity (`u64`), then each range (`u64` first block, `u32`
    /// count) and its bytes, little-endian.
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(self.blocks.to_le_bytes());
        let mut at: usize = 0;
        for range in &self.ranges {
            digest.update(range.first.to_le_bytes());
            digest.update(range.count.to_le_bytes());
            let len = range.count as usize * BLOCK_BYTES;
            // The data holds every block of the ranges once the card is read; a card being
            // checked may hold fewer, and its fingerprint then covers what it holds.
            let end = at.saturating_add(len).min(self.data.len());
            digest.update(self.data.get(at..end).unwrap_or_default());
            at = end;
        }
        digest.finalize().into()
    }

    /// The content of the card, for a replay: every other block reads zeros.
    #[must_use]
    pub fn source(&self) -> SparseBlocks {
        let mut source = SparseBlocks::new(self.blocks);
        let mut blocks = self.data.as_chunks::<BLOCK_BYTES>().0.iter();
        for range in &self.ranges {
            for lba in range.first..range.end() {
                let block: Block = blocks.next().copied().unwrap_or([0; BLOCK_BYTES]);
                // The ranges lie within the card: checked when the session was read or recorded.
                let _ = source.insert(lba, block);
            }
        }
        source
    }
}

/// The SHA-256 of what a life changed on a card: each erased range (`u64` first block, `u64`
/// count), then each written block (`u64` number and its 512 bytes), in order, little-endian.
#[must_use]
pub fn changes_sha256(changes: &Changes<'_>) -> [u8; 32] {
    let mut digest = Sha256::new();
    for (first, count) in changes.erased() {
        digest.update(first.to_le_bytes());
        digest.update(count.to_le_bytes());
    }
    for (lba, block) in changes.blocks() {
        digest.update(lba.to_le_bytes());
        digest.update(block);
    }
    digest.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chips::sdcard::{BlockStorage as _, MemoryStorage, Overlay};

    fn numbered(blocks: usize, written: &[u64]) -> MemoryStorage {
        let mut storage = MemoryStorage::new(blocks);
        for &lba in written {
            let block = [u8::try_from(lba % 251 + 1).unwrap(); BLOCK_BYTES];
            storage.write_block(lba, &block).unwrap();
        }
        storage
    }

    #[test]
    fn a_card_is_kept_as_its_blocks_that_are_not_zeros_in_canonical_ranges() {
        let mut storage = numbered(64, &[0, 1, 2, 5, 63]);
        let start = SdCardStart::read(&mut storage, true).unwrap();
        assert_eq!((start.blocks(), start.write_protected()), (64, true));
        assert_eq!(
            start.ranges(),
            [
                BlockRange { first: 0, count: 3 },
                BlockRange { first: 5, count: 1 },
                BlockRange {
                    first: 63,
                    count: 1
                }
            ]
        );
        assert_eq!(start.data().len(), 5 * BLOCK_BYTES);
        let mut replayed = start.source();
        let mut block = [0; BLOCK_BYTES];
        for lba in 0..64 {
            let mut original = [0; BLOCK_BYTES];
            storage.read_block(lba, &mut original).unwrap();
            replayed.read_block(lba, &mut block).unwrap();
            assert_eq!(block, original, "block {lba}");
        }
    }

    #[test]
    fn the_fingerprint_follows_the_capacity_the_ranges_and_their_bytes() {
        let start = |blocks, written: &[u64]| {
            SdCardStart::read(&mut numbered(blocks, written), false)
                .unwrap()
                .fingerprint()
        };
        let base = start(64, &[3, 4]);
        assert_eq!(base, start(64, &[3, 4]));
        assert_ne!(base, start(128, &[3, 4]), "the capacity");
        assert_ne!(base, start(64, &[3, 5]), "the ranges");
        let mut other = SdCardStart::read(&mut numbered(64, &[3, 4]), false).unwrap();
        other.data[0] ^= 1;
        assert_ne!(base, other.fingerprint(), "the bytes");
    }

    #[test]
    fn a_card_past_the_bound_is_refused_with_the_size_found() {
        #[derive(Debug)]
        struct Full;
        /// A card of 128 MiB whose first blocks, two past the bound, are not zeros.
        impl BlockSource for Full {
            fn block_count(&self) -> u64 {
                2 * MAX_SD_RANGES as u64
            }
            fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError> {
                into.fill(if lba < MAX_SD_RANGES as u64 + 2 {
                    0xA5
                } else {
                    0
                });
                Ok(())
            }
        }
        assert_eq!(
            SdCardStart::read(&mut Full, false),
            Err(SdCardError::TooFull {
                found: (MAX_SD_NONZERO_BYTES + 2 * BLOCK_BYTES) as u64,
                max: MAX_SD_NONZERO_BYTES as u64
            })
        );
    }

    #[test]
    fn the_changes_of_a_life_are_fingerprinted_in_order() {
        let base = SparseBlocks::new(64);
        let mut overlay = Overlay::new(base);
        let empty = changes_sha256(&overlay.changes());
        overlay.write_block(7, &[1; BLOCK_BYTES]).unwrap();
        let written = changes_sha256(&overlay.changes());
        assert_ne!(empty, written);
        overlay.erase(10, 4).unwrap();
        assert_ne!(written, changes_sha256(&overlay.changes()));
    }
}
