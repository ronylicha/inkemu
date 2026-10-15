//! The bus's counts for a resource monitor (design `SP4b`): the register accesses of the CPU by
//! block of the peripheral window, and the write versions of the RAM pages, which the RAMs keep
//! for the decode cache anyway. Nothing the run does reads them.
use super::SocBus;
use crate::debug::Access;
use crate::memmap::{
    PERIPH_BASE, PERIPH_SIZE, RTC_RAM_BASE, RTC_RAM_SIZE, SRAM_DBUS_BASE, SRAM_SIZE, SRAM0_BASE,
    SRAM0_SIZE,
};
use crate::usage::{BLOCK_BYTES, BlockUse, MemoryUse};
use emu_core::PAGE_SHIFT;

/// The blocks of the peripheral window.
pub(super) const BLOCKS: usize = (PERIPH_SIZE / BLOCK_BYTES) as usize;

// A RAM's pages are the 4 KiB pages of the address space (`Ram::page_versions`): a RAM that
// starts and ends on a page boundary shows its pages from its base, all of them its own.
const _: () = assert!(
    SRAM_DBUS_BASE.trailing_zeros() >= PAGE_SHIFT
        && SRAM0_BASE.trailing_zeros() >= PAGE_SHIFT
        && RTC_RAM_BASE.trailing_zeros() >= PAGE_SHIFT
        && SRAM_SIZE.trailing_zeros() >= PAGE_SHIFT
        && SRAM0_SIZE.trailing_zeros() >= PAGE_SHIFT
        && RTC_RAM_SIZE.trailing_zeros() >= PAGE_SHIFT
);

/// The register accesses of the CPU, reads and writes, by block of the peripheral window.
#[derive(Debug)]
pub(super) struct RegisterAccesses {
    counts: Box<[[u64; 2]]>,
}

impl RegisterAccesses {
    pub(super) fn new() -> RegisterAccesses {
        RegisterAccesses {
            counts: vec![[0; 2]; BLOCKS].into_boxed_slice(),
        }
    }
}

impl SocBus {
    /// The CPU accessed the register at `reg_addr`, an address of the peripheral window.
    pub(super) fn note_register(&mut self, reg_addr: u32, access: Access) {
        let block = reg_addr.wrapping_sub(PERIPH_BASE) / BLOCK_BYTES;
        if let Some(counts) = usize::try_from(block)
            .ok()
            .and_then(|block| self.registers.counts.get_mut(block))
        {
            let direction = match access {
                Access::Read => 0,
                Access::Write => 1,
            };
            counts[direction] += 1;
        }
    }

    /// The blocks the CPU reached, in address order.
    pub(crate) fn block_uses(&self) -> Vec<BlockUse> {
        self.registers
            .counts
            .iter()
            .zip((PERIPH_BASE..).step_by(BLOCK_BYTES as usize))
            .filter(|([reads, writes], _)| reads + writes > 0)
            .map(|(&[reads, writes], base)| BlockUse {
                base,
                reads,
                writes,
            })
            .collect()
    }

    /// The write version of the SRAM, the SRAM0 and the RTC memory: a mark of this instant.
    pub(crate) fn memory_versions(&self) -> [u64; 3] {
        [
            self.sram.version(),
            self.sram0.version(),
            self.rtc.version(),
        ]
    }

    /// The pages of the SRAM (by its data-bus addresses), the SRAM0 and the RTC memory, with
    /// their versions at the last reset (`at_reset`, as [`SocBus::memory_versions`] gives them).
    pub(crate) fn memory_uses(&self, at_reset: [u64; 3]) -> Vec<MemoryUse> {
        [
            (SRAM_DBUS_BASE, SRAM_SIZE, &self.sram),
            (SRAM0_BASE, SRAM0_SIZE, &self.sram0),
            (RTC_RAM_BASE, RTC_RAM_SIZE, &self.rtc),
        ]
        .into_iter()
        .zip(at_reset)
        .map(|((base, size, ram), version_at_reset)| MemoryUse {
            base,
            size,
            page_bytes: 1 << PAGE_SHIFT,
            page_versions: ram.page_versions().to_vec(),
            version: ram.version(),
            version_at_reset,
        })
        .collect()
    }
}
