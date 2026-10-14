//! `CrossPoint` 1.6.5 in the lockstep against QEMU (SP1b-4d, decision J6): criterion 3 of the spec
//! (`docs/superpowers/specs/2026-10-11-inkemu-design.md`, §10.3) on a real firmware of the X4,
//! from the first instruction of the ROM to the first access to a register QEMU does not have that
//! no row of the window `crosspoint` names.
//!
//! `crosspoint-1.6.5-x3-x4.bin` is not in this repository, is never downloaded, and this test runs
//! nowhere in the CI: it reads the path of a file of the user's, refuses any file but the pinned
//! one, and needs Docker with the image `espressif/idf:v5.5.2` already pulled:
//!
//! ```text
//! export INKEMU_ROM_ELF_DIR=/path/to/rom          # inkemu rom fetch --all --rom-dir /path/to/rom
//! export INKEMU_CROSSPOINT_X3_X4=/path/to/crosspoint-1.6.5-x3-x4.bin
//! cargo test --release -p inkemu-lockstep --test crosspoint -- --ignored
//! ```
//!
//! The flash both sides boot is composed from the application and erased bytes, as
//! `inkemu run --firmware` composes it: no byte of a device's flash is involved, and nothing prints
//! one. About twenty minutes on a development host, QEMU's speed being the limit.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the user's file to hash it: a test of the repository

use std::fmt::Write as _;
use std::process::Command;

/// `crosspoint-1.6.5-x3-x4.bin`, the release asset of `CrossPoint` 1.6.5 for the X3 and the X4
/// (the same pin as `crates/emu-cli/tests/real_firmware.rs`).
const CROSSPOINT_X3_X4_SHA256: &str =
    "8eccf614b6d6f82284d3ec775a383fd421ec1880481ff863cf44a831e0385764";

/// The window, as measured on 2026-10-14 (QEMU `esp_develop_9.2.2_20250817`): the instructions
/// compared, and the access that ends it, `APB_SARADC.APB_TSENS_CTRL` read by the application.
const COMPARED: &str = "31088420";
const END: &str =
    "window end: access to 0x60040058, a register QEMU does not have, at pc 0x4214917c";

fn variable(name: &str, example: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; export {name}={example}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR, INKEMU_CROSSPOINT_X3_X4 (a file of the user's) and Docker with espressif/idf:v5.5.2"]
fn crosspoint_compares_with_qemu_to_the_first_register_qemu_lacks() {
    let file = variable(
        "INKEMU_CROSSPOINT_X3_X4",
        "/path/to/crosspoint-1.6.5-x3-x4.bin",
    );
    let read = sha256_hex(&std::fs::read(&file).unwrap_or_else(|e| panic!("{file}: {e}")));
    assert!(
        read == CROSSPOINT_X3_X4_SHA256,
        "INKEMU_CROSSPOINT_X3_X4={file} is not the pinned file: its SHA-256 is {read}, \
         {CROSSPOINT_X3_X4_SHA256} expected; nothing was run"
    );
    let rom = variable("INKEMU_ROM_ELF_DIR", "/path/to/rom");
    let usage = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/crosspoint.usage");
    let output = Command::new(env!("CARGO_BIN_EXE_inkemu-lockstep"))
        .args([
            "--window",
            "crosspoint",
            "--firmware",
            &file,
            "--rom-dir",
            &rom,
        ])
        .args(["--expect-compared", COMPARED, "--expect-end", "absent"])
        .args(["--expect-usage", usage])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "inkemu-lockstep ended otherwise than the measured window\n{stdout}\n{stderr}"
    );
    assert!(stdout.contains(END), "{stdout}");
}
