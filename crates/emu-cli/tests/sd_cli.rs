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

fn sd_recover() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_inkemu"));
    cmd.args(["sd", "recover"]);
    cmd
}

/// A journal of version 2 for a card of `blocks` blocks that writes `lba` with `byte`, bound to
/// no file: what a copy of an image carries.
fn foreign_journal(blocks: u64, lba: u64, byte: u8) -> Vec<u8> {
    use sha2::{Digest as _, Sha256};
    let mut bytes = b"inkemu-sd-journal 2\n".to_vec();
    bytes.extend_from_slice(&[0; 16]);
    bytes.extend_from_slice(&blocks.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&lba.to_le_bytes());
    bytes.extend_from_slice(&[byte; 512]);
    let digest = Sha256::digest(&bytes);
    bytes.extend_from_slice(&digest);
    bytes
}

#[test]
fn recover_applies_a_journal_of_another_file_only_when_told() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.img");
    sd_create()
        .arg(&path)
        .args(["--size", "64"])
        .assert()
        .success();
    let journal = dir.path().join(".card.img.journal");
    std::fs::write(&journal, foreign_journal(64 * 2048, 5, 0x42)).unwrap();
    let block_5 = || std::fs::read(&path).unwrap()[5 * 512..6 * 512].to_vec();
    sd_recover()
        .arg(&path)
        .assert()
        .code(2)
        .stderr(contains("written for another image file"))
        .stderr(contains("--accept-journal"))
        .stderr(contains("may hold part of"));
    assert_eq!(block_5(), [0; 512]);
    assert!(journal.exists());
    sd_recover()
        .arg(&path)
        .arg("--accept-journal")
        .assert()
        .success()
        .stdout(contains(
            "the unfinished save is applied (1 blocks, 0 erased ranges)",
        ));
    assert_eq!(block_5(), [0x42; 512]);
    assert!(!journal.exists());
    sd_recover()
        .arg(&path)
        .assert()
        .success()
        .stdout(contains("no unfinished save, nothing to do"));
}
