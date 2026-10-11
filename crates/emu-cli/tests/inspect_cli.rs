//! `inkemu inspect`: exit 0 and a description for a valid image, exit 2 and the reason otherwise.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use assert_cmd::Command;
use firmware::builder::{ImageSpec, build_image};
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
        .stderr(contains("cannot read /nonexistent/fw.bin"));
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
