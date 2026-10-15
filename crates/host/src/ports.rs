//! Where the panel image and the buttons of a device plug into the engine (design of sub-project
//! 2c, §4; ADR 0010).
//!
//! A front end shows what a [`PanelSource`] gives and presses what a [`ButtonPort`] offers; it
//! knows nothing of the chips behind them. Until the display controller and the button ladder are
//! wired to the machine (sub-project 2d), [`crate::pattern`] implements both with a test pattern,
//! and [`crate::catalog::ports_for`] is the one place that chooses, per device.
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

/// What a panel is, constant for a life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelDescription {
    /// Pixels per line, in the controller's native scan order.
    pub width: u32,
    /// Lines, in the controller's native scan order.
    pub height: u32,
    /// How many grey levels the panel shows, evenly spaced from black to white (2 or 4).
    pub gray_levels: u8,
    pub rotation: Rotation,
    pub origin: PanelOrigin,
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

/// The image a panel shows after a refresh that completed.
///
/// `pixels` holds `width × height` bytes in native scan order, line after line: 0 is black, 255
/// white, and every value lies on one of the `gray_levels` evenly spaced levels (four levels: 0,
/// 85, 170, 255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelFrame {
    pub width: u32,
    pub height: u32,
    pub gray_levels: u8,
    /// Strictly increasing from 1 over a life: one per completed refresh.
    pub generation: u64,
    pub refresh: RefreshKind,
    /// The rectangle the refresh drove; `None` for the whole panel.
    pub region: Option<Region>,
    /// How long the refresh lasts in device time, when the controller's model knows it.
    pub duration_nanos: Option<u64>,
    pub pixels: Arc<[u8]>,
}

/// The first bytes of an encoded [`PanelFrame`].
pub const FRAME_MAGIC: [u8; 4] = *b"IKPF";
/// The version of the encoding that follows [`FRAME_MAGIC`].
pub const FRAME_VERSION: u8 = 1;
/// The bytes before the pixels of an encoded frame.
pub const FRAME_HEADER_LEN: usize = 48;

impl PanelFrame {
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
    /// | 6 | 1 | grey levels |
    /// | 7 | 1 | 1 when a region follows, else 0 |
    /// | 8 | 4 | width |
    /// | 12 | 4 | height |
    /// | 16 | 8 | generation |
    /// | 24 | 8 | duration in nanoseconds, `u64::MAX` when unknown |
    /// | 32 | 16 | region: x, y, width, height (zeros when none) |
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FRAME_HEADER_LEN + self.pixels.len());
        bytes.extend_from_slice(&FRAME_MAGIC);
        bytes.push(FRAME_VERSION);
        bytes.push(match self.refresh {
            RefreshKind::Full => 0,
            RefreshKind::Partial => 1,
        });
        bytes.push(self.gray_levels);
        bytes.push(u8::from(self.region.is_some()));
        bytes.extend_from_slice(&self.width.to_le_bytes());
        bytes.extend_from_slice(&self.height.to_le_bytes());
        bytes.extend_from_slice(&self.generation.to_le_bytes());
        bytes.extend_from_slice(&self.duration_nanos.unwrap_or(u64::MAX).to_le_bytes());
        let region = self.region.unwrap_or(Region {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
        for value in [region.x, region.y, region.width, region.height] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&self.pixels);
        bytes
    }
}

/// The image of a device's panel.
pub trait PanelSource {
    /// What the panel is; the same value for the whole life.
    fn describe(&self) -> PanelDescription;

    /// The image of the last refresh that completed since the previous call, if any; `None` when
    /// no refresh completed. Several refreshes between two calls give only the last one.
    fn poll(&mut self, machine: &mut Esp32C3) -> Option<PanelFrame>;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonSpec {
    /// A stable name: `power`, `back`, `confirm`, `left`, `right`, `up`, `down`, … An interface
    /// uses it as the key of its label.
    pub id: &'static str,
    pub edge: Edge,
    /// The place of the button along its edge, from left to right or from top to bottom.
    pub order: u8,
}

/// Why a button was not pressed or released.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ButtonError {
    #[error("the device has no button named `{id}`")]
    Unknown { id: String },
    /// A button that reaches the firmware cannot enter a session before its format holds buttons
    /// (format 2, sub-project 2d): pressing it would make the session replay something else.
    #[error(
        "the button `{id}` reaches the firmware and a session cannot hold it yet: stop recording to press it"
    )]
    NotInSession { id: String },
    /// A replay takes no input: it gives back what the session recorded.
    #[error("a replay takes no input")]
    Replaying,
}

/// The buttons of a device.
pub trait ButtonPort {
    /// The buttons, in a fixed order.
    fn buttons(&self) -> &[ButtonSpec];

    /// Whether pressing a button changes what the firmware sees. A port that does must journal
    /// its inputs in the machine; until sessions hold buttons, the engine refuses them while it
    /// records ([`ButtonError::NotInSession`]).
    fn reaches_firmware(&self) -> bool;

    /// Presses (`pressed`) or releases the button `id` at the present instant of `machine`.
    ///
    /// # Errors
    /// [`ButtonError::Unknown`] for a name not in [`ButtonPort::buttons`].
    fn set(&mut self, machine: &mut Esp32C3, id: &str, pressed: bool) -> Result<(), ButtonError>;
}

/// A machine with its ports, from power-on.
pub struct Life {
    pub machine: Esp32C3,
    pub panel: Box<dyn PanelSource>,
    pub buttons: Box<dyn ButtonPort>,
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
    /// The flash content is longer than the flash of the device.
    #[error("{0}")]
    Flash(#[from] soc_esp32c3::FlashImageError),
}

/// How the emulator powers a device on, on its own thread: given once, used at every power-on.
pub trait Recipe: Send {
    /// A machine in its power-on state with `flash` from offset 0 (`None`: erased), and its ports.
    ///
    /// # Errors
    /// [`PowerOnError`] when the flash content does not fit the device.
    fn power_on(&mut self, flash: Option<Vec<u8>>) -> Result<Life, PowerOnError>;
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
        let frame = PanelFrame {
            width: 3,
            height: 2,
            gray_levels: 4,
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
        };
        let bytes = frame.encode();
        assert_eq!(bytes.len(), FRAME_HEADER_LEN + 6);
        assert_eq!(&bytes[..4], b"IKPF");
        assert_eq!(bytes[4..8], [1, 1, 4, 1]);
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
        let frame = PanelFrame {
            width: 1,
            height: 1,
            gray_levels: 2,
            generation: 1,
            refresh: RefreshKind::Full,
            region: None,
            duration_nanos: Some(1_700_000_000),
            pixels: Arc::from([255u8].as_slice()),
        };
        let bytes = frame.encode();
        assert_eq!(bytes[5..8], [0, 2, 0]);
        assert_eq!(bytes[24..32], 1_700_000_000u64.to_le_bytes());
        assert_eq!(bytes[32..48], [0; 16]);
    }
}
