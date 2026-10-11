//! Synchronous exceptions of a machine-mode-only RV32 hart.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exception {
    InstrAccessFault,
    IllegalInstr,
    Breakpoint,
    LoadMisaligned,
    LoadAccessFault,
    StoreMisaligned,
    StoreAccessFault,
    EcallM,
}

impl Exception {
    /// `mcause` exception code (Privileged spec, table "Machine cause register values").
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Exception::InstrAccessFault => 1,
            Exception::IllegalInstr => 2,
            Exception::Breakpoint => 3,
            Exception::LoadMisaligned => 4,
            Exception::LoadAccessFault => 5,
            Exception::StoreMisaligned => 6,
            Exception::StoreAccessFault => 7,
            Exception::EcallM => 11,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trap {
    pub cause: Exception,
    pub tval: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_codes_match_the_privileged_spec() {
        let codes = [
            (Exception::InstrAccessFault, 1),
            (Exception::IllegalInstr, 2),
            (Exception::Breakpoint, 3),
            (Exception::LoadMisaligned, 4),
            (Exception::LoadAccessFault, 5),
            (Exception::StoreMisaligned, 6),
            (Exception::StoreAccessFault, 7),
            (Exception::EcallM, 11),
        ];
        for (exception, code) in codes {
            assert_eq!(exception.code(), code);
        }
    }
}
