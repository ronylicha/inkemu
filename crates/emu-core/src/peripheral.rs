//! A memory-mapped device, seen as 32-bit registers.

/// The `SoC` bus splits sub-word accesses and resolves the device, so a peripheral only ever sees
/// whole registers at 4-byte-aligned offsets from its own base.
///
/// `C` is the context the `SoC` hands to every access: through it, and through nothing else, a
/// peripheral reads device time, drives its interrupt lines, asks to be called back at an instant
/// and reports what it cannot do (ADR 0003). The `tick` of spec §3.2 is that call-back: a
/// peripheral is called at the instant it asked for, never polled.
pub trait Peripheral<C: ?Sized>: std::fmt::Debug {
    /// `None` means this model does not implement the register: the bus then answers with the
    /// documented reset value and reports the access.
    fn read(&mut self, ctx: &mut C, offset: u32) -> Option<u32>;
    /// `false` means this model does not implement the register: the write is dropped and reported.
    fn write(&mut self, ctx: &mut C, offset: u32, value: u32) -> bool;
}
