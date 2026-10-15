//! The glass: which RAM pixel each of its pixels shows, its optical state, and the image published
//! after each refresh (spec §5.3).

use emu_core::Cycles;

use super::config::{GateScan, Optics, PanelGeometry, RAM_GATES};
use super::ram::{Plane, Ram};
use super::waveform::{Drive, Waveform};

/// What a published image comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefreshKind {
    /// The glass at power-on, before any refresh (generation 0).
    PowerOn,
    /// A refresh in display mode 1: every pixel driven by its LUT (`0xF7`, `0xD7` on the X4).
    Full,
    /// A refresh in display mode 2: on the X4 the differential one (`0xFC`) and the custom-LUT one
    /// (`0xCC`).
    Partial,
}

/// The physical panel after a refresh: what the glass shows, not what the RAM holds.
///
/// The grey levels come from a model of the glass, not from a measurement: each pixel moves in
/// proportion to its voltage and the time it is applied, and stops at black or white. Its
/// parameters (the drive of a full swing, the frame periods, the waveforms of the OTP) are not
/// measured on a device; [`Ssd1677Config::is_approximation`](super::Ssd1677Config::is_approximation)
/// says so, and an interface showing this image must say "approximation" while it is true (spec
/// §5.3). Which pixels a refresh drives, and whether they end black or white, follow the
/// controller's documented behaviour; the intermediate greys and the trace of partial refreshes are
/// the model's.
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

/// The RAM row gate `gate` shows when `lines` gates are scanned with `scan` (datasheet Table 8-2):
/// `None` when the gate is not scanned, or when it would show a row past the RAM (a MUX the
/// firmware set beyond 680 lines with SM = 1 sends the upper half of the gates there): such a gate
/// is not driven.
pub(crate) fn ram_row_of_gate(gate: u16, scan: GateScan, lines: u16) -> Option<u16> {
    if gate >= lines {
        return None;
    }
    let half = lines.div_ceil(2);
    let even = gate.is_multiple_of(2);
    let row = match (scan.sm, scan.gd) {
        (false, false) => gate,
        (false, true) => gate ^ 1,
        (true, false) if even => gate / 2,
        (true, false) => half + gate / 2,
        (true, true) if even => half + gate / 2,
        (true, true) => gate / 2,
    };
    (row < lines && row < RAM_GATES).then_some(row)
}

/// The RAM-plane options of `0x21`: a nibble per plane, normal, read as 0, or inverted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaneOption {
    Normal,
    Zero,
    Inverse,
}

impl PlaneOption {
    /// `0000` normal, `0100` bypass as 0, `1000` inverse; another value (reported by the write)
    /// reads as normal.
    pub(crate) fn from_nibble(nibble: u8) -> PlaneOption {
        match nibble & 0x0F {
            0b0100 => PlaneOption::Zero,
            0b1000 => PlaneOption::Inverse,
            _ => PlaneOption::Normal,
        }
    }

    fn apply(self, bit: bool) -> bool {
        match self {
            PlaneOption::Normal => bit,
            PlaneOption::Zero => false,
            PlaneOption::Inverse => !bit,
        }
    }
}

/// What the display reads from the controller's registers.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scan {
    /// MUX + 1: the gates scanned, from G0.
    pub(crate) lines: u16,
    /// The SM and GD bits of `0x01`.
    pub(crate) order: GateScan,
    pub(crate) red: PlaneOption,
    pub(crate) black_white: PlaneOption,
}

/// A pixel no gate or source drives in this refresh.
const UNDRIVEN: u8 = u8::MAX;

/// A display in progress: the LUT of every pixel, fixed at the activation.
#[derive(Clone, Debug)]
pub(crate) struct Display {
    pub(crate) started: Cycles,
    pub(crate) ends: Cycles,
    pub(crate) kind: RefreshKind,
    waveform: Waveform,
    luts: Vec<u8>,
    border: Option<Drive>,
}

/// The glass.
#[derive(Clone, Debug)]
pub(crate) struct Panel {
    geometry: PanelGeometry,
    optics: Optics,
    swing: i64,
    /// The gate that drives each physical row.
    gate_of_row: Vec<u16>,
    states: Vec<i64>,
    border: i64,
    image: PanelImage,
}

impl Panel {
    /// The glass at power-on. `geometry` and `optics` are validated.
    pub(crate) fn new(geometry: PanelGeometry, optics: Optics) -> Panel {
        let swing = i64::try_from(optics.full_swing).unwrap_or(i64::MAX);
        let height = geometry.height;
        let mut gate_of_row = vec![0u16; usize::from(height)];
        for gate in 0..height {
            // A validated geometry makes the native scan a bijection of its rows.
            let row = ram_row_of_gate(gate, geometry.native_scan, height).unwrap_or(gate);
            let physical = if geometry.rows_reversed {
                height - 1 - row
            } else {
                row
            };
            gate_of_row[usize::from(physical)] = gate;
        }
        let state = level(optics.initial_gray, swing);
        let count = usize::from(geometry.width) * usize::from(height);
        let mut panel = Panel {
            image: PanelImage {
                width: geometry.width,
                height,
                pixels: Vec::new(),
                border: optics.initial_gray,
                generation: 0,
                kind: RefreshKind::PowerOn,
                interrupted: false,
                started: Cycles::ZERO,
                at: Cycles::ZERO,
            },
            geometry,
            optics,
            swing,
            gate_of_row,
            states: vec![state; count],
            border: state,
        };
        panel.image.pixels = panel.grays();
        panel
    }

    pub(crate) fn image(&self) -> &PanelImage {
        &self.image
    }

    /// The RAM pixel (column, row) physical pixel (`x`, `y`) shows under `scan`, if any.
    fn ram_pixel(&self, x: u16, y: u16, scan: Scan) -> Option<(u16, u16)> {
        let gate = self.gate_of_row[usize::from(y)];
        let row = ram_row_of_gate(gate, scan.order, scan.lines)?;
        let column = if self.geometry.columns_reversed {
            self.geometry.width - 1 - x
        } else {
            x
        };
        Some((self.geometry.first_source + column, row))
    }

    /// Fixes the LUT of every pixel for a display from `started` to `ends`.
    pub(crate) fn start(
        &self,
        ram: &Ram,
        scan: Scan,
        waveform: Waveform,
        border: Option<Drive>,
        kind: RefreshKind,
        (started, ends): (Cycles, Cycles),
    ) -> Display {
        let mut luts = Vec::with_capacity(self.states.len());
        for y in 0..self.geometry.height {
            for x in 0..self.geometry.width {
                // A pixel whose RAM bit lies past the RAM is not driven: no index can fall outside it.
                let lut = self
                    .ram_pixel(x, y, scan)
                    .and_then(|(column, row)| {
                        let red = scan.red.apply(ram.bit(Plane::Red, column, row)?);
                        let black_white =
                            scan.black_white
                                .apply(ram.bit(Plane::BlackWhite, column, row)?);
                        Some(u8::from(red) << 1 | u8::from(black_white))
                    })
                    .unwrap_or(UNDRIVEN);
                luts.push(lut);
            }
        }
        Display {
            started,
            ends,
            kind,
            waveform,
            luts,
            border,
        }
    }

    /// Applies `display` up to `until_ns` nanoseconds into it (all of it with `None`) and publishes
    /// the image, dated `at`.
    pub(crate) fn finish(&mut self, display: &Display, at: Cycles, until_ns: Option<u64>) {
        let drive = |lut: usize| {
            display.waveform.transfer(
                Drive::Lut(lut),
                self.swing,
                self.optics.positive_drives_black,
                until_ns,
            )
        };
        let transfers = [drive(0), drive(1), drive(2), drive(3)];
        for (state, &lut) in self.states.iter_mut().zip(&display.luts) {
            if let Some(transfer) = transfers.get(usize::from(lut)) {
                *state = transfer.apply(*state);
            }
        }
        if let Some(border) = display.border {
            let transfer = display.waveform.transfer(
                border,
                self.swing,
                self.optics.positive_drives_black,
                until_ns,
            );
            self.border = transfer.apply(self.border);
        }
        self.image = PanelImage {
            width: self.geometry.width,
            height: self.geometry.height,
            pixels: self.grays(),
            border: gray(self.border, self.swing),
            generation: self.image.generation + 1,
            kind: display.kind,
            interrupted: until_ns.is_some(),
            started: display.started,
            at,
        };
    }

    fn grays(&self) -> Vec<u8> {
        self.states.iter().map(|&s| gray(s, self.swing)).collect()
    }
}

/// The optical state of grey level `gray`.
fn level(gray: u8, swing: i64) -> i64 {
    let scaled = i128::from(swing) * i128::from(gray) / 255;
    i64::try_from(scaled).unwrap_or(swing)
}

/// The grey level of optical state `state`, rounded to the nearest (half up).
fn gray(state: i64, swing: i64) -> u8 {
    let scaled = (2 * i128::from(state) * 255 + i128::from(swing)) / (2 * i128::from(swing));
    u8::try_from(scaled.clamp(0, 255)).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_show_the_rows_of_table_8_2() {
        let rows = |sm, gd| {
            (0..4)
                .chain(338..342)
                .chain(676..680)
                .map(|g| ram_row_of_gate(g, GateScan { sm, gd }, 680).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(false, false),
            [0, 1, 2, 3, 338, 339, 340, 341, 676, 677, 678, 679]
        );
        assert_eq!(
            rows(false, true),
            [1, 0, 3, 2, 339, 338, 341, 340, 677, 676, 679, 678]
        );
        assert_eq!(
            rows(true, false),
            [0, 340, 1, 341, 169, 509, 170, 510, 338, 678, 339, 679]
        );
        assert_eq!(
            rows(true, true),
            [340, 0, 341, 1, 509, 169, 510, 170, 678, 338, 679, 339]
        );
    }

    #[test]
    fn gates_past_the_mux_ratio_are_not_scanned() {
        let scan = GateScan {
            sm: true,
            gd: false,
        };
        assert_eq!(ram_row_of_gate(479, scan, 480), Some(479));
        assert_eq!(ram_row_of_gate(1, scan, 480), Some(240));
        assert_eq!(ram_row_of_gate(480, scan, 480), None);
        // MUX 3FFh (1024 lines) with SM = 1: odd gates show rows from 512, past the RAM from 680.
        assert_eq!(ram_row_of_gate(335, scan, 1024), Some(679));
        assert_eq!(ram_row_of_gate(337, scan, 1024), None);
        assert_eq!(ram_row_of_gate(679, scan, 1024), None);
        assert_eq!(ram_row_of_gate(678, scan, 1024), Some(339));
        // 681 lines (MUX 2A8h): the last gate of a 680-row panel would show row 680.
        assert_eq!(ram_row_of_gate(679, scan, 681), None);
        // An odd count with GD = 1: the last gate would show a row past the count.
        assert_eq!(
            ram_row_of_gate(
                4,
                GateScan {
                    sm: false,
                    gd: true
                },
                5
            ),
            None
        );
        assert_eq!(ram_row_of_gate(4, GateScan { sm: true, gd: true }, 5), None);
        assert_eq!(
            ram_row_of_gate(
                4,
                GateScan {
                    sm: true,
                    gd: false
                },
                5
            ),
            Some(2)
        );
    }

    #[test]
    fn the_x4_panel_shows_the_last_ram_row_at_the_top() {
        let panel = Panel::new(PanelGeometry::gdeq0426t82(), Optics::default());
        let scan = Scan {
            lines: 480,
            order: GateScan {
                sm: true,
                gd: false,
            },
            red: PlaneOption::Normal,
            black_white: PlaneOption::Normal,
        };
        assert_eq!(panel.ram_pixel(0, 0, scan), Some((0, 479)));
        assert_eq!(panel.ram_pixel(799, 479, scan), Some((799, 0)));
        assert_eq!(panel.ram_pixel(5, 100, scan), Some((5, 379)));
        // SM = 0 on this panel interlaces the rows: the second row shows another RAM row (the
        // first one is G479 either way).
        let interlaced = Scan {
            order: GateScan::default(),
            ..scan
        };
        assert_eq!(panel.ram_pixel(0, 0, interlaced), Some((0, 479)));
        assert_eq!(panel.ram_pixel(0, 1, scan), Some((0, 478)));
        assert_eq!(panel.ram_pixel(0, 1, interlaced), Some((0, 477)));
        // Only 240 gates scanned: the top half of the glass is not driven.
        let short = Scan { lines: 240, ..scan };
        assert_eq!(panel.ram_pixel(0, 0, short), None);
        assert_eq!(panel.ram_pixel(0, 479, short), Some((0, 0)));
    }

    #[test]
    fn columns_can_start_late_and_run_backwards() {
        let geometry = PanelGeometry {
            width: 8,
            height: 2,
            first_source: 100,
            columns_reversed: true,
            native_scan: GateScan {
                sm: false,
                gd: true,
            },
            rows_reversed: false,
        };
        let panel = Panel::new(geometry, Optics::default());
        let scan = Scan {
            lines: 680,
            order: GateScan {
                sm: false,
                gd: true,
            },
            red: PlaneOption::Normal,
            black_white: PlaneOption::Normal,
        };
        assert_eq!(panel.ram_pixel(0, 0, scan), Some((107, 0)));
        assert_eq!(panel.ram_pixel(7, 1, scan), Some((100, 1)));
    }

    #[test]
    fn grey_levels_round_and_the_power_on_image_is_uniform() {
        let swing = 6_000;
        assert_eq!(gray(0, swing), 0);
        assert_eq!(gray(swing, swing), 255);
        assert_eq!(gray(swing / 2, swing), 128);
        assert_eq!(level(255, swing), swing);
        assert_eq!(level(0, swing), 0);
        for g in [0u8, 1, 77, 128, 254, 255] {
            assert_eq!(gray(level(g, swing), swing), g);
        }
        let optics = Optics {
            initial_gray: 200,
            ..Optics::default()
        };
        let panel = Panel::new(PanelGeometry::gdeq0426t82(), optics);
        let image = panel.image();
        assert_eq!(
            (image.width, image.height, image.pixels.len()),
            (800, 480, 384_000)
        );
        assert!(image.pixels.iter().all(|&p| p == 200));
        assert_eq!(
            (image.border, image.generation, image.kind),
            (200, 0, RefreshKind::PowerOn)
        );
        assert!(!image.interrupted);
    }

    #[test]
    fn plane_options_read_normal_zero_or_inverted() {
        assert_eq!(PlaneOption::from_nibble(0x0), PlaneOption::Normal);
        assert_eq!(PlaneOption::from_nibble(0x4), PlaneOption::Zero);
        assert_eq!(PlaneOption::from_nibble(0x8), PlaneOption::Inverse);
        assert_eq!(PlaneOption::from_nibble(0xC), PlaneOption::Normal);
        assert!(PlaneOption::Normal.apply(true) && !PlaneOption::Zero.apply(true));
        assert!(PlaneOption::Inverse.apply(false));
    }
}
