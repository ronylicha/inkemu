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

impl std::fmt::Display for Exception {
    /// The exception in the words of the privileged specification (table "Machine cause
    /// register values"), for a message a user reads.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Exception::InstrAccessFault => "instruction access fault",
            Exception::IllegalInstr => "illegal instruction",
            Exception::Breakpoint => "breakpoint",
            Exception::LoadMisaligned => "load address misaligned",
            Exception::LoadAccessFault => "load access fault",
            Exception::StoreMisaligned => "store address misaligned",
            Exception::StoreAccessFault => "store access fault",
            Exception::EcallM => "environment call from machine mode",
        })
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

    #[test]
    fn an_exception_is_displayed_in_words_not_as_an_identifier() {
        let names = [
            (Exception::InstrAccessFault, "instruction access fault"),
            (Exception::IllegalInstr, "illegal instruction"),
            (Exception::Breakpoint, "breakpoint"),
            (Exception::LoadMisaligned, "load address misaligned"),
            (Exception::LoadAccessFault, "load access fault"),
            (Exception::StoreMisaligned, "store address misaligned"),
            (Exception::StoreAccessFault, "store access fault"),
            (Exception::EcallM, "environment call from machine mode"),
        ];
        for (exception, name) in names {
            assert_eq!(exception.to_string(), name);
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == ' '),
                "{name}"
            );
        }
    }
}
