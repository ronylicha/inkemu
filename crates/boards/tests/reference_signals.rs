//! The peripherals of the ESP32-C3 through which the X4 reaches its chips (sub-project 2b), driven
//! by the ESP-IDF drivers of `tools/reference-firmware/soc_signals`, booted from the real mask ROM
//! behind the bootloader inkemu ships, with chips attached to the pins of the X4 through the signal
//! traits of `emu-core` (ADR 0010): a panel on GP-SPI2 (hardware chip select GPIO21, data/command
//! GPIO4) whose reset (GPIO5) it hears and whose BUSY line (GPIO6) it drives, an SD card on the same
//! bus (software chip select GPIO12), and a voltage on GPIO1 for the SAR ADC. No chip sits on I2C0.
//!
//! The image is named by `INKEMU_SOC_SIGNALS_APP` and checked against the hash pinned beside its
//! build, `tools/reference-firmware/SHA256SUMS.inputs-soc-signals`:
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_SOC_SIGNALS_APP=<soc_signals.bin> cargo test --release -p boards --test reference_signals -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the image and the ROM the environment names

use boards::MachineOptions;
use emu_core::{AnalogSource, GpioInput, GpioOutput, Level, Nanos, SpiTarget};
use sha2::{Digest, Sha256};
use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
use soc_esp32c3::{ConsoleChannel, Esp32C3, RomImage, SpiWires, UsbHost};
use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::Path;
use std::rc::Rc;

/// The edges the panel gives its BUSY line after its reset is released, and their spacing.
const BUSY_EDGES: u64 = 5;
const BUSY_EDGE_NS: u64 = 1_000_000;
/// The voltage on GPIO1, half of the 3.3 V supply, as a battery divider gives it.
const GPIO1_MILLIVOLTS: u32 = 1_650;
/// The trace of the run below, measured on 2026-10-15 with every other assertion of the test
/// passing (release build, twice, the same).
const TRACE_HASH: u64 = 0x08d6_3d64_7e5a_8a29;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn rom() -> RomImage {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").expect(
        "set INKEMU_ROM_ELF_DIR: fetch the ROMs with `inkemu rom fetch --all --rom-dir <dir>`",
    );
    let elf = std::fs::read(Path::new(&dir).join("esp32c3_rev3_rom.elf")).unwrap();
    RomImage::from_elf(&elf).unwrap()
}

/// `soc_signals.bin`, checked against the hash `tools/reference-firmware/SHA256SUMS.inputs-soc-signals`
/// pins: the one place it is written.
fn application() -> Vec<u8> {
    let path = std::env::var("INKEMU_SOC_SIGNALS_APP").expect(
        "set INKEMU_SOC_SIGNALS_APP to the soc_signals.bin of tools/reference-firmware/build.sh",
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let sums = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/reference-firmware/SHA256SUMS.inputs-soc-signals");
    let text = std::fs::read_to_string(&sums).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; the image is not pinned yet: see tools/reference-firmware/README.md",
            sums.display()
        )
    });
    let pinned = text
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name == "soc_signals.bin").then_some(hash)
        })
        .unwrap_or_else(|| panic!("{}: no line for soc_signals.bin", sums.display()));
    assert_eq!(
        hex(&Sha256::digest(&bytes)),
        pinned,
        "{path} is not the pinned soc_signals.bin; rebuild it with tools/reference-firmware/build.sh"
    );
    bytes
}

/// The panel: it hears its reset line, drives its BUSY line and receives the frames of GP-SPI2
/// while selected, answering each byte with its complement.
#[derive(Debug, Default)]
struct Panel {
    /// What the `SoC` drove on the reset line, change by change.
    reset: Vec<Option<Level>>,
    /// The instant the reset was released after being held low: BUSY toggles from then on.
    released: Option<Nanos>,
    /// Each change of its chip select, with its instant.
    selects: Vec<(bool, Nanos)>,
    /// Each frame with the level of the data/command line.
    frames: Vec<(Option<Level>, Vec<u8>)>,
    /// The instant of each frame.
    frames_at: Vec<Nanos>,
}

impl Panel {
    /// The edges of BUSY up to `at`.
    fn edges_by(&self, at: Nanos) -> u64 {
        self.released.map_or(0, |from| {
            (at.get().saturating_sub(from.get()) / BUSY_EDGE_NS).min(BUSY_EDGES)
        })
    }
}

impl GpioOutput for Panel {
    fn driven(&mut self, level: Option<Level>, at: Nanos) {
        if level == Some(Level::High)
            && self.released.is_none()
            && self.reset.contains(&Some(Level::Low))
        {
            self.released = Some(at);
        }
        self.reset.push(level);
    }
}

impl GpioInput for Panel {
    /// Low until the first edge, then toggling at each.
    fn level(&mut self, at: Nanos) -> Option<Level> {
        Some(Level::from_bool(self.edges_by(at) % 2 == 1))
    }

    fn next_change(&mut self, at: Nanos) -> Option<Nanos> {
        let from = self.released?;
        let edges = self.edges_by(at);
        (edges < BUSY_EDGES).then(|| Nanos(from.get() + (edges + 1) * BUSY_EDGE_NS))
    }
}

impl SpiTarget for Panel {
    fn select(&mut self, selected: bool, at: Nanos) {
        self.selects.push((selected, at));
    }

    fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], miso: &mut [u8], at: Nanos) {
        self.frames.push((dc, mosi.to_vec()));
        self.frames_at.push(at);
        for (answer, byte) in miso.iter_mut().zip(mosi) {
            *answer = !byte;
        }
    }
}

/// The SD card: it answers each byte with the byte plus one.
#[derive(Debug, Default)]
struct Card {
    selects: Vec<bool>,
    frames: Vec<Vec<u8>>,
}

impl SpiTarget for Card {
    fn select(&mut self, selected: bool, _at: Nanos) {
        self.selects.push(selected);
    }

    fn transfer(&mut self, _dc: Option<Level>, mosi: &[u8], miso: &mut [u8], _at: Nanos) {
        self.frames.push(mosi.to_vec());
        for (answer, byte) in miso.iter_mut().zip(mosi) {
            *answer = byte.wrapping_add(1);
        }
    }
}

#[derive(Debug)]
struct Supply(u32);

impl AnalogSource for Supply {
    fn millivolts(&mut self, _at: Nanos) -> u32 {
        self.0
    }
}

/// The X4 with `soc_signals` behind the bootloader inkemu ships, as `inkemu run --firmware`
/// composes it, its USB host open (the firmware's console is USB-Serial-JTAG).
fn x4() -> Esp32C3 {
    let profile = boards::builtin("x4").unwrap();
    let flash = profile.compose_flash(&application(), None).unwrap();
    let options = MachineOptions {
        strict_mmio: true,
        trace_hash: true,
        usb_host: UsbHost::Open,
    };
    profile
        .instantiate_with_flash(rom(), flash.bytes, options)
        .unwrap()
}

/// The console line that starts with `prefix`, without it.
fn line_after<'a>(console: &'a str, prefix: &str) -> &'a str {
    console
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no line `{prefix}...`:\n{console}"))
}

/// Attaches the panel (reset GPIO5, BUSY GPIO6, GP-SPI2 under chip select GPIO21 with its
/// data/command line on GPIO4), the SD card (GP-SPI2 under GPIO12) and the voltage on GPIO1: the
/// pins of the X4.
fn attach_the_chips(machine: &mut Esp32C3) -> (Rc<RefCell<Panel>>, Rc<RefCell<Card>>) {
    let panel = Rc::new(RefCell::new(Panel::default()));
    let card = Rc::new(RefCell::new(Card::default()));
    machine
        .attach_output(5, Box::new(Rc::clone(&panel)))
        .unwrap();
    machine
        .attach_input(6, Box::new(Rc::clone(&panel)))
        .unwrap();
    let bus = |cs, dc| SpiWires {
        sclk: 8,
        mosi: 10,
        miso: Some(7),
        cs,
        dc,
    };
    machine
        .attach_spi(bus(21, Some(4)), Box::new(Rc::clone(&panel)))
        .unwrap();
    machine
        .attach_spi(bus(12, None), Box::new(Rc::clone(&card)))
        .unwrap();
    machine
        .attach_analog(1, Box::new(Supply(GPIO1_MILLIVOLTS)))
        .unwrap();
    (panel, card)
}

/// GP-SPI2: the command with DC low, the data with DC high, each answered by the panel; CMD0 to
/// the card under its software chip select. Each chip was selected for its own frames only.
fn assert_the_spi_frames(panel: &Panel, card: &Card, console: &str) {
    assert_eq!(
        panel.frames,
        [
            (Some(Level::Low), vec![0x12]),
            (Some(Level::High), vec![0x01, 0x80]),
        ]
    );
    // The panel's chip select is low once without a frame, for a few instructions, as on a chip:
    // `spicommon_cs_initialize` makes GPIO21 a plain GPIO output first (`gpio_output_enable`
    // routes `GPIO_OUT`, low, and selects the GPIO function), then connects `FSPICS0` to it
    // (ESP-IDF v5.5.2 `spi_common.c:756-768`, `gpio.c:215-222`). Then one selection per frame.
    let selects: Vec<bool> = panel
        .selects
        .iter()
        .map(|&(selected, _)| selected)
        .collect();
    assert_eq!(
        selects,
        [true, false, true, false, true, false],
        "selects {:?}, frames at {:?}",
        panel.selects,
        panel.frames_at
    );
    let pulse = panel.selects[1].1.get() - panel.selects[0].1.get();
    assert!(
        pulse < 10_000 && panel.selects[1].1 < panel.frames_at[0],
        "the routing pulse lasts {pulse} ns, before the first frame: {:?}",
        panel.selects
    );
    assert_eq!(line_after(console, "soc_signals: panel answered "), "fe 7f");
    assert_eq!(card.frames, [vec![0x40, 0x00, 0x00, 0x00, 0x00, 0x95]]);
    assert_eq!(card.selects, [true, false]);
    assert_eq!(
        line_after(console, "soc_signals: card answered "),
        "41 01 01 01 01 96"
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_SOC_SIGNALS_APP"]
fn the_esp_idf_drivers_reach_the_chips_attached_to_the_pins_of_the_x4() {
    let mut machine = x4();
    let (panel, card) = attach_the_chips(&mut machine);
    let mut consoles = Consoles::default();
    let bounds = Bounds {
        slice: 10_000_000,
        max_cycles: 2_000_000_000,
        max_nanos: 10_000_000_000,
    };
    let driven = drive(&mut machine, bounds, &mut consoles, |_, _, seen| {
        seen.text(ConsoleChannel::UsbSerialJtag)
            .contains("soc_signals: done\n")
    });
    let console = consoles.text(ConsoleChannel::UsbSerialJtag);
    assert_eq!(driven, Driven::Done, "{console}");
    assert_eq!(consoles.diagnostics, [], "{console}");

    // GPIO: the reset held low then released once, the five edges of BUSY counted by the
    // interrupt handler, BUSY high after them.
    let panel = panel.borrow();
    assert!(
        panel
            .reset
            .ends_with(&[Some(Level::Low), Some(Level::High)]),
        "{:?}",
        panel.reset
    );
    assert_eq!(
        line_after(&console, "soc_signals: busy level "),
        "0 before the reset is released"
    );
    assert_eq!(
        line_after(&console, "soc_signals: busy edges "),
        "5, busy level 1"
    );

    assert_the_spi_frames(&panel, &card.borrow(), &console);

    // The SAR ADC: the calibration of the chip's eFuse brings the reading back to the voltage
    // within a step of the 12-bit converter.
    let adc = line_after(&console, "soc_signals: adc raw ");
    let millivolts: u32 = adc
        .split_once(", ")
        .and_then(|(_, mv)| mv.strip_suffix(" mV"))
        .and_then(|mv| mv.parse().ok())
        .unwrap_or_else(|| panic!("`{adc}`"));
    assert!(
        millivolts.abs_diff(GPIO1_MILLIVOLTS) <= 2,
        "{millivolts} mV read for {GPIO1_MILLIVOLTS} mV"
    );

    // I2C0: no chip acknowledges the address, as on an X4.
    assert_eq!(
        line_after(&console, "soc_signals: i2c probe of 0x68: "),
        "ESP_ERR_NOT_FOUND"
    );
    assert_eq!(
        machine.trace_hash(),
        Some(TRACE_HASH),
        "the trace hash of this run is {:#018x}: when the run above is the one intended, pin it in TRACE_HASH",
        machine.trace_hash().unwrap_or_default()
    );
}
