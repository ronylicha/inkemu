//! Pluggable timing models: a source of per-instruction-class base costs.

/// Coarse instruction categories that timing models assign costs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InstrClass {
    Alu,
    Mul,
    Div,
    Load,
    Store,
    BranchTaken,
    BranchNotTaken,
    Jump,
    Csr,
    System,
}

/// A source of instruction costs. Memory wait states come from the bus, not from here.
pub trait TimingModel: std::fmt::Debug {
    fn name(&self) -> &'static str;
    /// `false` until the model has been fitted against measurements on real hardware.
    fn is_calibrated(&self) -> bool;
    fn instr_cycles(&self, class: InstrClass) -> u32;
}
