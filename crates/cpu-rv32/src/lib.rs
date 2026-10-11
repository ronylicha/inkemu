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
mod exec;
pub mod hart;
pub mod trap;

pub use hart::{Hart, HartConfig, MisalignedPolicy, StepOutcome};
pub use trap::{Exception, Trap};
