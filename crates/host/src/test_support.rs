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
    fn power_on(
        &mut self,
        _flash: Option<Vec<u8>>,
        _: bool,
        _: Option<boards::SdStart>,
    ) -> Result<Life, PowerOnError> {
        let mut program = stop_watchdogs();
        program.push(HANG);
        let look = x4_pattern_look();
        let (panel, buttons) = pattern::ports(look.panel, &look.buttons);
        Ok(Life {
            machine: Esp32C3::new(flat_rom(&program), &SocConfig::default()),
            panel: Box::new(SlowToEnd {
                panel,
                slow_end: self.slow_end,
            }),
            buttons,
            sd: None,
        })
    }
}

/// The look of the X4 behind the test pattern: its panel and its keys, drawn by the pattern.
fn x4_pattern_look() -> catalog::Look {
    let mut look = boards::builtin("x4").map_or_else(
        |_| catalog::Look {
            panel: crate::ports::PanelDescription {
                width: 800,
                height: 480,
                shades: crate::ports::Shades::Levels(4),
                rotation: crate::ports::Rotation::Deg90,
                origin: crate::ports::PanelOrigin::TestPattern,
                approximate: false,
            },
            buttons: Vec::new(),
        },
        |x4| catalog::look(&x4),
    );
    look.panel.origin = crate::ports::PanelOrigin::TestPattern;
    look.panel.shades = crate::ports::Shades::Levels(4);
    look.panel.approximate = false;
    look
}

/// [`looping_device`] holding the SD image at `path`, opened and locked as a run opens it: the
/// image a second run on the same card cannot open while this one lives.
///
/// # Panics
/// When the image cannot be opened.
#[must_use]
pub fn looping_device_with_sd(path: &std::path::Path) -> Launch {
    let image = crate::sd_image::SdImage::open(
        path,
        crate::sd_image::Access::ReadWrite,
        &[],
        &mut Vec::new(),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    looping_device(Duration::ZERO).with_sd(image)
}

/// A run of the looping device, with no flash file and no limit, whose power-off lasts at least
/// `slow_end`.
#[must_use]
pub fn looping_device(slow_end: Duration) -> Launch {
    Launch::new(
        Box::new(LoopingDevice { slow_end }),
        FlashKeeper::without_file(None, 4 << 20),
        Limits::default(),
        x4_pattern_look(),
        None,
    )
}
