//! The memory-mapped bus seen by a CPU core.
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessSize {
    Byte,
    Half,
    Word,
}

impl AccessSize {
    #[must_use]
    pub const fn bytes(self) -> u32 {
        match self {
            AccessSize::Byte => 1,
            AccessSize::Half => 2,
            AccessSize::Word => 4,
        }
    }
}

/// Why the bus is accessed; peripherals and fault reporting need the distinction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessKind {
    Fetch,
    Load,
    Store,
}

/// A completed read and the wait states it cost on top of the instruction's base cycles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadOutcome {
    pub value: u32,
    pub stall: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum BusError {
    #[error("no device is mapped at {addr:#010x} ({kind:?}); check the device memory map")]
    Unmapped { addr: u32, kind: AccessKind },
    #[error(
        "misaligned {size:?} access at {addr:#010x}; align the address to the access size \
         or let the CPU core handle unaligned accesses"
    )]
    Misaligned { addr: u32, size: AccessSize },
}

/// Reads may have side effects on peripherals, hence `&mut self`.
///
/// Sub-word accesses use a right-aligned value: a [`AccessSize::Byte`] read returns its byte in
/// bits 7..0 and a [`AccessSize::Half`] read its halfword in bits 15..0, with every higher bit
/// zero. A peripheral that exposes a wider register must shift and mask before answering.
pub trait Bus {
    /// The returned value is right-aligned and zero-extended to the access size.
    /// # Errors
    /// Returns [`BusError`] when nothing answers at `addr`.
    fn read(
        &mut self,
        addr: u32,
        size: AccessSize,
        kind: AccessKind,
    ) -> Result<ReadOutcome, BusError>;

    /// Writes the low `size` bytes of `value`; bits above the access size are ignored.
    /// Returns the stall cycles of the write.
    /// # Errors
    /// Returns [`BusError`] when nothing answers at `addr`.
    fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError>;

    /// A counter that changes whenever the 4 KiB page containing `addr` may hold new bytes.
    /// Decode caches compare it to detect self-modifying code and DMA writes.
    fn page_version(&self, addr: u32) -> u64;

    /// Whether the `len` bytes a fetch at `addr` returned are memory content, which stays the same
    /// until [`Bus::page_version`] changes. A `SoC` answers `false` when the fetch was served
    /// without memory behind it (a flash window page no MMU entry maps): the instruction is then
    /// decoded again at every execution, so the bus sees, and reports, each faulty fetch.
    fn fetch_is_memory(&self, addr: u32, len: u32) -> bool {
        let _ = (addr, len);
        true
    }

    /// Timing-only hook called when an instruction is served from a decode cache:
    /// the `SoC` charges what the fetch would have cost (e.g. flash-cache hit or miss) without moving data.
    fn fetch_stall(&mut self, addr: u32, len: u32) -> u32 {
        let _ = (addr, len);
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn misaligned_message_names_the_correction() {
        let msg = BusError::Misaligned {
            addr: 0x1001,
            size: AccessSize::Word,
        }
        .to_string();
        assert!(
            msg.contains("align the address to the access size"),
            "{msg}"
        );
    }
}
