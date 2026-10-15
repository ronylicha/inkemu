//! The chips around the `SoC` of a device: the SSD1677 e-paper controller of the X4, the button
//! ladders and the battery its SAR ADC reads (spec §3.2, `SP2a` design), and the SD card it reads
//! over SPI with the block storage behind it (`SP2e` design).
//!
//! A chip here does not know the `SoC`: it sees whole command and data bytes, a reset line, the
//! bytes of an SPI bus while it is selected, and the instant of device time each call happens at;
//! an analog source gives a voltage. Wiring them to the pins, the SPI controller and the ADC of a
//! `SoC` belongs to the device's board (`boards`).
//! Nothing here performs host I/O, reads a host clock or starts a thread: the crate builds for
//! `wasm32-unknown-unknown`.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod analog;
pub mod sdcard;
pub mod ssd1677;

pub use analog::{Battery, BatteryError, ButtonLadder, LadderButton, LadderError, Millivolts};
pub use ssd1677::{
    ConfigError, Diagnostic, DiagnosticKind, DisplayMode, GateScan, Optics, OtpWaveform,
    PanelGeometry, PanelImage, Plane, Provenance, RefreshKind, Ssd1677, Ssd1677Config,
    Ssd1677Timing, Stage, Temperature,
};
