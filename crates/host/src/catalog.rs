//! The panel and the buttons of each device, and how the emulator powers one on (design of
//! sub-project 2c, §4).
//!
//! This is the one place that decides which ports a device gets. Until the display controller and
//! the button ladder are wired (sub-project 2d), every device gets the test pattern
//! ([`crate::pattern`]) with the geometry of its panel and the list of its buttons; sub-project 2d
//! replaces the pattern of the X4 here, and no interface changes.
use crate::pattern;
use crate::ports::{
    ButtonPort, ButtonSpec, Edge, Life, PanelDescription, PanelOrigin, PanelSource, PowerOnError,
    Recipe, Rotation,
};
use boards::{DeviceProfile, MachineOptions};
use soc_esp32c3::RomImage;

const fn button(id: &'static str, edge: Edge, order: u8) -> ButtonSpec {
    ButtonSpec { id, edge, order }
}

/// The seven keys of the X4 (`docs/research/xteink-hardware.md`, "Buttons"): Back, Confirm, Left
/// and Right on the front, Up and Down on the right side, Power on top.
/// ASSUMPTION: the order of the four front keys from left to right is not established; a photo of
/// a unit settles it.
pub const X4_BUTTONS: [ButtonSpec; 7] = [
    button("power", Edge::Top, 0),
    button("up", Edge::Right, 0),
    button("down", Edge::Right, 1),
    button("back", Edge::Bottom, 0),
    button("confirm", Edge::Bottom, 1),
    button("left", Edge::Bottom, 2),
    button("right", Edge::Bottom, 3),
];

/// The X3 uses the same ladders and power key as the X4 (`docs/research/xteink-hardware.md`).
/// ASSUMPTION: its physical layout is not known; the X4's is used.
pub const X3_BUTTONS: [ButtonSpec; 7] = X4_BUTTONS;

/// What a device looks like to an interface: its panel and its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Look {
    pub panel: PanelDescription,
    pub buttons: &'static [ButtonSpec],
}

/// The look of the device whose profile is `id`.
///
/// - X4: 800 × 480, 4 grey levels (GDEQ0426T82 behind an SSD1677).
/// - X3: 792 × 528, 4 grey levels.
/// - Any other device: 800 × 480, 2 levels, no button.
///
/// ASSUMPTION: the rotation of both panels in their device (90°, native landscape scan shown in
/// portrait) is not established; the X4 firmware draws in portrait (rotation 3 of `GxEPD2` in the
/// sample). The first upright boot screen of `CrossPoint` under sub-project 2d settles it.
#[must_use]
pub fn look(id: &str) -> Look {
    let panel = |width, height, gray_levels| PanelDescription {
        width,
        height,
        gray_levels,
        rotation: Rotation::Deg90,
        origin: PanelOrigin::TestPattern,
    };
    match id {
        "x4" => Look {
            panel: panel(800, 480, 4),
            buttons: &X4_BUTTONS,
        },
        "x3" => Look {
            panel: panel(792, 528, 4),
            buttons: &X3_BUTTONS,
        },
        _ => Look {
            panel: panel(800, 480, 2),
            buttons: &[],
        },
    }
}

/// A built-in device, as an interface lists it before anything runs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    /// The console shown first: `uart0`, `uart1` or `usb-serial-jtag`.
    pub console: &'static str,
    pub flash_mib: u32,
    pub look: Look,
    /// The bootloader inkemu ships for the device (`esp-idf-v5.5.2-esp32c3-dio`…), when one
    /// starts it.
    pub shipped_bootloader: Option<&'static str>,
    /// Why no bootloader inkemu ships starts it, when none does: a bootloader must be given.
    pub no_bootloader: Option<String>,
}

/// The built-in devices, in the order of their profiles.
///
/// # Errors
/// [`boards::ProfileError`] when a built-in profile does not load, which its tests forbid.
pub fn devices() -> Result<Vec<DeviceView>, boards::ProfileError> {
    boards::builtin_ids()
        .iter()
        .map(|id| {
            let profile = boards::builtin(id)?;
            let (shipped_bootloader, no_bootloader) = match profile.bootloader {
                Ok(shipped) => (Some(shipped.id), None),
                Err(none) => (None, Some(none.to_string())),
            };
            Ok(DeviceView {
                look: look(&profile.id),
                console: profile.console.name(),
                flash_mib: profile.flash.size() >> 20,
                id: profile.id,
                name: profile.name,
                shipped_bootloader,
                no_bootloader,
            })
        })
        .collect()
}

/// The ports of a device whose profile is `profile`, for one life.
#[must_use]
pub fn ports_for(profile: &DeviceProfile) -> (Box<dyn PanelSource>, Box<dyn ButtonPort>) {
    let look = look(&profile.id);
    pattern::ports(look.panel, look.buttons)
}

/// Powers on the device of a profile, with a ROM and run-time options, and its ports.
#[derive(Debug)]
pub struct ProfileRecipe {
    pub profile: DeviceProfile,
    pub rom: RomImage,
    pub options: MachineOptions,
}

impl Recipe for ProfileRecipe {
    fn power_on(&mut self, flash: Option<Vec<u8>>, recorded: bool) -> Result<Life, PowerOnError> {
        let rom = self.rom.clone();
        let options = MachineOptions {
            trace_hash: self.options.trace_hash || recorded,
            ..self.options
        };
        let machine = match flash {
            Some(bytes) => self.profile.instantiate_with_flash(rom, bytes, options)?,
            None => self.profile.instantiate(rom, options),
        };
        let (panel, buttons) = ports_for(&self.profile);
        Ok(Life {
            machine,
            panel,
            buttons,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soc_esp32c3::test_support::{HANG, flat_rom};

    #[test]
    fn every_built_in_device_has_a_panel_and_the_x4_its_seven_keys() {
        for id in boards::builtin_ids() {
            let look = look(id);
            assert!(look.panel.width > 0 && look.panel.height > 0, "{id}");
            assert_eq!(look.panel.gray_levels, 4, "{id}");
        }
        let x4 = look("x4");
        let ids: Vec<&str> = x4.buttons.iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            ["power", "up", "down", "back", "confirm", "left", "right"]
        );
        assert_eq!((x4.panel.width, x4.panel.height), (800, 480));
        assert_eq!(look("unknown").buttons.len(), 0);
    }

    /// The list an interface shows: every built-in device, the X4 with its console and flash.
    #[test]
    fn the_devices_are_the_built_in_profiles() {
        let devices = devices().unwrap();
        let ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, boards::builtin_ids());
        let x4 = devices.iter().find(|d| d.id == "x4").unwrap();
        assert_eq!(
            (x4.name.as_str(), x4.console, x4.flash_mib),
            ("Xteink X4", "usb-serial-jtag", 16)
        );
        assert_eq!(x4.look, look("x4"));
        assert_ne!(
            x4.shipped_bootloader.is_some(),
            x4.no_bootloader.is_some(),
            "one or the other"
        );
    }

    /// Every button sits at its own place along its edge.
    #[test]
    fn no_two_buttons_share_a_place() {
        for id in boards::builtin_ids() {
            let buttons = look(id).buttons;
            for (i, one) in buttons.iter().enumerate() {
                for other in &buttons[i + 1..] {
                    assert!(
                        one.id != other.id && (one.edge, one.order) != (other.edge, other.order),
                        "{id}: {one:?} {other:?}"
                    );
                }
            }
        }
    }

    /// A power-on gives the machine and the pattern of the profile, and keeps the flash given.
    #[test]
    fn the_recipe_of_a_profile_powers_it_on_with_its_ports() {
        let profile = boards::builtin("x4").unwrap();
        let mut recipe = ProfileRecipe {
            profile,
            rom: flat_rom(&[HANG]),
            options: MachineOptions::default(),
        };
        let life = recipe.power_on(Some(vec![0x42; 16]), false).unwrap();
        assert_eq!(life.panel.describe(), look("x4").panel);
        assert_eq!(life.buttons.buttons(), X4_BUTTONS);
        assert_eq!(&life.machine.flash_contents()[..2], [0x42, 0x42]);
        assert_eq!(life.machine.trace_hash(), None);
        let erased = recipe.power_on(None, true).unwrap();
        assert_eq!(erased.machine.flash_contents()[0], 0xFF);
        assert!(
            erased.machine.trace_hash().is_some(),
            "a recorded life keeps its trace hash"
        );
        assert!(recipe.power_on(Some(vec![0; 17 << 20]), false).is_err());
    }
}
