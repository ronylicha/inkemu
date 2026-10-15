//! The chips on the X3's I2C bus (research `sp3-x3-chips.md` §1 point 4): the BQ27220 fuel gauge
//! at `0x55`, the DS3231 real-time clock at `0x68` and the QMI8658 motion sensor at `0x6B`
//! (`0x6A` on the other level of its SA0 pin).
//!
//! Each answers the conditions of a bus the `SoC` masters through [`emu_core::I2cTarget`]: an
//! address after a START, the bytes written to it, the bytes it drives, a STOP. A chip keeps a
//! register pointer between transactions, as the three datasheets describe, so a read without a
//! pointer write continues where the last access left it.
//!
//! Time is the device's, handed with every call: the clock of the DS3231 runs from its configured
//! start by device time, the sample counter of the QMI8658 counts device time since its sensors
//! were enabled. Nothing reads a host clock: a run is the same whenever and wherever it happens.
//!
//! What a firmware does that a datasheet refuses, leaves undefined or the model does not carry
//! out becomes an [`I2cDiagnostic`] (spec §7: never hide an error), kept in a bounded store per
//! chip.

use std::fmt;

use emu_core::Nanos;

pub mod bq27220;
pub mod ds3231;
pub mod qmi8658;

/// The diagnostics a chip keeps until they are taken; past it they are counted, not kept.
pub const MAX_DIAGNOSTICS: usize = 256;

/// What a chip on the I2C bus reports.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum I2cDiagnosticKind {
    /// A command, subcommand or feature the model keeps without effect, or refuses: `what` names
    /// it, `value` is the code or address the firmware sent.
    NotModelled { what: &'static str, value: u32 },
    /// A write to a register the datasheet makes read only. The BQ27220 refuses the byte (NACK);
    /// the DS3231 and the QMI8658 acknowledge it and ignore it.
    ReadOnly { register: u8 },
    /// A register pointer past the chip's register map.
    NoSuchRegister { register: u8 },
    /// A value the datasheet leaves undefined (a BCD digit above 9, an hour of 25, a reserved
    /// code): the model does not take it.
    Undefined { register: u8, value: u8 },
}

/// A diagnostic, the chip that reported it and the instant of device time it happened at.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct I2cDiagnostic {
    pub at: Nanos,
    pub chip: &'static str,
    pub kind: I2cDiagnosticKind,
}

impl fmt::Display for I2cDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let chip = self.chip;
        write!(f, "at {} ns, ", self.at.get())?;
        match self.kind {
            I2cDiagnosticKind::NotModelled { what, value } => write!(
                f,
                "the {chip} received {what} {value:#06x}, which the model keeps without effect; a \
                 firmware that needs it gets the chip's power-on behaviour, and the chip's model \
                 must carry it out before that firmware can be trusted here"
            ),
            I2cDiagnosticKind::ReadOnly { register } => write!(
                f,
                "the {chip} refused a write to its read-only register {register:#04x}; the \
                 datasheet makes that register read only, so a firmware writes only the registers \
                 it lists as writable"
            ),
            I2cDiagnosticKind::NoSuchRegister { register } => write!(
                f,
                "the {chip} has no register {register:#04x} and ignored the access; the register \
                 pointer must name a register of the datasheet's map"
            ),
            I2cDiagnosticKind::Undefined { register, value } => write!(
                f,
                "the {chip} received {value:#04x} for register {register:#04x}, a value its \
                 datasheet leaves undefined, and did not take it; write a value the datasheet \
                 defines for that register"
            ),
        }
    }
}

/// The diagnostics of one chip: at most [`MAX_DIAGNOSTICS`] kept, the others counted.
#[derive(Clone, Debug)]
pub(crate) struct Diagnostics {
    chip: &'static str,
    kept: Vec<I2cDiagnostic>,
    /// The kinds [`Diagnostics::report_once`] has reported, so that it does not report them again.
    reported: Vec<I2cDiagnosticKind>,
    dropped: u64,
}

impl Diagnostics {
    pub(crate) const fn new(chip: &'static str) -> Diagnostics {
        Diagnostics {
            chip,
            kept: Vec::new(),
            reported: Vec::new(),
            dropped: 0,
        }
    }

    /// Keeps `kind`, or counts it when [`MAX_DIAGNOSTICS`] are waiting.
    pub(crate) fn report(&mut self, at: Nanos, kind: I2cDiagnosticKind) {
        if self.kept.len() < MAX_DIAGNOSTICS {
            self.kept.push(I2cDiagnostic {
                at,
                chip: self.chip,
                kind,
            });
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// Reports `kind` the first time only: a firmware that polls a feature the model lacks would
    /// otherwise fill the store with one message. The kinds remembered are bounded as the store
    /// is; past [`MAX_DIAGNOSTICS`] distinct kinds, a new one is counted as dropped.
    pub(crate) fn report_once(&mut self, at: Nanos, kind: I2cDiagnosticKind) {
        if self.reported.contains(&kind) {
            return;
        }
        if self.reported.len() < MAX_DIAGNOSTICS {
            self.reported.push(kind.clone());
            self.report(at, kind);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub(crate) fn take(&mut self) -> Vec<I2cDiagnostic> {
        std::mem::take(&mut self.kept)
    }

    pub(crate) const fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// Where a chip stands in the transaction on the bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Not addressed: the bytes on the bus are for another chip.
    Released,
    /// Addressed for a write, waiting for the register pointer.
    Pointer,
    /// Addressed for a write, the pointer set: each byte goes to a register.
    Writing,
    /// Addressed for a read: each byte comes from a register.
    Reading,
    /// Addressed for a write whose pointer named no register: the bytes that follow go nowhere.
    Ignoring,
}

/// Transactions as an I2C master makes them, for the tests of the three chips.
#[cfg(test)]
pub(crate) mod bus {
    use emu_core::{I2cTarget, Nanos};

    /// START, the address for a write, `bytes`, STOP: the acknowledgement of the address, then of
    /// each byte the master sent (none after a refused address).
    pub(crate) fn write<T: I2cTarget>(
        chip: &mut T,
        address: u8,
        bytes: &[u8],
        at: Nanos,
    ) -> Vec<bool> {
        let mut acks = vec![chip.address(address, false, at)];
        if acks[0] {
            for byte in bytes {
                acks.push(chip.write(*byte, at));
            }
        }
        chip.stop(at);
        acks
    }

    /// The read Arduino's `Wire` makes after `endTransmission(false)` (research
    /// `sp3-x3-firmware.md` §2.1): START, the address for a write, `register`, repeated START, the
    /// address for a read, `count` bytes (a NACK after the last one), STOP. `None` when the chip refuses
    /// its address or the register.
    pub(crate) fn read_register<T: I2cTarget>(
        chip: &mut T,
        address: u8,
        register: u8,
        count: usize,
        at: Nanos,
    ) -> Option<Vec<u8>> {
        if !chip.address(address, false, at) || !chip.write(register, at) {
            chip.stop(at);
            return None;
        }
        read_after_start(chip, address, count, at)
    }

    /// A read with no pointer write before it: it starts at the chip's register pointer.
    pub(crate) fn quick_read<T: I2cTarget>(
        chip: &mut T,
        address: u8,
        count: usize,
        at: Nanos,
    ) -> Option<Vec<u8>> {
        read_after_start(chip, address, count, at)
    }

    fn read_after_start<T: I2cTarget>(
        chip: &mut T,
        address: u8,
        count: usize,
        at: Nanos,
    ) -> Option<Vec<u8>> {
        if !chip.address(address, true, at) {
            chip.stop(at);
            return None;
        }
        let bytes = (0..count).map(|i| chip.read(i + 1 < count, at)).collect();
        chip.stop(at);
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_keeps_a_bounded_number_and_counts_the_rest() {
        let mut store = Diagnostics::new("TEST");
        for register in 0..=u8::MAX {
            store.report(Nanos(1), I2cDiagnosticKind::NoSuchRegister { register });
        }
        store.report(Nanos(2), I2cDiagnosticKind::ReadOnly { register: 1 });
        store.report(Nanos(2), I2cDiagnosticKind::ReadOnly { register: 2 });
        assert_eq!(store.take().len(), MAX_DIAGNOSTICS);
        assert_eq!(store.dropped(), 2, "the 256 first fill the store");
        assert!(store.take().is_empty(), "taken once");
        store.report(Nanos(3), I2cDiagnosticKind::ReadOnly { register: 3 });
        assert_eq!(store.take().len(), 1, "room again once taken");
    }

    #[test]
    fn a_kind_reported_once_is_not_reported_again() {
        let mut store = Diagnostics::new("TEST");
        let kind = I2cDiagnosticKind::NotModelled {
            what: "a feature",
            value: 7,
        };
        store.report_once(Nanos(1), kind.clone());
        store.report_once(Nanos(2), kind);
        store.report_once(
            Nanos(3),
            I2cDiagnosticKind::NotModelled {
                what: "a feature",
                value: 8,
            },
        );
        let kept = store.take();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].at, Nanos(1));
        assert_eq!(kept[0].chip, "TEST");
    }

    #[test]
    fn the_kinds_remembered_are_bounded_too() {
        let mut store = Diagnostics::new("TEST");
        for value in 0..300 {
            store.report_once(
                Nanos(0),
                I2cDiagnosticKind::NotModelled {
                    what: "a feature",
                    value,
                },
            );
        }
        assert_eq!(store.take().len(), MAX_DIAGNOSTICS);
        assert_eq!(store.dropped(), 300 - 256);
    }

    #[test]
    fn every_message_names_the_chip_the_cause_and_the_remedy() {
        let kinds = [
            I2cDiagnosticKind::NotModelled {
                what: "a Control() subcommand",
                value: 0x41,
            },
            I2cDiagnosticKind::ReadOnly { register: 0x08 },
            I2cDiagnosticKind::NoSuchRegister { register: 0x70 },
            I2cDiagnosticKind::Undefined {
                register: 0x00,
                value: 0x6A,
            },
        ];
        for kind in kinds {
            let text = I2cDiagnostic {
                at: Nanos(1_500),
                chip: "BQ27220",
                kind,
            }
            .to_string();
            assert!(text.starts_with("at 1500 ns, the BQ27220 "), "{text}");
            assert!(text.contains(';'), "a cause, then a remedy: {text}");
        }
    }

    /// A deterministic xorshift generator for the test below.
    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// Whatever a master puts on the bus, in whatever order, no chip panics: STARTs to its own
    /// address (mostly) or another, reads and writes, bytes acknowledged or not, STOPs or none,
    /// repeated STARTs, and time that moves on by nanoseconds or by years.
    #[test]
    fn no_sequence_of_conditions_makes_a_chip_panic() {
        use emu_core::I2cTarget;

        let mut chips: Vec<(u8, Box<dyn I2cTarget>)> = vec![
            (
                0x55,
                Box::new(bq27220::Bq27220::new(bq27220::Bq27220Config::default()).unwrap()),
            ),
            (
                0x68,
                Box::new(ds3231::Ds3231::new(ds3231::Ds3231Config::default()).unwrap()),
            ),
            (
                0x6B,
                Box::new(qmi8658::Qmi8658::new(qmi8658::Qmi8658Config::default()).unwrap()),
            ),
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for (own, chip) in &mut chips {
            let mut now: u64 = 0;
            for _ in 0..200_000 {
                let draw = xorshift(&mut state);
                let [operation, choice, byte, ..] = draw.to_le_bytes();
                let at = Nanos(now);
                match operation % 16 {
                    0..=2 => {
                        let address = if choice % 4 == 0 { byte & 0x7F } else { *own };
                        let _ = chip.address(address, choice % 2 == 0, at);
                    }
                    3..=8 => {
                        let _ = chip.write(byte, at);
                    }
                    9..=12 => {
                        let _ = chip.read(choice % 8 != 0, at);
                    }
                    13 => chip.stop(at),
                    14 => now = now.saturating_add(u64::from(byte) << (choice % 48)),
                    _ => now = now.saturating_add(u64::from(choice) * 1_000),
                }
            }
        }
    }
}
