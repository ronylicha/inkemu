//! The GPIO matrix and the IO MUX: one model answers both blocks (ADR 0012, decision 2b-D3).
//!
//! It owns everything that decides what a pad does: the registers of the two blocks, the levels of
//! the output signals of the modelled peripherals (GP-SPI2's chip selects), and the chips the board
//! attaches to the pads. From them it works out, after every change, what the chip drives on each
//! pad, what each pad reads, which SPI chips are selected, and the GPIO interrupt (source 16).
//!
//! The GPIO registers reach it at their offset from `0x6000_4000`, the IO MUX registers at their
//! offset from the same base (`0x5000` and beyond): `Peripherals::device` routes both blocks here
//! with the base of GPIO.
mod pad;
#[cfg(test)]
mod tests;

pub(crate) use pad::FSPICS0;
pub use pad::{PADS, Signal};

use crate::clock::TIME_BASE;
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::Unsupported;
use crate::periph::{Accepted, GPIO_BASE, IO_MUX_BASE, Storage, consecutive, index_in};
use emu_core::{AnalogSource, Cycles, GpioInput, GpioOutput, Level, Nanos, Peripheral, SpiTarget};
use pad::{Drive, FUN_IE, Function, InputSource, PAD_DRIVER, PAD_MASK};
use thiserror::Error;

// The IO MUX follows GPIO in the address space: its registers reach this model past GPIO's.
const _: () = assert!(IO_MUX_BASE > GPIO_BASE + 0x1000);

/// Offset of a GPIO register from the base of GPIO; an unknown name fails the build.
const fn gpio(name: &str) -> u32 {
    svd_regs::esp32c3::addr("GPIO", name) - GPIO_BASE
}
/// Offset of an IO MUX register from the base of GPIO.
const fn io_mux(name: &str) -> u32 {
    svd_regs::esp32c3::addr("IO_MUX", name) - GPIO_BASE
}

const OUT: u32 = gpio("OUT");
const OUT_W1TS: u32 = gpio("OUT_W1TS");
const OUT_W1TC: u32 = gpio("OUT_W1TC");
const ENABLE: u32 = gpio("ENABLE");
const ENABLE_W1TS: u32 = gpio("ENABLE_W1TS");
const ENABLE_W1TC: u32 = gpio("ENABLE_W1TC");
/// `GPIO_STRAP_REG`: the strapping pins sampled at reset. Bit 3 set selects SPI flash boot (report
/// §D.3). Read-only.
const STRAP: u32 = gpio("STRAP");
const IN: u32 = gpio("IN");
const STATUS: u32 = gpio("STATUS");
const STATUS_W1TS: u32 = gpio("STATUS_W1TS");
const STATUS_W1TC: u32 = gpio("STATUS_W1TC");
const PCPU_INT: u32 = gpio("PCPU_INT");
const STATUS_NEXT: u32 = gpio("STATUS_NEXT");
const PIN0: u32 = gpio("PIN0");
const FUNC0_IN_SEL_CFG: u32 = gpio("FUNC0_IN_SEL_CFG");
const FUNC0_OUT_SEL_CFG: u32 = gpio("FUNC0_OUT_SEL_CFG");
const PAD0: u32 = io_mux("GPIO0");
const _: () = assert!(
    consecutive("GPIO", &["PIN0", "PIN1"])
        && consecutive("GPIO", &["PIN20", "PIN21"])
        && consecutive("GPIO", &["FUNC0_IN_SEL_CFG", "FUNC1_IN_SEL_CFG"])
        && consecutive("GPIO", &["FUNC126_IN_SEL_CFG", "FUNC127_IN_SEL_CFG"])
        && consecutive("GPIO", &["FUNC0_OUT_SEL_CFG", "FUNC1_OUT_SEL_CFG"])
        && consecutive("GPIO", &["FUNC20_OUT_SEL_CFG", "FUNC21_OUT_SEL_CFG"])
        && consecutive("IO_MUX", &["PIN_CTRL", "GPIO0", "GPIO1"])
        && consecutive("IO_MUX", &["GPIO20", "GPIO21"])
);

/// `GPIO_FUNCm_IN_SEL_CFG` exists for the 128 input signals.
const IN_SIGNALS: usize = 128;
/// `OUT`, `ENABLE`, `STATUS` and `IN` are 26-bit fields (`DATA_ORIG`, `DATA_NEXT`, `INTERRUPT`),
/// of which the low 22 are pads (TRM §5.5.3).
const FIELD_26: u32 = 0x03FF_FFFF;
/// `GPIO_FUNCm_IN_SEL_CFG` `[6:0]` and `GPIO_FUNCn_OUT_SEL_CFG` `[10:0]`.
const IN_SEL_FIELDS: u32 = 0x7F;
const OUT_SEL_FIELDS: u32 = 0x7FF;
/// `GPIO_PINn` `INT_TYPE` `[9:7]` and `INT_ENA` `[17:13]` (`gpio_reg.h:176-194`).
const INT_TYPE_SHIFT: u32 = 7;
const INT_TYPE_MASK: u32 = 0x7;
const INT_ENA_SHIFT: u32 = 13;
const INT_ENA_MASK: u32 = 0x1F;
/// `INT_ENA` bit 0, the interrupt of the CPU (`GPIO_LL_PRO_CPU_INTR_ENA`, `gpio_ll.h:35`).
const CPU_INTERRUPT: u32 = 1;

/// Source 16, `GPIO_PROCPU_INTR` (TRM table 8.3-1).
const SOURCE: Source = Source::of("GPIO_INTERRUPT_PRO_MAP");

/// The voltage of a pad driven, or pulled, high, for a reading of the SAR ADC on a pad no analog
/// chip is attached to.
///
/// ASSUMPTION: the pads' supply is 3.3 V, the X4's. A board with another supply would need it
/// from its profile.
pub(crate) const PAD_SUPPLY_MV: u32 = 3_300;

/// The IO MUX registers stored without effect beyond what the pads take from them: `PIN_CTRL` and
/// the 22 pads, then `DATE` (report BC §A.2). The function, the pulls and the input enable of a pad
/// act (2b-D4); its drive strength, its glitch filter and its sleep settings do not, nor do the
/// clock outputs of `PIN_CTRL`.
pub(crate) fn accepted() -> Vec<Accepted> {
    vec![
        Accepted::svd(
            "IO_MUX",
            "PIN_CTRL",
            "clock outputs (PIN_CTRL); of each pad, the drive strength, the input filter and the sleep settings (the function, the pulls and the input enable act)",
            "SP2d",
        )
        .block(1 + u32::try_from(PADS).unwrap_or(0)),
        Accepted::svd(
            "IO_MUX",
            "DATE",
            "none: the version date of the block, stored as written",
            "SP2d",
        ),
    ]
}

/// What a chip on GP-SPI2 is attached by: the pins of its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpiWires {
    /// The clock: the pin the matrix sends FSPICLK to.
    pub sclk: u8,
    /// Master out: the pin the matrix sends FSPID to.
    pub mosi: u8,
    /// Master in: the pin FSPIQ is read from, `None` for a chip that never answers.
    pub miso: Option<u8>,
    /// The chip select, active low: a GPIO the firmware writes, or the pin of FSPICS0 to FSPICS5.
    pub cs: u8,
    /// The data/command line, `None` for a chip that has none.
    pub dc: Option<u8>,
}

/// Why a chip could not be attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum AttachError {
    #[error(
        "GPIO{pin} does not exist: the ESP32-C3 has GPIO0 to GPIO21; check the pin of the wiring"
    )]
    NoSuchPin { pin: u8 },
    #[error(
        "GPIO{pin} already has a chip driving it: one chip drives an input pin; attach the others as outputs, or wire them to another pin"
    )]
    InputTaken { pin: u8 },
    #[error(
        "GPIO{pin} has no ADC channel: the ESP32-C3 converts GPIO0 to GPIO4 (ADC1) and GPIO5 (ADC2); wire the analog chip to one of them"
    )]
    NoAnalogChannel { pin: u8 },
    #[error(
        "GPIO{pin} already has an analog chip: one voltage per pin; wire the other chip to another pin"
    )]
    AnalogTaken { pin: u8 },
}

/// A chip on GP-SPI2, with what it was last told of its chip select.
#[derive(Debug)]
struct SpiChip {
    wires: SpiWires,
    target: Box<dyn SpiTarget>,
    selected: bool,
}

/// The chips attached to the pads. They survive every reset: they are the board, not the chip.
#[derive(Debug)]
struct Chips {
    /// The chip driving each pad, by pad.
    inputs: Vec<Option<Box<dyn GpioInput>>>,
    outputs: Vec<(usize, Box<dyn GpioOutput>)>,
    spi: Vec<SpiChip>,
    analog: Vec<(usize, Box<dyn AnalogSource>)>,
}

/// The registers of the two blocks the pads are decided by.
#[derive(Clone, Debug)]
struct Registers {
    out: u32,
    enable: u32,
    status: u32,
    pin: [u32; PADS],
    in_sel: [u32; IN_SIGNALS],
    out_sel: [u32; PADS],
    pads: [u32; PADS],
    /// `IO_MUX.PIN_CTRL` and `IO_MUX.DATE`.
    stored: Storage,
    /// The level of each peripheral output signal the model drives, bit `s` for signal `s`.
    signal_high: u128,
}

impl Registers {
    /// The values after a reset: the SVD's (`0x80` in each output selector, `0xB00` in each pad:
    /// the input enabled, the pull-up on, drive strength 2).
    ///
    /// ASSUMPTION: every pad resets to the SVD's `0xB00`, where TRM table 5.12-1 gives some of them
    /// no pull-up (GPIO2 to GPIO8, GPIO10) or no input (GPIO0, GPIO1, GPIO11, GPIO18, GPIO19). The
    /// lockstep compares these registers with what the ROM and ESP-IDF write (row 11 of
    /// `docs/lockstep-differences.md`); only the reading of a pad no firmware configured depends
    /// on it. A read of `IO_MUX_GPIOn` on a chip right after reset settles it.
    fn reset() -> Registers {
        let reset_of = |offset: u32| {
            svd_regs::esp32c3::lookup(GPIO_BASE + offset).map_or(0, |r| r.reset_value)
        };
        let mut signal_high = 0u128;
        for signal in 0..IN_SIGNALS {
            let signal = u32::try_from(signal).unwrap_or(0);
            if pad::rest(signal).is_some_and(|rest| rest.level.is_high()) {
                signal_high |= 1 << signal;
            }
        }
        Registers {
            out: 0,
            enable: 0,
            status: 0,
            pin: [0; PADS],
            in_sel: [0; IN_SIGNALS],
            out_sel: std::array::from_fn(|n| reset_of(FUNC0_OUT_SEL_CFG + word(n))),
            pads: std::array::from_fn(|n| reset_of(PAD0 + word(n))),
            stored: Storage::for_offsets(GPIO_BASE, &[io_mux("PIN_CTRL"), io_mux("DATE")]),
            signal_high,
        }
    }
}

/// The offset of the `n`-th register of a block of consecutive registers.
fn word(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(0) * svd_regs::REGISTER_BYTES
}

/// Device time as the chips count it.
fn nanos(ctx: &Ctx) -> Nanos {
    Nanos(TIME_BASE.cycles_to_nanos(ctx.now()))
}

/// The index of pad `pin`, if the chip has it.
fn pad_of(pin: u8) -> Result<usize, AttachError> {
    let pad = usize::from(pin);
    if pad < PADS {
        Ok(pad)
    } else {
        Err(AttachError::NoSuchPin { pin })
    }
}

/// The GPIO matrix and the IO MUX, with the chips of the board.
#[derive(Debug)]
pub struct IoMatrix {
    /// The strapping pins of the device, latched at power-on (`GPIO.STRAP`).
    strapping: u32,
    regs: Registers,
    /// The level of every pad at the last evaluation, bit `n` for pad `n`.
    levels: u32,
    /// `GPIO.IN` at the last evaluation: the levels of the pads whose input is enabled.
    input: u32,
    /// What the chip drives on each pad, as the attached chips last heard it.
    driven: [Option<Level>; PADS],
    chips: Chips,
    /// The wake-up at the instant an attached chip said its level may change.
    wake: Deadline,
}

impl IoMatrix {
    /// The pads after a power-on, with no chip attached.
    #[must_use]
    pub fn new(strapping: u32) -> IoMatrix {
        let mut io = IoMatrix {
            strapping,
            regs: Registers::reset(),
            levels: 0,
            input: 0,
            driven: [None; PADS],
            chips: Chips {
                inputs: (0..PADS).map(|_| None).collect(),
                outputs: Vec::new(),
                spi: Vec::new(),
                analog: Vec::new(),
            },
            wake: Deadline::default(),
        };
        for pad in 0..PADS {
            io.driven[pad] = io.drive_of(pad);
            let level = pad::level(io.driven[pad], None, io.regs.pads[pad]);
            if level.is_high() {
                io.levels |= 1 << pad;
            }
        }
        io.input = io.levels & io.input_enabled();
        io
    }

    /// A reset of the digital domain: the registers of both blocks and the peripheral signals
    /// return to their reset values; the chips stay, and hear what changes on their lines.
    pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
        self.regs = Registers::reset();
        self.settle(ctx);
    }

    /// The wake-up an attached chip asked for has come: its level is read again.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.wake.called(ctx.now());
        self.settle(ctx);
    }

    /// A peripheral drives one of its output signals (GP-SPI2's chip selects): the pads it is
    /// routed to change, and the chips on them hear it.
    pub(crate) fn set_signal(&mut self, ctx: &mut Ctx, signal: Signal, level: Level) {
        if pad::rest(signal).is_none() {
            return;
        }
        let bit = 1u128 << signal;
        let high = if level.is_high() {
            self.regs.signal_high | bit
        } else {
            self.regs.signal_high & !bit
        };
        if high != self.regs.signal_high {
            self.regs.signal_high = high;
            self.settle(ctx);
        }
    }

    /// One transfer of GP-SPI2, at the present time: `mosi` goes to every selected chip whose
    /// clock and MOSI pins are the pins FSPICLK and FSPID are routed to; the bytes heard are those
    /// of the chips whose MISO pin is the pin FSPIQ is read from, else the line's rest level. The
    /// pads are evaluated again afterwards: a chip may have changed a line it drives.
    ///
    /// ASSUMPTION: when two selected chips answer on the same MISO pin, the line carries the AND of
    /// what they drive, as two open-drain drivers would; on a real board it is a fault of the
    /// firmware (P, open question 6 for the rest level).
    pub(crate) fn spi_transfer(&mut self, ctx: &mut Ctx, mosi: &[u8]) -> Vec<u8> {
        let at = nanos(ctx);
        let clocks = self.pads_carrying(pad::FSPICLK);
        let data = self.pads_carrying(pad::FSPID);
        let (miso_pad, rest) = match self.input_of(pad::FSPIQ) {
            InputSource::Pad { pad, inverted } => (
                Some(pad),
                Level::from_bool((self.levels >> pad) & 1 != 0) != Level::from_bool(inverted),
            ),
            InputSource::Constant(level) => (None, level.is_high()),
        };
        let rest_byte = if rest { 0xFF } else { 0x00 };
        let levels = self.levels;
        let mut heard: Option<Vec<u8>> = None;
        for chip in &mut self.chips.spi {
            let on = |pin: u8, mask: u32| mask & (1 << pin) != 0;
            if !chip.selected || !on(chip.wires.sclk, clocks) || !on(chip.wires.mosi, data) {
                continue;
            }
            let dc = chip.wires.dc.map(|pin| Level::from_bool(on(pin, levels)));
            let mut answer = vec![rest_byte; mosi.len()];
            chip.target.transfer(dc, mosi, &mut answer, at);
            if chip.wires.miso.map(usize::from) == miso_pad && miso_pad.is_some() {
                heard = Some(match heard {
                    None => answer,
                    Some(before) => before.iter().zip(&answer).map(|(a, b)| a & b).collect(),
                });
            }
        }
        self.settle(ctx);
        heard.unwrap_or_else(|| vec![rest_byte; mosi.len()])
    }

    /// The voltage on pad `pad` for the SAR ADC, in millivolts, at the present time: what the
    /// analog chip attached there says, else the pad's level (`PAD_SUPPLY_MV` high, 0 low).
    pub(crate) fn millivolts(&mut self, ctx: &mut Ctx, pad: usize) -> u32 {
        let at = nanos(ctx);
        if let Some((_, source)) = self.chips.analog.iter_mut().find(|(p, _)| *p == pad) {
            return source.millivolts(at);
        }
        self.settle(ctx);
        if (self.levels >> pad) & 1 != 0 {
            PAD_SUPPLY_MV
        } else {
            0
        }
    }

    /// Attaches a chip that drives pin `pin`.
    /// # Errors
    /// [`AttachError::NoSuchPin`], or [`AttachError::InputTaken`] when a chip already drives it.
    pub(crate) fn attach_input(
        &mut self,
        ctx: &mut Ctx,
        pin: u8,
        chip: Box<dyn GpioInput>,
    ) -> Result<(), AttachError> {
        let pad = pad_of(pin)?;
        let slot = &mut self.chips.inputs[pad];
        if slot.is_some() {
            return Err(AttachError::InputTaken { pin });
        }
        *slot = Some(chip);
        self.settle(ctx);
        Ok(())
    }

    /// Attaches a chip that watches pin `pin`; it hears at once what the chip drives there.
    /// # Errors
    /// [`AttachError::NoSuchPin`].
    pub(crate) fn attach_output(
        &mut self,
        ctx: &Ctx,
        pin: u8,
        mut chip: Box<dyn GpioOutput>,
    ) -> Result<(), AttachError> {
        let pad = pad_of(pin)?;
        chip.driven(self.driven[pad], nanos(ctx));
        self.chips.outputs.push((pad, chip));
        Ok(())
    }

    /// Attaches a chip to GP-SPI2 by its pins; it is told at once if its chip select is active.
    /// # Errors
    /// [`AttachError::NoSuchPin`] for any of its pins.
    pub(crate) fn attach_spi(
        &mut self,
        ctx: &mut Ctx,
        wires: SpiWires,
        target: Box<dyn SpiTarget>,
    ) -> Result<(), AttachError> {
        for pin in [
            Some(wires.sclk),
            Some(wires.mosi),
            wires.miso,
            Some(wires.cs),
            wires.dc,
        ]
        .into_iter()
        .flatten()
        {
            pad_of(pin)?;
        }
        self.chips.spi.push(SpiChip {
            wires,
            target,
            selected: false,
        });
        self.settle(ctx);
        Ok(())
    }

    /// Attaches a chip that puts a voltage on pin `pin`, one of the six with an ADC channel.
    /// # Errors
    /// [`AttachError::NoSuchPin`], [`AttachError::NoAnalogChannel`] or
    /// [`AttachError::AnalogTaken`].
    pub(crate) fn attach_analog(
        &mut self,
        pin: u8,
        source: Box<dyn AnalogSource>,
    ) -> Result<(), AttachError> {
        let pad = pad_of(pin)?;
        if pad > crate::periph::saradc::LAST_ADC_PAD {
            return Err(AttachError::NoAnalogChannel { pin });
        }
        if self.chips.analog.iter().any(|(p, _)| *p == pad) {
            return Err(AttachError::AnalogTaken { pin });
        }
        self.chips.analog.push((pad, source));
        Ok(())
    }

    /// Works out every pad again at the present time: what the chip drives (the chips watching a
    /// pad hear a change), what each pad reads (the chips driving one are asked), which SPI chips
    /// are selected, the interrupts the pads trigger and the wake-up for the next change a chip
    /// announced.
    pub(crate) fn settle(&mut self, ctx: &mut Ctx) {
        let at = nanos(ctx);
        let mut levels = 0;
        for pad in 0..PADS {
            let driven = self.drive_of(pad);
            if driven != self.driven[pad] {
                self.driven[pad] = driven;
                for (watched, chip) in &mut self.chips.outputs {
                    if *watched == pad {
                        chip.driven(driven, at);
                    }
                }
            }
            let external = self.chips.inputs[pad]
                .as_mut()
                .and_then(|chip| chip.level(at));
            if pad::level(driven, external, self.regs.pads[pad]).is_high() {
                levels |= 1 << pad;
            }
        }
        self.levels = levels;
        for chip in &mut self.chips.spi {
            let selected = levels & (1 << chip.wires.cs) == 0;
            if selected != chip.selected {
                chip.selected = selected;
                chip.target.select(selected, at);
            }
        }
        let input = levels & self.input_enabled();
        self.trigger(self.input, input);
        self.input = input;
        ctx.irq(SOURCE, self.regs.status & self.cpu_enabled() != 0);
        self.arm(ctx, at);
    }

    /// What the chip drives on pad `pad` now.
    fn drive_of(&self, pad: usize) -> Option<Level> {
        let gpio = Drive {
            level: Level::from_bool((self.regs.out >> pad) & 1 != 0),
            enable: (self.regs.enable >> pad) & 1 != 0,
        };
        pad::driven(
            pad::function(pad, self.regs.pads[pad]),
            self.regs.out_sel[pad],
            gpio,
            self.regs.pin[pad] & PAD_DRIVER != 0,
            |signal| self.signal(signal),
        )
    }

    /// The drive of a modelled peripheral output signal.
    fn signal(&self, signal: Signal) -> Option<Drive> {
        pad::rest(signal).map(|rest| Drive {
            level: Level::from_bool((self.regs.signal_high >> signal) & 1 != 0),
            enable: rest.enable,
        })
    }

    /// The pads the output signal `signal` reaches, through the matrix or straight from the IO MUX,
    /// and that are driven: one bit per pad.
    fn pads_carrying(&self, signal: Signal) -> u32 {
        (0..PADS)
            .filter(|&pad| {
                let routed = match pad::function(pad, self.regs.pads[pad]) {
                    Function::Matrix => self.regs.out_sel[pad] & pad::OUT_SEL_MASK == signal,
                    Function::Fspi(direct) => direct == signal,
                    Function::U0Txd | Function::Other => false,
                };
                routed && self.driven[pad].is_some()
            })
            .fold(0, |mask, pad| mask | (1 << pad))
    }

    /// Where input signal `signal` comes from.
    fn input_of(&self, signal: Signal) -> InputSource {
        let direct = (0..PADS).find(|&pad| {
            pad::function(pad, self.regs.pads[pad]) == Function::Fspi(signal)
                && signal == pad::FSPIQ
        });
        let index = usize::try_from(signal).unwrap_or(0);
        pad::input_source(self.regs.in_sel[index], direct)
    }

    /// The pads whose input is enabled (`FUN_IE`).
    fn input_enabled(&self) -> u32 {
        (0..PADS)
            .filter(|&pad| self.regs.pads[pad] & FUN_IE != 0)
            .fold(0, |mask, pad| mask | (1 << pad))
    }

    /// The pads whose interrupt goes to the CPU (`INT_ENA` bit 0).
    fn cpu_enabled(&self) -> u32 {
        (0..PADS)
            .filter(|&pad| (self.regs.pin[pad] >> INT_ENA_SHIFT) & CPU_INTERRUPT != 0)
            .fold(0, |mask, pad| mask | (1 << pad))
    }

    /// `GPIO_PINn.INT_TYPE` of pad `pad`.
    fn int_type(&self, pad: usize) -> u32 {
        (self.regs.pin[pad] >> INT_TYPE_SHIFT) & INT_TYPE_MASK
    }

    /// Sets the `STATUS` bit of every pad whose trigger fires between `before` and `now` (two
    /// samples of `GPIO.IN`).
    ///
    /// ASSUMPTION: a level trigger (types 4 and 5) sets `STATUS` at every evaluation while the
    /// level holds, so a bit software clears comes back at once if the level is still there; the
    /// TRM does not say (P, open question 1). Types 6 and 7 trigger nothing.
    fn trigger(&mut self, before: u32, now: u32) {
        for pad in 0..PADS {
            let was = (before >> pad) & 1 != 0;
            let is = (now >> pad) & 1 != 0;
            let fires = match self.int_type(pad) {
                1 => !was && is,
                2 => was && !is,
                3 => was != is,
                4 => !is,
                5 => is,
                _ => false,
            };
            if fires {
                self.regs.status |= 1 << pad;
            }
        }
    }

    /// `GPIO.STATUS_NEXT`: the pads whose trigger holds now.
    ///
    /// ASSUMPTION: only a level holds over time; an edge trigger shows here only in the evaluation
    /// it fires in, which a read never sees.
    fn status_next(&self) -> u32 {
        (0..PADS)
            .filter(|&pad| {
                let is = (self.input >> pad) & 1 != 0;
                match self.int_type(pad) {
                    4 => !is,
                    5 => is,
                    _ => false,
                }
            })
            .fold(0, |mask, pad| mask | (1 << pad))
    }

    /// Asks to be woken at the earliest instant a chip driving a pad with an interrupt said its
    /// level may change; no wake-up when no such pad has one.
    fn arm(&mut self, ctx: &mut Ctx, at: Nanos) {
        let mut next: Option<Nanos> = None;
        for pad in 0..PADS {
            if self.int_type(pad) == 0 {
                continue;
            }
            if let Some(chip) = self.chips.inputs[pad].as_mut()
                && let Some(change) = chip.next_change(at)
                && change > at
            {
                next = Some(next.map_or(change, |earlier| earlier.min(change)));
            }
        }
        match next {
            Some(instant) => {
                let cycles: Cycles = TIME_BASE.nanos_to_cycles(u128::from(instant.get()));
                self.wake.arm(ctx, cycles, Event::Pins);
            }
            None => self.wake.cancel(ctx),
        }
    }

    /// The register of a pad block at `offset`, if `offset` is in it.
    fn block(offset: u32, first: u32, count: usize) -> Option<usize> {
        index_in(offset, first, count)
    }
}

impl Peripheral<Ctx> for IoMatrix {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        let regs = &self.regs;
        let value = match offset {
            OUT => regs.out,
            ENABLE => regs.enable,
            // Write-only strobes read 0. ASSUMPTION: the TRM marks them WT and says nothing of a
            // read; ESP-IDF writes them through bit fields, which read them first.
            OUT_W1TS | OUT_W1TC | ENABLE_W1TS | ENABLE_W1TC | STATUS_W1TS | STATUS_W1TC => 0,
            STRAP => self.strapping,
            IN => {
                self.settle(ctx);
                self.input
            }
            STATUS => {
                self.settle(ctx);
                self.regs.status
            }
            PCPU_INT => {
                self.settle(ctx);
                self.regs.status & self.cpu_enabled()
            }
            STATUS_NEXT => {
                self.settle(ctx);
                self.status_next()
            }
            _ => {
                if let Some(n) = Self::block(offset, PIN0, PADS) {
                    regs.pin[n]
                } else if let Some(m) = Self::block(offset, FUNC0_IN_SEL_CFG, IN_SIGNALS) {
                    regs.in_sel[m]
                } else if let Some(n) = Self::block(offset, FUNC0_OUT_SEL_CFG, PADS) {
                    regs.out_sel[n]
                } else if let Some(n) = Self::block(offset, PAD0, PADS) {
                    regs.pads[n]
                } else {
                    return regs.stored.read(offset);
                }
            }
        };
        Some(value)
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        let regs = &mut self.regs;
        match offset {
            OUT => regs.out = value & FIELD_26,
            OUT_W1TS => regs.out |= value & FIELD_26,
            OUT_W1TC => regs.out &= !value,
            ENABLE => regs.enable = value & FIELD_26,
            ENABLE_W1TS => regs.enable |= value & FIELD_26,
            ENABLE_W1TC => regs.enable &= !value,
            STATUS => regs.status = value & PAD_MASK,
            STATUS_W1TS => regs.status |= value & PAD_MASK,
            STATUS_W1TC => regs.status &= !value,
            // `STRAP`, `IN`, `PCPU_INT` and `STATUS_NEXT` are read-only.
            STRAP | IN | PCPU_INT | STATUS_NEXT => return false,
            _ => {
                if let Some(n) = Self::block(offset, PIN0, PADS) {
                    let int_ena = (value >> INT_ENA_SHIFT) & INT_ENA_MASK;
                    if int_ena & !CPU_INTERRUPT != 0 {
                        ctx.unsupported(Unsupported::PadInterrupt {
                            pin: u8::try_from(n).unwrap_or(u8::MAX),
                            int_ena,
                        });
                    }
                    regs.pin[n] = value;
                } else if let Some(m) = Self::block(offset, FUNC0_IN_SEL_CFG, IN_SIGNALS) {
                    regs.in_sel[m] = value & IN_SEL_FIELDS;
                } else if let Some(n) = Self::block(offset, FUNC0_OUT_SEL_CFG, PADS) {
                    let signal = value & pad::OUT_SEL_MASK;
                    if signal != pad::GPIO_OUT_SIGNAL && pad::rest(signal).is_none() {
                        ctx.unsupported(Unsupported::PadSignal {
                            pin: u8::try_from(n).unwrap_or(u8::MAX),
                            signal,
                        });
                    }
                    regs.out_sel[n] = value & OUT_SEL_FIELDS;
                } else if let Some(n) = Self::block(offset, PAD0, PADS) {
                    regs.pads[n] = value;
                } else if !regs.stored.write(offset, value) {
                    return false;
                }
            }
        }
        self.settle(ctx);
        true
    }
}
