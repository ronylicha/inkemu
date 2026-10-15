//! The glass of an e-paper panel, whatever controller drives it (spec §5.3; ADR 0015): its optics,
//! the image published after a refresh, and the arithmetic that moves a pixel's optical state.
//!
//! A pixel's optical state lies between black (0) and white (the full swing). A phase of a
//! waveform holds a voltage for some time; the state moves in proportion to the voltage and the
//! time, and stops at black or white: a clamped translation `x ↦ min(max(x + a, lo), hi)`
//! (`Transfer`). A sequence of them is again one, so a whole waveform is one translation per
//! drive. The SSD1677 of the X4 and the UC8253 of the X3 read their waveforms differently and share
//! everything here.

use emu_core::Cycles;

/// Where a set of parameters comes from, for an interface to say whether it shows a measured
/// panel (spec §1.3 point 4, §5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provenance {
    /// Built for the model where no source gives the panel's own values: shown as "approximation".
    Approximation,
    /// Measured on a device, or read from its OTP.
    Measured,
}

const MILLISECOND: u64 = 1_000_000;

/// How the glass answers the waveform (spec §5.3, research `sp2a-external-chips.md` §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Optics {
    /// The defaults are not measured: [`Provenance::Approximation`].
    pub provenance: Provenance,
    /// The drive a pixel needs to go from black to white, in millivolt-nanoseconds (the pixel's
    /// voltage times the time it is applied). ASSUMPTION: 6 V·s by default; not measured.
    pub full_swing: u64,
    /// A positive pixel voltage (source above VCOM) darkens the pixel. True on the X4: its factory
    /// grey LUT only ends on the right colours that way; true on the X3, whose drivers drive VDH
    /// toward black and VDL toward white (research `sp3-x3-chips.md` §2.5).
    pub positive_drives_black: bool,
    /// The grey level of every pixel and of the border at power-on, 0 black to 255 white.
    pub initial_gray: u8,
}

impl Default for Optics {
    fn default() -> Self {
        Optics {
            provenance: Provenance::Approximation,
            full_swing: 6_000 * MILLISECOND * 1_000,
            positive_drives_black: true,
            initial_gray: 255,
        }
    }
}

/// What a published image comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefreshKind {
    /// The glass at power-on, before any refresh (generation 0).
    PowerOn,
    /// A refresh that drives every pixel by its waveform: on the X4, display mode 1 (`0xF7`,
    /// `0xD7`); on the X3, a refresh of the whole active area.
    Full,
    /// A refresh of part of the glass, or a differential one: on the X4, display mode 2 (`0xFC`,
    /// `0xCC`); on the X3, a refresh in partial mode (`PTIN`).
    Partial,
}

/// The physical panel after a refresh: what the glass shows, not what the RAM holds.
///
/// The grey levels come from a model of the glass, not from a measurement: each pixel moves in
/// proportion to its voltage and the time it is applied, and stops at black or white. Its
/// parameters (the drive of a full swing, the frame periods, the waveforms) are not measured on a
/// device; the controller's configuration says so (`is_approximation`), and an interface showing
/// this image must say "approximation" while it is true (spec §5.3). Which pixels a refresh drives,
/// and whether they end black or white, follow the controller's documented behaviour; the
/// intermediate greys and the trace of partial refreshes are the model's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelImage {
    pub width: u16,
    pub height: u16,
    /// Row by row from the top-left corner of the glass, one byte a pixel: 0 black, 255 white.
    pub pixels: Vec<u8>,
    /// The border (VBD), 0 black to 255 white.
    pub border: u8,
    /// 0 at power-on, then one more for every refresh published.
    pub generation: u64,
    pub kind: RefreshKind,
    /// A hardware reset cut the display short: the image is the state the glass had reached.
    pub interrupted: bool,
    /// The instant the display began driving the glass (`Cycles::ZERO` at power-on).
    pub started: Cycles,
    /// The instant the display ended (or was cut).
    pub at: Cycles,
}

/// `x ↦ min(max(x + add, lo), hi)`, with `lo ≤ hi`: what a run of phases does to an optical state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Transfer {
    add: i64,
    lo: i64,
    hi: i64,
}

impl Transfer {
    /// Leaves every state of `[0, swing]` where it is.
    pub(crate) fn identity(swing: i64) -> Transfer {
        Transfer {
            add: 0,
            lo: 0,
            hi: swing,
        }
    }

    /// `self`, then `next`. `clamp(clamp(z, A, B), C, D)` is `clamp(z, clamp(A, C, D), clamp(B, C,
    /// D))` for `A ≤ B` and `C ≤ D`, so the composition keeps the form.
    pub(crate) fn then(self, next: Transfer) -> Transfer {
        Transfer {
            add: self.add.saturating_add(next.add),
            lo: self.lo.saturating_add(next.add).clamp(next.lo, next.hi),
            hi: self.hi.saturating_add(next.add).clamp(next.lo, next.hi),
        }
    }

    /// Moves every state by `delta`, stopping at 0 and `swing`.
    pub(crate) fn shift(delta: i64, swing: i64) -> Transfer {
        Transfer {
            add: delta,
            lo: 0,
            hi: swing,
        }
    }

    pub(crate) fn apply(self, state: i64) -> i64 {
        state.saturating_add(self.add).clamp(self.lo, self.hi)
    }
}

/// The optical state of grey level `gray`.
pub(crate) fn level(gray: u8, swing: i64) -> i64 {
    let scaled = i128::from(swing) * i128::from(gray) / 255;
    i64::try_from(scaled).unwrap_or(swing)
}

/// The grey level of optical state `state`, rounded to the nearest (half up).
pub(crate) fn gray(state: i64, swing: i64) -> u8 {
    let scaled = (2 * i128::from(state) * 255 + i128::from(swing)) / (2 * i128::from(swing));
    u8::try_from(scaled.clamp(0, 255)).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_levels_round_both_ways() {
        let swing = 6_000;
        assert_eq!(gray(0, swing), 0);
        assert_eq!(gray(swing, swing), 255);
        assert_eq!(gray(swing / 2, swing), 128);
        assert_eq!(level(255, swing), swing);
        assert_eq!(level(0, swing), 0);
        for g in [0u8, 1, 77, 128, 254, 255] {
            assert_eq!(gray(level(g, swing), swing), g);
        }
    }

    #[test]
    fn a_transfer_composes_and_clamps() {
        let swing = 100;
        let up = Transfer::shift(30, swing);
        let down = Transfer::shift(-80, swing);
        for start in [0, 10, 50, 90, 100] {
            let step = down.apply(up.apply(start));
            assert_eq!(up.then(down).apply(start), step, "{start}");
        }
        // Down to black from anywhere, then up 30: always 30.
        let flash = Transfer::shift(-200, swing).then(Transfer::shift(30, swing));
        assert_eq!(flash.apply(0), 30);
        assert_eq!(flash.apply(100), 30);
        assert_eq!(Transfer::identity(swing).apply(42), 42);
    }
}
