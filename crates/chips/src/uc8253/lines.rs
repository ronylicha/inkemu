//! The X3's controller on its lines (ADR 0015): the adapter of the X4's SSD1677
//! ([`crate::lines::PanelLines`]), with `BUSY_N` active low and the bit-banged `REV` read
//! `FreeInk`'s probe makes before the SPI controller owns the pins (research `sp3-x3-firmware.md` §2.2).
use emu_core::{Cycles, Level};

use super::{ConfigError, Uc8253, Uc8253Config};
use crate::lines::{
    ONE_GIGAHERTZ, PanelController, PanelInput, PanelInputs, PanelLines, PanelOutput, PanelOutputs,
};

/// A UC8253 seen from its pins.
pub type Uc8253Lines = PanelLines<Uc8253>;
/// A line of the UC8253 that the `SoC` drives.
pub type Uc8253Output = PanelOutput<Uc8253>;
/// The lines the UC8253 hears.
pub type Uc8253Outputs = PanelOutputs<Uc8253>;
/// A line the UC8253 drives.
pub type Uc8253Input = PanelInput<Uc8253>;
/// The lines the UC8253 drives.
pub type Uc8253Inputs = PanelInputs<Uc8253>;

impl PanelLines<Uc8253> {
    /// The chip at power-on under `config`, counting time in nanoseconds.
    ///
    /// # Errors
    /// The parameters [`ConfigError`] lists.
    pub fn new(config: Uc8253Config) -> Result<Uc8253Lines, ConfigError> {
        Ok(PanelLines::wrap(Uc8253::new(config, ONE_GIGAHERTZ)?))
    }
}

impl PanelController for Uc8253 {
    fn write_command(&mut self, now: Cycles, command: u8) {
        Uc8253::write_command(self, now, command);
    }

    fn write_data(&mut self, now: Cycles, data: u8) {
        Uc8253::write_data(self, now, data);
    }

    fn is_read_command(&self, command: u8) -> bool {
        Uc8253::is_read_command(self, command)
    }

    fn read_data(&mut self, now: Cycles) -> Option<u8> {
        Uc8253::read_data(self, now)
    }

    fn set_reset(&mut self, now: Cycles, asserted: bool) {
        Uc8253::set_reset(self, now, asserted);
    }

    fn busy_level(&mut self, now: Cycles) -> Level {
        Uc8253::advance(self, now);
        self.busy_pin(now)
    }

    fn next_deadline(&self) -> Option<Cycles> {
        Uc8253::next_deadline(self)
    }

    fn advance(&mut self, now: Cycles) {
        Uc8253::advance(self, now);
    }
}
