//! Semantics of each decoded instruction.
use crate::csr::Privilege;
use crate::hart::{Hart, MisalignedPolicy};
use crate::instr::{AluOp, BranchOp, CsrOp, Decoded, Instr, LoadOp, MulOp, Reg, StoreOp};
use crate::trap::{Exception, Trap};
use emu_core::{AccessKind, AccessSize, Bus, InstrClass};

const BYTE_MASK: u32 = 0xff;
const HALF_MASK: u32 = 0xffff;

/// The effects of a successfully executed instruction that the hart commits at retirement.
pub(crate) struct Done {
    pub class: InstrClass,
    pub stall: u32,
    pub next_pc: u32,
    pub wait: bool,
}

impl Done {
    const fn simple(class: InstrClass, next_pc: u32) -> Done {
        Done {
            class,
            stall: 0,
            next_pc,
            wait: false,
        }
    }
}

// Casts reinterpret two's complement (ISA semantics); sub-word loads keep the low bytes of the bus word.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
impl Hart {
    fn r(&self, reg: Reg) -> u32 {
        self.x[usize::from(reg)]
    }

    fn w(&mut self, reg: Reg, value: u32) {
        if reg != 0 {
            self.x[usize::from(reg)] = value;
        }
    }

    pub(crate) fn execute(&mut self, d: Decoded, bus: &mut dyn Bus) -> Result<Done, Trap> {
        let next = self.pc.wrapping_add(u32::from(d.len));
        let done = Done::simple;
        Ok(match d.instr {
            Instr::Lui { rd, imm } => {
                self.w(rd, imm);
                done(InstrClass::Alu, next)
            }
            Instr::Auipc { rd, imm } => {
                self.w(rd, self.pc.wrapping_add(imm));
                done(InstrClass::Alu, next)
            }
            Instr::Jal { rd, offset } => {
                self.w(rd, next);
                done(InstrClass::Jump, self.pc.wrapping_add(offset as u32))
            }
            Instr::Jalr { rd, rs1, offset } => {
                let target = self.r(rs1).wrapping_add(offset as u32) & !1;
                self.w(rd, next);
                done(InstrClass::Jump, target)
            }
            Instr::Branch {
                op,
                rs1,
                rs2,
                offset,
            } => {
                let taken = branch_taken(op, self.r(rs1), self.r(rs2));
                if taken {
                    done(InstrClass::BranchTaken, self.pc.wrapping_add(offset as u32))
                } else {
                    done(InstrClass::BranchNotTaken, next)
                }
            }
            Instr::Load {
                op,
                rd,
                rs1,
                offset,
            } => self.exec_load(bus, op, rd, self.r(rs1).wrapping_add(offset as u32), next)?,
            Instr::Store {
                op,
                rs1,
                rs2,
                offset,
            } => self.exec_store(
                bus,
                op,
                self.r(rs1).wrapping_add(offset as u32),
                self.r(rs2),
                next,
            )?,
            Instr::OpImm { op, rd, rs1, imm } => {
                let value = alu(op, self.r(rs1), imm as u32);
                self.w(rd, value);
                done(InstrClass::Alu, next)
            }
            Instr::Op { op, rd, rs1, rs2 } => {
                let value = alu(op, self.r(rs1), self.r(rs2));
                self.w(rd, value);
                done(InstrClass::Alu, next)
            }
            Instr::MulDiv { op, rd, rs1, rs2 } => {
                let value = muldiv(op, self.r(rs1), self.r(rs2));
                self.w(rd, value);
                done(muldiv_class(op), next)
            }
            Instr::Csr {
                op,
                rd,
                src,
                uimm,
                csr,
            } => {
                self.exec_csr(d, op, rd, src, uimm, csr)?;
                done(InstrClass::Csr, next)
            }
            Instr::Fence | Instr::FenceI => done(InstrClass::System, next),
            Instr::Ecall | Instr::Ebreak | Instr::Mret | Instr::Wfi => {
                return self.exec_privileged(d, next);
            }
            Instr::Illegal(raw) => {
                return Err(trap(Exception::IllegalInstr, raw));
            }
        })
    }

    /// `ecall`, `ebreak`, `mret` and `wfi`: what they do depends on the mode the hart is in.
    fn exec_privileged(&mut self, d: Decoded, next: u32) -> Result<Done, Trap> {
        let user = self.privilege == Privilege::User;
        match d.instr {
            Instr::Ecall if user => Err(trap(Exception::EcallU, 0)),
            Instr::Ecall => Err(trap(Exception::EcallM, 0)),
            Instr::Ebreak => Err(trap(Exception::Breakpoint, self.pc)),
            // `mret` belongs to machine mode (privileged specification); TRM register 1.5 for
            // `wfi`: "If this bit is set, executing WFI instruction in User mode will cause
            // illegal instruction exception."
            Instr::Mret if user => Err(trap(Exception::IllegalInstr, d.raw)),
            Instr::Wfi if user && self.csr.user_wfi_traps() => {
                Err(trap(Exception::IllegalInstr, d.raw))
            }
            Instr::Mret => {
                let target = self.return_from_trap();
                Ok(Done::simple(InstrClass::System, target))
            }
            Instr::Wfi => Ok(Done {
                wait: true,
                ..Done::simple(InstrClass::System, next)
            }),
            // INVARIANT: `execute` sends only the four instructions above here.
            _ => Err(trap(Exception::IllegalInstr, d.raw)),
        }
    }

    fn exec_load(
        &mut self,
        bus: &mut dyn Bus,
        op: LoadOp,
        rd: Reg,
        addr: u32,
        next_pc: u32,
    ) -> Result<Done, Trap> {
        let size = match op {
            LoadOp::B | LoadOp::Bu => AccessSize::Byte,
            LoadOp::H | LoadOp::Hu => AccessSize::Half,
            LoadOp::W => AccessSize::Word,
        };
        self.check_alignment(addr, size, Exception::LoadMisaligned)?;
        let fault = Trap {
            cause: Exception::LoadAccessFault,
            tval: addr,
        };
        self.check_data(addr, size, AccessKind::Load, fault)?;
        let out = bus.read(addr, size, AccessKind::Load).map_err(|_| fault)?;
        // The bus contract already guarantees a right-aligned, zero-extended value; masking here
        // keeps a peripheral that returns its whole register word from leaking upper bits into rd.
        let value = match op {
            LoadOp::B => i32::from(out.value as u8 as i8) as u32,
            LoadOp::H => i32::from(out.value as u16 as i16) as u32,
            LoadOp::Bu => out.value & BYTE_MASK,
            LoadOp::Hu => out.value & HALF_MASK,
            LoadOp::W => out.value,
        };
        self.w(rd, value);
        Ok(Done {
            stall: out.stall,
            ..Done::simple(InstrClass::Load, next_pc)
        })
    }

    fn exec_store(
        &mut self,
        bus: &mut dyn Bus,
        op: StoreOp,
        addr: u32,
        value: u32,
        next_pc: u32,
    ) -> Result<Done, Trap> {
        let size = match op {
            StoreOp::B => AccessSize::Byte,
            StoreOp::H => AccessSize::Half,
            StoreOp::W => AccessSize::Word,
        };
        self.check_alignment(addr, size, Exception::StoreMisaligned)?;
        let fault = Trap {
            cause: Exception::StoreAccessFault,
            tval: addr,
        };
        self.check_data(addr, size, AccessKind::Store, fault)?;
        let stall = bus.write(addr, size, value).map_err(|_| fault)?;
        Ok(Done {
            stall,
            ..Done::simple(InstrClass::Store, next_pc)
        })
    }

    /// The PMP's verdict on a load or a store: checked in the mode `MPRV` gives, after the
    /// alignment (an address-misaligned exception comes before an access fault) and before the
    /// bus is touched, so a refused store writes nothing.
    fn check_data(
        &mut self,
        addr: u32,
        size: AccessSize,
        kind: AccessKind,
        fault: Trap,
    ) -> Result<(), Trap> {
        let privilege = self.csr.data_privilege(self.privilege);
        if self.csr.pmp_allows(addr, size.bytes(), kind, privilege) {
            Ok(())
        } else {
            Err(fault)
        }
    }

    fn check_alignment(&self, addr: u32, size: AccessSize, cause: Exception) -> Result<(), Trap> {
        let misaligned = !addr.is_multiple_of(size.bytes());
        if misaligned && self.config.misaligned == MisalignedPolicy::Trap {
            return Err(Trap { cause, tval: addr });
        }
        Ok(())
    }

    fn exec_csr(
        &mut self,
        d: Decoded,
        op: CsrOp,
        rd: Reg,
        src: u8,
        uimm: bool,
        csr: u16,
    ) -> Result<(), Trap> {
        let illegal = Trap {
            cause: Exception::IllegalInstr,
            tval: d.raw,
        };
        let operand = if uimm { u32::from(src) } else { self.r(src) };
        // A CSR above the current mode does not exist for the instruction: it reads nothing.
        if !self.csr.accessible(csr, self.privilege) {
            return Err(illegal);
        }
        // `csrrw` to x0 only writes (the specification: "shall not read the CSR"); every other
        // form reads.
        if op != CsrOp::Rw || rd != 0 {
            self.csr.note_read(csr);
        }
        let old = self
            .csr
            .read(csr, self.cycles, self.instret)
            .ok_or(illegal)?;
        // csrrs/csrrc with a zero source never write (and may target read-only CSRs).
        let writes = op == CsrOp::Rw || src != 0;
        if writes {
            let new = match op {
                CsrOp::Rw => operand,
                CsrOp::Rs => old | operand,
                CsrOp::Rc => old & !operand,
            };
            if !self
                .csr
                .write_with_counters(csr, new, self.cycles, self.instret)
            {
                return Err(illegal);
            }
        }
        self.w(rd, old);
        Ok(())
    }
}

const fn trap(cause: Exception, tval: u32) -> Trap {
    Trap { cause, tval }
}

fn muldiv_class(op: MulOp) -> InstrClass {
    match op {
        MulOp::Mul | MulOp::Mulh | MulOp::Mulhsu | MulOp::Mulhu => InstrClass::Mul,
        MulOp::Div | MulOp::Divu | MulOp::Rem | MulOp::Remu => InstrClass::Div,
    }
}

#[allow(clippy::cast_possible_wrap)] // signed comparison reinterprets the registers as two's complement
fn branch_taken(op: BranchOp, a: u32, b: u32) -> bool {
    match op {
        BranchOp::Eq => a == b,
        BranchOp::Ne => a != b,
        BranchOp::Lt => (a as i32) < (b as i32),
        BranchOp::Ge => (a as i32) >= (b as i32),
        BranchOp::Ltu => a < b,
        BranchOp::Geu => a >= b,
    }
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)] // two's-complement reinterpretation is the ISA semantics
fn alu(op: AluOp, a: u32, b: u32) -> u32 {
    let shamt = b & 0x1f;
    match op {
        AluOp::Add => a.wrapping_add(b),
        AluOp::Sub => a.wrapping_sub(b),
        AluOp::Sll => a << shamt,
        AluOp::Srl => a >> shamt,
        AluOp::Sra => ((a as i32) >> shamt) as u32,
        AluOp::Slt => u32::from((a as i32) < (b as i32)),
        AluOp::Sltu => u32::from(a < b),
        AluOp::Xor => a ^ b,
        AluOp::Or => a | b,
        AluOp::And => a & b,
    }
}

// Casts reinterpret two's complement; the `>> 32` products keep only the high word by design.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn muldiv(op: MulOp, a: u32, b: u32) -> u32 {
    let (sa, sb) = (a as i32, b as i32);
    match op {
        MulOp::Mul => a.wrapping_mul(b),
        MulOp::Mulh => ((i64::from(sa) * i64::from(sb)) >> 32) as u32,
        MulOp::Mulhsu => ((i64::from(sa) * i64::from(b)) >> 32) as u32,
        MulOp::Mulhu => ((u64::from(a) * u64::from(b)) >> 32) as u32,
        MulOp::Div => {
            if b == 0 {
                u32::MAX
            } else {
                sa.wrapping_div(sb) as u32
            }
        }
        MulOp::Divu => a.checked_div(b).unwrap_or(u32::MAX),
        MulOp::Rem => {
            if b == 0 {
                a
            } else {
                sa.wrapping_rem(sb) as u32
            }
        }
        MulOp::Remu => {
            if b == 0 {
                a
            } else {
                a % b
            }
        }
    }
}
