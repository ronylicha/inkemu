//! RV32IMC CPU model for inkemu.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod decode;
mod decode_c;
pub mod instr;

pub use decode::decode;
pub use instr::*;
#[cfg(any(test, feature = "test-support"))]
pub mod asm;
pub mod csr;
pub mod csr_esp32c3;
mod dcache;
mod exec;
pub mod hart;
pub mod timing;
pub mod trap;

pub use csr::VendorCsrs;
pub use hart::{Hart, HartConfig, MisalignedPolicy, StepOutcome};
pub use timing::DatasheetTiming;
pub use trap::{Exception, Trap};
