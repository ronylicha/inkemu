//! Test-only RV32 encoder, so tests read as assembly instead of hex.
#![allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]

fn r(f7: u32, rs2: u8, rs1: u8, f3: u32, rd: u8, op: u32) -> u32 {
    (f7 << 25)
        | (u32::from(rs2) << 20)
        | (u32::from(rs1) << 15)
        | (f3 << 12)
        | (u32::from(rd) << 7)
        | op
}
fn i(imm: i32, rs1: u8, f3: u32, rd: u8, op: u32) -> u32 {
    ((imm as u32 & 0xfff) << 20) | (u32::from(rs1) << 15) | (f3 << 12) | (u32::from(rd) << 7) | op
}
fn s(imm: i32, rs2: u8, rs1: u8, f3: u32, op: u32) -> u32 {
    let imm = imm as u32;
    (((imm >> 5) & 0x7f) << 25)
        | (u32::from(rs2) << 20)
        | (u32::from(rs1) << 15)
        | (f3 << 12)
        | ((imm & 0x1f) << 7)
        | op
}
fn b(imm: i32, rs2: u8, rs1: u8, f3: u32) -> u32 {
    let imm = imm as u32;
    (((imm >> 12) & 1) << 31)
        | (((imm >> 5) & 0x3f) << 25)
        | (u32::from(rs2) << 20)
        | (u32::from(rs1) << 15)
        | (f3 << 12)
        | (((imm >> 1) & 0xf) << 8)
        | (((imm >> 11) & 1) << 7)
        | 0x63
}
fn csr_insn(csr: u16, src: u8, f3: u32, rd: u8) -> u32 {
    (u32::from(csr) << 20) | (u32::from(src) << 15) | (f3 << 12) | (u32::from(rd) << 7) | 0x73
}
pub fn addi(rd: u8, rs1: u8, imm: i32) -> u32 {
    i(imm, rs1, 0, rd, 0x13)
}
pub fn add(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(0, rs2, rs1, 0, rd, 0x33)
}
pub fn sub(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(0x20, rs2, rs1, 0, rd, 0x33)
}
pub fn lui(rd: u8, imm: u32) -> u32 {
    (imm & 0xffff_f000) | (u32::from(rd) << 7) | 0x37
}
pub fn auipc(rd: u8, imm: u32) -> u32 {
    (imm & 0xffff_f000) | (u32::from(rd) << 7) | 0x17
}
pub fn jal(rd: u8, imm: i32) -> u32 {
    let imm = imm as u32;
    (((imm >> 20) & 1) << 31)
        | (((imm >> 1) & 0x3ff) << 21)
        | (((imm >> 11) & 1) << 20)
        | (imm & 0xff000)
        | (u32::from(rd) << 7)
        | 0x6f
}
pub fn jalr(rd: u8, rs1: u8, imm: i32) -> u32 {
    i(imm, rs1, 0, rd, 0x67)
}
pub fn beq(rs1: u8, rs2: u8, imm: i32) -> u32 {
    b(imm, rs2, rs1, 0)
}
pub fn bne(rs1: u8, rs2: u8, imm: i32) -> u32 {
    b(imm, rs2, rs1, 1)
}
pub fn blt(rs1: u8, rs2: u8, imm: i32) -> u32 {
    b(imm, rs2, rs1, 4)
}
pub fn lb(rd: u8, rs1: u8, imm: i32) -> u32 {
    i(imm, rs1, 0, rd, 0x03)
}
pub fn lh(rd: u8, rs1: u8, imm: i32) -> u32 {
    i(imm, rs1, 1, rd, 0x03)
}
pub fn lw(rd: u8, rs1: u8, imm: i32) -> u32 {
    i(imm, rs1, 2, rd, 0x03)
}
pub fn lbu(rd: u8, rs1: u8, imm: i32) -> u32 {
    i(imm, rs1, 4, rd, 0x03)
}
pub fn lhu(rd: u8, rs1: u8, imm: i32) -> u32 {
    i(imm, rs1, 5, rd, 0x03)
}
pub fn sb(rs2: u8, rs1: u8, imm: i32) -> u32 {
    s(imm, rs2, rs1, 0, 0x23)
}
pub fn sh(rs2: u8, rs1: u8, imm: i32) -> u32 {
    s(imm, rs2, rs1, 1, 0x23)
}
pub fn sw(rs2: u8, rs1: u8, imm: i32) -> u32 {
    s(imm, rs2, rs1, 2, 0x23)
}
pub fn mul(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(1, rs2, rs1, 0, rd, 0x33)
}
pub fn mulh(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(1, rs2, rs1, 1, rd, 0x33)
}
pub fn mulhsu(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(1, rs2, rs1, 2, rd, 0x33)
}
pub fn mulhu(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(1, rs2, rs1, 3, rd, 0x33)
}
pub fn div(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(1, rs2, rs1, 4, rd, 0x33)
}
pub fn divu(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(1, rs2, rs1, 5, rd, 0x33)
}
pub fn rem(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(1, rs2, rs1, 6, rd, 0x33)
}
pub fn remu(rd: u8, rs1: u8, rs2: u8) -> u32 {
    r(1, rs2, rs1, 7, rd, 0x33)
}
pub fn csrrw(rd: u8, csr: u16, rs1: u8) -> u32 {
    csr_insn(csr, rs1, 1, rd)
}
pub fn csrrs(rd: u8, csr: u16, rs1: u8) -> u32 {
    csr_insn(csr, rs1, 2, rd)
}
pub fn csrrwi(rd: u8, csr: u16, uimm: u8) -> u32 {
    csr_insn(csr, uimm, 5, rd)
}
pub const fn ecall() -> u32 {
    0x0000_0073
}
pub const fn ebreak() -> u32 {
    0x0010_0073
}
pub const fn mret() -> u32 {
    0x3020_0073
}
pub const fn wfi() -> u32 {
    0x1050_0073
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoder_matches_reference_encodings() {
        assert_eq!(addi(1, 0, 5), 0x0050_0093);
        assert_eq!(sub(3, 1, 2), 0x4020_81b3);
        assert_eq!(jal(1, -4), 0xffdf_f0ef);
        assert_eq!(beq(1, 2, 8), 0x0020_8463);
        assert_eq!(sw(5, 2, 12), 0x0051_2623);
        assert_eq!(csrrw(1, 0x300, 2), 0x3001_10f3);
    }
}
