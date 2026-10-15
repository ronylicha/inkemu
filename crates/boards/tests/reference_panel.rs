//! The wired X4 (sub-project 2d, ADR 0013) with a firmware that uses its board as a firmware does:
//! `tools/reference-firmware/x4_panel_buttons`, booted from the real mask ROM behind the bootloader
//! inkemu ships, powered on by the gesture of the profile. It holds the latch of its supply, reads
//! the cable and the battery, drives the SSD1677 for a full refresh, shows each press of a button
//! by a black square and a differential refresh, and lets go of its latch when its power key is
//! pressed again. The tests read what it prints, what the glass shows and how the run ends.
//!
//! The image is named by `INKEMU_X4_PANEL_APP` and checked against the hash pinned beside its
//! build, `tools/reference-firmware/SHA256SUMS.inputs-x4-panel-buttons`:
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_X4_PANEL_APP=<x4_panel_buttons.bin> cargo test --release -p boards --test reference_panel -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the image and the ROM the environment names

use boards::{ButtonsHandle, MachineOptions, PanelHandle, Wired, WiredOptions};
use chips::ssd1677::{PanelImage, RefreshKind};
use sha2::{Digest, Sha256};
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::reset::ResetCause;
use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, RomImage, StopReason, UsbHost};
use std::fmt::Write as _;
use std::path::Path;

const MILLISECOND: u64 = 1_000_000;
/// How long a test holds a button: five readings of the firmware, which polls every 20 ms.
const HOLD_NS: u64 = 100 * MILLISECOND;
/// The squares of the presses: the n-th key of the profile's order at x = 120 + 96 n, 48 pixels
/// wide, rows 216 to 264 of the glass (`x4_panel_buttons_main.c`, `show_press`).
const SQUARE_TOP: usize = 216;
const SQUARE_SIDE: usize = 48;
/// The trace of the first run below, measured on 2026-10-15 with every other assertion of the test
/// passing (release build, twice, the same).
const TRACE_HASH: u64 = 0xfe61_5ecc_12c4_2f8e;

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

/// `x4_panel_buttons.bin`, checked against the hash
/// `tools/reference-firmware/SHA256SUMS.inputs-x4-panel-buttons` pins: the one place it is written.
fn application() -> Vec<u8> {
    let path = std::env::var("INKEMU_X4_PANEL_APP").expect(
        "set INKEMU_X4_PANEL_APP to the x4_panel_buttons.bin of tools/reference-firmware/build.sh",
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let sums = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/reference-firmware/SHA256SUMS.inputs-x4-panel-buttons");
    let text = std::fs::read_to_string(&sums).unwrap_or_else(|e| panic!("{}: {e}", sums.display()));
    let pinned = text
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name == "x4_panel_buttons.bin").then_some(hash)
        })
        .unwrap_or_else(|| panic!("{}: no line for x4_panel_buttons.bin", sums.display()));
    assert_eq!(
        hex(&Sha256::digest(&bytes)),
        pinned,
        "{path} is not the pinned x4_panel_buttons.bin; rebuild it with tools/reference-firmware/build.sh"
    );
    bytes
}

/// The X4 with the firmware behind the bootloader inkemu ships, as `inkemu run --firmware`
/// composes it, powered on by the gesture of its profile; the USB host is open (the firmware's
/// console is USB-Serial-JTAG).
fn power_on(usb_power: bool) -> (Esp32C3, PanelHandle, ButtonsHandle) {
    let profile = boards::builtin("x4").unwrap();
    let flash = profile.compose_flash(&application(), None).unwrap();
    let options = MachineOptions {
        strict_mmio: true,
        trace_hash: true,
        usb_host: UsbHost::Open,
    };
    let wired = WiredOptions {
        usb_power,
        ..WiredOptions::default()
    };
    let Wired {
        machine,
        panel,
        buttons,
        ..
    } = Wired::power_on(&profile, rom(), Some(flash.bytes), options, wired).unwrap();
    (machine, panel.unwrap(), buttons.unwrap())
}

const BOUNDS: Bounds = Bounds {
    slice: 10_000_000,
    max_cycles: 4_000_000_000,
    max_nanos: 20_000_000_000,
};

/// Drives the machine until its console holds `text`.
fn until(machine: &mut Esp32C3, consoles: &mut Consoles, text: &str) {
    until_nth(machine, consoles, text, 1);
}

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

/// Presses `id` a millisecond from now and releases it [`HOLD_NS`] later; returns when it is let go.
fn press(machine: &mut Esp32C3, buttons: &ButtonsHandle, id: &str) -> u64 {
    press_after(machine, buttons, id, MILLISECOND)
}

/// Presses `id` `delay` nanoseconds from now and releases it [`HOLD_NS`] later; returns when it is
/// let go.
fn press_after(machine: &mut Esp32C3, buttons: &ButtonsHandle, id: &str, delay: u64) -> u64 {
    let pressed = machine.device_time_nanos() + delay;
    let released = pressed + HOLD_NS;
    for (at, down) in [(pressed, true), (released, false)] {
        let input = buttons.button_input(id, down).unwrap();
        machine
            .send_board_input(TIME_BASE.nanos_to_cycles(u128::from(at)), input)
            .unwrap();
    }
    released
}

/// The mean grey of the rectangle [left, left + width) × [top, top + height) of the glass.
fn mean(image: &PanelImage, left: usize, top: usize, width: usize, height: usize) -> u64 {
    let stride = usize::from(image.width);
    let sum: u64 = (top..top + height)
        .flat_map(|row| &image.pixels[row * stride + left..row * stride + left + width])
        .map(|&grey| u64::from(grey))
        .sum();
    sum / (width * height) as u64
}

fn is_black(grey: u64) -> bool {
    grey < 64
}

fn is_white(grey: u64) -> bool {
    grey > 192
}

/// The mean grey of the square of the key at `place` in the profile's order.
fn square(image: &PanelImage, place: usize) -> u64 {
    mean(
        image,
        120 + 96 * place,
        SQUARE_TOP,
        SQUARE_SIDE,
        SQUARE_SIDE,
    )
}

/// The image the glass showed last, checked to be of `generation` and `kind`.
fn shown(panel: &PanelHandle, generation: u64, kind: RefreshKind) -> PanelImage {
    let lines = panel.lines();
    let image = lines.chip().panel_image().clone();
    assert_eq!(
        (image.generation, image.kind, image.interrupted),
        (generation, kind, false),
        "the glass after refresh {generation}"
    );
    image
}

/// The firmware's whole life on battery: the frame and its corner square after the full refresh,
/// one square per press after a differential refresh each, every other part of the glass
/// unchanged, then the power key: the latch let go, the X4 off when the key is released.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_X4_PANEL_APP"]
fn the_x4_shows_its_frame_and_a_square_per_press_then_turns_off_on_its_power_key() {
    let (mut machine, panel, buttons) = power_on(false);
    let mut consoles = Consoles::default();
    until(
        &mut machine,
        &mut consoles,
        "x4_panel_buttons: power key released, armed\n",
    );
    let console = consoles.text(ConsoleChannel::UsbSerialJtag);
    let mut from = 0;
    for expected in [
        "x4_panel_buttons: start, latch held\n",
        "x4_panel_buttons: usb cable unplugged\n",
        "x4_panel_buttons: battery ",
        "x4_panel_buttons: full refresh, busy ",
        "x4_panel_buttons: ready\n",
    ] {
        let at = console[from..]
            .find(expected)
            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{console}"));
        from += at + expected.len();
    }
    // The battery through its divider: the profile's 3 900 mV within the ADC's calibration.
    let battery: u32 = console
        .lines()
        .find_map(|line| line.strip_prefix("x4_panel_buttons: battery "))
        .and_then(|rest| rest.strip_suffix(" mV"))
        .and_then(|mv| mv.parse().ok())
        .unwrap_or_else(|| panic!("no battery line:\n{console}"));
    assert!(battery.abs_diff(3_900) <= 20, "battery {battery} mV");

    let full = shown(&panel, 1, RefreshKind::Full);
    assert_eq!((full.width, full.height), (800, 480));
    // The frame on all four edges, the square in the top-left corner: the top row of the glass is
    // the one the firmware wrote first, at the last RAM row.
    assert!(is_black(mean(&full, 0, 0, 800, 4)), "top edge");
    assert!(is_black(mean(&full, 0, 476, 800, 4)), "bottom edge");
    assert!(is_black(mean(&full, 0, 0, 4, 480)), "left edge");
    assert!(is_black(mean(&full, 796, 0, 4, 480)), "right edge");
    assert!(is_black(mean(&full, 16, 16, 64, 64)), "corner square");
    assert!(is_white(mean(&full, 100, 100, 600, 100)), "the middle");
    for place in 0..6 {
        assert!(is_white(square(&full, place)), "no square {place} yet");
    }

    // Confirm, second of the profile's keys, then Down, the sixth.
    for (refresh, (id, place)) in [("confirm", 1), ("down", 5)].into_iter().enumerate() {
        press(&mut machine, &buttons, id);
        let number = refresh + 1;
        until(
            &mut machine,
            &mut consoles,
            &format!("x4_panel_buttons: partial refresh {number}, busy "),
        );
        assert!(
            consoles
                .text(ConsoleChannel::UsbSerialJtag)
                .contains(&format!("x4_panel_buttons: pressed {id}\n")),
            "{id}"
        );
        let image = shown(&panel, 1 + number as u64, RefreshKind::Partial);
        assert!(is_black(square(&image, place)), "the square of {id}");
        assert!(
            is_black(mean(&image, 16, 16, 64, 64)),
            "the corner square stays"
        );
        assert!(
            is_white(mean(&image, 100, 100, 600, 100)),
            "the middle stays white"
        );
        for other in [0, 2, 3, 4] {
            assert!(is_white(square(&image, other)), "no square {other}");
        }
    }

    // The power key, armed: the latch let go while the key still holds the supply, then off.
    let released = press(&mut machine, &buttons, "power");
    let driven = drive(&mut machine, BOUNDS, &mut consoles, |_, _, _| false);
    let console = consoles.text(ConsoleChannel::UsbSerialJtag);
    assert_eq!(driven, Driven::Stopped(StopReason::PowerOff), "{console}");
    assert!(
        console.contains("x4_panel_buttons: power key pressed, latch released\n"),
        "{console}"
    );
    let lost = machine.power_lost_nanos().unwrap();
    assert!(
        (released..released + MILLISECOND).contains(&lost),
        "off when the key is released at {released} ns: {lost} ns"
    );
    assert_eq!(consoles.diagnostics, [], "{console}");
    assert_eq!(panel.lines_mut().take_faults(), [], "{console}");
    assert_eq!(
        machine.trace_hash(),
        Some(TRACE_HASH),
        "the trace hash of this run is {:#018x}: when the run above is the one intended, pin it in TRACE_HASH",
        machine.trace_hash().unwrap_or_default()
    );
}

/// With the cable in, the firmware reads it, and its power key sends it into deep sleep woken by
/// that key (GPIO3 low, 2d-D11), as `CrossPoint` does on a cable: the X4 stays powered, sleeps until
/// the next press, and wakes into a new life that says why and draws its second corner square.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_X4_PANEL_APP"]
fn with_the_cable_in_the_power_key_sends_the_x4_to_deep_sleep_and_wakes_it() {
    let (mut machine, panel, buttons) = power_on(true);
    let mut consoles = Consoles::default();
    until(
        &mut machine,
        &mut consoles,
        "x4_panel_buttons: power key released, armed\n",
    );
    assert!(
        consoles
            .text(ConsoleChannel::UsbSerialJtag)
            .contains("x4_panel_buttons: usb cable plugged\n")
    );
    let first = shown(&panel, 1, RefreshKind::Full);
    assert!(
        is_white(mean(&first, 720, 400, 64, 64)),
        "no second square yet"
    );

    press(&mut machine, &buttons, "power");
    until(
        &mut machine,
        &mut consoles,
        "x4_panel_buttons: power key released, sleeping\n",
    );
    // Asleep, nothing wakes it but the key: pressed half a second later, well after the firmware's
    // last 20 ms before its sleep, then released.
    let slept_from = machine.device_time_nanos();
    let woken_at = press_after(&mut machine, &buttons, "power", 500 * MILLISECOND) - HOLD_NS;
    until(
        &mut machine,
        &mut consoles,
        "x4_panel_buttons: start, woken by the power key, latch held\n",
    );
    assert!(
        machine.device_time_nanos() >= woken_at,
        "woken before the press: asleep from {slept_from} ns, pressed at {woken_at} ns"
    );
    // The second life arms its key once the press that woke it is let go, after its refresh.
    until_nth(
        &mut machine,
        &mut consoles,
        "x4_panel_buttons: power key released, armed\n",
        2,
    );
    let console = consoles.text(ConsoleChannel::UsbSerialJtag);
    assert!(
        console.contains(
            "x4_panel_buttons: power key pressed, cable in: deep sleep until the power key\n"
        ),
        "{console}"
    );
    let second = shown(&panel, 2, RefreshKind::Full);
    assert!(
        is_black(mean(&second, 16, 16, 64, 64)),
        "the first corner square"
    );
    assert!(
        is_black(mean(&second, 720, 400, 64, 64)),
        "the second corner square"
    );
    assert!(is_white(mean(&second, 100, 100, 600, 100)), "the middle");
    assert_eq!(machine.power_lost_nanos(), None);
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
    assert_eq!(panel.lines_mut().take_faults(), [], "{console}");
}
