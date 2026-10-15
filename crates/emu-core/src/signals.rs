//! The signals between a system-on-chip and the chips around it (spec §3.2, ADR 0010).
//!
//! A chip outside the `SoC` (a display controller, a memory card, a button ladder) never sees the
//! `SoC`: it sees lines. Four traits say what a chip does on them, and nothing here knows which `SoC`
//! drives the other end:
//!
//! - [`GpioInput`]: the chip drives a line the `SoC` reads (a button, a BUSY line);
//! - [`GpioOutput`]: the chip watches a line the `SoC` drives (a reset, a power latch);
//! - [`SpiTarget`]: the chip sits on an SPI bus the `SoC` masters, selected by its chip-select line,
//!   with an optional data/command line;
//! - [`AnalogSource`]: the chip puts a voltage on an analog input.
//!
//! The `SoC` calls them; a chip never calls the `SoC`. Time is device time in nanoseconds since
//! power-on ([`Nanos`]), not cycles of a clock of the `SoC`: a chip counts its own durations (a
//! refresh, a reset pulse) in real time. A chip whose lines depend only on its state and on that
//! time gives a deterministic run.
//!
//! A chip that has several lines (the BUSY output and the SPI interface of a display controller)
//! is attached once per line; `Rc<RefCell<T>>` implements the four traits for `T`, so the same chip
//! can be shared between its attachments. The `SoC` never calls a chip from within a call to that
//! chip, so the `RefCell` is never borrowed twice.
use std::cell::RefCell;
use std::rc::Rc;

/// The logic level of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    Low,
    High,
}

impl Level {
    /// `High` for `true`, `Low` for `false`.
    #[must_use]
    pub const fn from_bool(high: bool) -> Level {
        if high { Level::High } else { Level::Low }
    }

    #[must_use]
    pub const fn is_high(self) -> bool {
        matches!(self, Level::High)
    }

    /// The other level.
    #[must_use]
    pub const fn inverted(self) -> Level {
        match self {
            Level::Low => Level::High,
            Level::High => Level::Low,
        }
    }
}

/// Device time handed to a chip: nanoseconds since the device was powered on. It only grows,
/// across resets and sleeps of the `SoC`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nanos(pub u64);

impl Nanos {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A chip that drives a line the `SoC` reads.
pub trait GpioInput: std::fmt::Debug {
    /// The level the chip drives on the line at `at`, or `None` when it leaves the line alone: the
    /// `SoC`'s pad then reads its own pull resistor.
    fn level(&mut self, at: Nanos) -> Option<Level>;

    /// The first instant after `at` at which [`GpioInput::level`] may answer something else by
    /// itself, without anything the `SoC` does: the end of a busy period, the next edge of a clock
    /// the chip generates. `None` (the default): not before the `SoC` acts on the chip again.
    ///
    /// The `SoC` asks after every call it makes to the chip, and calls [`GpioInput::level`] again at
    /// that instant when it needs to see the change as it happens (an interrupt on the line).
    fn next_change(&mut self, at: Nanos) -> Option<Nanos> {
        let _ = at;
        None
    }
}

/// A chip that watches a line the `SoC` drives.
pub trait GpioOutput: std::fmt::Debug {
    /// The `SoC` changed what it drives on the line, at `at`: `Some(level)` while its output is
    /// enabled, `None` when it releases the line (an input, a disabled output, an open-drain output
    /// at its high level). Called only when the value changes, and once when the chip is attached.
    fn driven(&mut self, level: Option<Level>, at: Nanos);
}

/// A chip on an SPI bus the `SoC` masters: the transfers it sees while its chip-select line is
/// active.
///
/// The transfers are whole: the bytes the master clocked out, and the bytes the chip clocked back,
/// one for one. A chip sees a transfer only while it is selected; traffic made by toggling the
/// lines one by one from software is seen as level changes by a [`GpioOutput`] on those lines,
/// not here.
pub trait SpiTarget: std::fmt::Debug {
    /// The chip-select line changed at `at`: `selected` is `true` when it became active (low).
    fn select(&mut self, selected: bool, at: Nanos);

    /// One transfer while the chip is selected, ending at `at`. `mosi` holds the bytes the master
    /// sent, first byte first, most significant bit first. `miso` is as long as `mosi` and holds the
    /// level the data-in line idles at (`0xFF` or `0x00`): the chip overwrites the bytes it drives.
    /// `dc` is the level of the chip's data/command line during the transfer, `None` when the chip
    /// has no such line.
    fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], miso: &mut [u8], at: Nanos);
}

/// A chip that puts a voltage on an analog input of the `SoC`.
pub trait AnalogSource: std::fmt::Debug {
    /// The voltage on the line at `at`, in millivolts.
    fn millivolts(&mut self, at: Nanos) -> u32;
}

impl<T: GpioInput + ?Sized> GpioInput for Rc<RefCell<T>> {
    fn level(&mut self, at: Nanos) -> Option<Level> {
        self.borrow_mut().level(at)
    }

    fn next_change(&mut self, at: Nanos) -> Option<Nanos> {
        self.borrow_mut().next_change(at)
    }
}

impl<T: GpioOutput + ?Sized> GpioOutput for Rc<RefCell<T>> {
    fn driven(&mut self, level: Option<Level>, at: Nanos) {
        self.borrow_mut().driven(level, at);
    }
}

impl<T: SpiTarget + ?Sized> SpiTarget for Rc<RefCell<T>> {
    fn select(&mut self, selected: bool, at: Nanos) {
        self.borrow_mut().select(selected, at);
    }

    fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], miso: &mut [u8], at: Nanos) {
        self.borrow_mut().transfer(dc, mosi, miso, at);
    }
}

impl<T: AnalogSource + ?Sized> AnalogSource for Rc<RefCell<T>> {
    fn millivolts(&mut self, at: Nanos) -> u32 {
        self.borrow_mut().millivolts(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_converts_from_and_to_a_bool_and_inverts() {
        assert_eq!(Level::from_bool(true), Level::High);
        assert_eq!(Level::from_bool(false), Level::Low);
        assert!(Level::High.is_high());
        assert!(!Level::Low.is_high());
        assert_eq!(Level::High.inverted(), Level::Low);
        assert_eq!(Level::Low.inverted(), Level::High);
        assert_eq!(Nanos(7).get(), 7);
        assert!(Nanos(1) < Nanos(2));
    }

    /// A chip with one line of each kind, which records what it is told.
    #[derive(Debug, Default)]
    struct Recorder {
        calls: Vec<String>,
    }

    impl GpioInput for Recorder {
        fn level(&mut self, at: Nanos) -> Option<Level> {
            self.calls.push(format!("level {}", at.get()));
            Some(Level::High)
        }
    }

    impl GpioOutput for Recorder {
        fn driven(&mut self, level: Option<Level>, at: Nanos) {
            self.calls.push(format!("driven {level:?} {}", at.get()));
        }
    }

    impl SpiTarget for Recorder {
        fn select(&mut self, selected: bool, at: Nanos) {
            self.calls.push(format!("select {selected} {}", at.get()));
        }

        fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], miso: &mut [u8], at: Nanos) {
            self.calls
                .push(format!("transfer {dc:?} {mosi:02x?} {}", at.get()));
            for (out, sent) in miso.iter_mut().zip(mosi) {
                *out = sent.wrapping_add(1);
            }
        }
    }

    impl AnalogSource for Recorder {
        fn millivolts(&mut self, at: Nanos) -> u32 {
            self.calls.push(format!("millivolts {}", at.get()));
            1650
        }
    }

    #[test]
    fn a_shared_chip_answers_every_attachment_it_is_shared_between() {
        let chip = Rc::new(RefCell::new(Recorder::default()));
        let mut as_input: Box<dyn GpioInput> = Box::new(Rc::clone(&chip));
        let mut as_output: Box<dyn GpioOutput> = Box::new(Rc::clone(&chip));
        let mut as_target: Box<dyn SpiTarget> = Box::new(Rc::clone(&chip));
        let mut as_source: Box<dyn AnalogSource> = Box::new(Rc::clone(&chip));
        assert_eq!(as_input.level(Nanos(1)), Some(Level::High));
        assert_eq!(as_input.next_change(Nanos(1)), None, "the default");
        as_output.driven(Some(Level::Low), Nanos(2));
        as_target.select(true, Nanos(3));
        let mut miso = [0xFF; 2];
        as_target.transfer(Some(Level::High), &[0x12, 0x34], &mut miso, Nanos(4));
        assert_eq!(miso, [0x13, 0x35]);
        assert_eq!(as_source.millivolts(Nanos(5)), 1650);
        assert_eq!(
            chip.borrow().calls,
            [
                "level 1",
                "driven Some(Low) 2",
                "select true 3",
                "transfer Some(High) [12, 34] 4",
                "millivolts 5",
            ]
        );
    }
}
