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
    // Encodings: GNU as, binutils 2.42-1ubuntu1+6 (riscv64-unknown-elf,
    // -march=rv32imc_zicsr_zifencei -mabi=ilp32); see task-4-report.md for the objdump listing.
    use super::*;
    use crate::instr::*;

    fn d(raw: u32) -> Instr {
        let x = decode(raw);
        assert_eq!(x.len, 4);
        x.instr
    }

    fn check(cases: &[(u32, Instr)]) {
        for &(raw, expected) in cases {
            let got = decode(raw);
            assert_eq!(got.len, 4, "{raw:#010x}");
            assert_eq!(got.raw, raw, "{raw:#010x}");
            assert_eq!(got.instr, expected, "{raw:#010x}");
        }
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

    #[test]
    fn decodes_auipc_and_jalr() {
        check(&[
            (
                0x1234_5297,
                Instr::Auipc {
                    rd: 5,
                    imm: 0x1234_5000,
                },
            ),
            (
                0xffff_ff97,
                Instr::Auipc {
                    rd: 31,
                    imm: 0xffff_f000,
                },
            ),
            (
                0x0081_00e7,
                Instr::Jalr {
                    rd: 1,
                    rs1: 2,
                    offset: 8,
                },
            ),
            (
                0x8000_8067,
                Instr::Jalr {
                    rd: 0,
                    rs1: 1,
                    offset: -2048,
                },
            ),
            (
                0x7fff_03e7,
                Instr::Jalr {
                    rd: 7,
                    rs1: 30,
                    offset: 2047,
                },
            ),
        ]);
    }

    #[test]
    fn branch_immediates_cover_extremes_and_asymmetric_values() {
        check(&[
            (
                0x7e20_8fe3,
                Instr::Branch {
                    op: BranchOp::Eq,
                    rs1: 1,
                    rs2: 2,
                    offset: 4094,
                },
            ),
            (
                0x8020_8063,
                Instr::Branch {
                    op: BranchOp::Eq,
                    rs1: 1,
                    rs2: 2,
                    offset: -4096,
                },
            ),
            (
                0x7e41_9fe3,
                Instr::Branch {
                    op: BranchOp::Ne,
                    rs1: 3,
                    rs2: 4,
                    offset: 4094,
                },
            ),
            (
                0x8041_9063,
                Instr::Branch {
                    op: BranchOp::Ne,
                    rs1: 3,
                    rs2: 4,
                    offset: -4096,
                },
            ),
            (
                0x5a62_c163,
                Instr::Branch {
                    op: BranchOp::Lt,
                    rs1: 5,
                    rs2: 6,
                    offset: 0x5a2,
                },
            ),
            (
                0xa462_cee3,
                Instr::Branch {
                    op: BranchOp::Lt,
                    rs1: 5,
                    rs2: 6,
                    offset: -0x5a4,
                },
            ),
            (
                0x0e83_df63,
                Instr::Branch {
                    op: BranchOp::Ge,
                    rs1: 7,
                    rs2: 8,
                    offset: 254,
                },
            ),
            (
                0xf083_d0e3,
                Instr::Branch {
                    op: BranchOp::Ge,
                    rs1: 7,
                    rs2: 8,
                    offset: -256,
                },
            ),
            (
                0x02a4_ee63,
                Instr::Branch {
                    op: BranchOp::Ltu,
                    rs1: 9,
                    rs2: 10,
                    offset: 0x3c,
                },
            ),
            (
                0xfca4_e0e3,
                Instr::Branch {
                    op: BranchOp::Ltu,
                    rs1: 9,
                    rs2: 10,
                    offset: -0x40,
                },
            ),
        ]);
    }

    #[test]
    fn branch_immediates_geu_and_remaining_offsets() {
        check(&[
            (
                0x00c5_f263,
                Instr::Branch {
                    op: BranchOp::Geu,
                    rs1: 11,
                    rs2: 12,
                    offset: 4,
                },
            ),
            (
                0x80c5_f163,
                Instr::Branch {
                    op: BranchOp::Geu,
                    rs1: 11,
                    rs2: 12,
                    offset: -4094,
                },
            ),
        ]);
    }

    #[test]
    fn jal_immediates_cover_extremes_and_asymmetric_values() {
        check(&[
            (
                0x7fff_f0ef,
                Instr::Jal {
                    rd: 1,
                    offset: 1_048_574,
                },
            ),
            (
                0x8000_006f,
                Instr::Jal {
                    rd: 0,
                    offset: -1_048_576,
                },
            ),
            (
                0x5a40_04ef,
                Instr::Jal {
                    rd: 9,
                    offset: 0x5a4,
                },
            ),
        ]);
    }

    #[test]
    fn decodes_loads_and_stores_of_every_width() {
        check(&[
            (
                0x8000_8283,
                Instr::Load {
                    op: LoadOp::B,
                    rd: 5,
                    rs1: 1,
                    offset: -2048,
                },
            ),
            (
                0x7ff0_8283,
                Instr::Load {
                    op: LoadOp::B,
                    rd: 5,
                    rs1: 1,
                    offset: 2047,
                },
            ),
            (
                0x5a41_1303,
                Instr::Load {
                    op: LoadOp::H,
                    rd: 6,
                    rs1: 2,
                    offset: 1444,
                },
            ),
            (
                0xfff1_a383,
                Instr::Load {
                    op: LoadOp::W,
                    rd: 7,
                    rs1: 3,
                    offset: -1,
                },
            ),
            (
                0x05a2_4403,
                Instr::Load {
                    op: LoadOp::Bu,
                    rd: 8,
                    rs1: 4,
                    offset: 90,
                },
            ),
            (
                0xfa62_d483,
                Instr::Load {
                    op: LoadOp::Hu,
                    rd: 9,
                    rs1: 5,
                    offset: -90,
                },
            ),
            (
                0x8050_8023,
                Instr::Store {
                    op: StoreOp::B,
                    rs1: 1,
                    rs2: 5,
                    offset: -2048,
                },
            ),
            (
                0x7e50_8fa3,
                Instr::Store {
                    op: StoreOp::B,
                    rs1: 1,
                    rs2: 5,
                    offset: 2047,
                },
            ),
            (
                0x5a61_1223,
                Instr::Store {
                    op: StoreOp::H,
                    rs1: 2,
                    rs2: 6,
                    offset: 1444,
                },
            ),
            (
                0xfa61_1323,
                Instr::Store {
                    op: StoreOp::H,
                    rs1: 2,
                    rs2: 6,
                    offset: -90,
                },
            ),
        ]);
    }

    #[test]
    fn stores_word_and_halfword_offsets() {
        check(&[(
            0x5a71_a223,
            Instr::Store {
                op: StoreOp::W,
                rs1: 3,
                rs2: 7,
                offset: 1444,
            },
        )]);
    }

    #[test]
    fn decodes_op_imm_variants() {
        check(&[
            (
                0x8003_2293,
                Instr::OpImm {
                    op: AluOp::Slt,
                    rd: 5,
                    rs1: 6,
                    imm: -2048,
                },
            ),
            (
                0x7ff3_3293,
                Instr::OpImm {
                    op: AluOp::Sltu,
                    rd: 5,
                    rs1: 6,
                    imm: 2047,
                },
            ),
            (
                0x5a43_4293,
                Instr::OpImm {
                    op: AluOp::Xor,
                    rd: 5,
                    rs1: 6,
                    imm: 1444,
                },
            ),
            (
                0xfff3_6293,
                Instr::OpImm {
                    op: AluOp::Or,
                    rd: 5,
                    rs1: 6,
                    imm: -1,
                },
            ),
            (
                0x05a3_7293,
                Instr::OpImm {
                    op: AluOp::And,
                    rd: 5,
                    rs1: 6,
                    imm: 90,
                },
            ),
            (
                0x01f3_1293,
                Instr::OpImm {
                    op: AluOp::Sll,
                    rd: 5,
                    rs1: 6,
                    imm: 31,
                },
            ),
            (
                0x0113_5293,
                Instr::OpImm {
                    op: AluOp::Srl,
                    rd: 5,
                    rs1: 6,
                    imm: 17,
                },
            ),
            (
                0x4033_5293,
                Instr::OpImm {
                    op: AluOp::Sra,
                    rd: 5,
                    rs1: 6,
                    imm: 3,
                },
            ),
        ]);
    }

    #[test]
    fn decodes_op_register_variants() {
        check(&[
            (
                0x0073_12b3,
                Instr::Op {
                    op: AluOp::Sll,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0073_22b3,
                Instr::Op {
                    op: AluOp::Slt,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0073_32b3,
                Instr::Op {
                    op: AluOp::Sltu,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0073_42b3,
                Instr::Op {
                    op: AluOp::Xor,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0073_52b3,
                Instr::Op {
                    op: AluOp::Srl,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x4073_52b3,
                Instr::Op {
                    op: AluOp::Sra,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0073_62b3,
                Instr::Op {
                    op: AluOp::Or,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0073_72b3,
                Instr::Op {
                    op: AluOp::And,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
        ]);
    }

    #[test]
    fn decodes_all_m_extension_ops() {
        check(&[
            (
                0x0273_02b3,
                Instr::MulDiv {
                    op: MulOp::Mul,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0273_12b3,
                Instr::MulDiv {
                    op: MulOp::Mulh,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0273_22b3,
                Instr::MulDiv {
                    op: MulOp::Mulhsu,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0273_32b3,
                Instr::MulDiv {
                    op: MulOp::Mulhu,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0273_42b3,
                Instr::MulDiv {
                    op: MulOp::Div,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0273_52b3,
                Instr::MulDiv {
                    op: MulOp::Divu,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0273_62b3,
                Instr::MulDiv {
                    op: MulOp::Rem,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
            (
                0x0273_72b3,
                Instr::MulDiv {
                    op: MulOp::Remu,
                    rd: 5,
                    rs1: 6,
                    rs2: 7,
                },
            ),
        ]);
    }

    #[test]
    fn decodes_fences_and_csr_forms() {
        check(&[
            (0x0ff0_000f, Instr::Fence),
            (0x0000_100f, Instr::FenceI),
            (
                0x3001_20f3,
                Instr::Csr {
                    op: CsrOp::Rs,
                    rd: 1,
                    src: 2,
                    uimm: false,
                    csr: 0x300,
                },
            ),
            (
                0x3412_31f3,
                Instr::Csr {
                    op: CsrOp::Rc,
                    rd: 3,
                    src: 4,
                    uimm: false,
                    csr: 0x341,
                },
            ),
            (
                0x3002_d0f3,
                Instr::Csr {
                    op: CsrOp::Rw,
                    rd: 1,
                    src: 5,
                    uimm: true,
                    csr: 0x300,
                },
            ),
            (
                0x3058_e173,
                Instr::Csr {
                    op: CsrOp::Rs,
                    rd: 2,
                    src: 17,
                    uimm: true,
                    csr: 0x305,
                },
            ),
            (
                0x344f_f1f3,
                Instr::Csr {
                    op: CsrOp::Rc,
                    rd: 3,
                    src: 31,
                    uimm: true,
                    csr: 0x344,
                },
            ),
        ]);
    }

    #[test]
    fn reserved_32bit_encodings_trap_with_raw_word() {
        for raw in [
            0x0000_3023, // store funct3=3
            0x0000_200f,
            0x0000_300f,
            0x0000_400f,
            0x0000_500f,
            0x0000_600f,
            0x0000_700f, // misc-mem funct3=2..7
            0x4073_12b3, // OP funct3=1 with funct7=0x20 (only sub/sra take 0x20)
            0x0473_12b3, // OP funct3=1 with funct7=0x02 (unknown funct7)
            0x4000_1293, // OP-IMM funct3=1 (slli) with funct7=0x20
            0x0400_5293, // OP-IMM funct3=5 with funct7=0x02
            0x0000_1067, // JALR funct3=1
            0x0000_6003, // load funct3=6
            0x0000_7003, // load funct3=7
            0x1200_0073, // SFENCE.VMA: funct3=0 system word that is not ecall/ebreak/mret/wfi
        ] {
            assert_eq!(d(raw), Instr::Illegal(raw), "{raw:#010x}");
        }
    }
}
