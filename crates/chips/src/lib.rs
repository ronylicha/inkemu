//! The chips around the `SoC` of a device: the SSD1677 e-paper controller of the X4, the button
//! ladders and the battery its SAR ADC reads (spec §3.2, `SP2a` design), the SD card it reads
//! over SPI with the block storage behind it (`SP2e` design), and the X3's UC8253 controller
//! (`SP3` design, ADR 0015). Both e-paper controllers share their glass ([`glass`]) and their
//! lines ([`lines`]). The X3's I2C bus carries a BQ27220 fuel gauge, a DS3231 real-time clock and
//! a QMI8658 motion sensor ([`i2c`]).
//!
//! A chip here does not know the `SoC`: it sees whole command and data bytes, a reset line, the
//! bytes of an SPI bus while it is selected, and the instant of device time each call happens at;
//! an analog source gives a voltage. Wiring them to the pins, the SPI controller and the ADC of a
//! `SoC` belongs to the device's board (`boards`).
//! Nothing here performs host I/O, reads a host clock or starts a thread: the crate builds for
//! `wasm32-unknown-unknown`.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod analog;
pub mod glass;
pub mod i2c;
pub mod lines;
pub mod sdcard;
pub mod ssd1677;
pub mod switch;
pub mod uc8253;

pub use analog::{Battery, BatteryError, ButtonLadder, LadderButton, LadderError, Millivolts};
pub use i2c::{
    I2cDiagnostic, I2cDiagnosticKind,
    bq27220::{Bq27220, Bq27220Config},
    ds3231::{DateTime, Ds3231, Ds3231Config},
    qmi8658::{Qmi8658, Qmi8658Config},
};
pub use sdcard::lines::{SdLines, SdPower};
pub use ssd1677::lines::{
    LineFault, Ssd1677Input, Ssd1677Inputs, Ssd1677Lines, Ssd1677Output, Ssd1677Outputs,
};
pub use ssd1677::{
    ConfigError, Diagnostic, DiagnosticKind, DisplayMode, GateScan, Optics, OtpWaveform,
    PanelGeometry, PanelImage, Plane, Provenance, RefreshKind, Ssd1677, Ssd1677Config,
    Ssd1677Timing, Stage, Temperature,
};
pub use switch::PushButton;
pub use uc8253::lines::{Uc8253Input, Uc8253Inputs, Uc8253Lines, Uc8253Output, Uc8253Outputs};
pub use uc8253::{Uc8253, Uc8253Config, Uc8253Geometry, Uc8253Timing};
