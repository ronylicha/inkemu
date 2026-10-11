//! Initial, uncalibrated instruction costs (spec §4.2). Memory wait states are added by the bus.
//!
//! Espressif publishes no per-instruction latencies for the ESP32-C3 core (spec §1.3). Every
//! value below is an estimate for a single-issue, short-pipeline RISC-V core. They are fitted
//! against `CoreMark` in `SP1b`; until then the model reports `is_calibrated() == false`.

use emu_core::{InstrClass, TimingModel};

/// Estimate: single-cycle ALU operations on a short pipeline.
const ALU: u32 = 1;
/// Estimate: loads take one pipeline cycle; SRAM/flash wait states are added by the bus.
const LOAD: u32 = 1;
/// Estimate: stores take one pipeline cycle; bus wait states are added by the bus.
const STORE: u32 = 1;
/// Estimate: a taken branch refills the front of the pipeline (2-cycle penalty).
const BRANCH_TAKEN: u32 = 3;
/// Estimate: a not-taken branch falls through in one cycle.
const BRANCH_NOT_TAKEN: u32 = 1;
/// Estimate: unconditional jumps refill the pipeline like a taken branch.
const JUMP: u32 = 3;
/// Estimate: hardware multiplier, pending `CoreMark` fitting in `SP1b`.
const MUL: u32 = 2;
/// Estimate: iterative divider, pending `CoreMark` fitting in `SP1b`.
const DIV: u32 = 33;
/// Estimate: CSR access, pending `CoreMark` fitting in `SP1b`.
const CSR: u32 = 1;
/// Estimate: system instructions (`ECALL`, `EBREAK`, `MRET`, `WFI`), pending `CoreMark` fitting in `SP1b`.
const SYSTEM: u32 = 3;

/// Datasheet-derived starting point for instruction timing. Not calibrated.
#[derive(Debug, Default, Clone, Copy)]
pub struct DatasheetTiming;

impl TimingModel for DatasheetTiming {
    fn name(&self) -> &'static str {
        "datasheet-uncalibrated"
    }

    fn is_calibrated(&self) -> bool {
        false
    }

    fn instr_cycles(&self, class: InstrClass) -> u32 {
        match class {
            InstrClass::Alu => ALU,
            InstrClass::Load => LOAD,
            InstrClass::Store => STORE,
            InstrClass::BranchTaken => BRANCH_TAKEN,
            InstrClass::BranchNotTaken => BRANCH_NOT_TAKEN,
            InstrClass::Jump => JUMP,
            InstrClass::Mul => MUL,
            InstrClass::Div => DIV,
            InstrClass::Csr => CSR,
            InstrClass::System => SYSTEM,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::{InstrClass, TimingModel};
    // `default()` is the required constructor (Task 7 interface); clippy would prefer the unit literal.
    #[allow(clippy::default_constructed_unit_structs)]
    #[test]
    fn model_is_explicitly_uncalibrated_and_plausible() {
        let t = DatasheetTiming::default();
        assert_eq!(t.name(), "datasheet-uncalibrated");
        assert!(!t.is_calibrated());
        assert!(
            t.instr_cycles(InstrClass::BranchTaken) > t.instr_cycles(InstrClass::BranchNotTaken)
        );
        assert!(t.instr_cycles(InstrClass::Div) > t.instr_cycles(InstrClass::Mul));
        for c in [
            InstrClass::Alu,
            InstrClass::Mul,
            InstrClass::Div,
            InstrClass::Load,
            InstrClass::Store,
            InstrClass::BranchTaken,
            InstrClass::BranchNotTaken,
            InstrClass::Jump,
            InstrClass::Csr,
            InstrClass::System,
        ] {
            assert!(t.instr_cycles(c) >= 1);
        }
    }
}
