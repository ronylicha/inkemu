//! What the machine counts of its own use, for a resource monitor (design `SP4b`, ADR 0017):
//! where device time went, at which CPU frequency, the interrupts raised and taken, the
//! peripheral registers the CPU reached, and the RAM pages written.
//!
//! The counts are kept where time or an event passes already (a sleep, a clock change, an
//! interrupt, a register access), never per instruction; the RAM pages are the write versions
//! the RAMs keep for the decode cache. Nothing the run does reads them, and a reading
//! ([`Esp32C3::usage`](crate::Esp32C3::usage)) changes nothing: observing the machine does not
//! change what it executes. What they mean to a user (shares, names, a datasheet estimate)
//! belongs to the front end: the `monitor` crate.
use crate::ctx::MAP_REGISTERS;
use emu_core::Cycles;

/// The peripheral window is counted in blocks of this many bytes: the SVD places its blocks at
/// least this far apart, but for `RNG`, a register inside `APB_CTRL`'s
/// (`docs/research/sp4b-resource-monitor.md` §1).
pub const BLOCK_BYTES: u32 = 0x800;

/// The peripheral interrupt sources the interrupt controller routes.
pub const SOURCES: usize = MAP_REGISTERS as usize;

/// The interrupt lines of the CPU, line 0 included (it is never taken: a source routed to it is
/// disabled).
pub const LINES: usize = 32;

/// A reading of what the machine counted since it was built (power-on): every count goes on
/// across resets, a reset only marks where the RAM writes since it start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    /// Device time since power-on, in cycles of the time base (`clock::TIME_BASE`).
    pub device_time: Cycles,
    /// CPU cycles and instructions since power-on.
    pub cycles: u64,
    pub instructions: u64,
    /// The CPU clock frequency now.
    pub cpu_hz: u64,
    /// Resets since power-on.
    pub resets: u64,
    /// Device time the chip spent in deep sleep, in cycles of the time base.
    pub deep_sleep: Cycles,
    /// The time the chip was awake at each CPU frequency, in the order each was first used.
    pub clocks: Vec<ClockUse>,
    /// The times each interrupt source was raised, as the interrupt controller saw it: a rise
    /// between two of its samplings at an instruction boundary. Source `n` is at index `n`.
    pub raised: Vec<u64>,
    /// The interrupts the CPU took on each of its lines: line `n` is at index `n`.
    pub taken: Vec<u64>,
    /// The register accesses of the CPU to each block of the peripheral window it reached, in
    /// address order: a block it never reached is not listed.
    pub blocks: Vec<BlockUse>,
    /// The RAMs: the SRAM (by its data-bus addresses), the SRAM0 and the RTC fast memory.
    pub memories: Vec<MemoryUse>,
}

/// The device time the chip spent awake at one CPU frequency, in cycles of the time base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockUse {
    pub hz: u64,
    /// Awake: executing or waiting in `wfi`.
    pub awake: Cycles,
    /// Waiting in `wfi` for an interrupt.
    pub waiting: Cycles,
}

/// The register accesses of the CPU to the [`BLOCK_BYTES`] bytes of the peripheral window from
/// `base`: loads and stores, whatever their width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockUse {
    pub base: u32,
    pub reads: u64,
    pub writes: u64,
}

/// The pages of one RAM and their write versions (`emu_core::Ram::page_versions`): a page at
/// version 0 was never written; one at or above `version_at_reset` was written since the last
/// reset (power-on included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryUse {
    pub base: u32,
    pub size: u32,
    pub page_bytes: u32,
    pub page_versions: Vec<u64>,
    /// The version the next write takes: the writes since power-on are `version - 1`.
    pub version: u64,
    /// The version the next write took when the last reset happened (1 before any).
    pub version_at_reset: u64,
}
