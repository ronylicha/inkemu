//! `inkemu inspect`: exit 0 and a description for a valid image, exit 2 and the reason otherwise.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use assert_cmd::Command;
use firmware::builder::{ImageSpec, app_desc_bytes, build_image};
use predicates::prelude::*;
use predicates::str::contains;
use std::path::PathBuf;

fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fw.bin");
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn inspect() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_inkemu"));
    cmd.arg("inspect");
    cmd
}

#[test]
fn inspect_describes_a_valid_image() {
    let (_d, path) = write_temp(&build_image(&ImageSpec::default()));
    inspect()
        .arg(&path)
        .assert()
        .success()
        .stdout(contains("ESP32-C3"))
        .stdout(contains("0x4038086e"))
        .stdout(contains("16 MB"))
        .stdout(contains("demo 1.0.0"))
        .stdout(contains("SHA-256 appended (verified)"))
        .stdout(contains("IRAM"));
}

#[test]
fn inspect_escapes_control_characters_from_firmware_metadata() {
    let mut spec = ImageSpec::default();
    spec.segments[0].1 = app_desc_bytes("1.0\u{1b}]0;pwned\u{7}", "demo\u{1b}[31m", "v5.5.2");
    let (_d, path) = write_temp(&build_image(&spec));
    inspect()
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::function(|out: &[u8]| !out.contains(&0x1b)))
        .stdout(predicate::function(|out: &[u8]| !out.contains(&0x07)))
        .stdout(contains("demo\\u{1b}[31m 1.0\\u{1b}]0;pwned\\u{7}"));
}

#[test]
fn inspect_explains_encrypted_ota() {
    let mut bytes = b"XOTA".to_vec();
    bytes.resize(1024, 0);
    let (_d, path) = write_temp(&bytes);
    inspect()
        .arg(&path)
        .assert()
        .code(2)
        .stderr(contains("encrypted OTA update"));
}

#[test]
fn inspect_refuses_a_full_flash_image() {
    // Bootloader image at 0x0, partition table magic at 0x8000: a flash dump, not an app image.
    let mut bytes = build_image(&ImageSpec::default());
    bytes.resize(0x9000, 0xFF);
    bytes[0x8000..0x8002].copy_from_slice(&[0xAA, 0x50]);
    let (_d, path) = write_temp(&bytes);
    inspect()
        .arg(&path)
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(contains("full flash image"))
        .stderr(contains("; select the application .bin"));
}

#[test]
fn inspect_explains_wrong_format() {
    let (_d, path) = write_temp(&[0x7f, b'E', b'L', b'F'].repeat(16));
    inspect()
        .arg(&path)
        .assert()
        .code(2)
        .stderr(contains("expected 0xe9"));
}

#[test]
fn inspect_reports_missing_file() {
    inspect()
        .arg("/nonexistent/fw.bin")
        .assert()
        .code(2)
        .stderr(contains("cannot read /nonexistent/fw.bin"))
        .stderr(contains("; check the path and read permissions"));
}

/// Real-firmware check, opt-in: set `INKEMU_REAL_FIRMWARE` to a `CrossPoint` X3/X4 .bin (never committed).
#[test]
#[ignore = "needs a third-party firmware file; run with INKEMU_REAL_FIRMWARE=<path> cargo test -- --ignored"]
fn inspect_real_crosspoint_firmware() {
    let path = std::env::var("INKEMU_REAL_FIRMWARE").expect("set INKEMU_REAL_FIRMWARE");
    inspect()
        .arg(path)
        .assert()
        .success()
        .stdout(contains("ESP32-C3"))
        .stdout(contains("DROM"));
}

/// A reader that goes away (`inkemu inspect fw.bin | head -0`) is a normal way to end, not a panic.
#[test]
fn inspect_with_a_closed_stdout_does_not_panic() {
    use std::process::{Command, Stdio};
    let (_d, path) = write_temp(&build_image(&ImageSpec::default()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_inkemu"))
        .arg("inspect")
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // the reader goes away before inkemu writes
    let output = child.wait_with_output().unwrap();
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("panicked"),
        "{output:?}"
    );
    assert!(output.status.success(), "{output:?}");
}
