//! A device powered on with its chips on its pins: the machine a front end runs, and the handles
//! through which it reads the panel, presses the keys and takes the SD card back (design of
//! sub-project 2d, 2d-D9, 2d-D14, 2d-D23; design of sub-project 2e, §9).
//!
//! [`Wired::power_on`] reads the wiring of the profile ([`crate::wiring`]), builds each chip,
//! attaches it to the machine by its pins, then hands the machine the power-on gesture as dated
//! board inputs, which the journal keeps: a replay that gives the same inputs is the same run
//! (2d-R2). A profile without `[wiring]` gives the bare machine of [`DeviceProfile::instantiate`]
//! and no handle (2d-R4).
use crate::wiring::{Button, ButtonSource, Rotation, USB_POWER, Wiring};
use crate::{DeviceProfile, MachineOptions};
use chips::sdcard::{BlockSource, CapacityError, Overlay, SdCard, SdCardConfig, SdSlot};
use chips::{ButtonLadder, ConfigError, PushButton, SdLines, Ssd1677Lines};
use emu_core::{
    BoardInput, BoardInputError, BoardInputs, Cycles, Frequency, GpioInput, GpioOutput, Level,
    Nanos, Supply,
};
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::flash::FlashImageError;
use soc_esp32c3::{AttachError, Esp32C3, InputError, RomImage, SpiWires};
use std::cell::{Ref, RefCell, RefMut};
use std::num::NonZeroU64;
use std::rc::Rc;

/// The time base of the chips on the board, 1 GHz: an instant of the signals (`Nanos`) is an
/// instant of the chip (2d-D2).
const CHIP_TIME_BASE: Frequency = match NonZeroU64::new(1_000_000_000) {
    Some(hz) => Frequency::new(hz),
    None => unreachable!(),
};

/// The card of a wired slot: its content, read from a file or a session, under the overlay that
/// keeps what the firmware writes (ADR 0014).
pub type WiredCard = SdCard<Overlay<Box<dyn BlockSource>>>;

/// The card a life starts with.
#[derive(Debug)]
pub struct SdStart {
    /// What the card holds at power-on.
    pub source: Box<dyn BlockSource>,
    pub config: SdCardConfig,
}

/// What a front end chooses for one life of a wired device.
#[derive(Debug, Default)]
pub struct WiredOptions {
    /// How long the power key is held at power-on, in milliseconds: `None` the profile's
    /// `[power] hold_ms`, `Some(0)` not pressed (2d-D14).
    pub power_hold_ms: Option<u32>,
    /// A cable powers the device from power-on.
    pub usb_power: bool,
    /// The card in the slot at power-on; `None` an empty slot.
    pub sd: Option<SdStart>,
}

/// Why a device cannot be powered on with its wiring.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WiredError {
    #[error(transparent)]
    Flash(#[from] FlashImageError),
    #[error("`{key}`: {source}")]
    Attach {
        key: &'static str,
        source: AttachError,
    },
    #[error("the configuration of the display controller is refused: {0}")]
    Display(ConfigError),
    #[error("the SD card cannot be built: {0}")]
    SdCard(CapacityError),
    #[error(
        "{device} has no SD card slot in its profile; start it without a card, or wire `[wiring.sd]`"
    )]
    NoSdSlot { device: String },
    #[error(
        "{device} has no USB detect line in its profile; start it without the cable, or wire `[wiring.power]`"
    )]
    NoUsbDetect { device: String },
    #[error(
        "{device} has no power key in its profile; start it without a power-on hold, or name one in `[power]`"
    )]
    NoPowerKey { device: String },
    #[error("the power-on gesture is refused: {0}")]
    Gesture(InputError),
}

/// What the board holds that its inputs change and its supply depends on.
#[derive(Debug)]
struct BoardState {
    buttons: Vec<Button>,
    keys: Vec<Rc<RefCell<PushButton>>>,
    ladders: Vec<Rc<RefCell<ButtonLadder>>>,
    /// The power lines are wired: the cable is an input and the latch is watched.
    power_lines: bool,
    cable: bool,
    /// What the `SoC` drives on the latch.
    latch: Option<Level>,
    /// The target of the power key.
    power_key: Option<usize>,
}

impl BoardState {
    fn pressed(&self, target: usize) -> bool {
        match self.buttons.get(target).map(|button| button.source) {
            Some(ButtonSource::Key(index)) => self
                .keys
                .get(index)
                .is_some_and(|key| key.borrow().is_pressed()),
            Some(ButtonSource::Ladder { ladder, index }) => self
                .ladders
                .get(ladder)
                .and_then(|ladder| ladder.borrow().is_pressed(index))
                .unwrap_or(false),
            None => false,
        }
    }

    fn set_pressed(&mut self, target: usize, pressed: bool) {
        match self.buttons.get(target).map(|button| button.source) {
            Some(ButtonSource::Key(index)) => {
                if let Some(key) = self.keys.get(index) {
                    key.borrow_mut().set_pressed(pressed);
                }
            }
            Some(ButtonSource::Ladder { ladder, index }) => {
                if let Some(ladder) = self.ladders.get(ladder) {
                    // The index comes from the ladder's own buttons: it is always there.
                    let _ = ladder.borrow_mut().set_pressed(index, pressed);
                }
            }
            None => {}
        }
    }

    /// The target of the cable, past the buttons.
    fn cable_target(&self) -> Option<usize> {
        self.power_lines.then_some(self.buttons.len())
    }
}

/// The inputs and the supply of a wired board (2d-D9, 2d-D13): the buttons in the order of the
/// wiring, value 1 pressed and 0 released, then `usb-power`, 1 plugged in and 0 out. The supply
/// holds while the cable is in, while the latch is not driven low, or while the power key is held
/// (it bridges the battery to the rail).
#[derive(Debug)]
struct WiredBoard(Rc<RefCell<BoardState>>);

impl BoardInputs for WiredBoard {
    fn check(&self, input: BoardInput) -> Result<(), BoardInputError> {
        let state = self.0.borrow();
        let target = usize::from(input.target);
        if target >= state.buttons.len() && state.cable_target() != Some(target) {
            return Err(BoardInputError::UnknownTarget {
                target: input.target,
            });
        }
        if input.value > 1 {
            return Err(BoardInputError::Value {
                target: input.target,
                value: input.value,
            });
        }
        Ok(())
    }

    fn apply(&mut self, input: BoardInput, _at: Nanos) {
        let mut state = self.0.borrow_mut();
        let target = usize::from(input.target);
        if state.cable_target() == Some(target) {
            state.cable = input.value == 1;
        } else {
            state.set_pressed(target, input.value == 1);
        }
    }
}

impl Supply for WiredBoard {
    fn powered(&mut self, _at: Nanos) -> bool {
        let state = self.0.borrow();
        state.cable
            || state.latch != Some(Level::Low)
            || state.power_key.is_some_and(|key| state.pressed(key))
    }
}

/// The latch line, as the board hears what the `SoC` drives on it.
#[derive(Debug)]
struct Latch(Rc<RefCell<BoardState>>);

impl GpioOutput for Latch {
    fn driven(&mut self, level: Option<Level>, _at: Nanos) {
        self.0.borrow_mut().latch = level;
    }
}

/// The USB detect line: high while the cable powers the device, low without it.
/// ASSUMPTION: the board drives it low without a cable (a divider on VBUS; RW §4).
#[derive(Debug)]
struct UsbDetect(Rc<RefCell<BoardState>>);

impl GpioInput for UsbDetect {
    fn level(&mut self, _at: Nanos) -> Option<Level> {
        Some(Level::from_bool(self.0.borrow().cable))
    }
}

/// The display of a wired device: its controller on its lines.
#[derive(Clone, Debug)]
pub struct PanelHandle {
    lines: Rc<RefCell<Ssd1677Lines>>,
    rotation: Rotation,
}

impl PanelHandle {
    /// The controller and what its lines saw; borrow it between two runs of the machine.
    #[must_use]
    pub fn lines(&self) -> Ref<'_, Ssd1677Lines> {
        self.lines.borrow()
    }

    /// The controller, for what a front end takes from it (its diagnostics, its faults).
    #[must_use]
    pub fn lines_mut(&self) -> RefMut<'_, Ssd1677Lines> {
        self.lines.borrow_mut()
    }

    /// How the panel is turned in the case.
    #[must_use]
    pub fn rotation(&self) -> Rotation {
        self.rotation
    }
}

/// The buttons and the cable of a wired device, as a front end presses them: through
/// [`Esp32C3::send_board_input`], with the inputs this handle builds (2d-R2).
#[derive(Clone, Debug)]
pub struct ButtonsHandle {
    state: Rc<RefCell<BoardState>>,
    buttons: Vec<Button>,
}

impl ButtonsHandle {
    /// The buttons, in the order of their targets.
    #[must_use]
    pub fn buttons(&self) -> &[Button] {
        &self.buttons
    }

    /// The input that presses (`true`) or releases the button `id`; `None` for no such button.
    #[must_use]
    pub fn button_input(&self, id: &str, pressed: bool) -> Option<BoardInput> {
        let target = self.buttons.iter().position(|button| button.id == id)?;
        Some(BoardInput {
            target: u16::try_from(target).ok()?,
            value: u32::from(pressed),
        })
    }

    /// The input that plugs the cable in (`true`) or out; `None` without a USB detect line.
    #[must_use]
    pub fn cable_input(&self, plugged: bool) -> Option<BoardInput> {
        let target = self.state.borrow().cable_target()?;
        Some(BoardInput {
            target: u16::try_from(target).ok()?,
            value: u32::from(plugged),
        })
    }

    /// Whether the button `id` is held now, as the board's last applied input left it.
    #[must_use]
    pub fn is_pressed(&self, id: &str) -> Option<bool> {
        let target = self.buttons.iter().position(|button| button.id == id)?;
        Some(self.state.borrow().pressed(target))
    }

    /// Whether the cable is in now; `None` without a USB detect line.
    #[must_use]
    pub fn cable(&self) -> Option<bool> {
        let state = self.state.borrow();
        state.power_lines.then_some(state.cable)
    }

    /// The name of the target `target`: a button's id, or [`USB_POWER`].
    #[must_use]
    pub fn target_name(&self, target: u16) -> Option<&str> {
        let index = usize::from(target);
        match self.buttons.get(index) {
            Some(button) => Some(&button.id),
            None => (self.state.borrow().cable_target() == Some(index)).then_some(USB_POWER),
        }
    }
}

/// The SD card slot of a wired device, from which a front end takes the card at the end of a life
/// to keep what was written (ADR 0014).
#[derive(Clone, Debug)]
pub struct SdHandle {
    lines: Rc<RefCell<WiredSlot>>,
}

/// The slot of a wired device on its lines.
pub type WiredSlot = SdLines<Overlay<Box<dyn BlockSource>>>;

impl SdHandle {
    /// Whether a card is in the slot.
    #[must_use]
    pub fn has_card(&self) -> bool {
        self.lines.borrow().slot().card().is_some()
    }

    /// The card, taken out of the slot: what it holds now under [`SdCard::into_storage`].
    #[must_use]
    pub fn take_card(&self) -> Option<WiredCard> {
        self.lines.borrow_mut().slot_mut().take()
    }

    /// The slot and the card in it, for what a front end reads of it (its diagnostics).
    #[must_use]
    pub fn lines(&self) -> RefMut<'_, WiredSlot> {
        self.lines.borrow_mut()
    }
}

/// A device powered on with its wiring.
#[derive(Debug)]
pub struct Wired {
    pub machine: Esp32C3,
    /// The display; `None` for a bare device.
    pub panel: Option<PanelHandle>,
    /// The buttons and the cable; `None` for a bare device.
    pub buttons: Option<ButtonsHandle>,
    /// The SD card slot; `None` for a device without one.
    pub sd: Option<SdHandle>,
}

fn attached(key: &'static str, result: Result<(), AttachError>) -> Result<(), WiredError> {
    result.map_err(|source| WiredError::Attach { key, source })
}

impl Wired {
    /// Powers on the device of `profile` with `flash` (a blank flash for `None`), its chips on its
    /// pins, and gives the machine the power-on gesture: the power key pressed at 0 and released
    /// after the hold, and the cable plugged in at 0 when `wired.usb_power` asks (2d-D14).
    ///
    /// # Errors
    /// [`WiredError`]: an image longer than the flash, a card the slot cannot take, an option the
    /// device has no wiring for, or a chip the machine refuses, which the checks of the profile
    /// rule out.
    pub fn power_on(
        profile: &DeviceProfile,
        rom: RomImage,
        flash: Option<Vec<u8>>,
        options: MachineOptions,
        wired: WiredOptions,
    ) -> Result<Wired, WiredError> {
        let mut machine = match flash {
            Some(image) => profile.instantiate_with_flash(rom, image, options)?,
            None => profile.instantiate(rom, options),
        };
        let device = || profile.name.clone();
        let Some(wiring) = &profile.wiring else {
            if wired.sd.is_some() {
                return Err(WiredError::NoSdSlot { device: device() });
            }
            if wired.usb_power {
                return Err(WiredError::NoUsbDetect { device: device() });
            }
            if wired.power_hold_ms.is_some_and(|hold| hold > 0) {
                return Err(WiredError::NoPowerKey { device: device() });
            }
            return Ok(Wired {
                machine,
                panel: None,
                buttons: None,
                sd: None,
            });
        };
        if wired.sd.is_some() && wiring.sd.is_none() {
            return Err(WiredError::NoSdSlot { device: device() });
        }
        if wired.usb_power && wiring.power.is_none() {
            return Err(WiredError::NoUsbDetect { device: device() });
        }
        let hold_ms = match (&wiring.gesture, wired.power_hold_ms) {
            (_, Some(0)) | (None, None) => 0,
            (Some(gesture), None) => gesture.hold_ms,
            (Some(_), Some(hold)) => hold,
            (None, Some(_)) => return Err(WiredError::NoPowerKey { device: device() }),
        };
        let panel = attach_display(&mut machine, wiring)?;
        let sd = attach_sd(&mut machine, wiring, wired.sd)?;
        let state = attach_board(&mut machine, wiring)?;
        let buttons = ButtonsHandle {
            buttons: state.borrow().buttons.clone(),
            state: Rc::clone(&state),
        };
        give_the_gesture(&mut machine, wiring, &buttons, hold_ms, wired.usb_power)?;
        Ok(Wired {
            machine,
            panel: Some(panel),
            buttons: Some(buttons),
            sd,
        })
    }
}

fn attach_display(machine: &mut Esp32C3, wiring: &Wiring) -> Result<PanelHandle, WiredError> {
    let display = wiring.display;
    let lines = Rc::new(RefCell::new(
        Ssd1677Lines::new(display.config.ssd1677()).map_err(WiredError::Display)?,
    ));
    let wires = SpiWires {
        sclk: display.sclk,
        mosi: display.mosi,
        miso: None,
        cs: display.cs,
        dc: Some(display.dc),
    };
    attached(
        "wiring.display",
        machine.attach_spi(wires, Box::new(Rc::clone(&lines))),
    )?;
    let outputs = Ssd1677Lines::outputs(&lines);
    attached(
        "wiring.display.rst",
        machine.attach_output(display.rst, Box::new(outputs.reset)),
    )?;
    attached(
        "wiring.display.sclk",
        machine.attach_output(display.sclk, Box::new(outputs.clock)),
    )?;
    attached(
        "wiring.display.mosi",
        machine.attach_output(display.mosi, Box::new(outputs.data)),
    )?;
    attached(
        "wiring.display.dc",
        machine.attach_output(display.dc, Box::new(outputs.data_command)),
    )?;
    let inputs = Ssd1677Lines::inputs(&lines);
    attached(
        "wiring.display.busy",
        machine.attach_input(display.busy, Box::new(inputs.busy)),
    )?;
    attached(
        "wiring.display.mosi",
        machine.attach_input(display.mosi, Box::new(inputs.data)),
    )?;
    machine.attach_clocked(Box::new(Rc::clone(&lines)));
    Ok(PanelHandle {
        lines,
        rotation: display.rotation,
    })
}

fn attach_sd(
    machine: &mut Esp32C3,
    wiring: &Wiring,
    start: Option<SdStart>,
) -> Result<Option<SdHandle>, WiredError> {
    let Some(sd) = wiring.sd else {
        return Ok(None);
    };
    let slot = match start {
        None => SdSlot::empty(),
        Some(start) => SdSlot::holding(
            SdCard::new(start.config, Overlay::new(start.source), CHIP_TIME_BASE)
                .map_err(WiredError::SdCard)?,
        ),
    };
    let lines = Rc::new(RefCell::new(SdLines::new(slot)));
    let wires = SpiWires {
        sclk: wiring.display.sclk,
        mosi: wiring.display.mosi,
        miso: Some(sd.miso),
        cs: sd.cs,
        dc: None,
    };
    attached(
        "wiring.sd",
        machine.attach_spi(wires, Box::new(Rc::clone(&lines))),
    )?;
    Ok(Some(SdHandle { lines }))
}

fn attach_board(
    machine: &mut Esp32C3,
    wiring: &Wiring,
) -> Result<Rc<RefCell<BoardState>>, WiredError> {
    let mut keys = Vec::with_capacity(wiring.keys.len());
    for key in &wiring.keys {
        let button = Rc::new(RefCell::new(PushButton::new(key.active)));
        attached(
            "wiring.button",
            machine.attach_input(key.pin, Box::new(Rc::clone(&button))),
        )?;
        keys.push(button);
    }
    let mut ladders = Vec::with_capacity(wiring.ladders.len());
    for ladder in &wiring.ladders {
        let chip = Rc::new(RefCell::new(ladder.ladder.clone()));
        attached(
            "wiring.ladder",
            machine.attach_analog(ladder.pin, Box::new(Rc::clone(&chip))),
        )?;
        ladders.push(chip);
    }
    if let Some(battery) = wiring.battery {
        attached(
            "wiring.battery",
            machine.attach_analog(battery.pin, Box::new(battery.battery)),
        )?;
    }
    let buttons = wiring.buttons();
    let power_key = wiring
        .gesture
        .as_ref()
        .and_then(|gesture| buttons.iter().position(|button| button.id == gesture.key));
    let state = Rc::new(RefCell::new(BoardState {
        buttons,
        keys,
        ladders,
        power_lines: wiring.power.is_some(),
        cable: false,
        latch: None,
        power_key,
    }));
    machine.attach_board_inputs(Box::new(WiredBoard(Rc::clone(&state))));
    if let Some(power) = wiring.power {
        attached(
            "wiring.power.latch",
            machine.attach_output(power.latch, Box::new(Latch(Rc::clone(&state)))),
        )?;
        attached(
            "wiring.power.usb_detect",
            machine.attach_input(power.usb_detect, Box::new(UsbDetect(Rc::clone(&state)))),
        )?;
        // Last: the supply is asked at once, every line of the board in place.
        machine.attach_supply(Box::new(WiredBoard(Rc::clone(&state))));
    }
    Ok(state)
}

/// The power key pressed at 0 and released after `hold_ms`, and the cable plugged in at 0, as
/// journaled board inputs (2d-D14).
fn give_the_gesture(
    machine: &mut Esp32C3,
    wiring: &Wiring,
    buttons: &ButtonsHandle,
    hold_ms: u32,
    usb_power: bool,
) -> Result<(), WiredError> {
    let key = wiring
        .gesture
        .as_ref()
        .filter(|_| hold_ms > 0)
        .map(|gesture| gesture.key.as_str());
    let press = |pressed| key.and_then(|key| buttons.button_input(key, pressed));
    if let Some(input) = press(true) {
        machine
            .send_board_input(Cycles::ZERO, input)
            .map_err(WiredError::Gesture)?;
    }
    if usb_power && let Some(input) = buttons.cable_input(true) {
        machine
            .send_board_input(Cycles::ZERO, input)
            .map_err(WiredError::Gesture)?;
    }
    if let Some(input) = press(false) {
        let release = TIME_BASE.nanos_to_cycles(u128::from(hold_ms) * 1_000_000);
        machine
            .send_board_input(release, input)
            .map_err(WiredError::Gesture)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
