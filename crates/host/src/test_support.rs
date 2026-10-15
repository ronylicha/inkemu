//! A device for the tests of a front end that drives an [`Emulator`](crate::device::Emulator)
//! (the Tauri shell): a machine that loops for ever behind the test pattern of the X4, without a
//! flash file, whose power-off can be made slow. Built for the tests of this crate and, with the
//! feature `test-support`, for a front end's tests; never in a release.
use crate::catalog;
use crate::device::{FlashKeeper, Launch};
use crate::engine::Limits;
use crate::pattern;
use crate::ports::{Life, PanelDescription, PanelFrame, PanelSource, PowerOnError, Recipe};
use soc_esp32c3::test_support::{HANG, flat_rom, stop_watchdogs};
use soc_esp32c3::{Esp32C3, SocConfig};
use std::time::Duration;

/// A panel whose drop, at the end of its life, takes `slow_end`: the power-off that ends the
/// life lasts at least that long.
struct SlowToEnd {
    panel: Box<dyn PanelSource>,
    slow_end: Duration,
}

impl PanelSource for SlowToEnd {
    fn describe(&self) -> PanelDescription {
        self.panel.describe()
    }

    fn poll(
        &mut self,
        machine: &mut Esp32C3,
    ) -> Result<Option<PanelFrame>, crate::ports::FrameError> {
        self.panel.poll(machine)
    }
}

impl Drop for SlowToEnd {
    fn drop(&mut self) {
        std::thread::sleep(self.slow_end);
    }
}

struct LoopingDevice {
    slow_end: Duration,
}

impl Recipe for LoopingDevice {
    fn power_on(&mut self, _flash: Option<Vec<u8>>, _: bool) -> Result<Life, PowerOnError> {
        let mut program = stop_watchdogs();
        program.push(HANG);
        let look = catalog::look("x4");
        let (panel, buttons) = pattern::ports(look.panel, look.buttons);
        Ok(Life {
            machine: Esp32C3::new(flat_rom(&program), &SocConfig::default()),
            panel: Box::new(SlowToEnd {
                panel,
                slow_end: self.slow_end,
            }),
            buttons,
        })
    }
}

/// A run of the looping device, with no flash file and no limit, whose power-off lasts at least
/// `slow_end`.
#[must_use]
pub fn looping_device(slow_end: Duration) -> Launch {
    Launch::new(
        Box::new(LoopingDevice { slow_end }),
        FlashKeeper::without_file(None, 4 << 20),
        Limits::default(),
        catalog::look("x4"),
        None,
    )
}
