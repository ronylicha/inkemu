//! The X3's e-paper controller, a UC8253 (first batch) or a `UC8279d` (later batch), as
//! `CrossPoint` drives it: an interpreter of its commands at the level of command and data bytes (ADR 0015;
//! research `sp3-x3-chips.md` §2, `sp3-x3-firmware.md` §3).
//!
//! The X3's commands have the layout of the UC8179's datasheet (DS79), not of the small UC8253's
//! (research §1): a 4-byte `TRES`, a 9-byte `PTL`, two RAM planes of 800 × 600 bits. The model
//! plays KW mode with its waveform in the LUT registers, the mode every X3 driver sets
//! (`PSR` `3F`): `DTM1` writes the OLD plane, `DTM2` the NEW one, and `DRF` drives each pixel by the
//! LUT of its old-to-new transition (WW, KW, WK, KK) while VCOM follows LUT `0x20`.
//!
//! `BUSY_N` is low while the chip works (`PON`, `POF`, `DRF`, `DSLP`, a reset), high otherwise: the
//! opposite of the X4's SSD1677. The SPI framing belongs to whoever wires the chip
//! ([`lines`]): it hands over whole bytes, telling commands from data. Time is the device's, an
//! instant in cycles of a 1 GHz time base once wired; the chip never reads a clock and asks to be
//! called back ([`Uc8253::next_deadline`]).

mod config;
mod diagnostic;
pub mod lines;
#[cfg(test)]
mod tests;
mod waveform;

use emu_core::{Cycles, Frequency, Level};

pub use crate::glass::{PanelImage, RefreshKind};
use crate::glass::{gray, level};
pub use config::{
    ConfigError, FRAME_RATE_CODES, LUT_BYTES, LUT_MAX_BYTES, RAM_GATES, RAM_SOURCES, Uc8253Config,
    Uc8253Geometry, Uc8253Timing, Variant,
};
pub use diagnostic::{Diagnostic, DiagnosticKind};

use waveform::{LUT_KK, LUT_KW, LUT_WK, LUT_WW, LUTS, Levels, Waveform};

/// The diagnostics kept until [`Uc8253::take_diagnostics`]; past it they are counted, not kept.
pub const MAX_DIAGNOSTICS: usize = 1024;

/// Bytes of a RAM row: 800 bits.
const ROW_BYTES: usize = RAM_SOURCES as usize / 8;

// The commands (DS79's table; research `sp3-x3-chips.md` §2.2).
const PSR: u8 = 0x00;
const PWR: u8 = 0x01;
const POF: u8 = 0x02;
const PFS: u8 = 0x03;
const PON: u8 = 0x04;
const PMES: u8 = 0x05;
const BTST: u8 = 0x06;
const DSLP: u8 = 0x07;
const DTM1: u8 = 0x10;
const DSP: u8 = 0x11;
const DRF: u8 = 0x12;
const DTM2: u8 = 0x13;
const DUSPI: u8 = 0x15;
const AUTO: u8 = 0x17;
const LUTC: u8 = 0x20;
const LUTKK: u8 = 0x24;
const LUTBD: u8 = 0x25;
const LUTOPT: u8 = 0x2A;
const KWOPT: u8 = 0x2B;
const PLL: u8 = 0x30;
const TSC: u8 = 0x40;
const TSE: u8 = 0x41;
const TSW: u8 = 0x42;
const TSR: u8 = 0x43;
const PBC: u8 = 0x44;
const CDI: u8 = 0x50;
const LPD: u8 = 0x51;
const EVS: u8 = 0x52;
const TCON: u8 = 0x60;
const TRES: u8 = 0x61;
const GSST: u8 = 0x65;
const REV: u8 = 0x70;
const FLG: u8 = 0x71;
const CRC: u8 = 0x72;
const AMV: u8 = 0x80;
const VV: u8 = 0x81;
const VDCS: u8 = 0x82;
const PTL: u8 = 0x90;
const PTIN: u8 = 0x91;
const PTOUT: u8 = 0x92;
const PGM: u8 = 0xA0;
const APG: u8 = 0xA1;
const ROTP: u8 = 0xA2;
const CCSET: u8 = 0xE0;
/// `0xE1`: in neither datasheet; both X3 drivers send `02` ("LV selection" in `FreeInk`). Stored.
const LVSEL_X3: u8 = 0xE1;
const PWS: u8 = 0xE3;
const LVSEL: u8 = 0xE4;
const TSSET: u8 = 0xE5;
const TSBDRY: u8 = 0xE7;

/// `DSLP`'s check code.
const DEEP_SLEEP_CODE: u8 = 0xA5;

/// `PSR` bits (DS79): LUT from the registers, KW mode, gate scan up, source shift right, booster
/// on, no soft reset.
const PSR_REG: u8 = 1 << 5;
const PSR_KW: u8 = 1 << 4;
const PSR_UD: u8 = 1 << 3;
const PSR_SHL: u8 = 1 << 2;
const PSR_SHD_N: u8 = 1 << 1;
const PSR_RST_N: u8 = 1 << 0;
/// `CDI` byte 1 bit 3 in DS79's layout: copy NEW to OLD after a refresh.
const CDI_N2OCP: u8 = 1 << 3;

/// How many parameter bytes a command takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Params {
    Exactly(u16),
    Between(u16, u16),
    /// Pixel bytes, as many as written.
    Stream,
}

/// What the model does with a command byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Write(Params),
    /// A command whose data bytes the MCU reads.
    Read,
    Unsupported(&'static str, Params),
}

/// The commands, with `CDI`'s length by variant (DS79 two bytes, the `UC8279d` one).
fn kind(command: u8, variant: Variant) -> Option<Kind> {
    use Kind::{Read, Unsupported, Write};
    use Params::{Between, Exactly, Stream};
    Some(match command {
        POF | PON | PMES | DRF | PTIN | PTOUT => Write(Exactly(0)),
        PSR => Write(Between(1, 2)),
        PWR => Write(Between(4, 5)),
        BTST => Write(Between(3, 4)),
        PFS | DSLP | PLL | TSE | EVS | TCON | AMV | VDCS | CCSET | LVSEL_X3 | PWS | LVSEL
        | TSSET | TSBDRY => Write(Exactly(1)),
        CDI => match variant {
            Variant::Uc8253 => Write(Between(1, 2)),
            Variant::Uc8279d => Write(Exactly(1)),
        },
        TSW | KWOPT => Write(Exactly(3)),
        TRES | GSST => Write(Exactly(4)),
        PTL => Write(Exactly(9)),
        LUTOPT => Write(Exactly(2)),
        DTM1 | DTM2 => Write(Stream),
        LUTC..=LUTBD => Write(Between(1, LUT_MAX_BYTES)),
        DSP | TSC | TSR | PBC | LPD | REV | FLG | CRC | VV | ROTP => Read,
        DUSPI => Unsupported("Dual SPI", Exactly(1)),
        AUTO => Unsupported("Auto Sequence", Exactly(1)),
        PGM => Unsupported("Program Mode", Exactly(0)),
        APG => Unsupported("Active Program", Exactly(0)),
        _ => return None,
    })
}

/// A rectangle of RAM, in pixels, its ends included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Area {
    x0: u16,
    y0: u16,
    x1: u16,
    y1: u16,
}

impl Area {
    fn contains(self, x: u16, y: u16) -> bool {
        (self.x0..=self.x1).contains(&x) && (self.y0..=self.y1).contains(&y)
    }
}

/// The registers, at their reset values (DS79). A soft reset, a hardware reset and power-on all
/// restore them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Registers {
    psr: [u8; 2],
    power: [u8; 5],
    pll: u8,
    cdi: [u8; 2],
    /// `TRES`: the active area's width and height.
    resolution: (u16, u16),
    /// `GSST`: its origin.
    start: (u16, u16),
    /// `PTL`: the partial window, its ends included, and `PT_SCAN`.
    window: Area,
    partial: bool,
    luts: [[u8; LUT_BYTES]; LUTS],
}

impl Registers {
    fn reset(variant: Variant) -> Registers {
        Registers {
            psr: [0x0F, 0x8D],
            power: [0x07, 0x17, 0x3A, 0x3A, 0x03],
            pll: 0x06,
            cdi: match variant {
                Variant::Uc8253 => [0x31, 0x07],
                Variant::Uc8279d => [0xD7, 0x00],
            },
            resolution: (RAM_SOURCES, RAM_GATES),
            start: (0, 0),
            window: Area {
                x0: 0,
                y0: 0,
                x1: RAM_SOURCES - 1,
                y1: RAM_GATES - 1,
            },
            partial: false,
            luts: [[0; LUT_BYTES]; LUTS],
        }
    }

    /// The active area `TRES` and `GSST` set, within the RAM.
    fn active(&self) -> Area {
        let (width, height) = self.resolution;
        let (x0, y0) = self.start;
        Area {
            x0: x0.min(RAM_SOURCES - 1),
            y0: y0.min(RAM_GATES - 1),
            x1: x0.saturating_add(width.max(1) - 1).min(RAM_SOURCES - 1),
            y1: y0.saturating_add(height.max(1) - 1).min(RAM_GATES - 1),
        }
    }

    /// The area a pixel stream fills: the partial window in partial mode, else the active area.
    fn written(&self) -> Area {
        if self.partial {
            self.window
        } else {
            self.active()
        }
    }

    /// `DDX[0]`: 1 means a bit of 1 is white (the default, every X3 value).
    fn one_is_white(&self, variant: Variant) -> bool {
        match variant {
            Variant::Uc8253 => self.cdi[0] & 0b01 != 0,
            Variant::Uc8279d => (self.cdi[0] >> 4) & 0b01 != 0,
        }
    }

    /// `N2OCP`: NEW copied to OLD after a refresh (DS79's layout only).
    fn copies_new_to_old(&self, variant: Variant) -> bool {
        variant == Variant::Uc8253 && self.cdi[0] & CDI_N2OCP != 0
    }
}

/// A plane of the RAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Plane {
    /// `DTM1` in KW mode: the frame on the glass.
    Old,
    /// `DTM2`: the frame to show.
    New,
}

/// The two planes, 800 × 600 bits each.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ram {
    planes: [Vec<u8>; 2],
}

impl Ram {
    fn new() -> Ram {
        // ASSUMPTION: the RAM reads 0 at power-on; the datasheet leaves it undefined.
        let bytes = ROW_BYTES * usize::from(RAM_GATES);
        Ram {
            planes: [vec![0; bytes], vec![0; bytes]],
        }
    }

    fn index(plane: Plane) -> usize {
        match plane {
            Plane::Old => 0,
            Plane::New => 1,
        }
    }

    fn bit(&self, plane: Plane, x: u16, y: u16) -> Option<bool> {
        let byte = self
            .planes
            .get(Ram::index(plane))?
            .get(usize::from(y) * ROW_BYTES + usize::from(x / 8))?;
        Some(byte & (0x80 >> (x % 8)) != 0)
    }

    /// Writes the eight pixels of `byte` from (`x`, `y`), those within `area`.
    fn write(&mut self, plane: Plane, x: u16, y: u16, byte: u8, area: Area) {
        let Some(row) = self.planes.get_mut(Ram::index(plane)) else {
            return;
        };
        for bit in 0..8u16 {
            let column = x + bit;
            if !area.contains(column, y) {
                continue;
            }
            let Some(target) = row.get_mut(usize::from(y) * ROW_BYTES + usize::from(column / 8))
            else {
                continue;
            };
            let mask = 0x80 >> (column % 8);
            if byte & (0x80 >> bit) != 0 {
                *target |= mask;
            } else {
                *target &= !mask;
            }
        }
    }
}

/// The supply and reset state of the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Power {
    Running,
    /// `DSLP` with its code: every command dropped until `RST_N` goes low.
    Asleep,
    /// `RST_N` is low.
    InReset,
}

/// What keeps `BUSY_N` low.
#[derive(Clone, Debug)]
enum Work {
    PowerOn,
    PowerOff,
    DeepSleep,
    SoftReset,
    /// The initialisation after `RST_N` rose.
    HardwareReset,
    Refresh(Box<Refresh>),
}

#[derive(Clone, Debug)]
struct Operation {
    work: Work,
    started: Cycles,
    until: Cycles,
}

/// A refresh under way: the LUT of every glass pixel, fixed when `DRF` came.
#[derive(Clone, Debug)]
struct Refresh {
    waveform: Waveform,
    /// For each glass pixel, row by row, the LUT that drives it; [`UNDRIVEN`] for one not driven.
    luts: Vec<u8>,
    /// The RAM driven, for `N2OCP`.
    area: Area,
    kind: RefreshKind,
}

/// A glass pixel no LUT drives in a refresh.
const UNDRIVEN: u8 = u8::MAX;

/// The command the data bytes belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Current {
    /// No command since the last reset: a data byte is reported once.
    Idle,
    /// A dropped or unknown command: data go nowhere.
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
    /// Where the next pixel byte of a stream goes.
    cursor: (u16, u16),
    extra_reported: bool,
    past_reported: bool,
}

/// The X3's controller and the glass it drives.
#[derive(Debug)]
pub struct Uc8253 {
    config: Uc8253Config,
    time_base: Frequency,
    now: Cycles,
    regs: Registers,
    ram: Ram,
    power: Power,
    /// `PON` brought the supplies up and no `POF` took them down.
    powered: bool,
    /// `DSP` came after the last plane.
    data_flag: bool,
    operation: Option<Operation>,
    current: Current,
    /// The glass: the optical state of each pixel, row by row, and the image last published.
    states: Vec<i64>,
    swing: i64,
    image: PanelImage,
    diagnostics: Vec<Diagnostic>,
    dropped: u64,
}

impl Uc8253 {
    /// The chip at power-on under `config`, counting time in cycles of `time_base`.
    ///
    /// # Errors
    /// The parameters [`ConfigError`] lists.
    pub fn new(config: Uc8253Config, time_base: Frequency) -> Result<Uc8253, ConfigError> {
        config.validate()?;
        let swing = i64::try_from(config.optics.full_swing).map_err(|_| ConfigError::FullSwing)?;
        let (width, height) = (config.panel.sources, config.panel.gates);
        let state = level(config.optics.initial_gray, swing);
        let count = usize::from(width) * usize::from(height);
        let image = PanelImage {
            width,
            height,
            pixels: vec![config.optics.initial_gray; count],
            border: config.optics.initial_gray,
            generation: 0,
            kind: RefreshKind::PowerOn,
            interrupted: false,
            started: Cycles::ZERO,
            at: Cycles::ZERO,
        };
        Ok(Uc8253 {
            regs: Registers::reset(config.variant),
            config,
            time_base,
            now: Cycles::ZERO,
            ram: Ram::new(),
            power: Power::Running,
            powered: false,
            data_flag: false,
            operation: None,
            current: Current::Idle,
            states: vec![state; count],
            swing,
            image,
            diagnostics: Vec::new(),
            dropped: 0,
        })
    }

    /// The configuration the chip was built with.
    #[must_use]
    pub fn config(&self) -> &Uc8253Config {
        &self.config
    }

    /// What the glass shows since the last refresh ended.
    #[must_use]
    pub fn panel_image(&self) -> &PanelImage {
        &self.image
    }

    /// A pixel of the RAM, `None` past it.
    #[must_use]
    pub fn ram_bit(&self, plane: Plane, x: u16, y: u16) -> Option<bool> {
        if x >= RAM_SOURCES || y >= RAM_GATES {
            return None;
        }
        self.ram.bit(plane, x, y)
    }

    /// Whether the supplies are up (`PON` done, no `POF` since).
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.powered
    }

    /// Whether the chip is in deep sleep.
    #[must_use]
    pub fn is_asleep(&self) -> bool {
        self.power == Power::Asleep
    }

    /// The level of `BUSY_N` at `now`: low while the chip works or `RST_N` is low.
    #[must_use]
    pub fn busy_pin(&self, now: Cycles) -> Level {
        let working = self.operation.as_ref().is_some_and(|op| op.until > now);
        Level::from_bool(!(working || self.power == Power::InReset))
    }

    /// The next instant `BUSY_N` rises or an image is published.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Cycles> {
        self.operation.as_ref().map(|op| op.until)
    }

    /// The diagnostics since the last call, oldest first.
    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// The diagnostics dropped past [`MAX_DIAGNOSTICS`].
    #[must_use]
    pub fn dropped_diagnostics(&self) -> u64 {
        self.dropped
    }

    /// Whether `command` is one whose data bytes the MCU reads.
    #[must_use]
    pub fn is_read_command(&self, command: u8) -> bool {
        matches!(kind(command, self.config.variant), Some(Kind::Read))
    }

    fn report(&mut self, kind: DiagnosticKind) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(Diagnostic { at: self.now, kind });
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn time(&mut self, now: Cycles) {
        self.now = self.now.max(now);
    }

    /// `ns` nanoseconds after `from`, in cycles of the time base.
    fn after(&self, from: Cycles, ns: u64) -> Cycles {
        let cycles = self.time_base.nanos_to_cycles(u128::from(ns));
        Cycles(from.get().saturating_add(cycles.get()))
    }

    fn start(&mut self, work: Work, ns: u64) {
        let until = self.after(self.now, ns);
        self.operation = Some(Operation {
            work,
            started: self.now,
            until,
        });
    }

    /// Brings the chip to `now`: an operation whose end has come ends, and a refresh publishes
    /// its image.
    pub fn advance(&mut self, now: Cycles) {
        self.time(now);
        let Some(op) = self.operation.take_if(|op| op.until <= self.now) else {
            return;
        };
        match op.work {
            Work::PowerOn => self.powered = true,
            Work::PowerOff => self.powered = false,
            Work::DeepSleep => {
                self.powered = false;
                self.power = Power::Asleep;
            }
            Work::SoftReset | Work::HardwareReset => {}
            Work::Refresh(refresh) => self.finish(&refresh, op.started, op.until, None),
        }
    }

    /// `RST_N` went low (`asserted`) or was released.
    pub fn set_reset(&mut self, now: Cycles, asserted: bool) {
        self.advance(now);
        if asserted {
            if self.power == Power::InReset {
                return;
            }
            if let Some(op) = self.operation.take()
                && let Work::Refresh(refresh) = op.work
            {
                let elapsed = self
                    .time_base
                    .cycles_to_nanos(Cycles(self.now.get().saturating_sub(op.started.get())));
                self.report(DiagnosticKind::ResetDuringRefresh);
                self.finish(&refresh, op.started, self.now, Some(elapsed));
            }
            self.close_command();
            self.regs = Registers::reset(self.config.variant);
            self.power = Power::InReset;
            self.powered = false;
            self.operation = None;
            self.current = Current::Idle;
        } else if self.power == Power::InReset {
            self.power = Power::Running;
            self.start(Work::HardwareReset, self.config.timing.hardware_reset);
        }
    }

    /// A command byte (D/C# low).
    pub fn write_command(&mut self, now: Cycles, command: u8) {
        self.advance(now);
        self.close_command();
        let refused = match (self.power, self.operation.as_ref().map(|op| &op.work)) {
            (Power::InReset, _) | (_, Some(Work::HardwareReset | Work::SoftReset)) => {
                Some(DiagnosticKind::WhileInReset { command })
            }
            (Power::Asleep, _) => Some(DiagnosticKind::WhileAsleep { command }),
            (Power::Running, Some(_)) if !matches!(command, REV | FLG | VV) => {
                Some(DiagnosticKind::WhileBusy { command })
            }
            _ => None,
        };
        if let Some(diagnostic) = refused {
            self.report(diagnostic);
            self.current = Current::Ignoring;
            return;
        }
        let Some(kind) = kind(command, self.config.variant) else {
            self.report(DiagnosticKind::UnknownCommand { command });
            self.current = Current::Ignoring;
            return;
        };
        if let Kind::Unsupported(name, _) = kind {
            self.report(DiagnosticKind::UnsupportedCommand { command, name });
        }
        let cursor = {
            let area = self.regs.written();
            (area.x0, area.y0)
        };
        self.current = Current::Active(Active {
            command,
            kind,
            received: 0,
            read: 0,
            cursor,
            extra_reported: false,
            past_reported: false,
        });
        self.execute(command);
    }

    /// What a command without parameters does at once.
    fn execute(&mut self, command: u8) {
        match command {
            PON if !self.powered => self.start(Work::PowerOn, self.config.timing.power_on),
            // A PON on a powered chip gives no BUSY_N pulse (FreeInk, `Uc8253X3Driver.cpp:238-239`).
            POF if self.powered => self.start(Work::PowerOff, self.config.timing.power_off),
            DRF => self.refresh(),
            PTIN => self.regs.partial = true,
            PTOUT => self.regs.partial = false,
            DTM1 | DTM2 => self.data_flag = false,
            DSP => self.data_flag = true,
            _ => {}
        }
    }

    /// A data byte (D/C# high).
    pub fn write_data(&mut self, now: Cycles, data: u8) {
        self.advance(now);
        let mut active = match self.current {
            Current::Idle => {
                self.report(DiagnosticKind::DataWithoutCommand { data });
                self.current = Current::Ignoring;
                return;
            }
            Current::Ignoring => return,
            Current::Active(active) => active,
        };
        let index = active.received;
        active.received = active.received.saturating_add(1);
        let params = match active.kind {
            Kind::Write(params) | Kind::Unsupported(_, params) => params,
            Kind::Read => Params::Exactly(0),
        };
        let max = match params {
            Params::Exactly(n) | Params::Between(_, n) => Some(n),
            Params::Stream => None,
        };
        if max.is_some_and(|max| index >= max) {
            if !active.extra_reported {
                active.extra_reported = true;
                self.report(DiagnosticKind::ExtraData {
                    command: active.command,
                });
            }
        } else if matches!(active.kind, Kind::Write(_)) {
            self.parameter(&mut active, index, data);
        }
        self.current = Current::Active(active);
    }

    /// Byte `index` of the parameters of the active command.
    fn parameter(&mut self, active: &mut Active, index: u16, data: u8) {
        let i = usize::from(index);
        let regs = &mut self.regs;
        match active.command {
            PSR => {
                if let Some(byte) = regs.psr.get_mut(i) {
                    *byte = data;
                }
                if index == 0 && data & PSR_RST_N == 0 {
                    // A soft reset: the registers return to their defaults (DS53: BUSY_N low for
                    // at least 50 µs, commands ignored), the byte after this one with them.
                    self.regs = Registers::reset(self.config.variant);
                    self.powered = false;
                    self.start(Work::SoftReset, self.config.timing.soft_reset);
                    active.kind = Kind::Write(Params::Exactly(1));
                    active.extra_reported = true;
                }
            }
            PWR => {
                if let Some(byte) = regs.power.get_mut(i) {
                    *byte = data;
                }
            }
            PLL => regs.pll = data,
            CDI => {
                if let Some(byte) = regs.cdi.get_mut(i) {
                    *byte = data;
                }
            }
            TRES => regs.resolution = ten_bits_pair(regs.resolution, index, data),
            GSST => regs.start = ten_bits_pair(regs.start, index, data),
            PTL => regs.window = window_byte(regs.window, index, data),
            DTM1 => self.pixels(active, Plane::Old, data),
            DTM2 => self.pixels(active, Plane::New, data),
            DSLP => {
                if data == DEEP_SLEEP_CODE {
                    self.start(Work::DeepSleep, self.config.timing.deep_sleep);
                } else {
                    self.report(DiagnosticKind::WrongCheckCode { code: data });
                }
            }
            LUTC..=LUTKK => {
                let lut = usize::from(active.command - LUTC);
                if let Some(byte) = regs.luts.get_mut(lut).and_then(|lut| lut.get_mut(i)) {
                    *byte = data;
                }
            }
            // Accepted without effect on the model: the supply timings (`PFS`, `TCON`, `PWS`), the
            // booster (`BTST`), VCOM's level (`VDCS`: the model takes each pixel's voltage from
            // VCOM_DC, as the SSD1677's does), the temperature settings, the border LUT, `0xE1`.
            _ => {}
        }
    }

    /// A pixel byte of a stream into `plane`: eight pixels at the cursor, which moves on along the
    /// row, then to the next row of the area.
    fn pixels(&mut self, active: &mut Active, plane: Plane, data: u8) {
        let area = self.regs.written();
        let (x, y) = active.cursor;
        if y > area.y1 {
            if !active.past_reported {
                active.past_reported = true;
                self.report(DiagnosticKind::PastTheWindow {
                    command: active.command,
                });
            }
            return;
        }
        self.ram.write(plane, x, y, data, area);
        let next = x.saturating_add(8);
        active.cursor = if next > area.x1 {
            (area.x0, y.saturating_add(1))
        } else {
            (next, y)
        };
    }

    /// The command under way ends (another one, or a reset, comes): one that took too few
    /// parameters is reported, a LUT of another length than KW mode reads too.
    fn close_command(&mut self) {
        let Current::Active(active) = self.current else {
            return;
        };
        let min = match active.kind {
            Kind::Write(Params::Exactly(n) | Params::Between(n, _))
            | Kind::Unsupported(_, Params::Exactly(n) | Params::Between(n, _)) => n,
            _ => 0,
        };
        if active.received < min {
            self.report(DiagnosticKind::MissingData {
                command: active.command,
                expected: min,
                received: active.received,
            });
        }
        let lut_bytes = u16::try_from(LUT_BYTES).unwrap_or(u16::MAX);
        if (LUTC..=LUTKK).contains(&active.command)
            && active.received != lut_bytes
            && active.received >= min
        {
            self.report(DiagnosticKind::LutLength {
                command: active.command,
                bytes: active.received,
            });
        }
        self.current = Current::Ignoring;
    }

    /// The next byte of a read: what the controller drives, `None` when it drives nothing.
    pub fn read_data(&mut self, now: Cycles) -> Option<u8> {
        self.advance(now);
        let Current::Active(mut active) = self.current else {
            return None;
        };
        if active.kind != Kind::Read {
            return None;
        }
        let index = usize::from(active.read);
        active.read = active.read.saturating_add(1);
        self.current = Current::Active(active);
        let byte = match self.config.variant {
            // The first batch drives nothing on a read: every byte reads as the line floats
            // (research `sp3-x3-chips.md` §2.6, "field-confirmed").
            Variant::Uc8253 => None,
            Variant::Uc8279d => self.uc8279d_read(active.command, index),
        };
        if byte.is_none() && index == 0 {
            self.report(DiagnosticKind::NothingToRead {
                command: active.command,
            });
        }
        byte
    }

    /// What the `UC8279d` drives on read `index` of `command`.
    ///
    /// ASSUMPTION: `REV` reads `00 03 66 FF FF`: the third byte `0x66` is the one `FreeInk` and the
    /// stock X3 firmware select this controller by; the other four follow Papyrix's description
    /// (reserved `00`, `CHIP_VER` `03`, then the LUT version). The two sources disagree (research
    /// `sp3-x3-chips.md` §2.6); a `UC8279d`'s datasheet or a logic trace settles it.
    fn uc8279d_read(&self, command: u8, index: usize) -> Option<u8> {
        match command {
            REV => [0x00, 0x03, 0x66, 0xFF, 0xFF].get(index).copied(),
            FLG => (index == 0).then(|| self.flags()),
            DSP => (index == 0).then(|| u8::from(self.data_flag) << 7),
            TSC => [self.config.ambient_celsius.to_le_bytes()[0], 0]
                .get(index)
                .copied(),
            _ => None,
        }
    }

    /// `FLG` (DS79): 6 `PTL_flag`, 4 `I2C_BUSYN` (idle), 3 `data_flag`, 2 `PON`, 1 `POF`, 0
    /// `BUSY_N` (1 idle).
    fn flags(&self) -> u8 {
        let busy = self.busy_pin(self.now) == Level::Low;
        u8::from(self.regs.partial) << 6
            | 1 << 4
            | u8::from(self.data_flag) << 3
            | u8::from(self.powered) << 2
            | u8::from(!self.powered) << 1
            | u8::from(!busy)
    }

    /// `DRF`: the refresh starts, each pixel driven by the LUT of its transition, or is refused.
    fn refresh(&mut self) {
        let psr = self.regs.psr[0];
        let refused = if psr & PSR_REG == 0 {
            Some(DiagnosticKind::RefreshNotModelled {
                what: "the LUTs from the OTP (PSR REG = 0)",
            })
        } else if psr & PSR_KW == 0 {
            Some(DiagnosticKind::RefreshNotModelled {
                what: "KWR mode (PSR KW/R = 0)",
            })
        } else if psr & PSR_SHD_N == 0 {
            Some(DiagnosticKind::RefreshNotModelled {
                what: "the booster off (PSR SHD_N = 0)",
            })
        } else if !self.powered {
            Some(DiagnosticKind::RefreshWithoutPower)
        } else {
            None
        };
        if let Some(diagnostic) = refused {
            self.report(diagnostic);
            return;
        }
        let frame_ns = self
            .config
            .timing
            .frame_period
            .get(usize::from(self.regs.pll & 0x1F))
            .copied()
            .unwrap_or(1);
        let waveform = Waveform::new(
            &self.regs.luts,
            Levels::from_power_setting(self.regs.power),
            frame_ns,
        );
        let area = self.regs.written();
        let luts = self.luts_of_pixels(area);
        let kind = if luts.contains(&UNDRIVEN) {
            RefreshKind::Partial
        } else {
            RefreshKind::Full
        };
        let duration = waveform.duration_ns();
        self.start(
            Work::Refresh(Box::new(Refresh {
                waveform,
                luts,
                area,
                kind,
            })),
            duration,
        );
    }

    /// The RAM pixel glass pixel (`x`, `y`) shows: its source and gate, then the scan directions
    /// of `PSR` within the active area. `None` for a line outside it.
    fn ram_pixel(&self, x: u16, y: u16) -> Option<(u16, u16)> {
        let panel = self.config.panel;
        let source = if panel.columns_reversed {
            panel.sources - 1 - x
        } else {
            x
        };
        let gate = if panel.rows_reversed {
            panel.gates - 1 - y
        } else {
            y
        };
        let active = self.regs.active();
        let psr = self.regs.psr[0];
        let column = if psr & PSR_SHL != 0 {
            source
        } else {
            (active.x0 + active.x1).checked_sub(source)?
        };
        let row = if psr & PSR_UD != 0 {
            gate
        } else {
            (active.y0 + active.y1).checked_sub(gate)?
        };
        active.contains(column, row).then_some((column, row))
    }

    /// The LUT of every glass pixel for a refresh of `area`.
    fn luts_of_pixels(&self, area: Area) -> Vec<u8> {
        let panel = self.config.panel;
        let one_is_white = self.regs.one_is_white(self.config.variant);
        let mut luts = Vec::with_capacity(self.states.len());
        for y in 0..panel.gates {
            for x in 0..panel.sources {
                let lut = self
                    .ram_pixel(x, y)
                    .filter(|&(column, row)| area.contains(column, row))
                    .and_then(|(column, row)| {
                        let white = |plane| {
                            self.ram
                                .bit(plane, column, row)
                                .map(|bit| bit == one_is_white)
                        };
                        Some(match (white(Plane::Old)?, white(Plane::New)?) {
                            (true, true) => LUT_WW,
                            (false, true) => LUT_KW,
                            (true, false) => LUT_WK,
                            (false, false) => LUT_KK,
                        })
                    })
                    .and_then(|lut| u8::try_from(lut).ok())
                    .unwrap_or(UNDRIVEN);
                luts.push(lut);
            }
        }
        luts
    }

    /// Applies `refresh`, begun at `started`, up to `until_ns` into it (all of it with `None`),
    /// and publishes the image dated `at`; a whole refresh copies NEW to OLD when `N2OCP` says so.
    fn finish(&mut self, refresh: &Refresh, started: Cycles, at: Cycles, until_ns: Option<u64>) {
        let optics = self.config.optics;
        let transfers: Vec<_> = (0..LUTS)
            .map(|lut| {
                refresh
                    .waveform
                    .transfer(lut, self.swing, optics.positive_drives_black, until_ns)
            })
            .collect();
        for (state, &lut) in self.states.iter_mut().zip(&refresh.luts) {
            if let Some(transfer) = transfers.get(usize::from(lut)) {
                *state = transfer.apply(*state);
            }
        }
        if until_ns.is_none() && self.regs.copies_new_to_old(self.config.variant) {
            let area = refresh.area;
            for y in area.y0..=area.y1 {
                for x in area.x0..=area.x1 {
                    if let Some(bit) = self.ram.bit(Plane::New, x, y) {
                        self.ram.write(
                            Plane::Old,
                            x,
                            y,
                            if bit { 0x80 } else { 0 },
                            Area {
                                x0: x,
                                y0: y,
                                x1: x,
                                y1: y,
                            },
                        );
                    }
                }
            }
        }
        self.image = PanelImage {
            width: self.config.panel.sources,
            height: self.config.panel.gates,
            pixels: self.states.iter().map(|&s| gray(s, self.swing)).collect(),
            border: self.image.border,
            generation: self.image.generation + 1,
            kind: refresh.kind,
            interrupted: until_ns.is_some(),
            started,
            at,
        };
    }
}

/// `TRES`/`GSST` byte `index` into the pair (horizontal, vertical): `[9:8]` then `[7:3]` of the
/// horizontal (its low three bits always 0), then `[9:8]` and `[7:0]` of the vertical (DS79).
fn ten_bits_pair((h, v): (u16, u16), index: u16, data: u8) -> (u16, u16) {
    let byte = u16::from(data);
    match index {
        0 => (h & 0x0FF | (byte & 0b11) << 8, v),
        1 => (h & 0x300 | (byte & 0xF8), v),
        2 => (h, v & 0x0FF | (byte & 0b11) << 8),
        _ => (h, v & 0x300 | byte),
    }
}

/// `PTL` byte `index` (DS79): `HRST[9:8]`, `HRST[7:3]`, `HRED[9:8]`, `HRED[7:3]` (its low three bits
/// read 111), `VRST[9:8]`, `VRST[7:0]`, `VRED[9:8]`, `VRED[7:0]`, `PT_SCAN`. The window keeps its
/// ends in the RAM.
fn window_byte(area: Area, index: u16, data: u8) -> Area {
    let byte = u16::from(data);
    let high = |value: u16| value & 0x0FF | (byte & 0b11) << 8;
    let low = |value: u16| value & 0x300 | byte;
    let mut next = match index {
        0 => Area {
            x0: high(area.x0),
            ..area
        },
        1 => Area {
            x0: area.x0 & 0x300 | (byte & 0xF8),
            ..area
        },
        2 => Area {
            x1: high(area.x1),
            ..area
        },
        3 => Area {
            x1: area.x1 & 0x300 | (byte & 0xF8) | 0b111,
            ..area
        },
        4 => Area {
            y0: high(area.y0),
            ..area
        },
        5 => Area {
            y0: low(area.y0),
            ..area
        },
        6 => Area {
            y1: high(area.y1),
            ..area
        },
        7 => Area {
            y1: low(area.y1),
            ..area
        },
        // `PT_SCAN`: whether the gates outside the window are scanned too; their sources carry no
        // LUT, so the glass outside the window does not change either way.
        _ => area,
    };
    next.x0 = next.x0.min(RAM_SOURCES - 1);
    next.x1 = next.x1.min(RAM_SOURCES - 1);
    next.y0 = next.y0.min(RAM_GATES - 1);
    next.y1 = next.y1.min(RAM_GATES - 1);
    next
}
