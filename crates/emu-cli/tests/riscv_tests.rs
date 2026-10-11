//! Runs every committed riscv-tests ELF; any failure outside the documented skip list fails CI.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use cpu_rv32::HartConfig;
use emu_cli::rvtest::{RunResult, TestMachine};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const MAX_STEPS: u64 = 5_000_000;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_dir() -> PathBuf {
    repo_root().join("tests/fixtures/riscv-tests")
}

fn skip_list() -> Vec<String> {
    let text = std::fs::read_to_string(repo_root().join("tools/conformance/riscv-tests-skip.txt"))
        .unwrap();
    text.lines()
        .filter_map(|l| l.split('#').next())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// `(file name, sha256 hex)` pairs of `SHA256SUMS`, the manifest written by `build-riscv-tests.sh`.
fn manifest() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(fixture_dir().join("SHA256SUMS")).unwrap();
    text.lines()
        .map(|line| {
            let (hash, name) = line.split_once("  ").expect("`<sha256>  <name>` line");
            (name.to_string(), hash.to_string())
        })
        .collect()
}

fn fixtures() -> Vec<PathBuf> {
    manifest()
        .into_iter()
        .map(|(name, _)| fixture_dir().join(name))
        .collect()
}

#[test]
fn all_riscv_tests_pass_except_documented_skips() {
    let skips = skip_list();
    let files = fixtures();
    let mut failures = Vec::new();
    let mut ran = 0_usize;
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if skips.contains(&name) {
            continue;
        }
        let mut m =
            TestMachine::from_elf(&std::fs::read(path).unwrap(), HartConfig::default()).unwrap();
        ran += 1;
        match m.run(MAX_STEPS) {
            RunResult::Passed => {}
            other => failures.push(format!("{name}: {other:?}")),
        }
    }
    println!("riscv-tests: {ran} ran, {} skipped", skips.len());
    assert!(
        failures.is_empty(),
        "{} failing:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn fixture_directory_matches_the_sha256_manifest() {
    let listed = manifest();
    let mut on_disk: Vec<String> = std::fs::read_dir(fixture_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("rv32"))
        .collect();
    on_disk.sort();
    let mut names: Vec<String> = listed.iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    assert_eq!(
        on_disk, names,
        "rv32* files differ from SHA256SUMS: rerun tools/conformance/build-riscv-tests.sh"
    );
    assert!(!names.is_empty(), "empty manifest");
    for (name, expected) in listed {
        let digest = Sha256::digest(std::fs::read(fixture_dir().join(&name)).unwrap());
        assert_eq!(
            emu_cli::term::hex(&digest),
            expected,
            "{name} does not match its manifest hash"
        );
    }
}

#[test]
fn skip_list_entries_exist_and_have_reasons() {
    let text = std::fs::read_to_string(repo_root().join("tools/conformance/riscv-tests-skip.txt"))
        .unwrap();
    let names: Vec<String> = fixtures()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (name, reason) = line.split_once('#').unwrap_or((line, ""));
        assert!(
            names.contains(&name.trim().to_string()),
            "unknown test in skip list: {name}"
        );
        assert!(
            reason.trim().len() > 10,
            "skip entry needs a reason citing the spec: {line}"
        );
    }
}

#[test]
fn same_test_twice_gives_same_trace_hash() {
    let bytes = std::fs::read(fixture_dir().join("rv32ui-p-add")).unwrap();
    let run = || {
        let mut m = TestMachine::from_elf(&bytes, HartConfig::default()).unwrap();
        assert_eq!(m.run(MAX_STEPS), RunResult::Passed);
        m.trace_hash()
    };
    assert_eq!(run(), run());
}

/// `rv32ui-p-lw` keeps four data words between `begin_signature` and `end_signature`
/// (`objdump -s -j .data`: `ff00ff00 00ff00ff f00ff00f 0ff00ff0` as bytes, i.e. these little-endian words).
const LW_SIGNATURE: [u32; 4] = [0x00ff_00ff, 0xff00_ff00, 0x0ff0_0ff0, 0xf00f_f00f];

#[test]
fn signature_returns_the_words_of_the_signature_region() {
    let bytes = std::fs::read(fixture_dir().join("rv32ui-p-lw")).unwrap();
    let mut m = TestMachine::from_elf(&bytes, HartConfig::default()).unwrap();
    assert_eq!(m.run(MAX_STEPS), RunResult::Passed);
    assert_eq!(m.signature().unwrap(), LW_SIGNATURE);
}

#[test]
fn signature_is_empty_when_the_region_is_empty() {
    // rv32ui-p-add has begin_signature == end_signature.
    let bytes = std::fs::read(fixture_dir().join("rv32ui-p-add")).unwrap();
    let mut m = TestMachine::from_elf(&bytes, HartConfig::default()).unwrap();
    assert_eq!(m.run(MAX_STEPS), RunResult::Passed);
    assert_eq!(m.signature().unwrap(), Vec::<u32>::new());
}
