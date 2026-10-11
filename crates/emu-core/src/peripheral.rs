//! A memory-mapped device, seen as 32-bit registers.

/// The `SoC` bus splits sub-word accesses and resolves the device, so a peripheral only ever sees
/// whole registers at 4-byte-aligned offsets from its own base.
///
/// The `tick` that spec §3.2 lists arrives with the first time-dependent peripherals (SP1b-3).
pub trait Peripheral: std::fmt::Debug {
    /// `None` means this model does not implement the register: the bus then answers with the
    /// documented reset value and reports the access.
    fn read(&mut self, offset: u32) -> Option<u32>;
    /// `false` means this model does not implement the register: the write is dropped and reported.
    fn write(&mut self, offset: u32, value: u32) -> bool;
}
