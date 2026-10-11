//! ESP32-C3 system-on-chip model for inkemu.
//!
//! It holds the memory map ([`memmap`]), the chip revision ([`chip`]) and run-time configuration
//! ([`config`]), the flash chip ([`flash`]), the mask-ROM image built from an Espressif ROM ELF ([`rom`]), the address space that
//! routes each access to the ROM, the RAMs or a peripheral ([`bus`]), the modelled peripherals
//! ([`periph`]), the diagnostics that report what is not modelled ([`diag`]) and the machine that
//! runs one hart on all of it ([`machine`]).
//!
//! This crate performs no host I/O (no file, network or clock): callers hand it bytes.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod bus;
pub mod chip;
pub mod config;
pub mod diag;
pub mod flash;
pub mod machine;
pub mod memmap;
pub mod periph;
pub mod rom;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use chip::ChipRevision;
pub use config::SocConfig;
pub use diag::Diagnostic;
pub use flash::{FlashConfig, FlashDevice, FlashImageError, JedecId, NorFlash};
pub use machine::{ConsoleChannel, Esp32C3, StopReason};
pub use rom::{RomError, RomImage};
