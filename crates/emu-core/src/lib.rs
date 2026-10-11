//! Core emulation traits and shared types for inkemu.
pub mod bus;
pub mod memory;
pub mod peripheral;
pub mod scheduler;
pub mod time;
pub mod timing;
pub mod trace;

pub use bus::{AccessKind, AccessSize, Bus, BusError, ReadOutcome};
pub use memory::{PAGE_SHIFT, Ram};
pub use peripheral::Peripheral;
pub use scheduler::{EventId, Scheduler};
pub use time::{Cycles, Frequency, VirtualClock};
pub use timing::{InstrClass, TimingModel};
pub use trace::TraceHash;
