//! The architectural state the lockstep compares: the PC, `x1`–`x31` and the machine CSRs both
//! sides have that QEMU's `-d cpu` prints.

use std::fmt;

/// A compared quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    Pc,
    /// An integer register, 1 to 31 (`x0` is 0 on both sides by construction).
    X(u8),
    Mstatus,
    Mie,
    Mip,
    Mtvec,
    Mepc,
    Mcause,
    Mtval,
    Mscratch,
}

/// The machine CSRs compared, with their addresses and the names QEMU prints. QEMU prints more:
/// the supervisor and hypervisor CSRs of its generic core, which the ESP32-C3 does not have,
/// `mhartid` (0 on both sides, a single hart) and `satp`; none of them is compared.
pub const CSRS: [(Field, u16, &str); 8] = [
    (Field::Mstatus, 0x300, "mstatus"),
    (Field::Mie, 0x304, "mie"),
    (Field::Mip, 0x344, "mip"),
    (Field::Mtvec, 0x305, "mtvec"),
    (Field::Mepc, 0x341, "mepc"),
    (Field::Mcause, 0x342, "mcause"),
    (Field::Mtval, 0x343, "mtval"),
    (Field::Mscratch, 0x340, "mscratch"),
];

/// Every compared quantity, in the order a report lists them.
pub const FIELDS: usize = 1 + 31 + CSRS.len();

/// The ABI names of the integer registers, as QEMU prints them.
const ABI: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
    "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
    "t5", "t6",
];

impl Field {
    /// The quantity at place `index` of [`Field::all`].
    #[must_use]
    pub const fn at(index: usize) -> Option<Field> {
        match index {
            0 => Some(Field::Pc),
            // INVARIANT: 1..=31 fits a u8.
            #[allow(clippy::cast_possible_truncation)]
            1..=31 => Some(Field::X(index as u8)),
            _ => match index.checked_sub(32) {
                Some(csr) if csr < CSRS.len() => Some(CSRS[csr].0),
                _ => None,
            },
        }
    }

    /// The place of the quantity in a [`CpuState`].
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Field::Pc => 0,
            Field::X(n) => usize::from(n.clamp(1, 31)),
            csr => 32 + CSRS.iter().position(|(f, _, _)| *f == csr).unwrap_or(0),
        }
    }

    /// Every compared quantity, PC first.
    pub fn all() -> impl Iterator<Item = Field> {
        (0..FIELDS).filter_map(Field::at)
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Field::Pc => f.write_str("pc"),
            Field::X(n) => write!(f, "x{n}/{}", ABI[usize::from(*n) & 31]),
            csr => {
                let name = CSRS
                    .iter()
                    .find(|(field, _, _)| field == csr)
                    .map_or("?", |(_, _, name)| *name);
                f.write_str(name)
            }
        }
    }
}

/// The compared quantities of one side at one instruction boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuState {
    values: [u32; FIELDS],
}

impl CpuState {
    /// A state from its PC, its 32 integer registers (`x0` is ignored) and a reader of CSRs by
    /// address; a CSR the reader does not have reads 0.
    pub fn new(pc: u32, x: [u32; 32], csr: impl Fn(u16) -> Option<u32>) -> CpuState {
        let mut values = [0; FIELDS];
        values[0] = pc;
        values[1..32].copy_from_slice(&x[1..32]);
        for (i, (_, address, _)) in CSRS.iter().enumerate() {
            values[32 + i] = csr(*address).unwrap_or(0);
        }
        CpuState { values }
    }

    /// The value of a quantity.
    #[must_use]
    pub fn get(&self, field: Field) -> u32 {
        self.values[field.index()]
    }

    /// Sets a quantity.
    pub fn set(&mut self, field: Field, value: u32) {
        self.values[field.index()] = value;
    }

    /// The PC.
    #[must_use]
    pub fn pc(&self) -> u32 {
        self.values[0]
    }

    /// Integer register `n` (`x0` reads 0).
    #[must_use]
    pub fn x(&self, n: u8) -> u32 {
        if n == 0 || n > 31 {
            0
        } else {
            self.values[usize::from(n)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_its_own_place_and_comes_back_from_it() {
        let fields: Vec<Field> = Field::all().collect();
        assert_eq!(fields.len(), FIELDS);
        for (i, field) in fields.iter().enumerate() {
            assert_eq!(field.index(), i);
            assert_eq!(Field::at(i), Some(*field));
        }
        assert_eq!(Field::at(FIELDS), None);
    }

    #[test]
    fn a_state_reads_the_csrs_by_address_and_names_registers_as_qemu_does() {
        let mut x = [0u32; 32];
        x[2] = 0x3FC8_0000;
        let state = CpuState::new(0x4000_0000, x, |address| Some(u32::from(address)));
        assert_eq!(state.pc(), 0x4000_0000);
        assert_eq!(state.x(2), 0x3FC8_0000);
        assert_eq!(state.get(Field::Mtvec), 0x305);
        assert_eq!(Field::X(2).to_string(), "x2/sp");
        assert_eq!(Field::Mscratch.to_string(), "mscratch");
    }
}
