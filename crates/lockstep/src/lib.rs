//! The lockstep against Espressif's QEMU (spec §4.4, ADR 0008): QEMU's trace and the machine are
//! compared after every instruction, under a numbered table of admitted differences.
//!
//! The crate reads QEMU's trace from any source ([`trace`]), drives a machine through its stepping
//! API and compares ([`lockstep`]), holds the table and the blocks QEMU lacks ([`table`]) and
//! writes the report of a divergence ([`report`]). It performs no host I/O: the development binary
//! `inkemu-lockstep` starts QEMU and hands the trace over.

pub mod lockstep;
pub mod report;
pub mod state;
pub mod table;
pub mod trace;

pub use lockstep::{Counts, End, HISTORY, Lockstep, RESET_PC, Target, WindowEnd};
pub use report::{HistoryEntry, Nearest, Report};
pub use state::{CpuState, Field};
pub use table::{DIFFERENCES, Difference, Treatment, UNEMULATED, Unemulated};
pub use trace::{QemuTrap, TraceError, TraceEvent, TraceReader, render_block};
