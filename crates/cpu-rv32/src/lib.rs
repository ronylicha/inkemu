//! RV32IMC CPU model for inkemu.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod decode;
mod decode_c;
pub mod instr;

pub use decode::decode;
pub use instr::*;
