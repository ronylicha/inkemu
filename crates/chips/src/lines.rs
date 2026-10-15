//! An e-paper controller on its lines: what a board attaches to the pins of a `SoC` (design of
//! sub-project 2d, 2d-D2 to 2d-D5; ADR 0012; ADR 0015 for the X3's UC8253).
//!
//! [`PanelLines`] holds a controller and implements the two signal traits it answers as a whole:
//! [`SpiTarget`] (the bytes a hardware SPI controller sends, D/C# telling commands from data) and
//! [`Clocked`] (BUSY changes and images are published at instants of its own). Its other lines
//! are handles on the same chip, shared by `Rc<RefCell<_>>`: [`PanelLines::outputs`] gives what the
//! chip hears (RES#, and SCL, SDA and D/C# toggled one by one by software), and
//! [`PanelLines::inputs`] what it drives (BUSY, and SDA during a read). The controller itself is
//! any [`PanelController`]: the SSD1677 of the X4, the UC8253 of the X3.
//!
//! The chip counts time in cycles of a 1 GHz base, so that an instant of the signals (`Nanos`) is
//! an instant of the chip without conversion.
//!
//! **Bit-banged traffic.** A firmware may toggle the lines from software instead of using its SPI
//! controller: `FreeInk` probes both panels that way before the controller owns the pins
//! (`XteinkDetect.cpp:50-90` and `:182-250` at `111fdcc7`). While the chip is selected, each rising
//! edge of SCL samples SDA, most significant bit first; the eighth makes a command (D/C# low) or a
//! datum (D/C# high). After a read command, D/C# high, the chip drives SDA itself: the byte comes
//! from [`PanelController::read_data`], its most significant bit at once, the next one at each
//! falling edge of SCL; a byte the chip does not give leaves the line to its pull. CS# high or D/C#
//! low ends the read. A firmware that samples before each rising edge (the X4's probe) and one
//! that samples after it (the X3's) read the same bits.
use std::cell::RefCell;
use std::num::NonZeroU64;
use std::rc::Rc;

use emu_core::{Clocked, Cycles, Frequency, GpioInput, GpioOutput, Level, Nanos, SpiTarget};

/// The time base of a wired controller: one cycle per nanosecond.
pub(crate) const ONE_GIGAHERTZ: Frequency = match NonZeroU64::new(1_000_000_000) {
    Some(hz) => Frequency::new(hz),
    // INVARIANT: a non-zero constant, evaluated at compile time.
    None => panic!("a clock frequency is never zero"),
};

/// What a controller does with the bytes and lines its pins carry, at instants of its own time
/// base (cycles of 1 GHz once wired).
pub trait PanelController: std::fmt::Debug {
    /// A command byte, D/C# low.
    fn write_command(&mut self, now: Cycles, command: u8);
    /// A data byte, D/C# high.
    fn write_data(&mut self, now: Cycles, data: u8);
    /// Whether `command` is one whose data bytes the MCU reads.
    fn is_read_command(&self, command: u8) -> bool;
    /// The next byte of the read in progress, `None` when the chip drives nothing.
    fn read_data(&mut self, now: Cycles) -> Option<u8>;
    /// RES# went low (`asserted`) or high.
    fn set_reset(&mut self, now: Cycles, asserted: bool);
    /// The level the chip drives on BUSY at `now`, the chip brought to `now` first.
    fn busy_level(&mut self, now: Cycles) -> Level;
    /// The next instant BUSY changes or an image is published.
    fn next_deadline(&self) -> Option<Cycles>;
    /// Brings the chip to `now`.
    fn advance(&mut self, now: Cycles);
}

/// A fault of the wiring, not of the chip: what the board did that the chip cannot take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineFault {
    /// A transfer reached the chip with no D/C# line: its bytes are neither commands nor data.
    TransferWithoutDc { at: Nanos },
}

/// The bit-banged read in progress: the byte the chip drives and the bit it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reading {
    byte: Option<u8>,
    bit: u8,
}

/// The bit-banged traffic as the chip hears it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Lines {
    selected: bool,
    sclk: Option<Level>,
    sda: Option<Level>,
    dc: Option<Level>,
    /// Bits sampled since the last whole byte.
    shift: u8,
    bits: u8,
    /// The last byte written was a read command: the read starts when D/C# goes high.
    read_armed: bool,
    reading: Option<Reading>,
}

/// A controller seen from its pins.
#[derive(Debug)]
pub struct PanelLines<C: PanelController> {
    pub(crate) chip: C,
    lines: Lines,
    faults: Vec<LineFault>,
}

/// The faults kept until [`PanelLines::take_faults`]; past it they are dropped.
pub const MAX_LINE_FAULTS: usize = 64;

impl<C: PanelController> PanelLines<C> {
    /// `chip` on its lines, nothing heard yet.
    pub(crate) fn wrap(chip: C) -> PanelLines<C> {
        PanelLines {
            chip,
            lines: Lines::default(),
            faults: Vec::new(),
        }
    }

    /// The chip, for what a board reads of it (its image, its diagnostics).
    #[must_use]
    pub fn chip(&self) -> &C {
        &self.chip
    }

    /// The chip, for what a board takes from it (its diagnostics).
    pub fn chip_mut(&mut self) -> &mut C {
        &mut self.chip
    }

    /// The faults of the wiring since the last call, oldest first.
    pub fn take_faults(&mut self) -> Vec<LineFault> {
        std::mem::take(&mut self.faults)
    }

    /// The lines the chip hears, each a handle on `shared`.
    #[must_use]
    pub fn outputs(shared: &Rc<RefCell<PanelLines<C>>>) -> PanelOutputs<C> {
        let line = |role| PanelOutput {
            lines: Rc::clone(shared),
            role,
        };
        PanelOutputs {
            reset: line(OutputRole::Reset),
            clock: line(OutputRole::Clock),
            data: line(OutputRole::Data),
            data_command: line(OutputRole::DataCommand),
        }
    }

    /// The lines the chip drives, each a handle on `shared`.
    #[must_use]
    pub fn inputs(shared: &Rc<RefCell<PanelLines<C>>>) -> PanelInputs<C> {
        let line = |role| PanelInput {
            lines: Rc::clone(shared),
            role,
        };
        PanelInputs {
            busy: line(InputRole::Busy),
            data: line(InputRole::Data),
        }
    }

    fn fault(&mut self, fault: LineFault) {
        if self.faults.len() < MAX_LINE_FAULTS {
            self.faults.push(fault);
        }
    }

    /// Forgets the bits of a byte not finished and any read: the chip was deselected, or a
    /// transfer of the SPI controller took the bus.
    fn drop_bits(&mut self) {
        let lines = &mut self.lines;
        lines.shift = 0;
        lines.bits = 0;
        lines.read_armed = false;
        lines.reading = None;
    }

    /// Hands a whole byte to the chip as D/C# says.
    fn byte(&mut self, dc: Level, byte: u8, at: Nanos) {
        let now = Cycles(at.get());
        match dc {
            Level::Low => {
                self.chip.write_command(now, byte);
                self.lines.read_armed = self.chip.is_read_command(byte);
            }
            Level::High => self.chip.write_data(now, byte),
        }
    }

    fn start_reading(&mut self, at: Nanos) {
        self.lines.read_armed = false;
        let byte = self.chip.read_data(Cycles(at.get()));
        self.lines.reading = Some(Reading { byte, bit: 0 });
    }

    fn clock(&mut self, level: Option<Level>, at: Nanos) {
        let before = self.lines.sclk;
        self.lines.sclk = level;
        if !self.lines.selected {
            return;
        }
        match (before, level) {
            (Some(Level::Low), Some(Level::High)) => {
                if self.lines.reading.is_some() {
                    return;
                }
                let bit = u8::from(self.lines.sda == Some(Level::High));
                self.lines.shift = (self.lines.shift << 1) | bit;
                self.lines.bits += 1;
                if self.lines.bits == 8 {
                    let byte = self.lines.shift;
                    self.lines.shift = 0;
                    self.lines.bits = 0;
                    // ASSUMPTION: a released D/C# counts as data, the level its pad most likely
                    // reads (an input with the pad's pull-up); FreeInk always drives it.
                    let dc = self.lines.dc.unwrap_or(Level::High);
                    self.byte(dc, byte, at);
                }
            }
            (Some(Level::High), Some(Level::Low)) => {
                if let Some(reading) = &mut self.lines.reading {
                    reading.bit += 1;
                    if reading.bit == 8 {
                        self.start_reading(at);
                    }
                }
            }
            _ => {}
        }
    }

    fn data_command(&mut self, level: Option<Level>, at: Nanos) {
        self.lines.dc = level;
        match level {
            Some(Level::High) if self.lines.selected && self.lines.read_armed => {
                self.start_reading(at);
            }
            Some(Level::Low) => self.lines.reading = None,
            _ => {}
        }
    }

    fn driven_data(&self) -> Option<Level> {
        let reading = self.lines.reading?;
        let byte = reading.byte?;
        Some(Level::from_bool(byte & (0x80 >> reading.bit) != 0))
    }
}

impl<C: PanelController> SpiTarget for PanelLines<C> {
    fn select(&mut self, selected: bool, _at: Nanos) {
        self.lines.selected = selected;
        self.drop_bits();
    }

    /// Each byte is a command with D/C# low, a datum with it high. The panel has no output on the
    /// bus: `miso` is left as it came.
    fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], _miso: &mut [u8], at: Nanos) {
        self.drop_bits();
        let Some(dc) = dc else {
            self.fault(LineFault::TransferWithoutDc { at });
            return;
        };
        for &byte in mosi {
            self.byte(dc, byte, at);
        }
        // A read command sent by the controller has nothing to read back on this panel.
        self.lines.read_armed = false;
    }
}

impl<C: PanelController> Clocked for PanelLines<C> {
    fn next_deadline(&mut self, _at: Nanos) -> Option<Nanos> {
        self.chip.next_deadline().map(|at| Nanos(at.get()))
    }

    fn advance(&mut self, at: Nanos) {
        self.chip.advance(Cycles(at.get()));
    }
}

/// Which line the chip hears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputRole {
    Reset,
    Clock,
    Data,
    DataCommand,
}

/// A line of the chip that the `SoC` drives.
#[derive(Debug)]
pub struct PanelOutput<C: PanelController> {
    lines: Rc<RefCell<PanelLines<C>>>,
    role: OutputRole,
}

impl<C: PanelController> GpioOutput for PanelOutput<C> {
    fn driven(&mut self, level: Option<Level>, at: Nanos) {
        let mut lines = self.lines.borrow_mut();
        match self.role {
            // RES# has a pull-up in the controller (FreeInk: "RST_N has an internal pull-up"): a
            // released line keeps it out of reset.
            OutputRole::Reset => lines
                .chip
                .set_reset(Cycles(at.get()), level == Some(Level::Low)),
            OutputRole::Clock => lines.clock(level, at),
            OutputRole::Data => lines.lines.sda = level,
            OutputRole::DataCommand => lines.data_command(level, at),
        }
    }
}

/// The lines the chip hears: attach each to its pin as a `GpioOutput`.
#[derive(Debug)]
pub struct PanelOutputs<C: PanelController> {
    /// RES#.
    pub reset: PanelOutput<C>,
    /// SCL, the clock.
    pub clock: PanelOutput<C>,
    /// SDA, the data line, as the `SoC` drives it.
    pub data: PanelOutput<C>,
    /// D/C#.
    pub data_command: PanelOutput<C>,
}

/// Which line the chip drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputRole {
    Busy,
    Data,
}

/// A line the chip drives.
#[derive(Debug)]
pub struct PanelInput<C: PanelController> {
    lines: Rc<RefCell<PanelLines<C>>>,
    role: InputRole,
}

impl<C: PanelController> GpioInput for PanelInput<C> {
    fn level(&mut self, at: Nanos) -> Option<Level> {
        let mut lines = self.lines.borrow_mut();
        match self.role {
            InputRole::Busy => Some(lines.chip.busy_level(Cycles(at.get()))),
            InputRole::Data => lines.driven_data(),
        }
    }

    /// BUSY may change at the chip's next deadline; SDA changes only on an edge of SCL.
    fn next_change(&mut self, at: Nanos) -> Option<Nanos> {
        match self.role {
            InputRole::Busy => self
                .lines
                .borrow()
                .chip
                .next_deadline()
                .map(|next| Nanos(next.get()))
                .filter(|&next| next > at),
            InputRole::Data => None,
        }
    }
}

/// The lines the chip drives: attach each to its pin as a `GpioInput`.
#[derive(Debug)]
pub struct PanelInputs<C: PanelController> {
    /// BUSY.
    pub busy: PanelInput<C>,
    /// SDA during a read.
    pub data: PanelInput<C>,
}
