//! The text of an instruction as a debugger shows it (design `SP4a`, 4a-D4): the registers by
//! their ABI names, the CSRs by their names, branch and jump targets as absolute addresses, and
//! the pseudo-instructions of the RISC-V assembly manual where an instruction is one of them
//! (`li`, `mv`, `nop`, `ret`, `j`, `jr`, `beqz`, `bnez`, `not`, `neg`, `seqz`, `snez`, `csrr`,
//! `csrw`, `csrs`, `csrc` and their immediate forms), as GNU `objdump` writes them.
//!
//! A compressed instruction is written in its 32-bit form: the decoder expands it
//! ([`crate::decode`]), and [`Decoded::len`] says it was compressed. An illegal instruction is
//! written `illegal`, without its bits: a debugger may show code read from the flash, and the
//! decoded text is all it needs.
use crate::instr::{AluOp, BranchOp, CsrOp, Decoded, Instr, LoadOp, MulOp, Reg, StoreOp};

/// The ABI names of the 32 integer registers, `x0` first (RISC-V psABI, "Integer Register
/// Convention"). `x8` is `s0`, also called `fp`.
pub const ABI_NAMES: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
    "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
    "t5", "t6",
];

/// The ABI name of register `reg`; `reg` is masked to 5 bits, as every decoded field is.
#[must_use]
pub fn register_name(reg: Reg) -> &'static str {
    ABI_NAMES[usize::from(reg & 0x1f)]
}

const PMPCFG_NAMES: [&str; 4] = ["pmpcfg0", "pmpcfg1", "pmpcfg2", "pmpcfg3"];
const PMPADDR_NAMES: [&str; 16] = [
    "pmpaddr0",
    "pmpaddr1",
    "pmpaddr2",
    "pmpaddr3",
    "pmpaddr4",
    "pmpaddr5",
    "pmpaddr6",
    "pmpaddr7",
    "pmpaddr8",
    "pmpaddr9",
    "pmpaddr10",
    "pmpaddr11",
    "pmpaddr12",
    "pmpaddr13",
    "pmpaddr14",
    "pmpaddr15",
];

/// The name of the CSR at `address`, for the CSRs the ESP32-C3's core has and those a debugger
/// of it names (the trigger and debug-mode CSRs of the RISC-V debug specification, which the core
/// has too, TRM §1.4); `None` for another address, which is written in hexadecimal.
#[must_use]
pub fn csr_name(address: u16) -> Option<&'static str> {
    use crate::csr;
    use crate::csr_esp32c3 as esp;
    use crate::pmp::{PMPADDR0, PMPCFG0};
    if let Some(index) = address.checked_sub(PMPCFG0)
        && let Some(name) = PMPCFG_NAMES.get(usize::from(index))
    {
        return Some(name);
    }
    if let Some(index) = address.checked_sub(PMPADDR0)
        && let Some(name) = PMPADDR_NAMES.get(usize::from(index))
    {
        return Some(name);
    }
    Some(match address {
        esp::USTATUS => "ustatus",
        csr::MSTATUS => "mstatus",
        csr::MISA => "misa",
        csr::MIE => "mie",
        csr::MTVEC => "mtvec",
        csr::MCOUNTEREN => "mcounteren",
        csr::MSCRATCH => "mscratch",
        csr::MEPC => "mepc",
        csr::MCAUSE => "mcause",
        csr::MTVAL => "mtval",
        csr::MIP => "mip",
        TSELECT => "tselect",
        TDATA1 => "tdata1",
        TDATA2 => "tdata2",
        TCONTROL => "tcontrol",
        DCSR => "dcsr",
        DPC => "dpc",
        DSCRATCH0 => "dscratch0",
        DSCRATCH1 => "dscratch1",
        esp::PCER_MACHINE => "mpcer",
        esp::PCMR_MACHINE => "mpcmr",
        esp::PCCR_MACHINE => "mpccr",
        esp::PCER_USER => "upcer",
        esp::PCMR_USER => "upcmr",
        esp::PCCR_USER => "upccr",
        csr::MCYCLE => "mcycle",
        csr::MINSTRET => "minstret",
        csr::MCYCLEH => "mcycleh",
        csr::MINSTRETH => "minstreth",
        csr::CYCLE => "cycle",
        csr::INSTRET => "instret",
        csr::CYCLEH => "cycleh",
        csr::INSTRETH => "instreth",
        csr::MVENDORID => "mvendorid",
        csr::MARCHID => "marchid",
        csr::MIMPID => "mimpid",
        csr::MHARTID => "mhartid",
        _ => return None,
    })
}

/// `tselect` (RISC-V debug specification 0.13.2, §5.2): named for the disassembly; the core does
/// not model the triggers yet.
const TSELECT: u16 = 0x7A0;
const TDATA1: u16 = 0x7A1;
const TDATA2: u16 = 0x7A2;
const TCONTROL: u16 = 0x7A5;
const DCSR: u16 = 0x7B0;
const DPC: u16 = 0x7B1;
const DSCRATCH0: u16 = 0x7B2;
const DSCRATCH1: u16 = 0x7B3;

fn csr_text(address: u16) -> String {
    csr_name(address).map_or_else(|| format!("{address:#05x}"), str::to_owned)
}

/// The address a branch or a jump at `pc` reaches with `offset`.
fn target(pc: u32, offset: i32) -> String {
    format!("{:#010x}", pc.wrapping_add_signed(offset))
}

/// The text of `decoded`, fetched at `pc`.
#[must_use]
pub fn disassemble(decoded: &Decoded, pc: u32) -> String {
    let r = register_name;
    match decoded.instr {
        Instr::Lui { rd, imm } => format!("lui {}, {:#x}", r(rd), imm >> 12),
        Instr::Auipc { rd, imm } => format!("auipc {}, {:#x}", r(rd), imm >> 12),
        Instr::Jal { rd: 0, offset } => format!("j {}", target(pc, offset)),
        Instr::Jal { rd: 1, offset } => format!("jal {}", target(pc, offset)),
        Instr::Jal { rd, offset } => format!("jal {}, {}", r(rd), target(pc, offset)),
        Instr::Jalr {
            rd: 0,
            rs1: 1,
            offset: 0,
        } => "ret".to_owned(),
        Instr::Jalr {
            rd: 0,
            rs1,
            offset: 0,
        } => format!("jr {}", r(rs1)),
        Instr::Jalr {
            rd: 1,
            rs1,
            offset: 0,
        } => format!("jalr {}", r(rs1)),
        Instr::Jalr { rd, rs1, offset } => format!("jalr {}, {offset}({})", r(rd), r(rs1)),
        Instr::Branch {
            op,
            rs1,
            rs2,
            offset,
        } => branch(op, rs1, rs2, &target(pc, offset)),
        Instr::Load {
            op,
            rd,
            rs1,
            offset,
        } => format!("{} {}, {offset}({})", load_name(op), r(rd), r(rs1)),
        Instr::Store {
            op,
            rs1,
            rs2,
            offset,
        } => format!("{} {}, {offset}({})", store_name(op), r(rs2), r(rs1)),
        Instr::OpImm { op, rd, rs1, imm } => op_imm(op, rd, rs1, imm),
        Instr::Op { op, rd, rs1, rs2 } => op_reg(op, rd, rs1, rs2),
        Instr::MulDiv { op, rd, rs1, rs2 } => {
            format!("{} {}, {}, {}", mul_name(op), r(rd), r(rs1), r(rs2))
        }
        Instr::Csr {
            op,
            rd,
            src,
            uimm,
            csr,
        } => csr_instruction(op, rd, src, uimm, csr),
        Instr::Fence => "fence".to_owned(),
        Instr::FenceI => "fence.i".to_owned(),
        Instr::Ecall => "ecall".to_owned(),
        Instr::Ebreak => "ebreak".to_owned(),
        Instr::Mret => "mret".to_owned(),
        Instr::Wfi => "wfi".to_owned(),
        Instr::Illegal(_) => "illegal".to_owned(),
    }
}

fn branch(op: BranchOp, rs1: Reg, rs2: Reg, to: &str) -> String {
    let r = register_name;
    match (op, rs1, rs2) {
        (BranchOp::Eq, rs, 0) => format!("beqz {}, {to}", r(rs)),
        (BranchOp::Ne, rs, 0) => format!("bnez {}, {to}", r(rs)),
        (BranchOp::Lt, rs, 0) => format!("bltz {}, {to}", r(rs)),
        (BranchOp::Ge, rs, 0) => format!("bgez {}, {to}", r(rs)),
        (BranchOp::Lt, 0, rs) => format!("bgtz {}, {to}", r(rs)),
        (BranchOp::Ge, 0, rs) => format!("blez {}, {to}", r(rs)),
        _ => {
            let name = match op {
                BranchOp::Eq => "beq",
                BranchOp::Ne => "bne",
                BranchOp::Lt => "blt",
                BranchOp::Ge => "bge",
                BranchOp::Ltu => "bltu",
                BranchOp::Geu => "bgeu",
            };
            format!("{name} {}, {}, {to}", r(rs1), r(rs2))
        }
    }
}

const fn load_name(op: LoadOp) -> &'static str {
    match op {
        LoadOp::B => "lb",
        LoadOp::H => "lh",
        LoadOp::W => "lw",
        LoadOp::Bu => "lbu",
        LoadOp::Hu => "lhu",
    }
}

const fn store_name(op: StoreOp) -> &'static str {
    match op {
        StoreOp::B => "sb",
        StoreOp::H => "sh",
        StoreOp::W => "sw",
    }
}

const fn mul_name(op: MulOp) -> &'static str {
    match op {
        MulOp::Mul => "mul",
        MulOp::Mulh => "mulh",
        MulOp::Mulhsu => "mulhsu",
        MulOp::Mulhu => "mulhu",
        MulOp::Div => "div",
        MulOp::Divu => "divu",
        MulOp::Rem => "rem",
        MulOp::Remu => "remu",
    }
}

fn op_imm(op: AluOp, rd: Reg, rs1: Reg, imm: i32) -> String {
    let r = register_name;
    match (op, rd, rs1, imm) {
        (AluOp::Add, 0, 0, 0) => "nop".to_owned(),
        (AluOp::Add, rd, 0, imm) => format!("li {}, {imm}", r(rd)),
        (AluOp::Add, rd, rs, 0) => format!("mv {}, {}", r(rd), r(rs)),
        (AluOp::Xor, rd, rs, -1) => format!("not {}, {}", r(rd), r(rs)),
        (AluOp::Sltu, rd, rs, 1) => format!("seqz {}, {}", r(rd), r(rs)),
        _ => {
            let name = match op {
                AluOp::Add | AluOp::Sub => "addi",
                AluOp::Sll => "slli",
                AluOp::Slt => "slti",
                AluOp::Sltu => "sltiu",
                AluOp::Xor => "xori",
                AluOp::Srl => "srli",
                AluOp::Sra => "srai",
                AluOp::Or => "ori",
                AluOp::And => "andi",
            };
            format!("{name} {}, {}, {imm}", r(rd), r(rs1))
        }
    }
}

fn op_reg(op: AluOp, rd: Reg, rs1: Reg, rs2: Reg) -> String {
    let r = register_name;
    match (op, rs1) {
        // `c.mv` expands to `add rd, x0, rs2` (ISA manual, "Integer Register-Register
        // Operations" of the C extension).
        (AluOp::Add, 0) => format!("mv {}, {}", r(rd), r(rs2)),
        (AluOp::Sub, 0) => format!("neg {}, {}", r(rd), r(rs2)),
        (AluOp::Sltu, 0) => format!("snez {}, {}", r(rd), r(rs2)),
        _ => {
            let name = match op {
                AluOp::Add => "add",
                AluOp::Sub => "sub",
                AluOp::Sll => "sll",
                AluOp::Slt => "slt",
                AluOp::Sltu => "sltu",
                AluOp::Xor => "xor",
                AluOp::Srl => "srl",
                AluOp::Sra => "sra",
                AluOp::Or => "or",
                AluOp::And => "and",
            };
            format!("{name} {}, {}, {}", r(rd), r(rs1), r(rs2))
        }
    }
}

fn csr_instruction(op: CsrOp, rd: Reg, src: u8, uimm: bool, csr: u16) -> String {
    let r = register_name;
    let name = csr_text(csr);
    let operand = if uimm {
        src.to_string()
    } else {
        r(src).to_owned()
    };
    let suffix = if uimm { "i" } else { "" };
    match (op, rd, src, uimm) {
        // `csrrs rd, csr, x0`: a read; with `uimm`, `csrrsi rd, csr, 0` reads too.
        (CsrOp::Rs, rd, 0, _) => format!("csrr {}, {name}", r(rd)),
        (CsrOp::Rw, 0, _, _) => format!("csrw{suffix} {name}, {operand}"),
        (CsrOp::Rs, 0, _, _) => format!("csrs{suffix} {name}, {operand}"),
        (CsrOp::Rc, 0, _, _) => format!("csrc{suffix} {name}, {operand}"),
        _ => {
            let base = match op {
                CsrOp::Rw => "csrrw",
                CsrOp::Rs => "csrrs",
                CsrOp::Rc => "csrrc",
            };
            format!("{base}{suffix} {}, {name}, {operand}", r(rd))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm;
    use crate::decode;

    fn text(raw: u32, pc: u32) -> String {
        disassemble(&decode(raw), pc)
    }

    #[test]
    fn registers_are_named_by_their_abi_names() {
        assert_eq!(register_name(0), "zero");
        assert_eq!(register_name(2), "sp");
        assert_eq!(register_name(8), "s0");
        assert_eq!(register_name(10), "a0");
        assert_eq!(register_name(31), "t6");
        assert_eq!(register_name(32 + 10), "a0", "a register field is 5 bits");
    }

    #[test]
    fn the_csrs_of_the_core_have_names_and_another_is_written_in_hexadecimal() {
        assert_eq!(csr_name(0x300), Some("mstatus"));
        assert_eq!(csr_name(0x3A2), Some("pmpcfg2"));
        assert_eq!(csr_name(0x3BF), Some("pmpaddr15"));
        assert_eq!(csr_name(0x7E2), Some("mpccr"));
        assert_eq!(csr_name(0x7A0), Some("tselect"));
        assert_eq!(csr_name(0x3C0), None);
        assert_eq!(text(asm::csrrw(1, 0x3C0, 2), 0), "csrrw ra, 0x3c0, sp");
    }

    #[test]
    fn plain_instructions_are_written_as_objdump_writes_them() {
        let cases = [
            (asm::add(10, 11, 12), "add a0, a1, a2"),
            (asm::sub(5, 6, 7), "sub t0, t1, t2"),
            (asm::addi(2, 2, -16), "addi sp, sp, -16"),
            (asm::lui(10, 0x6000_0000), "lui a0, 0x60000"),
            (asm::auipc(1, 0x1000), "auipc ra, 0x1"),
            (asm::lw(10, 2, 12), "lw a0, 12(sp)"),
            (asm::lbu(5, 10, -1), "lbu t0, -1(a0)"),
            (asm::sw(1, 2, 28), "sw ra, 28(sp)"),
            (asm::sh(6, 5, 2), "sh t1, 2(t0)"),
            (asm::mul(10, 10, 11), "mul a0, a0, a1"),
            (asm::remu(15, 14, 13), "remu a5, a4, a3"),
            (asm::jalr(5, 6, 8), "jalr t0, 8(t1)"),
            (asm::ecall(), "ecall"),
            (asm::ebreak(), "ebreak"),
            (asm::mret(), "mret"),
            (asm::wfi(), "wfi"),
        ];
        for (raw, expected) in cases {
            assert_eq!(text(raw, 0x4000_0000), expected, "{raw:#010x}");
        }
    }

    #[test]
    fn jumps_and_branches_name_the_address_they_reach() {
        let pc = 0x4200_0100;
        assert_eq!(text(asm::jal(0, -8), pc), "j 0x420000f8");
        assert_eq!(text(asm::jal(1, 0x20), pc), "jal 0x42000120");
        assert_eq!(text(asm::jal(5, 4), pc), "jal t0, 0x42000104");
        assert_eq!(text(asm::beq(10, 11, 8), pc), "beq a0, a1, 0x42000108");
        assert_eq!(text(asm::bne(10, 0, -4), pc), "bnez a0, 0x420000fc");
        assert_eq!(text(asm::beq(10, 0, 4), pc), "beqz a0, 0x42000104");
        assert_eq!(text(asm::blt(10, 0, 4), pc), "bltz a0, 0x42000104");
        assert_eq!(text(asm::blt(0, 10, 4), pc), "bgtz a0, 0x42000104");
        assert_eq!(
            text(asm::jal(0, 0), 0xffff_fffc),
            "j 0xfffffffc",
            "a jump to itself"
        );
    }

    #[test]
    fn the_pseudo_instructions_of_the_assembly_manual_are_used() {
        let cases = [
            (asm::addi(0, 0, 0), "nop"),
            (asm::addi(10, 0, 42), "li a0, 42"),
            (asm::addi(10, 11, 0), "mv a0, a1"),
            (asm::jalr(0, 1, 0), "ret"),
            (asm::jalr(0, 5, 0), "jr t0"),
            (asm::jalr(1, 15, 0), "jalr a5"),
            (asm::sub(10, 0, 11), "neg a0, a1"),
            (asm::csrrs(10, 0x342, 0), "csrr a0, mcause"),
            (asm::csrrw(0, 0x305, 10), "csrw mtvec, a0"),
            (asm::csrrs(0, 0x300, 11), "csrs mstatus, a1"),
            (asm::csrrc(0, 0x300, 11), "csrc mstatus, a1"),
            (asm::csrrwi(0, 0x7E1, 1), "csrwi mpcmr, 1"),
            (asm::csrrsi(0, 0x300, 8), "csrsi mstatus, 8"),
            (asm::csrrci(0, 0x300, 8), "csrci mstatus, 8"),
            (asm::csrrw(10, 0x340, 11), "csrrw a0, mscratch, a1"),
            (asm::csrrsi(10, 0x344, 2), "csrrsi a0, mip, 2"),
        ];
        for (raw, expected) in cases {
            assert_eq!(text(raw, 0), expected, "{raw:#010x}");
        }
        // `xori rd, rs, -1`, `sltiu rd, rs, 1`, `sltu rd, x0, rs`: no encoder, the bits.
        assert_eq!(text(0xfff5_c513, 0), "not a0, a1");
        assert_eq!(text(0x0015_b513, 0), "seqz a0, a1");
        assert_eq!(text(0x00b0_3533, 0), "snez a0, a1");
    }

    #[test]
    fn a_compressed_instruction_is_written_in_its_expanded_form() {
        let cases = [
            (0x0505, "addi a0, a0, 1"),
            (0x4515, "li a0, 5"),
            (0x852e, "mv a0, a1"),
            (0x8082, "ret"),
            (0x0001, "nop"),
        ];
        for (parcel, expected) in cases {
            let decoded = decode(parcel);
            assert_eq!(decoded.len, 2, "{parcel:#06x}");
            assert_eq!(
                disassemble(&decoded, 0x4038_0000),
                expected,
                "{parcel:#06x}"
            );
        }
    }

    #[test]
    fn an_illegal_instruction_is_said_without_its_bits() {
        assert_eq!(text(0xffff_ffff, 0), "illegal");
        assert_eq!(text(0x0000_0000, 0), "illegal", "the all-zero parcel");
    }
}
