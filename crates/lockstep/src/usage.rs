//! What each row of the table did in a run, and the check of decision H3: every row is used, at
//! every site it declares and with every bit it declares there, and none is used outside what it
//! declares.
//!
//! The comparison records what it actually did (the site, the bits it took from QEMU, how far
//! the gap of a counter moved), not what the row allowed; [`Usage::check`] then holds that record
//! to the table, so a fault of the comparison that copied a bit no row declares fails the run as
//! surely as a register entry, or a bit of a mask, the window never needed.

use crate::table::{Difference, Location, Site, Treatment};
use soc_esp32c3::CompatActs;
use std::collections::BTreeMap;
use std::fmt;

/// What a row did at one site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SiteUse {
    /// Accesses admitted and values taken from QEMU there.
    pub uses: u64,
    /// The bits taken from QEMU, all uses together.
    pub adopted: u32,
    /// The largest move of the gap of a counter, or of a clock from its expected value.
    pub largest_move: u32,
}

/// What one row did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowUse {
    pub uses: u64,
    pub sites: BTreeMap<Site, SiteUse>,
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
    /// A site the row declares was never used: a register entry the window does not reach.
    UnusedSite { row: u16, site: Site },
    /// Bits the row declares at a site were never taken from QEMU there.
    UnusedBits { row: u16, site: Site, bits: u32 },
    /// A row the table does not have was used.
    NoSuchRow { row: u16 },
    /// The row acted at a site it does not declare, or took bits it does not declare there.
    Outside { row: u16, site: Site, bits: u32 },
    /// A counter's gap, or a clock, moved past the bound of its row.
    OverBound {
        row: u16,
        location: Location,
        moved: u32,
    },
}

impl Misuse {
    /// Whether the misuse is a part of the table the run did not use, which a window cut short
    /// may leave unused without the table being wrong.
    #[must_use]
    pub const fn is_unused(&self) -> bool {
        matches!(
            self,
            Misuse::Unused { .. } | Misuse::UnusedSite { .. } | Misuse::UnusedBits { .. }
        )
    }
}

impl fmt::Display for Misuse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Misuse::Unused { row } => write!(f, "row {row} was never used"),
            Misuse::UnusedSite { row, site } => {
                write!(f, "row {row} was never used at {site}, which it declares")
            }
            Misuse::UnusedBits { row, site, bits } => write!(
                f,
                "row {row} never took bits {bits:#010x} at {site}, which it declares"
            ),
            Misuse::NoSuchRow { row } => {
                write!(f, "row {row}, which the table does not have, was used")
            }
            Misuse::Outside { row, site, bits } => write!(
                f,
                "row {row} took bits {bits:#010x} at {site}, which it does not declare"
            ),
            Misuse::OverBound {
                row,
                location,
                moved,
            } => write!(
                f,
                "row {row} admitted a move of {moved} at {location}, past its bound"
            ),
        }
    }
}

/// The verdict of [`Usage::check`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    /// What fails the run.
    pub failures: Vec<Misuse>,
    /// What a window cut short left unused: reported, not failed.
    pub unused_in_partial_window: Vec<Misuse>,
}

impl Check {
    /// Whether the run passes.
    #[must_use]
    pub fn passes(&self) -> bool {
        self.failures.is_empty()
    }
}

impl Usage {
    /// One use of `row` at `site`: `adopted` the bits taken from QEMU (0 for an access admitted
    /// with nothing copied), `moved` how far the gap of a counter, or a clock, moved.
    pub fn record(&mut self, row: u16, site: Site, adopted: u32, moved: u32) {
        let use_ = self.rows.entry(row).or_default();
        use_.uses += 1;
        let at = use_.sites.entry(site).or_default();
        at.uses += 1;
        at.adopted |= adopted;
        at.largest_move = at.largest_move.max(moved);
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

    /// Holds the record to `rows` (decision H3): every row used, at every site it declares and
    /// with every bit it declares there, none outside what it declares, no counter past its
    /// bound. When the window was cut short (`complete` false), what it left unused is
    /// reported in [`Check::unused_in_partial_window`] instead of failing the run.
    #[must_use]
    pub fn check(&self, rows: &[Difference], complete: bool) -> Check {
        let mut misuses = Vec::new();
        for (&number, use_) in &self.rows {
            let Some(row) = rows.iter().find(|row| row.number == number) else {
                misuses.push(Misuse::NoSuchRow { row: number });
                continue;
            };
            for (&site, at) in &use_.sites {
                match row.declared(site) {
                    Some(declared) if at.adopted & !declared == 0 => {}
                    declared => misuses.push(Misuse::Outside {
                        row: number,
                        site,
                        bits: at.adopted & !declared.unwrap_or(0),
                    }),
                }
                if let Some(bound) = row.bound(site.location)
                    && at.largest_move > bound
                {
                    misuses.push(Misuse::OverBound {
                        row: number,
                        location: site.location,
                        moved: at.largest_move,
                    });
                }
            }
        }
        for row in rows {
            let Some(use_) = self.rows.get(&row.number).filter(|use_| use_.uses > 0) else {
                misuses.push(Misuse::Unused { row: row.number });
                continue;
            };
            for (site, bits) in row.sites() {
                match use_.sites.get(&site) {
                    None => misuses.push(Misuse::UnusedSite {
                        row: row.number,
                        site,
                    }),
                    Some(at) if bits & !at.adopted != 0 => misuses.push(Misuse::UnusedBits {
                        row: row.number,
                        site,
                        bits: bits & !at.adopted,
                    }),
                    Some(_) => {}
                }
            }
        }
        let (unused, failures): (Vec<Misuse>, Vec<Misuse>) = misuses
            .into_iter()
            .partition(|misuse| !complete && misuse.is_unused());
        Check {
            failures,
            unused_in_partial_window: unused,
        }
    }
}

impl fmt::Display for Usage {
    /// One line per row: its uses, then each site with its uses, the bits taken and, for a
    /// counter or a clock, the largest move.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (number, use_) in &self.rows {
            write!(f, "row {number:>2}: {} uses", use_.uses)?;
            for (site, at) in &use_.sites {
                write!(f, "; {site} x{} bits {:#010x}", at.uses, at.adopted)?;
                if at.largest_move > 0 {
                    write!(f, " largest move {}", at.largest_move)?;
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
    use crate::table::{Compat, Drift, Register};

    const ROWS: &[Difference] = &[
        Difference {
            number: 1,
            summary: "a register",
            treatments: &[Treatment::Registers(&[
                Register {
                    address: 0x6000_0010,
                    mask: 0x0000_00F0,
                    qemu: None,
                    after_command: None,
                },
                Register {
                    address: 0x6000_0014,
                    mask: 0,
                    qemu: None,
                    after_command: None,
                },
            ])],
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
                drift: Drift::Within(1),
                origins: &[],
            }],
        },
        Difference {
            number: 4,
            summary: "a setting",
            treatments: &[Treatment::Compat(Compat::SystimerNoAlarmOnStoppedCounter)],
        },
    ];

    fn mmio(address: u32) -> Site {
        Site::at(Location::Mmio(address))
    }

    fn csr(address: u16) -> Site {
        Site::at(Location::Csr(address))
    }

    fn every_site_used() -> Usage {
        let mut usage = Usage::default();
        usage.record(1, mmio(0x6000_0010), 0x30, 0);
        usage.record(1, mmio(0x6000_0010), 0xC0, 0);
        usage.record(1, mmio(0x6000_0014), 0, 0);
        usage.record(2, csr(0x300), 0x1800, 0);
        usage.record(3, csr(0x802), u32::MAX, 1);
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
    fn a_run_that_uses_every_site_and_bit_within_its_declaration_passes() {
        assert!(every_site_used().check(ROWS, true).passes());
    }

    #[test]
    fn a_row_never_used_fails_the_run_unless_the_window_was_cut_short() {
        let mut usage = every_site_used();
        usage.rows.remove(&2);
        // A setting that never acted is not a use, whatever the machine was built with.
        usage.rows.remove(&4);
        usage.record_compat(ROWS, &CompatActs::default());
        let check = usage.check(ROWS, true);
        assert_eq!(
            check.failures,
            vec![Misuse::Unused { row: 2 }, Misuse::Unused { row: 4 }]
        );
        let check = usage.check(ROWS, false);
        assert!(check.passes(), "{check:?}");
        assert_eq!(check.unused_in_partial_window.len(), 2);
    }

    #[test]
    fn a_dead_register_entry_or_bit_inside_a_used_row_fails_the_run() {
        let mut usage = Usage::default();
        usage.record(1, mmio(0x6000_0010), 0x30, 0);
        usage.record(2, csr(0x300), 0x1800, 0);
        usage.record(3, csr(0x802), 5, 1);
        usage.record_compat(
            ROWS,
            &CompatActs {
                systimer_alarms_withheld: 1,
                ..CompatActs::default()
            },
        );
        assert_eq!(
            usage.check(ROWS, true).failures,
            vec![
                Misuse::UnusedBits {
                    row: 1,
                    site: mmio(0x6000_0010),
                    bits: 0xC0
                },
                Misuse::UnusedSite {
                    row: 1,
                    site: mmio(0x6000_0014)
                },
            ]
        );
    }

    #[test]
    fn a_row_applied_outside_its_registers_or_its_bits_fails_the_run_even_cut_short() {
        let mut usage = every_site_used();
        usage.record(1, mmio(0x6000_0010), 0x100, 0);
        usage.record(1, mmio(0x6000_0018), 0, 0);
        usage.record(2, csr(0x304), 1, 0);
        let expected = vec![
            Misuse::Outside {
                row: 1,
                site: mmio(0x6000_0010),
                bits: 0x100,
            },
            Misuse::Outside {
                row: 1,
                site: mmio(0x6000_0018),
                bits: 0,
            },
            Misuse::Outside {
                row: 2,
                site: csr(0x304),
                bits: 1,
            },
        ];
        assert_eq!(usage.check(ROWS, true).failures, expected);
        assert_eq!(usage.check(ROWS, false).failures, expected);
    }

    #[test]
    fn a_counter_past_its_bound_and_an_unknown_row_fail_the_run() {
        let mut usage = every_site_used();
        usage.record(3, csr(0x802), u32::MAX, 2);
        usage.record(9, csr(0x300), 0, 0);
        assert_eq!(
            usage.check(ROWS, true).failures,
            vec![
                Misuse::OverBound {
                    row: 3,
                    location: Location::Csr(0x802),
                    moved: 2
                },
                Misuse::NoSuchRow { row: 9 },
            ]
        );
    }
}
