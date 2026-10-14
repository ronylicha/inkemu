//! The lockstep against Espressif's QEMU (spec §4.4, ADR 0008): QEMU's trace and the machine are
//! compared after every instruction, under a numbered table of admitted differences.
//!
//! The crate reads QEMU's trace from any source ([`trace`]), drives a machine through its stepping
//! API and compares ([`lockstep`]), holds the table and the peripherals QEMU instantiates
//! ([`table`]), records what each row did and checks it against the table ([`usage`]) and writes
//! the report of a divergence ([`report`]). It performs no host I/O: the development binary
//! `inkemu-lockstep` starts QEMU and hands the trace over.

pub mod lockstep;
pub mod report;
pub mod state;
pub mod table;
pub mod trace;
pub mod usage;

pub use lockstep::{
    Counts, End, HISTORY, Lockstep, MAX_REPRINTS, MAX_SLEEP_SLICES, RESET_PC, Target, WindowEnd,
    loops_on_itself,
};
pub use report::{Adoption, Cause, HistoryEntry, Nearest, Report};
pub use state::{CpuState, Field};
pub use table::{
    DIFFERENCES, Difference, Drift, Location, QEMU_DEVICES, Register, Site, Treatment,
};
pub use trace::{QemuTrap, TraceError, TraceEvent, TraceReader, render_block};
pub use usage::{Check, Misuse, RowUse, SiteUse, Usage};
