//! The ports of a wired device (design of sub-project 2d, 2d-D16 to 2d-D19): its panel shows what
//! the display controller published, and its buttons are inputs of its board, which the machine
//! dates and journals.
//!
//! The panel gives each refresh once, as the controller's model published it: its pixels as they
//! are (any value, [`Shades::Continuous`]), full or partial by the controller's display mode, and
//! the duration from the instant the display began driving the glass to the instant it ended. The
//! buttons send [`boards::ButtonsHandle::button_input`] at the present instant of the machine: a
//! session holds them.
use crate::ports::{
    ButtonError, ButtonPort, ButtonSpec, Edge, FrameError, FrameParts, PanelDescription,
    PanelFrame, PanelOrigin, PanelSource, RefreshKind, Rotation, Shades,
};
use boards::wiring::{self, Wiring};
use boards::{ButtonsHandle, PanelHandle};
use soc_esp32c3::Esp32C3;
use std::sync::Arc;

/// The rotation of the profile, as the ports say it.
#[must_use]
pub const fn rotation(rotation: wiring::Rotation) -> Rotation {
    match rotation {
        wiring::Rotation::Deg0 => Rotation::Deg0,
        wiring::Rotation::Deg90 => Rotation::Deg90,
        wiring::Rotation::Deg180 => Rotation::Deg180,
        wiring::Rotation::Deg270 => Rotation::Deg270,
    }
}

const fn edge(edge: wiring::Edge) -> Edge {
    match edge {
        wiring::Edge::Top => Edge::Top,
        wiring::Edge::Right => Edge::Right,
        wiring::Edge::Bottom => Edge::Bottom,
        wiring::Edge::Left => Edge::Left,
    }
}

/// The buttons of a wired device, in the order of their targets on its board (2d-D9), with their
/// places on the case.
#[must_use]
pub fn button_specs(wiring: &Wiring) -> Vec<ButtonSpec> {
    wiring
        .buttons()
        .into_iter()
        .map(|button| ButtonSpec {
            id: button.id,
            edge: edge(button.placement.edge),
            order: button.placement.order,
        })
        .collect()
}

/// The panel of a wired device: the controller's geometry, any shade, the rotation of the
/// profile, and whether the model rests on parameters nobody measured (2d-D16, 2d-D18), the X4's
/// SSD1677 as the X3's UC8253 (ADR 0015).
#[must_use]
pub fn description(wiring: &Wiring) -> PanelDescription {
    let config = wiring.display.config;
    let (width, height) = config.size();
    PanelDescription {
        width: u32::from(width),
        height: u32::from(height),
        shades: Shades::Continuous,
        rotation: rotation(wiring.display.rotation),
        origin: PanelOrigin::Device,
        approximate: config.is_approximation(),
    }
}

/// The two ports of a wired device whose panel and buttons are `panel` and `buttons`.
#[must_use]
pub fn ports(
    wiring: &Wiring,
    panel: PanelHandle,
    buttons: ButtonsHandle,
) -> (Box<dyn PanelSource>, Box<dyn ButtonPort>) {
    (
        Box::new(WiredPanel {
            description: description(wiring),
            handle: panel,
            shown: 0,
        }),
        Box::new(WiredButtons {
            specs: button_specs(wiring),
            handle: buttons,
        }),
    )
}

/// The panel: the image of the controller, each generation once.
struct WiredPanel {
    description: PanelDescription,
    handle: PanelHandle,
    /// The generation last given; 0 before the first refresh.
    shown: u64,
}

impl PanelSource for WiredPanel {
    fn describe(&self) -> PanelDescription {
        self.description
    }

    fn poll(&mut self, _machine: &mut Esp32C3) -> Result<Option<PanelFrame>, FrameError> {
        let image = self.handle.image();
        if image.generation <= self.shown {
            return Ok(None);
        }
        self.shown = image.generation;
        // The chip counts in cycles of 1 GHz: nanoseconds.
        let duration = image.at.get().saturating_sub(image.started.get());
        PanelFrame::new(FrameParts {
            width: u32::from(image.width),
            height: u32::from(image.height),
            shades: Shades::Continuous,
            generation: image.generation,
            refresh: match image.kind {
                chips::RefreshKind::Partial => RefreshKind::Partial,
                chips::RefreshKind::Full | chips::RefreshKind::PowerOn => RefreshKind::Full,
            },
            region: None,
            duration_nanos: Some(duration),
            pixels: Arc::from(image.pixels.as_slice()),
        })
        .map(Some)
    }
}

/// The buttons: each press and release an input of the board, now.
struct WiredButtons {
    specs: Vec<ButtonSpec>,
    handle: ButtonsHandle,
}

impl ButtonPort for WiredButtons {
    fn buttons(&self) -> &[ButtonSpec] {
        &self.specs
    }

    fn reaches_firmware(&self) -> bool {
        true
    }

    fn set(&mut self, machine: &mut Esp32C3, id: &str, pressed: bool) -> Result<(), ButtonError> {
        let input = self
            .handle
            .button_input(id, pressed)
            .ok_or_else(|| ButtonError::Unknown { id: id.to_owned() })?;
        machine
            .send_board_input(machine.device_time(), input)
            .map_err(|e| ButtonError::Refused {
                id: id.to_owned(),
                detail: e.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boards::{MachineOptions, Wired, WiredOptions};
    use cpu_rv32::asm;
    use emu_core::Cycles;
    use soc_esp32c3::test_support::{HANG, flat_rom};

    fn x4() -> boards::DeviceProfile {
        boards::builtin("x4").unwrap()
    }

    fn wired(program: &[u32]) -> (Esp32C3, Box<dyn PanelSource>, Box<dyn ButtonPort>) {
        let profile = x4();
        let options = WiredOptions {
            power_hold_ms: Some(0),
            ..WiredOptions::default()
        };
        let Wired {
            machine,
            panel,
            buttons,
            ..
        } = Wired::power_on(
            &profile,
            flat_rom(program),
            None,
            MachineOptions::default(),
            options,
        )
        .unwrap();
        let wiring = profile.wiring.as_ref().unwrap();
        let (panel, buttons) = ports(wiring, panel.unwrap(), buttons.unwrap());
        (machine, panel, buttons)
    }

    /// 2d-D16, 2d-D18: the X4 shows its controller's image, turned by the profile, in any shade,
    /// and says the rendering is approximate.
    #[test]
    fn the_x4_panel_is_its_controller_turned_by_the_profile_and_approximate() {
        let (_, panel, buttons) = wired(&[HANG]);
        assert_eq!(
            panel.describe(),
            PanelDescription {
                width: 800,
                height: 480,
                shades: Shades::Continuous,
                rotation: Rotation::Deg90,
                origin: PanelOrigin::Device,
                approximate: true,
            }
        );
        let ids: Vec<&str> = buttons.buttons().iter().map(|b| b.id.as_str()).collect();
        assert_eq!(
            ids,
            ["power", "back", "confirm", "left", "right", "up", "down"]
        );
        assert!(buttons.reaches_firmware());
        let confirm = &buttons.buttons()[2];
        assert_eq!((confirm.edge, confirm.order), (Edge::Bottom, 1));
    }

    /// A display of the controller, `mode` its `0x22` option, begun at `at` nanoseconds and run to
    /// its end.
    fn display(handle: &PanelHandle, mode: u8, at: u64) {
        let mut lines = handle.ssd1677().unwrap();
        let chip = lines.chip_mut();
        chip.write_command(Cycles(at), 0x22);
        chip.write_data(Cycles(at), mode);
        chip.write_command(Cycles(at), 0x20);
        chip.advance(Cycles(at + 10_000_000_000));
    }

    /// 2d-D16: each refresh once, full or partial by the controller's display mode, with the
    /// duration of the display and its pixels as published.
    #[test]
    fn each_refresh_is_given_once_with_its_kind_and_duration() {
        let profile = x4();
        let options = WiredOptions {
            power_hold_ms: Some(0),
            ..WiredOptions::default()
        };
        let wired = Wired::power_on(
            &profile,
            flat_rom(&[HANG]),
            None,
            MachineOptions::default(),
            options,
        )
        .unwrap();
        let (mut machine, handle) = (wired.machine, wired.panel.unwrap());
        let wiring = profile.wiring.as_ref().unwrap();
        let (mut panel, _) = ports(wiring, handle.clone(), wired.buttons.unwrap());
        assert_eq!(
            panel.poll(&mut machine),
            Ok(None),
            "nothing before a refresh"
        );
        display(&handle, 0xF7, 2_000_000);
        let full = panel.poll(&mut machine).unwrap().expect("the full refresh");
        let published = handle.image().clone();
        assert_eq!((full.generation(), full.refresh()), (1, RefreshKind::Full));
        assert_eq!(
            full.duration_nanos(),
            Some(published.at.get() - published.started.get())
        );
        assert!(full.duration_nanos().unwrap() > 0);
        assert_eq!(full.pixels(), published.pixels.as_slice());
        assert_eq!((full.width(), full.height()), (800, 480));
        assert_eq!(panel.poll(&mut machine), Ok(None), "given once");
        display(&handle, 0xFC, 20_000_000_000);
        let partial = panel.poll(&mut machine).unwrap().expect("the fast refresh");
        assert_eq!(
            (partial.generation(), partial.refresh()),
            (2, RefreshKind::Partial)
        );
    }

    /// 2d-D19: a press is an input of the board at the present instant, which the journal keeps.
    #[test]
    fn a_press_is_a_board_input_now_and_the_journal_keeps_it() {
        let (mut machine, _, mut buttons) = wired(&[asm::addi(0, 0, 0), HANG]);
        machine.run_within(Cycles(u64::MAX), Some(1_000));
        let now = machine.device_time();
        buttons.set(&mut machine, "confirm", true).unwrap();
        let journal = machine.input_journal().inputs();
        assert!(
            matches!(
                journal.last(),
                Some(soc_esp32c3::AcceptedInput::Board { at, input })
                    if *at == now && input.target == 2 && input.value == 1
            ),
            "{journal:?}"
        );
        assert_eq!(
            buttons.set(&mut machine, "menu", true),
            Err(ButtonError::Unknown { id: "menu".into() })
        );
    }
}
