//! Decoded instruction form shared by the 32-bit and compressed decoders.
pub type Reg = u8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AluOp {
    Add,
    Sub,
    Sll,
    Slt,
    Sltu,
    Xor,
    Srl,
    Sra,
    Or,
    And,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulOp {
    Mul,
    Mulh,
    Mulhsu,
    Mulhu,
    Div,
    Divu,
    Rem,
    Remu,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchOp {
    Eq,
    Ne,
    Lt,
    Ge,
    Ltu,
    Geu,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadOp {
    B,
    H,
    W,
    Bu,
    Hu,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOp {
    B,
    H,
    W,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsrOp {
    Rw,
    Rs,
    Rc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instr {
    Lui {
        rd: Reg,
        imm: u32,
    },
    Auipc {
        rd: Reg,
        imm: u32,
    },
    Jal {
        rd: Reg,
        offset: i32,
    },
    Jalr {
        rd: Reg,
        rs1: Reg,
        offset: i32,
    },
    Branch {
        op: BranchOp,
        rs1: Reg,
        rs2: Reg,
        offset: i32,
    },
    Load {
        op: LoadOp,
        rd: Reg,
        rs1: Reg,
        offset: i32,
    },
    Store {
        op: StoreOp,
        rs1: Reg,
        rs2: Reg,
        offset: i32,
    },
    /// Register-immediate ALU op. For shifts `imm` is the shift amount (0..=31). Never `Sub`.
    OpImm {
        op: AluOp,
        rd: Reg,
        rs1: Reg,
        imm: i32,
    },
    Op {
        op: AluOp,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
    },
    MulDiv {
        op: MulOp,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
    },
    /// `src` is a register number, or the 5-bit immediate when `uimm` is true.
    Csr {
        op: CsrOp,
        rd: Reg,
        src: u8,
        uimm: bool,
        csr: u16,
    },
    Fence,
    FenceI,
    Ecall,
    Ebreak,
    Mret,
    Wfi,
    /// The raw parcel (16-bit for compressed, 32-bit otherwise); becomes `mtval`.
    Illegal(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub instr: Instr,
    pub len: u8,
    /// The instruction bits actually fetched (16-bit parcel for compressed); reported as `mtval` on faults.
    pub raw: u32,
}
