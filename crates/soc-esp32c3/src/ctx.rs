//! What a peripheral reaches beyond its own registers: the one door between a device model and the
//! rest of the machine (ADR 0003).
use crate::diag::{Diagnostic, Diagnostics, Unsupported};

/// The context of a peripheral access. The bus owns it and hands it to every register read and
/// write; the machine reads what the peripherals left in it after each instruction.
#[derive(Debug)]
pub struct Ctx {
    /// What the run has to tell the user. Reached from outside the crate through the machine.
    pub(crate) diag: Diagnostics,
    /// PC of the instruction being executed, for diagnostics.
    pc: u32,
    /// Every refusal in the order it was reported, for the tests of the device models: the
    /// diagnostics list a repeated refusal once, and a device test wants to see each one.
    #[cfg(test)]
    refused: std::collections::VecDeque<Unsupported>,
}

impl Ctx {
    #[must_use]
    pub fn new(strict: bool) -> Ctx {
        Ctx {
            diag: Diagnostics::new(strict),
            pc: 0,
            #[cfg(test)]
            refused: std::collections::VecDeque::new(),
        }
    }

    /// Tells the context which instruction is running, so diagnostics can name it.
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
    }

    /// PC of the instruction being executed.
    #[must_use]
    pub const fn pc(&self) -> u32 {
        self.pc
    }

    /// Reports an operation the device cannot perform faithfully: it becomes a diagnostic that
    /// names the instruction, and `--strict-mmio` stops on it.
    pub fn unsupported(&mut self, what: Unsupported) {
        #[cfg(test)]
        self.refused.push_back(what);
        self.diag
            .record(Diagnostic::Unsupported { what, pc: self.pc });
    }

    /// The oldest refusal a device reported and no test took yet.
    #[cfg(test)]
    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.refused.pop_front()
    }
}

impl Default for Ctx {
    /// A context that stops on nothing: what a test of one device needs.
    fn default() -> Ctx {
        Ctx::new(false)
    }
}
