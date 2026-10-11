//! Exit-code contract of `inkemu rvtest`: 0 passed, 1 failed, 2 timeout or error.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/riscv-tests")
        .join(name)
}

fn rvtest() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_inkemu"));
    cmd.arg("rvtest");
    cmd
}

#[test]
fn passing_test_exits_zero() {
    rvtest()
        .arg(fixture("rv32ui-p-add"))
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Passed"));
}

#[test]
fn failing_test_exits_one_and_names_the_sub_test() {
    // rv32mi-p-pmpaddr needs the PMP CSRs, which this core does not implement.
    rvtest()
        .arg(fixture("rv32mi-p-pmpaddr"))
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAILED at test #1"));
}

#[test]
fn step_budget_exhaustion_exits_two() {
    rvtest()
        .arg(fixture("rv32ui-p-add"))
        .args(["--max-steps", "3"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no verdict after 3 steps"));
}

#[test]
fn non_elf_input_exits_two_with_a_message() {
    rvtest()
        .arg(fixture("SHA256SUMS"))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not a valid ELF file"));
}

#[test]
fn allow_misaligned_flag_changes_the_misaligned_policy() {
    rvtest().arg(fixture("rv32ui-p-ma_data")).assert().code(1);
    rvtest()
        .arg("--allow-misaligned")
        .arg(fixture("rv32ui-p-ma_data"))
        .assert()
        .code(0);
}

#[test]
fn trace_hash_flag_prints_a_stable_hash() {
    let run = || {
        let out = rvtest()
            .arg(fixture("rv32ui-p-add"))
            .arg("--trace-hash")
            .assert()
            .code(0)
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };
    let first = run();
    assert!(first.contains("trace-hash "), "{first}");
    assert_eq!(first, run());
}

#[test]
fn signature_flag_writes_one_hex_word_per_line() {
    let dir = std::env::temp_dir().join(format!("inkemu-sig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sig = dir.join("add.sig");
    rvtest()
        .arg(fixture("rv32ui-p-add"))
        .arg("--signature")
        .arg(&sig)
        .assert()
        .code(0);
    let text = std::fs::read_to_string(&sig).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        text.lines()
            .all(|l| l.len() == 8 && l.chars().all(|c| c.is_ascii_hexdigit())),
        "{text}"
    );
}
