//! Core emulation traits and shared types for inkemu.
pub mod bus;
pub mod memory;
pub mod peripheral;
pub mod scheduler;
pub mod signals;
pub mod time;
pub mod timing;
pub mod trace;

pub use bus::{AccessKind, AccessSize, Bus, BusError, ReadOutcome};
pub use memory::{PAGE_SHIFT, Ram};
pub use peripheral::Peripheral;
pub use scheduler::{EventId, Scheduler};
pub use signals::{
    AnalogSource, BoardInput, BoardInputError, BoardInputs, Clocked, GpioInput, GpioOutput,
    I2cTarget, Level, Nanos, SpiTarget, Supply,
};
pub use time::{Cycles, Frequency};
pub use timing::{InstrClass, TimingModel};
pub use trace::TraceHash;
