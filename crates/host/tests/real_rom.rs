//! The interface's way of running a device, on the real mask ROM: `power_on` and `open_replay`
//! as the desktop application calls them, the machine on its thread. `INKEMU_ROM_ELF_DIR` must
//! hold the ROM ELF files (`inkemu rom fetch --all --rom-dir DIR`); the CI job `rom-boot` runs
//! these tests.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use host::device::{self, Command, Emulator, Setup, Speed, Update, VerdictView};
use host::engine::Limits;
use host::run::RunEnd;
use soc_esp32c3::ConsoleChannel;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(120);

fn rom_dir() -> PathBuf {
    PathBuf::from(std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR"))
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

/// The updates up to the end of the first life, that one included.
fn first_life(updates: &Receiver<Update>) -> Vec<Update> {
    let mut seen = Vec::new();
    loop {
        let update = updates.recv_timeout(WAIT).expect("the life ends");
        let done = matches!(update, Update::LifeEnded { .. });
        seen.push(update);
        if done {
            return seen;
        }
    }
}

fn console(updates: &[Update], channel: ConsoleChannel) -> String {
    updates
        .iter()
        .filter_map(|update| match update {
            Update::Console { channel: one, text } if *one == channel.name() => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn setup(dir: &std::path::Path, record: Option<PathBuf>) -> Setup {
    Setup {
        device: "x4".into(),
        rom_dir: Some(rom_dir()),
        flash_file: Some(dir.join("flash.bin")),
        record,
        limits: Limits {
            max_cycles: None,
            max_device_nanos: Some(30_000_000),
        },
        ..Setup::default()
    }
}

/// The ROM boots on the emulator's thread, says its banner on the console of the profile, and
/// the first life ends on its limit with the flash file written.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn power_on_boots_the_mask_rom_on_its_thread() {
    let dir = tempfile::tempdir().unwrap();
    let launch = device::power_on(&setup(dir.path(), None)).unwrap();
    assert_eq!(launch.console, ConsoleChannel::UsbSerialJtag);
    let header = launch.notices.last().expect("the first line").text.clone();
    assert!(
        header.starts_with("inkemu: Xteink X4 (ESP32-C3 "),
        "{header}"
    );
    assert!(
        header.ends_with("flash: erased (16 MiB), kept in flash.bin"),
        "{header}"
    );
    let (emulator, updates) = started(launch);
    let life = first_life(&updates);
    assert!(
        console(&life, ConsoleChannel::UsbSerialJtag).starts_with("ESP-ROM:esp32c3"),
        "{life:?}"
    );
    assert!(matches!(
        life.last(),
        Some(Update::LifeEnded {
            life: 1,
            end: RunEnd::TimeLimit,
            ..
        })
    ));
    assert_eq!(
        std::fs::metadata(dir.path().join("flash.bin"))
            .unwrap()
            .len(),
        16 << 20
    );
    emulator.command(Command::Stop).unwrap();
}

/// A life recorded from the interface replays identically through `open_replay`.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_recorded_life_replays_identically_on_the_mask_rom() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("boot.inkrec");
    let (emulator, updates) =
        started(device::power_on(&setup(dir.path(), Some(session.clone()))).unwrap());
    let life = first_life(&updates);
    assert!(
        life.iter().any(|update| matches!(
            update,
            Update::Notice(notice) if notice.text.starts_with("inkemu: session recorded in boot.inkrec")
        )),
        "{life:?}"
    );
    emulator.shutdown();
    let replay = device::open_replay(&session, Some(rom_dir()), None).unwrap();
    let (_replay, updates) = started(replay);
    let life = first_life(&updates);
    assert!(
        matches!(
            life.last(),
            Some(Update::LifeEnded {
                verdict: Some(VerdictView::Identical),
                ..
            })
        ),
        "{life:?}"
    );
}
