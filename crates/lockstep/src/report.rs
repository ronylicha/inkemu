//! The report of a divergence (decision H8, spec §4.4): the instruction, the state both sides
//! started it from, the two states after it with their differences marked, the instructions
//! before it, and the row of the table that comes nearest to applying.

use crate::state::{CpuState, Field};
use crate::trace::QemuTrap;
use soc_esp32c3::Executed;
use std::fmt;

/// One compared instruction of the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Its place in the comparison, the first instruction being 0.
    pub index: u64,
    pub pc: u32,
    /// The bits of the instruction, `None` when its fetch faulted.
    pub instruction: Option<u32>,
    /// The interrupt line the machine took after it.
    pub interrupt: Option<u32>,
    /// A value copied from QEMU after it: the row, the register and the value.
    pub injected: Option<(u16, u8, u32)>,
}

/// The row of the table that comes nearest to the divergence, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nearest {
    pub row: u16,
    pub why: String,
}

/// A divergence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Instructions compared before the one that diverged.
    pub index: u64,
    /// The instruction the machine executed; `None` for the power-on state.
    pub executed: Option<Executed>,
    /// The interrupt line the machine took after it.
    pub interrupt: Option<u32>,
    /// The trap QEMU took before its state.
    pub qemu_trap: Option<QemuTrap>,
    /// The machine's state before the instruction (the state both sides were found equal in).
    pub before: CpuState,
    /// QEMU's state after it.
    pub qemu: CpuState,
    /// The machine's state after it.
    pub inkemu: CpuState,
    /// The quantities that differ outside the masks of the table.
    pub differing: Vec<Field>,
    /// The instructions before it, oldest first, at most [`crate::HISTORY`].
    pub history: Vec<HistoryEntry>,
    pub nearest: Option<Nearest>,
}

fn instruction(raw: Option<u32>) -> String {
    match raw {
        None => "(fetch faulted)".to_owned(),
        Some(raw) if raw & 0b11 != 0b11 => format!("{raw:04x}    "),
        Some(raw) => format!("{raw:08x}"),
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "divergence after {} compared instructions", self.index)?;
        match &self.executed {
            None => writeln!(f, "at power-on, before the first instruction")?,
            Some(executed) => {
                write!(
                    f,
                    "instruction {} at {:#010x}",
                    instruction(executed.instruction.map(|d| d.raw)),
                    executed.pc
                )?;
                if let Some(decoded) = executed.instruction {
                    write!(f, " ({:?})", decoded.instr)?;
                }
                if let Some(trap) = executed.trap {
                    write!(f, ", raised {} (tval {:#010x})", trap.cause, trap.tval)?;
                }
                writeln!(f)?;
            }
        }
        match self.interrupt {
            Some(line) => writeln!(f, "inkemu then took interrupt line {line}")?,
            None => writeln!(f, "inkemu took no interrupt after it")?,
        }
        match self.qemu_trap {
            Some(trap) => writeln!(
                f,
                "QEMU then took {} cause {} (epc {:#010x}, tval {:#010x})",
                if trap.asynchronous {
                    "interrupt"
                } else {
                    "exception"
                },
                trap.cause,
                trap.epc,
                trap.tval
            )?,
            None => writeln!(f, "QEMU took no trap after it")?,
        }
        writeln!(f)?;
        writeln!(
            f,
            "{:<14} {:>10} {:>10} {:>10}",
            "", "before", "QEMU", "inkemu"
        )?;
        for field in Field::all() {
            let marker = if self.differing.contains(&field) {
                "  <<"
            } else if self.qemu.get(field) != self.inkemu.get(field) {
                "  (masked)"
            } else {
                ""
            };
            writeln!(
                f,
                "{:<14} {:08x}   {:08x}   {:08x}{marker}",
                field.to_string(),
                self.before.get(field),
                self.qemu.get(field),
                self.inkemu.get(field)
            )?;
        }
        writeln!(f)?;
        writeln!(
            f,
            "the {} instructions before it, the same on both sides:",
            self.history.len()
        )?;
        for entry in &self.history {
            write!(
                f,
                "  #{:<9} {:#010x}  {}",
                entry.index,
                entry.pc,
                instruction(entry.instruction)
            )?;
            if let Some(line) = entry.interrupt {
                write!(f, "  then interrupt line {line}")?;
            }
            if let Some((row, reg, value)) = entry.injected {
                write!(f, "  x{reg} := {value:#010x} (row {row})")?;
            }
            writeln!(f)?;
        }
        match &self.nearest {
            Some(nearest) => writeln!(
                f,
                "nearest row of docs/lockstep-differences.md: {} ({})",
                nearest.row, nearest.why
            ),
            None => writeln!(f, "no row of docs/lockstep-differences.md comes near"),
        }
    }
}
