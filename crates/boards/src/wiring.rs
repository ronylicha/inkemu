//! The wiring of a device: which chip hangs on which pin of the `SoC`, the keys of its user and where
//! they sit on its case, and how its panel is turned (design of sub-project 2d, 2d-D9, 2d-D14,
//! 2d-D15; `docs/research/sp2d-x4-wiring.md` §2).
//!
//! A profile says it in `[wiring]` and `[power]`; this module reads those tables, refuses what
//! cannot be wired, naming the key, and gives the rest typed. Attaching it to a machine is the work
//! of [`crate::wired`]. A profile without `[wiring]` is a bare `SoC`. The X3 (design of SP3, ADR
//! 0015) adds an I2C bus with its three chips (`[wiring.i2c]`), a UC8253 controller and the switched
//! supply of its card.
use chips::{
    Battery, Bq27220Config, ButtonLadder, DateTime, Ds3231Config, LadderButton, Millivolts,
    Qmi8658Config, Ssd1677Config, Uc8253Config,
};
use emu_core::Level;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

/// The last GPIO of the ESP32-C3: it has GPIO0 to GPIO21.
const LAST_GPIO: u8 = 21;
/// The last pin of ADC1, which converts GPIO0 to GPIO4. ADC2 (GPIO5) is left out: ESP-IDF refuses
/// it while the radio runs, and no firmware of the family reads it.
const LAST_ADC1_PIN: u8 = 4;
/// The name of the target that plugs the power cable in (2d-D9): no button may take it.
pub const USB_POWER: &str = "usb-power";

/// The side of the case a key sits on, the case seen from the front with the panel upright.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

impl Edge {
    /// The name the profile gives the edge.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Edge::Top => "top",
            Edge::Right => "right",
            Edge::Bottom => "bottom",
            Edge::Left => "left",
        }
    }
}

impl fmt::Display for Edge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Where a key sits: its edge, then its rank along it (left to right, top to bottom), from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    pub edge: Edge,
    pub order: u8,
}

/// The clockwise rotation that brings the controller's scan upright in the case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rotation {
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    /// The rotation in degrees, clockwise.
    #[must_use]
    pub const fn degrees(self) -> u16 {
        match self {
            Rotation::Deg0 => 0,
            Rotation::Deg90 => 90,
            Rotation::Deg180 => 180,
            Rotation::Deg270 => 270,
        }
    }

    const fn from_degrees(degrees: u16) -> Option<Rotation> {
        match degrees {
            0 => Some(Rotation::Deg0),
            90 => Some(Rotation::Deg90),
            180 => Some(Rotation::Deg180),
            270 => Some(Rotation::Deg270),
            _ => None,
        }
    }
}

/// A display controller inkemu models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Controller {
    /// The X4's.
    Ssd1677,
    /// The X3's, in either of its batches ([`chips::uc8253::Variant`]).
    Uc8253,
}

/// A named configuration of a controller: the panel behind it and how the model drives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelConfig {
    /// The X4's panel ([`Ssd1677Config::gdeq0426t82`]).
    Gdeq0426t82,
    /// The X3's panel behind the first batch's controller ([`Uc8253Config::xteink_x3`]).
    XteinkX3,
    /// The X3's panel behind the later batch's `UC8279d` ([`Uc8253Config::xteink_x3_uc8279d`]).
    XteinkX3Uc8279d,
}

/// The configuration of a controller, of its own type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControllerConfig {
    Ssd1677(Ssd1677Config),
    Uc8253(Uc8253Config),
}

impl PanelConfig {
    /// The configuration of the controller.
    #[must_use]
    pub fn controller(self) -> ControllerConfig {
        match self {
            PanelConfig::Gdeq0426t82 => ControllerConfig::Ssd1677(Ssd1677Config::gdeq0426t82()),
            PanelConfig::XteinkX3 => ControllerConfig::Uc8253(Uc8253Config::xteink_x3()),
            PanelConfig::XteinkX3Uc8279d => {
                ControllerConfig::Uc8253(Uc8253Config::xteink_x3_uc8279d())
            }
        }
    }

    /// The glass, in pixels: its width (the controller's sources) and height (its gates).
    #[must_use]
    pub fn size(self) -> (u16, u16) {
        match self.controller() {
            ControllerConfig::Ssd1677(config) => (config.panel.width, config.panel.height),
            ControllerConfig::Uc8253(config) => (config.panel.sources, config.panel.gates),
        }
    }

    /// Whether what the model shows rests on parameters nobody measured: an interface says
    /// "approximation" while it is true.
    #[must_use]
    pub fn is_approximation(self) -> bool {
        match self.controller() {
            ControllerConfig::Ssd1677(config) => config.is_approximation(),
            ControllerConfig::Uc8253(config) => config.is_approximation(),
        }
    }
}

/// The display controller and its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayWiring {
    pub controller: Controller,
    pub config: PanelConfig,
    pub sclk: u8,
    /// The data line: written by the `SoC`, and read back from the controller after a read
    /// command when the firmware turns the pin into an input (the X4's bit-banged reads).
    pub mosi: u8,
    pub cs: u8,
    pub dc: u8,
    pub rst: u8,
    pub busy: u8,
    pub rotation: Rotation,
    /// A resistor of the board pulls the data line up: released by the `SoC` and by the
    /// controller, it reads high (the X3, whose `UC8253` drives nothing on a read).
    pub mosi_pull_up: bool,
}

/// Keys that pull one ADC pin to a level each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LadderWiring {
    pub pin: u8,
    /// The ladder at rest, its keys in the order of the profile.
    pub ladder: ButtonLadder,
    /// The place of each key, in the order of [`ButtonLadder::buttons`].
    pub placements: Vec<Placement>,
}

/// A key on a pin of its own, which the `SoC` reads as a level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyWiring {
    pub id: String,
    pub pin: u8,
    /// The level the pin reads while the key is held; released, the key leaves the pin to its
    /// pull.
    pub active: Level,
    pub placement: Placement,
}

/// The battery, read through a divider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryWiring {
    pub pin: u8,
    pub battery: Battery,
}

/// The lines of the supply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerLines {
    /// The latch the firmware drives: low cuts the battery off; released or high, it holds it on.
    pub latch: u8,
    /// High while a cable powers the device.
    pub usb_detect: u8,
}

/// The SD card slot: on the display's SCLK and MOSI, with a select and a MISO of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SdWiring {
    pub cs: u8,
    pub miso: u8,
    /// The pin that switches the card's supply, high on (the X3's GPIO13); `None` for a card always
    /// powered (the X4's).
    pub power: Option<u8>,
}

/// The I2C bus and the chips on it (ADR 0015): the X3's gauge, clock and motion sensor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct I2cWiring {
    pub scl: u8,
    pub sda: u8,
    /// Resistors of the board pull SCL and SDA up: a line nobody drives reads high, whatever the
    /// pads' own pulls. Every profile says it.
    pub pull_up: bool,
    /// A BQ27220 fuel gauge, at `0x55`.
    pub gauge: Option<Bq27220Config>,
    /// A DS3231 real-time clock, at `0x68`.
    pub clock: Option<Ds3231Config>,
    /// A QMI8658 motion sensor, at the address its configuration names.
    pub motion: Option<Qmi8658Config>,
}

/// The gesture that turns the device on (2d-D14): the key and how long it is held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerGesture {
    pub key: String,
    pub hold_ms: u32,
}

/// Where a button is wired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSource {
    /// The key of [`Wiring::keys`] at this index.
    Key(usize),
    /// The key `index` of the ladder `ladder` of [`Wiring::ladders`].
    Ladder { ladder: usize, index: usize },
}

/// A button as a front end lists it and as the board numbers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub id: String,
    pub placement: Placement,
    pub source: ButtonSource,
}

/// The wiring of a device, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wiring {
    pub display: DisplayWiring,
    /// The ladders, in the order of the profile.
    pub ladders: Vec<LadderWiring>,
    /// The keys of `[wiring.button]`, in the order of their ids.
    pub keys: Vec<KeyWiring>,
    pub battery: Option<BatteryWiring>,
    pub power: Option<PowerLines>,
    pub sd: Option<SdWiring>,
    pub i2c: Option<I2cWiring>,
    /// The power-on gesture of `[power]`, when the device has a power key.
    pub gesture: Option<PowerGesture>,
}

impl Wiring {
    /// Every button, in the order of their targets (2d-D9): the keys of `[wiring.button]`, then
    /// the keys of each ladder in the order of the profile.
    #[must_use]
    pub fn buttons(&self) -> Vec<Button> {
        let keys = self.keys.iter().enumerate().map(|(index, key)| Button {
            id: key.id.clone(),
            placement: key.placement,
            source: ButtonSource::Key(index),
        });
        let ladders = self
            .ladders
            .iter()
            .enumerate()
            .flat_map(|(ladder, wiring)| {
                wiring
                    .ladder
                    .buttons()
                    .iter()
                    .zip(&wiring.placements)
                    .enumerate()
                    .map(move |(index, (button, &placement))| Button {
                        id: button.name.clone(),
                        placement,
                        source: ButtonSource::Ladder { ladder, index },
                    })
            });
        keys.chain(ladders).collect()
    }

    /// The target of the button `id`, or of [`USB_POWER`] when the device has a USB detect line.
    #[must_use]
    pub fn target_of(&self, id: &str) -> Option<u16> {
        let buttons = self.buttons();
        if id == USB_POWER {
            return self.power.and_then(|_| u16::try_from(buttons.len()).ok());
        }
        buttons
            .iter()
            .position(|button| button.id == id)
            .and_then(|position| u16::try_from(position).ok())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWiring {
    display: RawDisplay,
    #[serde(default)]
    ladder: Vec<RawLadder>,
    #[serde(default)]
    button: BTreeMap<String, RawKey>,
    battery: Option<RawBattery>,
    power: Option<RawPowerLines>,
    sd: Option<RawSd>,
    i2c: Option<RawI2c>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDisplay {
    controller: String,
    config: String,
    sclk: u8,
    mosi: u8,
    cs: u8,
    dc: u8,
    rst: u8,
    busy: u8,
    rotation: u16,
    #[serde(default)]
    mosi_pull_up: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLadder {
    pin: u8,
    idle_mv: u32,
    buttons: Vec<RawLadderKey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLadderKey {
    id: String,
    mv: u32,
    edge: Edge,
    order: u8,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawLevel {
    Low,
    High,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKey {
    pin: u8,
    active: RawLevel,
    edge: Edge,
    order: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBattery {
    pin: u8,
    top_ohms: u32,
    bottom_ohms: u32,
    cell_mv: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPowerLines {
    latch: u8,
    usb_detect: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSd {
    cs: u8,
    miso: u8,
    power: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawI2c {
    scl: u8,
    sda: u8,
    pull_up: bool,
    bq27220: Option<RawGauge>,
    ds3231: Option<RawClock>,
    qmi8658: Option<RawMotion>,
}

/// What a profile says of a BQ27220: the values a battery at rest reads; the gauge derives its
/// other registers from them (`Bq27220Config`'s defaults).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGauge {
    voltage_mv: u16,
    state_of_charge: u16,
    current_ma: i16,
}

/// What a profile says of a DS3231: the time it shows at power-on, as `YYYY-MM-DDTHH:MM:SS`, and
/// whether its oscillator-stop flag is set (a clock never set).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClock {
    start: String,
    oscillator_stopped: bool,
}

/// What a profile says of a QMI8658: its address and the acceleration it feels at rest, in
/// thousandths of g along its X, Y and Z axes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMotion {
    address: u8,
    acceleration_mg: [i32; 3],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPower {
    key: String,
    hold_ms: u32,
}

/// The pins taken so far, each with the key that took it.
#[derive(Default)]
struct Pins {
    taken: Vec<(u8, String)>,
}

impl Pins {
    fn take(&mut self, pin: u8, key: String) -> Result<(), String> {
        if pin > LAST_GPIO {
            return Err(format!(
                "`{key}` = {pin}: GPIO{pin} does not exist on the ESP32-C3, which has GPIO0 to GPIO{LAST_GPIO}; wire the line to one of them"
            ));
        }
        if let Some((_, other)) = self.taken.iter().find(|(taken, _)| *taken == pin) {
            return Err(format!(
                "`{key}` = {pin}: GPIO{pin} is already `{other}`; a pin carries one line (only the SD card shares the display's SCLK and MOSI); wire it to a free pin"
            ));
        }
        self.taken.push((pin, key));
        Ok(())
    }

    fn take_analog(&mut self, pin: u8, key: String) -> Result<(), String> {
        if pin <= LAST_GPIO && pin > LAST_ADC1_PIN {
            return Err(format!(
                "`{key}` = {pin}: GPIO{pin} is not a channel of ADC1, which converts GPIO0 to GPIO{LAST_ADC1_PIN}; wire the analog line to one of them"
            ));
        }
        self.take(pin, key)
    }
}

/// The buttons named so far, with their key and place.
#[derive(Default)]
struct Names {
    seen: Vec<(String, Placement, String)>,
}

impl Names {
    fn add(&mut self, id: &str, placement: Placement, key: String) -> Result<(), String> {
        let well_formed = !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !well_formed || id == USB_POWER {
            return Err(format!(
                "`{key}` = \"{id}\": a button id is lower-case letters, digits and `-`, other than `{USB_POWER}`, as a script names it (`press <id>`); rename the button"
            ));
        }
        if let Some((_, _, other)) = self.seen.iter().find(|(seen, _, _)| seen == id) {
            return Err(format!(
                "`{key}` = \"{id}\": `{other}` names a button `{id}` already; give each button its own id"
            ));
        }
        if let Some((other, _, _)) = self.seen.iter().find(|(_, at, _)| *at == placement) {
            return Err(format!(
                "`{key}` = \"{id}\": the {} edge has `{other}` at order {} already; give each button its own place",
                placement.edge, placement.order
            ));
        }
        self.seen.push((id.to_owned(), placement, key));
        Ok(())
    }
}

fn display(raw: &RawDisplay, pins: &mut Pins) -> Result<DisplayWiring, String> {
    let controller = match raw.controller.as_str() {
        "ssd1677" => Controller::Ssd1677,
        "uc8253" => Controller::Uc8253,
        other => {
            return Err(format!(
                "`wiring.display.controller` = \"{other}\": inkemu models no such controller; the controllers it models: ssd1677, uc8253"
            ));
        }
    };
    let config = match (controller, raw.config.as_str()) {
        (Controller::Ssd1677, "gdeq0426t82") => PanelConfig::Gdeq0426t82,
        (Controller::Uc8253, "xteink-x3") => PanelConfig::XteinkX3,
        (Controller::Uc8253, "xteink-x3-uc8279d") => PanelConfig::XteinkX3Uc8279d,
        (Controller::Ssd1677, other) => {
            return Err(format!(
                "`wiring.display.config` = \"{other}\": the SSD1677 has no such configuration; its configurations: gdeq0426t82"
            ));
        }
        (Controller::Uc8253, other) => {
            return Err(format!(
                "`wiring.display.config` = \"{other}\": the UC8253 has no such configuration; its configurations: xteink-x3, xteink-x3-uc8279d"
            ));
        }
    };
    let rotation = Rotation::from_degrees(raw.rotation).ok_or_else(|| {
        format!(
            "`wiring.display.rotation` = {}: a rotation is 0, 90, 180 or 270 degrees, clockwise",
            raw.rotation
        )
    })?;
    for (line, pin) in [
        ("sclk", raw.sclk),
        ("mosi", raw.mosi),
        ("cs", raw.cs),
        ("dc", raw.dc),
        ("rst", raw.rst),
        ("busy", raw.busy),
    ] {
        pins.take(pin, format!("wiring.display.{line}"))?;
    }
    Ok(DisplayWiring {
        controller,
        config,
        sclk: raw.sclk,
        mosi: raw.mosi,
        cs: raw.cs,
        dc: raw.dc,
        rst: raw.rst,
        busy: raw.busy,
        rotation,
        mosi_pull_up: raw.mosi_pull_up,
    })
}

fn ladder(
    at: usize,
    raw: RawLadder,
    pins: &mut Pins,
    names: &mut Names,
) -> Result<LadderWiring, String> {
    pins.take_analog(raw.pin, format!("wiring.ladder[{at}].pin"))?;
    let mut buttons = Vec::with_capacity(raw.buttons.len());
    let mut placements = Vec::with_capacity(raw.buttons.len());
    for (index, key) in raw.buttons.into_iter().enumerate() {
        let placement = Placement {
            edge: key.edge,
            order: key.order,
        };
        names.add(
            &key.id,
            placement,
            format!("wiring.ladder[{at}].buttons[{index}].id"),
        )?;
        placements.push(placement);
        buttons.push(LadderButton {
            name: key.id,
            level: Millivolts(key.mv),
        });
    }
    let ladder = ButtonLadder::new(Millivolts(raw.idle_mv), buttons).map_err(|error| {
        format!("`wiring.ladder[{at}]`: {error}; change the levels or the buttons of the ladder")
    })?;
    Ok(LadderWiring {
        pin: raw.pin,
        ladder,
        placements,
    })
}

/// The wiring and the power gesture of a profile, checked; `None` without `[wiring]`.
///
/// # Errors
/// What cannot be wired, as the `detail` of [`crate::ProfileError::Wiring`]: the key, the cause,
/// then the remedy.
pub(crate) fn check(
    raw: Option<RawWiring>,
    power: Option<RawPower>,
) -> Result<Option<Wiring>, String> {
    let Some(raw) = raw else {
        return match power {
            None => Ok(None),
            Some(_) => Err(
                "`[power]` names the power key of a wired device and the profile has no `[wiring]`; add the wiring of the device, or remove `[power]`"
                    .to_owned(),
            ),
        };
    };
    let mut pins = Pins::default();
    let mut names = Names::default();
    let display = display(&raw.display, &mut pins)?;
    let mut keys = Vec::with_capacity(raw.button.len());
    for (id, key) in raw.button {
        let placement = Placement {
            edge: key.edge,
            order: key.order,
        };
        names.add(&id, placement, format!("wiring.button.{id}"))?;
        pins.take(key.pin, format!("wiring.button.{id}.pin"))?;
        keys.push(KeyWiring {
            id,
            pin: key.pin,
            active: match key.active {
                RawLevel::Low => Level::Low,
                RawLevel::High => Level::High,
            },
            placement,
        });
    }
    let ladders = raw
        .ladder
        .into_iter()
        .enumerate()
        .map(|(at, raw)| ladder(at, raw, &mut pins, &mut names))
        .collect::<Result<Vec<_>, _>>()?;
    let battery = match raw.battery {
        None => None,
        Some(raw) => {
            pins.take_analog(raw.pin, "wiring.battery.pin".to_owned())?;
            let battery = Battery::new(Millivolts(raw.cell_mv), raw.top_ohms, raw.bottom_ohms)
                .map_err(|error| {
                    format!("`wiring.battery`: {error}; give the divider its bottom resistor")
                })?;
            Some(BatteryWiring {
                pin: raw.pin,
                battery,
            })
        }
    };
    let power_lines = match raw.power {
        None => None,
        Some(raw) => {
            pins.take(raw.latch, "wiring.power.latch".to_owned())?;
            pins.take(raw.usb_detect, "wiring.power.usb_detect".to_owned())?;
            Some(PowerLines {
                latch: raw.latch,
                usb_detect: raw.usb_detect,
            })
        }
    };
    let sd = raw.sd.map(|raw| sd(&raw, &mut pins)).transpose()?;
    let i2c = raw.i2c.map(|raw| i2c(raw, &mut pins)).transpose()?;
    let gesture = match power {
        None => None,
        Some(power) => {
            if !names.seen.iter().any(|(id, _, _)| *id == power.key) {
                let ids: Vec<&str> = names.seen.iter().map(|(id, _, _)| id.as_str()).collect();
                return Err(format!(
                    "`power.key` = \"{}\": no button of `[wiring]` has that id; name one of: {}",
                    power.key,
                    ids.join(", ")
                ));
            }
            Some(PowerGesture {
                key: power.key,
                hold_ms: power.hold_ms,
            })
        }
    };
    Ok(Some(Wiring {
        display,
        ladders,
        keys,
        battery,
        power: power_lines,
        sd,
        i2c,
        gesture,
    }))
}

/// The card slot of `[wiring.sd]`: its select, its MISO and the pin that switches its supply.
fn sd(raw: &RawSd, pins: &mut Pins) -> Result<SdWiring, String> {
    pins.take(raw.cs, "wiring.sd.cs".to_owned())?;
    pins.take(raw.miso, "wiring.sd.miso".to_owned())?;
    if let Some(power) = raw.power {
        pins.take(power, "wiring.sd.power".to_owned())?;
    }
    Ok(SdWiring {
        cs: raw.cs,
        miso: raw.miso,
        power: raw.power,
    })
}

/// The I2C bus of `[wiring.i2c]` and its chips, each checked as the chip itself would refuse it.
fn i2c(raw: RawI2c, pins: &mut Pins) -> Result<I2cWiring, String> {
    pins.take(raw.scl, "wiring.i2c.scl".to_owned())?;
    pins.take(raw.sda, "wiring.i2c.sda".to_owned())?;
    let gauge = raw
        .bq27220
        .map(|raw| {
            let config = Bq27220Config {
                voltage_mv: raw.voltage_mv,
                state_of_charge: raw.state_of_charge,
                current_ma: raw.current_ma,
                average_current_ma: raw.current_ma,
                ..Bq27220Config::default()
            };
            chips::Bq27220::new(config.clone())
                .map(|_| config)
                .map_err(|error| format!("`wiring.i2c.bq27220`: {error}"))
        })
        .transpose()?;
    let clock = raw
        .ds3231
        .map(|raw| {
            let start = date_time(&raw.start).ok_or_else(|| {
                format!(
                    "`wiring.i2c.ds3231.start` = \"{}\": a time is `YYYY-MM-DDTHH:MM:SS` between 2000-01-01T00:00:00 and 2199-12-31T23:59:59; write it so",
                    raw.start
                )
            })?;
            let config = Ds3231Config {
                start,
                oscillator_stopped: raw.oscillator_stopped,
                ..Ds3231Config::default()
            };
            chips::Ds3231::new(config)
                .map(|_| config)
                .map_err(|error| format!("`wiring.i2c.ds3231`: {error}"))
        })
        .transpose()?;
    let motion = raw
        .qmi8658
        .map(|raw| {
            let config = Qmi8658Config {
                address: raw.address,
                acceleration_mg: raw.acceleration_mg,
                ..Qmi8658Config::default()
            };
            chips::Qmi8658::new(config)
                .map(|_| config)
                .map_err(|error| format!("`wiring.i2c.qmi8658`: {error}"))
        })
        .transpose()?;
    Ok(I2cWiring {
        scl: raw.scl,
        sda: raw.sda,
        pull_up: raw.pull_up,
        gauge,
        clock,
        motion,
    })
}

/// `YYYY-MM-DDTHH:MM:SS`, a time a DS3231 can show; `None` for anything else.
fn date_time(text: &str) -> Option<DateTime> {
    let (date, time) = text.split_once('T')?;
    let numbers = |part: &str, separator: char| -> Option<Vec<u16>> {
        part.split(separator)
            .map(|field| {
                (field.len() >= 2 && field.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| field.parse().ok())
                    .flatten()
            })
            .collect()
    };
    let (date, time) = (numbers(date, '-')?, numbers(time, ':')?);
    let [year, month, day] = date.as_slice() else {
        return None;
    };
    let [hour, minute, second] = time.as_slice() else {
        return None;
    };
    let small = |value: u16| u8::try_from(value).ok();
    DateTime::new(
        *year,
        small(*month)?,
        small(*day)?,
        small(*hour)?,
        small(*minute)?,
        small(*second)?,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use crate::{DeviceProfile, ProfileError, builtin};

    use super::*;

    const BASE: &str = "id = \"demo\"\nname = \"Demo board\"\nconsole = \"uart0\"\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v1.1\"\n[flash]\nsize_mib = 4\njedec_id = 0xEF4016\n[[partition]]\nname = \"app\"\ntype = \"app\"\nsubtype = \"factory\"\noffset = 0x10000\nsize = 0x10000\n";

    const DISPLAY: &str = "[wiring.display]\ncontroller = \"ssd1677\"\nconfig = \"gdeq0426t82\"\nsclk = 8\nmosi = 10\ncs = 21\ndc = 4\nrst = 5\nbusy = 6\nrotation = 90\n";

    fn load(wiring: &str) -> Result<DeviceProfile, ProfileError> {
        DeviceProfile::from_toml("demo.toml", &format!("{BASE}{wiring}"))
    }

    /// The detail of the refusal of `wiring`, which must be one.
    fn refusal(wiring: &str) -> String {
        match load(wiring) {
            Err(ProfileError::Wiring { origin, detail }) => {
                assert_eq!(origin, "demo.toml");
                detail
            }
            other => panic!("not a wiring refusal: {other:?}"),
        }
    }

    #[test]
    fn a_profile_without_wiring_loads_bare() {
        assert_eq!(load("").unwrap().wiring, None);
    }

    /// ADR 0015: the X3 is wired as the research found it: the X4's display bus with its UC8253,
    /// the X4's ladders with the side keys on the left and right edges, the card's switched
    /// supply on GPIO13, and the three chips of its I2C bus on GPIO0 and GPIO20.
    #[test]
    fn the_x3_is_wired_as_the_research_found_it() {
        let wiring = builtin("x3").unwrap().wiring.unwrap();
        let display = wiring.display;
        assert_eq!(
            (display.controller, display.config, display.rotation),
            (Controller::Uc8253, PanelConfig::XteinkX3, Rotation::Deg90)
        );
        assert_eq!(
            (
                display.sclk,
                display.mosi,
                display.cs,
                display.dc,
                display.rst,
                display.busy
            ),
            (8, 10, 21, 4, 5, 6)
        );
        assert_eq!(display.config.size(), (792, 528));
        assert!(display.config.is_approximation());
        assert!(display.mosi_pull_up, "the data line reads 0xFF released");
        assert!(wiring.i2c.as_ref().unwrap().pull_up, "the bus is pulled up");
        let ids: Vec<String> = wiring.buttons().into_iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            ["power", "back", "confirm", "left", "right", "up", "down"]
        );
        let edges: Vec<Edge> = wiring.buttons().iter().map(|b| b.placement.edge).collect();
        assert_eq!(edges[5..], [Edge::Left, Edge::Right], "up, then down");
        assert_eq!(wiring.target_of(USB_POWER), None, "no cable line");
        assert_eq!((wiring.battery, wiring.power), (None, None));
        assert_eq!(
            wiring.sd,
            Some(SdWiring {
                cs: 12,
                miso: 7,
                power: Some(13)
            })
        );
        let i2c = wiring.i2c.unwrap();
        assert_eq!((i2c.scl, i2c.sda), (0, 20));
        let gauge = i2c.gauge.unwrap();
        assert_eq!(
            (gauge.voltage_mv, gauge.state_of_charge, gauge.current_ma),
            (3850, 70, -15)
        );
        let clock = i2c.clock.unwrap();
        assert_eq!(clock.start, DateTime::new(2026, 1, 1, 0, 0, 0).unwrap());
        assert!(!clock.oscillator_stopped);
        let motion = i2c.motion.unwrap();
        assert_eq!(
            (motion.address, motion.acceleration_mg),
            (0x6B, [0, 0, 1000])
        );
        assert_eq!(
            wiring.gesture,
            Some(PowerGesture {
                key: "power".to_owned(),
                hold_ms: 1500
            })
        );
    }

    const I2C: &str = "[wiring.i2c]\nscl = 0\nsda = 20\npull_up = true\n";

    #[test]
    fn the_i2c_chips_are_refused_as_the_chips_refuse_them_naming_the_key() {
        let detail = refusal(&format!(
            "{DISPLAY}{I2C}[wiring.i2c.bq27220]\nvoltage_mv = 3850\nstate_of_charge = 101\ncurrent_ma = 0\n"
        ));
        assert!(detail.starts_with("`wiring.i2c.bq27220`: "), "{detail}");
        let detail = refusal(&format!(
            "{DISPLAY}{I2C}[wiring.i2c.qmi8658]\naddress = 0x6C\nacceleration_mg = [0, 0, 1000]\n"
        ));
        assert!(detail.starts_with("`wiring.i2c.qmi8658`: "), "{detail}");
        for start in [
            "2026-13-01T00:00:00",
            "1999-12-31T23:59:59",
            "2026-01-01 00:00:00",
        ] {
            let detail = refusal(&format!(
                "{DISPLAY}{I2C}[wiring.i2c.ds3231]\nstart = \"{start}\"\noscillator_stopped = true\n"
            ));
            assert!(
                detail.starts_with(&format!("`wiring.i2c.ds3231.start` = \"{start}\"")),
                "{detail}"
            );
        }
        let detail = refusal(&format!(
            "{DISPLAY}[wiring.i2c]\nscl = 4\nsda = 20\npull_up = true\n"
        ));
        assert!(
            detail.starts_with("`wiring.i2c.scl` = 4: GPIO4 is already `wiring.display.dc`"),
            "{detail}"
        );
    }

    /// A bus says whether the board pulls it up: a profile cannot leave it out.
    #[test]
    fn an_i2c_bus_without_its_pull_ups_said_is_refused() {
        let error = load(&format!("{DISPLAY}[wiring.i2c]\nscl = 0\nsda = 20\n"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing field `pull_up`"), "{error}");
    }

    #[test]
    fn a_controller_takes_only_its_own_configurations() {
        let detail = refusal(&DISPLAY.replace("\"ssd1677\"", "\"uc8253\""));
        assert!(
            detail.starts_with(
                "`wiring.display.config` = \"gdeq0426t82\": the UC8253 has no such configuration"
            ),
            "{detail}"
        );
        let x3 = DISPLAY
            .replace("\"ssd1677\"", "\"uc8253\"")
            .replace("\"gdeq0426t82\"", "\"xteink-x3-uc8279d\"");
        let wiring = load(&x3).unwrap().wiring.unwrap();
        assert_eq!(wiring.display.config, PanelConfig::XteinkX3Uc8279d);
    }

    #[test]
    fn the_x4_is_wired_as_the_research_found_it() {
        let wiring = builtin("x4").unwrap().wiring.unwrap();
        let display = wiring.display;
        assert_eq!(
            (
                display.sclk,
                display.mosi,
                display.cs,
                display.dc,
                display.rst,
                display.busy
            ),
            (8, 10, 21, 4, 5, 6)
        );
        assert_eq!(display.rotation.degrees(), 90);
        assert_eq!((wiring.ladders[0].pin, wiring.ladders[1].pin), (1, 2));
        let battery = wiring.battery.unwrap();
        assert_eq!(
            (battery.pin, battery.battery.voltage()),
            (0, Millivolts(1950))
        );
        assert_eq!(
            wiring.power,
            Some(PowerLines {
                latch: 13,
                usb_detect: 20
            })
        );
        assert_eq!(
            wiring.sd,
            Some(SdWiring {
                cs: 12,
                miso: 7,
                power: None
            })
        );
        assert_eq!(wiring.i2c, None, "the X4 has no I2C chip");
        assert_eq!(
            wiring.gesture,
            Some(PowerGesture {
                key: "power".to_owned(),
                hold_ms: 1500
            })
        );
        let power = &wiring.keys[0];
        assert_eq!((power.pin, power.active), (3, Level::Low));
    }

    /// 2d-D9: the keys of `[wiring.button]`, then the ladders in the order of the file, then the
    /// cable.
    #[test]
    fn the_targets_of_the_x4_are_its_power_key_then_its_ladders_then_the_cable() {
        let wiring = builtin("x4").unwrap().wiring.unwrap();
        let ids: Vec<String> = wiring.buttons().into_iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            ["power", "back", "confirm", "left", "right", "up", "down"]
        );
        assert_eq!(wiring.target_of("power"), Some(0));
        assert_eq!(wiring.target_of("down"), Some(6));
        assert_eq!(wiring.target_of(USB_POWER), Some(7));
        assert_eq!(wiring.target_of("menu"), None);
        let right = &wiring.buttons()[4];
        assert_eq!(
            (right.placement, right.source),
            (
                Placement {
                    edge: Edge::Bottom,
                    order: 3
                },
                ButtonSource::Ladder {
                    ladder: 0,
                    index: 3
                }
            )
        );
    }

    #[test]
    fn a_pin_the_soc_lacks_is_refused_by_its_key() {
        let detail = refusal(&DISPLAY.replace("busy = 6", "busy = 22"));
        assert!(
            detail.starts_with("`wiring.display.busy` = 22: GPIO22 does not exist"),
            "{detail}"
        );
    }

    #[test]
    fn a_pin_taken_twice_is_refused_naming_both_keys() {
        let detail = refusal(&format!(
            "{DISPLAY}[wiring.power]\nlatch = 5\nusb_detect = 20\n"
        ));
        assert!(
            detail.starts_with("`wiring.power.latch` = 5: GPIO5 is already `wiring.display.rst`"),
            "{detail}"
        );
    }

    #[test]
    fn the_sd_card_shares_sclk_and_mosi_and_nothing_else() {
        let shared = load(&format!("{DISPLAY}[wiring.sd]\ncs = 12\nmiso = 7\n")).unwrap();
        assert_eq!(
            shared.wiring.unwrap().sd,
            Some(SdWiring {
                cs: 12,
                miso: 7,
                power: None
            })
        );
        let detail = refusal(&format!("{DISPLAY}[wiring.sd]\ncs = 21\nmiso = 7\n"));
        assert!(
            detail.starts_with("`wiring.sd.cs` = 21: GPIO21 is already `wiring.display.cs`"),
            "{detail}"
        );
    }

    #[test]
    fn an_analog_pin_outside_adc1_is_refused() {
        let ladder = "[[wiring.ladder]]\npin = 7\nidle_mv = 3300\nbuttons = [{ id = \"up\", mv = 1000, edge = \"right\", order = 0 }]\n";
        let detail = refusal(&format!("{DISPLAY}{ladder}"));
        assert!(
            detail.starts_with("`wiring.ladder[0].pin` = 7: GPIO7 is not a channel of ADC1"),
            "{detail}"
        );
        let battery = "[wiring.battery]\npin = 9\ntop_ohms = 1\nbottom_ohms = 1\ncell_mv = 3900\n";
        let detail = refusal(&format!("{DISPLAY}{battery}"));
        assert!(
            detail.starts_with("`wiring.battery.pin` = 9: GPIO9 is not a channel of ADC1"),
            "{detail}"
        );
    }

    #[test]
    fn two_buttons_with_one_id_are_refused() {
        let wiring = format!(
            "{DISPLAY}[[wiring.ladder]]\npin = 1\nidle_mv = 3300\nbuttons = [{{ id = \"up\", mv = 1000, edge = \"right\", order = 0 }}]\n[wiring.button.up]\npin = 3\nactive = \"low\"\nedge = \"top\"\norder = 0\n"
        );
        let detail = refusal(&wiring);
        assert!(
            detail.starts_with("`wiring.ladder[0].buttons[0].id` = \"up\": `wiring.button.up` names a button `up` already"),
            "{detail}"
        );
    }

    #[test]
    fn two_buttons_at_one_place_are_refused() {
        let wiring = format!(
            "{DISPLAY}[[wiring.ladder]]\npin = 1\nidle_mv = 3300\nbuttons = [{{ id = \"up\", mv = 1000, edge = \"right\", order = 0 }}, {{ id = \"down\", mv = 500, edge = \"right\", order = 0 }}]\n"
        );
        let detail = refusal(&wiring);
        assert!(
            detail.starts_with("`wiring.ladder[0].buttons[1].id` = \"down\": the right edge has `up` at order 0 already"),
            "{detail}"
        );
    }

    #[test]
    fn a_button_id_a_script_cannot_name_is_refused() {
        for id in ["Up", "usb-power", "a b"] {
            let wiring = format!(
                "{DISPLAY}[[wiring.ladder]]\npin = 1\nidle_mv = 3300\nbuttons = [{{ id = \"{id}\", mv = 1000, edge = \"right\", order = 0 }}]\n"
            );
            let detail = refusal(&wiring);
            assert!(
                detail.contains("a button id is lower-case letters"),
                "{detail}"
            );
        }
    }

    #[test]
    fn a_ladder_its_chip_refuses_is_refused_by_its_key() {
        let wiring = format!(
            "{DISPLAY}[[wiring.ladder]]\npin = 1\nidle_mv = 3300\nbuttons = [{{ id = \"up\", mv = 3300, edge = \"right\", order = 0 }}]\n"
        );
        let detail = refusal(&wiring);
        assert!(
            detail.starts_with(
                "`wiring.ladder[0]`: the button `up` reads 3300 mV, not below the idle level"
            ),
            "{detail}"
        );
    }

    #[test]
    fn a_power_key_that_names_no_button_is_refused() {
        let detail = refusal(&format!(
            "{DISPLAY}[power]\nkey = \"power\"\nhold_ms = 1000\n"
        ));
        assert!(
            detail.starts_with("`power.key` = \"power\": no button of `[wiring]` has that id"),
            "{detail}"
        );
        let detail = refusal("[power]\nkey = \"power\"\nhold_ms = 1000\n");
        assert!(
            detail.starts_with("`[power]` names the power key of a wired device"),
            "{detail}"
        );
    }

    #[test]
    fn an_unknown_controller_or_configuration_is_refused() {
        let detail = refusal(&DISPLAY.replace("\"ssd1677\"", "\"uc8151\""));
        assert!(
            detail.starts_with("`wiring.display.controller` = \"uc8151\""),
            "{detail}"
        );
        let detail = refusal(&DISPLAY.replace("\"gdeq0426t82\"", "\"gdey0213\""));
        assert!(
            detail.starts_with("`wiring.display.config` = \"gdey0213\""),
            "{detail}"
        );
    }

    #[test]
    fn a_rotation_off_the_quarter_turns_is_refused() {
        let detail = refusal(&DISPLAY.replace("rotation = 90", "rotation = 45"));
        assert!(
            detail.starts_with("`wiring.display.rotation` = 45"),
            "{detail}"
        );
    }

    #[test]
    fn an_unknown_key_of_the_wiring_is_refused_by_the_parser() {
        let error = load(&format!("{DISPLAY}colour = \"black\"\n")).unwrap_err();
        assert!(matches!(error, ProfileError::Parse { .. }), "{error:?}");
        assert!(error.to_string().contains("colour"), "{error}");
    }
}
