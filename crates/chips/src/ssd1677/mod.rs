//! The SSD1677, an e-paper controller with 960 sources and 680 gates, as the X4 drives it: an
//! interpreter of its commands at the level of command and data bytes (`SP2a` design, decisions
//! A1–A13 and A17; datasheet Rev 1.0, research `sp2a-external-chips.md`).
//!
//! The SPI framing (SCL, SDA, D/C#, CS#) belongs to whoever wires the chip: it hands over whole
//! bytes, telling commands from data. Time is the device's, an instant in cycles of the machine's
//! time base, passed with every call; the chip never reads a clock. It asks to be called back:
//! [`Ssd1677::next_deadline`] is the next instant BUSY changes or an image is published, and
//! [`Ssd1677::advance`] brings the chip to an instant.

mod config;
mod diagnostic;
mod panel;
mod ram;
pub mod reference;
#[cfg(test)]
mod tests;
mod waveform;

use emu_core::{Cycles, Frequency};

pub use config::{
    ConfigError, DisplayMode, GateScan, LUT_BYTES, Optics, OtpWaveform, PanelGeometry, Provenance,
    RAM_GATES, RAM_SOURCES, Ssd1677Config, Ssd1677Timing, Temperature,
};
pub use diagnostic::{Diagnostic, DiagnosticKind, Stage};
pub use panel::{PanelImage, RefreshKind};
pub use ram::Plane;

use config::find_waveform;
use panel::{Display, Panel, PlaneOption, Scan};
use ram::{Counter, Entry, Ram, Window};
use waveform::{
    Drive, Waveform, gate_code_is_listed, source_millivolts, vcom_millivolts, vsl_millivolts,
};

/// The diagnostics kept until [`Ssd1677::take_diagnostics`]; past it they are counted, not kept.
pub const MAX_DIAGNOSTICS: usize = 1024;

const MILLISECOND: u64 = 1_000_000;
const SECOND: u64 = 1_000 * MILLISECOND;

/// The bits of Display Update Control 2 (`0x22`), one stage each (research §3.5).
const CLOCK_ON: u8 = 0x80;
const ANALOG_ON: u8 = 0x40;
const LOAD_TEMPERATURE: u8 = 0x20;
const LOAD_LUT: u8 = 0x10;
const MODE_TWO: u8 = 0x08;
const DISPLAY: u8 = 0x04;
const ANALOG_OFF: u8 = 0x02;
const CLOCK_OFF: u8 = 0x01;

/// `0x18`: the internal sensor; any other value selects the external one (48h at reset).
const INTERNAL_SENSOR: u8 = 0x80;
const EXTERNAL_SENSOR: u8 = 0x48;

/// How many parameter bytes a command takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Params {
    Exactly(u16),
    Between(u16, u16),
    /// RAM data, as many bytes as written.
    Stream,
}

/// What the model does with a command byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Write(Params),
    /// A command whose data bytes are read by the MCU.
    Read,
    Unsupported(&'static str),
    Reserved,
}

/// The commands of the datasheet's Table 7-1.
fn kind(command: u8) -> Option<Kind> {
    use Kind::{Read, Reserved, Unsupported, Write};
    use Params::{Between, Exactly, Stream};
    Some(match command {
        0x12 | 0x20 | 0x28 | 0x7F => Write(Exactly(0)),
        0x03 | 0x10 | 0x11 | 0x14 | 0x15 | 0x18 | 0x22 | 0x29 | 0x2C | 0x3C | 0x41 | 0x46
        | 0x47 => Write(Exactly(1)),
        0x2B | 0x4E | 0x4F => Write(Exactly(2)),
        // `0x1A`: the X4's firmware sends A[11:4] alone (`FreeInk` `Ssd1677Driver.cpp:469-470`),
        // and the panel takes it: the byte of A[3:0] is optional, its nibble kept. `0x21`: some
        // boards send a second byte, which the SSD1677's datasheet does not list; stored only.
        0x1A | 0x21 => Write(Between(1, 2)),
        0x01 | 0x04 => Write(Exactly(3)),
        0x44 | 0x45 => Write(Exactly(4)),
        0x0C => Write(Exactly(5)),
        0x37 | 0x38 => Write(Exactly(10)),
        0x32 => Write(Exactly(105)),
        0x24 | 0x26 => Write(Stream),
        0x1B | 0x27 | 0x2D | 0x2E | 0x2F => Read,
        0x08 => Unsupported("Initial Code Setting OTP Program"),
        0x09 => Unsupported("Write Register for Initial Code Setting"),
        0x0A => Unsupported("Read Register for Initial Code Setting"),
        0x1C => Unsupported("Write Command to External temperature sensor"),
        0x25 => Unsupported("Write RAM (Dithering)"),
        0x2A => Unsupported("Program VCOM OTP"),
        0x30 => Unsupported("Program WS OTP"),
        0x31 => Unsupported("Load WS OTP"),
        0x34 => Unsupported("CRC calculation"),
        0x35 => Unsupported("CRC Status Read"),
        0x36 => Unsupported("Program OTP selection"),
        0x39 => Unsupported("OTP program mode"),
        0x4D => Unsupported("Dithering engine Start/Stop"),
        0x3A | 0x3B => Reserved,
        _ => return None,
    })
}

/// A ten-bit value written in two bytes: the low byte (`index` 0), then bits 9:8 (`index` 1).
fn ten_bits(value: u16, index: u16, byte: u8) -> u16 {
    if index == 0 {
        value & 0x300 | u16::from(byte)
    } else {
        value & 0xFF | u16::from(byte & 0b11) << 8
    }
}

/// The registers, at their reset values (datasheet Table 7-1). SW reset, hardware reset and
/// power-on all restore them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Registers {
    /// `MUX[9:0]`: the scanned gates are `mux + 1`.
    mux: u16,
    gate_scan: GateScan,
    /// The LUT register with its voltages (`0x32`, `0x03`, `0x04`, `0x2C`), or the setting the
    /// last LUT load copied.
    lut: [u8; LUT_BYTES],
    entry: Entry,
    window: Window,
    counter: Counter,
    /// `0x21`: the RAM options, and a second byte some boards send, stored only.
    update_control_1: [u8; 2],
    update_control_2: u8,
    sensor: u8,
    /// The twelve-bit temperature register.
    temperature: u16,
    /// `0x29`. ASSUMPTION: 49h (normal mode, 10 s) at reset; the datasheet gives it as an example.
    vcom_sense: u8,
    display_option: [u8; 10],
    user_id: [u8; 10],
    border: u8,
    read_ram_red: bool,
}

impl Registers {
    fn reset(user_id: [u8; 10]) -> Registers {
        let mut lut = [0u8; LUT_BYTES];
        // VSH1 41h, VSH2 A8h, VSL 32h, VGH and VCOM 00h.
        lut[106] = 0x41;
        lut[107] = 0xA8;
        lut[108] = 0x32;
        Registers {
            mux: 0x2A7,
            gate_scan: GateScan::default(),
            lut,
            entry: Entry::RESET,
            window: Window::RESET,
            counter: Counter::default(),
            update_control_1: [0; 2],
            update_control_2: 0xFF,
            sensor: EXTERNAL_SENSOR,
            temperature: Temperature::MAX.register(),
            vcom_sense: 0x49,
            display_option: [0; 10],
            user_id,
            border: 0xC0,
            read_ram_red: false,
        }
    }
}

/// The supply and reset state of the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Power {
    Running,
    /// `0x10` entered deep sleep: BUSY high, every command dropped, until a hardware reset.
    Asleep,
    /// RES# is low.
    InReset,
}

/// The command the data bytes belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Current {
    /// No command since the last reset: a data byte is reported once.
    Idle,
    /// A dropped or unknown command, or a data byte already reported: data go nowhere.
    Ignoring,
    Active(Active),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Active {
    command: u8,
    kind: Kind,
    /// Data bytes written, saturating.
    received: u16,
    /// Data bytes read.
    read: u16,
    /// One report of each kind per command.
    extra_reported: bool,
    range_reported: bool,
    nothing_reported: bool,
}

/// An SSD1677 and the glass it drives.
#[derive(Debug)]
pub struct Ssd1677 {
    config: Ssd1677Config,
    time_base: Frequency,
    /// The latest instant seen.
    now: Cycles,
    regs: Registers,
    ram: Ram,
    panel: Panel,
    clock: bool,
    analog: bool,
    power: Power,
    /// The instant BUSY falls, when an operation runs.
    busy_until: Option<Cycles>,
    display: Option<Display>,
    current: Current,
    status: Status,
    /// The current command already reported a reserved value: one report per command (A5).
    reserved_reported: bool,
    diagnostics: Vec<Diagnostic>,
    dropped: u64,
}

/// The detection flags of the status byte (`0x2F`), cleared by a reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Status {
    /// Bit 5: the HV ready detection found the supplies not ready.
    hv_not_ready: bool,
    /// Bit 4: the VCI detection found VCI below the level.
    vci_low: bool,
}

impl Ssd1677 {
    /// The chip at power-on, under `config`, counting time in cycles of `time_base`.
    ///
    /// # Errors
    /// The parameters [`ConfigError`] lists.
    pub fn new(config: Ssd1677Config, time_base: Frequency) -> Result<Ssd1677, ConfigError> {
        config.validate()?;
        let panel = Panel::new(config.panel.clone(), config.optics);
        Ok(Ssd1677 {
            regs: Registers::reset(config.user_id),
            // ASSUMPTION: the RAM reads 0 at power-on; the datasheet leaves it undefined.
            ram: Ram::filled(0x00),
            panel,
            time_base,
            now: Cycles::ZERO,
            clock: false,
            analog: false,
            power: Power::Running,
            busy_until: None,
            display: None,
            current: Current::Idle,
            status: Status::default(),
            reserved_reported: false,
            diagnostics: Vec::new(),
            dropped: 0,
            config,
        })
    }

    /// The parameters the chip was built with.
    #[must_use]
    pub fn config(&self) -> &Ssd1677Config {
        &self.config
    }

    /// Changes what the internal sensor reads, from the next temperature load.
    pub fn set_ambient(&mut self, ambient: Temperature) {
        self.config.ambient = ambient;
    }

    /// A RAM plane: 680 rows of 120 bytes, the most significant bit the leftmost pixel.
    #[must_use]
    pub fn ram(&self, plane: Plane) -> &[u8] {
        self.ram.plane(plane)
    }

    /// The image the glass shows since the last refresh (generation 0 before any).
    #[must_use]
    pub fn panel_image(&self) -> &PanelImage {
        self.panel.image()
    }

    /// Whether the chip is busy at `now`: an operation runs, or it sleeps. Not while RES# is low.
    #[must_use]
    pub fn is_busy(&self, now: Cycles) -> bool {
        match self.power {
            Power::Asleep => true,
            Power::InReset => false,
            Power::Running => self.busy_until.is_some_and(|until| now < until),
        }
    }

    /// The level of the BUSY pin at `now`: high while busy, unless the configuration inverts it.
    #[must_use]
    pub fn busy_pin(&self, now: Cycles) -> bool {
        self.is_busy(now) == self.config.busy_active_high
    }

    /// The next instant something changes without a call: BUSY falls, or a display ends and its
    /// image is published. `None` when nothing is pending (deep sleep included).
    #[must_use]
    pub fn next_deadline(&self) -> Option<Cycles> {
        let display = self.display.as_ref().map(|d| d.ends);
        match (display, self.busy_until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Brings the chip to `now`: publishes a display that ended, lowers BUSY. An instant before
    /// the latest one seen counts as the latest: the chip's time does not go back.
    pub fn advance(&mut self, now: Cycles) {
        self.now = self.now.max(now);
        if let Some(display) = self.display.take_if(|d| d.ends <= self.now) {
            self.panel.finish(&display, display.ends, None);
        }
        if self.busy_until.is_some_and(|until| until <= self.now) {
            self.busy_until = None;
        }
    }

    /// The diagnostics since the last call, oldest first.
    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// How many diagnostics were dropped because [`MAX_DIAGNOSTICS`] were waiting.
    #[must_use]
    pub fn dropped_diagnostics(&self) -> u64 {
        self.dropped
    }

    /// Drives RES#: `asserted` is the line low. Asserting it stops whatever runs (a display keeps
    /// what it drove so far) and restores the registers; releasing it wakes a sleeping chip, which
    /// stays busy for the hardware-reset time. ASSUMPTION: the RAM is kept and BUSY is low while
    /// RES# is low.
    pub fn set_reset(&mut self, now: Cycles, asserted: bool) {
        self.advance(now);
        match (asserted, self.power) {
            (true, Power::InReset) | (false, Power::Running | Power::Asleep) => {}
            (true, _) => {
                self.close_command();
                // A display cut once its first frame may have run publishes what the glass
                // reached; one whose stages before it (supplies, temperature, LUT) had not ended
                // is abandoned: the glass did not change.
                if let Some(display) = self.display.take_if(|d| d.started <= self.now) {
                    self.report(DiagnosticKind::ResetDuringDisplay);
                    let into = self.ns_between(display.started, self.now);
                    self.panel.finish(&display, self.now, Some(into));
                }
                self.display = None;
                self.restore_registers();
                self.busy_until = None;
                self.power = Power::InReset;
                self.current = Current::Ignoring;
            }
            (false, Power::InReset) => {
                self.power = Power::Running;
                self.current = Current::Idle;
                self.busy_for(self.config.timing.hardware_reset);
            }
        }
    }

    /// A byte with D/C# low.
    pub fn write_command(&mut self, now: Cycles, command: u8) {
        self.advance(now);
        self.close_command();
        let refused = match self.power {
            Power::InReset => Some(DiagnosticKind::WhileInReset { command }),
            Power::Asleep => Some(DiagnosticKind::WhileAsleep { command }),
            Power::Running if self.is_busy(self.now) && command != 0x2F => {
                Some(DiagnosticKind::WhileBusy { command })
            }
            Power::Running => None,
        };
        if let Some(refusal) = refused {
            self.report(refusal);
            self.current = Current::Ignoring;
            return;
        }
        let Some(kind) = kind(command) else {
            self.report(DiagnosticKind::UnknownCommand { command });
            self.current = Current::Ignoring;
            return;
        };
        match kind {
            Kind::Reserved => {
                self.report(DiagnosticKind::ReservedCommand { command });
                self.current = Current::Ignoring;
                return;
            }
            Kind::Unsupported(name) => {
                self.report(DiagnosticKind::UnsupportedCommand { command, name });
                self.current = Current::Ignoring;
                return;
            }
            Kind::Write(_) | Kind::Read => {}
        }
        self.current = Current::Active(Active {
            command,
            kind,
            received: 0,
            read: 0,
            extra_reported: false,
            range_reported: false,
            nothing_reported: false,
        });
        self.start_command(command);
    }

    /// A byte with D/C# high, written by the MCU.
    pub fn write_data(&mut self, now: Cycles, data: u8) {
        self.advance(now);
        let mut active = match self.current {
            Current::Ignoring => return,
            Current::Idle => {
                if self.power == Power::Running {
                    self.report(DiagnosticKind::DataWithoutCommand { data });
                }
                self.current = Current::Ignoring;
                return;
            }
            Current::Active(active) => active,
        };
        let index = active.received;
        active.received = active.received.saturating_add(1);
        let accepted = match active.kind {
            Kind::Write(Params::Exactly(count) | Params::Between(_, count)) => index < count,
            Kind::Write(Params::Stream) => true,
            Kind::Read | Kind::Unsupported(_) | Kind::Reserved => false,
        };
        if accepted {
            if matches!(active.kind, Kind::Write(Params::Stream)) {
                self.write_ram(&mut active, data);
            } else {
                self.parameter(active.command, index, data);
            }
        } else if !active.extra_reported {
            active.extra_reported = true;
            self.report(DiagnosticKind::ExtraData {
                command: active.command,
            });
        }
        // A parameter can put the chip to sleep or start an operation; the command stays the
        // current one either way, so that more data are reported against it.
        if let Current::Active(current) = &mut self.current {
            *current = active;
        }
    }

    /// A byte with D/C# high, read by the MCU (the half-duplex SDA of the X4-family probe, or
    /// SDO). `None` when the chip drives nothing: the line floats, the board decides its level.
    pub fn read_data(&mut self, now: Cycles) -> Option<u8> {
        self.advance(now);
        if self.power != Power::Running {
            return None;
        }
        let Current::Active(mut active) = self.current else {
            return None;
        };
        let index = active.read;
        active.read = active.read.saturating_add(1);
        let byte = if active.kind == Kind::Read {
            self.read_byte(&mut active, index)
        } else {
            if !active.nothing_reported {
                active.nothing_reported = true;
                self.report(DiagnosticKind::NothingToRead {
                    command: active.command,
                });
            }
            None
        };
        self.current = Current::Active(active);
        byte
    }

    // --- Internals --------------------------------------------------------------------------

    fn report(&mut self, kind: DiagnosticKind) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(Diagnostic { at: self.now, kind });
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// Reports the first reserved or unlisted value of the current command; the others of the
    /// same command are not reported again.
    fn reserved(&mut self, command: u8, value: u8) {
        if !self.reserved_reported {
            self.reserved_reported = true;
            self.report(DiagnosticKind::ReservedValue { command, value });
        }
    }

    /// `ns` nanoseconds after now, as an instant.
    fn after(&self, ns: u64) -> Cycles {
        Cycles(
            self.now
                .get()
                .saturating_add(self.time_base.nanos_to_cycles(u128::from(ns)).get()),
        )
    }

    /// Nanoseconds from `from` to `to`, 0 when `to` is earlier.
    fn ns_between(&self, from: Cycles, to: Cycles) -> u64 {
        self.time_base
            .cycles_to_nanos(Cycles(to.get().saturating_sub(from.get())))
    }

    /// BUSY high for `ns` nanoseconds from now; nothing for 0.
    fn busy_for(&mut self, ns: u64) {
        if ns > 0 {
            self.busy_until = Some(self.after(ns));
        }
    }

    /// Reports the parameters the current command did not get.
    fn close_command(&mut self) {
        if let Current::Active(active) = self.current {
            let expected = match active.kind {
                Kind::Write(Params::Exactly(count) | Params::Between(count, _)) => count,
                _ => 0,
            };
            if active.received < expected {
                self.report(DiagnosticKind::MissingData {
                    command: active.command,
                    expected,
                    received: active.received,
                });
            }
        }
        self.current = Current::Ignoring;
        self.reserved_reported = false;
    }

    /// The registers at their reset values, the supplies off, the status bits cleared.
    fn restore_registers(&mut self) {
        self.regs = Registers::reset(self.config.user_id);
        self.clock = false;
        self.analog = false;
        self.status = Status::default();
    }

    /// What a command does when its byte arrives.
    fn start_command(&mut self, command: u8) {
        match command {
            0x12 => {
                self.restore_registers();
                self.busy_for(self.config.timing.soft_reset);
            }
            0x20 => self.activate(),
            0x28 => {
                if self.powered(Stage::VcomSense) {
                    let seconds = u64::from(self.regs.vcom_sense & 0x0F) + 1;
                    self.busy_for(seconds * SECOND);
                }
            }
            0x24 | 0x26 | 0x27 => {
                let at = self.regs.counter;
                if !at.within(self.regs.window) {
                    self.report(DiagnosticKind::CounterOutsideWindow {
                        command,
                        x: at.x,
                        y: at.y,
                    });
                }
            }
            _ => {}
        }
    }

    /// Whether the clock and the analog supplies are on, reporting the one that is not.
    fn powered(&mut self, stage: Stage) -> bool {
        if !self.clock {
            self.report(DiagnosticKind::WithoutClock { stage });
            false
        } else if !self.analog {
            self.report(DiagnosticKind::WithoutAnalog { stage });
            false
        } else {
            true
        }
    }

    /// Reports `byte` of `command` unless the datasheet lists it.
    fn check(&mut self, command: u8, byte: u8, listed: bool) {
        if !listed {
            self.reserved(command, byte);
        }
    }

    /// Parameter byte `index` of `command`.
    fn parameter(&mut self, command: u8, index: u16, byte: u8) {
        match command {
            0x01 => self.driver_output(index, byte),
            0x03 | 0x04 | 0x2C | 0x32 => self.waveform_byte(command, index, byte),
            0x10 => self.deep_sleep(byte),
            0x14 => self.hv_ready_detection(byte),
            0x15 => self.vci_detection(byte),
            0x44 | 0x45 => self.window_byte(command, index, byte),
            0x46 | 0x47 => {
                let plane = if command == 0x46 {
                    Plane::Red
                } else {
                    Plane::BlackWhite
                };
                self.ram.auto_write(plane, byte);
                self.check(command, byte, byte & 0b1000 == 0);
                self.busy_for(self.config.timing.auto_write);
            }
            0x4E => self.regs.counter.x = ten_bits(self.regs.counter.x, index, byte),
            0x4F => self.regs.counter.y = ten_bits(self.regs.counter.y, index, byte),
            _ => self.register_byte(command, index, byte),
        }
    }

    /// `0x01`: MUX low, MUX high, then the gate-scan bits (TB = 1 is reserved and read as 0).
    fn driver_output(&mut self, index: u16, byte: u8) {
        match index {
            0 | 1 => {
                self.regs.mux = ten_bits(self.regs.mux, index, byte);
                if index == 1 {
                    let lines = self.regs.mux + 1;
                    let listed = byte & !0b11 == 0 && (300..=RAM_GATES).contains(&lines);
                    self.check(0x01, byte, listed);
                }
            }
            _ => {
                self.regs.gate_scan = GateScan {
                    sm: byte & 0b010 != 0,
                    gd: byte & 0b100 != 0,
                };
                self.check(0x01, byte, byte & !0b110 == 0);
            }
        }
    }

    /// The bytes of the LUT register: `0x32` (VS, TP, RP, frame rates), `0x03` (VGH), `0x04`
    /// (VSH1, VSH2, VSL), `0x2C` (DCVCOM).
    fn waveform_byte(&mut self, command: u8, index: u16, byte: u8) {
        let index = usize::from(index);
        let (at, listed) = match command {
            0x03 => (105, gate_code_is_listed(byte)),
            0x04 if index < 2 => (106 + index, source_millivolts(byte).is_some()),
            0x04 => (108, vsl_millivolts(byte).is_some()),
            0x2C => (109, vcom_millivolts(byte).is_some()),
            // LUT4 (bytes 40 to 49) drives VCOM, for which code 11 is "N/A".
            _ => {
                let vcom_lut = (40..50).contains(&index);
                let not_applicable = (0..4).any(|phase| (byte >> (2 * phase)) & 0b11 == 0b11);
                (index, !(vcom_lut && not_applicable))
            }
        };
        self.regs.lut[at] = byte;
        self.check(command, byte, listed);
    }

    /// The registers that only hold their bytes. The booster soft-start (`0x0C`) and the VCOM
    /// glitch control (`0x2B`) shape currents and edges the model has none of: accepted without
    /// effect.
    fn register_byte(&mut self, command: u8, index: u16, byte: u8) {
        let regs = &mut self.regs;
        let listed = match (command, index) {
            (0x11, _) => {
                regs.entry = Entry(byte & 0b111);
                byte & !0b111 == 0
            }
            (0x18, _) => {
                regs.sensor = byte;
                byte == INTERNAL_SENSOR || byte == EXTERNAL_SENSOR
            }
            (0x1A, 0) => {
                regs.temperature = regs.temperature & 0x00F | u16::from(byte) << 4;
                true
            }
            (0x1A, _) => {
                regs.temperature = regs.temperature & 0xFF0 | u16::from(byte >> 4);
                // A[3:0] sits in the high nibble; the low one must be 0.
                byte.trailing_zeros() >= 4
            }
            (0x21, 0) => {
                regs.update_control_1[0] = byte;
                let option = |nibble: u8| matches!(nibble, 0b0000 | 0b0100 | 0b1000);
                option(byte >> 4) && option(byte & 0x0F)
            }
            (0x21, _) => {
                regs.update_control_1[1] = byte;
                true
            }
            (0x22, _) => {
                regs.update_control_2 = byte;
                true
            }
            (0x29, _) => {
                regs.vcom_sense = byte;
                byte & 0xB0 == 0 && byte & 0x40 != 0
            }
            (0x37, _) => {
                regs.display_option[usize::from(index)] = byte;
                true
            }
            (0x38, _) => {
                regs.user_id[usize::from(index)] = byte;
                true
            }
            (0x3C, _) => {
                regs.border = byte;
                byte & 0b1100 == 0
            }
            (0x41, _) => {
                regs.read_ram_red = byte & 1 != 0;
                byte & !1 == 0
            }
            _ => true,
        };
        self.check(command, byte, listed);
    }

    /// A byte of `0x44` (X: start low, start high, end low, end high, up to 3BFh) or `0x45` (Y, up
    /// to 2A7h).
    fn window_byte(&mut self, command: u8, index: u16, byte: u8) {
        let window = &mut self.regs.window;
        let (field, limit) = match (command, index < 2) {
            (0x44, true) => (&mut window.x_start, RAM_SOURCES - 1),
            (0x44, false) => (&mut window.x_end, RAM_SOURCES - 1),
            (_, true) => (&mut window.y_start, RAM_GATES - 1),
            (_, false) => (&mut window.y_end, RAM_GATES - 1),
        };
        let low_byte = index.is_multiple_of(2);
        *field = ten_bits(*field, u16::from(!low_byte), byte);
        let listed = low_byte || (byte & !0b11 == 0 && *field <= limit);
        self.check(command, byte, listed);
    }

    /// A data byte of `0x24` or `0x26`, at the counter.
    fn write_ram(&mut self, active: &mut Active, byte: u8) {
        let plane = if active.command == 0x24 {
            Plane::BlackWhite
        } else {
            Plane::Red
        };
        let at = self.regs.counter;
        if !self.ram.write(plane, at, byte) && !active.range_reported {
            active.range_reported = true;
            self.report(DiagnosticKind::RamOutOfRange {
                command: active.command,
                x: at.x,
                y: at.y,
            });
        }
        self.regs.counter.advance(self.regs.window, self.regs.entry);
    }

    /// Data byte `index` read after a read command.
    fn read_byte(&mut self, active: &mut Active, index: u16) -> Option<u8> {
        let regs = &self.regs;
        let index = usize::from(index);
        match active.command {
            0x1B => {
                // A[11:4], then A[3:0] in the high nibble: the register shifted into 16 bits.
                (regs.temperature << 4).to_be_bytes().get(index).copied()
            }
            0x2D => {
                // A: VCOM OTP selection (0x37 byte A); B: the VCOM register; C–G: display modes
                // (0x37 bytes B–F); H–K: waveform version (0x37 bytes G–J).
                let o = &regs.display_option;
                let bytes = [
                    o[0],
                    regs.lut[109],
                    o[1],
                    o[2],
                    o[3],
                    o[4],
                    o[5],
                    o[6],
                    o[7],
                    o[8],
                    o[9],
                ];
                bytes.get(index).copied()
            }
            0x2E => regs.user_id.get(index).copied(),
            0x2F => (index == 0).then(|| {
                u8::from(self.status.hv_not_ready) << 5
                    | u8::from(self.status.vci_low) << 4
                    | u8::from(self.is_busy(self.now)) << 2
                    | 0x01
            }),
            _ => {
                // 0x27: a dummy byte first, then the RAM at the counter, which advances
                // ("Address pointers will advance accordingly", Table 7-1; §8.5 says otherwise).
                if index == 0 {
                    return Some(0x00);
                }
                let plane = if regs.read_ram_red {
                    Plane::Red
                } else {
                    Plane::BlackWhite
                };
                let at = regs.counter;
                let byte = self.ram.read(plane, at);
                if byte.is_none() && !active.range_reported {
                    active.range_reported = true;
                    self.report(DiagnosticKind::RamOutOfRange {
                        command: active.command,
                        x: at.x,
                        y: at.y,
                    });
                }
                self.regs.counter.advance(self.regs.window, self.regs.entry);
                byte
            }
        }
    }

    /// `0x10`: 00 normal; 11 deep sleep, the RAM lost; 01 and 10 reserved, read as a deep sleep
    /// that keeps the RAM (ASSUMPTION, the SSD1680's mode 1).
    fn deep_sleep(&mut self, byte: u8) {
        if byte & !0b11 != 0 {
            self.reserved(0x10, byte);
        }
        match byte & 0b11 {
            0b00 => return,
            0b11 => {
                // ASSUMPTION: what the RAM reads after deep sleep 11 is undefined; 0 here.
                self.ram.fill(0x00);
            }
            _ => self.reserved(0x10, byte),
        }
        self.power = Power::Asleep;
        self.clock = false;
        self.analog = false;
        self.busy_until = None;
    }

    /// `0x14`: n = `A[6:4]`, m = `A[2:0]`. With the supplies up the HV is ready at the first
    /// detection, after a cool-down of 10 ms × (n + 1). Without them (the command requires CLKEN
    /// and ANALOGEN) the detection cannot succeed: status bit 5 reads "not ready".
    fn hv_ready_detection(&mut self, byte: u8) {
        if byte & 0x88 != 0 {
            self.reserved(0x14, byte);
        }
        if self.powered(Stage::HvReadyDetection) {
            self.status.hv_not_ready = false;
            let cool_down = u64::from((byte >> 4) & 0b111) + 1;
            self.busy_for(cool_down * 10 * MILLISECOND);
        } else {
            self.status.hv_not_ready = true;
        }
    }

    /// `0x15`: compares VCI with 2.2 V to 2.6 V (`A[2:0]` = 3 to 7; 2.3 V at reset).
    fn vci_detection(&mut self, byte: u8) {
        let level = byte & 0b111;
        if byte & !0b111 != 0 || level < 3 {
            self.reserved(0x15, byte);
        }
        let millivolts = if level >= 3 {
            2_200 + 100 * u32::from(level - 3)
        } else {
            2_300
        };
        if self.powered(Stage::VciDetection) {
            self.status.vci_low = self.config.vci_millivolts < millivolts;
            self.busy_for(self.config.timing.vci_detection);
        }
    }

    /// The temperature load: the internal sensor, or the external one; returns its duration.
    fn load_temperature(&mut self) -> u64 {
        let timing = self.config.timing;
        if self.regs.sensor == INTERNAL_SENSOR {
            self.regs.temperature = self.config.ambient.register();
            timing.temperature_internal
        } else {
            match self.config.external_sensor {
                Some(t) => self.regs.temperature = t.register(),
                None => self.report(DiagnosticKind::NoExternalSensor),
            }
            timing.temperature_external
        }
    }

    /// `0x20`: runs the stages `0x22` enables, in their order (research §3.5).
    fn activate(&mut self) {
        let control = self.regs.update_control_2;
        let timing = self.config.timing;
        let mut ns: u64 = 0;
        if control & CLOCK_ON != 0 && !self.clock {
            self.clock = true;
            ns = ns.saturating_add(timing.clock_on);
        }
        if control & ANALOG_ON != 0 && !self.analog {
            if self.clock {
                self.analog = true;
                ns = ns.saturating_add(timing.analog_on);
            } else {
                self.report(DiagnosticKind::WithoutClock {
                    stage: Stage::AnalogOn,
                });
            }
        }
        if control & LOAD_TEMPERATURE != 0 {
            if self.clock {
                ns = ns.saturating_add(self.load_temperature());
            } else {
                self.report(DiagnosticKind::WithoutClock {
                    stage: Stage::TemperatureLoad,
                });
            }
        }
        let mode = if control & MODE_TWO != 0 {
            DisplayMode::Two
        } else {
            DisplayMode::One
        };
        let mut waveform_found = true;
        if control & LOAD_LUT != 0 {
            if self.clock {
                let temperature = Temperature::from_register(self.regs.temperature);
                if let Some(waveform) = find_waveform(&self.config.otp, mode, temperature) {
                    self.regs.lut = waveform.setting;
                } else {
                    waveform_found = false;
                    self.report(DiagnosticKind::NoWaveform { mode, temperature });
                }
                ns = ns.saturating_add(timing.lut_load);
            } else {
                self.report(DiagnosticKind::WithoutClock {
                    stage: Stage::LutLoad,
                });
            }
        }
        if control & DISPLAY != 0 && waveform_found && self.powered(Stage::Display) {
            ns = ns.saturating_add(self.start_display(mode, ns));
        }
        if control & ANALOG_OFF != 0 && self.analog {
            self.analog = false;
            ns = ns.saturating_add(timing.analog_off);
        }
        if control & CLOCK_OFF != 0 {
            self.clock = false;
        }
        self.busy_for(ns);
        // A display of no frame ends now.
        self.advance(self.now);
    }

    /// Starts the display `before_ns` nanoseconds from now; returns its duration.
    fn start_display(&mut self, mode: DisplayMode, before_ns: u64) -> u64 {
        if mode == DisplayMode::Two && self.regs.display_option[5] & 0x40 != 0 {
            self.report(DiagnosticKind::RamPingPong);
        }
        let waveform = Waveform::from_setting(&self.regs.lut, &self.config.timing.frame_period);
        let duration = waveform.duration_ns();
        let scan = Scan {
            lines: self.regs.mux + 1,
            order: self.regs.gate_scan,
            red: PlaneOption::from_nibble(self.regs.update_control_1[0] >> 4),
            black_white: PlaneOption::from_nibble(self.regs.update_control_1[0]),
        };
        let border = self.border_drive();
        let kind = match mode {
            DisplayMode::One => RefreshKind::Full,
            DisplayMode::Two => RefreshKind::Partial,
        };
        let started = self.after(before_ns);
        let ends = self.after(before_ns.saturating_add(duration));
        self.display =
            Some(
                self.panel
                    .start(&self.ram, scan, waveform, border, kind, (started, ends)),
            );
        duration
    }

    /// What drives the border during the display (`0x3C`): a LUT, a fixed level, or nothing (VCOM
    /// and `HiZ` leave it as it is).
    fn border_drive(&self) -> Option<Drive> {
        let border = self.regs.border;
        match border >> 6 {
            0b00 => Some(Drive::Lut(usize::from(border & 0b11))),
            0b01 => Some(Drive::Fixed((border >> 4) & 0b11)),
            _ => None,
        }
    }
}
