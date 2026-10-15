//! One test per command and per mode of the SSD1677, and the edge cases (`SP2a` design §5,
//! criterion 1). Durations follow the default stage durations (research §5) on a 160 MHz time base.

use super::*;
use DiagnosticKind as D;

/// Cycles of the 160 MHz time base in `n` milliseconds.
fn ms(n: u64) -> u64 {
    n * 160_000
}

/// A panel of 32×4 pixels, rows and columns in RAM order: displays stay quick to compute.
fn small() -> Ssd1677Config {
    Ssd1677Config {
        panel: PanelGeometry {
            width: 32,
            height: 4,
            first_source: 0,
            columns_reversed: false,
            native_scan: GateScan::default(),
            rows_reversed: false,
        },
        ..Ssd1677Config::default()
    }
}

/// The MCU side: sends bytes at the current instant and waits for BUSY as a firmware does.
struct Host {
    chip: Ssd1677,
    now: Cycles,
}

impl Host {
    /// The chip under `config`, its internal sensor selected as the X4's firmware does first
    /// (`18 80`): the external one of the reset value has nothing on the X4.
    fn with(config: Ssd1677Config) -> Host {
        let time_base = Frequency::from_hz(160_000_000).unwrap();
        let mut host = Host {
            chip: Ssd1677::new(config, time_base).unwrap(),
            now: Cycles::ZERO,
        };
        host.send(0x18, &[0x80]);
        host
    }

    fn new() -> Host {
        Host::with(small())
    }

    fn send(&mut self, command: u8, data: &[u8]) {
        self.chip.write_command(self.now, command);
        for &byte in data {
            self.chip.write_data(self.now, byte);
        }
    }

    fn read(&mut self, command: u8, count: usize) -> Vec<Option<u8>> {
        self.chip.write_command(self.now, command);
        (0..count).map(|_| self.chip.read_data(self.now)).collect()
    }

    fn skip(&mut self, cycles: u64) {
        self.now = self.now + cycles;
        self.chip.advance(self.now);
    }

    /// Waits for BUSY to fall, calling the chip at each instant it asks for; returns the cycles
    /// waited.
    fn wait(&mut self) -> u64 {
        let start = self.now;
        while self.chip.is_busy(self.now) {
            let next = self
                .chip
                .next_deadline()
                .expect("busy with nothing pending");
            assert!(next >= self.now, "a deadline in the past");
            self.now = next;
            self.chip.advance(next);
        }
        self.now.get() - start.get()
    }

    fn diagnostics(&mut self) -> Vec<DiagnosticKind> {
        self.chip
            .take_diagnostics()
            .into_iter()
            .map(|d| d.kind)
            .collect()
    }

    /// Clock and analog on: `22 C0`, `20`.
    fn power_on(&mut self) {
        self.send(0x22, &[0xC0]);
        self.send(0x20, &[]);
        self.wait();
    }

    /// Runs `0x22 control`, `0x20` and returns how long BUSY stayed high.
    fn update(&mut self, control: u8) -> u64 {
        self.send(0x22, &[control]);
        self.send(0x20, &[]);
        self.wait()
    }

    /// Writes `bytes` to RAM row `row` of the plane of `command`, from the first byte.
    fn write_row(&mut self, command: u8, row: u16, bytes: &[u8]) {
        let [low, high] = row.to_le_bytes();
        self.send(0x4E, &[0, 0]);
        self.send(0x4F, &[low, high]);
        self.send(command, bytes);
    }

    /// Every row of the small panel in the plane of `command`.
    fn fill_rows(&mut self, command: u8, bytes: &[u8]) {
        for row in 0..4 {
            self.write_row(command, row, bytes);
        }
    }

    fn status(&mut self) -> u8 {
        self.read(0x2F, 1)[0].unwrap()
    }

    fn pixel(&self, x: usize, y: usize) -> u8 {
        let image = self.chip.panel_image();
        image.pixels[y * usize::from(image.width) + x]
    }
}

// --- Power-on, resets, the command table -------------------------------------------------

#[test]
fn at_power_on_the_chip_is_idle_and_the_glass_white() {
    let host = Host::with(Ssd1677Config::default());
    assert!(!host.chip.is_busy(Cycles::ZERO));
    assert!(!host.chip.busy_pin(Cycles::ZERO));
    assert_eq!(host.chip.next_deadline(), None);
    let image = host.chip.panel_image();
    assert_eq!((image.width, image.height), (800, 480));
    assert_eq!((image.generation, image.kind), (0, RefreshKind::PowerOn));
    assert!(image.pixels.iter().all(|&p| p == 255));
    assert!(host.chip.ram(Plane::BlackWhite).iter().all(|&b| b == 0));
    assert_eq!(host.chip.ram(Plane::Red).len(), 120 * 680);
    assert_eq!(host.chip.config().panel.width, 800);
}

#[test]
fn a_hardware_reset_drops_commands_while_low_then_restores_the_registers() {
    let mut h = Host::new();
    h.send(0x11, &[0x00]);
    h.send(0x4E, &[0x10, 0]);
    h.chip.set_reset(h.now, true);
    assert!(!h.chip.is_busy(h.now), "BUSY is low while RES# is low");
    h.send(0x24, &[0xFF, 0xFF]);
    assert_eq!(h.chip.read_data(h.now), None);
    assert_eq!(h.diagnostics(), [D::WhileInReset { command: 0x24 }]);
    h.skip(ms(10));
    h.chip.set_reset(h.now, true);
    h.chip.set_reset(h.now, false);
    assert!(h.chip.is_busy(h.now));
    assert_eq!(h.chip.next_deadline(), Some(h.now + ms(1)));
    assert_eq!(h.wait(), ms(1));
    // Entry 03h and the counter at 0 again.
    h.send(0x24, &[0x0F, 0xF0]);
    assert_eq!(&h.chip.ram(Plane::BlackWhite)[..2], [0x0F, 0xF0]);
    h.chip.set_reset(h.now, false);
    assert!(
        !h.chip.is_busy(h.now),
        "releasing a released line does nothing"
    );
    assert_eq!(h.diagnostics(), []);
}

#[test]
fn a_software_reset_restores_the_registers_keeps_the_ram_and_takes_2_ms() {
    let mut h = Host::new();
    h.send(0x24, &[0x55]);
    h.send(0x11, &[0x00]);
    h.send(0x12, &[]);
    assert_eq!(h.wait(), ms(2));
    assert_eq!(h.chip.ram(Plane::BlackWhite)[0], 0x55);
    h.send(0x24, &[0xAA, 0xBB]);
    assert_eq!(&h.chip.ram(Plane::BlackWhite)[..2], [0xAA, 0xBB]);
    assert_eq!(h.diagnostics(), []);
}

#[test]
fn the_model_has_every_command_of_the_datasheet_and_no_other() {
    // Datasheet Table 7-1.
    let datasheet = [
        0x01, 0x03, 0x04, 0x08, 0x09, 0x0A, 0x0C, 0x10, 0x11, 0x12, 0x14, 0x15, 0x18, 0x1A, 0x1B,
        0x1C, 0x20, 0x21, 0x22, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E,
        0x2F, 0x30, 0x31, 0x32, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x41, 0x44,
        0x45, 0x46, 0x47, 0x4D, 0x4E, 0x4F, 0x7F,
    ];
    let known: Vec<u8> = (0..=u8::MAX).filter(|&c| kind(c).is_some()).collect();
    assert_eq!(known, datasheet);
}

#[test]
fn an_unknown_command_is_reported_and_its_data_ignored() {
    let mut h = Host::new();
    // The X4-family probe: VER, then a read the SSD1677 does not answer.
    assert_eq!(h.read(0x70, 2), [None, None]);
    h.send(0x71, &[1, 2, 3]);
    assert_eq!(
        h.diagnostics(),
        [
            D::UnknownCommand { command: 0x70 },
            D::UnknownCommand { command: 0x71 }
        ]
    );
}

#[test]
fn reserved_and_unmodelled_commands_are_named_and_their_data_accepted() {
    let mut h = Host::new();
    // A state to keep: RAM bytes, a counter mid-row, the temperature, VCOM, a display option, an ID.
    h.send(0x24, &[0x12, 0x34]);
    h.send(0x26, &[0x56]);
    h.send(0x1A, &[0x19, 0x00]);
    h.send(0x2C, &[0x30]);
    h.send(0x37, &[0, 0, 0, 0, 0, 0, 7, 8, 9, 10]);
    h.send(0x38, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    let snapshot = |h: &mut Host| {
        (
            h.chip.ram(Plane::BlackWhite).to_vec(),
            h.chip.ram(Plane::Red).to_vec(),
            h.read(0x1B, 2),
            h.read(0x2D, 11),
            h.read(0x2E, 10),
            h.status(),
            h.chip.panel_image().generation,
        )
    };
    let before = snapshot(&mut h);
    h.send(0x3A, &[1]);
    h.send(0x3B, &[]);
    let unsupported = [
        (0x08, "Initial Code Setting OTP Program"),
        (0x09, "Write Register for Initial Code Setting"),
        (0x0A, "Read Register for Initial Code Setting"),
        (0x1C, "Write Command to External temperature sensor"),
        (0x25, "Write RAM (Dithering)"),
        (0x2A, "Program VCOM OTP"),
        (0x30, "Program WS OTP"),
        (0x31, "Load WS OTP"),
        (0x34, "CRC calculation"),
        (0x35, "CRC Status Read"),
        (0x36, "Program OTP selection"),
        (0x39, "OTP program mode"),
        (0x4D, "Dithering engine Start/Stop"),
    ];
    for (command, _) in unsupported {
        h.send(command, &[0x80, 0, 0x78, 0]);
    }
    let mut expected = vec![
        D::ReservedCommand { command: 0x3A },
        D::ReservedCommand { command: 0x3B },
    ];
    expected.extend(
        unsupported
            .iter()
            .map(|&(command, name)| D::UnsupportedCommand { command, name }),
    );
    assert_eq!(h.diagnostics(), expected);
    assert!(!h.chip.is_busy(h.now));
    assert_eq!(h.chip.next_deadline(), None);
    // RAM and registers unchanged; the counter still where the first writes left it.
    assert_eq!(
        snapshot(&mut h),
        before,
        "a reserved or unmodelled command changed the state"
    );
    h.send(0x24, &[0x9A]);
    assert_eq!(
        &h.chip.ram(Plane::BlackWhite)[..4],
        [0x12, 0x34, 0x00, 0x9A]
    );
}

#[test]
fn data_without_a_command_is_reported_once() {
    let mut h = Host::new();
    // No command since this reset.
    h.chip.set_reset(h.now, true);
    h.chip.set_reset(h.now, false);
    h.wait();
    h.send_data_raw(&[0x12, 0x34]);
    assert_eq!(h.diagnostics(), [D::DataWithoutCommand { data: 0x12 }]);
}

impl Host {
    fn send_data_raw(&mut self, data: &[u8]) {
        for &byte in data {
            self.chip.write_data(self.now, byte);
        }
    }
}

#[test]
fn extra_and_missing_parameters_are_reported_once_per_command() {
    let mut h = Host::new();
    h.send(0x11, &[0x03, 0x01, 0x02]);
    h.send(0x01, &[0xDF]);
    h.send(0x7F, &[]);
    h.send(0x21, &[0x40]);
    h.send(0x21, &[0x40, 0x00]);
    h.send(0x21, &[0x40, 0x00, 0x00]);
    h.send(0x32, &[0; 70]);
    h.chip.set_reset(h.now, true);
    assert_eq!(
        h.diagnostics(),
        [
            D::ExtraData { command: 0x11 },
            D::MissingData {
                command: 0x01,
                expected: 3,
                received: 1
            },
            D::ExtraData { command: 0x21 },
            D::MissingData {
                command: 0x32,
                expected: 105,
                received: 70
            },
        ]
    );
}

#[test]
fn a_nop_ends_a_ram_write() {
    let mut h = Host::new();
    h.send(0x24, &[0xFF]);
    h.send(0x7F, &[]);
    h.send_data_raw(&[0xEE]);
    assert_eq!(&h.chip.ram(Plane::BlackWhite)[..2], [0xFF, 0x00]);
    assert_eq!(h.diagnostics(), [D::ExtraData { command: 0x7F }]);
}

#[test]
fn a_command_during_busy_is_dropped_but_the_status_reads() {
    let mut h = Host::new();
    h.send(0x22, &[0xC0]);
    h.send(0x20, &[]);
    assert!(h.chip.is_busy(h.now));
    assert!(h.chip.busy_pin(h.now));
    h.send(0x24, &[0x12, 0x34]);
    assert_eq!(h.status(), 0b0000_0101, "busy bit and chip ID");
    assert_eq!(h.chip.ram(Plane::BlackWhite)[0], 0);
    assert_eq!(h.diagnostics(), [D::WhileBusy { command: 0x24 }]);
    assert_eq!(h.wait(), ms(100));
    assert_eq!(h.status(), 0b0000_0001);
}

#[test]
fn the_busy_pin_follows_the_configured_polarity() {
    let mut config = small();
    config.busy_active_high = false;
    let mut h = Host::with(config);
    assert!(
        h.chip.busy_pin(h.now),
        "idle reads high on an inverted line"
    );
    h.send(0x12, &[]);
    assert!(!h.chip.busy_pin(h.now));
    assert!(h.chip.is_busy(h.now));
}

#[test]
fn time_does_not_go_back() {
    let mut h = Host::new();
    h.chip.advance(Cycles(ms(10)));
    h.chip.write_command(Cycles(ms(5)), 0x70);
    let diagnostic = &h.chip.take_diagnostics()[0];
    assert_eq!(diagnostic.at, Cycles(ms(10)));
    h.chip.write_command(Cycles(ms(5)), 0x12);
    assert_eq!(h.chip.next_deadline(), Some(Cycles(ms(12))));
}

#[test]
fn the_diagnostics_kept_are_bounded_and_the_rest_counted() {
    let mut h = Host::new();
    for _ in 0..1_100 {
        h.send(0x70, &[]);
    }
    assert_eq!(h.chip.take_diagnostics().len(), MAX_DIAGNOSTICS);
    assert_eq!(h.chip.dropped_diagnostics(), 1_100 - 1_024);
    h.send(0x70, &[]);
    assert_eq!(h.chip.take_diagnostics().len(), 1);
}

#[test]
fn parameters_a_model_cannot_be_built_on_are_refused() {
    let mut config = small();
    config.panel.width = 0;
    let time_base = Frequency::from_hz(160_000_000).unwrap();
    assert_eq!(
        Ssd1677::new(config, time_base).unwrap_err(),
        ConfigError::EmptyPanel
    );
}

// --- Registers and their reserved values -------------------------------------------------

#[test]
fn driver_output_control_chooses_the_scanned_gates_and_their_order() {
    let mut h = Host::new();
    // Two lines only (and fewer than the 300 the datasheet allows).
    h.send(0x01, &[0x01, 0x00, 0x00]);
    h.fill_rows(0x24, &[0x00; 4]);
    h.update(0xF7);
    assert_eq!((h.pixel(0, 0), h.pixel(0, 1)), (0, 0));
    assert_eq!(
        (h.pixel(0, 2), h.pixel(31, 3)),
        (255, 255),
        "gates 2 and 3 not scanned"
    );
    // SM = 1 on this panel wired in sequence: physical row 1 (G1) shows RAM row 2.
    h.send(0x01, &[0x03, 0x00, 0x02]);
    h.fill_rows(0x24, &[0xFF; 4]);
    h.write_row(0x24, 2, &[0x00; 4]);
    h.update(0xF7);
    let rows: Vec<u8> = (0..4).map(|y| h.pixel(0, y)).collect();
    assert_eq!(rows, [255, 0, 255, 255]);
    // A MUX past 680 lines and TB = 1, both reserved: one report for the command.
    h.send(0x01, &[0xFF, 0x03, 0x01]);
    let value = |value| D::ReservedValue {
        command: 0x01,
        value,
    };
    assert_eq!(h.diagnostics(), [value(0x00), value(0x00), value(0x03)]);
}

#[test]
fn a_mux_past_the_ram_with_sm_leaves_the_gates_past_the_ram_undriven() {
    // A panel of all 680 gates; MUX 2A8h (681 lines) with SM = 1 sends gate 679 to row 680.
    let mut config = small();
    config.panel.height = 680;
    config.panel.width = 8;
    let mut h = Host::with(config);
    h.send(0x01, &[0xA8, 0x02, 0x02]);
    h.send(0x47, &[0x77]);
    h.wait();
    h.update(0xF7);
    let image = h.chip.panel_image();
    let white_rows: Vec<usize> = (0..680).filter(|&y| image.pixels[y * 8] == 255).collect();
    assert_eq!(
        white_rows,
        [679],
        "only the gate past the RAM stays undriven"
    );
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x01,
            value: 0x02
        }]
    );
}

#[test]
fn voltage_registers_check_their_codes_and_booster_and_vcom_control_have_no_effect() {
    let mut h = Host::new();
    h.send(0x03, &[0x17]);
    h.send(0x04, &[0x41, 0xA8, 0x32]);
    h.send(0x2C, &[0x30]);
    h.send(0x0C, &[0xAE, 0xC7, 0xC3, 0xC0, 0x80]);
    h.send(0x2B, &[0x04, 0x63]);
    assert_eq!(h.diagnostics(), []);
    h.send(0x03, &[0x06]);
    h.send(0x04, &[0x22, 0xCF, 0x33]);
    h.send(0x2C, &[0x31]);
    let value = |command, value| D::ReservedValue { command, value };
    // One report per command, its first reserved value (`0x04` had three).
    assert_eq!(
        h.diagnostics(),
        [value(0x03, 0x06), value(0x04, 0x22), value(0x2C, 0x31)]
    );
    h.send(0x04, &[0x41, 0xA8, 0x33]);
    assert_eq!(
        h.diagnostics(),
        [value(0x04, 0x33)],
        "a later byte, reported"
    );
}

#[test]
fn data_entry_mode_y_writes_down_a_column() {
    let mut h = Host::new();
    h.send(0x11, &[0x07]);
    h.send(0x44, &[0x00, 0x00, 0x0F, 0x00]);
    h.send(0x45, &[0x00, 0x00, 0x01, 0x00]);
    h.send(0x24, &[1, 2, 3, 4]);
    let ram = h.chip.ram(Plane::BlackWhite);
    assert_eq!((ram[0], ram[120], ram[1], ram[121]), (1, 2, 3, 4));
    h.send(0x11, &[0x08]);
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x11,
            value: 0x08
        }]
    );
}

#[test]
fn ram_writes_follow_the_window_and_report_where_they_cannot_land() {
    let mut h = Host::new();
    // The last byte of the RAM, a window of one byte: both bytes land there.
    h.send(0x44, &[0xB8, 0x03, 0xBF, 0x03]);
    h.send(0x45, &[0xA7, 0x02, 0xA7, 0x02]);
    h.send(0x4E, &[0xB8, 0x03]);
    h.send(0x4F, &[0xA7, 0x02]);
    h.send(0x26, &[0x11, 0x22]);
    assert_eq!(h.chip.ram(Plane::Red)[120 * 680 - 1], 0x22);
    // A counter past the RAM writes nothing; then it wraps into the window.
    h.send(0x44, &[0x00, 0x00, 0xBF, 0x03]);
    h.send(0x4E, &[0xC0, 0x03]);
    h.send(0x24, &[0xFF, 0x77]);
    let ram = h.chip.ram(Plane::BlackWhite);
    assert_eq!(ram.iter().filter(|&&b| b != 0).count(), 1);
    assert_eq!(
        ram[120 * 679],
        0x77,
        "the second byte, at the start of the window's row"
    );
    // A counter outside the window.
    h.send(0x44, &[0x00, 0x00, 0x07, 0x00]);
    h.send(0x4E, &[0x10, 0x00]);
    h.send(0x24, &[]);
    // A window past the RAM.
    h.send(0x44, &[0x00, 0x00, 0xC0, 0x03]);
    h.send(0x45, &[0xA8, 0x02, 0x00, 0x00]);
    assert_eq!(
        h.diagnostics(),
        [
            D::CounterOutsideWindow {
                command: 0x24,
                x: 0x3C0,
                y: 0x2A7
            },
            D::RamOutOfRange {
                command: 0x24,
                x: 0x3C0,
                y: 0x2A7
            },
            D::CounterOutsideWindow {
                command: 0x24,
                x: 0x10,
                y: 0x2A7
            },
            D::ReservedValue {
                command: 0x44,
                value: 0x03
            },
            D::ReservedValue {
                command: 0x45,
                value: 0x02
            },
        ]
    );
}

#[test]
fn auto_write_fills_red_with_0x46_and_black_white_with_0x47() {
    let mut h = Host::new();
    h.send(0x46, &[0xF7]);
    assert_eq!(h.wait(), ms(2));
    assert!(h.chip.ram(Plane::Red).iter().all(|&b| b == 0xFF));
    assert!(h.chip.ram(Plane::BlackWhite).iter().all(|&b| b == 0x00));
    h.send(0x47, &[0x88]);
    h.wait();
    assert_eq!(h.chip.ram(Plane::BlackWhite)[0], 0xFF);
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x47,
            value: 0x88
        }]
    );
}

#[test]
fn ram_reads_start_with_a_dummy_byte_and_follow_the_read_option() {
    let mut h = Host::new();
    h.send(0x24, &[1, 2]);
    h.send(0x4E, &[0, 0]);
    h.send(0x26, &[3, 4]);
    h.send(0x4E, &[0, 0]);
    assert_eq!(h.read(0x27, 3), [Some(0), Some(1), Some(2)]);
    h.send(0x41, &[0x01]);
    h.send(0x4E, &[0, 0]);
    assert_eq!(h.read(0x27, 3), [Some(0), Some(3), Some(4)]);
    // Past the RAM: nothing drives the line.
    h.send(0x4F, &[0xA8, 0x02]);
    assert_eq!(h.read(0x27, 3), [Some(0), None, None]);
    h.send(0x41, &[0x02]);
    assert_eq!(
        h.diagnostics(),
        [
            D::CounterOutsideWindow {
                command: 0x27,
                x: 16,
                y: 0x2A8
            },
            D::RamOutOfRange {
                command: 0x27,
                x: 16,
                y: 0x2A8
            },
            D::ReservedValue {
                command: 0x41,
                value: 0x02
            }
        ]
    );
}

#[test]
fn a_read_after_a_write_command_drives_nothing_and_is_reported_once() {
    let mut h = Host::new();
    h.send(0x11, &[0x03]);
    assert_eq!(h.chip.read_data(h.now), None);
    assert_eq!(h.chip.read_data(h.now), None);
    assert_eq!(h.diagnostics(), [D::NothingToRead { command: 0x11 }]);
    h.send(0x2F, &[0x00]);
    assert_eq!(h.diagnostics(), [D::ExtraData { command: 0x2F }]);
}

#[test]
fn the_temperature_register_is_written_read_and_loaded() {
    let mut h = Host::new();
    assert_eq!(
        h.read(0x1B, 3),
        [Some(0x7F), Some(0xF0), None],
        "7FFh at reset"
    );
    h.send(0x1A, &[0x5A, 0x00]);
    assert_eq!(h.read(0x1B, 2), [Some(0x5A), Some(0x00)]);
    h.send(0x1A, &[0x19, 0x0F]);
    assert_eq!(h.read(0x1B, 2), [Some(0x19), Some(0x00)]);
    // One byte, as the X4's firmware sends it: A[3:0] is kept.
    h.send(0x1A, &[0x5B]);
    assert_eq!(h.read(0x1B, 2), [Some(0x5B), Some(0x00)]);
    // The internal sensor reads the ambient temperature, in 5 ms.
    h.send(0x18, &[0x80]);
    h.send(0x1A, &[0x00, 0x00]);
    assert_eq!(h.update(0xA0), ms(5));
    assert_eq!(h.read(0x1B, 2), [Some(0x19), Some(0x00)], "25 °C");
    h.chip.set_ambient(Temperature::from_celsius(-10));
    h.update(0xA0);
    assert_eq!(h.read(0x1B, 2), [Some(0xF6), Some(0x00)]);
    // The external sensor of the reset value, with nothing connected: the register stays.
    h.send(0x18, &[0x48]);
    h.send(0x1A, &[0x5A, 0x00]);
    h.update(0xA0);
    assert_eq!(h.read(0x1B, 2), [Some(0x5A), Some(0x00)]);
    h.send(0x18, &[0x47]);
    assert_eq!(
        h.diagnostics(),
        [
            D::ReservedValue {
                command: 0x1A,
                value: 0x0F
            },
            D::NoExternalSensor,
            D::ReservedValue {
                command: 0x18,
                value: 0x47
            }
        ]
    );
    let mut config = small();
    config.external_sensor = Some(Temperature::from_celsius(40));
    let mut h = Host::with(config);
    h.send(0x18, &[0x48]);
    h.update(0xA0);
    assert_eq!(h.read(0x1B, 2), [Some(0x28), Some(0x00)]);
}

#[test]
fn the_status_and_id_reads_give_their_registers() {
    let mut config = small();
    config.user_id = [9; 10];
    let mut h = Host::with(config);
    assert_eq!(h.status(), 0x01);
    assert_eq!(h.read(0x2E, 11)[..10], [Some(9); 10]);
    assert_eq!(h.read(0x2E, 11)[10], None);
    let id: Vec<u8> = (0..10).collect();
    h.send(0x38, &id);
    assert_eq!(
        h.read(0x2E, 10),
        id.iter().map(|&b| Some(b)).collect::<Vec<_>>()
    );
    h.send(0x12, &[]);
    h.wait();
    assert_eq!(
        h.read(0x2E, 1),
        [Some(9)],
        "the OTP's ID again after a reset"
    );
    let option: Vec<u8> = (1..=10).collect();
    h.send(0x37, &option);
    h.send(0x2C, &[0x30]);
    assert_eq!(
        h.read(0x2D, 12),
        [1, 0x30, 2, 3, 4, 5, 6, 7, 8, 9, 10]
            .into_iter()
            .map(Some)
            .chain([None])
            .collect::<Vec<_>>()
    );
    assert_eq!(h.diagnostics(), []);
}

// --- Display Update Control and Master Activation ----------------------------------------

#[test]
fn each_stage_of_update_control_2_takes_its_time() {
    let mut h = Host::new();
    assert_eq!(h.update(0xC0), ms(100), "clock and analog on");
    assert_eq!(h.update(0xC0), 0, "already on");
    assert_eq!(h.update(0x03), ms(200), "analog and clock off");
    assert_eq!(h.update(0x03), 0, "already off");
    assert_eq!(h.update(0x91), ms(1), "clock on, LUT load, clock off");
    assert_eq!(h.diagnostics(), []);
}

#[test]
fn a_stage_without_its_supply_is_skipped_and_reported() {
    let mut h = Host::new();
    assert_eq!(h.update(0x40), 0);
    assert_eq!(h.update(0x20), 0);
    assert_eq!(h.update(0x10), 0);
    assert_eq!(h.update(0x84), 0, "clock on, no analog: no display");
    assert_eq!(h.update(0x01), 0);
    h.send(0x14, &[0x00]);
    h.send(0x15, &[0x04]);
    h.send(0x28, &[]);
    assert_eq!(
        h.diagnostics(),
        [
            D::WithoutClock {
                stage: Stage::AnalogOn
            },
            D::WithoutClock {
                stage: Stage::TemperatureLoad
            },
            D::WithoutClock {
                stage: Stage::LutLoad
            },
            D::WithoutAnalog {
                stage: Stage::Display
            },
            D::WithoutClock {
                stage: Stage::HvReadyDetection
            },
            D::WithoutClock {
                stage: Stage::VciDetection
            },
            D::WithoutClock {
                stage: Stage::VcomSense
            },
        ]
    );
    assert_eq!(h.chip.panel_image().generation, 0);
}

#[test]
fn a_full_update_publishes_its_image_when_the_display_ends_and_busy_falls_after_power_off() {
    let mut h = Host::new();
    h.fill_rows(0x24, &[0x0F, 0xFF, 0x00, 0xFF]);
    h.send(0x22, &[0xF7]);
    h.send(0x20, &[]);
    // Clock 0, analog 100, temperature 5, LUT 1, display 1600, analog off 200.
    let start = h.now;
    assert_eq!(h.chip.next_deadline(), Some(start + ms(1_706)));
    h.skip(ms(1_706));
    let image = h.chip.panel_image();
    assert_eq!((image.generation, image.kind), (1, RefreshKind::Full));
    assert_eq!(image.at, start + ms(1_706));
    assert!(!image.interrupted);
    assert!(h.chip.is_busy(h.now));
    assert_eq!(h.chip.next_deadline(), Some(start + ms(1_906)));
    assert_eq!(h.wait(), ms(200));
    assert_eq!(h.chip.next_deadline(), None);
    let row: Vec<u8> = (0..32).map(|x| h.pixel(x, 2)).collect();
    let mut expected = vec![0; 4];
    expected.extend([255; 12]);
    expected.extend([0; 8]);
    expected.extend([255; 8]);
    assert_eq!(row, expected);
    assert_eq!(h.read(0x1B, 1), [Some(0x19)], "25 °C loaded");
}

#[test]
fn a_mode_two_update_is_partial_and_differential() {
    let mut h = Host::new();
    h.power_on();
    // RED holds the old frame (white), B/W the new one: black on the first two bytes.
    h.fill_rows(0x26, &[0xFF; 4]);
    h.fill_rows(0x24, &[0x00, 0x00, 0xFF, 0xFF]);
    assert_eq!(h.update(0xFC), ms(506));
    let image = h.chip.panel_image();
    assert_eq!((image.generation, image.kind), (1, RefreshKind::Partial));
    // 95 % of a swing toward black: 5 % of 255 is 12.75.
    assert_eq!((h.pixel(0, 0), h.pixel(31, 3)), (13, 255));
    // A page made black by a full update, back to white by a partial one: 95 % of the way.
    h.fill_rows(0x24, &[0x00; 4]);
    h.update(0xF7);
    assert_eq!(h.pixel(0, 0), 0);
    h.fill_rows(0x26, &[0x00; 4]);
    h.fill_rows(0x24, &[0xFF; 4]);
    h.update(0xFC);
    assert_eq!(h.pixel(0, 0), 242, "a trace of the black page stays");
    // A full update clears it.
    h.update(0xF7);
    assert_eq!(h.pixel(0, 0), 255);
}

#[test]
fn a_half_update_loads_the_waveform_of_the_forced_temperature() {
    let mut h = Host::new();
    h.power_on();
    h.fill_rows(0x24, &[0x00; 4]);
    h.send(0x21, &[0x40]);
    h.send(0x1A, &[0x5A, 0x00]);
    // LUT 1, display 1500 (the setting above 50 °C), analog off 200.
    assert_eq!(h.update(0xD7), ms(1_701));
    assert_eq!(h.pixel(5, 1), 0);
}

#[test]
fn no_waveform_for_the_temperature_means_no_display() {
    let mut h = Host::new();
    h.power_on();
    // −128 °C: every range's lower bound is excluded.
    h.send(0x1A, &[0x80, 0x00]);
    h.fill_rows(0x24, &[0x00; 4]);
    assert_eq!(h.update(0xDC), ms(1));
    assert_eq!(h.chip.panel_image().generation, 0);
    assert_eq!(
        h.diagnostics(),
        [D::NoWaveform {
            mode: DisplayMode::Two,
            temperature: Temperature::MIN
        }]
    );
}

#[test]
fn update_control_1_reads_the_planes_normal_as_zero_or_inverted() {
    let mut h = Host::new();
    h.power_on();
    h.fill_rows(0x24, &[0x00; 4]);
    h.fill_rows(0x26, &[0xFF; 4]);
    // RED bypassed as 0 or inverted: LUT0, no drive in mode 2.
    for option in [0x40, 0x80, 0x08] {
        h.send(0x21, &[option]);
        h.update(0xFC);
        assert_eq!(h.pixel(0, 0), 255, "{option:#04x}");
    }
    // Both normal: LUT2, toward black.
    h.send(0x21, &[0x00]);
    h.update(0xFC);
    assert_eq!(h.pixel(0, 0), 13);
    h.send(0x21, &[0x0C]);
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x21,
            value: 0x0C
        }]
    );
}

/// The X4's factory four-level LUT (`FreeInk` `lut_factory_quality`, research §4), 110 bytes.
const FACTORY_GREY: [u8; 110] = [
    0x00, 0x4A, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x80, 0x62, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x88, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0xA8, 0x44, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x08, 0x0B, 0x02, 0x03, 0x00, //
    0x0C, 0x02, 0x07, 0x02, 0x00, //
    0x01, 0x00, 0x02, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x01, //
    0x22, 0x22, 0x22, 0x22, 0x22, //
    0x17, 0x41, 0xA8, 0x32, 0x30,
];

/// Loads a 110-byte LUT as `FreeInk`'s `setCustomLut`: `32`, then `03`, `04`, `2C`.
fn load_lut(h: &mut Host, lut: &[u8; 110]) {
    h.send(0x32, &lut[..105]);
    h.send(0x03, &lut[105..106]);
    h.send(0x04, &lut[106..109]);
    h.send(0x2C, &lut[109..110]);
}

#[test]
fn a_lut_written_by_the_firmware_drives_four_grey_levels() {
    let mut h = Host::new();
    h.power_on();
    // Codes 00, 01, 10, 11 on the four bytes of each row: RED is the high bit.
    h.fill_rows(0x24, &[0x00, 0xFF, 0x00, 0xFF]);
    h.fill_rows(0x26, &[0x00, 0x00, 0xFF, 0xFF]);
    load_lut(&mut h, &FACTORY_GREY);
    h.send(0x21, &[0x00]);
    // 50 frames of 20 ms, no stage but the display: the supplies are on.
    assert_eq!(h.update(0xCC), ms(1_000));
    let levels: Vec<u8> = [0, 8, 16, 24].iter().map(|&x| h.pixel(x, 0)).collect();
    assert_eq!(levels, [255, 153, 128, 0]);
    assert_eq!(h.diagnostics(), []);
    // Code 11 in LUT4 (VCOM) is "N/A".
    let mut lut = FACTORY_GREY;
    lut[40] = 0xC0;
    load_lut(&mut h, &lut);
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x32,
            value: 0xC0
        }]
    );
}

#[test]
fn a_display_of_no_frame_is_published_at_once() {
    let mut h = Host::new();
    h.power_on();
    load_lut(&mut h, &[0; 110]);
    assert_eq!(h.update(0x0C), 0);
    assert_eq!(h.chip.panel_image().generation, 1);
    // The 0 codes of 0x04 are not listed (those of 0x03 and 0x2C are): one report for the command.
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x04,
            value: 0x00
        }]
    );
}

#[test]
fn the_border_follows_a_lut_a_fixed_level_or_nothing() {
    let mut h = Host::new();
    h.fill_rows(0x24, &[0xFF; 4]);
    // HiZ at reset: untouched.
    h.update(0xF7);
    assert_eq!(h.chip.panel_image().border, 255);
    // Fixed at VSH1 for the whole display: black.
    h.send(0x3C, &[0x50]);
    h.update(0xF7);
    assert_eq!(h.chip.panel_image().border, 0);
    // Following LUT1 (white) of the full waveform.
    h.send(0x3C, &[0x01]);
    h.update(0xF7);
    assert_eq!(h.chip.panel_image().border, 255);
    // Following LUT0 (black).
    h.send(0x3C, &[0x00]);
    h.update(0xF7);
    assert_eq!(h.chip.panel_image().border, 0);
    // VCOM: untouched.
    h.send(0x3C, &[0x80]);
    h.update(0xF7);
    assert_eq!(h.chip.panel_image().border, 0);
    h.send(0x3C, &[0xC4]);
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x3C,
            value: 0xC4
        }]
    );
}

#[test]
fn a_hardware_reset_during_the_display_keeps_what_the_glass_reached() {
    let mut h = Host::new();
    h.fill_rows(0x24, &[0x00; 4]);
    h.send(0x22, &[0xF7]);
    h.send(0x20, &[]);
    // The display starts at 106 ms; LUT0's first group (20 frames toward white) is done at 506
    // ms, five frames of the second (toward black) at 606 ms: 25 % of a swing.
    h.skip(ms(606));
    h.chip.set_reset(h.now, true);
    let image = h.chip.panel_image();
    assert_eq!((image.generation, image.interrupted), (1, true));
    assert_eq!(h.pixel(0, 0), 191);
    assert!(!h.chip.is_busy(h.now));
    assert_eq!(h.chip.next_deadline(), None);
    assert_eq!(h.diagnostics(), [D::ResetDuringDisplay]);
}

// --- Deep sleep and detections -----------------------------------------------------------

#[test]
fn deep_sleep_mode_11_loses_the_ram_and_only_a_hardware_reset_wakes_it() {
    let mut h = Host::new();
    h.send(0x24, &[0xAB]);
    h.send(0x10, &[0x03]);
    assert!(h.chip.is_busy(h.now));
    assert_eq!(h.chip.next_deadline(), None);
    assert!(h.chip.ram(Plane::BlackWhite).iter().all(|&b| b == 0));
    h.send(0x12, &[]);
    assert_eq!(h.chip.read_data(h.now), None);
    assert_eq!(h.diagnostics(), [D::WhileAsleep { command: 0x12 }]);
    h.chip.set_reset(h.now, true);
    h.chip.set_reset(h.now, false);
    assert_eq!(h.wait(), ms(1));
    h.send(0x24, &[0xCD]);
    assert_eq!(h.chip.ram(Plane::BlackWhite)[0], 0xCD);
}

#[test]
fn deep_sleep_codes_00_01_and_extra_bits() {
    let mut h = Host::new();
    h.send(0x24, &[0xAB]);
    h.send(0x10, &[0x00]);
    assert!(!h.chip.is_busy(h.now));
    h.send(0x10, &[0x01]);
    assert!(h.chip.is_busy(h.now));
    assert_eq!(h.chip.ram(Plane::BlackWhite)[0], 0xAB, "kept");
    h.chip.set_reset(h.now, true);
    h.chip.set_reset(h.now, false);
    h.wait();
    h.send(0x10, &[0x07]);
    assert!(h.chip.ram(Plane::BlackWhite).iter().all(|&b| b == 0));
    let value = |value| D::ReservedValue {
        command: 0x10,
        value,
    };
    assert_eq!(h.diagnostics(), [value(0x01), value(0x07)]);
}

#[test]
fn hv_ready_detection_waits_its_cool_down() {
    let mut h = Host::new();
    h.power_on();
    h.send(0x14, &[0x20]);
    assert_eq!(h.wait(), ms(30));
    assert_eq!(h.status() & 0x20, 0);
    h.send(0x14, &[0x88]);
    assert_eq!(h.wait(), ms(10));
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x14,
            value: 0x88
        }]
    );
}

#[test]
fn vci_detection_compares_the_supply_with_the_level() {
    let mut config = small();
    config.vci_millivolts = 2_250;
    let mut h = Host::with(config);
    h.power_on();
    h.send(0x15, &[0x04]);
    assert_eq!(h.wait(), ms(1));
    assert_eq!(h.status(), 0b0001_0001, "below 2.3 V");
    h.send(0x15, &[0x03]);
    h.wait();
    assert_eq!(h.status(), 0b0000_0001, "above 2.2 V");
    h.send(0x15, &[0x01]);
    h.wait();
    assert_eq!(h.status(), 0b0001_0001, "a reserved level reads 2.3 V");
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x15,
            value: 0x01
        }]
    );
}

#[test]
fn vcom_sensing_holds_busy_for_its_duration() {
    let mut h = Host::new();
    h.power_on();
    h.send(0x28, &[]);
    assert_eq!(h.wait(), ms(10_000), "49h at reset: 10 s");
    h.send(0x29, &[0x41]);
    h.send(0x28, &[]);
    assert_eq!(h.wait(), ms(2_000));
    h.send(0x29, &[0x01]);
    assert_eq!(
        h.diagnostics(),
        [D::ReservedValue {
            command: 0x29,
            value: 0x01
        }]
    );
}

#[test]
fn hv_ready_detection_without_the_supplies_reads_not_ready() {
    let mut h = Host::new();
    h.send(0x14, &[0x00]);
    assert!(!h.chip.is_busy(h.now));
    assert_eq!(h.status(), 0b0010_0001, "bit 5: not ready");
    h.power_on();
    h.send(0x14, &[0x00]);
    assert_eq!(h.wait(), ms(10));
    assert_eq!(h.status(), 0b0000_0001);
    h.update(0x03);
    h.send(0x14, &[0x00]);
    assert_eq!(h.status(), 0b0010_0001);
    let skipped = D::WithoutClock {
        stage: Stage::HvReadyDetection,
    };
    assert_eq!(h.diagnostics(), [skipped.clone(), skipped]);
    // A reset clears the flag.
    h.send(0x12, &[]);
    h.wait();
    assert_eq!(h.status(), 0b0000_0001);
}

// --- Resets during an operation, BUSY polarity, RAM ping-pong ---------------------------

#[test]
fn a_hardware_reset_before_the_display_starts_abandons_it() {
    let mut h = Host::new();
    h.fill_rows(0x24, &[0x00; 4]);
    h.send(0x22, &[0xF7]);
    h.send(0x20, &[]);
    // The display starts at 106 ms (analog on, temperature, LUT): a reset at 50 ms comes first.
    h.skip(ms(50));
    h.chip.set_reset(h.now, true);
    let image = h.chip.panel_image();
    assert_eq!((image.generation, image.kind), (0, RefreshKind::PowerOn));
    assert!(image.pixels.iter().all(|&p| p == 255));
    assert_eq!(h.chip.next_deadline(), None);
    assert_eq!(h.diagnostics(), []);
    // At the instant it starts, the display has begun though no frame has run: published as cut,
    // the glass unchanged.
    h.chip.set_reset(h.now, false);
    h.wait();
    h.send(0x18, &[0x80]);
    h.send(0x22, &[0xF7]);
    h.send(0x20, &[]);
    h.skip(ms(106));
    h.chip.set_reset(h.now, true);
    let image = h.chip.panel_image();
    assert_eq!((image.generation, image.interrupted), (1, true));
    assert!(image.pixels.iter().all(|&p| p == 255));
    assert_eq!(h.diagnostics(), [D::ResetDuringDisplay]);
}

#[test]
fn a_hardware_reset_cuts_a_busy_operation_without_a_display() {
    type Start = fn(&mut Host);
    let starts: [(&str, Start); 3] = [
        ("SW reset", |h| h.send(0x12, &[])),
        ("auto write", |h| h.send(0x46, &[0xF7])),
        ("analog on", |h| {
            h.send(0x22, &[0xC0]);
            h.send(0x20, &[]);
        }),
    ];
    for (name, start) in starts {
        let mut h = Host::new();
        start(&mut h);
        assert!(h.chip.is_busy(h.now), "{name}");
        h.skip(ms(1) / 2);
        h.chip.set_reset(h.now, true);
        assert!(!h.chip.is_busy(h.now), "{name}");
        assert_eq!(h.chip.next_deadline(), None, "{name}");
        h.chip.set_reset(h.now, false);
        assert_eq!(h.chip.next_deadline(), Some(h.now + ms(1)), "{name}");
        assert_eq!(h.wait(), ms(1), "{name}");
        // The supplies are off again: a display needs them started anew.
        assert_eq!(h.update(0x04), 0, "{name}");
        assert_eq!(
            h.diagnostics(),
            [D::WithoutClock {
                stage: Stage::Display
            }],
            "{name}"
        );
    }
}

#[test]
fn an_inverted_busy_pin_reads_high_in_reset_and_low_in_deep_sleep() {
    let mut config = small();
    config.busy_active_high = false;
    let mut h = Host::with(config);
    h.chip.set_reset(h.now, true);
    assert!(!h.chip.is_busy(h.now));
    assert!(
        h.chip.busy_pin(h.now),
        "not busy while RES# is low: high on an inverted line"
    );
    h.chip.set_reset(h.now, false);
    assert!(!h.chip.busy_pin(h.now), "busy for the reset time");
    h.wait();
    assert!(h.chip.busy_pin(h.now));
    h.send(0x10, &[0x03]);
    assert!(h.chip.is_busy(h.now));
    assert!(
        !h.chip.busy_pin(h.now),
        "busy in deep sleep: low on an inverted line"
    );
    h.skip(ms(60_000));
    assert!(!h.chip.busy_pin(h.now));
}

#[test]
fn ram_ping_pong_in_display_mode_two_is_reported() {
    let mut h = Host::new();
    h.power_on();
    // 0x37 byte F, bit 6: RAM ping-pong, for display mode 2 only.
    h.send(0x37, &[0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0]);
    h.update(0xFC);
    h.update(0xF7);
    assert_eq!(h.diagnostics(), [D::RamPingPong]);
    assert_eq!(h.chip.panel_image().generation, 2, "both displays ran");
}

/// A deterministic xorshift generator for the test below.
fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn no_sequence_of_bytes_resets_and_reads_makes_the_chip_panic() {
    // All 680 gates, so that a MUX and a scan order the firmware sets reach every row.
    let mut config = small();
    config.panel.width = 8;
    config.panel.height = 680;
    let mut h = Host::with(config);
    let commands: Vec<u8> = (0..=u8::MAX).filter(|&c| kind(c).is_some()).collect();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    for _ in 0..100_000 {
        let draw = xorshift(&mut state);
        let [operation, choice, byte, data, ..] = draw.to_le_bytes();
        match operation % 16 {
            0..=4 => {
                let command = if choice % 4 == 0 {
                    byte
                } else {
                    commands[usize::from(byte) % commands.len()]
                };
                h.chip.write_command(h.now, command);
            }
            5..=11 => h.chip.write_data(h.now, data),
            12 => {
                let _ = h.chip.read_data(h.now);
            }
            13 => h.chip.set_reset(h.now, choice % 2 == 0),
            14 => {
                if let Some(next) = h.chip.next_deadline() {
                    h.now = h.now.max(next);
                    h.chip.advance(h.now);
                }
            }
            _ => h.skip(u64::from(choice) * 1_000),
        }
        let _ = h.chip.take_diagnostics();
    }
    assert!(h.chip.panel_image().generation > 0, "displays ran");
}
