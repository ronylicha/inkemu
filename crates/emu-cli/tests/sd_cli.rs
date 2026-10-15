//! `inkemu sd create`: a blank SD image, its line on standard output, and its refusals (plan
//! `SP2e`, task 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]
use assert_cmd::Command;
use predicates::str::contains;

fn sd_create() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_inkemu"));
    cmd.args(["sd", "create"]);
    cmd
}

#[test]
fn creates_a_blank_image_and_says_what_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.img");
    sd_create()
        .arg(&path)
        .args(["--size", "64", "--label", "books"])
        .assert()
        .success()
        .stdout(contains(
            "64 MiB SDSC card, FAT32, 114688 clusters of 512 bytes, label BOOKS",
        ));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 64 * 1024 * 1024);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes[510..512], [0x55, 0xAA]);
}

#[test]
fn refuses_an_existing_path_a_size_too_small_and_a_bad_label() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.img");
    std::fs::write(&path, b"keep").unwrap();
    sd_create()
        .arg(&path)
        .args(["--size", "64"])
        .assert()
        .code(2)
        .stderr(contains("already exists"));
    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    sd_create()
        .arg(dir.path().join("small.img"))
        .args(["--size", "63"])
        .assert()
        .code(2)
        .stderr(contains("too small"));
    sd_create()
        .arg(dir.path().join("label.img"))
        .args(["--size", "64", "--label", "a/b"])
        .assert()
        .code(2)
        .stderr(contains("cannot be used"));
    sd_create()
        .arg(dir.path().join("missing").join("x.img"))
        .args(["--size", "64"])
        .assert()
        .code(2)
        .stderr(contains("cannot create the SD image"));
    assert!(!dir.path().join("small.img").exists());
}
