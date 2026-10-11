//! RV32IMC CPU model for inkemu.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod decode;
mod decode_c;
pub mod instr;

pub use decode::decode;
pub use instr::*;
#[cfg(test)]
pub(crate) mod asm;
pub mod csr;
mod dcache;
mod exec;
pub mod hart;
pub mod timing;
pub mod trap;

pub use hart::{Hart, HartConfig, MisalignedPolicy, StepOutcome};
pub use timing::DatasheetTiming;
pub use trap::{Exception, Trap};
