//! `docs/lockstep-differences.md` and `lockstep::DIFFERENCES` are one table (decision H3): every
//! row of the documentation is a row of the tool with the same treatments and the same
//! parameters, register by register and bit by bit, and the tool has no other row.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the documentation: it is a test of the repository

use lockstep::state::CSRS;
use lockstep::table::Compat;
use lockstep::{Drift, Location, Treatment};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

fn documentation() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/lockstep-differences.md"
    );
    std::fs::read_to_string(path).expect("docs/lockstep-differences.md")
}

/// The rows of the summary table: number, and the treatments its third column names.
fn summary(text: &str) -> Vec<(u16, BTreeSet<String>)> {
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

/// The parameters each row declares: the lines of the `text` block under `### Ligne N`.
fn parameters(text: &str) -> BTreeMap<u16, Vec<String>> {
    let mut rows = BTreeMap::new();
    let mut current: Option<(u16, Vec<String>, bool)> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("### Ligne ") {
            let number = rest
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("a row heading without a number: {line}"));
            current = Some((number, Vec::new(), false));
            continue;
        }
        let Some((number, lines, open)) = &mut current else {
            continue;
        };
        if line.trim() == "```text" && !*open {
            *open = true;
        } else if line.trim() == "```" && *open {
            rows.insert(*number, std::mem::take(lines));
            current = None;
        } else if *open {
            lines.push(line.trim().to_owned());
        }
    }
    rows
}

fn csr_name(csr: u16) -> &'static str {
    CSRS.iter()
        .find(|(_, address, _)| *address == csr)
        .map(|(_, _, name)| *name)
        .unwrap()
}

/// The lines the documentation gives for a treatment of the tool.
fn lines_of(treatment: &Treatment) -> Vec<String> {
    match *treatment {
        Treatment::Pair {
            field,
            bits,
            qemu,
            inkemu,
            until_written,
        } => vec![format!(
            "paire {} bits 0x{bits:08X} qemu 0x{qemu:08X} inkemu 0x{inkemu:08X}{}",
            csr_name(field.csr().unwrap()),
            if until_written {
                " jusqu'à écriture"
            } else {
                ""
            }
        )],
        Treatment::ExternalInterrupt { lag } => vec![format!("meip retard {lag}")],
        Treatment::Registers(registers) => registers
            .iter()
            .map(|register| {
                let mut line = format!(
                    "registre 0x{:08X} masque 0x{:08X}",
                    register.address, register.mask
                );
                if let Some(command) = register.after_command {
                    let _ = write!(line, " après 0x{command:02X}");
                }
                if let Some(value) = register.qemu {
                    let _ = write!(line, " qemu 0x{value:08X}");
                }
                line
            })
            .collect(),
        Treatment::Counter {
            location,
            drift,
            origins,
        } => {
            let mut line = format!("compteur {}", location_text(location));
            match drift {
                Drift::Within(bound) => {
                    let _ = write!(line, " écart {bound}");
                }
                Drift::Truncated { first, last } => {
                    let _ = write!(line, " recul par mise à jour 0x{first:08X}-0x{last:08X}");
                }
            }
            line.push_str(" origines");
            for origin in origins {
                let _ = write!(line, " {}", location_text(*origin));
            }
            vec![line]
        }
        Treatment::SlowClock {
            register,
            capture,
            trigger,
            hz,
            bound,
            qemu,
        } => vec![format!(
            "horloge lente 0x{register:08X} capture 0x{capture:08X} bit {} {hz} Hz écart {bound} \
             qemu 0x{qemu:08X}",
            trigger.trailing_zeros()
        )],
        Treatment::Compat(setting) => vec![format!(
            "réglage {}",
            match setting {
                Compat::SystimerNoAlarmOnStoppedCounter => "systimer_no_alarm_on_stopped_counter",
                Compat::CycleCounterOnTimeBase => "cycle_counter_on_time_base",
            }
        )],
    }
}

fn location_text(location: Location) -> String {
    match location {
        Location::Mmio(address) => format!("0x{address:08X}"),
        Location::Csr(csr) => format!("csr 0x{csr:03X}"),
    }
}

#[test]
fn the_table_of_the_documentation_is_the_table_of_the_tool() {
    let text = documentation();
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
    assert_eq!(summary(&text), tool);
}

#[test]
fn every_row_of_the_documentation_declares_the_parameters_of_the_tool() {
    let documented = parameters(&documentation());
    let tool: BTreeMap<u16, Vec<String>> = lockstep::DIFFERENCES
        .iter()
        .map(|row| {
            (
                row.number,
                row.treatments.iter().flat_map(lines_of).collect(),
            )
        })
        .collect();
    for (number, lines) in &tool {
        assert_eq!(
            documented.get(number),
            Some(lines),
            "row {number} of docs/lockstep-differences.md"
        );
    }
    assert_eq!(
        documented.keys().collect::<Vec<_>>(),
        tool.keys().collect::<Vec<_>>(),
        "the rows of the documentation are those of the tool"
    );
}

#[test]
fn the_rows_are_numbered_from_1_without_a_gap() {
    let numbers: Vec<u16> = lockstep::DIFFERENCES.iter().map(|row| row.number).collect();
    let expected: Vec<u16> = (1..=u16::try_from(numbers.len()).unwrap()).collect();
    assert_eq!(numbers, expected);
}
