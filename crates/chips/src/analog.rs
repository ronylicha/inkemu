//! What the SAR ADC of the `SoC` reads: the voltage at a pin, imposed by a button ladder or by the
//! battery through its divider (spec §3.3, research `sp2a-external-chips.md` §7–§8).
//!
//! A source imposes a voltage; turning it into a code is the ADC's work, which depends on the
//! attenuation the firmware chose. So every level here is in millivolts at the pin.

use std::fmt;

/// A voltage at a pin, in millivolts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Millivolts(pub u32);

impl fmt::Display for Millivolts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} mV", self.0)
    }
}

/// One key of a ladder: its name in the profile and the level the pin reads while it is pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LadderButton {
    pub name: String,
    pub level: Millivolts,
}

/// Why a ladder cannot be built, or a key not be found.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LadderError {
    #[error("a button ladder needs at least one button")]
    NoButton,
    #[error("two buttons of the ladder are named `{0}`")]
    DuplicateName(String),
    #[error(
        "the button `{name}` reads {level}, the idle level of the ladder: pressing it would not be seen"
    )]
    AtIdleLevel { name: String, level: Millivolts },
    #[error("the ladder has no button {index} (it has {count})")]
    NoSuchButton { index: usize, count: usize },
}

/// Keys that pull one ADC pin to a level each, through resistors: the X4 has two of them, on GPIO1
/// and GPIO2.
///
/// Nothing pressed, the pin reads the idle level (the pull-up rail). Several keys of one ladder
/// pressed together read the lowest of their levels. ASSUMPTION: a series ladder, where the key
/// nearest to ground wins; no source describes the X4's ladders when two keys are held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ButtonLadder {
    idle: Millivolts,
    buttons: Vec<LadderButton>,
    pressed: Vec<bool>,
}

impl ButtonLadder {
    /// A ladder at rest.
    ///
    /// # Errors
    /// No button, two buttons with one name, or a button at the idle level.
    pub fn new(idle: Millivolts, buttons: Vec<LadderButton>) -> Result<ButtonLadder, LadderError> {
        if buttons.is_empty() {
            return Err(LadderError::NoButton);
        }
        for (index, button) in buttons.iter().enumerate() {
            if buttons[..index].iter().any(|b| b.name == button.name) {
                return Err(LadderError::DuplicateName(button.name.clone()));
            }
            if button.level == idle {
                return Err(LadderError::AtIdleLevel {
                    name: button.name.clone(),
                    level: button.level,
                });
            }
        }
        let pressed = vec![false; buttons.len()];
        Ok(ButtonLadder {
            idle,
            buttons,
            pressed,
        })
    }

    /// The level of the pin when no key is pressed.
    #[must_use]
    pub fn idle(&self) -> Millivolts {
        self.idle
    }

    /// The keys, in the order of the profile; their index is the one the other methods take.
    #[must_use]
    pub fn buttons(&self) -> &[LadderButton] {
        &self.buttons
    }

    /// The index of the key named `name`.
    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.buttons.iter().position(|b| b.name == name)
    }

    /// Presses (`true`) or releases (`false`) a key.
    ///
    /// # Errors
    /// `index` names no key.
    pub fn set_pressed(&mut self, index: usize, pressed: bool) -> Result<(), LadderError> {
        let count = self.pressed.len();
        let slot = self
            .pressed
            .get_mut(index)
            .ok_or(LadderError::NoSuchButton { index, count })?;
        *slot = pressed;
        Ok(())
    }

    /// Whether the key `index` is pressed; `None` when there is no such key.
    #[must_use]
    pub fn is_pressed(&self, index: usize) -> Option<bool> {
        self.pressed.get(index).copied()
    }

    /// Releases every key.
    pub fn release_all(&mut self) {
        self.pressed.fill(false);
    }

    /// The voltage the pin reads now.
    #[must_use]
    pub fn voltage(&self) -> Millivolts {
        self.buttons
            .iter()
            .zip(&self.pressed)
            .filter(|(_, pressed)| **pressed)
            .map(|(button, _)| button.level)
            .min()
            .unwrap_or(self.idle)
    }
}

/// Why a battery cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BatteryError {
    #[error("the bottom resistor of the divider is 0 Ω: the pin would read 0 V whatever the cell")]
    ZeroBottomResistor,
}

/// A cell read through a resistor divider: the X4 reads its battery on GPIO0 through two 10 kΩ
/// resistors, half the cell's voltage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    cell: Millivolts,
    top_ohms: u32,
    bottom_ohms: u32,
}

impl Battery {
    /// A cell at `cell`, behind a divider of `top_ohms` (cell to pin) over `bottom_ohms` (pin to
    /// ground). A `top_ohms` of 0 puts the cell on the pin directly.
    ///
    /// # Errors
    /// `bottom_ohms` is 0.
    pub fn new(cell: Millivolts, top_ohms: u32, bottom_ohms: u32) -> Result<Battery, BatteryError> {
        if bottom_ohms == 0 {
            return Err(BatteryError::ZeroBottomResistor);
        }
        Ok(Battery {
            cell,
            top_ohms,
            bottom_ohms,
        })
    }

    /// The voltage of the cell.
    #[must_use]
    pub fn cell(&self) -> Millivolts {
        self.cell
    }

    /// Sets the voltage of the cell (the user, a script, a discharge model).
    pub fn set_cell(&mut self, cell: Millivolts) {
        self.cell = cell;
    }

    /// The voltage at the pin: the cell's times `bottom / (top + bottom)`, rounded to the nearest
    /// millivolt (half up).
    #[must_use]
    pub fn voltage(&self) -> Millivolts {
        // u128: twice a u32 times a u32 does not fit in a u64.
        let bottom = u128::from(self.bottom_ohms);
        let total = u128::from(self.top_ohms) + bottom;
        let pin = (2 * u128::from(self.cell.0) * bottom + total) / (2 * total);
        // `pin` is at most the cell's voltage, a u32.
        Millivolts(u32::try_from(pin).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(name: &str, mv: u32) -> LadderButton {
        LadderButton {
            name: name.to_owned(),
            level: Millivolts(mv),
        }
    }

    /// The front ladder of the X4 (GPIO1), in millivolts: `CrossPoint`'s averages at 12 dB on the
    /// nominal 0–2500 mV line (research §7).
    fn x4_front() -> ButtonLadder {
        ButtonLadder::new(
            Millivolts(3300),
            vec![
                button("back", 2144),
                button("confirm", 1645),
                button("left", 911),
                button("right", 3),
            ],
        )
        .unwrap()
    }

    #[test]
    fn a_ladder_at_rest_reads_its_idle_level() {
        assert_eq!(x4_front().voltage(), Millivolts(3300));
    }

    #[test]
    fn each_key_pressed_alone_reads_its_level_and_its_release_restores_idle() {
        let mut ladder = x4_front();
        for (index, level) in [2144, 1645, 911, 3].into_iter().enumerate() {
            ladder.set_pressed(index, true).unwrap();
            assert_eq!(ladder.voltage(), Millivolts(level));
            assert_eq!(ladder.is_pressed(index), Some(true));
            ladder.set_pressed(index, false).unwrap();
            assert_eq!(ladder.voltage(), Millivolts(3300));
        }
    }

    #[test]
    fn two_keys_held_read_the_lowest_level() {
        let mut ladder = x4_front();
        ladder.set_pressed(0, true).unwrap();
        ladder.set_pressed(2, true).unwrap();
        assert_eq!(ladder.voltage(), Millivolts(911));
        ladder.set_pressed(2, false).unwrap();
        assert_eq!(ladder.voltage(), Millivolts(2144));
        ladder.set_pressed(1, true).unwrap();
        ladder.release_all();
        assert_eq!(ladder.voltage(), Millivolts(3300));
    }

    #[test]
    fn keys_are_found_by_name() {
        let ladder = x4_front();
        assert_eq!(ladder.index_of("left"), Some(2));
        assert_eq!(ladder.index_of("up"), None);
        assert_eq!(ladder.buttons().len(), 4);
        assert_eq!(ladder.idle(), Millivolts(3300));
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let mut ladder = x4_front();
        assert_eq!(
            ladder.set_pressed(4, true),
            Err(LadderError::NoSuchButton { index: 4, count: 4 })
        );
        assert_eq!(ladder.is_pressed(4), None);
    }

    #[test]
    fn a_ladder_without_keys_with_a_twice_named_key_or_a_key_at_idle_is_refused() {
        assert_eq!(
            ButtonLadder::new(Millivolts(3300), Vec::new()),
            Err(LadderError::NoButton)
        );
        assert_eq!(
            ButtonLadder::new(Millivolts(3300), vec![button("up", 1), button("up", 2)]),
            Err(LadderError::DuplicateName("up".to_owned()))
        );
        let refused = ButtonLadder::new(Millivolts(3300), vec![button("down", 3300)]);
        assert_eq!(
            refused,
            Err(LadderError::AtIdleLevel {
                name: "down".to_owned(),
                level: Millivolts(3300)
            })
        );
        assert_eq!(
            refused.unwrap_err().to_string(),
            "the button `down` reads 3300 mV, the idle level of the ladder: pressing it would not be seen"
        );
    }

    #[test]
    fn the_x4_battery_reads_half_its_cell() {
        let mut battery = Battery::new(Millivolts(3700), 10_000, 10_000).unwrap();
        assert_eq!(battery.voltage(), Millivolts(1850));
        battery.set_cell(Millivolts(4201));
        assert_eq!(battery.cell(), Millivolts(4201));
        // 2100.5 rounds half up.
        assert_eq!(battery.voltage(), Millivolts(2101));
    }

    #[test]
    fn a_divider_rounds_to_the_nearest_millivolt() {
        // 3 / (1 + 2) of 1000 mV: 666.67 mV, rounded to 667.
        let battery = Battery::new(Millivolts(1000), 1, 2).unwrap();
        assert_eq!(battery.voltage(), Millivolts(667));
        // 1 / 3 of 1000 mV: 333.33 mV, rounded to 333.
        let battery = Battery::new(Millivolts(1000), 2, 1).unwrap();
        assert_eq!(battery.voltage(), Millivolts(333));
    }

    #[test]
    fn without_a_top_resistor_the_pin_reads_the_cell_and_extremes_do_not_overflow() {
        let battery = Battery::new(Millivolts(u32::MAX), 0, u32::MAX).unwrap();
        assert_eq!(battery.voltage(), Millivolts(u32::MAX));
        let battery = Battery::new(Millivolts(u32::MAX), u32::MAX, u32::MAX).unwrap();
        assert_eq!(battery.voltage(), Millivolts(u32::MAX / 2 + 1));
    }

    #[test]
    fn a_divider_without_a_bottom_resistor_is_refused() {
        assert_eq!(
            Battery::new(Millivolts(3700), 10_000, 0),
            Err(BatteryError::ZeroBottomResistor)
        );
    }
}
