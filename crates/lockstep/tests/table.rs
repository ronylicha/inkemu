//! `docs/lockstep-differences.md` and `lockstep::DIFFERENCES` are one table (decision H3): every
//! row of the documentation is a row of the tool with the same treatments, and the tool has no
//! other row.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the documentation: it is a test of the repository

use std::collections::BTreeSet;

/// The rows of the documentation: number, and the treatments the third column names.
fn documented() -> Vec<(u16, BTreeSet<String>)> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/lockstep-differences.md"
    );
    let text = std::fs::read_to_string(path).expect("docs/lockstep-differences.md");
    let mut rows = Vec::new();
    for line in text.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // `| N° | Différence | Traitement | ...`: a row has a number in its first cell.
        let Some(Ok(number)) = cells.get(1).map(|cell| cell.parse::<u16>()) else {
            continue;
        };
        let treatment = cells[3];
        let kinds = ["(i)", "(ii)", "(iii)", "(iv)"]
            .into_iter()
            .filter(|kind| treatment.contains(kind))
            .map(|kind| kind.trim_matches(|c| c == '(' || c == ')').to_owned())
            .collect();
        rows.push((number, kinds));
    }
    rows
}

#[test]
fn the_table_of_the_documentation_is_the_table_of_the_tool() {
    let documented = documented();
    let tool: Vec<(u16, BTreeSet<String>)> = lockstep::DIFFERENCES
        .iter()
        .map(|row| {
            (
                row.number,
                row.treatments
                    .iter()
                    .map(|treatment| treatment.kind().to_owned())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(documented, tool);
}

#[test]
fn the_rows_are_numbered_from_1_without_a_gap() {
    let numbers: Vec<u16> = lockstep::DIFFERENCES.iter().map(|row| row.number).collect();
    let expected: Vec<u16> = (1..=u16::try_from(numbers.len()).unwrap()).collect();
    assert_eq!(numbers, expected);
}
