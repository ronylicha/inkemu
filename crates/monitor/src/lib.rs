//! The resource monitor of inkemu's front ends (design `SP4b`, ADR 0017).
//!
//! The machine counts its own use where time and events pass already, and gives a reading of it
//! without effect (`soc_esp32c3::usage`, `Esp32C3::usage`). This crate says what a reading means
//! to a user ([`view()`]): where device time went and the load, the time at each CPU frequency, the
//! RAM pages written since power-on and since the last reset, the interrupt sources and the
//! peripheral blocks by their names in the SVD ([`names`]), and an estimate of the chip's
//! consumption from the datasheet, said as such ([`power`]). It shows counts, never the content
//! of memory or of the flash.
//!
//! It performs no host I/O: the desktop application drives it through `host`, and the web version
//! (sub-project 6) will drive it in a Web Worker.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod names;
pub mod power;
pub mod view;

pub use power::{PowerRow, PowerState, PowerView};
pub use view::{
    BlockView, ClockView, HostSide, LineView, MemoryRegion, MemoryView, MonitorView, PageState,
    PanelUse, RecentView, Refresh, SourceView, TimeView, view,
};
