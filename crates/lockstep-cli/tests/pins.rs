//! The counts `docs/lockstep-differences.md` gives are the ones the runs pin: the instructions
//! each window compares (the CI's `--expect-compared`, or the local test of `CrossPoint`), and
//! the uses of every row in every window (the usage files `--expect-usage` reads). A usage file
//! that names a row the window does not have is refused before anything runs.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the repository's files, writes one in a temp dir

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(repository().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// A count as the documentation writes it, its thousands spaced: `6 813 949`.
fn count(text: &str) -> u64 {
    text.replace(' ', "")
        .parse()
        .unwrap_or_else(|e| panic!("`{text}`: {e}"))
}

/// The cells of the table rows of `section`.
fn cells(section: &str) -> impl Iterator<Item = Vec<&str>> {
    section
        .lines()
        .filter(|line| line.starts_with('|'))
        .map(|line| line.split('|').map(str::trim).collect())
}

/// The file pinning the uses of `window`, and the instructions it compares.
fn pins(window: &str) -> (String, u64) {
    if window == "crosspoint" {
        let test = read("crates/lockstep-cli/tests/crosspoint.rs");
        let compared = test
            .lines()
            .find_map(|line| line.strip_prefix("const COMPARED: &str = \""))
            .and_then(|rest| rest.strip_suffix("\";"))
            .expect("COMPARED in crosspoint.rs");
        (
            read("crates/lockstep-cli/tests/crosspoint.usage"),
            count(compared),
        )
    } else {
        let ci = read(".github/workflows/ci.yml").replace("\\\n", " ");
        let compared = ci
            .lines()
            .filter(|line| line.contains(&format!("--window {window} ")))
            .find_map(|line| {
                let (_, rest) = line.split_once("--expect-compared ")?;
                rest.split_whitespace().next()
            })
            .unwrap_or_else(|| panic!("no step of the CI pins the window {window}"));
        (
            read(&format!("tools/ci/lockstep-{window}.usage")),
            count(compared),
        )
    }
}

fn uses(file: &str) -> BTreeMap<u16, u64> {
    file.lines()
        .filter_map(|line| line.strip_prefix("row "))
        .map(|rest| {
            let (row, uses) = rest.split_once(':').unwrap();
            let uses = uses.trim().strip_suffix(" uses").unwrap();
            (row.trim().parse().unwrap(), uses.parse().unwrap())
        })
        .collect()
}

#[test]
fn the_counts_of_the_documentation_are_those_the_runs_pin() {
    let doc = read("docs/lockstep-differences.md");
    let windows = doc
        .split("## Les fenêtres")
        .nth(1)
        .unwrap()
        .split("\n## ")
        .next()
        .unwrap();
    let compared: BTreeMap<String, u64> = cells(windows)
        .filter(|cells| cells.get(2).is_some_and(|cell| cell.starts_with('`')))
        .map(|cells| (cells[1].to_owned(), count(cells[6])))
        .collect();
    // The uses the summary table gives, by window and row: `hello 4 789 611 ; nvs 5 108 772`.
    let table = doc.split("\n## Tableau").nth(1).unwrap();
    let mut documented: BTreeMap<String, BTreeMap<u16, u64>> = BTreeMap::new();
    for cells in cells(table) {
        let Some(Ok(row)) = cells.get(1).map(|cell| cell.parse::<u16>()) else {
            continue;
        };
        for entry in cells[5].split(';').map(str::trim).filter(|e| !e.is_empty()) {
            let Some((window, n)) = entry.split_once(' ') else {
                continue;
            };
            documented
                .entry(window.to_owned())
                .or_default()
                .insert(row, count(n));
        }
    }
    for window in lockstep::WINDOWS {
        let (file, pinned) = pins(window.name);
        assert_eq!(
            compared.get(window.name),
            Some(&pinned),
            "instructions of the window {}",
            window.name
        );
        assert_eq!(
            documented.get(window.name),
            Some(&uses(&file)),
            "uses of the rows in the window {}",
            window.name
        );
    }
}

#[test]
fn a_usage_file_that_names_a_row_outside_the_window_is_refused_before_the_run() {
    let path =
        std::env::temp_dir().join(format!("inkemu-lockstep-pins-{}.usage", std::process::id()));
    std::fs::write(&path, "row 2: 1 uses\nrow 34: 21 uses\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_inkemu-lockstep"))
        .args([
            "--window",
            "hello",
            "--trace",
            "/dev/null",
            "--flash",
            "/nonexistent",
        ])
        .args(["--rom-dir", "/nonexistent"])
        .arg("--expect-usage")
        .arg(&path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("names row 34, which is not a row of the window hello"),
        "{stderr}"
    );
}
