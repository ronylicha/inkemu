//! A test pattern behind the two ports, for as long as no display controller and no button ladder
//! are wired to the machine (design of sub-project 2c, C8).
//!
//! The pattern reads nothing of the firmware and writes nothing to it: an interface that shows it
//! says so ([`PanelOrigin::TestPattern`], [`ButtonPort::reaches_firmware`] false). It exists so that
//! everything between a panel and a screen — frames, generations, full and partial refreshes, grey
//! levels, rotation, buttons — can be built and tested before sub-project 2d. It depends only on
//! the device time of the machine and on the buttons pressed, so it is the same on every host.
//!
//! What it draws, in the panel's native scan order: a black border; four bands of the four grey
//! levels; a checkerboard of one-pixel squares (any blur of the scaling shows there); one square
//! per button, filled while the button is down; and a bar that fills one tenth per second of
//! device time. Each second is a partial refresh of the bar, each change of a button a partial
//! refresh of the squares, and every tenth refresh a full one, as e-ink firmware does to clear
//! ghosting.
use crate::ports::{
    ButtonError, ButtonPort, ButtonSpec, PanelDescription, PanelFrame, PanelOrigin, PanelSource,
    RefreshKind, Region,
};
use soc_esp32c3::Esp32C3;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// How long the pattern says a full refresh lasts: the full refresh of an SSD1677 (about 1.6 to
/// 1.8 s, `docs/research/xteink-hardware.md`).
pub const FULL_REFRESH_NANOS: u64 = 1_700_000_000;
/// How long the pattern says a partial refresh lasts (about 0.5 to 0.6 s on an SSD1677).
pub const PARTIAL_REFRESH_NANOS: u64 = 500_000_000;
/// Every this many refreshes, one is full.
pub const FULL_EVERY: u64 = 10;

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const BORDER: u32 = 4;
const MARGIN: u32 = 24;
const SQUARE: u32 = 32;
const SQUARE_PITCH: u32 = 48;
const BAR_HEIGHT: u32 = 24;
const BAR_STEPS: u64 = 10;
const BLACK: u8 = 0;
const WHITE: u8 = 255;

/// What the panel and the buttons of a pattern share: which buttons are down, and whether the
/// squares must be drawn again.
#[derive(Debug, Default)]
struct Shared {
    /// Bit `n` is the button at place `n` of the list.
    pressed: u64,
    changed: bool,
}

/// Makes the two ports of a test pattern for a panel described by `panel` (its origin is set to
/// [`PanelOrigin::TestPattern`]) and the buttons `buttons`.
#[must_use]
pub fn ports(
    panel: PanelDescription,
    buttons: &'static [ButtonSpec],
) -> (Box<dyn PanelSource>, Box<dyn ButtonPort>) {
    let (panel, buttons) = pattern(panel, buttons);
    (Box::new(panel), Box::new(buttons))
}

fn pattern(
    panel: PanelDescription,
    buttons: &'static [ButtonSpec],
) -> (PatternPanel, PatternButtons) {
    let shared = Rc::new(RefCell::new(Shared::default()));
    let description = PanelDescription {
        origin: PanelOrigin::TestPattern,
        ..panel
    };
    let pixels = vec![WHITE; pixel_count(&description)];
    (
        PatternPanel {
            description,
            buttons: buttons.len(),
            shared: Rc::clone(&shared),
            pixels,
            generation: 0,
            second: None,
        },
        PatternButtons {
            specs: buttons,
            shared,
        },
    )
}

fn pixel_count(description: &PanelDescription) -> usize {
    usize::try_from(u64::from(description.width) * u64::from(description.height))
        .unwrap_or(usize::MAX)
}

/// The panel of the pattern.
struct PatternPanel {
    description: PanelDescription,
    buttons: usize,
    shared: Rc<RefCell<Shared>>,
    pixels: Vec<u8>,
    generation: u64,
    /// The second of device time the bar shows; `None` before the first refresh.
    second: Option<u64>,
}

impl PatternPanel {
    fn width(&self) -> u32 {
        self.description.width
    }

    fn height(&self) -> u32 {
        self.description.height
    }

    /// Paints the rectangle with `value`, clipped to the panel.
    fn fill(&mut self, region: Region, value: u8) {
        let (width, height) = (self.width(), self.height());
        let right = region.x.saturating_add(region.width).min(width);
        let bottom = region.y.saturating_add(region.height).min(height);
        for y in region.y.min(bottom)..bottom {
            let line = usize::try_from(u64::from(y) * u64::from(width)).unwrap_or(usize::MAX);
            let start = line.saturating_add(region.x.min(right) as usize);
            let end = line.saturating_add(right as usize);
            if let Some(span) = self.pixels.get_mut(start..end) {
                span.fill(value);
            }
        }
    }

    fn level(&self, level: u8) -> u8 {
        PanelFrame::level_value(level, self.description.gray_levels)
    }

    fn draw_background(&mut self) {
        let (width, height) = (self.width(), self.height());
        self.pixels.fill(WHITE);
        let whole = Region {
            x: 0,
            y: 0,
            width,
            height,
        };
        self.fill(whole, BLACK);
        self.fill(
            Region {
                x: BORDER,
                y: BORDER,
                width: width.saturating_sub(2 * BORDER),
                height: height.saturating_sub(2 * BORDER),
            },
            WHITE,
        );
        // Four bands, one per level, black to white, across the top half.
        let levels = self.description.gray_levels.max(2);
        let inner = width.saturating_sub(2 * MARGIN);
        let band = inner / u32::from(levels);
        for level in 0..levels {
            let value = self.level(level);
            self.fill(
                Region {
                    x: MARGIN + band * u32::from(level),
                    y: MARGIN,
                    width: band,
                    height: height / 3,
                },
                value,
            );
        }
        // A checkerboard of one-pixel squares under the bands.
        let top = MARGIN + height / 3 + MARGIN / 2;
        for y in top..(top + MARGIN).min(height) {
            for x in MARGIN..width.saturating_sub(MARGIN) {
                if (x + y) % 2 == 0 {
                    let index = u64::from(y) * u64::from(width) + u64::from(x);
                    if let Some(pixel) = usize::try_from(index)
                        .ok()
                        .and_then(|index| self.pixels.get_mut(index))
                    {
                        *pixel = BLACK;
                    }
                }
            }
        }
    }

    fn squares_region(&self) -> Region {
        let count = u32::try_from(self.buttons).unwrap_or(u32::MAX);
        Region {
            x: MARGIN,
            y: self
                .height()
                .saturating_sub(2 * MARGIN + BAR_HEIGHT + SQUARE),
            width: count.saturating_mul(SQUARE_PITCH),
            height: SQUARE,
        }
    }

    fn draw_squares(&mut self) {
        let area = self.squares_region();
        self.fill(area, WHITE);
        let pressed = self.shared.borrow().pressed;
        for place in 0..self.buttons.min(64) {
            let offset = u32::try_from(place).unwrap_or(0) * SQUARE_PITCH;
            let square = Region {
                x: area.x + offset,
                y: area.y,
                width: SQUARE,
                height: SQUARE,
            };
            self.fill(square, BLACK);
            if pressed & (1 << place) == 0 {
                self.fill(
                    Region {
                        x: square.x + 2,
                        y: square.y + 2,
                        width: SQUARE - 4,
                        height: SQUARE - 4,
                    },
                    WHITE,
                );
            }
        }
    }

    fn bar_region(&self) -> Region {
        Region {
            x: MARGIN,
            y: self.height().saturating_sub(MARGIN + BAR_HEIGHT),
            width: self.width().saturating_sub(2 * MARGIN),
            height: BAR_HEIGHT,
        }
    }

    fn draw_bar(&mut self, second: u64) {
        let area = self.bar_region();
        self.fill(area, BLACK);
        self.fill(
            Region {
                x: area.x + 2,
                y: area.y + 2,
                width: area.width.saturating_sub(4),
                height: area.height.saturating_sub(4),
            },
            WHITE,
        );
        let filled = second % BAR_STEPS + 1;
        let width = u32::try_from(u64::from(area.width.saturating_sub(4)) * filled / BAR_STEPS)
            .unwrap_or(0);
        let gray = self.level(1);
        self.fill(
            Region {
                x: area.x + 2,
                y: area.y + 2,
                width,
                height: area.height.saturating_sub(4),
            },
            gray,
        );
    }

    fn frame(&mut self, region: Option<Region>) -> PanelFrame {
        self.generation += 1;
        let full = region.is_none() || self.generation % FULL_EVERY == 1;
        PanelFrame {
            width: self.width(),
            height: self.height(),
            gray_levels: self.description.gray_levels,
            generation: self.generation,
            refresh: if full {
                RefreshKind::Full
            } else {
                RefreshKind::Partial
            },
            region: if full { None } else { region },
            duration_nanos: Some(if full {
                FULL_REFRESH_NANOS
            } else {
                PARTIAL_REFRESH_NANOS
            }),
            pixels: Arc::from(self.pixels.as_slice()),
        }
    }
}

/// The smallest rectangle holding both, or the one there is.
fn union(one: Option<Region>, other: Region) -> Region {
    let Some(one) = one else {
        return other;
    };
    let x = one.x.min(other.x);
    let y = one.y.min(other.y);
    let right = (one.x + one.width).max(other.x + other.width);
    let bottom = (one.y + one.height).max(other.y + other.height);
    Region {
        x,
        y,
        width: right - x,
        height: bottom - y,
    }
}

impl PanelSource for PatternPanel {
    fn describe(&self) -> PanelDescription {
        self.description
    }

    fn poll(&mut self, machine: &mut Esp32C3) -> Option<PanelFrame> {
        self.poll_at(machine.device_time_nanos())
    }
}

impl PatternPanel {
    /// [`PanelSource::poll`] at `device_nanos` of device time.
    fn poll_at(&mut self, device_nanos: u64) -> Option<PanelFrame> {
        let second = device_nanos / NANOS_PER_SECOND;
        let Some(shown) = self.second else {
            self.second = Some(second);
            self.draw_background();
            self.draw_squares();
            self.draw_bar(second);
            self.shared.borrow_mut().changed = false;
            return Some(self.frame(None));
        };
        let mut region = None;
        if self.shared.borrow().changed {
            self.shared.borrow_mut().changed = false;
            self.draw_squares();
            region = Some(self.squares_region());
        }
        if second != shown {
            self.second = Some(second);
            self.draw_bar(second);
            region = Some(union(region, self.bar_region()));
        }
        // A full refresh draws everything again, as a clearing refresh would.
        if region.is_some() && (self.generation + 1) % FULL_EVERY == 1 {
            self.draw_background();
            self.draw_squares();
            self.draw_bar(second);
        }
        region.map(|region| self.frame(Some(region)))
    }
}

/// The buttons of the pattern: they reach the pattern, not the firmware.
struct PatternButtons {
    specs: &'static [ButtonSpec],
    shared: Rc<RefCell<Shared>>,
}

impl ButtonPort for PatternButtons {
    fn buttons(&self) -> &[ButtonSpec] {
        self.specs
    }

    fn reaches_firmware(&self) -> bool {
        false
    }

    fn set(&mut self, _machine: &mut Esp32C3, id: &str, pressed: bool) -> Result<(), ButtonError> {
        let place = self
            .specs
            .iter()
            .position(|spec| spec.id == id)
            .ok_or_else(|| ButtonError::Unknown { id: id.to_owned() })?;
        let bit = 1u64 << place.min(63);
        let mut shared = self.shared.borrow_mut();
        let before = shared.pressed;
        if pressed {
            shared.pressed |= bit;
        } else {
            shared.pressed &= !bit;
        }
        shared.changed |= shared.pressed != before;
        Ok(())
    }
}

/// A panel and buttons for the tests of this crate.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::ports::{Edge, Rotation};

    pub(crate) const BUTTONS: [ButtonSpec; 2] = [
        ButtonSpec {
            id: "power",
            edge: Edge::Top,
            order: 0,
        },
        ButtonSpec {
            id: "confirm",
            edge: Edge::Bottom,
            order: 0,
        },
    ];

    pub(crate) const PANEL: PanelDescription = PanelDescription {
        width: 200,
        height: 120,
        gray_levels: 4,
        rotation: Rotation::Deg90,
        origin: PanelOrigin::Device,
    };
}

#[cfg(test)]
mod tests {
    use super::fixtures::{BUTTONS, PANEL};
    use super::*;
    use crate::ports::Rotation;
    use soc_esp32c3::SocConfig;
    use soc_esp32c3::test_support::{HANG, flat_rom};

    fn machine() -> Esp32C3 {
        Esp32C3::new(flat_rom(&[HANG]), &SocConfig::default())
    }

    #[test]
    fn the_pattern_says_it_is_a_pattern() {
        let (panel, buttons) = ports(PANEL, &BUTTONS);
        assert_eq!(panel.describe().origin, PanelOrigin::TestPattern);
        assert_eq!(panel.describe().rotation, Rotation::Deg90);
        assert!(!buttons.reaches_firmware());
        assert_eq!(buttons.buttons(), BUTTONS);
    }

    /// The first poll is a full refresh of generation 1 holding every level; then nothing until
    /// something changes. The port reads the device time of the machine it is given.
    #[test]
    fn the_first_image_is_a_full_refresh_with_every_level() {
        let mut machine = machine();
        let (mut panel, _) = ports(PANEL, &BUTTONS);
        let frame = panel.poll(&mut machine).expect("a first image");
        assert_eq!(frame.generation, 1);
        assert_eq!(frame.refresh, RefreshKind::Full);
        assert_eq!(frame.region, None);
        assert_eq!(frame.duration_nanos, Some(FULL_REFRESH_NANOS));
        assert_eq!(frame.pixels.len(), 200 * 120);
        for value in [0u8, 85, 170, 255] {
            assert!(frame.pixels.contains(&value), "{value}");
        }
        assert!(
            frame.pixels.iter().all(|p| [0, 85, 170, 255].contains(p)),
            "every pixel lies on a level"
        );
        assert_eq!(panel.poll(&mut machine), None);
    }

    /// A pressed button is a partial refresh of the squares, its square filled; a release is
    /// another; an unknown name is refused and changes nothing.
    #[test]
    fn a_button_is_a_partial_refresh_of_its_square() {
        let mut machine = machine();
        let (mut panel, mut buttons) = ports(PANEL, &BUTTONS);
        let first = panel.poll(&mut machine).unwrap();
        buttons.set(&mut machine, "confirm", true).unwrap();
        let pressed = panel.poll(&mut machine).expect("the press shows");
        assert_eq!(pressed.generation, 2);
        assert_eq!(pressed.refresh, RefreshKind::Partial);
        let region = pressed.region.expect("a partial refresh names its region");
        // The middle of the second square: white before, black now.
        let x = region.x + SQUARE_PITCH + SQUARE / 2;
        let y = region.y + SQUARE / 2;
        let at = (y * 200 + x) as usize;
        assert_eq!((first.pixels[at], pressed.pixels[at]), (WHITE, BLACK));
        // Pressing it again changes nothing.
        buttons.set(&mut machine, "confirm", true).unwrap();
        assert_eq!(panel.poll(&mut machine), None);
        buttons.set(&mut machine, "confirm", false).unwrap();
        let released = panel.poll(&mut machine).unwrap();
        assert_eq!(released.pixels[at], WHITE);
        assert_eq!(
            buttons.set(&mut machine, "home", true),
            Err(ButtonError::Unknown { id: "home".into() })
        );
        assert_eq!(panel.poll(&mut machine), None);
    }

    /// Each second of device time is a refresh of the bar, and every tenth refresh is full.
    #[test]
    fn device_time_fills_the_bar_and_every_tenth_refresh_is_full() {
        let (mut panel, _) = pattern(PANEL, &BUTTONS);
        panel.poll_at(0).unwrap();
        assert_eq!(panel.poll_at(NANOS_PER_SECOND - 1), None);
        let mut kinds = Vec::new();
        for second in 1..=10u64 {
            let frame = panel
                .poll_at(second * NANOS_PER_SECOND)
                .expect("a new second is a refresh");
            assert_eq!(frame.generation, second + 1);
            kinds.push(frame.refresh);
        }
        let full: Vec<u64> = kinds
            .iter()
            .zip(2u64..)
            .filter(|(kind, _)| **kind == RefreshKind::Full)
            .map(|(_, generation)| generation)
            .collect();
        assert_eq!(full, [11], "generation 1, 11, 21… are full");
    }
}
