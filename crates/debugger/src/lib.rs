//! The debugger of inkemu's front ends (design `SP4a`, ADR 0016).
//!
//! The machine gives the mechanism: it stops at a breakpoint or after a watched access without
//! changing what it executes, and reads its memory without effect (`soc_esp32c3::debug`). This
//! crate holds the policy and what an interface shows: the points a user may set and why one is
//! refused ([`points`]), the regions of the address space ([`region`]), and the views of a machine
//! ([`view`]): its CPU, its code, its memory — never the content of the flash, nor a peripheral
//! register a read could change.
//!
//! It performs no host I/O: the desktop application drives it through `host`, the web version
//! (sub-project 6) will drive it in a Web Worker, and the GDB server of sub-project 4d will set
//! the same points.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod points;
pub mod region;
pub mod view;

pub use points::{
    MAX_BREAKPOINTS, MAX_WATCH_LENGTH, MAX_WATCHPOINTS, PointError, Points, PointsView, WatchView,
    watch_on, watch_on_name,
};
pub use region::Region;
pub use view::{
    CpuView, CsrView, DebugView, Instruction, Line, MAX_DISASSEMBLY_LINES, MAX_MEMORY_BYTES,
    MemoryRequest, MemoryView, RegionSpan, RegisterView, StopView, ViewRequest, inspect,
};
