//! What the emulator has to tell the user about the run: accesses it does not model, CPU exceptions.
use cpu_rv32::Exception;
use std::collections::BTreeMap;
use std::fmt;

/// Past this many distinct diagnostics, further new ones are counted (see [`Diagnostics::suppressed`])
/// but no longer listed: a runaway program must not grow the log without bound.
pub const MAX_DISTINCT_DIAGNOSTICS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Read,
    Write,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diagnostic {
    /// Spec §5.2: `value` is what the read returned, or what the ignored write carried.
    UnmodeledMmio {
        addr: u32,
        direction: Direction,
        pc: u32,
        register: Option<(&'static str, &'static str)>,
        value: u32,
        reset_explicit: bool,
    },
    /// An 8- or 16-bit store to a peripheral register (decision D9 of the SP1b-1 plan).
    SubWordWrite { addr: u32, bytes: u32, pc: u32 },
    Exception {
        cause: Exception,
        tval: u32,
        pc: u32,
    },
    /// Emitted once when [`MAX_DISTINCT_DIAGNOSTICS`] is reached.
    Suppressed,
}

/// Deduplication key: what the diagnostic is about, not where it was first seen.
type Key = (u8, u32, u32);

impl Diagnostic {
    fn key(&self) -> Key {
        match *self {
            Diagnostic::UnmodeledMmio {
                addr,
                direction: Direction::Read,
                ..
            } => (0, addr, 0),
            Diagnostic::UnmodeledMmio {
                addr,
                direction: Direction::Write,
                ..
            } => (1, addr, 0),
            Diagnostic::SubWordWrite { addr, bytes, .. } => (2, addr, bytes),
            Diagnostic::Exception { cause, pc, .. } => (3, pc, cause.code()),
            Diagnostic::Suppressed => (4, 0, 0),
        }
    }

    const fn is_mmio(&self) -> bool {
        matches!(
            self,
            Diagnostic::UnmodeledMmio { .. } | Diagnostic::SubWordWrite { .. }
        )
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Diagnostic::UnmodeledMmio {
                addr,
                direction,
                pc,
                register,
                value,
                reset_explicit,
            } => {
                let what = match direction {
                    Direction::Read => "read",
                    Direction::Write => "write",
                };
                match register {
                    Some((peripheral, name)) => write!(
                        f,
                        "unmodeled register {what}: {peripheral}.{name} at {addr:#010x} (pc {pc:#010x}); "
                    )?,
                    None => write!(
                        f,
                        "unmodeled {what} at {addr:#010x} (pc {pc:#010x}): no register is documented there; "
                    )?,
                }
                match (direction, register.is_some() && reset_explicit) {
                    (Direction::Read, true) => {
                        write!(f, "returning its documented reset value {value:#010x}")
                    }
                    (Direction::Read, false) => {
                        write!(f, "returning 0, the SVD documents no reset value")
                    }
                    (Direction::Write, _) => write!(f, "value {value:#010x} ignored"),
                }
            }
            Diagnostic::SubWordWrite { addr, bytes, pc } => write!(
                f,
                "{bytes}-byte write to a 32-bit peripheral register at {addr:#010x} (pc {pc:#010x}); the whole register was written, its other bytes as zero"
            ),
            Diagnostic::Exception { cause, tval, pc } => write!(
                f,
                "CPU exception {cause:?} at pc {pc:#010x} (tval {tval:#010x})"
            ),
            Diagnostic::Suppressed => write!(
                f,
                "more than {MAX_DISTINCT_DIAGNOSTICS} distinct diagnostics; further new ones are counted but not listed"
            ),
        }
    }
}

#[derive(Debug)]
pub struct Diagnostics {
    strict: bool,
    /// The first diagnostic of each key and its number of occurrences. A `BTreeMap` keeps every
    /// observable order deterministic.
    seen: BTreeMap<Key, (Diagnostic, u64)>,
    pending: Vec<Diagnostic>,
    violation: Option<Diagnostic>,
    capped: bool,
    /// Occurrences dropped because [`MAX_DISTINCT_DIAGNOSTICS`] distinct ones were already known.
    suppressed: u64,
}

impl Diagnostics {
    #[must_use]
    pub fn new(strict: bool) -> Diagnostics {
        Diagnostics {
            strict,
            seen: BTreeMap::new(),
            pending: Vec::new(),
            violation: None,
            capped: false,
            suppressed: 0,
        }
    }

    pub fn record(&mut self, diagnostic: Diagnostic) {
        if self.strict && self.violation.is_none() && diagnostic.is_mmio() {
            self.violation = Some(diagnostic.clone());
        }
        let key = diagnostic.key();
        if let Some((_, count)) = self.seen.get_mut(&key) {
            *count += 1;
            return;
        }
        if self.seen.len() >= MAX_DISTINCT_DIAGNOSTICS {
            self.suppressed += 1;
            if !std::mem::replace(&mut self.capped, true) {
                self.pending.push(Diagnostic::Suppressed);
            }
            return;
        }
        self.seen.insert(key, (diagnostic.clone(), 1));
        self.pending.push(diagnostic);
    }

    /// Diagnostics seen for the first time since the last call, in order of first occurrence.
    pub fn drain(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.pending)
    }

    /// In strict mode, the first violation; `None` otherwise or once taken. A violation is the first
    /// unmodeled access **or** the first 8- or 16-bit write to a peripheral register, modeled or not.
    pub fn take_violation(&mut self) -> Option<Diagnostic> {
        self.violation.take()
    }

    #[must_use]
    pub fn occurrences(&self, diagnostic: &Diagnostic) -> u64 {
        self.seen
            .get(&diagnostic.key())
            .map_or(0, |&(_, count)| count)
    }

    /// How many occurrences were dropped because [`MAX_DISTINCT_DIAGNOSTICS`] distinct diagnostics were
    /// already known: none of them is listed, and each is counted here.
    #[must_use]
    pub const fn suppressed(&self) -> u64 {
        self.suppressed
    }

    /// At most `limit` diagnostics that occurred more than once, the most frequent first (ties in key
    /// order, so the result is deterministic), each with its number of occurrences.
    #[must_use]
    pub fn most_repeated(&self, limit: usize) -> Vec<(Diagnostic, u64)> {
        let mut repeated: Vec<(Diagnostic, u64)> = self
            .seen
            .values()
            .filter(|(_, count)| *count > 1)
            .cloned()
            .collect();
        // The map iterates in key order and the sort is stable: equal counts keep key order.
        repeated.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
        repeated.truncate(limit);
        repeated
    }

    #[must_use]
    pub fn distinct(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_at(addr: u32, pc: u32) -> Diagnostic {
        Diagnostic::UnmodeledMmio {
            addr,
            direction: Direction::Read,
            pc,
            register: Some(("LEDC", "CONF")),
            value: 7,
            reset_explicit: true,
        }
    }

    #[test]
    fn repeated_diagnostic_is_listed_once_and_counted() {
        let mut d = Diagnostics::new(false);
        for pc in 0..1000 {
            d.record(read_at(0x6001_9000, pc)); // the PC differs, the register does not
        }
        let listed = d.drain();
        assert_eq!(listed.len(), 1);
        assert_eq!(d.occurrences(&listed[0]), 1000);
        assert!(d.drain().is_empty(), "draining twice lists nothing new");
    }

    #[test]
    fn read_and_write_of_the_same_register_are_two_diagnostics() {
        let mut d = Diagnostics::new(false);
        d.record(read_at(0x6001_9000, 0));
        d.record(Diagnostic::UnmodeledMmio {
            addr: 0x6001_9000,
            direction: Direction::Write,
            pc: 0,
            register: None,
            value: 1,
            reset_explicit: false,
        });
        assert_eq!(d.drain().len(), 2);
    }

    #[test]
    fn distinct_diagnostics_are_capped() {
        let mut d = Diagnostics::new(false);
        for i in 0..u32::try_from(MAX_DISTINCT_DIAGNOSTICS).unwrap() + 10 {
            d.record(Diagnostic::SubWordWrite {
                addr: 0x6000_0000 + 4 * i,
                bytes: 1,
                pc: 0,
            });
        }
        let listed = d.drain();
        assert_eq!(d.distinct(), MAX_DISTINCT_DIAGNOSTICS);
        assert_eq!(listed.len(), MAX_DISTINCT_DIAGNOSTICS + 1);
        assert_eq!(listed.last(), Some(&Diagnostic::Suppressed));
    }

    fn sub_word_write_at(index: u32) -> Diagnostic {
        Diagnostic::SubWordWrite {
            addr: 0x6000_0000 + 4 * index,
            bytes: 1,
            pc: 0,
        }
    }

    #[test]
    fn occurrences_past_the_cap_are_counted_as_suppressed() {
        let mut d = Diagnostics::new(false);
        let cap = u32::try_from(MAX_DISTINCT_DIAGNOSTICS).unwrap();
        for i in 0..cap + 10 {
            d.record(sub_word_write_at(i));
        }
        assert_eq!(d.suppressed(), 10);
        d.record(sub_word_write_at(cap + 3)); // one more occurrence of a dropped diagnostic
        assert_eq!(d.suppressed(), 11);
        d.record(sub_word_write_at(0)); // a listed diagnostic is counted as such, not as suppressed
        assert_eq!(d.suppressed(), 11);
        assert_eq!(d.occurrences(&sub_word_write_at(0)), 2);
    }

    #[test]
    fn most_repeated_lists_the_frequent_ones_first_and_skips_the_single_ones() {
        let mut d = Diagnostics::new(false);
        let times = |d: &mut Diagnostics, index: u32, n: u32| {
            for _ in 0..n {
                d.record(sub_word_write_at(index));
            }
        };
        times(&mut d, 7, 1); // seen once: never listed
        times(&mut d, 3, 5);
        times(&mut d, 1, 9);
        times(&mut d, 2, 5); // ties with index 3: key order puts the lower address first
        let counts: Vec<(u32, u64)> = d
            .most_repeated(10)
            .into_iter()
            .map(|(diagnostic, count)| match diagnostic {
                Diagnostic::SubWordWrite { addr, .. } => ((addr - 0x6000_0000) / 4, count),
                other => panic!("unexpected {other}"),
            })
            .collect();
        assert_eq!(counts, [(1, 9), (2, 5), (3, 5)]);
        assert_eq!(d.most_repeated(2).len(), 2, "the limit applies");
        assert_eq!(Diagnostics::new(false).most_repeated(3).len(), 0);
    }

    #[test]
    fn writes_of_different_widths_to_one_address_are_two_diagnostics() {
        let mut d = Diagnostics::new(false);
        for bytes in [1, 2] {
            d.record(Diagnostic::SubWordWrite {
                addr: 0x6000_9054,
                bytes,
                pc: 0,
            });
        }
        assert_eq!(d.drain().len(), 2);
    }

    #[test]
    fn strict_mode_reports_the_first_mmio_diagnostic_as_a_violation() {
        let mut strict = Diagnostics::new(true);
        strict.record(Diagnostic::Exception {
            cause: cpu_rv32::Exception::IllegalInstr,
            tval: 0,
            pc: 4,
        });
        assert_eq!(
            strict.take_violation(),
            None,
            "a CPU exception is not an MMIO violation"
        );
        strict.record(read_at(0x6001_9000, 8));
        strict.record(read_at(0x6001_9004, 12));
        assert_eq!(strict.take_violation(), Some(read_at(0x6001_9000, 8)));
        assert_eq!(strict.take_violation(), None);
        let mut lenient = Diagnostics::new(false);
        lenient.record(read_at(0x6001_9000, 8));
        assert_eq!(lenient.take_violation(), None);
    }

    #[test]
    fn messages_name_peripheral_register_address_and_pc() {
        let text = read_at(0x6001_9000, 0x4000_1234).to_string();
        for part in ["LEDC.CONF", "0x60019000", "0x40001234", "0x00000007"] {
            assert!(text.contains(part), "{part} missing from: {text}");
        }
        let undocumented = Diagnostic::UnmodeledMmio {
            addr: 0x6000_4038,
            direction: Direction::Read,
            pc: 0,
            register: Some(("GPIO", "STRAP")),
            value: 0,
            reset_explicit: false,
        };
        assert!(
            undocumented
                .to_string()
                .contains("documents no reset value"),
            "{undocumented}"
        );
        let nowhere = Diagnostic::UnmodeledMmio {
            addr: 0x600C_5000,
            direction: Direction::Write,
            pc: 0,
            register: None,
            value: 9,
            reset_explicit: false,
        };
        assert!(
            nowhere.to_string().contains("no register is documented"),
            "{nowhere}"
        );
    }
}
