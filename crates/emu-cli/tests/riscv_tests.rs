//! Runs every committed riscv-tests ELF; any failure outside the documented skip list fails CI.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use cpu_rv32::HartConfig;
use emu_cli::rvtest::{RunResult, TestMachine};
use std::path::PathBuf;

const MAX_STEPS: u64 = 5_000_000;
/// rv32ui + um + uc + mi physical-memory (`-p`) variants; the `-v` ones need S-mode.
const MIN_FIXTURES: usize = 60;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
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

fn fixtures() -> Vec<PathBuf> {
    let dir = repo_root().join("tests/fixtures/riscv-tests");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("rv32"))
        })
        .collect();
    v.sort();
    v
}

#[test]
fn all_riscv_tests_pass_except_documented_skips() {
    let skips = skip_list();
    let files = fixtures();
    assert!(
        files.len() >= MIN_FIXTURES,
        "fixtures missing: run tools/conformance/build-riscv-tests.sh"
    );
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
    let path = repo_root().join("tests/fixtures/riscv-tests/rv32ui-p-add");
    let bytes = std::fs::read(path).unwrap();
    let run = || {
        let mut m = TestMachine::from_elf(&bytes, HartConfig::default()).unwrap();
        m.run(MAX_STEPS);
        m.trace_hash()
    };
    assert_eq!(run(), run());
}
