//! Initial, uncalibrated instruction costs (spec §4.2). Memory wait states are added by the bus.
//!
//! Espressif publishes no per-instruction latencies for the ESP32-C3 core (spec §1.3). Every
//! value below is an estimate for a single-issue, short-pipeline RISC-V core, and the model
//! reports `is_calibrated() == false`. `CoreMark` holds their average on its own instruction mix
//! within 15 % of the score Espressif publishes (spec §10.3 criterion 5, ADR 0009); it fits none
//! of them. They are fitted only against a device, by the calibration bench of sub-project 8.

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
/// Estimate: hardware multiplier; to be measured on a device (sub-project 8).
const MUL: u32 = 2;
/// Estimate: iterative divider; to be measured on a device (sub-project 8).
const DIV: u32 = 33;
/// Estimate: CSR access; to be measured on a device (sub-project 8).
const CSR: u32 = 1;
/// Estimate: system instructions (`ECALL`, `EBREAK`, `MRET`, `WFI`); to be measured on a device
/// (sub-project 8).
const SYSTEM: u32 = 3;

/// Estimated starting point for instruction timing. Not calibrated.
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

/// The timing of Espressif's QEMU run with `-icount 3`: every instruction is one cycle, whatever
/// its class (QEMU's virtual clock counts instructions, 2^3 ns each, and nothing else). The chip
/// model turns instructions into device time at 8 ns each; this model only says that no class costs
/// more than another. It exists for the lockstep against QEMU (decision H2) and describes QEMU,
/// not a chip: it is not calibrated.
#[derive(Debug, Default, Clone, Copy)]
pub struct QemuIcountTiming;

impl TimingModel for QemuIcountTiming {
    fn name(&self) -> &'static str {
        "qemu-icount"
    }

    fn is_calibrated(&self) -> bool {
        false
    }

    fn instr_cycles(&self, _class: InstrClass) -> u32 {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::{InstrClass, TimingModel};

    #[test]
    fn the_qemu_icount_model_prices_every_class_at_one_cycle() {
        let t = QemuIcountTiming;
        assert_eq!(t.name(), "qemu-icount");
        assert!(!t.is_calibrated());
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
            assert_eq!(t.instr_cycles(c), 1);
        }
    }
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
