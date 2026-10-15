//! The signals between a system-on-chip and the chips around it (spec §3.2, ADR 0012).
//!
//! A chip outside the `SoC` (a display controller, a memory card, a button ladder) never sees the
//! `SoC`: it sees lines. Four traits say what a chip does on them, a fifth when it lives in time,
//! and nothing here knows which `SoC` drives the other end:
//!
//! - [`GpioInput`]: the chip drives a line the `SoC` reads (a button, a BUSY line);
//! - [`GpioOutput`]: the chip watches a line the `SoC` drives (a reset, a power latch);
//! - [`SpiTarget`]: the chip sits on an SPI bus the `SoC` masters, selected by its chip-select line,
//!   with an optional data/command line;
//! - [`AnalogSource`]: the chip puts a voltage on an analog input;
//! - [`I2cTarget`]: the chip sits on an I2C bus the `SoC` masters, answering its address;
//! - [`Clocked`]: the chip does work of its own over time (a display refresh) and is advanced to
//!   the instants it names, whatever the `SoC` is doing meanwhile.
//!
//! Two more say what the board around the chips does (ADR 0013): [`BoardInputs`], the elements a
//! user acts on, whose inputs the `SoC` dates, journals and applies; [`Supply`], whether the `SoC`
//! has power.
//!
//! The `SoC` calls them; a chip never calls the `SoC`. Time is device time in nanoseconds since
//! power-on ([`Nanos`]), not cycles of a clock of the `SoC`: a chip counts its own durations (a
//! refresh, a reset pulse) in real time. A chip whose lines depend only on its state and on that
//! time gives a deterministic run.
//!
//! A chip that has several lines (the BUSY output and the SPI interface of a display controller)
//! is attached once per line; `Rc<RefCell<T>>` implements the five traits for `T`, so the same chip
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
    /// The `SoC` asks only while an interrupt is configured on the line (on the ESP32-C3,
    /// `GPIO_PINn.INT_TYPE` not 0), each time it works its pads out again: at each access to the
    /// pad registers, at each transfer of its SPI controller, when the caller says an input
    /// changed, and at the instant it was woken for. It then wakes at the instant given and
    /// calls [`GpioInput::level`] there, so that the edge raises the interrupt as it happens.
    /// Without an interrupt on the line, a change is seen at the next read of the line, not
    /// before: a chip that must act at an instant whatever the `SoC` reads implements
    /// [`Clocked`].
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
/// lines one by one from software (the half-duplex read of a panel on its MOSI line, for one) is
/// seen as level changes by a [`GpioOutput`] on the clock and data lines, and answered by a
/// [`GpioInput`] on the line the chip drives, not here.
///
/// - A selection may come without a transfer: a driver routing its chip-select pin can pulse it
///   (ESP-IDF's `spicommon_cs_initialize` drives it low for a few instructions before it connects
///   the controller's chip select). A selection alone is no command.
/// - With a chip select the controller drives itself, [`SpiTarget::select`] with `true`, the
///   transfer and [`SpiTarget::select`] with `false` come at the same instant, the end of the
///   transaction: the controller asserts and releases the line around its own clock pulses. With
///   a chip select driven from software, the selection spans as many transfers as the software
///   makes before releasing it.
pub trait SpiTarget: std::fmt::Debug {
    /// The chip-select line changed at `at`: `selected` is `true` when it became active (low).
    fn select(&mut self, selected: bool, at: Nanos);

    /// One transfer while the chip is selected, ending at `at`. `mosi` holds the bytes the master
    /// sent, first byte first, most significant bit first. `dc` is the level of the chip's
    /// data/command line during the transfer, `None` when the chip has no such line.
    ///
    /// `miso` is as long as `mosi` and holds what the master would read if the chip drove nothing:
    /// `0xFF` when the master's data-in line reads high at that instant (a pull-up on the pad), `0x00`
    /// when it reads low (a pad with no pull, or a pull-down). The chip overwrites the bytes it
    /// drives.
    fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], miso: &mut [u8], at: Nanos);
}

/// A chip that puts a voltage on an analog input of the `SoC`.
pub trait AnalogSource: std::fmt::Debug {
    /// The voltage on the line at `at`, in millivolts.
    fn millivolts(&mut self, at: Nanos) -> u32;
}

/// A chip on an I2C bus the `SoC` masters (ADR 0015): what it sees of each transaction, condition by
/// condition, each at the instant it ends on the bus.
///
/// Every chip on the bus sees every address and every STOP; only a chip that acknowledged its
/// address sees the bytes that follow, until the next START or STOP. A chip never stretches the
/// clock: the `SoC` paces every bit.
///
/// - [`I2cTarget::address`]: a START (or a repeated START) and the address byte after it;
/// - [`I2cTarget::write`]: a byte the master writes to the chip, which acknowledged its address for
///   a write;
/// - [`I2cTarget::read`]: a byte the chip drives for the master, which addressed it for a read;
/// - [`I2cTarget::stop`]: a STOP on the bus.
pub trait I2cTarget: std::fmt::Debug {
    /// A START, then the address byte, ending at `at`: `address` is the seven-bit address, `read`
    /// the direction bit (`true` for a read). Whether the chip acknowledges: `true` for an ACK
    /// (the chip pulls SDA low on the ninth clock).
    fn address(&mut self, address: u8, read: bool, at: Nanos) -> bool;

    /// A byte the master wrote, ending at `at`, while the chip is addressed for a write. Whether
    /// the chip acknowledges it.
    fn write(&mut self, byte: u8, at: Nanos) -> bool;

    /// The byte the chip drives for the master, ending at `at`, while it is addressed for a read.
    /// `acknowledged` is what the master answers on the ninth clock: `true` (ACK) when it wants
    /// another byte, `false` (NACK) for the last one.
    fn read(&mut self, acknowledged: bool, at: Nanos) -> u8;

    /// A STOP, ending at `at`: whatever was addressed is released.
    fn stop(&mut self, at: Nanos);
}

/// A chip that does work of its own over time (a display controller running a refresh, a card
/// finishing a write): the `SoC` advances it to each instant it names, in device time, whether
/// the `SoC` is running, waiting for an interrupt or waiting for a timer.
///
/// The `SoC` asks [`Clocked::next_deadline`] when the chip is attached and again after every call
/// it makes to a chip (an advance, a transfer, a level asked or heard), and calls
/// [`Clocked::advance`] once the deadline has come, before any other call it makes to a chip at
/// that instant: a chip never answers for an instant past its deadline before it has reached it.
/// This holds while the `SoC` runs, while its CPU waits for an interrupt and during a deep sleep
/// of the `SoC`, the chips of the board keeping their power. Calls come in order of time, and a
/// chip is advanced at most once at a given instant: a deadline that is not after the instant the
/// chip was last advanced at is taken up again at the next instant of the `SoC`'s time base, so
/// a chip cannot hold the `SoC` at one instant, and one whose work is always due is advanced at
/// each instant of that time base.
pub trait Clocked: std::fmt::Debug {
    /// The instant the chip must be advanced at, at the latest: `at` or before when it has work
    /// due now, `None` when it has nothing to do until the `SoC` acts on it.
    fn next_deadline(&mut self, at: Nanos) -> Option<Nanos>;

    /// Brings the chip to `at`, doing the work due by then. `at` is device time, at or after the
    /// deadline: while the `SoC`'s CPU waits (an interrupt, a timer, a deep sleep), the first
    /// instant of the `SoC`'s time base at or after the deadline, exactly; while it runs, at the
    /// latest the end of the instruction under way when the deadline came, earlier when an access
    /// of that instruction reaches a chip.
    fn advance(&mut self, at: Nanos);
}

/// An input of the user to the board (a key pressed, a cable plugged in), as the `SoC` receives,
/// dates and journals it: a target the board numbers and a value the board interprets. The `SoC`
/// learns nothing else of it (ADR 0013).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoardInput {
    /// What the input acts on, numbered by the board (a key, the power cable).
    pub target: u16,
    /// What it does there (for a key: 1 pressed, 0 released).
    pub value: u32,
}

/// Why a board refuses an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BoardInputError {
    #[error("the board has no input numbered {target}")]
    UnknownTarget { target: u16 },
    #[error("input {target} of the board takes no value {value}")]
    Value { target: u16, value: u32 },
}

/// The inputs of a board: the elements a user acts on (keys, a cable), which change what chips
/// attached to the `SoC` drive.
///
/// The `SoC` asks [`BoardInputs::check`] when it accepts an input, and calls
/// [`BoardInputs::apply`] at the input's date, before it works its pads out again: a front end that
/// presses a key now and a replay that hands every input over at power-on are the same run.
pub trait BoardInputs: std::fmt::Debug {
    /// Whether the board takes `input`.
    ///
    /// # Errors
    /// [`BoardInputError`] when it names no element of the board or a value the element cannot
    /// take.
    fn check(&self, input: BoardInput) -> Result<(), BoardInputError>;

    /// Applies `input`, which [`BoardInputs::check`] accepted, at device time `at`.
    fn apply(&mut self, input: BoardInput, at: Nanos);
}

/// The power supply of a board: whether the `SoC` has power. The `SoC` asks after each change of
/// what it drives on a pin, after each board input it applies and at power-on; once the answer is
/// `false` the `SoC` stops for good at that instant (ADR 0013).
pub trait Supply: std::fmt::Debug {
    /// Whether the `SoC` has power at `at`.
    fn powered(&mut self, at: Nanos) -> bool;
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

impl<T: I2cTarget + ?Sized> I2cTarget for Rc<RefCell<T>> {
    fn address(&mut self, address: u8, read: bool, at: Nanos) -> bool {
        self.borrow_mut().address(address, read, at)
    }

    fn write(&mut self, byte: u8, at: Nanos) -> bool {
        self.borrow_mut().write(byte, at)
    }

    fn read(&mut self, acknowledged: bool, at: Nanos) -> u8 {
        self.borrow_mut().read(acknowledged, at)
    }

    fn stop(&mut self, at: Nanos) {
        self.borrow_mut().stop(at);
    }
}

impl<T: Clocked + ?Sized> Clocked for Rc<RefCell<T>> {
    fn next_deadline(&mut self, at: Nanos) -> Option<Nanos> {
        self.borrow_mut().next_deadline(at)
    }

    fn advance(&mut self, at: Nanos) {
        self.borrow_mut().advance(at);
    }
}

impl<T: BoardInputs + ?Sized> BoardInputs for Rc<RefCell<T>> {
    fn check(&self, input: BoardInput) -> Result<(), BoardInputError> {
        self.borrow().check(input)
    }

    fn apply(&mut self, input: BoardInput, at: Nanos) {
        self.borrow_mut().apply(input, at);
    }
}

impl<T: Supply + ?Sized> Supply for Rc<RefCell<T>> {
    fn powered(&mut self, at: Nanos) -> bool {
        self.borrow_mut().powered(at)
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

    impl I2cTarget for Recorder {
        fn address(&mut self, address: u8, read: bool, at: Nanos) -> bool {
            self.calls
                .push(format!("address {address:#04x} {read} {}", at.get()));
            address == 0x55
        }

        fn write(&mut self, byte: u8, at: Nanos) -> bool {
            self.calls.push(format!("write {byte:#04x} {}", at.get()));
            true
        }

        fn read(&mut self, acknowledged: bool, at: Nanos) -> u8 {
            self.calls.push(format!("read {acknowledged} {}", at.get()));
            0x2C
        }

        fn stop(&mut self, at: Nanos) {
            self.calls.push(format!("stop {}", at.get()));
        }
    }

    impl Clocked for Recorder {
        fn next_deadline(&mut self, at: Nanos) -> Option<Nanos> {
            self.calls.push(format!("next_deadline {}", at.get()));
            Some(Nanos(at.get() + 10))
        }

        fn advance(&mut self, at: Nanos) {
            self.calls.push(format!("advance {}", at.get()));
        }
    }

    #[test]
    fn a_shared_chip_answers_every_attachment_it_is_shared_between() {
        let chip = Rc::new(RefCell::new(Recorder::default()));
        let mut as_input: Box<dyn GpioInput> = Box::new(Rc::clone(&chip));
        let mut as_output: Box<dyn GpioOutput> = Box::new(Rc::clone(&chip));
        let mut as_target: Box<dyn SpiTarget> = Box::new(Rc::clone(&chip));
        let mut as_source: Box<dyn AnalogSource> = Box::new(Rc::clone(&chip));
        let mut as_clocked: Box<dyn Clocked> = Box::new(Rc::clone(&chip));
        let mut as_i2c: Box<dyn I2cTarget> = Box::new(Rc::clone(&chip));
        assert_eq!(as_input.level(Nanos(1)), Some(Level::High));
        assert_eq!(as_input.next_change(Nanos(1)), None, "the default");
        as_output.driven(Some(Level::Low), Nanos(2));
        as_target.select(true, Nanos(3));
        let mut miso = [0xFF; 2];
        as_target.transfer(Some(Level::High), &[0x12, 0x34], &mut miso, Nanos(4));
        assert_eq!(miso, [0x13, 0x35]);
        assert_eq!(as_source.millivolts(Nanos(5)), 1650);
        assert_eq!(as_clocked.next_deadline(Nanos(6)), Some(Nanos(16)));
        as_clocked.advance(Nanos(16));
        assert!(as_i2c.address(0x55, false, Nanos(17)));
        assert!(!as_i2c.address(0x68, true, Nanos(18)));
        assert!(as_i2c.write(0x2C, Nanos(19)));
        assert_eq!(as_i2c.read(false, Nanos(20)), 0x2C);
        as_i2c.stop(Nanos(21));
        assert_eq!(
            chip.borrow().calls,
            [
                "level 1",
                "driven Some(Low) 2",
                "select true 3",
                "transfer Some(High) [12, 34] 4",
                "millivolts 5",
                "next_deadline 6",
                "advance 16",
                "address 0x55 false 17",
                "address 0x68 true 18",
                "write 0x2c 19",
                "read false 20",
                "stop 21",
            ]
        );
    }

    /// A board with one key (target 0, values 0 and 1) and a supply that follows it.
    #[derive(Debug, Default)]
    struct OneKey {
        pressed: bool,
        applied: Vec<(BoardInput, Nanos)>,
    }

    impl BoardInputs for OneKey {
        fn check(&self, input: BoardInput) -> Result<(), BoardInputError> {
            match input {
                BoardInput { target: 0, value } if value <= 1 => Ok(()),
                BoardInput { target: 0, value } => Err(BoardInputError::Value { target: 0, value }),
                BoardInput { target, .. } => Err(BoardInputError::UnknownTarget { target }),
            }
        }

        fn apply(&mut self, input: BoardInput, at: Nanos) {
            self.pressed = input.value == 1;
            self.applied.push((input, at));
        }
    }

    impl Supply for OneKey {
        fn powered(&mut self, _at: Nanos) -> bool {
            self.pressed
        }
    }

    #[test]
    fn a_shared_board_takes_its_inputs_and_answers_for_its_supply() {
        let board = Rc::new(RefCell::new(OneKey::default()));
        let mut inputs: Box<dyn BoardInputs> = Box::new(Rc::clone(&board));
        let mut supply: Box<dyn Supply> = Box::new(Rc::clone(&board));
        let press = BoardInput {
            target: 0,
            value: 1,
        };
        assert_eq!(inputs.check(press), Ok(()));
        assert_eq!(
            inputs.check(BoardInput {
                target: 3,
                value: 1
            }),
            Err(BoardInputError::UnknownTarget { target: 3 })
        );
        assert_eq!(
            inputs
                .check(BoardInput {
                    target: 0,
                    value: 2
                })
                .unwrap_err()
                .to_string(),
            "input 0 of the board takes no value 2"
        );
        assert!(!supply.powered(Nanos(0)));
        inputs.apply(press, Nanos(9));
        assert!(supply.powered(Nanos(9)));
        assert_eq!(board.borrow().applied, [(press, Nanos(9))]);
    }
}
