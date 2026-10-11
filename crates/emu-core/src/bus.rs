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
    #[error("misaligned {size:?} access at {addr:#010x}")]
    Misaligned { addr: u32, size: AccessSize },
}

/// Reads may have side effects on peripherals, hence `&mut self`.
pub trait Bus {
    /// # Errors
    /// Returns [`BusError`] when nothing answers at `addr`.
    fn read(
        &mut self,
        addr: u32,
        size: AccessSize,
        kind: AccessKind,
    ) -> Result<ReadOutcome, BusError>;

    /// Returns the stall cycles of the write.
    /// # Errors
    /// Returns [`BusError`] when nothing answers at `addr`.
    fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError>;

    /// A counter that changes whenever the 4 KiB page containing `addr` may hold new bytes.
    /// Decode caches compare it to detect self-modifying code and DMA writes.
    fn page_version(&self, addr: u32) -> u64;
}
