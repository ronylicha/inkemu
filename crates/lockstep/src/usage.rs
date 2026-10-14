//! What each row of the table did in a run, and the check of decision H3: every row is used, and
//! none is used outside what it declares.
//!
//! The comparison records what it actually did (the location, the bits it took from QEMU, the
//! gap of a counter), not what the row allowed; [`Usage::check`] then holds that record to the
//! table, so a fault of the comparison that copied a bit no row declares fails the run as surely
//! as a row the window never needed.

use crate::table::{Difference, Location, Treatment};
use soc_esp32c3::CompatActs;
use std::collections::BTreeMap;
use std::fmt;

/// What a row did at one location.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocationUse {
    /// Accesses admitted and values taken from QEMU there.
    pub uses: u64,
    /// The bits taken from QEMU, all uses together.
    pub adopted: u32,
    /// The largest gap between the two values of a counter.
    pub largest_gap: u32,
}

/// What one row did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowUse {
    pub uses: u64,
    pub locations: BTreeMap<Location, LocationUse>,
}

/// What every row did in a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    rows: BTreeMap<u16, RowUse>,
}

/// A way a run used the table that H3 forbids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Misuse {
    /// The row was never used: the window does not need it.
    Unused { row: u16 },
    /// A row the table does not have was used.
    NoSuchRow { row: u16 },
    /// The row acted at a location it does not declare, or took bits it does not declare there.
    Outside {
        row: u16,
        location: Location,
        bits: u32,
    },
    /// A counter's gap went past the bound of its row.
    OverBound {
        row: u16,
        location: Location,
        gap: u32,
    },
}

impl fmt::Display for Misuse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Misuse::Unused { row } => write!(f, "row {row} was never used"),
            Misuse::NoSuchRow { row } => {
                write!(f, "row {row}, which the table does not have, was used")
            }
            Misuse::Outside {
                row,
                location,
                bits,
            } => write!(
                f,
                "row {row} took bits {bits:#010x} at {location}, which it does not declare"
            ),
            Misuse::OverBound { row, location, gap } => {
                write!(
                    f,
                    "row {row} admitted a gap of {gap} at {location}, past its bound"
                )
            }
        }
    }
}

impl Usage {
    /// One use of `row` at `location`: `adopted` the bits taken from QEMU (0 for an access
    /// admitted with nothing copied), `gap` the gap of a counter.
    pub fn record(&mut self, row: u16, location: Location, adopted: u32, gap: u32) {
        let use_ = self.rows.entry(row).or_default();
        use_.uses += 1;
        let at = use_.locations.entry(location).or_default();
        at.uses += 1;
        at.adopted |= adopted;
        at.largest_gap = at.largest_gap.max(gap);
    }

    /// The uses of the rows of treatment (iv), as the machine counted what their settings did.
    pub fn record_compat(&mut self, rows: &[Difference], acts: &CompatActs) {
        for row in rows {
            for treatment in row.treatments {
                if let Treatment::Compat(setting) = treatment {
                    let uses = setting.acts(acts);
                    if uses > 0 {
                        self.rows.entry(row.number).or_default().uses += uses;
                    }
                }
            }
        }
    }

    /// What each row did, by number.
    #[must_use]
    pub fn rows(&self) -> &BTreeMap<u16, RowUse> {
        &self.rows
    }

    /// The uses of `row`.
    #[must_use]
    pub fn uses(&self, row: u16) -> u64 {
        self.rows.get(&row).map_or(0, |use_| use_.uses)
    }

    /// Holds the record to `rows` (decision H3): every row used, none outside what it declares.
    ///
    /// # Errors
    /// Every [`Misuse`] found, in the order of the rows.
    pub fn check(&self, rows: &[Difference]) -> Result<(), Vec<Misuse>> {
        let mut misuses = Vec::new();
        for (&number, use_) in &self.rows {
            let Some(row) = rows.iter().find(|row| row.number == number) else {
                misuses.push(Misuse::NoSuchRow { row: number });
                continue;
            };
            for (&location, at) in &use_.locations {
                match row.declared(location) {
                    Some(declared) if at.adopted & !declared == 0 => {}
                    declared => misuses.push(Misuse::Outside {
                        row: number,
                        location,
                        bits: at.adopted & !declared.unwrap_or(0),
                    }),
                }
                if let Some(bound) = row.bound(location)
                    && at.largest_gap > bound
                {
                    misuses.push(Misuse::OverBound {
                        row: number,
                        location,
                        gap: at.largest_gap,
                    });
                }
            }
        }
        for row in rows {
            if self.uses(row.number) == 0 {
                misuses.push(Misuse::Unused { row: row.number });
            }
        }
        if misuses.is_empty() {
            Ok(())
        } else {
            Err(misuses)
        }
    }
}

impl fmt::Display for Usage {
    /// One line per row: its uses, then each location with the bits taken and, for a counter,
    /// the largest gap.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (number, use_) in &self.rows {
            write!(f, "row {number:>2}: {} uses", use_.uses)?;
            for (location, at) in &use_.locations {
                write!(f, "; {location} x{} bits {:#010x}", at.uses, at.adopted)?;
                if at.largest_gap > 0 {
                    write!(f, " gap {}", at.largest_gap)?;
                }
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Field;
    use crate::table::{Compat, Register};

    const ROWS: &[Difference] = &[
        Difference {
            number: 1,
            summary: "a register",
            treatments: &[Treatment::Registers(&[Register {
                address: 0x6000_0010,
                mask: 0x0000_00F0,
                qemu: None,
                after_command: None,
            }])],
        },
        Difference {
            number: 2,
            summary: "a pair",
            treatments: &[Treatment::Pair {
                field: Field::Mstatus,
                bits: 0x1800,
                qemu: 0,
                inkemu: 0x1800,
                until_written: false,
            }],
        },
        Difference {
            number: 3,
            summary: "a counter",
            treatments: &[Treatment::Counter {
                location: Location::Csr(0x802),
                bound: 1,
            }],
        },
        Difference {
            number: 4,
            summary: "a setting",
            treatments: &[Treatment::Compat(Compat::SystimerNoAlarmOnStoppedCounter)],
        },
    ];

    fn every_row_used() -> Usage {
        let mut usage = Usage::default();
        usage.record(1, Location::Mmio(0x6000_0010), 0x30, 0);
        usage.record(2, Location::Csr(0x300), 0x1800, 0);
        usage.record(3, Location::Csr(0x802), u32::MAX, 1);
        usage.record_compat(
            ROWS,
            &CompatActs {
                systimer_alarms_withheld: 1,
                ..CompatActs::default()
            },
        );
        usage
    }

    #[test]
    fn a_run_that_uses_every_row_within_its_declaration_passes() {
        assert_eq!(every_row_used().check(ROWS), Ok(()));
    }

    #[test]
    fn a_row_never_used_fails_the_run() {
        let mut usage = Usage::default();
        usage.record(1, Location::Mmio(0x6000_0010), 0x30, 0);
        usage.record(2, Location::Csr(0x300), 0x1800, 0);
        usage.record(3, Location::Csr(0x802), 5, 1);
        // A setting that never acted is not a use, whatever the machine was built with.
        usage.record_compat(ROWS, &CompatActs::default());
        assert_eq!(usage.check(ROWS), Err(vec![Misuse::Unused { row: 4 }]));
    }

    #[test]
    fn a_row_applied_outside_its_registers_or_its_bits_fails_the_run() {
        let mut usage = every_row_used();
        usage.record(1, Location::Mmio(0x6000_0010), 0x100, 0);
        usage.record(1, Location::Mmio(0x6000_0014), 0, 0);
        usage.record(2, Location::Csr(0x304), 1, 0);
        assert_eq!(
            usage.check(ROWS),
            Err(vec![
                Misuse::Outside {
                    row: 1,
                    location: Location::Mmio(0x6000_0010),
                    bits: 0x100
                },
                Misuse::Outside {
                    row: 1,
                    location: Location::Mmio(0x6000_0014),
                    bits: 0
                },
                Misuse::Outside {
                    row: 2,
                    location: Location::Csr(0x304),
                    bits: 1
                },
            ])
        );
    }

    #[test]
    fn a_counter_past_its_bound_and_an_unknown_row_fail_the_run() {
        let mut usage = every_row_used();
        usage.record(3, Location::Csr(0x802), u32::MAX, 2);
        usage.record(9, Location::Csr(0x300), 0, 0);
        assert_eq!(
            usage.check(ROWS),
            Err(vec![
                Misuse::OverBound {
                    row: 3,
                    location: Location::Csr(0x802),
                    gap: 2
                },
                Misuse::NoSuchRow { row: 9 },
            ])
        );
    }
}
