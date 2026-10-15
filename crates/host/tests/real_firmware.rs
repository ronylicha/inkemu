//! Two firmwares of real Xteink X4 units on the desktop application's path (design of
//! sub-project 2d, 2d-D22; design of sub-project 2e, §6): `device::power_on` as the interface
//! calls it, the wired X4 powered on by its gesture, the machine on its thread, the panel read
//! through its port and captured as a user sees it.
//!
//! `CrossPoint` 1.6.5 (`crosspoint-1.6.5-x3-x4.bin`) and the stock firmware 3.0.2 EN (a dump of
//! the 16 MiB flash of a unit) are files of the user: these tests run on the machine of a user
//! who has them, never in the CI. Each reads the path of its file from a variable and checks its
//! SHA-256 before anything runs:
//!
//! ```text
//! export INKEMU_ROM_ELF_DIR=/path/to/rom
//! export INKEMU_CROSSPOINT_X3_X4=/path/to/crosspoint-1.6.5-x3-x4.bin
//! export INKEMU_STOCK_X4_FLASH=/path/to/x4-stock-3.0.2-en-flash.bin
//! cargo test --release -p host --test real_firmware -- --ignored --nocapture
//! ```
//!
//! The captures go to `target/real-firmware/` for the eye. A dump of a unit holds its NVS: no line
//! here reads the flash but to hash it, and nothing quotes it.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the files the environment names

use host::device::{self, Command, Emulator, SdSetup, Setup, Speed, Update};
use host::engine::Limits;
use host::run::RunEnd;
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// `crosspoint-1.6.5-x3-x4.bin` (measured 2026-10-14).
const CROSSPOINT_X3_X4_SHA256: &str =
    "8eccf614b6d6f82284d3ec775a383fd421ec1880481ff863cf44a831e0385764";
/// The dump of the 16 MiB flash of an X4 running its stock firmware 3.0.2 EN.
const STOCK_X4_FLASH_SHA256: &str =
    "03866a1fc23a6a624469f508da3a747159b2315303b98779fd8161276a989a88";

/// How long the host waits for one update before the test fails instead of hanging.
const WAIT: Duration = Duration::from_mins(20);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// The path `variable` holds, accepted only when its file has the SHA-256 `sha256`.
fn pinned_file(variable: &str, sha256: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var(variable).unwrap_or_else(|_| panic!("set {variable}")));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(
        hex(&Sha256::digest(&bytes)),
        sha256,
        "{} is not the file this test pins",
        path.display()
    );
    path
}

fn rom_dir() -> PathBuf {
    PathBuf::from(std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR"))
}

/// Where the captures go: `target/real-firmware/`.
fn capture_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/real-firmware");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn started(launch: device::Launch) -> (Emulator, Receiver<Update>) {
    let (sender, updates) = mpsc::channel();
    let emulator = Emulator::start(
        launch,
        Speed::Fastest,
        Box::new(move |update| {
            let _ = sender.send(update);
        }),
    )
    .unwrap();
    (emulator, updates)
}

/// What a life showed: its console text, its refreshes, its end.
#[derive(Debug, Default)]
struct Life {
    console: String,
    refreshes: Vec<(u64, host::ports::RefreshKind)>,
    notices: Vec<String>,
    end: Option<RunEnd>,
}

/// The updates of the first life, to its end.
fn first_life(updates: &Receiver<Update>) -> Life {
    let mut life = Life::default();
    loop {
        match updates.recv_timeout(WAIT).expect("the life ends") {
            Update::Console { text, .. } => life.console.push_str(&text),
            Update::Frame {
                generation,
                refresh,
                ..
            } => life.refreshes.push((generation, refresh)),
            Update::Notice(notice) => life.notices.push(notice.text),
            Update::LifeEnded { end, .. } => {
                life.end = Some(end);
                return life;
            }
            Update::Status(_) | Update::Stopped => {}
        }
    }
}

/// The smallest rectangle holding the dark pixels of an image `width` pixels wide:
/// `(left, top, right, bottom)`, exclusive; `None` when none is dark.
fn dark_box(pixels: &[u8], width: usize) -> Option<(usize, usize, usize, usize)> {
    pixels
        .iter()
        .enumerate()
        .filter(|&(_, &p)| p < 128)
        .fold(None, |found, (at, _)| {
            let (column, row) = (at % width, at / width);
            Some(match found {
                None => (column, row, column + 1, row + 1),
                Some((left, top, right, bottom)) => (
                    left.min(column),
                    top.min(row),
                    right.max(column + 1),
                    bottom.max(row + 1),
                ),
            })
        })
}

/// `CrossPoint` 1.6.5 powered on by the gesture with a blank card of 1 GiB: it mounts the card and
/// draws its screens; the last one is captured for the eye.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_CROSSPOINT_X3_X4 (a file of the user's)"]
fn crosspoint_1_6_5_draws_its_screens_with_a_blank_card() {
    let firmware = pinned_file("INKEMU_CROSSPOINT_X3_X4", CROSSPOINT_X3_X4_SHA256);
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card.img");
    let setup = Setup {
        device: "x4".into(),
        firmware: Some(firmware),
        rom_dir: Some(rom_dir()),
        sd: Some(SdSetup {
            path: card.clone(),
            read_only: false,
            create_mib: Some(1024),
        }),
        limits: Limits {
            max_cycles: None,
            max_device_nanos: Some(8_000_000_000),
        },
        ..Setup::default()
    };
    let (emulator, updates) = started(device::power_on(&setup).unwrap());
    let life = first_life(&updates);
    let png = capture_dir().join("crosspoint-1.6.5-blank-card.png");
    emulator
        .command(Command::Capture { path: png.clone() })
        .unwrap();
    emulator.shutdown().unwrap();
    eprintln!(
        "CrossPoint with a blank card: {} refreshes {:?}, end {:?}, capture {}",
        life.refreshes.len(),
        life.refreshes,
        life.end,
        png.display()
    );
    eprintln!("console tail: {}", tail(&life.console, 1500));
    assert!(
        life.console.contains("[SD] SD card detected"),
        "{}",
        life.console
    );
    assert!(
        life.refreshes.len() >= 2,
        "the boot screen then another: {:?}",
        life.refreshes
    );
    assert_eq!(life.end, Some(RunEnd::TimeLimit));
}

/// `CrossPoint` 1.6.5 powered on by the gesture without a card: it says the card is missing and
/// draws "SD card error", a line of text that the rotation of the profile turns upright in the
/// portrait: wider than tall, near the middle (2d-D22; the design expected the upper half, the
/// firmware centres it).
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_CROSSPOINT_X3_X4 (a file of the user's)"]
fn crosspoint_1_6_5_without_a_card_draws_its_error_screen() {
    let firmware = pinned_file("INKEMU_CROSSPOINT_X3_X4", CROSSPOINT_X3_X4_SHA256);
    let setup = Setup {
        device: "x4".into(),
        firmware: Some(firmware),
        rom_dir: Some(rom_dir()),
        limits: Limits {
            max_cycles: None,
            max_device_nanos: Some(6_000_000_000),
        },
        ..Setup::default()
    };
    let (emulator, updates) = started(device::power_on(&setup).unwrap());
    let life = first_life(&updates);
    let png = capture_dir().join("crosspoint-1.6.5-no-card.png");
    emulator
        .command(Command::Capture { path: png.clone() })
        .unwrap();
    let frame = emulator.frame().expect("an image");
    emulator.shutdown().unwrap();
    eprintln!(
        "CrossPoint without a card: {} refreshes {:?}, end {:?}, capture {}",
        life.refreshes.len(),
        life.refreshes,
        life.end,
        png.display()
    );
    eprintln!("console tail: {}", tail(&life.console, 1500));
    assert!(
        life.console.contains("[SD] SD card not detected"),
        "{}",
        life.console
    );
    let (width, height, pixels) = host::capture::rotated(&frame, host::ports::Rotation::Deg90);
    assert_eq!((width, height), (480, 800), "portrait");
    let (left, top, right, bottom) = dark_box(&pixels, width as usize).expect("a text");
    eprintln!("the dark pixels lie in ({left}, {top}) to ({right}, {bottom})");
    assert!(right - left > 2 * (bottom - top), "a line of text, upright");
    let middle = top.midpoint(bottom);
    assert!((300..500).contains(&middle), "near the middle: {middle}");
}

/// The stock firmware 3.0.2 powered on by the gesture of the profile (1.5 s): it holds its latch
/// and stays on; what it draws is captured, and said for the hand-over.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_STOCK_X4_FLASH (a file of the user's)"]
fn the_stock_firmware_powered_on_by_the_gesture() {
    let flash = pinned_file("INKEMU_STOCK_X4_FLASH", STOCK_X4_FLASH_SHA256);
    let setup = Setup {
        device: "x4".into(),
        image: Some(flash),
        rom_dir: Some(rom_dir()),
        limits: Limits {
            max_cycles: None,
            max_device_nanos: Some(8_000_000_000),
        },
        ..Setup::default()
    };
    let (emulator, updates) = started(device::power_on(&setup).unwrap());
    let life = first_life(&updates);
    let png = capture_dir().join("stock-3.0.2.png");
    let captured = emulator.command(Command::Capture { path: png.clone() });
    emulator.shutdown().unwrap();
    eprintln!(
        "stock 3.0.2: {} refreshes {:?}, end {:?}, capture {:?}",
        life.refreshes.len(),
        life.refreshes,
        life.end,
        captured.map(|()| png.display().to_string())
    );
    eprintln!("notices: {:?}", life.notices);
    eprintln!("console tail: {}", tail(&life.console, 1500));
    assert_eq!(life.end, Some(RunEnd::TimeLimit), "{:?}", life.notices);
}

/// `CrossPoint` 1.6.5 on the X3 (ADR 0015). Its first boot finds the three chips of the I2C bus,
/// takes the device for an X3 and keeps the verdict in NVS, then stops for good on a lock of
/// Arduino's `Wire` that its own failed read of the gauge left taken (research
/// `sp3-x3-firmware.md` §7): a firmware's fault, which a unit shows too. The second boot reads the
/// verdict back, never takes that path, mounts the card and draws its home screen, which the
/// rotation of the profile turns upright in the portrait.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_CROSSPOINT_X3_X4 (a file of the user's)"]
fn crosspoint_1_6_5_on_the_x3_reaches_its_home_screen_at_its_second_boot() {
    let firmware = pinned_file("INKEMU_CROSSPOINT_X3_X4", CROSSPOINT_X3_X4_SHA256);
    let dir = tempfile::tempdir().unwrap();
    let flash = dir.path().join("flash.bin");
    let card = |create_mib| SdSetup {
        path: dir.path().join("card.img"),
        read_only: false,
        create_mib,
    };
    // The first life makes the blank card and composes the flash; the second takes both again.
    let life_of = |firmware: Option<PathBuf>, seconds: u64| {
        let first = firmware.is_some();
        let setup = Setup {
            device: "x3".into(),
            firmware,
            flash_file: Some(flash.clone()),
            rom_dir: Some(rom_dir()),
            sd: Some(card(first.then_some(1024))),
            limits: Limits {
                max_cycles: None,
                max_device_nanos: Some(seconds * 1_000_000_000),
            },
            ..Setup::default()
        };
        let (emulator, updates) = started(device::power_on(&setup).unwrap());
        (emulator, first_life(&updates))
    };
    let (emulator, first) = life_of(Some(firmware), 2);
    emulator.shutdown().unwrap();
    eprintln!("first boot: {}", tail(&first.console, 800));
    assert!(
        first
            .console
            .contains("Xteink probe scores: pass1=3 pass2=3"),
        "the three chips answer: {}",
        first.console
    );
    let (emulator, second) = life_of(None, 10);
    let png = capture_dir().join("crosspoint-1.6.5-x3.png");
    emulator
        .command(Command::Capture { path: png.clone() })
        .unwrap();
    let frame = emulator.frame().expect("an image");
    emulator.shutdown().unwrap();
    eprintln!(
        "CrossPoint on the X3: {} refreshes {:?}, end {:?}, capture {}",
        second.refreshes.len(),
        second.refreshes,
        second.end,
        png.display()
    );
    eprintln!("console tail: {}", tail(&second.console, 1500));
    for line in [
        "Using cached device type: X3",
        "SDK IMU initialized",
        "SDK RTC found",
        "[SD] SD card detected",
    ] {
        assert!(second.console.contains(line), "{line}: {}", second.console);
    }
    assert!(
        second.refreshes.len() >= 2,
        "the boot screen then another: {:?}",
        second.refreshes
    );
    let (width, height, pixels) = host::capture::rotated(&frame, host::ports::Rotation::Deg90);
    assert_eq!((width, height), (528, 792), "portrait");
    let (left, top, right, bottom) = dark_box(&pixels, width as usize).expect("a screen");
    eprintln!("the dark pixels lie in ({left}, {top}) to ({right}, {bottom})");
    assert_eq!(second.end, Some(RunEnd::TimeLimit));
}

/// The last `chars` characters of `text`.
fn tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(chars)).collect()
}
