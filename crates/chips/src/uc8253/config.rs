//! The parameters of the X3's controller and of the glass it drives: what the sources leave open,
//! what belongs to the glass, and what no source gives (research `sp3-x3-chips.md` §2,
//! `sp3-x3-firmware.md` §3). The defaults are the X3's: a 792 × 528 panel behind the first batch's
//! controller.

use crate::glass::{Optics, Provenance};

/// Sources of the controller: the width of each RAM plane, in pixels (DS79: 800 × 600).
pub const RAM_SOURCES: u16 = 800;
/// Gates of the controller: the height of each RAM plane, in rows.
pub const RAM_GATES: u16 = 600;
/// Bytes of a waveform LUT the model reads in KW mode: seven groups of six bytes (DS79).
pub const LUT_BYTES: usize = 42;
/// The most bytes a LUT command takes: ten groups of six (DS79, LUTC and the KWR LUTs).
pub const LUT_MAX_BYTES: u16 = 60;
/// Frame-rate codes of `PLL` (`0x30`): five bits.
pub const FRAME_RATE_CODES: usize = 32;

const MICROSECOND: u64 = 1_000;
const MILLISECOND: u64 = 1_000_000;

/// Which controller of the X3's two batches the model plays (research `sp3-x3-chips.md` §1, §2.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    /// The first batch, the controller `FreeInk` calls UC8253: its commands have the layout of the
    /// UC8179 (DS79), it drives nothing on a read, and `CDI` takes two bytes.
    Uc8253,
    /// The later batch, a `UC8279d`: it answers `REV` with `0x66` in its third byte, the byte the
    /// firmwares select its driver by, and `CDI` takes one byte (DS53's layout).
    Uc8279d,
}

impl Variant {
    /// The name a profile and a diagnostic give it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Variant::Uc8253 => "UC8253",
            Variant::Uc8279d => "UC8279d",
        }
    }
}

/// How the glass's pixels sit on the controller's lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uc8253Geometry {
    /// Sources wired to the glass, from S0: its width.
    pub sources: u16,
    /// Gates wired to the glass, from G0: its height.
    pub gates: u16,
    /// The image's top row is the last gate (`G[gates - 1]`), its bottom row G0.
    pub rows_reversed: bool,
    /// The image's left column is the last source.
    pub columns_reversed: bool,
}

impl Uc8253Geometry {
    /// The X3's glass: 792 sources by 528 gates.
    ///
    /// ASSUMPTION: the glass's scan direction is not in any source (research `sp3-x3-firmware.md`
    /// §11 point 2). `FreeInk` writes each plane last row first, so that controller row `r` holds
    /// the portrait UI's column `r` (§3.4, §9): RAM is the transpose of the portrait image. Rows
    /// reversed (G0 at the bottom of the image) and a quarter turn clockwise in the case
    /// (`wiring.display.rotation = 90`) show the UI upright; columns reversed and a quarter turn
    /// the other way would show it the same. A photograph of an X3 running a known pattern settles
    /// which edge G0 is on.
    #[must_use]
    pub const fn xteink_x3() -> Uc8253Geometry {
        Uc8253Geometry {
            sources: 792,
            gates: 528,
            rows_reversed: true,
            columns_reversed: false,
        }
    }
}

impl Default for Uc8253Geometry {
    fn default() -> Self {
        Uc8253Geometry::xteink_x3()
    }
}

/// How long each operation keeps `BUSY_N` low, in nanoseconds of device time, and the frame period
/// of each `PLL` code. Every value no datasheet gives is an ASSUMPTION.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uc8253Timing {
    /// The defaults are sourced approximations and assumptions: [`Provenance::Approximation`].
    pub provenance: Provenance,
    /// `PON` until every supply is up: about 127 ms, measured by Papyrix on an X3 (research
    /// `sp3-x3-chips.md` §2.3).
    pub power_on: u64,
    /// `POF`. ASSUMPTION: 30 ms; no source measures it (§6 point 4).
    pub power_off: u64,
    /// `DSLP` with its check code. ASSUMPTION: 1 ms.
    pub deep_sleep: u64,
    /// A soft reset (`PSR` with `RST_N` 0): at least 50 µs (DS53).
    pub soft_reset: u64,
    /// After `RST_N` rises: "more than 1 ms" of initialisation, commands ignored (DS79).
    pub hardware_reset: u64,
    /// The period of one frame for each `FRS` code of `PLL`. ASSUMPTION: DS53's table, 5 Hz per
    /// step from 5 Hz at code 0 (code 9, the X3's, is 50 Hz; Papyrix's turbo refresh of 19 frames
    /// takes 382 ms, close to 20 ms a frame), its last code 200 Hz; DS79 gives 80 Hz for code 9
    /// (research `sp3-x3-chips.md` §2.5).
    pub frame_period: [u64; FRAME_RATE_CODES],
}

/// The frame period of `PLL` code `code` by DS53's table.
const fn ds53_frame_period(code: usize) -> u64 {
    let hertz = if code == FRAME_RATE_CODES - 1 {
        200
    } else {
        5 * (code as u64 + 1)
    };
    1_000 * MILLISECOND / hertz
}

impl Default for Uc8253Timing {
    fn default() -> Self {
        let mut frame_period = [0; FRAME_RATE_CODES];
        let mut code = 0;
        while code < FRAME_RATE_CODES {
            frame_period[code] = ds53_frame_period(code);
            code += 1;
        }
        Uc8253Timing {
            provenance: Provenance::Approximation,
            power_on: 127 * MILLISECOND,
            power_off: 30 * MILLISECOND,
            deep_sleep: MILLISECOND,
            soft_reset: 50 * MICROSECOND,
            hardware_reset: MILLISECOND,
            frame_period,
        }
    }
}

/// Everything a UC8253 model is built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uc8253Config {
    pub variant: Variant,
    pub panel: Uc8253Geometry,
    pub optics: Optics,
    pub timing: Uc8253Timing,
    /// What the internal sensor reads, in degrees Celsius (`TSC` of the `UC8279d`).
    pub ambient_celsius: i8,
}

impl Uc8253Config {
    /// The X3 of the first batch, at 25 °C.
    #[must_use]
    pub fn xteink_x3() -> Uc8253Config {
        Uc8253Config {
            variant: Variant::Uc8253,
            panel: Uc8253Geometry::xteink_x3(),
            optics: Optics::default(),
            timing: Uc8253Timing::default(),
            ambient_celsius: 25,
        }
    }

    /// The X3 of the later batch: the same glass behind a `UC8279d`.
    #[must_use]
    pub fn xteink_x3_uc8279d() -> Uc8253Config {
        Uc8253Config {
            variant: Variant::Uc8279d,
            ..Uc8253Config::xteink_x3()
        }
    }

    /// Whether any part of what the model shows rests on values not measured on a device: the
    /// optics or the durations. An interface showing the panel says "approximation" when it is
    /// true; it is true of the defaults.
    #[must_use]
    pub fn is_approximation(&self) -> bool {
        self.optics.provenance == Provenance::Approximation
            || self.timing.provenance == Provenance::Approximation
    }

    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        let panel = self.panel;
        if panel.sources == 0 || panel.gates == 0 {
            return Err(ConfigError::EmptyPanel);
        }
        if panel.sources > RAM_SOURCES || panel.gates > RAM_GATES {
            return Err(ConfigError::PanelTooLarge {
                sources: panel.sources,
                gates: panel.gates,
            });
        }
        if self.optics.full_swing == 0 || i64::try_from(self.optics.full_swing).is_err() {
            return Err(ConfigError::FullSwing);
        }
        if self.timing.frame_period.contains(&0) {
            return Err(ConfigError::FramePeriod);
        }
        Ok(())
    }
}

impl Default for Uc8253Config {
    fn default() -> Self {
        Uc8253Config::xteink_x3()
    }
}

/// Why a configuration is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "the panel has no source or no gate; give it the sources and gates its glass is wired to"
    )]
    EmptyPanel,
    #[error(
        "the panel has {sources} sources and {gates} gates, more than the controller's 800 × 600; give it the lines its glass is wired to"
    )]
    PanelTooLarge { sources: u16, gates: u16 },
    #[error(
        "the full swing of the optics is zero or out of range; give the drive that turns a pixel from black to white"
    )]
    FullSwing,
    #[error("a frame period is zero; give each PLL code a period")]
    FramePeriod,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_x3_is_792_by_528_with_its_frame_rates_and_approximate() {
        let config = Uc8253Config::xteink_x3();
        assert_eq!((config.panel.sources, config.panel.gates), (792, 528));
        assert_eq!(config.validate(), Ok(()));
        assert!(config.is_approximation());
        assert_eq!(config.timing.frame_period[9], 20 * MILLISECOND, "50 Hz");
        assert_eq!(config.timing.frame_period[0], 200 * MILLISECOND, "5 Hz");
        assert_eq!(config.timing.frame_period[31], 5 * MILLISECOND, "200 Hz");
        assert_eq!(Uc8253Config::xteink_x3_uc8279d().variant, Variant::Uc8279d);
        let mut measured = config.clone();
        measured.optics.provenance = Provenance::Measured;
        measured.timing.provenance = Provenance::Measured;
        assert!(!measured.is_approximation());
    }

    #[test]
    fn a_panel_the_controller_cannot_drive_is_refused() {
        let mut config = Uc8253Config::xteink_x3();
        config.panel.sources = 801;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::PanelTooLarge { sources: 801, .. })
        ));
        config.panel.sources = 0;
        assert_eq!(config.validate(), Err(ConfigError::EmptyPanel));
        let mut timing = Uc8253Config::xteink_x3();
        timing.timing.frame_period[3] = 0;
        assert_eq!(timing.validate(), Err(ConfigError::FramePeriod));
        let mut optics = Uc8253Config::xteink_x3();
        optics.optics.full_swing = 0;
        assert_eq!(optics.validate(), Err(ConfigError::FullSwing));
    }
}
