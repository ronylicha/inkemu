//! The windows of the lockstep (SP1b-4d, decision J3): a firmware, how many resets of both sides
//! the comparison goes on through, where it ends otherwise, and the rows of the table it uses.
//!
//! A row keeps its parameters in every window that uses it; a window that shows other bits or
//! other registers has rows of its own, measured on it. Every run holds the rows of its window
//! to H3 ([`crate::Usage::check`]): each one used at every site and bit it declares, and none
//! outside. `docs/lockstep-differences.md` lists the windows; a test holds the two lists equal.

use crate::table::{self, Difference};

/// One window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// Its name, as `inkemu-lockstep --window` takes it.
    pub name: &'static str,
    /// The firmware it compares, as the reference build names it (`tools/reference-firmware`).
    pub firmware: &'static str,
    /// The resets of both sides the comparison goes on through; the next one ends the window.
    pub resets: u32,
    /// The compared instructions after which the window ends, when nothing ended it before: a
    /// bound of the window, checked whole like any other end (unlike `--max-instructions`).
    pub bound: Option<u64>,
    /// The rows of the table it uses, in the order of the table.
    pub rows: &'static [Difference],
}

/// The windows, in the order of `docs/lockstep-differences.md`.
pub const WINDOWS: &[Window] = &[
    Window {
        name: "hello",
        firmware: "flash_hello.bin",
        resets: 1,
        bound: None,
        rows: table::HELLO,
    },
    Window {
        name: "nvs",
        firmware: "flash_nvs.bin",
        resets: 1,
        bound: None,
        rows: table::NVS,
    },
    Window {
        name: "nvs-ota",
        firmware: "flash_nvs_ota.bin",
        resets: 1,
        bound: None,
        rows: table::NVS_OTA,
    },
    Window {
        name: "crosspoint",
        firmware: "crosspoint-1.6.5-x3-x4.bin",
        resets: 0,
        bound: None,
        rows: table::CROSSPOINT,
    },
];

/// The window called `name`.
#[must_use]
pub fn window(name: &str) -> Option<&'static Window> {
    WINDOWS.iter().find(|window| window.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{DIFFERENCES, Location, Treatment};
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn every_row_of_a_window_is_a_row_of_the_table_and_every_row_serves_a_window() {
        let mut served = BTreeSet::new();
        for window in WINDOWS {
            let mut numbers = BTreeSet::new();
            for row in window.rows {
                assert!(
                    DIFFERENCES.contains(row),
                    "window {}: row {} is not the table's",
                    window.name,
                    row.number
                );
                assert!(
                    numbers.insert(row.number),
                    "window {}: row {} twice",
                    window.name,
                    row.number
                );
                served.insert(row.number);
            }
        }
        for row in DIFFERENCES {
            assert!(
                served.contains(&row.number),
                "row {} serves no window",
                row.number
            );
        }
    }

    /// Where a treatment acts, and whether only after a reset: a register's copies, a counter, a
    /// slow clock. An entry after a reset adds bits to the copies of the same address
    /// (`Difference::registers_at`), so it is owned apart.
    fn owned(treatment: &Treatment) -> Vec<(bool, Location)> {
        match *treatment {
            Treatment::Registers(registers) => registers
                .iter()
                .map(|register| (false, Location::Mmio(register.address)))
                .collect(),
            Treatment::RegistersAfterReset(entries) => entries
                .iter()
                .map(|entry| (true, Location::Mmio(entry.register.address)))
                .collect(),
            Treatment::Counter { location, .. } => vec![(false, location)],
            Treatment::SlowClock { register, .. } => vec![(false, Location::Mmio(register))],
            _ => Vec::new(),
        }
    }

    #[test]
    fn a_register_counter_or_clock_is_named_by_one_row_of_a_window() {
        for window in WINDOWS {
            let mut owner = BTreeMap::new();
            for row in window.rows {
                for treatment in row.treatments {
                    // Two entries of a row at one address (an SPI1 data register, one per
                    // command) are one owner.
                    for site in owned(treatment) {
                        let first = *owner.entry(site).or_insert(row.number);
                        assert_eq!(first, row.number, "window {}: {site:?}", window.name);
                    }
                }
            }
        }
    }

    #[test]
    fn windows_have_names_of_their_own() {
        let names: BTreeSet<&str> = WINDOWS.iter().map(|window| window.name).collect();
        assert_eq!(names.len(), WINDOWS.len());
        assert!(window("hello").is_some());
        assert!(window("nothing").is_none());
    }
}
