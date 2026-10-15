//! What a card keeps its blocks in (design `SP2e`, 2e-D12): a store of 512-byte blocks the card
//! reads, writes and erases, behind two traits, and three stores that do no host I/O.
//!
//! - [`MemoryStorage`]: every block in memory, for small cards and for tests.
//! - [`Overlay`]: the blocks a life wrote, over a source it only reads (an image file of the host,
//!   a session's blocks). An image of several gibibytes stays where it is; the overlay holds what
//!   changed, and an erase of the whole card is one range, not gibibytes of zeros.
//! - [`SparseBlocks`]: a source that holds its non-zero blocks only, as a session carries a card.
//!
//! A block number is a `u64`; every store checks it against its size and answers
//! [`StorageError::OutOfRange`] past it, never a panic.
use std::collections::BTreeMap;
use std::fmt;

/// The bytes of a block.
pub const BLOCK_BYTES: usize = 512;

/// One block.
pub type Block = [u8; BLOCK_BYTES];

const ZERO_BLOCK: Block = [0; BLOCK_BYTES];

/// Why a store cannot do what it is asked.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StorageError {
    #[error("block {lba} lies past the end of the storage, which holds {blocks} blocks")]
    OutOfRange { lba: u64, blocks: u64 },
    #[error("{bytes} bytes are not a whole number of {BLOCK_BYTES}-byte blocks")]
    NotWholeBlocks { bytes: usize },
    /// A store backed by the host failed: `detail` is what the host said.
    #[error("cannot {action} block {lba}: {detail}")]
    Failed {
        action: &'static str,
        lba: u64,
        detail: String,
    },
}

/// A store of blocks that is only read: the content a card starts from.
pub trait BlockSource: fmt::Debug {
    /// How many blocks the store holds.
    fn block_count(&self) -> u64;

    /// Copies block `lba` into `into`.
    ///
    /// # Errors
    /// [`StorageError::OutOfRange`] past the last block; [`StorageError::Failed`] when the store
    /// cannot read it.
    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError>;
}

/// A store of blocks a card reads, writes and erases.
pub trait BlockStorage: BlockSource {
    /// Replaces block `lba` with `data`.
    ///
    /// # Errors
    /// [`StorageError::OutOfRange`] past the last block; [`StorageError::Failed`] when the store
    /// cannot write it.
    fn write_block(&mut self, lba: u64, data: &Block) -> Result<(), StorageError>;

    /// Sets the `count` blocks from `first` to zeros. Nothing is done when `count` is 0.
    ///
    /// # Errors
    /// [`StorageError::OutOfRange`] when the range goes past the last block (nothing is
    /// erased then); [`StorageError::Failed`] when the store cannot erase it.
    fn erase(&mut self, first: u64, count: u64) -> Result<(), StorageError>;
}

/// The end of the range of `count` blocks from `first`, when it lies within `blocks`.
fn range_end(first: u64, count: u64, blocks: u64) -> Result<u64, StorageError> {
    match first.checked_add(count) {
        Some(end) if end <= blocks => Ok(end),
        _ => Err(StorageError::OutOfRange {
            lba: first.max(blocks),
            blocks,
        }),
    }
}

/// Every block in memory.
#[derive(Clone, PartialEq, Eq)]
pub struct MemoryStorage {
    bytes: Vec<u8>,
}

impl fmt::Debug for MemoryStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStorage")
            .field("blocks", &self.block_count())
            .finish_non_exhaustive()
    }
}

impl MemoryStorage {
    /// `blocks` blocks of zeros.
    #[must_use]
    pub fn new(blocks: usize) -> MemoryStorage {
        MemoryStorage {
            bytes: vec![0; blocks.saturating_mul(BLOCK_BYTES)],
        }
    }

    /// The blocks `bytes` holds, the first block first.
    ///
    /// # Errors
    /// [`StorageError::NotWholeBlocks`] when the length is not a multiple of [`BLOCK_BYTES`].
    pub fn from_bytes(bytes: Vec<u8>) -> Result<MemoryStorage, StorageError> {
        if !bytes.len().is_multiple_of(BLOCK_BYTES) {
            return Err(StorageError::NotWholeBlocks { bytes: bytes.len() });
        }
        Ok(MemoryStorage { bytes })
    }

    /// Every block, the first block first.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Every block, the first block first.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The bytes of block `lba`, or the refusal.
    fn span(&self, lba: u64) -> Result<std::ops::Range<usize>, StorageError> {
        let out_of_range = || StorageError::OutOfRange {
            lba,
            blocks: self.block_count(),
        };
        let start = usize::try_from(lba)
            .ok()
            .and_then(|lba| lba.checked_mul(BLOCK_BYTES))
            .ok_or_else(out_of_range)?;
        let end = start.checked_add(BLOCK_BYTES).ok_or_else(out_of_range)?;
        if end > self.bytes.len() {
            return Err(out_of_range());
        }
        Ok(start..end)
    }
}

impl BlockSource for MemoryStorage {
    fn block_count(&self) -> u64 {
        // INVARIANT: a length in memory fits a `u64`.
        u64::try_from(self.bytes.len() / BLOCK_BYTES).unwrap_or(u64::MAX)
    }

    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError> {
        let span = self.span(lba)?;
        if let Some(bytes) = self.bytes.get(span) {
            into.copy_from_slice(bytes);
        }
        Ok(())
    }
}

impl BlockStorage for MemoryStorage {
    fn write_block(&mut self, lba: u64, data: &Block) -> Result<(), StorageError> {
        let span = self.span(lba)?;
        if let Some(bytes) = self.bytes.get_mut(span) {
            bytes.copy_from_slice(data);
        }
        Ok(())
    }

    fn erase(&mut self, first: u64, count: u64) -> Result<(), StorageError> {
        let end = range_end(first, count, self.block_count())?;
        // INVARIANT: both ends lie within the bytes in memory, which a `usize` counts.
        let to_byte = |block: u64| usize::try_from(block).unwrap_or(usize::MAX) * BLOCK_BYTES;
        if let Some(bytes) = self.bytes.get_mut(to_byte(first)..to_byte(end)) {
            bytes.fill(0);
        }
        Ok(())
    }
}

/// What a life changed in an [`Overlay`]: the ranges it erased, then the blocks it wrote,
/// each in the order of the blocks. A block written after an erase that covers it is listed as
/// written; its erase is undone by the write. Applying the ranges first, then the blocks, gives
/// the content the overlay reads.
#[derive(Clone, Copy, Debug)]
pub struct Changes<'a> {
    erased: &'a [(u64, u64)],
    written: &'a BTreeMap<u64, Box<Block>>,
}

impl<'a> Changes<'a> {
    /// The erased ranges, `(first, count)`, sorted, disjoint and not adjacent.
    pub fn erased(&self) -> impl Iterator<Item = (u64, u64)> + 'a {
        self.erased
            .iter()
            .map(|&(start, end)| (start, end.saturating_sub(start)))
    }

    /// The written blocks, by block number.
    pub fn blocks(&self) -> impl Iterator<Item = (u64, &'a Block)> + 'a {
        self.written.iter().map(|(&lba, block)| (lba, &**block))
    }

    /// How many blocks were written.
    #[must_use]
    pub fn written_count(&self) -> usize {
        self.written.len()
    }

    /// Whether nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.erased.is_empty() && self.written.is_empty()
    }
}

/// The blocks a card wrote and erased over a source it only reads.
#[derive(Debug)]
pub struct Overlay<S: BlockSource> {
    base: S,
    written: BTreeMap<u64, Box<Block>>,
    /// Half-open ranges `[start, end)` of erased blocks, sorted, disjoint and not adjacent.
    erased: Vec<(u64, u64)>,
}

impl<S: BlockSource> Overlay<S> {
    /// An overlay that has changed nothing yet.
    pub fn new(base: S) -> Overlay<S> {
        Overlay {
            base,
            written: BTreeMap::new(),
            erased: Vec::new(),
        }
    }

    /// What changed since [`Overlay::new`].
    #[must_use]
    pub fn changes(&self) -> Changes<'_> {
        Changes {
            erased: &self.erased,
            written: &self.written,
        }
    }

    /// The source under the changes.
    #[must_use]
    pub fn base(&self) -> &S {
        &self.base
    }

    /// The source under the changes; the changes are dropped.
    pub fn into_base(self) -> S {
        self.base
    }

    /// Whether block `lba` lies in an erased range.
    fn is_erased(&self, lba: u64) -> bool {
        let index = self.erased.partition_point(|&(_, end)| end <= lba);
        self.erased
            .get(index)
            .is_some_and(|&(start, _)| start <= lba)
    }

    fn check(&self, lba: u64) -> Result<(), StorageError> {
        range_end(lba, 1, self.base.block_count()).map(|_| ())
    }
}

impl<S: BlockSource> BlockSource for Overlay<S> {
    fn block_count(&self) -> u64 {
        self.base.block_count()
    }

    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError> {
        self.check(lba)?;
        if let Some(block) = self.written.get(&lba) {
            into.copy_from_slice(&**block);
            return Ok(());
        }
        if self.is_erased(lba) {
            into.copy_from_slice(&ZERO_BLOCK);
            return Ok(());
        }
        self.base.read_block(lba, into)
    }
}

impl<S: BlockSource> BlockStorage for Overlay<S> {
    fn write_block(&mut self, lba: u64, data: &Block) -> Result<(), StorageError> {
        self.check(lba)?;
        self.written.insert(lba, Box::new(*data));
        Ok(())
    }

    fn erase(&mut self, first: u64, count: u64) -> Result<(), StorageError> {
        let end = range_end(first, count, self.base.block_count())?;
        if count == 0 {
            return Ok(());
        }
        // The blocks written in the range are erased with it.
        let mut from_first = self.written.split_off(&first);
        let mut from_end = from_first.split_off(&end);
        self.written.append(&mut from_end);
        // The range joins every range it overlaps or touches.
        let low = self.erased.partition_point(|&(_, stop)| stop < first);
        let high = self.erased.partition_point(|&(start, _)| start <= end);
        let merged = match (
            self.erased.get(low),
            high.checked_sub(1).and_then(|last| self.erased.get(last)),
        ) {
            (Some(&(start, _)), Some(&(_, stop))) if low < high => {
                (start.min(first), stop.max(end))
            }
            _ => (first, end),
        };
        self.erased.splice(low..high.max(low), [merged]);
        Ok(())
    }
}

/// A source that holds only the blocks inserted in it; every other block reads zeros.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SparseBlocks {
    blocks: u64,
    data: BTreeMap<u64, Box<Block>>,
}

impl SparseBlocks {
    /// `blocks` blocks of zeros.
    #[must_use]
    pub fn new(blocks: u64) -> SparseBlocks {
        SparseBlocks {
            blocks,
            data: BTreeMap::new(),
        }
    }

    /// Sets block `lba` to `block`.
    ///
    /// # Errors
    /// [`StorageError::OutOfRange`] past the last block.
    pub fn insert(&mut self, lba: u64, block: Block) -> Result<(), StorageError> {
        range_end(lba, 1, self.blocks)?;
        self.data.insert(lba, Box::new(block));
        Ok(())
    }

    /// How many blocks were inserted.
    #[must_use]
    pub fn inserted(&self) -> usize {
        self.data.len()
    }
}

impl BlockSource for SparseBlocks {
    fn block_count(&self) -> u64 {
        self.blocks
    }

    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError> {
        range_end(lba, 1, self.blocks)?;
        into.copy_from_slice(self.data.get(&lba).map_or(&ZERO_BLOCK, |block| &**block));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(byte: u8) -> Block {
        [byte; BLOCK_BYTES]
    }

    fn read(store: &mut impl BlockSource, lba: u64) -> Block {
        let mut block = [0xAA; BLOCK_BYTES];
        store.read_block(lba, &mut block).unwrap();
        block
    }

    /// A source of `blocks` blocks, each filled with the low byte of its number plus one.
    #[derive(Debug)]
    struct Numbered {
        blocks: u64,
    }

    impl BlockSource for Numbered {
        fn block_count(&self) -> u64 {
            self.blocks
        }

        fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError> {
            range_end(lba, 1, self.blocks)?;
            into.fill(lba.to_le_bytes()[0].wrapping_add(1));
            Ok(())
        }
    }

    #[test]
    fn a_memory_store_starts_at_zeros_and_keeps_what_is_written() {
        let mut store = MemoryStorage::new(4);
        assert_eq!(store.block_count(), 4);
        assert_eq!(read(&mut store, 3), ZERO_BLOCK);
        store.write_block(2, &filled(7)).unwrap();
        assert_eq!(read(&mut store, 2), filled(7));
        assert_eq!(&store.bytes()[1024..1536], &[7; 512][..]);
        store.erase(2, 2).unwrap();
        assert_eq!(read(&mut store, 2), ZERO_BLOCK);
        assert_eq!(store.into_bytes().len(), 2048);
    }

    #[test]
    fn a_memory_store_refuses_bytes_that_are_not_whole_blocks_and_blocks_past_its_end() {
        assert_eq!(
            MemoryStorage::from_bytes(vec![0; 513]).unwrap_err(),
            StorageError::NotWholeBlocks { bytes: 513 }
        );
        let mut store = MemoryStorage::from_bytes(vec![1; 1024]).unwrap();
        let past = StorageError::OutOfRange { lba: 2, blocks: 2 };
        assert_eq!(store.read_block(2, &mut [0; 512]).unwrap_err(), past);
        assert_eq!(store.write_block(2, &ZERO_BLOCK).unwrap_err(), past);
        assert_eq!(
            store.read_block(u64::MAX, &mut [0; 512]).unwrap_err(),
            StorageError::OutOfRange {
                lba: u64::MAX,
                blocks: 2
            }
        );
        assert_eq!(
            store.erase(1, 2).unwrap_err(),
            StorageError::OutOfRange { lba: 2, blocks: 2 }
        );
        assert_eq!(
            store.erase(u64::MAX, 2).unwrap_err(),
            StorageError::OutOfRange {
                lba: u64::MAX,
                blocks: 2
            }
        );
        assert_eq!(
            read(&mut store, 1),
            filled(1),
            "a refused erase erases nothing"
        );
    }

    #[test]
    fn an_overlay_reads_its_source_until_a_block_is_written() {
        let mut overlay = Overlay::new(Numbered { blocks: 10 });
        assert_eq!(read(&mut overlay, 4), filled(5));
        overlay.write_block(4, &filled(0xEE)).unwrap();
        assert_eq!(read(&mut overlay, 4), filled(0xEE));
        assert_eq!(read(&mut overlay, 5), filled(6));
        assert_eq!(
            overlay.write_block(10, &ZERO_BLOCK).unwrap_err(),
            StorageError::OutOfRange {
                lba: 10,
                blocks: 10
            }
        );
        assert_eq!(overlay.base().blocks, 10);
    }

    #[test]
    fn an_erased_range_reads_zeros_and_a_block_written_in_it_afterwards_reads_what_was_written() {
        let mut overlay = Overlay::new(Numbered { blocks: 100 });
        overlay.write_block(10, &filled(9)).unwrap();
        overlay.write_block(30, &filled(3)).unwrap();
        overlay.erase(5, 20).unwrap();
        assert_eq!(
            read(&mut overlay, 10),
            ZERO_BLOCK,
            "the write before the erase is gone"
        );
        assert_eq!(read(&mut overlay, 24), ZERO_BLOCK);
        assert_eq!(read(&mut overlay, 25), filled(26));
        assert_eq!(read(&mut overlay, 4), filled(5));
        overlay.write_block(7, &filled(0x77)).unwrap();
        assert_eq!(read(&mut overlay, 7), filled(0x77));
        let changes = overlay.changes();
        assert_eq!(changes.erased().collect::<Vec<_>>(), [(5, 20)]);
        assert_eq!(
            changes
                .blocks()
                .map(|(lba, block)| (lba, block[0]))
                .collect::<Vec<_>>(),
            [(7, 0x77), (30, 3)]
        );
        assert_eq!(changes.written_count(), 2);
        assert!(!changes.is_empty());
    }

    #[test]
    fn erased_ranges_merge_when_they_overlap_or_touch_and_stay_sorted() {
        let mut overlay = Overlay::new(Numbered { blocks: 1000 });
        assert!(overlay.changes().is_empty());
        overlay.erase(100, 10).unwrap();
        overlay.erase(10, 10).unwrap();
        overlay.erase(300, 1).unwrap();
        overlay.erase(110, 5).unwrap(); // touches [100, 110)
        overlay.erase(0, 0).unwrap(); // nothing
        assert_eq!(
            overlay.changes().erased().collect::<Vec<_>>(),
            [(10, 10), (100, 15), (300, 1)]
        );
        overlay.erase(15, 290).unwrap(); // [15, 305) covers the three
        assert_eq!(overlay.changes().erased().collect::<Vec<_>>(), [(10, 295)]);
        overlay.erase(0, 5).unwrap();
        overlay.erase(999, 1).unwrap();
        assert_eq!(
            overlay.changes().erased().collect::<Vec<_>>(),
            [(0, 5), (10, 295), (999, 1)]
        );
        assert_eq!(
            overlay.erase(999, 2).unwrap_err(),
            StorageError::OutOfRange {
                lba: 1000,
                blocks: 1000
            },
            "the first block past the end"
        );
    }

    #[test]
    fn erasing_four_billion_blocks_keeps_one_range() {
        let blocks = 1u64 << 32;
        let mut overlay = Overlay::new(SparseBlocks::new(blocks));
        overlay.write_block(blocks - 1, &filled(1)).unwrap();
        overlay.erase(0, blocks).unwrap();
        assert_eq!(
            overlay.changes().erased().collect::<Vec<_>>(),
            [(0, blocks)]
        );
        assert_eq!(overlay.changes().written_count(), 0);
        assert_eq!(read(&mut overlay, blocks - 1), ZERO_BLOCK);
    }

    #[test]
    fn a_sparse_source_reads_zeros_where_nothing_was_inserted() {
        let mut sparse = SparseBlocks::new(8);
        sparse.insert(3, filled(0x33)).unwrap();
        assert_eq!(sparse.inserted(), 1);
        assert_eq!(read(&mut sparse, 3), filled(0x33));
        assert_eq!(read(&mut sparse, 4), ZERO_BLOCK);
        assert_eq!(
            sparse.insert(8, ZERO_BLOCK).unwrap_err(),
            StorageError::OutOfRange { lba: 8, blocks: 8 }
        );
        assert_eq!(
            sparse.read_block(8, &mut [0; 512]).unwrap_err(),
            StorageError::OutOfRange { lba: 8, blocks: 8 }
        );
        let mut overlay = Overlay::new(sparse);
        overlay.write_block(4, &filled(4)).unwrap();
        assert_eq!(read(&mut overlay, 3), filled(0x33));
        assert_eq!(overlay.into_base().inserted(), 1);
    }

    #[test]
    fn every_storage_error_says_what_failed() {
        assert_eq!(
            StorageError::OutOfRange { lba: 9, blocks: 8 }.to_string(),
            "block 9 lies past the end of the storage, which holds 8 blocks"
        );
        assert_eq!(
            StorageError::NotWholeBlocks { bytes: 7 }.to_string(),
            "7 bytes are not a whole number of 512-byte blocks"
        );
        assert_eq!(
            StorageError::Failed {
                action: "read",
                lba: 2,
                detail: "Input/output error".into()
            }
            .to_string(),
            "cannot read block 2: Input/output error"
        );
    }
}
