//! A push button wired between a pin and a supply rail: the X4's power key, between GPIO3 and
//! ground (design of sub-project 2d, 2d-D6; research `sp2d-x4-wiring.md` §1 point 5).
//!
//! Pressed, the key ties the pin to its rail: it drives the pin's active level. Released, it leaves
//! the pin alone, and the pad reads whatever pull the firmware gave it (`FreeInk` sets the X4's
//! power key `INPUT_PULLUP`).

use emu_core::{GpioInput, Level, Nanos};

/// A push button on a pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PushButton {
    /// The level the pin reads while the button is held: `Low` for a key to ground.
    active: Level,
    pressed: bool,
}

impl PushButton {
    /// A released button whose pin reads `active` while it is held.
    #[must_use]
    pub const fn new(active: Level) -> PushButton {
        PushButton {
            active,
            pressed: false,
        }
    }

    /// The level the pin reads while the button is held.
    #[must_use]
    pub const fn active(&self) -> Level {
        self.active
    }

    #[must_use]
    pub const fn is_pressed(&self) -> bool {
        self.pressed
    }

    /// Holds (`true`) or releases (`false`) the button.
    pub fn set_pressed(&mut self, pressed: bool) {
        self.pressed = pressed;
    }
}

impl GpioInput for PushButton {
    fn level(&mut self, _at: Nanos) -> Option<Level> {
        self.pressed.then_some(self.active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_the_button_drives_its_level_and_released_it_lets_the_pin_go() {
        let mut to_ground = PushButton::new(Level::Low);
        assert_eq!(to_ground.level(Nanos(0)), None, "released at first");
        to_ground.set_pressed(true);
        assert!(to_ground.is_pressed());
        assert_eq!(to_ground.level(Nanos(1)), Some(Level::Low));
        to_ground.set_pressed(false);
        assert_eq!(to_ground.level(Nanos(2)), None);
        let mut to_supply = PushButton::new(Level::High);
        to_supply.set_pressed(true);
        assert_eq!(to_supply.level(Nanos(3)), Some(Level::High));
        assert_eq!(to_supply.active(), Level::High);
        assert_eq!(
            to_supply.next_change(Nanos(3)),
            None,
            "it changes only when acted on"
        );
    }
}
