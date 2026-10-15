//! The wired X3 (sub-project 3, ADR 0015) with a firmware that uses its board as a firmware does:
//! `tools/reference-firmware/x3_panel_sensors`, booted from the real mask ROM behind the bootloader
//! inkemu ships, powered on by the gesture of the profile. It powers the card slot, reads the
//! BQ27220, the DS3231 and the QMI8658 through the I2C driver of ESP-IDF, sets the clock, probes an
//! address nobody answers, drives the UC8253 for a full refresh and a refresh in partial mode, and
//! goes into deep sleep woken by its power key once the gesture lets the key go. The tests read
//! what it prints, what the glass shows, what it left in the chips and how the run ends.
//!
//! The image is named by `INKEMU_X3_PANEL_APP` and checked against the hash pinned beside its
//! build, `tools/reference-firmware/SHA256SUMS.inputs-x3-panel-sensors`:
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_X3_PANEL_APP=<x3_panel_sensors.bin> cargo test --release -p boards --test reference_x3 -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the image and the ROM the environment names

use boards::{ButtonsHandle, I2cHandle, MachineOptions, PanelHandle, Wired, WiredOptions};
use chips::DateTime;
use chips::uc8253::{PanelImage, RefreshKind};
use emu_core::signals::Nanos;
use sha2::{Digest, Sha256};
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::reset::ResetCause;
use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, RomImage, UsbHost};
use std::fmt::Write as _;
use std::path::Path;

const MILLISECOND: u64 = 1_000_000;
/// How long the test holds the power key to wake the X3: five readings of a firmware polling
/// every 20 ms.
const HOLD_NS: u64 = 100 * MILLISECOND;
/// Every line the firmware prints starts with it.
const PREFIX: &str = "x3_panel_sensors: ";
/// The last line of a life: the power key let go, the deep sleep next.
const ASLEEP: &str = "x3_panel_sensors: power key released, deep sleep until it is pressed\n";
/// `WAKEUP_ENA` bit 2: GPIO (`rtc.h:664-672`).
const WAKE_ON_GPIO: u32 = 1 << 2;
/// The trace of the run below, measured on 2026-10-15 with every other assertion of the test
/// passing (release build, twice, the same).
const TRACE_HASH: u64 = 0xaa63_f902_ff80_c010;

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

/// `x3_panel_sensors.bin`, checked against the hash
/// `tools/reference-firmware/SHA256SUMS.inputs-x3-panel-sensors` pins: the one place it is written.
fn application() -> Vec<u8> {
    let path = std::env::var("INKEMU_X3_PANEL_APP").expect(
        "set INKEMU_X3_PANEL_APP to the x3_panel_sensors.bin of tools/reference-firmware/build.sh",
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let sums = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/reference-firmware/SHA256SUMS.inputs-x3-panel-sensors");
    let text = std::fs::read_to_string(&sums).unwrap_or_else(|e| panic!("{}: {e}", sums.display()));
    let pinned = text
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name == "x3_panel_sensors.bin").then_some(hash)
        })
        .unwrap_or_else(|| panic!("{}: no line for x3_panel_sensors.bin", sums.display()));
    assert_eq!(
        hex(&Sha256::digest(&bytes)),
        pinned,
        "{path} is not the pinned x3_panel_sensors.bin; rebuild it with tools/reference-firmware/build.sh"
    );
    bytes
}

/// The X3 with the firmware behind the bootloader inkemu ships, as `inkemu run --firmware`
/// composes it, powered on by the gesture of its profile, its slot empty; the USB host is open
/// (the firmware's console is USB-Serial-JTAG).
fn power_on() -> (Esp32C3, PanelHandle, ButtonsHandle, I2cHandle) {
    let profile = boards::builtin("x3").unwrap();
    let flash = profile.compose_flash(&application(), None).unwrap();
    let options = MachineOptions {
        strict_mmio: true,
        trace_hash: true,
        usb_host: UsbHost::Open,
    };
    let Wired {
        machine,
        panel,
        buttons,
        i2c,
        ..
    } = Wired::power_on(
        &profile,
        rom(),
        Some(flash.bytes),
        options,
        WiredOptions::default(),
    )
    .unwrap();
    (machine, panel.unwrap(), buttons.unwrap(), i2c.unwrap())
}

const BOUNDS: Bounds = Bounds {
    slice: 10_000_000,
    max_cycles: 4_000_000_000,
    max_nanos: 20_000_000_000,
};

/// Drives the machine until its console holds `text` `count` times: a line each life prints.
fn until_nth(machine: &mut Esp32C3, consoles: &mut Consoles, text: &str, count: usize) {
    let driven = drive(machine, BOUNDS, consoles, |_, _, seen| {
        seen.text(ConsoleChannel::UsbSerialJtag)
            .matches(text)
            .count()
            >= count
    });
    assert_eq!(
        driven,
        Driven::Done,
        "`{text}` never came {count} times:\n{}",
        consoles.text(ConsoleChannel::UsbSerialJtag)
    );
}

/// Drives the machine until the chip is in deep sleep.
fn until_deep_sleep(machine: &mut Esp32C3, consoles: &mut Consoles) {
    let driven = drive(machine, BOUNDS, consoles, |machine, _, _| {
        machine.deep_sleep().is_some()
    });
    assert_eq!(
        driven,
        Driven::Done,
        "no deep sleep:\n{}",
        consoles.text(ConsoleChannel::UsbSerialJtag)
    );
}

/// Presses `id` `delay` nanoseconds from now and releases it [`HOLD_NS`] later; returns when it is
/// pressed.
fn press_after(machine: &mut Esp32C3, buttons: &ButtonsHandle, id: &str, delay: u64) -> u64 {
    let pressed = machine.device_time_nanos() + delay;
    for (at, down) in [(pressed, true), (pressed + HOLD_NS, false)] {
        let input = buttons.button_input(id, down).unwrap();
        machine
            .send_board_input(TIME_BASE.nanos_to_cycles(u128::from(at)), input)
            .unwrap();
    }
    pressed
}

/// The lines the firmware printed, in order, without their prefix.
fn lines(consoles: &Consoles) -> Vec<String> {
    consoles
        .text(ConsoleChannel::UsbSerialJtag)
        .lines()
        .filter_map(|line| line.strip_prefix(PREFIX))
        .map(str::to_owned)
        .collect()
}

const WIDTH: usize = 792;
const HEIGHT: usize = 528;

/// A rectangle of the glass, [left, right) × [top, bottom).
#[derive(Clone, Copy, Debug)]
struct Rect {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl Rect {
    const fn new(left: usize, top: usize, right: usize, bottom: usize) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn holds(self, x: usize, y: usize) -> bool {
        (self.left..self.right).contains(&x) && (self.top..self.bottom).contains(&y)
    }
}

/// The square the refresh in partial mode adds (`SQUARE_X`, `SQUARE_Y`, `SQUARE_SIDE`).
const PARTIAL_SQUARE: Rect = Rect::new(400, 300, 464, 364);

/// The image the firmware draws, built from its definition (`panel_part`), the battery at
/// `percent`: a white glass with a black frame of four pixels, a square in the top-left corner and
/// a bar four pixels long per percent, plus the square of the refresh in partial mode when
/// `partial` asks. The waveform of the firmware drives each pixel for 20 frames toward one rail,
/// a full swing under the model, so every pixel is 0 or 255 and is compared byte by byte.
fn expected(percent: usize, partial: bool) -> Vec<u8> {
    let mut black = vec![
        Rect::new(0, 0, WIDTH, 4),
        Rect::new(0, HEIGHT - 4, WIDTH, HEIGHT),
        Rect::new(0, 0, 4, HEIGHT),
        Rect::new(WIDTH - 4, 0, WIDTH, HEIGHT),
        Rect::new(16, 16, 80, 80),
        Rect::new(16, 100, 16 + 4 * percent, 124),
    ];
    if partial {
        black.push(PARTIAL_SQUARE);
    }
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .map(|(x, y)| {
            if black.iter().any(|r| r.holds(x, y)) {
                0
            } else {
                255
            }
        })
        .collect()
}

/// The image the glass showed last, checked to be of `generation` and `kind`.
fn shown(panel: &PanelHandle, generation: u64, kind: RefreshKind) -> PanelImage {
    let image = panel.image().clone();
    assert_eq!(
        (usize::from(image.width), usize::from(image.height)),
        (WIDTH, HEIGHT)
    );
    assert_eq!(
        (image.generation, image.kind, image.interrupted),
        (generation, kind, false),
        "the glass after refresh {generation}"
    );
    image
}

/// What a life prints, `woken` the first line of a life the power key woke, `before` and `after`
/// the clock's two readings: the chips' values from the profile, the panel's busy times from the
/// model (`chips::uc8253`), an absent address refused.
fn life(woken: bool, before: &str, after: &str) -> Vec<String> {
    [
        if woken {
            "start, woken by the power key, card powered"
        } else {
            "start, card powered"
        },
        "gauge 3850 mV, 70 %, -15 mA, design 650 mAh",
        "gauge device number 0x0220",
        "clock status 0x08, control 0x04",
        before,
        after,
        "motion who am i 0x05, revision 0x68",
        "motion status 0x03, temperature 25 + 0/256 C",
        "motion acceleration 0 0 16384, rate 0 0 0",
        "probe of 0x6a: ESP_ERR_NOT_FOUND",
        "power on, busy 127 ms",
        "full refresh, busy 440 ms",
        "partial refresh, busy 440 ms",
        "power off, busy 30 ms",
        "panel asleep",
        "power key released, deep sleep until it is pressed",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// The firmware's two lives on the wired X3: the three chips of the I2C bus answer with the values
/// of the profile, the clock counts from the profile's start and then from the time the firmware
/// set, through the deep sleep of the chip; the glass shows the full refresh, then the square of
/// the refresh in partial mode and nothing else changed; the power key, once the gesture lets it
/// go, sends the X3 into deep sleep, and a press wakes it into a second life.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_X3_PANEL_APP"]
fn the_x3_reads_its_chips_draws_its_panel_and_sleeps_until_its_power_key() {
    let (mut machine, panel, buttons, i2c) = power_on();
    let mut consoles = Consoles::default();

    // The full refresh: the line comes once BUSY rose, the refresh over.
    until_nth(&mut machine, &mut consoles, "full refresh, busy ", 1);
    let full = shown(&panel, 1, RefreshKind::Full);
    assert!(
        full.pixels == expected(70, false),
        "the glass after the full refresh is not the frame, the corner square and the bar of 70 %"
    );

    until_nth(&mut machine, &mut consoles, ASLEEP, 1);
    let partial = shown(&panel, 2, RefreshKind::Partial);
    assert!(
        partial.pixels == expected(70, true),
        "the refresh in partial mode changed more, or less, than its square"
    );
    until_deep_sleep(&mut machine, &mut consoles);
    let console = consoles.text(ConsoleChannel::UsbSerialJtag);
    assert_eq!(
        lines(&consoles),
        life(
            false,
            "clock at start 2026-01-01T00:00:00, day 4",
            "clock 1.5 s after it was set 2026-10-15T12:34:57, day 4",
        ),
        "{console}"
    );
    let sleep = machine.deep_sleep().unwrap();
    assert_eq!(
        (sleep.wake_sources & WAKE_ON_GPIO, sleep.timer_wake_nanos),
        (WAKE_ON_GPIO, None),
        "woken by GPIO only"
    );

    // What the firmware left in the chips: the clock's control and the time it set, still
    // counting; the motion sensor's two sensors on.
    let at = Nanos(machine.device_time_nanos());
    {
        let clock = i2c.clock().unwrap();
        assert_eq!(clock.register(0x0E, at), Some(0x04), "the clock's control");
        let now = clock.now(at);
        let set = DateTime::new(2026, 10, 15, 12, 34, 56).unwrap();
        assert!(
            (set.seconds_since_2000() + 1..set.seconds_since_2000() + 4)
                .contains(&now.seconds_since_2000()),
            "the clock reads {now:?}, set to {set:?} about two seconds ago"
        );
    }
    assert_eq!(
        i2c.motion().unwrap().register(0x08, at),
        0x03,
        "CTRL7: both sensors on"
    );

    // Asleep, nothing wakes it but the key: pressed half a second later, then released.
    let woken_at = press_after(&mut machine, &buttons, "power", 500 * MILLISECOND);
    until_nth(&mut machine, &mut consoles, "full refresh, busy ", 2);
    let again = shown(&panel, 3, RefreshKind::Full);
    assert!(
        again.pixels == expected(70, false),
        "the second life's full refresh draws the same image, the square of the first gone"
    );
    until_nth(&mut machine, &mut consoles, ASLEEP, 2);
    until_deep_sleep(&mut machine, &mut consoles);
    let console = consoles.text(ConsoleChannel::UsbSerialJtag);
    let all = lines(&consoles);
    let second = &all[life(false, "", "").len()..];
    // The clock kept the time the first life set through the chip's deep sleep: its battery-backed
    // count does not restart with the ESP32-C3. Set at 12:34:56, read 1.5 s later, then the
    // panel's 1.1 s, the release of the key, the half second asleep and the boot: 12:34:59.
    assert_eq!(
        second,
        life(
            true,
            "clock at start 2026-10-15T12:34:59, day 4",
            "clock 1.5 s after it was set 2026-10-15T12:34:57, day 4",
        )
        .as_slice(),
        "{console}"
    );
    shown(&panel, 4, RefreshKind::Partial);

    // The one event of the run is the wake: a core reset of cause deep sleep, at the press.
    match consoles.diagnostics.as_slice() {
        [
            Diagnostic::Reset {
                cause: ResetCause::DeepSleep,
                at_us,
                ..
            },
        ] => assert!(
            (woken_at / 1_000..woken_at / 1_000 + 1_000).contains(at_us),
            "woken at {at_us} us, pressed at {woken_at} ns"
        ),
        other => panic!("{other:?}\n{console}"),
    }
    assert_eq!(panel.take_faults(), [], "{console}");
    assert_eq!(i2c.gauge().unwrap().take_diagnostics(), [], "{console}");
    assert_eq!(i2c.clock().unwrap().take_diagnostics(), [], "{console}");
    assert_eq!(i2c.motion().unwrap().take_diagnostics(), [], "{console}");
    assert_eq!(
        machine.trace_hash(),
        Some(TRACE_HASH),
        "the trace hash of this run is {:#018x}: when the run above is the one intended, pin it in TRACE_HASH",
        machine.trace_hash().unwrap_or_default()
    );
}
