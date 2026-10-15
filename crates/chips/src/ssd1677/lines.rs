//! The SSD1677 on its lines: what a board attaches to the pins of a `SoC` (design of sub-project 2d,
//! 2d-D2 to 2d-D5; ADR 0012). The adapter is [`crate::lines::PanelLines`], shared with the X3's
//! UC8253; this module says what the SSD1677 does on it and keeps the names the X4's board uses.
use emu_core::{Cycles, Level};

use super::{ConfigError, Ssd1677, Ssd1677Config};
use crate::lines::{
    PanelController, PanelInput, PanelInputs, PanelLines, PanelOutput, PanelOutputs,
};

#[cfg(test)]
use crate::lines::ONE_GIGAHERTZ;
pub use crate::lines::{LineFault, MAX_LINE_FAULTS};

/// An SSD1677 seen from its pins.
pub type Ssd1677Lines = PanelLines<Ssd1677>;
/// A line of the SSD1677 that the `SoC` drives.
pub type Ssd1677Output = PanelOutput<Ssd1677>;
/// The lines the SSD1677 hears.
pub type Ssd1677Outputs = PanelOutputs<Ssd1677>;
/// A line the SSD1677 drives.
pub type Ssd1677Input = PanelInput<Ssd1677>;
/// The lines the SSD1677 drives.
pub type Ssd1677Inputs = PanelInputs<Ssd1677>;

impl PanelLines<Ssd1677> {
    /// The chip at power-on under `config`, counting time in nanoseconds.
    ///
    /// # Errors
    /// The parameters [`ConfigError`] lists.
    pub fn new(config: Ssd1677Config) -> Result<Ssd1677Lines, ConfigError> {
        Ok(PanelLines::wrap(Ssd1677::new(
            config,
            crate::lines::ONE_GIGAHERTZ,
        )?))
    }
}

impl PanelController for Ssd1677 {
    fn write_command(&mut self, now: Cycles, command: u8) {
        Ssd1677::write_command(self, now, command);
    }

    fn write_data(&mut self, now: Cycles, data: u8) {
        Ssd1677::write_data(self, now, data);
    }

    fn is_read_command(&self, command: u8) -> bool {
        Ssd1677::is_read_command(command)
    }

    fn read_data(&mut self, now: Cycles) -> Option<u8> {
        Ssd1677::read_data(self, now)
    }

    fn set_reset(&mut self, now: Cycles, asserted: bool) {
        Ssd1677::set_reset(self, now, asserted);
    }

    fn busy_level(&mut self, now: Cycles) -> Level {
        Ssd1677::advance(self, now);
        Level::from_bool(self.busy_pin(now))
    }

    fn next_deadline(&self) -> Option<Cycles> {
        Ssd1677::next_deadline(self)
    }

    fn advance(&mut self, now: Cycles) {
        Ssd1677::advance(self, now);
    }
}

#[cfg(test)]
mod tests;
