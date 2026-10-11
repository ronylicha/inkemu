//! Per-page decoded-instruction cache, invalidated through `Bus::page_version`.
use crate::instr::Decoded;
use emu_core::{Bus, PAGE_SHIFT};
use std::collections::HashMap;

const PAGE_SIZE: u32 = 1 << PAGE_SHIFT;
/// Instructions are 2-byte aligned with the C extension.
const SLOTS_PER_PAGE: usize = (PAGE_SIZE / 2) as usize;
/// Longest instruction encoding (RV32 base ISA).
const MAX_INSTR_BYTES: u32 = 4;

#[derive(Clone, Debug)]
struct Page {
    version: u64,
    slots: Box<[Option<Decoded>]>,
}

/// Decoded instructions keyed by page; a page is dropped lazily when its bus version moves.
/// The map is only ever looked up, never iterated, so the cache cannot leak ordering into the simulation.
#[derive(Clone, Debug, Default)]
pub struct DecodeCache {
    pages: HashMap<u32, Page>,
    hits: u64,
    misses: u64,
}

impl DecodeCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Index of the 2-byte slot holding `pc`, or `None` for an odd `pc`, which would alias the slot below it.
    fn slot(pc: u32) -> Option<usize> {
        (pc & 1 == 0).then_some(((pc & (PAGE_SIZE - 1)) >> 1) as usize)
    }

    /// Returns the decoded instruction at `pc` if the page has not been written since it was cached.
    pub fn lookup(&mut self, pc: u32, bus: &dyn Bus) -> Option<Decoded> {
        let found = Self::slot(pc).and_then(|slot| {
            self.pages
                .get(&(pc >> PAGE_SHIFT))
                .filter(|page| page.version == bus.page_version(pc))
                .and_then(|page| page.slots.get(slot).copied().flatten())
        });
        if found.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        found
    }

    pub fn insert(&mut self, pc: u32, decoded: Decoded, bus: &dyn Bus) {
        let Some(slot) = Self::slot(pc) else { return };
        // An instruction crossing into the next page would need two versions; it is rare, so it is not cached.
        if (pc & (PAGE_SIZE - 1)) + u32::from(decoded.len) > PAGE_SIZE {
            return;
        }
        let version = bus.page_version(pc);
        let page = self.pages.entry(pc >> PAGE_SHIFT).or_insert_with(|| Page {
            version,
            slots: vec![None; SLOTS_PER_PAGE].into_boxed_slice(),
        });
        if page.version != version {
            page.slots.fill(None);
            page.version = version;
        }
        if let Some(entry) = page.slots.get_mut(slot) {
            *entry = Some(decoded);
        }
    }

    /// `(hits, misses)` since creation.
    #[must_use]
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }
}

const _: () = assert!(MAX_INSTR_BYTES <= PAGE_SIZE);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode;
    use emu_core::{AccessSize, Bus, Ram};

    #[test]
    fn hit_after_insert_then_miss_after_write_to_same_page() {
        let mut ram = Ram::new(0, 8192);
        let mut cache = DecodeCache::new();
        assert!(cache.lookup(0x10, &ram).is_none());
        cache.insert(0x10, decode(0x0050_0093), &ram);
        assert_eq!(cache.lookup(0x10, &ram).map(|d| d.raw), Some(0x0050_0093));
        ram.write(0x800, AccessSize::Word, 0).unwrap(); // same 4 KiB page
        assert!(cache.lookup(0x10, &ram).is_none());
    }

    #[test]
    fn write_to_other_page_keeps_entry() {
        let mut ram = Ram::new(0, 8192);
        let mut cache = DecodeCache::new();
        cache.insert(0x10, decode(0x0050_0093), &ram);
        ram.write(0x1000, AccessSize::Word, 0).unwrap();
        assert!(cache.lookup(0x10, &ram).is_some());
    }

    #[test]
    fn instruction_straddling_a_page_is_never_cached() {
        let ram = Ram::new(0, 8192);
        let mut cache = DecodeCache::new();
        cache.insert(0xffe, decode(0x0050_0093), &ram);
        assert!(cache.lookup(0xffe, &ram).is_none());
    }

    #[test]
    fn odd_pc_is_never_cached_so_it_cannot_alias_the_even_slot() {
        let ram = Ram::new(0, 8192);
        let mut cache = DecodeCache::new();
        cache.insert(0x11, decode(0x0050_0093), &ram);
        assert!(cache.lookup(0x10, &ram).is_none());
        assert!(cache.lookup(0x11, &ram).is_none());
    }

    #[test]
    fn rewritten_page_is_recached_under_its_new_version() {
        let mut ram = Ram::new(0, 8192);
        let mut cache = DecodeCache::new();
        cache.insert(0x10, decode(0x0050_0093), &ram);
        ram.write(0x800, AccessSize::Word, 0).unwrap();
        cache.insert(0x14, decode(0x0070_0093), &ram); // stale slot at 0x10 must be dropped
        assert!(cache.lookup(0x10, &ram).is_none());
        assert_eq!(cache.lookup(0x14, &ram).map(|d| d.raw), Some(0x0070_0093));
        assert_eq!(cache.stats(), (1, 1));
    }
}
