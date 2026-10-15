//! The panel and the buttons of each device, and how the emulator powers one on (design of
//! sub-project 2c, §4; design of sub-project 2d, 2d-D15 and 2d-D16).
//!
//! This is the one place that decides which ports a device gets. A device whose profile has a
//! `[wiring]` (the X4) shows the image its display controller published and presses its board:
//! its look and its ports come from the wiring ([`crate::ports_wired`]). A device described by
//! its `SoC` alone (the X3, until it is wired) gets the test pattern ([`crate::pattern`]) with the
//! geometry of its panel and the list of its buttons.
use crate::pattern;
use crate::ports::{
    ButtonPort, ButtonSpec, Life, PanelDescription, PanelOrigin, PanelSource, PowerOnError, Recipe,
    Rotation, Shades,
};
use crate::ports_wired;
use boards::{
    ButtonsHandle, DeviceProfile, MachineOptions, PanelHandle, SdStart, Wired, WiredOptions,
};
use soc_esp32c3::RomImage;

/// What a device looks like to an interface: its panel and its buttons.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Look {
    pub panel: PanelDescription,
    pub buttons: Vec<ButtonSpec>,
}

/// The look of the device of `profile`.
///
/// - A wired device (the X4): its controller's panel and the buttons of its board, from the
///   profile ([`ports_wired::description`], [`ports_wired::button_specs`]).
/// - The X3: 792 × 528, 4 grey levels, the keys of the X4, which uses the same ladders and power
///   key (`docs/research/xteink-hardware.md`).
/// - Any other device: 800 × 480, 2 levels, no button.
///
/// ASSUMPTION: the X3 is not wired yet; the rotation of its panel (90°, as the X4's) and the
/// places of its keys (the X4's) are not established. Its wiring settles them.
#[must_use]
pub fn look(profile: &DeviceProfile) -> Look {
    if let Some(wiring) = &profile.wiring {
        return Look {
            panel: ports_wired::description(wiring),
            buttons: ports_wired::button_specs(wiring),
        };
    }
    let panel = |width, height, levels| PanelDescription {
        width,
        height,
        shades: Shades::Levels(levels),
        rotation: Rotation::Deg90,
        origin: PanelOrigin::TestPattern,
        approximate: false,
    };
    match profile.id.as_str() {
        "x3" => Look {
            panel: panel(792, 528, 4),
            buttons: x4_buttons(),
        },
        _ => Look {
            panel: panel(800, 480, 2),
            buttons: Vec::new(),
        },
    }
}

/// The buttons of the X4, as its profile places them; none if its profile does not load, which
/// its tests forbid.
fn x4_buttons() -> Vec<ButtonSpec> {
    boards::builtin("x4")
        .ok()
        .and_then(|x4| x4.wiring.as_ref().map(ports_wired::button_specs))
        .unwrap_or_default()
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
    /// The SD card slot of the device, when it has one: an interface offers a card when it
    /// starts, and a new blank one within these sizes.
    pub sd_slot: Option<SdSlotView>,
    /// The device has a USB detection line: an interface offers the cable when it starts.
    pub usb_power: bool,
}

/// What an interface needs of a card slot: the sizes of a new blank card it may ask for, the
/// bounds `host::sd_format` holds (an interface keeps no copy of them, H9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SdSlotView {
    /// The smallest new card: FAT32's least.
    pub min_mib: u64,
    /// The largest: what the CSD of the SPI mode can say.
    pub max_mib: u64,
    /// The size offered first: 1 GiB.
    pub default_mib: u64,
}

impl SdSlotView {
    /// The slot of every device that has one.
    pub const OF_A_DEVICE: SdSlotView = SdSlotView {
        min_mib: crate::sd_format::MIN_MEBIBYTES,
        max_mib: crate::sd_format::MAX_MEBIBYTES,
        default_mib: 1024,
    };
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
            let wiring = profile.wiring.as_ref();
            Ok(DeviceView {
                look: look(&profile),
                console: profile.console.name(),
                flash_mib: profile.flash.size() >> 20,
                sd_slot: wiring
                    .is_some_and(|wiring| wiring.sd.is_some())
                    .then_some(SdSlotView::OF_A_DEVICE),
                usb_power: wiring.is_some_and(|wiring| wiring.power.is_some()),
                id: profile.id,
                name: profile.name,
                shipped_bootloader,
                no_bootloader,
            })
        })
        .collect()
}

/// The ports of a life of `profile`: its controller and its board when it is wired and `panel`
/// and `buttons` are their handles, the test pattern otherwise.
#[must_use]
pub fn ports_of(
    profile: &DeviceProfile,
    panel: Option<PanelHandle>,
    buttons: Option<ButtonsHandle>,
) -> (Box<dyn PanelSource>, Box<dyn ButtonPort>) {
    if let (Some(wiring), Some(panel), Some(buttons)) = (&profile.wiring, panel, buttons) {
        return ports_wired::ports(wiring, panel, buttons);
    }
    let look = look(profile);
    pattern::ports(look.panel, &look.buttons)
}

/// The life of `wired`, a power-on of `profile`, with its ports.
#[must_use]
pub fn life_of(profile: &DeviceProfile, wired: Wired) -> Life {
    let (panel, buttons) = ports_of(profile, wired.panel, wired.buttons);
    Life {
        machine: wired.machine,
        panel,
        buttons,
        sd: wired.sd,
    }
}

/// What a device's board starts each life with, besides its card: the hold of the power key
/// (the profile's for `None`, 0 for no press) and the cable (2d-D14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardStart {
    pub power_hold_ms: Option<u32>,
    pub usb_power: bool,
}

/// Powers on the device of a profile, wired, with a ROM and run-time options, and its ports.
#[derive(Debug)]
pub struct ProfileRecipe {
    pub profile: DeviceProfile,
    pub rom: RomImage,
    pub options: MachineOptions,
    pub board: BoardStart,
}

impl Recipe for ProfileRecipe {
    fn power_on(
        &mut self,
        flash: Option<Vec<u8>>,
        recorded: bool,
        sd: Option<SdStart>,
    ) -> Result<Life, PowerOnError> {
        let options = MachineOptions {
            trace_hash: self.options.trace_hash || recorded,
            ..self.options
        };
        let wired = WiredOptions {
            power_hold_ms: self.board.power_hold_ms,
            usb_power: self.board.usb_power,
            sd,
        };
        let wired = Wired::power_on(&self.profile, self.rom.clone(), flash, options, wired)?;
        Ok(life_of(&self.profile, wired))
    }

    fn power_on_by_key(
        &mut self,
        flash: Option<Vec<u8>>,
        recorded: bool,
        sd: Option<SdStart>,
    ) -> Result<Life, PowerOnError> {
        let board = self.board;
        self.board.power_hold_ms = Some(0);
        let life = self.power_on(flash, recorded, sd);
        self.board = board;
        life
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::Edge;
    use soc_esp32c3::test_support::{HANG, flat_rom};

    fn profile(id: &str) -> DeviceProfile {
        boards::builtin(id).unwrap()
    }

    /// 2d-D15, 2d-D16: the X4's look is its wiring; the X3, not wired, keeps the pattern with the
    /// keys of the X4.
    #[test]
    fn the_x4_looks_as_its_wiring_says_and_the_x3_as_its_pattern() {
        let x4 = look(&profile("x4"));
        assert_eq!(
            (x4.panel.width, x4.panel.height, x4.panel.shades),
            (800, 480, Shades::Continuous)
        );
        assert_eq!(
            (x4.panel.origin, x4.panel.rotation, x4.panel.approximate),
            (PanelOrigin::Device, Rotation::Deg90, true)
        );
        let ids: Vec<&str> = x4.buttons.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(
            ids,
            ["power", "back", "confirm", "left", "right", "up", "down"]
        );
        let up = &x4.buttons[5];
        assert_eq!((up.edge, up.order), (Edge::Right, 0));
        let x3 = look(&profile("x3"));
        assert_eq!(
            (x3.panel.width, x3.panel.height, x3.panel.shades),
            (792, 528, Shades::Levels(4))
        );
        assert_eq!(
            (x3.panel.origin, x3.panel.approximate),
            (PanelOrigin::TestPattern, false)
        );
        assert_eq!(x3.buttons, x4.buttons);
    }

    /// The list an interface shows: every built-in device, the X4 with its console, its flash,
    /// its card slot and its cable.
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
        assert!(x4.usb_power);
        assert_eq!(
            x4.sd_slot,
            Some(SdSlotView {
                min_mib: 64,
                max_mib: 2_097_024,
                default_mib: 1024
            }),
            "the bounds of a new card are host's"
        );
        assert_eq!(x4.look, look(&profile("x4")));
        assert_ne!(
            x4.shipped_bootloader.is_some(),
            x4.no_bootloader.is_some(),
            "one or the other"
        );
        let x3 = devices.iter().find(|d| d.id == "x3").unwrap();
        assert!(x3.sd_slot.is_none() && !x3.usb_power);
    }

    /// Every button sits at its own place along its edge.
    #[test]
    fn no_two_buttons_share_a_place() {
        for id in boards::builtin_ids() {
            let buttons = look(&profile(id)).buttons;
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

    /// A power-on gives the wired machine of the X4 with its ports, the gesture journaled, and
    /// keeps the flash given; the X3 gets the pattern.
    #[test]
    fn the_recipe_of_a_profile_powers_it_on_wired_with_its_ports() {
        let mut recipe = ProfileRecipe {
            profile: profile("x4"),
            rom: flat_rom(&[HANG]),
            options: MachineOptions::default(),
            board: BoardStart::default(),
        };
        let life = recipe.power_on(Some(vec![0x42; 16]), false, None).unwrap();
        assert_eq!(life.panel.describe(), look(&profile("x4")).panel);
        assert_eq!(life.buttons.buttons(), look(&profile("x4")).buttons);
        assert!(life.buttons.reaches_firmware());
        assert!(life.sd.is_some(), "the X4 has a slot, empty here");
        assert_eq!(
            life.machine.board_inputs_pending(),
            2,
            "the power-on gesture"
        );
        assert_eq!(&life.machine.flash_contents()[..2], [0x42, 0x42]);
        assert_eq!(life.machine.trace_hash(), None);
        let erased = recipe.power_on(None, true, None).unwrap();
        assert_eq!(erased.machine.flash_contents()[0], 0xFF);
        assert!(
            erased.machine.trace_hash().is_some(),
            "a recorded life keeps its trace hash"
        );
        assert!(
            recipe
                .power_on(Some(vec![0; 17 << 20]), false, None)
                .is_err()
        );
        let mut x3 = ProfileRecipe {
            profile: profile("x3"),
            rom: flat_rom(&[HANG]),
            options: MachineOptions::default(),
            board: BoardStart::default(),
        };
        let life = x3.power_on(None, false, None).unwrap();
        assert_eq!(life.panel.describe().origin, PanelOrigin::TestPattern);
        assert!(!life.buttons.reaches_firmware());
    }
}
