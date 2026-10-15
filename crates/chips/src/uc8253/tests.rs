use std::cell::RefCell;
use std::rc::Rc;

use emu_core::{GpioInput, GpioOutput, Nanos};

use super::lines::Uc8253Lines;
use super::*;
use crate::lines::ONE_GIGAHERTZ;

const MS: u64 = 1_000_000;
/// The X3's frame: 99 bytes a row, 528 rows.
const ROW: usize = 99;
const ROWS: usize = 528;

/// A one-group LUT: `levels` (phase A in bits 7:6), 20 frames in phase A, run once: at 50 Hz and
/// 15 V, 6 V·s, the model's full swing.
fn lut(levels: u8) -> [u8; LUT_BYTES] {
    let mut lut = [0; LUT_BYTES];
    lut[0] = levels;
    lut[1] = 20;
    lut[5] = 1;
    lut
}

/// What the refreshes of these tests take: 20 frames and the two of `VCOM_DC` at 20 ms.
const REFRESH_NS: u64 = 22 * 20 * MS;

/// The chip under `FreeInk`'s X3 driver, cut to what these tests need (research
/// `sp3-x3-firmware.md` §3.2): commands and data at instants of a clock that moves on by itself.
struct Driver {
    chip: Uc8253,
    now: u64,
}

impl Driver {
    fn new(config: Uc8253Config) -> Driver {
        Driver {
            chip: Uc8253::new(config, ONE_GIGAHERTZ).unwrap(),
            now: 0,
        }
    }

    fn x3() -> Driver {
        Driver::new(Uc8253Config::xteink_x3())
    }

    fn at(&self) -> Cycles {
        Cycles(self.now)
    }

    fn cmd(&mut self, command: u8, data: &[u8]) {
        self.now += 1_000;
        self.chip.write_command(self.at(), command);
        for &byte in data {
            self.chip.write_data(self.at(), byte);
        }
    }

    /// Waits for `BUSY_N` high, as `FreeInk`'s `X3TwoPhase` does, and gives how long it was low.
    fn wait(&mut self) -> u64 {
        let from = self.now;
        while self.chip.busy_pin(self.at()) == Level::Low {
            let next = self.chip.next_deadline().unwrap().get();
            self.now = next.max(self.now + 1);
            self.chip.advance(self.at());
        }
        self.now - from
    }

    fn init(&mut self) {
        self.cmd(PSR, &[0x3F, 0x0A]);
        self.cmd(TRES, &[0x03, 0x18, 0x02, 0x58]);
        self.cmd(GSST, &[0, 0, 0, 0]);
        self.cmd(PFS, &[0x20]);
        self.cmd(PWR, &[0x07, 0x17, 0x3F, 0x3F, 0x17]);
        self.cmd(VDCS, &[0x24]);
        self.cmd(BTST, &[0x25, 0x25, 0x3C, 0x37]);
        self.cmd(PLL, &[0x09]);
        self.cmd(LVSEL_X3, &[0x02]);
        let white = vec![0xFF; ROW * ROWS];
        self.plane(DTM1, &white);
        self.plane(DTM2, &white);
    }

    /// A plane, last row first as `EpdBus::sendPlaneFlipped` sends it, then `DSP`.
    fn plane(&mut self, command: u8, frame: &[u8]) {
        let mut flipped = Vec::with_capacity(frame.len());
        for row in frame.chunks(ROW).rev() {
            flipped.extend_from_slice(row);
        }
        self.cmd(command, &flipped);
        self.cmd(DSP, &[]);
    }

    fn luts(&mut self) {
        self.cmd(CDI, &[0x29, 0x07]);
        for (command, levels) in [
            (LUTC, 0x00),
            (0x21, 0x80),
            (0x22, 0x80),
            (0x23, 0x40),
            (LUTKK, 0x40),
        ] {
            self.cmd(command, &lut(levels));
        }
    }

    /// A full refresh to `frame`: the LUTs, `DTM2`, PON, DRF, then `DTM1` as the driver rewrites it.
    fn full(&mut self, frame: &[u8]) {
        self.luts();
        self.plane(DTM2, frame);
        if !self.chip.is_powered() {
            self.cmd(PON, &[]);
            self.wait();
        }
        self.cmd(DRF, &[]);
        self.wait();
        self.plane(DTM1, frame);
    }
}

/// A white frame with a black rectangle at columns `x0..x1`, rows `y0..y1` of the framebuffer.
fn frame_with_black(x0: usize, x1: usize, y0: usize, y1: usize) -> Vec<u8> {
    let mut frame = vec![0xFF; ROW * ROWS];
    for y in y0..y1 {
        for x in x0..x1 {
            frame[y * ROW + x / 8] &= !(0x80 >> (x % 8));
        }
    }
    frame
}

fn pixel(image: &PanelImage, x: usize, y: usize) -> u8 {
    image.pixels[y * usize::from(image.width) + x]
}

/// `FreeInk`'s X3 sequence draws its framebuffer on the glass as it lies in memory: rows written
/// last first land on the gates the glass shows top down (`rows_reversed`), so the image's (x, y)
/// is the framebuffer's.
#[test]
fn a_full_refresh_shows_the_framebuffer_on_the_glass() {
    let mut driver = Driver::x3();
    driver.init();
    driver.full(&frame_with_black(16, 40, 10, 30));
    let image = driver.chip.panel_image();
    assert_eq!((image.width, image.height), (792, 528));
    assert_eq!((image.generation, image.kind), (1, RefreshKind::Full));
    assert_eq!(pixel(image, 16, 10), 0, "black");
    assert_eq!(pixel(image, 39, 29), 0);
    assert_eq!(pixel(image, 40, 29), 255, "white");
    assert_eq!(pixel(image, 16, 30), 255);
    assert_eq!(pixel(image, 791, 527), 255);
    assert_eq!(image.at.get() - image.started.get(), REFRESH_NS);
    assert_eq!(driver.chip.take_diagnostics(), []);
}

/// `BUSY_N` is low while the supplies come up and while a refresh runs, high otherwise; a second
/// PON on a powered chip gives no pulse.
#[test]
fn busy_n_is_low_while_the_chip_works() {
    let mut driver = Driver::x3();
    driver.init();
    assert_eq!(
        driver.chip.busy_pin(driver.at()),
        Level::High,
        "idle at rest"
    );
    driver.cmd(PON, &[]);
    assert_eq!(driver.chip.busy_pin(driver.at()), Level::Low);
    assert_eq!(driver.wait(), 127 * MS, "PON");
    assert!(driver.chip.is_powered());
    driver.cmd(PON, &[]);
    assert_eq!(
        driver.chip.busy_pin(driver.at()),
        Level::High,
        "already powered"
    );
    driver.luts();
    driver.cmd(DRF, &[]);
    assert_eq!(driver.wait(), REFRESH_NS);
    driver.cmd(POF, &[]);
    assert_eq!(driver.wait(), 30 * MS);
    assert!(!driver.chip.is_powered());
}

/// A refresh in partial mode drives only the window `PTL` sets; the glass outside it keeps what
/// it showed.
#[test]
fn a_partial_window_refreshes_only_its_pixels() {
    let mut driver = Driver::x3();
    driver.init();
    driver.full(&frame_with_black(0, 792, 0, 528));
    let black = frame_with_black(0, 792, 0, 528);
    let white = vec![0xFF; ROW * ROWS];
    driver.plane(DTM1, &black);
    driver.luts();
    driver.cmd(PTIN, &[]);
    // Columns 0–791, rows 0–527 of RAM, then rows 100–199: the glass's rows 328–427.
    driver.cmd(PTL, &[0x00, 0x00, 0x03, 0x17, 0x00, 100, 0x00, 199, 0x01]);
    driver.cmd(DTM2, &white[..ROW * 100]);
    driver.cmd(PTOUT, &[]);
    driver.cmd(PTIN, &[]);
    driver.cmd(DRF, &[]);
    driver.wait();
    let image = driver.chip.panel_image();
    assert_eq!((image.generation, image.kind), (2, RefreshKind::Partial));
    assert_eq!(pixel(image, 0, 327), 0, "outside the window");
    assert_eq!(pixel(image, 0, 328), 255, "RAM row 199");
    assert_eq!(pixel(image, 791, 427), 255, "RAM row 100");
    assert_eq!(pixel(image, 0, 428), 0);
}

/// `N2OCP` (DS79 `CDI` bit 3, set in the X3's `0x29`) copies NEW to OLD after a refresh.
#[test]
fn a_refresh_copies_new_to_old_when_cdi_says_so() {
    let mut driver = Driver::x3();
    driver.init();
    driver.luts();
    driver.plane(DTM2, &frame_with_black(0, 8, 0, 1));
    driver.cmd(PON, &[]);
    driver.wait();
    assert_eq!(driver.chip.ram_bit(Plane::Old, 0, 527), Some(true));
    driver.cmd(DRF, &[]);
    driver.wait();
    assert_eq!(driver.chip.ram_bit(Plane::Old, 0, 527), Some(false));
    assert_eq!(driver.chip.ram_bit(Plane::Old, 8, 527), Some(true));
}

/// The first batch drives nothing on a read; the `UC8279d` answers `REV` with `0x66` in its third
/// byte, the byte `FreeInk` selects its driver by.
#[test]
fn rev_floats_on_a_uc8253_and_names_a_uc8279d() {
    let mut first = Driver::x3();
    first.cmd(REV, &[]);
    assert_eq!(first.chip.read_data(first.at()), None);
    assert_eq!(
        first
            .chip
            .take_diagnostics()
            .into_iter()
            .map(|d| d.kind)
            .collect::<Vec<_>>(),
        [DiagnosticKind::NothingToRead { command: REV }]
    );
    let mut later = Driver::new(Uc8253Config::xteink_x3_uc8279d());
    later.cmd(REV, &[]);
    let bytes: Vec<_> = (0..5).map(|_| later.chip.read_data(later.at())).collect();
    assert_eq!(
        bytes,
        [Some(0x00), Some(0x03), Some(0x66), Some(0xFF), Some(0xFF)]
    );
    later.cmd(FLG, &[]);
    assert_eq!(
        later.chip.read_data(later.at()),
        Some(0x13),
        "idle, powered off"
    );
}

/// Deep sleep takes the check code `0xA5` and drops every command until `RST_N` goes low; the
/// reset brings the registers back and keeps `BUSY_N` low for its initialisation.
#[test]
fn deep_sleep_ends_only_by_a_hardware_reset() {
    let mut driver = Driver::x3();
    driver.init();
    driver.cmd(DSLP, &[0x00]);
    driver.wait();
    assert!(!driver.chip.is_asleep(), "wrong code");
    driver.cmd(DSLP, &[0xA5]);
    driver.wait();
    assert!(driver.chip.is_asleep());
    let _ = driver.chip.take_diagnostics();
    driver.cmd(PON, &[]);
    assert_eq!(
        driver.chip.take_diagnostics()[0].kind,
        DiagnosticKind::WhileAsleep { command: PON }
    );
    driver.now += MS;
    driver.chip.set_reset(driver.at(), true);
    assert_eq!(driver.chip.busy_pin(driver.at()), Level::Low, "in reset");
    driver.now += 10 * MS;
    driver.chip.set_reset(driver.at(), false);
    assert_eq!(driver.wait(), MS, "initialisation");
    assert!(!driver.chip.is_asleep());
    driver.cmd(PON, &[]);
    assert_eq!(driver.chip.busy_pin(driver.at()), Level::Low, "awake");
}

/// A reset during a refresh leaves the glass where it had got to, and says so.
#[test]
fn a_reset_cuts_a_refresh_short() {
    let mut driver = Driver::x3();
    driver.init();
    driver.luts();
    driver.plane(DTM2, &frame_with_black(0, 792, 0, 528));
    driver.cmd(PON, &[]);
    driver.wait();
    driver.cmd(DRF, &[]);
    driver.now += 10 * 20 * MS;
    driver.chip.set_reset(driver.at(), true);
    let image = driver.chip.panel_image();
    assert!(image.interrupted);
    let half = pixel(image, 0, 0);
    assert!((120..=135).contains(&half), "half way to black: {half}");
    assert!(
        driver
            .chip
            .take_diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::ResetDuringRefresh)
    );
}

/// During a refresh only REV, FLG and VV are taken (DS79); a refresh needs the supplies and the
/// mode the model plays.
#[test]
fn commands_during_a_refresh_and_refreshes_the_model_refuses_are_named() {
    let mut driver = Driver::x3();
    driver.init();
    driver.luts();
    driver.cmd(DRF, &[]);
    assert_eq!(
        driver.chip.take_diagnostics()[0].kind,
        DiagnosticKind::RefreshWithoutPower
    );
    assert_eq!(driver.chip.panel_image().generation, 0);
    driver.cmd(PON, &[]);
    driver.wait();
    driver.cmd(DRF, &[]);
    driver.cmd(DTM2, &[0x00]);
    driver.cmd(FLG, &[]);
    let kinds: Vec<_> = driver
        .chip
        .take_diagnostics()
        .into_iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(kinds, [DiagnosticKind::WhileBusy { command: DTM2 }]);
    driver.wait();
    driver.cmd(PSR, &[0x2F]);
    driver.cmd(DRF, &[]);
    assert_eq!(
        driver.chip.take_diagnostics()[0].kind,
        DiagnosticKind::RefreshNotModelled {
            what: "KWR mode (PSR KW/R = 0)"
        }
    );
}

/// A LUT of another length than KW mode reads, a command short of its parameters and one the
/// datasheet does not have are named.
#[test]
fn malformed_traffic_is_named() {
    let mut driver = Driver::x3();
    driver.cmd(0x21, &[0; 49]);
    driver.cmd(TRES, &[0x03]);
    driver.cmd(0x3F, &[1, 2]);
    driver.cmd(AUTO, &[0xA5]);
    driver.cmd(PLL, &[]);
    let kinds: Vec<_> = driver
        .chip
        .take_diagnostics()
        .into_iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            DiagnosticKind::LutLength {
                command: 0x21,
                bytes: 49
            },
            DiagnosticKind::MissingData {
                command: TRES,
                expected: 4,
                received: 1
            },
            DiagnosticKind::UnknownCommand { command: 0x3F },
            DiagnosticKind::UnsupportedCommand {
                command: AUTO,
                name: "Auto Sequence"
            },
        ]
    );
}

/// The soft reset of `PSR` (`RST_N` 0) brings the registers back and holds `BUSY_N` low.
#[test]
fn a_soft_reset_brings_the_registers_back() {
    let mut driver = Driver::x3();
    driver.init();
    driver.cmd(PON, &[]);
    driver.wait();
    driver.cmd(PSR, &[0x0E]);
    assert_eq!(driver.chip.busy_pin(driver.at()), Level::Low);
    assert_eq!(driver.wait(), 50_000);
    assert!(!driver.chip.is_powered());
}

/// `FreeInk`'s probe of the X3's controller (research `sp3-x3-firmware.md` §2.2), bit-banged on the
/// lines: `0x70` written, D/C# high, three bytes sampled after each rising edge. A UC8253 leaves
/// the line to its pull; a `UC8279d` drives `00 03 66`.
#[test]
fn the_x3_probe_through_the_lines_reads_the_variant() {
    for (config, expected) in [
        (Uc8253Config::xteink_x3(), [None; 3]),
        (
            Uc8253Config::xteink_x3_uc8279d(),
            [Some(0x00), Some(0x03), Some(0x66)],
        ),
    ] {
        let lines = Rc::new(RefCell::new(Uc8253Lines::new(config).unwrap()));
        let mut outputs = Uc8253Lines::outputs(&lines);
        let mut inputs = Uc8253Lines::inputs(&lines);
        let mut t = 0;
        let mut tick = || {
            t += 1_000;
            Nanos(t)
        };
        emu_core::SpiTarget::select(&mut *lines.borrow_mut(), true, tick());
        outputs.clock.driven(Some(Level::Low), tick());
        outputs.data_command.driven(Some(Level::Low), tick());
        for bit in 0..8 {
            let high = 0x70 & (0x80 >> bit) != 0;
            outputs.data.driven(Some(Level::from_bool(high)), tick());
            outputs.clock.driven(Some(Level::High), tick());
            outputs.clock.driven(Some(Level::Low), tick());
        }
        outputs.data_command.driven(Some(Level::High), tick());
        outputs.data.driven(None, tick());
        let mut read = Vec::new();
        for _ in 0..3 {
            let mut byte = Some(0u8);
            for _ in 0..8 {
                outputs.clock.driven(Some(Level::Low), tick());
                outputs.clock.driven(Some(Level::High), tick());
                let level = inputs.data.level(tick());
                byte = match (byte, level) {
                    (Some(b), Some(level)) => Some(b << 1 | u8::from(level.is_high())),
                    _ => None,
                };
            }
            outputs.clock.driven(Some(Level::Low), tick());
            read.push(byte);
        }
        assert_eq!(read, expected);
    }
}
