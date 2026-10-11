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

/// An operation the emulator cannot perform faithfully: it is reported, and `--strict-mmio` stops on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// An SPI flash command the flash model does not implement: a program, an erase, a data read,
    /// or an opcode it does not know (D3).
    FlashCommand { opcode: u8 },
    /// An SPI1 user transfer whose command phase is not the 8-bit one the controller model
    /// implements: no command phase at all (`None`), or a command of `command_bits` bits.
    FlashTransfer {
        command_bits: Option<u32>,
        user: u32,
        user2: u32,
    },
    /// A write to `SHA.DMA_BLOCK_NUM`, `DMA_START` or `DMA_CONTINUE` (D7).
    ShaDma,
    /// A `SHA.MODE` the ESP32-C3 accelerator does not have.
    ShaMode { mode: u32 },
    /// A clock selection without a known CPU frequency (D22).
    ClockSource { sysclk_conf: u32, cpu_per_conf: u32 },
    /// `RTC_CNTL.CLK_CONF` selects a slow clock other than the RC oscillator, the only one modelled.
    SlowClock { clk_conf: u32 },
    /// A byte written to a UART whose transmit FIFO was full.
    /// ASSUMPTION: the byte is dropped. No source says what the silicon does with such a write;
    /// writing 129 bytes or more without waiting on `TXFIFO_CNT` on a real chip and reading what
    /// comes out would settle it.
    UartOverflow { uart: u8 },
    /// A byte sent by a UART whose `CLK_CONF.SCLK_SEL` selects a source clock the model has no
    /// frequency for: the RC fast clock (2), or no documented source (0). The byte is timed with
    /// the crystal.
    UartClock { uart: u8, sclk_sel: u32 },
}

impl Unsupported {
    /// The message of the diagnostic: what the instruction at `pc` asked for and why it was not
    /// done, then what the user can do.
    fn describe(self, pc: u32, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unsupported::FlashCommand { opcode } => write!(
                f,
                "flash command {opcode:#04x} is not modeled (pc {pc:#010x}): only the identification and status commands of the virtual flash are modeled, so nothing was read or changed; use firmware that sends no other flash command until SPI1 data reads and flash writes are emulated"
            ),
            Unsupported::FlashTransfer {
                command_bits,
                user,
                user2,
            } => {
                write!(f, "SPI1 was asked for a flash transfer ")?;
                match command_bits {
                    Some(bits) => write!(f, "with a {bits}-bit command")?,
                    None => write!(f, "without a command phase")?,
                }
                write!(
                    f,
                    " (USER {user:#010x}, USER2 {user2:#010x}, pc {pc:#010x}) but only transfers with an 8-bit command are modeled, so nothing was sent to the flash; use firmware whose flash commands are 8 bits long until other transfers are emulated"
                )
            }
            Unsupported::ShaDma => write!(
                f,
                "SHA.DMA_BLOCK_NUM, DMA_START or DMA_CONTINUE was written (pc {pc:#010x}) but only the typical SHA mode is modeled, so DMA-SHA produces no digest; use firmware that hashes through SHA.START/CONTINUE until DMA-SHA is emulated"
            ),
            Unsupported::ShaMode { mode } => write!(
                f,
                "SHA.MODE {mode} selects an algorithm the ESP32-C3 accelerator does not have (pc {pc:#010x}), so the block was not hashed; use SHA-1 (0), SHA-224 (1) or SHA-256 (2)"
            ),
            Unsupported::ClockSource {
                sysclk_conf,
                cpu_per_conf,
            } => write!(
                f,
                "the clock registers select a CPU clock inkemu cannot model (SYSCLK_CONF {sysclk_conf:#010x}, CPU_PER_CONF {cpu_per_conf:#010x}, pc {pc:#010x}), so emulated time keeps the previous frequency; select the crystal or the PLL at 80 or 160 MHz"
            ),
            Unsupported::SlowClock { clk_conf } => write!(
                f,
                "RTC_CNTL.CLK_CONF selects an RTC slow clock inkemu does not model (value {clk_conf:#010x}, pc {pc:#010x}), so the RTC timer and the RTC watchdog keep counting the 136 kHz RC oscillator; select the RC oscillator (ANA_CLK_RTC_SEL = 0) or expect their durations to differ from the device"
            ),
            Unsupported::UartOverflow { uart } => write!(
                f,
                "a byte was written to UART{uart} while its 128-byte transmit FIFO was full (pc {pc:#010x}), so inkemu dropped the byte; wait for room in the FIFO (UART.STATUS TXFIFO_CNT) before writing"
            ),
            Unsupported::UartClock { uart, sclk_sel } => write!(
                f,
                "UART{uart} sent a byte with a source clock inkemu does not model (CLK_CONF.SCLK_SEL = {sclk_sel}, pc {pc:#010x}), so the byte was timed with the 40 MHz crystal and its duration differs from the device; select the crystal (SCLK_SEL = 3) or APB_CLK (SCLK_SEL = 1)"
            ),
        }
    }
}

/// Why an access through the flash MMU could not be served from the flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingFault {
    /// Bit 8 of the MMU entry is set.
    InvalidEntry,
    /// The entry names a page past the end of the flash.
    BeyondFlash,
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
    /// An operation the emulator cannot perform faithfully.
    Unsupported { what: Unsupported, pc: u32 },
    /// An access to a flash window that no valid MMU entry backs.
    FlashMapping {
        vaddr: u32,
        entry: u32,
        content: u32,
        fault: MappingFault,
        pc: u32,
    },
    /// A synchronous exception the CPU took. It is no error of the emulator: the firmware's trap
    /// handler runs, as on the device.
    Exception {
        cause: Exception,
        tval: u32,
        pc: u32,
        /// The stack pointer when the instruction started. An exception is listed once per cause
        /// and PC: when the same instruction faults again with another stack pointer, the entry
        /// that is shown keeps the stack pointer of the first occurrence.
        sp: u32,
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
            Diagnostic::Unsupported { what, .. } => match what {
                Unsupported::FlashCommand { opcode } => (5, u32::from(opcode), 0),
                Unsupported::ShaDma => (6, 0, 0),
                Unsupported::ShaMode { mode } => (7, mode, 0),
                Unsupported::ClockSource {
                    sysclk_conf,
                    cpu_per_conf,
                } => (8, sysclk_conf, cpu_per_conf),
                Unsupported::SlowClock { .. } => (9, 0, 0),
                Unsupported::UartOverflow { uart } => (12, u32::from(uart), 0),
                Unsupported::UartClock { uart, sclk_sel } => (13, u32::from(uart), sclk_sel),
                // A command is 1 to 16 bits long, so 0 is free for "no command phase".
                Unsupported::FlashTransfer { command_bits, .. } => {
                    (11, command_bits.unwrap_or(0), 0)
                }
            },
            Diagnostic::FlashMapping { entry, fault, .. } => (10, entry, fault as u32),
        }
    }

    /// Whether `--strict-mmio` stops the run on this diagnostic.
    const fn stops_a_strict_run(&self) -> bool {
        matches!(
            self,
            Diagnostic::UnmodeledMmio { .. }
                | Diagnostic::SubWordWrite { .. }
                | Diagnostic::Unsupported { .. }
                | Diagnostic::FlashMapping { .. }
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
            Diagnostic::Unsupported { what, pc } => what.describe(pc, f),
            Diagnostic::FlashMapping {
                vaddr,
                entry,
                content,
                fault,
                pc,
            } => match fault {
                MappingFault::InvalidEntry => write!(
                    f,
                    "access to the flash window at {vaddr:#010x} through MMU entry {entry}, which is invalid (content {content:#05x}, pc {pc:#010x}): 0 was returned; the firmware used flash it had not mapped"
                ),
                MappingFault::BeyondFlash => write!(
                    f,
                    "access to the flash window at {vaddr:#010x} through MMU entry {entry} (content {content:#05x}, pc {pc:#010x}), which names a page beyond the end of the flash: 0 was returned; check the flash size of the device profile"
                ),
            },
            Diagnostic::Exception {
                cause,
                tval,
                pc,
                sp,
            } => write!(
                f,
                "CPU exception: {cause} at pc {pc:#010x} (tval {tval:#010x}, sp {sp:#010x}); the firmware's trap handler takes it, as on the device"
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
        if self.strict && self.violation.is_none() && diagnostic.stops_a_strict_run() {
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
    /// unmodeled access, sub-word register write, unsupported operation or invalid flash mapping.
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
            sp: 0,
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

    fn unsupported(what: Unsupported) -> Diagnostic {
        Diagnostic::Unsupported {
            what,
            pc: 0x4038_1000,
        }
    }

    fn mapping(entry: u32, fault: MappingFault) -> Diagnostic {
        Diagnostic::FlashMapping {
            vaddr: 0x3C00_0000 + (entry << 16),
            entry,
            content: 0x100,
            fault,
            pc: 0x4004_a186,
        }
    }

    #[test]
    fn strict_mode_stops_on_the_events_that_cannot_be_right_silently() {
        for diagnostic in [
            unsupported(Unsupported::FlashCommand { opcode: 0x02 }),
            unsupported(Unsupported::ShaDma),
            unsupported(Unsupported::ShaMode { mode: 5 }),
            unsupported(Unsupported::ClockSource {
                sysclk_conf: 0xC00,
                cpu_per_conf: 0xC,
            }),
            unsupported(Unsupported::SlowClock {
                clk_conf: 0x4000_0000,
            }),
            unsupported(Unsupported::UartOverflow { uart: 0 }),
            unsupported(Unsupported::UartClock {
                uart: 1,
                sclk_sel: 2,
            }),
            mapping(3, MappingFault::InvalidEntry),
            mapping(3, MappingFault::BeyondFlash),
        ] {
            let mut strict = Diagnostics::new(true);
            strict.record(diagnostic.clone());
            assert_eq!(
                strict.take_violation(),
                Some(diagnostic.clone()),
                "{diagnostic}"
            );
            let mut lenient = Diagnostics::new(false);
            lenient.record(diagnostic.clone());
            assert_eq!(lenient.take_violation(), None);
            assert_eq!(lenient.drain(), vec![diagnostic]);
        }
    }

    #[test]
    fn a_cpu_exception_is_still_not_a_strict_violation() {
        let mut d = Diagnostics::new(true);
        d.record(Diagnostic::Exception {
            cause: cpu_rv32::Exception::IllegalInstr,
            tval: 0,
            pc: 4,
            sp: 0,
        });
        assert_eq!(d.take_violation(), None);
    }

    #[test]
    fn unsupported_events_are_deduplicated_by_what_they_are_about() {
        let mut d = Diagnostics::new(false);
        d.record(unsupported(Unsupported::FlashCommand { opcode: 0x02 }));
        d.record(Diagnostic::Unsupported {
            what: Unsupported::FlashCommand { opcode: 0x02 },
            pc: 0x4200_0000,
        });
        d.record(unsupported(Unsupported::FlashCommand { opcode: 0x20 }));
        d.record(mapping(3, MappingFault::InvalidEntry));
        d.record(mapping(3, MappingFault::InvalidEntry));
        d.record(mapping(4, MappingFault::InvalidEntry));
        assert_eq!(d.distinct(), 4);
        assert_eq!(
            d.occurrences(&unsupported(Unsupported::FlashCommand { opcode: 0x02 })),
            2
        );
    }

    #[test]
    fn new_messages_state_the_cause_then_the_remedy() {
        let texts = [
            (
                unsupported(Unsupported::FlashCommand { opcode: 0x02 }),
                "flash command 0x02 is not modeled",
            ),
            (unsupported(Unsupported::ShaDma), "DMA-SHA"),
            (unsupported(Unsupported::ShaMode { mode: 5 }), "SHA.MODE 5"),
            (
                unsupported(Unsupported::ClockSource {
                    sysclk_conf: 0xC00,
                    cpu_per_conf: 0xC,
                }),
                "SYSCLK_CONF 0x00000c00",
            ),
            (
                unsupported(Unsupported::SlowClock {
                    clk_conf: 0x4000_0000,
                }),
                "RTC slow clock",
            ),
            (
                unsupported(Unsupported::UartClock {
                    uart: 1,
                    sclk_sel: 2,
                }),
                "UART1 sent a byte with a source clock inkemu does not model (CLK_CONF.SCLK_SEL = 2",
            ),
            (mapping(3, MappingFault::InvalidEntry), "MMU entry 3"),
            (
                mapping(3, MappingFault::BeyondFlash),
                "beyond the end of the flash",
            ),
        ];
        for (diagnostic, needle) in texts {
            let text = diagnostic.to_string();
            assert!(text.contains(needle), "{text}");
            assert!(text.contains("pc 0x40"), "{text}");
            let (cause, remedy) = text
                .split_once("; ")
                .unwrap_or_else(|| panic!("no remedy in: {text}"));
            assert!(!cause.is_empty() && !remedy.is_empty(), "{text}");
        }
    }

    #[test]
    fn a_refused_flash_command_is_described_truthfully_whatever_the_command_does() {
        // The same text serves a program (0x02), an erase (0x20), a data read (0x03, 0x0B, 0xBB: how
        // `esp_flash` reads NVS), deep power-down (0xB9) and an opcode no chip has (0x00).
        for opcode in [0x02, 0x20, 0x03, 0x0B, 0xBB, 0xB9, 0x00] {
            let text = unsupported(Unsupported::FlashCommand { opcode }).to_string();
            assert!(
                text.contains(&format!("flash command {opcode:#04x} is not modeled")),
                "{text}"
            );
            assert!(
                text.contains(
                    "only the identification and status commands of the virtual flash are modeled, so nothing was read or changed"
                ),
                "{text}"
            );
            assert!(
                !text.contains("read-only") && !text.contains("program or erase"),
                "a refused read is not a write: {text}"
            );
        }
    }

    #[test]
    fn a_dma_sha_write_is_not_described_as_a_started_hash() {
        // The same diagnostic reports a write to `DMA_BLOCK_NUM`, which configures and starts nothing.
        let text = unsupported(Unsupported::ShaDma).to_string();
        assert!(
            text.contains("SHA.DMA_BLOCK_NUM, DMA_START or DMA_CONTINUE was written"),
            "{text}"
        );
        assert!(!text.contains("was started"), "{text}");
    }

    #[test]
    fn a_flash_transfer_of_an_unmodeled_shape_says_which_shape() {
        let without = unsupported(Unsupported::FlashTransfer {
            command_bits: None,
            user: 0x1000_0000,
            user2: 0x7000_009F,
        });
        let sixteen_bits = unsupported(Unsupported::FlashTransfer {
            command_bits: Some(16),
            user: 0x9000_0000,
            user2: 0xF000_9F00,
        });
        assert!(
            without.to_string().contains("without a command phase"),
            "{without}"
        );
        assert!(
            sixteen_bits.to_string().contains("with a 16-bit command"),
            "{sixteen_bits}"
        );
        for diagnostic in [&without, &sixteen_bits] {
            let text = diagnostic.to_string();
            assert!(
                text.contains("USER 0x") && text.contains("USER2 0x") && text.contains("pc 0x40"),
                "{text}"
            );
            assert!(
                text.contains("nothing was sent to the flash; use firmware"),
                "{text}"
            );
        }
        // Two shapes are two diagnostics; the same shape with another opcode or data phase is one.
        let mut d = Diagnostics::new(true);
        d.record(without.clone());
        d.record(sixteen_bits);
        d.record(unsupported(Unsupported::FlashTransfer {
            command_bits: None,
            user: 0x0800_0000,
            user2: 0x7000_0002,
        }));
        assert_eq!(d.distinct(), 2);
        assert_eq!(d.occurrences(&without), 2);
        assert_eq!(
            d.take_violation(),
            Some(without),
            "a strict run stops on it"
        );
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

    #[test]
    fn an_exception_names_its_pc_its_tval_and_the_stack_pointer() {
        let text = Diagnostic::Exception {
            cause: cpu_rv32::Exception::LoadAccessFault,
            tval: 0x0000_0010,
            pc: 0x4200_1234,
            sp: 0x3FC9_0000,
        }
        .to_string();
        assert_eq!(
            text,
            "CPU exception: load access fault at pc 0x42001234 (tval 0x00000010, sp 0x3fc90000); the firmware's trap handler takes it, as on the device"
        );
        assert!(
            !text.contains("LoadAccessFault"),
            "words for the user, not the identifier of the code"
        );
    }

    #[test]
    fn the_same_exception_at_the_same_pc_is_listed_once_whatever_the_stack_pointer() {
        let mut d = Diagnostics::new(false);
        for sp in [0x3FC9_0000, 0x3FC9_0010] {
            d.record(Diagnostic::Exception {
                cause: cpu_rv32::Exception::IllegalInstr,
                tval: 0,
                pc: 4,
                sp,
            });
        }
        let listed = d.drain();
        assert_eq!(listed.len(), 1, "one line per cause and PC (decision E12)");
        assert!(
            matches!(
                listed[0],
                Diagnostic::Exception {
                    sp: 0x3FC9_0000,
                    ..
                }
            ),
            "the entry shown is the first occurrence, with its stack pointer: {:?}",
            listed[0]
        );
        assert_eq!(d.occurrences(&listed[0]), 2, "the second one is counted");
    }
}
