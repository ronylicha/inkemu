use crate::instr::{AluOp, BranchOp, Instr, LoadOp, Reg, StoreOp};

const REG_ZERO: Reg = 0;
const REG_RA: Reg = 1;
const REG_SP: Reg = 2;
const PRIME_REG_BASE: Reg = 8;

#[allow(clippy::cast_possible_truncation)] // fields are masked before the cast
fn full_reg(h: u16, shift: u16) -> Reg {
    ((h >> shift) & 0x1f) as Reg
}
#[allow(clippy::cast_possible_truncation)] // fields are masked before the cast
fn prime_reg(h: u16, shift: u16) -> Reg {
    ((h >> shift) & 0x7) as Reg + PRIME_REG_BASE
}

/// Sign-extends the low `bits` bits of `value`.
#[allow(clippy::cast_possible_wrap)]
fn sext(value: u32, bits: u32) -> i32 {
    ((value << (32 - bits)) as i32) >> (32 - bits)
}

/// 6-bit immediate of CI-format instructions: imm[5] = bit 12, imm[4:0] = bits 6:2.
fn ci_imm(h: u16) -> i32 {
    let h = u32::from(h);
    sext(((h >> 7) & 0x20) | ((h >> 2) & 0x1f), 6)
}
/// Shift amount of c.slli/c.srli/c.srai; `None` when shamt[5] is set (reserved on RV32).
fn ci_shamt(h: u16) -> Option<i32> {
    if h & 0x1000 != 0 {
        None
    } else {
        Some(i32::from((h >> 2) & 0x1f))
    }
}

fn cj_offset(h: u16) -> i32 {
    let h = u32::from(h);
    let v = ((h >> 1) & 0x800)
        | ((h >> 7) & 0x10)
        | ((h >> 1) & 0x300)
        | ((h << 2) & 0x400)
        | ((h >> 1) & 0x40)
        | ((h << 1) & 0x80)
        | ((h >> 2) & 0xe)
        | ((h << 3) & 0x20);
    sext(v, 12)
}

fn cb_offset(h: u16) -> i32 {
    let h = u32::from(h);
    let v = ((h >> 4) & 0x100)
        | ((h >> 7) & 0x18)
        | ((h << 1) & 0xc0)
        | ((h >> 2) & 0x6)
        | ((h << 3) & 0x20);
    sext(v, 9)
}

#[must_use]
pub(crate) fn decode_compressed(h: u16) -> Instr {
    if h == 0 {
        return Instr::Illegal(u32::from(h));
    }
    let funct3 = (h >> 13) & 0x7;
    match h & 0x3 {
        0 => decode_quadrant0(h, funct3),
        1 => decode_quadrant1(h, funct3),
        2 => decode_quadrant2(h, funct3),
        _ => Instr::Illegal(u32::from(h)),
    }
}

fn decode_quadrant0(h: u16, funct3: u16) -> Instr {
    let illegal = Instr::Illegal(u32::from(h));
    match funct3 {
        0 => {
            let hu = u32::from(h);
            let imm =
                ((hu >> 7) & 0x30) | ((hu >> 1) & 0x3c0) | ((hu >> 4) & 0x4) | ((hu >> 2) & 0x8);
            if imm == 0 {
                return illegal;
            }
            #[allow(clippy::cast_possible_wrap)] // at most 10 bits
            let imm = imm as i32;
            Instr::OpImm {
                op: AluOp::Add,
                rd: prime_reg(h, 2),
                rs1: REG_SP,
                imm,
            }
        }
        2 | 6 => {
            // c.lw / c.sw share the CL-format offset: imm[5:3]=h[12:10], imm[2]=h[6], imm[6]=h[5].
            let hu = u32::from(h);
            #[allow(clippy::cast_possible_wrap)] // at most 7 bits
            let offset = (((hu >> 7) & 0x38) | ((hu >> 4) & 0x4) | ((hu << 1) & 0x40)) as i32;
            if funct3 == 2 {
                Instr::Load {
                    op: LoadOp::W,
                    rd: prime_reg(h, 2),
                    rs1: prime_reg(h, 7),
                    offset,
                }
            } else {
                Instr::Store {
                    op: StoreOp::W,
                    rs1: prime_reg(h, 7),
                    rs2: prime_reg(h, 2),
                    offset,
                }
            }
        }
        _ => illegal,
    }
}

fn decode_quadrant1(h: u16, funct3: u16) -> Instr {
    match funct3 {
        0 => {
            let rd = full_reg(h, 7);
            Instr::OpImm {
                op: AluOp::Add,
                rd,
                rs1: rd,
                imm: ci_imm(h),
            }
        }
        1 => Instr::Jal {
            rd: REG_RA,
            offset: cj_offset(h),
        },
        2 => Instr::OpImm {
            op: AluOp::Add,
            rd: full_reg(h, 7),
            rs1: REG_ZERO,
            imm: ci_imm(h),
        },
        3 => decode_lui_addi16sp(h),
        4 => decode_misc_alu(h),
        5 => Instr::Jal {
            rd: REG_ZERO,
            offset: cj_offset(h),
        },
        6 => Instr::Branch {
            op: BranchOp::Eq,
            rs1: prime_reg(h, 7),
            rs2: REG_ZERO,
            offset: cb_offset(h),
        },
        _ => Instr::Branch {
            op: BranchOp::Ne,
            rs1: prime_reg(h, 7),
            rs2: REG_ZERO,
            offset: cb_offset(h),
        },
    }
}

fn decode_quadrant2(h: u16, funct3: u16) -> Instr {
    let illegal = Instr::Illegal(u32::from(h));
    match funct3 {
        0 => match ci_shamt(h) {
            Some(imm) => {
                let rd = full_reg(h, 7);
                Instr::OpImm {
                    op: AluOp::Sll,
                    rd,
                    rs1: rd,
                    imm,
                }
            }
            None => illegal,
        },
        2 => {
            let rd = full_reg(h, 7);
            if rd == REG_ZERO {
                return illegal;
            }
            let hu = u32::from(h);
            #[allow(clippy::cast_possible_wrap)] // at most 8 bits
            let offset = (((hu >> 7) & 0x20) | ((hu >> 2) & 0x1c) | ((hu << 4) & 0xc0)) as i32;
            Instr::Load {
                op: LoadOp::W,
                rd,
                rs1: REG_SP,
                offset,
            }
        }
        4 => decode_jr_mv_add(h),
        6 => {
            let hu = u32::from(h);
            #[allow(clippy::cast_possible_wrap)] // at most 8 bits
            let offset = (((hu >> 7) & 0x3c) | ((hu >> 1) & 0xc0)) as i32;
            Instr::Store {
                op: StoreOp::W,
                rs1: REG_SP,
                rs2: full_reg(h, 2),
                offset,
            }
        }
        _ => illegal,
    }
}

fn decode_lui_addi16sp(h: u16) -> Instr {
    let rd = full_reg(h, 7);
    let hu = u32::from(h);
    if rd == REG_SP {
        let v = ((hu >> 3) & 0x200)
            | ((hu >> 2) & 0x10)
            | ((hu << 1) & 0x40)
            | ((hu << 4) & 0x180)
            | ((hu << 3) & 0x20);
        let imm = sext(v, 10);
        if imm == 0 {
            return Instr::Illegal(hu);
        }
        return Instr::OpImm {
            op: AluOp::Add,
            rd: REG_SP,
            rs1: REG_SP,
            imm,
        };
    }
    let imm = sext((((hu >> 12) & 1) << 17) | (((hu >> 2) & 0x1f) << 12), 18);
    if imm == 0 {
        return Instr::Illegal(hu);
    }
    #[allow(clippy::cast_sign_loss)] // LUI takes the raw upper bits
    Instr::Lui {
        rd,
        imm: imm as u32,
    }
}

fn decode_misc_alu(h: u16) -> Instr {
    let rd = prime_reg(h, 7);
    let illegal = Instr::Illegal(u32::from(h));
    match (h >> 10) & 0x3 {
        0 => ci_shamt(h).map_or(illegal, |imm| Instr::OpImm {
            op: AluOp::Srl,
            rd,
            rs1: rd,
            imm,
        }),
        1 => ci_shamt(h).map_or(illegal, |imm| Instr::OpImm {
            op: AluOp::Sra,
            rd,
            rs1: rd,
            imm,
        }),
        2 => Instr::OpImm {
            op: AluOp::And,
            rd,
            rs1: rd,
            imm: ci_imm(h),
        },
        _ => {
            if h & 0x1000 != 0 {
                return illegal; // c.subw/c.addw: RV64 only
            }
            let op = [AluOp::Sub, AluOp::Xor, AluOp::Or, AluOp::And][usize::from((h >> 5) & 0x3)];
            Instr::Op {
                op,
                rd,
                rs1: rd,
                rs2: prime_reg(h, 2),
            }
        }
    }
}

fn decode_jr_mv_add(h: u16) -> Instr {
    let (rs1, rs2) = (full_reg(h, 7), full_reg(h, 2));
    let bit12 = h & 0x1000 != 0;
    match (bit12, rs1, rs2) {
        (false, REG_ZERO, REG_ZERO) => Instr::Illegal(u32::from(h)),
        (false, _, REG_ZERO) => Instr::Jalr {
            rd: REG_ZERO,
            rs1,
            offset: 0,
        },
        (false, _, _) => Instr::Op {
            op: AluOp::Add,
            rd: rs1,
            rs1: REG_ZERO,
            rs2,
        },
        (true, REG_ZERO, REG_ZERO) => Instr::Ebreak,
        (true, _, REG_ZERO) => Instr::Jalr {
            rd: REG_RA,
            rs1,
            offset: 0,
        },
        (true, _, _) => Instr::Op {
            op: AluOp::Add,
            rd: rs1,
            rs1,
            rs2,
        },
    }
}

#[cfg(test)]
mod tests {
    // Encodings: GNU as, binutils 2.42-1ubuntu1+6 (riscv64-unknown-elf,
    // -march=rv32imc_zicsr_zifencei -mabi=ilp32); see task-4-report.md for the objdump listing.
    use super::decode_compressed;
    use crate::decode::decode;
    use crate::instr::*;

    fn c(raw: u16) -> Instr {
        let x = decode(u32::from(raw));
        assert_eq!(x.len, 2, "{raw:#06x}");
        assert_eq!(x.raw, u32::from(raw), "{raw:#06x}");
        x.instr
    }

    fn check(cases: &[(u16, Instr)]) {
        for &(raw, expected) in cases {
            assert_eq!(c(raw), expected, "{raw:#06x}");
        }
    }

    #[test]
    fn expands_compressed_instructions() {
        assert_eq!(
            c(0x0085),
            Instr::OpImm {
                op: AluOp::Add,
                rd: 1,
                rs1: 1,
                imm: 1
            }
        ); // c.addi ra,1
        assert_eq!(
            c(0x557d),
            Instr::OpImm {
                op: AluOp::Add,
                rd: 10,
                rs1: 0,
                imm: -1
            }
        ); // c.li a0,-1
        assert_eq!(
            c(0x852e),
            Instr::Op {
                op: AluOp::Add,
                rd: 10,
                rs1: 0,
                rs2: 11
            }
        ); // c.mv a0,a1
        assert_eq!(
            c(0x8082),
            Instr::Jalr {
                rd: 0,
                rs1: 1,
                offset: 0
            }
        ); // c.jr ra (ret)
        assert_eq!(
            c(0x4092),
            Instr::Load {
                op: LoadOp::W,
                rd: 1,
                rs1: 2,
                offset: 4
            }
        ); // c.lwsp ra,4(sp)
        assert_eq!(
            c(0xc206),
            Instr::Store {
                op: StoreOp::W,
                rs1: 2,
                rs2: 1,
                offset: 4
            }
        ); // c.swsp ra,4(sp)
        assert_eq!(
            c(0x0800),
            Instr::OpImm {
                op: AluOp::Add,
                rd: 8,
                rs1: 2,
                imm: 16
            }
        ); // c.addi4spn s0,sp,16
        assert_eq!(
            c(0xc001),
            Instr::Branch {
                op: BranchOp::Eq,
                rs1: 8,
                rs2: 0,
                offset: 0
            }
        ); // c.beqz s0,0
        assert_eq!(c(0xbffd), Instr::Jal { rd: 0, offset: -2 }); // c.j -2
        assert_eq!(c(0x9002), Instr::Ebreak); // c.ebreak
    }

    #[test]
    fn illegal_compressed_encodings_trap() {
        for raw in [
            0x0000u16, // all-zero parcel
            0x0004,    // c.addi4spn with zero immediate
            0x9001,    // c.srli with shamt[5]=1: reserved on RV32
            0x2000,    // c.fld: no F extension
            0x4002,    // c.lwsp with rd=0
            0x8002,    // c.jr with rs1=0
        ] {
            assert_eq!(c(raw), Instr::Illegal(u32::from(raw)), "{raw:#06x}");
        }
    }

    #[test]
    fn quadrant_3_parcel_is_illegal_when_decoded_directly() {
        // `decode` never routes low bits 0b11 here; this covers the arm itself.
        assert_eq!(decode_compressed(0x0003), Instr::Illegal(0x0003));
    }

    #[test]
    fn compressed_loads_and_stores_cover_extremes() {
        // Encodings: GNU as, binutils 2.42 (`c.lw`, `c.sw`, `c.lwsp`, `c.swsp`).
        check(&[
            (
                0x48e0,
                Instr::Load {
                    op: LoadOp::W,
                    rd: 8,
                    rs1: 9,
                    offset: 84,
                },
            ), // c.lw s0,84(s1)
            (
                0xdde8,
                Instr::Store {
                    op: StoreOp::W,
                    rs1: 11,
                    rs2: 10,
                    offset: 124,
                },
            ), // c.sw a0,124(a1)
            (
                0x40be,
                Instr::Load {
                    op: LoadOp::W,
                    rd: 1,
                    rs1: 2,
                    offset: 204,
                },
            ), // c.lwsp ra,204(sp)
            (
                0xde16,
                Instr::Store {
                    op: StoreOp::W,
                    rs1: 2,
                    rs2: 5,
                    offset: 60,
                },
            ), // c.swsp t0,60(sp)
        ]);
    }

    #[test]
    fn compressed_jumps_cover_cj_extremes_and_asymmetric_values() {
        // Encodings: GNU as, binutils 2.42 (`c.jal`, `c.j`).
        check(&[
            (
                0x2ffd,
                Instr::Jal {
                    rd: 1,
                    offset: 2046,
                },
            ), // c.jal . + 2046
            (
                0xb001,
                Instr::Jal {
                    rd: 0,
                    offset: -2048,
                },
            ), // c.j . - 2048
            (
                0xa34d,
                Instr::Jal {
                    rd: 0,
                    offset: 0x5a2,
                },
            ), // c.j . + 0x5A2
        ]);
    }

    #[test]
    fn compressed_lui_and_addi16sp_cover_both_signs() {
        // Encodings: GNU as, binutils 2.42 (`c.lui`, `c.addi16sp`).
        check(&[
            (
                0x62fd,
                Instr::Lui {
                    rd: 5,
                    imm: 0x1f000,
                },
            ), // c.lui t0,0x1f
            (
                0x7281,
                Instr::Lui {
                    rd: 5,
                    imm: 0xfffe_0000,
                },
            ), // c.lui t0,0xfffe0 (imm -32)
            (
                0x617d,
                Instr::OpImm {
                    op: AluOp::Add,
                    rd: 2,
                    rs1: 2,
                    imm: 496,
                },
            ), // c.addi16sp sp,496
            (
                0x7101,
                Instr::OpImm {
                    op: AluOp::Add,
                    rd: 2,
                    rs1: 2,
                    imm: -512,
                },
            ), // c.addi16sp sp,-512
        ]);
    }

    #[test]
    fn compressed_addi4spn_covers_asymmetric_value() {
        check(&[(
            0x1fc0,
            Instr::OpImm {
                op: AluOp::Add,
                rd: 8,
                rs1: 2,
                imm: 1012,
            },
        )]); // c.addi4spn s0,sp,1012
    }

    #[test]
    fn compressed_alu_immediates_cover_shifts_and_andi() {
        // Encodings: GNU as, binutils 2.42 (`c.srli`, `c.srai`, `c.andi`, `c.slli`).
        check(&[
            (
                0x80fd,
                Instr::OpImm {
                    op: AluOp::Srl,
                    rd: 9,
                    rs1: 9,
                    imm: 31,
                },
            ), // c.srli s1,31
            (
                0x84c5,
                Instr::OpImm {
                    op: AluOp::Sra,
                    rd: 9,
                    rs1: 9,
                    imm: 17,
                },
            ), // c.srai s1,17
            (
                0x9881,
                Instr::OpImm {
                    op: AluOp::And,
                    rd: 9,
                    rs1: 9,
                    imm: -32,
                },
            ), // c.andi s1,-32
            (
                0x88fd,
                Instr::OpImm {
                    op: AluOp::And,
                    rd: 9,
                    rs1: 9,
                    imm: 31,
                },
            ), // c.andi s1,31
            (
                0x98e5,
                Instr::OpImm {
                    op: AluOp::And,
                    rd: 9,
                    rs1: 9,
                    imm: -7,
                },
            ), // c.andi s1,-7
            (
                0x02fe,
                Instr::OpImm {
                    op: AluOp::Sll,
                    rd: 5,
                    rs1: 5,
                    imm: 31,
                },
            ), // c.slli t0,31
        ]);
    }

    #[test]
    fn compressed_register_ops_cover_sub_xor_or_and() {
        // Encodings: GNU as, binutils 2.42 (`c.sub`, `c.xor`, `c.or`, `c.and`, `c.add`).
        check(&[
            (
                0x8c05,
                Instr::Op {
                    op: AluOp::Sub,
                    rd: 8,
                    rs1: 8,
                    rs2: 9,
                },
            ), // c.sub s0,s1
            (
                0x8d2d,
                Instr::Op {
                    op: AluOp::Xor,
                    rd: 10,
                    rs1: 10,
                    rs2: 11,
                },
            ), // c.xor a0,a1
            (
                0x8e55,
                Instr::Op {
                    op: AluOp::Or,
                    rd: 12,
                    rs1: 12,
                    rs2: 13,
                },
            ), // c.or a2,a3
            (
                0x8f7d,
                Instr::Op {
                    op: AluOp::And,
                    rd: 14,
                    rs1: 14,
                    rs2: 15,
                },
            ), // c.and a4,a5
            (
                0x929a,
                Instr::Op {
                    op: AluOp::Add,
                    rd: 5,
                    rs1: 5,
                    rs2: 6,
                },
            ), // c.add t0,t1
        ]);
    }

    #[test]
    fn compressed_branches_cover_cb_extremes_and_asymmetric_values() {
        // Encodings: GNU as, binutils 2.42 (`c.bnez`, `c.beqz`).
        check(&[
            (
                0xe4ad,
                Instr::Branch {
                    op: BranchOp::Ne,
                    rs1: 9,
                    rs2: 0,
                    offset: 106,
                },
            ), // c.bnez s1,. + 106
            (
                0xd081,
                Instr::Branch {
                    op: BranchOp::Eq,
                    rs1: 9,
                    rs2: 0,
                    offset: -256,
                },
            ), // c.beqz s1,. - 256
        ]);
    }

    #[test]
    fn compressed_jalr_and_li_addi() {
        // Encodings: GNU as, binutils 2.42 (`c.jalr`, `c.li`, `c.addi`).
        check(&[
            (
                0x9282,
                Instr::Jalr {
                    rd: 1,
                    rs1: 5,
                    offset: 0,
                },
            ), // c.jalr t0
            (
                0x5281,
                Instr::OpImm {
                    op: AluOp::Add,
                    rd: 5,
                    rs1: 0,
                    imm: -32,
                },
            ), // c.li t0,-32
            (
                0x12fd,
                Instr::OpImm {
                    op: AluOp::Add,
                    rd: 5,
                    rs1: 5,
                    imm: -1,
                },
            ), // c.addi t0,-1
        ]);
    }

    #[test]
    fn reserved_compressed_encodings_trap_with_parcel() {
        // Reserved encodings GNU refuses to assemble; raw parcels as emitted by `.hword` in the listing.
        for raw in [
            0x6281u16, // c.lui rd=5 with nzimm=0
            0x6101,    // c.addi16sp with nzimm=0
            0x1282,    // c.slli rd=5 with shamt[5]=1
            0x9c05,    // c.subw (bit12 set, RV64-only)
            0x9c25,    // c.addw (bit12 set, RV64-only)
            0x2002,    // quadrant 2 funct3=1 (c.fldsp): no F extension
            0x6002,    // quadrant 2 funct3=3 (c.flwsp): no F extension
            0xa002,    // quadrant 2 funct3=5 (c.fsdsp): no D extension
            0xe002,    // quadrant 2 funct3=7 (c.fswsp): no F extension
            0x8000,    // quadrant 0 funct3=4: reserved
        ] {
            assert_eq!(c(raw), Instr::Illegal(u32::from(raw)), "{raw:#06x}");
        }
    }

    #[test]
    fn compressed_branches_cover_every_cb_offset_bit() {
        // Encodings: GNU as, binutils 2.42-1ubuntu1+6 (`c.bnez`, `c.beqz`; objdump lines in task-4-report.md).
        check(&[
            (
                0xecfd,
                Instr::Branch {
                    op: BranchOp::Ne,
                    rs1: 9,
                    rs2: 0,
                    offset: 254,
                },
            ), // c.bnez s1,. + 254
            (
                0xccfd,
                Instr::Branch {
                    op: BranchOp::Eq,
                    rs1: 9,
                    rs2: 0,
                    offset: 254,
                },
            ), // c.beqz s1,. + 254
            (
                0xe8d9,
                Instr::Branch {
                    op: BranchOp::Ne,
                    rs1: 9,
                    rs2: 0,
                    offset: 150,
                },
            ), // c.bnez s1,. + 150
            (
                0xc8d9,
                Instr::Branch {
                    op: BranchOp::Eq,
                    rs1: 9,
                    rs2: 0,
                    offset: 150,
                },
            ), // c.beqz s1,. + 150
            (
                0xf8d9,
                Instr::Branch {
                    op: BranchOp::Ne,
                    rs1: 9,
                    rs2: 0,
                    offset: -106,
                },
            ), // c.bnez s1,. - 106
            (
                0xd8d9,
                Instr::Branch {
                    op: BranchOp::Eq,
                    rs1: 9,
                    rs2: 0,
                    offset: -106,
                },
            ), // c.beqz s1,. - 106
            (
                0xf089,
                Instr::Branch {
                    op: BranchOp::Ne,
                    rs1: 9,
                    rs2: 0,
                    offset: -254,
                },
            ), // c.bnez s1,. - 254
            (
                0xd4ad,
                Instr::Branch {
                    op: BranchOp::Eq,
                    rs1: 9,
                    rs2: 0,
                    offset: -150,
                },
            ), // c.beqz s1,. - 150
        ]);
    }

    #[test]
    fn compressed_addi4spn_covers_every_ciw_immediate_bit() {
        // Encodings: GNU as, binutils 2.42-1ubuntu1+6 (`c.addi4spn`; objdump lines in task-4-report.md).
        check(&[
            (
                0x0020,
                Instr::OpImm {
                    op: AluOp::Add,
                    rd: 8,
                    rs1: 2,
                    imm: 8,
                },
            ), // c.addi4spn s0,sp,8 (bit 3)
            (
                0x1fe0,
                Instr::OpImm {
                    op: AluOp::Add,
                    rd: 8,
                    rs1: 2,
                    imm: 1020,
                },
            ), // c.addi4spn s0,sp,1020
            (
                0x0040,
                Instr::OpImm {
                    op: AluOp::Add,
                    rd: 8,
                    rs1: 2,
                    imm: 4,
                },
            ), // c.addi4spn s0,sp,4 (bit 2)
        ]);
    }
}
