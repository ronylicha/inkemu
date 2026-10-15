//! The SSD1677 driven as the X4's firmware drives it: the byte sequences of `FreeInk`'s
//! `Ssd1677Driver` (the hardware layer of `CrossPoint`: `Free-Ink/freeink-sdk` at `111fdcc7` for
//! the release 1.6.5, read at `7fcd3b34`, the submodule of `CrossPoint`'s `main`, whose X4 path
//! sends the same bytes; research `sp2a-external-chips.md` §0 and §4), through the public interface
//! only, on the 800×480 panel of the X4 (`SP2a` design §5, criterion 2).
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

use chips::{DiagnosticKind, PanelImage, Plane, RefreshKind, Ssd1677, Ssd1677Config, Temperature};
use emu_core::{Cycles, Frequency};

const WIDTH: usize = 800;
const HEIGHT: usize = 480;
const ROW_BYTES: usize = WIDTH / 8;

/// Cycles of the 160 MHz time base in `n` milliseconds.
fn ms(n: u64) -> u64 {
    n * 160_000
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Full,
    Half,
    Fast,
}

/// The firmware's side of the bus: `EpdBus` and `Ssd1677Driver` with the X4's configuration.
struct X4 {
    chip: Ssd1677,
    now: Cycles,
}

impl X4 {
    fn new() -> X4 {
        let time_base = Frequency::from_hz(160_000_000).unwrap();
        X4 {
            chip: Ssd1677::new(Ssd1677Config::default(), time_base).unwrap(),
            now: Cycles::ZERO,
        }
    }

    fn delay(&mut self, milliseconds: u64) {
        self.now = self.now + ms(milliseconds);
        self.chip.advance(self.now);
    }

    fn cmd(&mut self, command: u8, data: &[u8]) {
        self.chip.write_command(self.now, command);
        for &byte in data {
            self.chip.write_data(self.now, byte);
        }
    }

    /// `EpdBus::waitBusy`: the caller wakes on each edge, at the instants the chip announces.
    fn wait_busy(&mut self) {
        while self.chip.busy_pin(self.now) {
            let next = self
                .chip
                .next_deadline()
                .expect("BUSY high with nothing pending");
            self.now = next;
            self.chip.advance(next);
        }
    }

    /// `EpdBus::reset`: RES# high 10 ms, low 10 ms, high 10 ms.
    fn reset(&mut self) {
        self.chip.set_reset(self.now, false);
        self.delay(10);
        self.chip.set_reset(self.now, true);
        self.delay(10);
        self.chip.set_reset(self.now, false);
        self.delay(10);
    }

    /// `Ssd1677Driver::begin`: reset, then `initController`.
    fn begin(&mut self) {
        self.reset();
        self.cmd(0x12, &[]);
        self.delay(10);
        self.wait_busy();
        self.cmd(0x18, &[0x80]);
        self.cmd(0x0C, &[0xAE, 0xC7, 0xC3, 0xC0, 0x80]);
        self.cmd(0x01, &[0xDF, 0x01, 0x02]);
        self.cmd(0x3C, &[0x80]);
        self.set_ram_area(0, 0, 800, 480);
        self.cmd(0x46, &[0xF7]);
        self.wait_busy();
        self.cmd(0x47, &[0xF7]);
        self.wait_busy();
    }

    /// `Ssd1677Driver::setRamArea` without mirroring: X increment, Y decrement, the gates reversed.
    fn set_ram_area(&mut self, x: u16, y: u16, w: u16, h: u16) {
        let y = 480 - y - h;
        let left = x.to_le_bytes();
        let right = (x + w - 1).to_le_bytes();
        let bottom = (y + h - 1).to_le_bytes();
        let top = y.to_le_bytes();
        self.cmd(0x11, &[0x01]);
        self.cmd(0x44, &[left[0], left[1], right[0], right[1]]);
        self.cmd(0x45, &[bottom[0], bottom[1], top[0], top[1]]);
        self.cmd(0x4E, &left);
        self.cmd(0x4F, &bottom);
    }

    /// `Ssd1677Driver::refresh` on the stock X4 sequences (`seqOverride` path).
    fn refresh(&mut self, mode: Mode) {
        self.cmd(0x21, &[if mode == Mode::Fast { 0x00 } else { 0x40 }]);
        self.cmd(0x3C, &[0xC0]);
        if mode == Mode::Half {
            self.cmd(0x1A, &[0x5A]);
        }
        let sequence = match mode {
            Mode::Full => 0xF7,
            Mode::Half => 0xD7,
            Mode::Fast => 0xFC,
        };
        self.cmd(0x22, &[sequence]);
        self.cmd(0x20, &[]);
        self.wait_busy();
    }

    /// `Ssd1677Driver::displayImpl`, single buffer (`prev == nullptr`), not deferred.
    fn display(&mut self, frame: &[u8], mode: Mode) -> u64 {
        self.set_ram_area(0, 0, 800, 480);
        self.cmd(0x24, frame);
        if mode != Mode::Fast {
            self.cmd(0x26, frame);
        }
        let start = self.now;
        self.refresh(mode);
        let busy = self.now.get() - start.get();
        self.set_ram_area(0, 0, 800, 480);
        self.cmd(0x24, frame);
        self.cmd(0x26, frame);
        busy
    }

    /// `powerOffController` then `deepSleep`'s `10 03`.
    fn deep_sleep(&mut self) {
        self.cmd(0x3C, &[0x80]);
        self.cmd(0x22, &[0x03]);
        self.cmd(0x20, &[]);
        self.delay(200);
        self.wait_busy();
        self.cmd(0x10, &[0x03]);
    }

    fn image(&self) -> &PanelImage {
        self.chip.panel_image()
    }

    fn diagnostics(&mut self) -> Vec<DiagnosticKind> {
        self.chip
            .take_diagnostics()
            .into_iter()
            .map(|d| d.kind)
            .collect()
    }
}

/// A frame buffer of the firmware: rows from the top, 1 white, the most significant bit leftmost.
fn frame(black: impl Fn(usize, usize) -> bool) -> Vec<u8> {
    let mut bytes = vec![0xFFu8; ROW_BYTES * HEIGHT];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if black(x, y) {
                bytes[y * ROW_BYTES + x / 8] &= !(0x80 >> (x % 8));
            }
        }
    }
    bytes
}

fn pixel(image: &PanelImage, x: usize, y: usize) -> u8 {
    image.pixels[y * WIDTH + x]
}

/// A black square in the top-left corner and a black bottom row.
fn page(x: usize, y: usize) -> bool {
    (x < 16 && y < 8) || y == HEIGHT - 1
}

#[test]
fn begin_resets_the_controller_and_fills_both_planes_white() {
    let mut x4 = X4::new();
    x4.begin();
    // 30 ms of reset, 10 ms after the soft reset (2 ms of BUSY), 2 ms for each fill.
    assert_eq!(x4.now, Cycles(ms(44)));
    assert!(!x4.chip.busy_pin(x4.now));
    assert!(x4.chip.ram(Plane::BlackWhite).iter().all(|&b| b == 0xFF));
    assert!(x4.chip.ram(Plane::Red).iter().all(|&b| b == 0xFF));
    assert_eq!(x4.diagnostics(), []);
}

#[test]
fn a_full_refresh_shows_the_frame_buffer_upright_in_about_two_seconds() {
    let mut x4 = X4::new();
    x4.begin();
    let busy = x4.display(&frame(page), Mode::Full);
    // Analog on 100, temperature 5, LUT 1, display 1600, analog off 200.
    assert_eq!(busy, ms(1_906));
    let image = x4.image();
    assert_eq!((image.width, image.height), (800, 480));
    assert_eq!((image.generation, image.kind), (1, RefreshKind::Full));
    assert!(!image.interrupted);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let expected = if page(x, y) { 0 } else { 255 };
            assert_eq!(pixel(image, x, y), expected, "pixel ({x}, {y})");
        }
    }
    assert_eq!(image.border, 255, "C0h: the border floats");
    assert_eq!(x4.diagnostics(), []);
}

#[test]
fn fast_refreshes_are_partial_differential_and_leave_a_trace() {
    let mut x4 = X4::new();
    x4.begin();
    x4.display(&frame(page), Mode::Full);
    // The full sequence switched the analog supplies off: the first fast one starts them.
    let moved = |x: usize, y: usize| ((100..116).contains(&x) && y < 8) || y == HEIGHT - 1;
    assert_eq!(x4.display(&frame(moved), Mode::Fast), ms(606));
    let image = x4.image();
    assert_eq!((image.generation, image.kind), (2, RefreshKind::Partial));
    // White to black at 95 % of a swing; black to white the same; unchanged pixels untouched.
    assert_eq!(pixel(image, 100, 0), 13);
    assert_eq!(pixel(image, 0, 0), 242);
    assert_eq!(pixel(image, 400, HEIGHT - 1), 0);
    assert_eq!(pixel(image, 400, 200), 255);
    // The supplies stay on: the next one takes half a second.
    assert_eq!(x4.display(&frame(moved), Mode::Fast), ms(506));
    assert_eq!(x4.diagnostics(), []);
}

#[test]
fn a_half_refresh_uses_the_waveform_of_the_forced_ninety_degrees() {
    let mut x4 = X4::new();
    x4.begin();
    x4.display(&frame(page), Mode::Fast);
    // The fast sequence left the supplies on: LUT 1, display 1500, analog off 200; no
    // temperature load, and `1A 5A` with one byte only, as the driver sends it.
    assert_eq!(x4.display(&frame(|_, _| false), Mode::Half), ms(1_701));
    assert!(x4.image().pixels.iter().all(|&p| p == 255));
    assert_eq!(x4.image().kind, RefreshKind::Full);
    x4.cmd(0x1B, &[]);
    assert_eq!(
        x4.chip.read_data(x4.now),
        Some(0x5A),
        "90 °C stays in the register"
    );
    assert_eq!(x4.diagnostics(), []);
}

/// `FreeInk`'s `lut_factory_quality`, the X4's absolute four-level LUT.
///
/// Copied from `FreeInk` (`Free-Ink/freeink-sdk`, `Ssd1677Luts.h`), Copyright (c) 2026 `FreeInk`,
/// MIT licence, itself derived from the `OpenX4` E-Paper Community SDK, Copyright (c) 2025 Open X4
/// E-Paper Contributors, MIT licence. The licence and the notice are in `third_party/freeink-sdk/`,
/// with the provenance of this table.
const FACTORY_GREY: [u8; 110] = [
    0x00, 0x4A, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x80, 0x62, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x88, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0xA8, 0x44, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x08, 0x0B, 0x02, 0x03, 0x00, 0x0C, 0x02, 0x07, 0x02, 0x00, //
    0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, //
    0x22, 0x22, 0x22, 0x22, 0x22, //
    0x17, 0x41, 0xA8, 0x32, 0x30,
];

/// The MIT licence asks its notice to travel with the copy: the two files stay in the repository
/// as long as the table does (removing them breaks this file's build), unmodified.
#[test]
fn the_copied_lut_keeps_freeink_s_licence_and_notice() {
    let license = include_str!("../../../third_party/freeink-sdk/LICENSE");
    let notice = include_str!("../../../third_party/freeink-sdk/NOTICE");
    assert!(license.starts_with("MIT License\n\nCopyright (c) 2026 FreeInk\n"));
    assert!(notice.contains("Copyright (c) 2025 Open X4 E-Paper Contributors"));
    assert!(notice.contains("panel initialization sequences and waveform LUTs are derived"));
}

#[test]
fn the_factory_grey_lut_gives_four_levels() {
    let mut x4 = X4::new();
    x4.begin();
    x4.display(&frame(|_, _| false), Mode::Full);
    // Absolute planes, black at code 11: the high bit in RED, the low bit in B/W; four bands of
    // 200 columns, codes 00, 01, 10, 11.
    let code = |x: usize| x / 200;
    let plane = |bit: usize| -> Vec<u8> {
        let mut bytes = vec![0u8; ROW_BYTES * HEIGHT];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let x = (index % ROW_BYTES) * 8;
            if code(x) >> bit & 1 == 1 {
                *byte = 0xFF;
            }
        }
        bytes
    };
    x4.set_ram_area(0, 0, 800, 480);
    x4.cmd(0x24, &plane(0));
    x4.cmd(0x26, &plane(1));
    // `setCustomLut`, then `displayGray` in factory mode.
    x4.cmd(0x32, &FACTORY_GREY[..105]);
    x4.cmd(0x03, &FACTORY_GREY[105..106]);
    x4.cmd(0x04, &FACTORY_GREY[106..109]);
    x4.cmd(0x2C, &FACTORY_GREY[109..110]);
    x4.cmd(0x3C, &[0xC0]);
    x4.cmd(0x21, &[0x00]);
    x4.cmd(0x22, &[0xCC]);
    let start = x4.now;
    x4.cmd(0x20, &[]);
    x4.wait_busy();
    // Analog on 100 (the full refresh switched it off), 50 frames of 20 ms.
    assert_eq!(x4.now.get() - start.get(), ms(1_100));
    let image = x4.image();
    let levels: Vec<u8> = [0, 200, 400, 600]
        .iter()
        .map(|&x| pixel(image, x, 240))
        .collect();
    assert_eq!(levels, [255, 153, 128, 0]);
    assert_eq!(image.kind, RefreshKind::Partial);
    assert_eq!(x4.diagnostics(), []);
}

#[test]
fn deep_sleep_holds_busy_until_begin_wakes_the_controller() {
    let mut x4 = X4::new();
    x4.begin();
    x4.display(&frame(page), Mode::Full);
    x4.deep_sleep();
    assert!(x4.chip.busy_pin(x4.now));
    assert_eq!(x4.chip.next_deadline(), None);
    // The glass keeps its page through sleep.
    assert_eq!(pixel(x4.image(), 0, 0), 0);
    x4.delay(60_000);
    x4.begin();
    assert!(!x4.chip.busy_pin(x4.now));
    assert_eq!(x4.display(&frame(page), Mode::Full), ms(1_906));
    assert_eq!(x4.image().generation, 2);
    assert_eq!(x4.diagnostics(), []);
}

#[test]
fn a_mux_past_the_ram_leaves_the_gates_past_it_undriven_without_a_panic() {
    let mut x4 = X4::new();
    x4.begin();
    // MUX 3FFh (1024 lines) with SM = 1: odd gates show RAM rows from 512, and G337 to G479 rows
    // past the 680 of the RAM.
    x4.cmd(0x01, &[0xFF, 0x03, 0x02]);
    // Both planes black: first value 0, steps of 680 gates and 960 sources.
    x4.cmd(0x46, &[0x77]);
    x4.wait_busy();
    x4.cmd(0x47, &[0x77]);
    x4.wait_busy();
    x4.refresh(Mode::Full);
    let image = x4.image();
    let white_rows = (0..HEIGHT)
        .filter(|&y| (0..WIDTH).all(|x| pixel(image, x, y) == 255))
        .count();
    let black_rows = (0..HEIGHT)
        .filter(|&y| (0..WIDTH).all(|x| pixel(image, x, y) == 0))
        .count();
    assert_eq!(
        (white_rows, black_rows),
        (72, 408),
        "72 odd gates from G337 to G479"
    );
    assert_eq!(
        x4.diagnostics(),
        [DiagnosticKind::ReservedValue {
            command: 0x01,
            value: 0x03
        }]
    );
}

#[test]
fn the_x4_family_probe_finds_no_uc81xx_answer() {
    let mut x4 = X4::new();
    x4.reset();
    x4.cmd(0x70, &[]);
    let ver: Vec<Option<u8>> = (0..5).map(|_| x4.chip.read_data(x4.now)).collect();
    x4.cmd(0x71, &[]);
    assert_eq!(ver, [None; 5], "the half-duplex line floats");
    assert_eq!(x4.chip.read_data(x4.now), None);
    assert_eq!(
        x4.diagnostics(),
        [
            DiagnosticKind::UnknownCommand { command: 0x70 },
            DiagnosticKind::UnknownCommand { command: 0x71 }
        ]
    );
}

#[test]
fn a_colder_panel_takes_the_same_waveform_and_a_hotter_one_the_other() {
    let mut x4 = X4::new();
    x4.chip.set_ambient(Temperature::from_celsius(60));
    x4.begin();
    // Above 50 °C the full sequence loads the 75-frame waveform.
    assert_eq!(x4.display(&frame(page), Mode::Full), ms(1_806));
    x4.chip.set_ambient(Temperature::from_celsius(0));
    assert_eq!(x4.display(&frame(page), Mode::Full), ms(1_906));
}
