use crate::decode_c::decode_compressed;
use crate::instr::{AluOp, BranchOp, CsrOp, Decoded, Instr, LoadOp, MulOp, Reg, StoreOp};

const OP_LUI: u32 = 0x37;
const OP_AUIPC: u32 = 0x17;
const OP_JAL: u32 = 0x6f;
const OP_JALR: u32 = 0x67;
const OP_BRANCH: u32 = 0x63;
const OP_LOAD: u32 = 0x03;
const OP_STORE: u32 = 0x23;
const OP_IMM: u32 = 0x13;
const OP_REG: u32 = 0x33;
const OP_MISC_MEM: u32 = 0x0f;
const OP_SYSTEM: u32 = 0x73;

const FUNCT7_BASE: u32 = 0x00;
const FUNCT7_ALT: u32 = 0x20;
const FUNCT7_MULDIV: u32 = 0x01;

const ENC_ECALL: u32 = 0x0000_0073;
const ENC_EBREAK: u32 = 0x0010_0073;
const ENC_MRET: u32 = 0x3020_0073;
const ENC_WFI: u32 = 0x1050_0073;

const COMPRESSED_MASK: u32 = 0b11;

#[must_use]
pub fn decode(raw: u32) -> Decoded {
    if raw & COMPRESSED_MASK != COMPRESSED_MASK {
        // INVARIANT: the mask keeps exactly 16 bits.
        #[allow(clippy::cast_possible_truncation)]
        let parcel = (raw & 0xffff) as u16;
        return Decoded {
            instr: decode_compressed(parcel),
            len: 2,
            raw: u32::from(parcel),
        };
    }
    Decoded {
        instr: decode32(raw),
        len: 4,
        raw,
    }
}

#[allow(clippy::cast_possible_truncation)] // every register field is masked to 5 bits
fn reg(raw: u32, shift: u32) -> Reg {
    ((raw >> shift) & 0x1f) as Reg
}

#[allow(clippy::cast_possible_wrap)]
fn imm_i(raw: u32) -> i32 {
    (raw as i32) >> 20
}
#[allow(clippy::cast_possible_wrap)]
fn imm_s(raw: u32) -> i32 {
    ((raw as i32) >> 25 << 5) | ((raw >> 7) & 0x1f) as i32
}
#[allow(clippy::cast_possible_wrap)]
fn imm_b(raw: u32) -> i32 {
    ((raw as i32) >> 31 << 12)
        | (((raw >> 7) & 1) << 11) as i32
        | (((raw >> 25) & 0x3f) << 5) as i32
        | (((raw >> 8) & 0xf) << 1) as i32
}
#[allow(clippy::cast_possible_wrap)]
fn imm_j(raw: u32) -> i32 {
    ((raw as i32) >> 31 << 20)
        | (raw & 0x000f_f000) as i32
        | (((raw >> 20) & 1) << 11) as i32
        | (((raw >> 21) & 0x3ff) << 1) as i32
}

fn decode32(raw: u32) -> Instr {
    let (rd, rs1, rs2) = (reg(raw, 7), reg(raw, 15), reg(raw, 20));
    let funct3 = (raw >> 12) & 0x7;
    let funct7 = raw >> 25;
    let illegal = Instr::Illegal(raw);
    match raw & 0x7f {
        OP_LUI => Instr::Lui {
            rd,
            imm: raw & 0xffff_f000,
        },
        OP_AUIPC => Instr::Auipc {
            rd,
            imm: raw & 0xffff_f000,
        },
        OP_JAL => Instr::Jal {
            rd,
            offset: imm_j(raw),
        },
        OP_JALR if funct3 == 0 => Instr::Jalr {
            rd,
            rs1,
            offset: imm_i(raw),
        },
        OP_BRANCH => {
            let op = match funct3 {
                0 => BranchOp::Eq,
                1 => BranchOp::Ne,
                4 => BranchOp::Lt,
                5 => BranchOp::Ge,
                6 => BranchOp::Ltu,
                7 => BranchOp::Geu,
                _ => return illegal,
            };
            Instr::Branch {
                op,
                rs1,
                rs2,
                offset: imm_b(raw),
            }
        }
        OP_LOAD => {
            let op = match funct3 {
                0 => LoadOp::B,
                1 => LoadOp::H,
                2 => LoadOp::W,
                4 => LoadOp::Bu,
                5 => LoadOp::Hu,
                _ => return illegal,
            };
            Instr::Load {
                op,
                rd,
                rs1,
                offset: imm_i(raw),
            }
        }
        OP_STORE => {
            let op = match funct3 {
                0 => StoreOp::B,
                1 => StoreOp::H,
                2 => StoreOp::W,
                _ => return illegal,
            };
            Instr::Store {
                op,
                rs1,
                rs2,
                offset: imm_s(raw),
            }
        }
        OP_IMM => decode_op_imm(raw, rd, rs1, funct3, funct7),
        OP_REG => decode_op_reg(raw, rd, rs1, rs2, funct3, funct7),
        OP_MISC_MEM => match funct3 {
            0 => Instr::Fence,
            1 => Instr::FenceI,
            _ => illegal,
        },
        OP_SYSTEM => decode_system(raw, rd, rs1, funct3),
        _ => illegal,
    }
}

fn decode_op_imm(raw: u32, rd: Reg, rs1: Reg, funct3: u32, funct7: u32) -> Instr {
    let shamt = i32::from(reg(raw, 20));
    let op = match (funct3, funct7) {
        (0, _) => AluOp::Add,
        (2, _) => AluOp::Slt,
        (3, _) => AluOp::Sltu,
        (4, _) => AluOp::Xor,
        (6, _) => AluOp::Or,
        (7, _) => AluOp::And,
        (1, FUNCT7_BASE) => {
            return Instr::OpImm {
                op: AluOp::Sll,
                rd,
                rs1,
                imm: shamt,
            };
        }
        (5, FUNCT7_BASE) => {
            return Instr::OpImm {
                op: AluOp::Srl,
                rd,
                rs1,
                imm: shamt,
            };
        }
        (5, FUNCT7_ALT) => {
            return Instr::OpImm {
                op: AluOp::Sra,
                rd,
                rs1,
                imm: shamt,
            };
        }
        _ => return Instr::Illegal(raw),
    };
    Instr::OpImm {
        op,
        rd,
        rs1,
        imm: imm_i(raw),
    }
}

fn decode_op_reg(raw: u32, rd: Reg, rs1: Reg, rs2: Reg, funct3: u32, funct7: u32) -> Instr {
    if funct7 == FUNCT7_MULDIV {
        let op = [
            MulOp::Mul,
            MulOp::Mulh,
            MulOp::Mulhsu,
            MulOp::Mulhu,
            MulOp::Div,
            MulOp::Divu,
            MulOp::Rem,
            MulOp::Remu,
        ][funct3 as usize];
        return Instr::MulDiv { op, rd, rs1, rs2 };
    }
    let op = match (funct3, funct7) {
        (0, FUNCT7_BASE) => AluOp::Add,
        (0, FUNCT7_ALT) => AluOp::Sub,
        (1, FUNCT7_BASE) => AluOp::Sll,
        (2, FUNCT7_BASE) => AluOp::Slt,
        (3, FUNCT7_BASE) => AluOp::Sltu,
        (4, FUNCT7_BASE) => AluOp::Xor,
        (5, FUNCT7_BASE) => AluOp::Srl,
        (5, FUNCT7_ALT) => AluOp::Sra,
        (6, FUNCT7_BASE) => AluOp::Or,
        (7, FUNCT7_BASE) => AluOp::And,
        _ => return Instr::Illegal(raw),
    };
    Instr::Op { op, rd, rs1, rs2 }
}

fn decode_system(raw: u32, rd: Reg, rs1: Reg, funct3: u32) -> Instr {
    // INVARIANT: the CSR address field is 12 bits.
    #[allow(clippy::cast_possible_truncation)]
    let csr = (raw >> 20) as u16;
    let csr_instr = |op, uimm| Instr::Csr {
        op,
        rd,
        src: rs1,
        uimm,
        csr,
    };
    match funct3 {
        0 => match raw {
            ENC_ECALL => Instr::Ecall,
            ENC_EBREAK => Instr::Ebreak,
            ENC_MRET => Instr::Mret,
            ENC_WFI => Instr::Wfi,
            _ => Instr::Illegal(raw),
        },
        1 => csr_instr(CsrOp::Rw, false),
        2 => csr_instr(CsrOp::Rs, false),
        3 => csr_instr(CsrOp::Rc, false),
        5 => csr_instr(CsrOp::Rw, true),
        6 => csr_instr(CsrOp::Rs, true),
        7 => csr_instr(CsrOp::Rc, true),
        _ => Instr::Illegal(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instr::*;
    fn d(raw: u32) -> Instr {
        let x = decode(raw);
        assert_eq!(x.len, 4);
        x.instr
    }

    #[test]
    fn decodes_base_integer_instructions() {
        assert_eq!(
            d(0x0050_0093),
            Instr::OpImm {
                op: AluOp::Add,
                rd: 1,
                rs1: 0,
                imm: 5
            }
        );
        assert_eq!(
            d(0x0020_81b3),
            Instr::Op {
                op: AluOp::Add,
                rd: 3,
                rs1: 1,
                rs2: 2
            }
        );
        assert_eq!(
            d(0x4020_81b3),
            Instr::Op {
                op: AluOp::Sub,
                rd: 3,
                rs1: 1,
                rs2: 2
            }
        );
        assert_eq!(
            d(0x1234_52b7),
            Instr::Lui {
                rd: 5,
                imm: 0x1234_5000
            }
        );
        assert_eq!(d(0xffdf_f0ef), Instr::Jal { rd: 1, offset: -4 });
        assert_eq!(
            d(0x0020_8463),
            Instr::Branch {
                op: BranchOp::Eq,
                rs1: 1,
                rs2: 2,
                offset: 8
            }
        );
        assert_eq!(
            d(0x0081_2283),
            Instr::Load {
                op: LoadOp::W,
                rd: 5,
                rs1: 2,
                offset: 8
            }
        );
        assert_eq!(
            d(0x0051_2623),
            Instr::Store {
                op: StoreOp::W,
                rs1: 2,
                rs2: 5,
                offset: 12
            }
        );
        assert_eq!(
            d(0x4030_d093),
            Instr::OpImm {
                op: AluOp::Sra,
                rd: 1,
                rs1: 1,
                imm: 3
            }
        );
    }

    #[test]
    fn decodes_m_extension() {
        assert_eq!(
            d(0x0220_81b3),
            Instr::MulDiv {
                op: MulOp::Mul,
                rd: 3,
                rs1: 1,
                rs2: 2
            }
        );
        assert_eq!(
            d(0x0220_d1b3),
            Instr::MulDiv {
                op: MulOp::Divu,
                rd: 3,
                rs1: 1,
                rs2: 2
            }
        );
    }

    #[test]
    fn decodes_system_instructions() {
        assert_eq!(d(0x0000_0073), Instr::Ecall);
        assert_eq!(d(0x0010_0073), Instr::Ebreak);
        assert_eq!(d(0x3020_0073), Instr::Mret);
        assert_eq!(d(0x1050_0073), Instr::Wfi);
        assert_eq!(
            d(0x3001_10f3),
            Instr::Csr {
                op: CsrOp::Rw,
                rd: 1,
                src: 2,
                uimm: false,
                csr: 0x300
            }
        );
        assert_eq!(d(0x0000_000f), Instr::Fence);
        assert_eq!(d(0x0000_100f), Instr::FenceI);
    }

    #[test]
    fn illegal_encodings_trap() {
        for raw in [
            0xFFFF_FFFF,
            0x0200_9093, /* slli with shamt[5]=1: reserved on RV32 */
            0x0000_2063, /* branch funct3=2 */
            0x0000_3003, /* load funct3=3 */
            0x0000_4073, /* system funct3=4 */
            0x0000_007F, /* reserved opcode */
        ] {
            assert_eq!(d(raw), Instr::Illegal(raw), "{raw:#010x}");
        }
    }
}
