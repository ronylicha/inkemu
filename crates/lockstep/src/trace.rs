//! The reader of QEMU's trace (decision H5): the text `-d cpu,nochain,int` writes with
//! `-accel tcg,one-insn-per-tb=on`, read as it comes, one line at a time.
//!
//! A `-d cpu` block is the state *before* the instruction at its PC (`docs/research/sp1b-test-oracle.md`
//! §1c, validated against the gdbstub): it starts with the line ` V      =   0`, gives one CSR per
//! line, then the 32 integer registers four to a line. A `-d int` line says that the CPU took a
//! trap between two blocks:
//! `riscv_cpu_do_interrupt: hart:0, async:1, cause:0000000b, epc:0x40381bc8, tval:0x00000000, desc=m_external`.
//! Any other line (QEMU's `Adding SPI flash device`, a warning) is counted and skipped.
//!
//! The input is untrusted text: a line is read with a bound ([`MAX_LINE_BYTES`]) and a block with
//! another ([`MAX_BLOCK_LINES`]); nothing is sized by a value read.

use crate::state::{CSRS, CpuState, Field};
use std::io::{BufRead, Read};

/// The longest line the reader accepts. QEMU's longest lines are those of the integer registers
/// (77 bytes) and of `-d int` (about 110).
pub const MAX_LINE_BYTES: usize = 512;
/// The most lines a `-d cpu` block may have after its first. QEMU 9.2's RISC-V dump has 39 for
/// the ESP32-C3 (40 with the first): a longer block is two blocks merged, or not QEMU's.
pub const MAX_BLOCK_LINES: usize = 39;

/// The first line of a `-d cpu` block: the virtualisation mode, which the ESP32-C3 never enters.
const BLOCK_START: &[u8] = b" V      =   0";
/// The prefix of a `-d int` line.
const TRAP_PREFIX: &str = "riscv_cpu_do_interrupt:";

/// A trap QEMU took between two blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QemuTrap {
    /// An interrupt (`async:1`), not an exception.
    pub asynchronous: bool,
    /// The cause without its interrupt bit: the CPU line for an interrupt.
    pub cause: u32,
    pub epc: u32,
    pub tval: u32,
}

/// What the reader found next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    /// A `-d cpu` block, and a fingerprint of its whole text: two consecutive blocks with the
    /// same fingerprint print the same state, which the dedupe rule needs (oracle §1c pitfall 1).
    State { state: CpuState, fingerprint: u64 },
    /// A `-d int` line.
    Trap(QemuTrap),
}

/// Why the trace could not be read.
#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error("line {line} of the QEMU trace is longer than {MAX_LINE_BYTES} bytes")]
    LineTooLong { line: u64 },
    #[error(
        "the -d cpu block that starts on line {line} of the QEMU trace has more than {MAX_BLOCK_LINES} lines"
    )]
    BlockTooLong { line: u64 },
    #[error("the -d cpu block that starts on line {line} of the QEMU trace has no {what}")]
    Missing { line: u64, what: String },
    #[error("line {line} of the QEMU trace is not what QEMU writes: {text:?}")]
    Malformed { line: u64, text: String },
    #[error("the QEMU trace ends inside the -d cpu block that starts on line {line}")]
    Truncated { line: u64 },
    #[error("the QEMU trace could not be read: {0}")]
    Read(#[from] std::io::Error),
}

/// A reader of the trace, over any buffered source.
#[derive(Debug)]
pub struct TraceReader<R> {
    input: R,
    line: Vec<u8>,
    /// Lines read so far.
    lines: u64,
    /// Lines that are neither part of a block nor a trap.
    skipped: u64,
    /// Blocks read whole.
    blocks: u64,
}

/// The FNV-1a hash of a 64-bit seed: the fingerprint of a block is the hash of its lines.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

impl<R: BufRead> TraceReader<R> {
    pub fn new(input: R) -> TraceReader<R> {
        TraceReader {
            input,
            line: Vec::with_capacity(MAX_LINE_BYTES + 1),
            lines: 0,
            skipped: 0,
            blocks: 0,
        }
    }

    /// Lines that were neither part of a block nor a trap (QEMU's own messages).
    #[must_use]
    pub const fn skipped_lines(&self) -> u64 {
        self.skipped
    }

    /// `-d cpu` blocks read whole so far: none at the end of the input is a trace that is not
    /// QEMU's, or a QEMU that ended before its first instruction.
    #[must_use]
    pub const fn blocks_read(&self) -> u64 {
        self.blocks
    }

    /// Reads one line into `self.line`, without its end; `false` at the end of the input.
    fn read_line(&mut self) -> Result<bool, TraceError> {
        self.line.clear();
        let limit = u64::try_from(MAX_LINE_BYTES).unwrap_or(u64::MAX) + 1;
        let read = (&mut self.input)
            .take(limit)
            .read_until(b'\n', &mut self.line)?;
        if read == 0 {
            return Ok(false);
        }
        self.lines += 1;
        if self.line.last() == Some(&b'\n') {
            self.line.pop();
        } else if self.line.len() > MAX_LINE_BYTES {
            return Err(TraceError::LineTooLong { line: self.lines });
        }
        if self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        Ok(true)
    }

    /// The next event, `None` at the end of the trace.
    ///
    /// # Errors
    /// [`TraceError`] when a line is too long, a block is cut, too long or lacks a quantity, a
    /// trap line or a register value is not what QEMU writes, or the source fails.
    pub fn next_event(&mut self) -> Result<Option<TraceEvent>, TraceError> {
        loop {
            if !self.read_line()? {
                return Ok(None);
            }
            if self.line.as_slice() == BLOCK_START {
                return self.read_block().map(Some);
            }
            if self.line.starts_with(TRAP_PREFIX.as_bytes()) {
                let trap = self.parse_trap()?;
                return Ok(Some(TraceEvent::Trap(trap)));
            }
            self.skipped += 1;
        }
    }

    fn malformed(&self) -> TraceError {
        TraceError::Malformed {
            line: self.lines,
            text: String::from_utf8_lossy(&self.line).into_owned(),
        }
    }

    fn parse_trap(&self) -> Result<QemuTrap, TraceError> {
        let text = std::str::from_utf8(&self.line).map_err(|_| self.malformed())?;
        let field = |name: &str| -> Option<&str> {
            text.split(", ").find_map(|part| part.strip_prefix(name))
        };
        let hex = |value: Option<&str>| -> Option<u32> {
            let value = value?;
            u32::from_str_radix(value.strip_prefix("0x").unwrap_or(value), 16).ok()
        };
        let asynchronous = match field("async:") {
            Some("1") => true,
            Some("0") => false,
            _ => return Err(self.malformed()),
        };
        Ok(QemuTrap {
            asynchronous,
            cause: hex(field("cause:")).ok_or_else(|| self.malformed())? & !(1 << 31),
            epc: hex(field("epc:")).ok_or_else(|| self.malformed())?,
            tval: hex(field("tval:")).ok_or_else(|| self.malformed())?,
        })
    }

    fn read_block(&mut self) -> Result<TraceEvent, TraceError> {
        let start = self.lines;
        let mut fingerprint = fnv(FNV_OFFSET, BLOCK_START);
        let mut state = CpuState::new(0, [0; 32], |_| None);
        // Which quantities the block gave: the PC, the 31 registers, the CSRs.
        let mut seen = [false; crate::state::FIELDS];
        for _ in 0..MAX_BLOCK_LINES {
            if !self.read_line()? {
                return Err(TraceError::Truncated { line: start });
            }
            // A block starting inside another is two blocks merged: the one cut is not QEMU's.
            if self.line.as_slice() == BLOCK_START {
                return Err(self.malformed());
            }
            fingerprint = fnv(fingerprint, &self.line);
            fingerprint = fnv(fingerprint, b"\n");
            let text = std::str::from_utf8(&self.line).map_err(|_| self.malformed())?;
            let mut words = text.split_ascii_whitespace();
            let Some(first) = words.next() else {
                return Err(self.malformed());
            };
            if first.starts_with('x') && first.contains('/') {
                // ` x28/t3   00000000 x29/t4   00000000 x30/t5   00000000 x31/t6   00000000`
                let mut last_register = false;
                let mut name = Some(first);
                while let Some(register) = name {
                    let value = words.next().ok_or_else(|| self.malformed())?;
                    let number = register
                        .strip_prefix('x')
                        .and_then(|rest| rest.split('/').next())
                        .and_then(|n| n.parse::<u8>().ok())
                        .filter(|n| *n < 32)
                        .ok_or_else(|| self.malformed())?;
                    let value = u32::from_str_radix(value, 16).map_err(|_| self.malformed())?;
                    if number > 0 {
                        // A quantity given twice is not what QEMU writes.
                        if seen[usize::from(number)] {
                            return Err(self.malformed());
                        }
                        state.set(Field::X(number), value);
                        seen[usize::from(number)] = true;
                    }
                    last_register = number == 31;
                    name = words.next();
                }
                if last_register {
                    return match Field::all().find(|field| !seen[field.index()]) {
                        None => {
                            self.blocks += 1;
                            Ok(TraceEvent::State { state, fingerprint })
                        }
                        Some(missing) => Err(TraceError::Missing {
                            line: start,
                            what: missing.to_string(),
                        }),
                    };
                }
                continue;
            }
            let field = if first == "pc" {
                Some(Field::Pc)
            } else {
                CSRS.iter()
                    .find(|(_, _, name)| *name == first)
                    .map(|(field, _, _)| *field)
            };
            // A quantity that is not compared (an S- or H-mode CSR, `mhartid`, `satp`) is part
            // of the fingerprint and nothing else.
            if let Some(field) = field {
                let value = words.next().ok_or_else(|| self.malformed())?;
                let value = u32::from_str_radix(value, 16).map_err(|_| self.malformed())?;
                if seen[field.index()] {
                    return Err(self.malformed());
                }
                state.set(field, value);
                seen[field.index()] = true;
            }
        }
        Err(TraceError::BlockTooLong { line: start })
    }
}

/// Writes `state` as one `-d cpu` block of QEMU 9.2, the CSRs that are not compared at 0: what a
/// test feeds the reader, and a check that the reader reads what QEMU writes.
#[must_use]
pub fn render_block(state: &CpuState) -> String {
    use std::fmt::Write as _;
    let mut out = String::from(" V      =   0\n");
    let csr = |field: Field| state.get(field);
    let _ = writeln!(out, " pc       {:08x}", state.pc());
    let lines: [(&str, Option<Field>); 27] = [
        ("mhartid", None),
        ("mstatus", Some(Field::Mstatus)),
        ("mstatush", None),
        ("hstatus", None),
        ("vsstatus", None),
        ("mip", Some(Field::Mip)),
        ("mie", Some(Field::Mie)),
        ("mideleg", None),
        ("hideleg", None),
        ("medeleg", None),
        ("hedeleg", None),
        ("mtvec", Some(Field::Mtvec)),
        ("stvec", None),
        ("vstvec", None),
        ("mepc", Some(Field::Mepc)),
        ("sepc", None),
        ("vsepc", None),
        ("mcause", Some(Field::Mcause)),
        ("scause", None),
        ("vscause", None),
        ("mtval", Some(Field::Mtval)),
        ("stval", None),
        ("htval", None),
        ("mtval2", None),
        ("mscratch", Some(Field::Mscratch)),
        ("sscratch", None),
        ("satp", None),
    ];
    for (name, field) in lines {
        let _ = writeln!(out, " {name:<8} {:08x}", field.map_or(0, csr));
    }
    for row in 0..8u8 {
        for column in 0..4u8 {
            let n = row * 4 + column;
            let name = format!("x{n}/{}", abi(n));
            let _ = write!(out, " {name:<8} {:08x}", state.x(n));
        }
        out.push('\n');
    }
    out
}

fn abi(n: u8) -> &'static str {
    const ABI: [&str; 32] = [
        "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
        "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
        "t5", "t6",
    ];
    ABI[usize::from(n) & 31]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first block of a real trace (`docs/research/sp1b-test-oracle.md` §1c), as QEMU wrote it.
    const FIRST_BLOCK: &str = "Adding SPI flash device
 V      =   0
 pc       40000000
 mhartid  00000000
 mstatus  00000000
 mstatush 00000000
 hstatus  00000000
 vsstatus 00000000
 mip      00000000
 mie      00000eee
 mideleg  00001444
 hideleg  00000000
 medeleg  00000000
 hedeleg  00000000
 mtvec    00000000
 stvec    00000000
 vstvec   00000000
 mepc     00000000
 sepc     00000000
 vsepc    00000000
 mcause   00000000
 scause   00000000
 vscause  00000000
 mtval    00000000
 stval    00000000
 htval    00000000
 mtval2   00000000
 mscratch 00000000
 sscratch 00000000
 satp     00000000
 x0/zero  00000000 x1/ra    00000000 x2/sp    00000000 x3/gp    00000000
 x4/tp    00000000 x5/t0    00000000 x6/t1    00000000 x7/t2    00000000
 x8/s0    00000000 x9/s1    00000000 x10/a0   00000000 x11/a1   00000000
 x12/a2   00000000 x13/a3   00000000 x14/a4   00000000 x15/a5   00000000
 x16/a6   00000000 x17/a7   00000000 x18/s2   00000000 x19/s3   00000000
 x20/s4   00000000 x21/s5   00000000 x22/s6   00000000 x23/s7   00000000
 x24/s8   00000000 x25/s9   00000000 x26/s10  00000000 x27/s11  00000000
 x28/t3   00000000 x29/t4   00000000 x30/t5   00000000 x31/t6   00000000
riscv_cpu_do_interrupt: hart:0, async:1, cause:0000000b, epc:0x40381bc8, tval:0x00000000, desc=m_external
";

    fn events(text: &str) -> Result<Vec<TraceEvent>, TraceError> {
        let mut reader = TraceReader::new(text.as_bytes());
        let mut out = Vec::new();
        while let Some(event) = reader.next_event()? {
            out.push(event);
        }
        Ok(out)
    }

    #[test]
    fn a_real_block_and_a_trap_line_are_read() {
        let events = events(FIRST_BLOCK).unwrap();
        assert_eq!(events.len(), 2);
        let TraceEvent::State { state, .. } = events[0] else {
            panic!("not a state: {:?}", events[0]);
        };
        assert_eq!(state.pc(), 0x4000_0000);
        assert_eq!(state.get(Field::Mie), 0xEEE);
        assert_eq!(
            events[1],
            TraceEvent::Trap(QemuTrap {
                asynchronous: true,
                cause: 0xB,
                epc: 0x4038_1BC8,
                tval: 0,
            })
        );
    }

    #[test]
    fn a_rendered_block_reads_back_the_state_it_was_made_from() {
        let mut x = [0u32; 32];
        for (n, value) in x.iter_mut().enumerate().skip(1) {
            *value = 0x1000_0000 + u32::try_from(n).unwrap();
        }
        let mut state = CpuState::new(0x4038_0000, x, |address| Some(0xA000 + u32::from(address)));
        state.set(Field::Mstatus, 0x1888);
        let events = events(&render_block(&state)).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], TraceEvent::State { state: read, .. } if read == state));
    }

    #[test]
    fn identical_blocks_share_a_fingerprint_and_different_ones_do_not() {
        let state = CpuState::new(0x4000_0000, [0; 32], |_| None);
        let mut other = state;
        other.set(Field::X(5), 1);
        let text = format!(
            "{}{}{}",
            render_block(&state),
            render_block(&state),
            render_block(&other)
        );
        let prints: Vec<u64> = events(&text)
            .unwrap()
            .into_iter()
            .map(|event| match event {
                TraceEvent::State { fingerprint, .. } => fingerprint,
                TraceEvent::Trap(_) => 0,
            })
            .collect();
        assert_eq!(prints[0], prints[1]);
        assert_ne!(prints[1], prints[2]);
    }

    #[test]
    fn a_line_longer_than_the_bound_is_refused_before_it_is_read_whole() {
        let text = format!("{}\n", "x".repeat(MAX_LINE_BYTES + 1));
        assert!(matches!(
            events(&text),
            Err(TraceError::LineTooLong { line: 1 })
        ));
        let text = format!("{}\n", "x".repeat(MAX_LINE_BYTES));
        assert_eq!(events(&text).unwrap(), Vec::new());
    }

    /// The lines of `text` that `keep` keeps, each with its end.
    fn lines_where(text: &str, keep: impl Fn(usize, &str) -> bool) -> String {
        let mut out = String::new();
        for (i, line) in text.lines().enumerate() {
            if keep(i, line) {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn a_block_cut_short_or_without_a_quantity_or_too_long_is_refused() {
        let block = render_block(&CpuState::new(0, [0; 32], |_| None));
        let cut = lines_where(&block, |i, _| i < 10);
        assert!(matches!(
            events(&cut),
            Err(TraceError::Truncated { line: 1 })
        ));
        let without_mtvec = lines_where(&block, |_, l| !l.starts_with(" mtvec"));
        assert!(matches!(
            events(&without_mtvec),
            Err(TraceError::Missing { what, .. }) if what == "mtvec"
        ));
        let mut long = String::from(" V      =   0\n");
        for _ in 0..MAX_BLOCK_LINES {
            long.push_str(" satp     00000000\n");
        }
        assert!(matches!(
            events(&long),
            Err(TraceError::BlockTooLong { line: 1 })
        ));
    }

    #[test]
    fn a_register_value_that_is_not_hex_is_refused_with_its_line() {
        let block = render_block(&CpuState::new(0, [0; 32], |_| None))
            .replace(" mepc     00000000", " mepc     0000zz00");
        assert!(matches!(
            events(&block),
            Err(TraceError::Malformed { line: 17, .. })
        ));
    }

    #[test]
    fn other_lines_are_counted_and_skipped() {
        let mut reader = TraceReader::new(FIRST_BLOCK.as_bytes());
        while reader.next_event().unwrap().is_some() {}
        assert_eq!(reader.skipped_lines(), 1, "Adding SPI flash device");
    }
}
