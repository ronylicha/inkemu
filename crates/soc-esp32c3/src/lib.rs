//! ESP32-C3 system-on-chip model for inkemu.
//!
//! It holds the memory map ([`memmap`]), the chip revision ([`chip`]), the CPU clock tree
//! ([`clock`]) and run-time configuration ([`config`]), the flash chip ([`flash`]), the mask-ROM
//! image built from an Espressif ROM ELF ([`rom`]), the address space that routes each access to
//! the ROM, the RAMs, the flash windows or a peripheral ([`bus`]), the modelled peripherals
//! ([`periph`]), the diagnostics that report what is not modelled ([`diag`]), the machine that
//! runs one hart on all of it ([`machine`]), the journal of the inputs it accepted ([`inputs`]),
//! and what it counts of its own use for a resource monitor ([`usage`]).
//!
//! This crate performs no host I/O (no file, network or clock): callers hand it bytes.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod bus;
pub mod chip;
pub mod clock;
pub mod config;
pub mod ctx;
pub mod debug;
pub mod diag;
pub mod flash;
pub mod inputs;
pub mod machine;
pub mod memmap;
pub mod periph;
pub mod reset;
pub mod rom;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod usage;

pub use chip::ChipRevision;
pub use config::{CompatActs, QemuCompat, SocConfig, Timing};
pub use debug::{Access, DebugStop, Peeked, RunStop, Watch, WatchHit, WatchOn};
pub use diag::{Diagnostic, Wake};
pub use flash::{
    FlashConfig, FlashConfigError, FlashDevice, FlashImageError, FlashWrite, JedecId, NorFlash,
};
pub use inputs::{
    AcceptedInput, InputJournal, MAX_JOURNAL_BOARD_INPUTS, MAX_JOURNAL_CONSOLE_BYTES,
    MAX_JOURNAL_CONSOLE_CHUNKS, MAX_JOURNAL_HOST_CHANGES, MAX_JOURNAL_INPUTS,
};
pub use machine::{
    ConsoleChannel, DeepSleep, Esp32C3, Executed, InputError, MAX_PENDING_HOST_BYTES,
    MAX_PENDING_HOST_CHANGES, Stepped, StopReason,
};
pub use periph::io::MAX_PENDING_BOARD_INPUTS;
pub use periph::{AttachError, SpiWires, UsbHost};
pub use rom::{RomError, RomImage};
pub use usage::Usage;
