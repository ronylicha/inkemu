//! ESP32-C3 system-on-chip model for inkemu: memory map, chip revision and the mask-ROM image.
//!
//! This crate performs no host I/O (no file, network or clock): callers hand it bytes.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod chip;
pub mod memmap;
pub mod rom;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
