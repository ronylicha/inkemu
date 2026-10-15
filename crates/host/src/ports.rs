//! Where the panel image and the buttons of a device plug into the engine (design of sub-project
//! 2c, §4; ADR 0011).
//!
//! A front end shows what a [`PanelSource`] gives and presses what a [`ButtonPort`] offers; it
//! knows nothing of the chips behind them. A wired device (the X4, sub-project 2d) implements
//! both with its display controller and its board ([`crate::ports_wired`]); a device not wired
//! yet with a test pattern ([`crate::pattern`]); [`crate::catalog::ports_of`] is the one place
//! that chooses, per device.
//!
//! **Threads.** The ports of a life are made with its machine by a [`Recipe`], on the thread of the
//! emulator, and die there: they need not be `Send`, so a chip may share an `Rc` with the chip
//! model that drives it.
//!
//! **When they are called.** Between two passes of the machine, after a step and after an input,
//! always with the machine stopped between two instructions, and with the machine itself, so that
//! a port that reads or drives the chip does it at the present instant of the device.
use soc_esp32c3::Esp32C3;
use std::sync::Arc;

/// How the panel is turned in the device: the clockwise rotation that brings the controller's
/// native scan order to what a user holding the device sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(into = "u16")]
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

    /// Whether the rotation swaps width and height.
    #[must_use]
    pub const fn swaps_sides(self) -> bool {
        matches!(self, Rotation::Deg90 | Rotation::Deg270)
    }
}

impl From<Rotation> for u16 {
    fn from(rotation: Rotation) -> u16 {
        rotation.degrees()
    }
}

/// Where the image of a panel comes from: what an interface must say about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PanelOrigin {
    /// A test pattern drawn by inkemu: the firmware's display is not wired yet.
    TestPattern,
    /// The display controller the firmware drives.
    Device,
}

/// The greys a panel shows (design of sub-project 2d, 2d-D17).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Shades {
    /// This many levels, evenly spaced from black to white (2 at least): a test pattern.
    Levels(u8),
    /// Any value from 0 (black) to 255 (white): the optics of a controller's model, whose greys
    /// rounding to a few levels would merge.
    Continuous,
}

/// What a panel is, constant for a life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelDescription {
    /// Pixels per line, in the controller's native scan order.
    pub width: u32,
    /// Lines, in the controller's native scan order.
    pub height: u32,
    pub shades: Shades,
    pub rotation: Rotation,
    pub origin: PanelOrigin,
    /// The image rests on optical or timing parameters nobody measured on a device: an interface
    /// says so under the panel (2d-D18).
    pub approximate: bool,
}

/// How a refresh changed the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefreshKind {
    /// The whole panel went through black and white before showing the image: the flash a user
    /// sees on an e-ink device.
    Full,
    /// Only pixels that change were driven, without a flash.
    Partial,
}

/// A rectangle of the panel, in native scan order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What a [`PanelSource`] says a refresh shows, before [`PanelFrame::new`] checks it.
///
/// `pixels` holds `width × height` bytes in native scan order, line after line: 0 is black, 255
/// white; with [`Shades::Levels`] every value lies on one of the evenly spaced levels (four
/// levels: 0, 85, 170, 255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameParts {
    pub width: u32,
    pub height: u32,
    pub shades: Shades,
    /// Strictly increasing from 1 over a life: one per completed refresh.
    pub generation: u64,
    pub refresh: RefreshKind,
    /// The rectangle the refresh drove; `None` for the whole panel.
    pub region: Option<Region>,
    /// How long the refresh lasts in device time, when the controller's model knows it.
    pub duration_nanos: Option<u64>,
    pub pixels: Arc<[u8]>,
}

/// Why the parts of a refresh are not an image a panel can show: a fault of the port that gave
/// them, which the engine says in the journal while the panel keeps its last image.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("a panel image of {width} × {height} pixels shows nothing")]
    Empty { width: u32, height: u32 },
    #[error("a {width} × {height} panel image holds {len} pixels")]
    Pixels { width: u32, height: u32, len: usize },
    #[error("a panel image of {0} grey levels: a panel shows 2 levels at least")]
    Levels(u8),
    #[error("a panel image of generation 0: the refreshes of a life count from 1")]
    Generation,
    #[error(
        "the region at ({x}, {y}) of {region_width} × {region_height} pixels is not within the {width} × {height} panel"
    )]
    Region {
        x: u32,
        y: u32,
        region_width: u32,
        region_height: u32,
        width: u32,
        height: u32,
    },
}

/// The image a panel shows after a refresh that completed: [`FrameParts`] that
/// [`PanelFrame::new`] found consistent, so that an interface can draw and encode it without
/// checking it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelFrame {
    parts: FrameParts,
}

/// The first bytes of an encoded [`PanelFrame`].
pub const FRAME_MAGIC: [u8; 4] = *b"IKPF";
/// The version of the encoding that follows [`FRAME_MAGIC`]: 2 says continuous shades by a 0 at
/// byte 6 (design of sub-project 2d, 2d-D17).
pub const FRAME_VERSION: u8 = 2;
/// The bytes before the pixels of an encoded frame.
pub const FRAME_HEADER_LEN: usize = 48;

impl PanelFrame {
    /// Checks `parts`: a panel of at least one pixel, exactly `width × height` pixels, two grey
    /// levels at least when they are levels, a generation counted from 1, and a region within the
    /// panel, not empty.
    ///
    /// # Errors
    /// [`FrameError`] naming the first part that is not consistent.
    pub fn new(parts: FrameParts) -> Result<PanelFrame, FrameError> {
        let (width, height) = (parts.width, parts.height);
        if width == 0 || height == 0 {
            return Err(FrameError::Empty { width, height });
        }
        let expected = usize::try_from(u64::from(width) * u64::from(height)).ok();
        if expected != Some(parts.pixels.len()) {
            return Err(FrameError::Pixels {
                width,
                height,
                len: parts.pixels.len(),
            });
        }
        if let Shades::Levels(levels) = parts.shades
            && levels < 2
        {
            return Err(FrameError::Levels(levels));
        }
        if parts.generation == 0 {
            return Err(FrameError::Generation);
        }
        if let Some(region) = parts.region {
            let within = |start: u32, length: u32, side: u32| {
                length > 0 && u64::from(start) + u64::from(length) <= u64::from(side)
            };
            if !within(region.x, region.width, width) || !within(region.y, region.height, height) {
                return Err(FrameError::Region {
                    x: region.x,
                    y: region.y,
                    region_width: region.width,
                    region_height: region.height,
                    width,
                    height,
                });
            }
        }
        Ok(PanelFrame { parts })
    }

    /// Pixels per line, in the controller's native scan order.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.parts.width
    }

    /// Lines, in the controller's native scan order.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.parts.height
    }

    #[must_use]
    pub const fn shades(&self) -> Shades {
        self.parts.shades
    }

    /// Strictly increasing from 1 over a life: one per completed refresh.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.parts.generation
    }

    #[must_use]
    pub const fn refresh(&self) -> RefreshKind {
        self.parts.refresh
    }

    /// The rectangle the refresh drove; `None` for the whole panel.
    #[must_use]
    pub const fn region(&self) -> Option<Region> {
        self.parts.region
    }

    /// How long the refresh lasts in device time, when the controller's model knows it.
    #[must_use]
    pub const fn duration_nanos(&self) -> Option<u64> {
        self.parts.duration_nanos
    }

    /// `width × height` bytes, line after line.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.parts.pixels
    }

    /// The value of grey level `level` (0 is black) on a panel of `levels` levels.
    #[must_use]
    pub fn level_value(level: u8, levels: u8) -> u8 {
        let top = u16::from(levels.max(2) - 1);
        let level = u16::from(level).min(top);
        // At most 255 · top / top: the quotient fits a byte.
        u8::try_from(level * 255 / top).unwrap_or(u8::MAX)
    }

    /// The frame as bytes for an interface that cannot share memory with the host: a header of
    /// [`FRAME_HEADER_LEN`] bytes, little-endian, then the pixels.
    ///
    /// | Offset | Size | Field |
    /// |---|---|---|
    /// | 0 | 4 | [`FRAME_MAGIC`] |
    /// | 4 | 1 | [`FRAME_VERSION`] |
    /// | 5 | 1 | refresh: 0 full, 1 partial |
    /// | 6 | 1 | grey levels, 0 for continuous shades |
    /// | 7 | 1 | 1 when a region follows, else 0 |
    /// | 8 | 4 | width |
    /// | 12 | 4 | height |
    /// | 16 | 8 | generation |
    /// | 24 | 8 | duration in nanoseconds, `u64::MAX` when unknown |
    /// | 32 | 16 | region: x, y, width, height (zeros when none) |
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let parts = &self.parts;
        let mut bytes = Vec::with_capacity(FRAME_HEADER_LEN + parts.pixels.len());
        bytes.extend_from_slice(&FRAME_MAGIC);
        bytes.push(FRAME_VERSION);
        bytes.push(match parts.refresh {
            RefreshKind::Full => 0,
            RefreshKind::Partial => 1,
        });
        bytes.push(match parts.shades {
            Shades::Levels(levels) => levels,
            Shades::Continuous => 0,
        });
        bytes.push(u8::from(parts.region.is_some()));
        bytes.extend_from_slice(&parts.width.to_le_bytes());
        bytes.extend_from_slice(&parts.height.to_le_bytes());
        bytes.extend_from_slice(&parts.generation.to_le_bytes());
        bytes.extend_from_slice(&parts.duration_nanos.unwrap_or(u64::MAX).to_le_bytes());
        let region = parts.region.unwrap_or(Region {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
        for value in [region.x, region.y, region.width, region.height] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&parts.pixels);
        bytes
    }
}

/// The image of a device's panel.
pub trait PanelSource {
    /// What the panel is; the same value for the whole life.
    fn describe(&self) -> PanelDescription;

    /// The image of the last refresh that completed since the previous call, if any; `None` when
    /// no refresh completed. Several refreshes between two calls give only the last one.
    ///
    /// # Errors
    /// [`FrameError`] when the refresh gave parts that are not an image ([`PanelFrame::new`]):
    /// the engine says so in the journal and the panel keeps its last image.
    fn poll(&mut self, machine: &mut Esp32C3) -> Result<Option<PanelFrame>, FrameError>;
}

/// Where a button sits on a device, as its user sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// One button of a device.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonSpec {
    /// A stable name, the profile's: `power`, `back`, `confirm`, `left`, `right`, `up`, `down`,
    /// … An interface uses it as the key of its label.
    pub id: String,
    pub edge: Edge,
    /// The place of the button along its edge, from left to right or from top to bottom.
    pub order: u8,
}

/// Why a button was not pressed or released.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ButtonError {
    #[error("the device has no button named `{id}`")]
    Unknown { id: String },
    /// The machine refused the input of the board (the device is off, too many inputs wait).
    #[error("the button `{id}` cannot be pressed now: {detail}")]
    Refused { id: String, detail: String },
    /// A replay takes no input: it gives back what the session recorded.
    #[error("a replay takes no input")]
    Replaying,
}

/// The buttons of a device.
pub trait ButtonPort {
    /// The buttons, in a fixed order.
    fn buttons(&self) -> &[ButtonSpec];

    /// Whether pressing a button changes what the firmware sees. A port that does gives the
    /// machine a board input ([`Esp32C3::send_board_input`]), which its journal keeps: a session
    /// holds it (format 2, design of sub-project 2d, 2d-D19).
    fn reaches_firmware(&self) -> bool;

    /// Presses (`pressed`) or releases the button `id` at the present instant of `machine`.
    ///
    /// # Errors
    /// [`ButtonError::Unknown`] for a name not in [`ButtonPort::buttons`],
    /// [`ButtonError::Refused`] when the machine refuses the input.
    fn set(&mut self, machine: &mut Esp32C3, id: &str, pressed: bool) -> Result<(), ButtonError>;
}

/// A machine with its ports, from power-on.
pub struct Life {
    pub machine: Esp32C3,
    pub panel: Box<dyn PanelSource>,
    pub buttons: Box<dyn ButtonPort>,
    /// The SD card slot of the device, from which the card is taken back when the life ends;
    /// `None` for a device without one.
    pub sd: Option<boards::SdHandle>,
}

impl std::fmt::Debug for Life {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Life")
            .field("panel", &self.panel.describe())
            .field("buttons", &self.buttons.buttons())
            .finish_non_exhaustive()
    }
}

/// Why a life could not begin.
#[derive(Debug, thiserror::Error)]
pub enum PowerOnError {
    /// The device cannot be powered on with its wiring: a flash content longer than its flash, a
    /// card its slot cannot take, an option it has no line for.
    #[error("{0}")]
    Device(#[from] boards::WiredError),
    /// A session cannot be replayed on this ROM or this device; the message says why and what to
    /// do.
    #[error("{0}")]
    Replay(String),
}

/// How the emulator powers a device on, on its own thread: given once, used at every power-on.
pub trait Recipe: Send {
    /// A machine in its power-on state with `flash` from offset 0 (`None`: erased), `sd` the
    /// card in its slot (`None`: empty), and its ports. `recorded`: the life is recorded, and the
    /// machine keeps the trace hash a session holds.
    ///
    /// # Errors
    /// [`PowerOnError`] when the flash content or the card does not fit the device.
    fn power_on(
        &mut self,
        flash: Option<Vec<u8>>,
        recorded: bool,
        sd: Option<boards::SdStart>,
    ) -> Result<Life, PowerOnError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_levels_are_evenly_spaced_from_black_to_white() {
        let four: Vec<u8> = (0..4)
            .map(|level| PanelFrame::level_value(level, 4))
            .collect();
        assert_eq!(four, [0, 85, 170, 255]);
        assert_eq!(PanelFrame::level_value(1, 2), 255);
        assert_eq!(
            PanelFrame::level_value(9, 4),
            255,
            "a level past the top is white"
        );
    }

    #[test]
    fn a_rotation_names_its_degrees_and_whether_it_swaps_the_sides() {
        let all = [
            Rotation::Deg0,
            Rotation::Deg90,
            Rotation::Deg180,
            Rotation::Deg270,
        ];
        assert_eq!(all.map(Rotation::degrees), [0, 90, 180, 270]);
        assert_eq!(all.map(Rotation::swaps_sides), [false, true, false, true]);
    }

    /// The interface decodes this layout: a change here is a change of the version.
    #[test]
    fn an_encoded_frame_is_its_header_then_its_pixels() {
        let frame = PanelFrame::new(FrameParts {
            width: 3,
            height: 2,
            shades: Shades::Levels(4),
            generation: 0x0102_0304_0506_0708,
            refresh: RefreshKind::Partial,
            region: Some(Region {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
            }),
            duration_nanos: None,
            pixels: Arc::from([0u8, 85, 170, 255, 0, 85].as_slice()),
        })
        .unwrap();
        let bytes = frame.encode();
        assert_eq!(bytes.len(), FRAME_HEADER_LEN + 6);
        assert_eq!(&bytes[..4], b"IKPF");
        assert_eq!(bytes[4..8], [2, 1, 4, 1]);
        assert_eq!(bytes[8..12], 3u32.to_le_bytes());
        assert_eq!(bytes[12..16], 2u32.to_le_bytes());
        assert_eq!(bytes[16..24], 0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(bytes[24..32], u64::MAX.to_le_bytes());
        assert_eq!(bytes[32..36], 1u32.to_le_bytes());
        assert_eq!(bytes[40..44], 2u32.to_le_bytes());
        assert_eq!(bytes[44..48], 1u32.to_le_bytes());
        assert_eq!(&bytes[48..], [0, 85, 170, 255, 0, 85]);
    }

    #[test]
    fn a_full_refresh_without_a_region_encodes_zeros_there() {
        let frame = PanelFrame::new(FrameParts {
            width: 1,
            height: 1,
            shades: Shades::Levels(2),
            generation: 1,
            refresh: RefreshKind::Full,
            region: None,
            duration_nanos: Some(1_700_000_000),
            pixels: Arc::from([255u8].as_slice()),
        })
        .unwrap();
        let bytes = frame.encode();
        assert_eq!(bytes[5..8], [0, 2, 0]);
        assert_eq!(bytes[24..32], 1_700_000_000u64.to_le_bytes());
        assert_eq!(bytes[32..48], [0; 16]);
    }

    /// 2d-D17: the greys of a controller's optics are any byte, which version 2 says by a 0 for
    /// the levels; the pixels go as they are.
    #[test]
    fn a_frame_of_continuous_shades_encodes_0_levels_and_its_bytes_as_they_are() {
        let frame = PanelFrame::new(FrameParts {
            width: 2,
            height: 1,
            shades: Shades::Continuous,
            generation: 3,
            refresh: RefreshKind::Full,
            region: None,
            duration_nanos: None,
            pixels: Arc::from([153u8, 128].as_slice()),
        })
        .unwrap();
        assert_eq!(frame.shades(), Shades::Continuous);
        let bytes = frame.encode();
        assert_eq!(bytes[4..7], [2, 0, 0]);
        assert_eq!(&bytes[48..], [153, 128]);
    }

    /// Parts that are not an image are refused, each for its first fault: an interface never
    /// receives a frame whose pixels do not fill its sides.
    #[test]
    fn parts_that_are_not_an_image_are_refused() {
        let good = FrameParts {
            width: 3,
            height: 2,
            shades: Shades::Levels(2),
            generation: 1,
            refresh: RefreshKind::Partial,
            region: Some(Region {
                x: 1,
                y: 1,
                width: 2,
                height: 1,
            }),
            duration_nanos: None,
            pixels: Arc::from([0u8; 6].as_slice()),
        };
        assert!(PanelFrame::new(good.clone()).is_ok());
        let refused = |change: fn(&mut FrameParts)| {
            let mut parts = good.clone();
            change(&mut parts);
            PanelFrame::new(parts).unwrap_err()
        };
        assert_eq!(
            refused(|p| p.width = 0),
            FrameError::Empty {
                width: 0,
                height: 2
            }
        );
        assert_eq!(
            refused(|p| p.pixels = Arc::from([0u8; 5].as_slice())),
            FrameError::Pixels {
                width: 3,
                height: 2,
                len: 5
            }
        );
        assert_eq!(
            refused(|p| p.shades = Shades::Levels(1)),
            FrameError::Levels(1)
        );
        assert_eq!(refused(|p| p.generation = 0), FrameError::Generation);
        for region in [
            Region {
                x: 2,
                y: 0,
                width: 2,
                height: 1,
            },
            Region {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            Region {
                x: u32::MAX,
                y: 0,
                width: 1,
                height: 1,
            },
        ] {
            let mut parts = good.clone();
            parts.region = Some(region);
            assert!(
                matches!(PanelFrame::new(parts), Err(FrameError::Region { .. })),
                "{region:?}"
            );
        }
    }
}
