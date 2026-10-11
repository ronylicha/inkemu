//! Machine-mode CSR file (Zicsr), plus the CSRs of a vendor core selected by [`VendorCsrs`].
use crate::csr_esp32c3::Esp32C3Csrs;
use emu_core::Cycles;

pub const MSTATUS: u16 = 0x300;
pub const MISA: u16 = 0x301;
pub const MIE: u16 = 0x304;
pub const MTVEC: u16 = 0x305;
pub const MSCRATCH: u16 = 0x340;
pub const MEPC: u16 = 0x341;
pub const MCAUSE: u16 = 0x342;
pub const MTVAL: u16 = 0x343;
pub const MIP: u16 = 0x344;
pub const MCYCLE: u16 = 0xB00;
pub const MINSTRET: u16 = 0xB02;
pub const MCYCLEH: u16 = 0xB80;
pub const MINSTRETH: u16 = 0xB82;
pub const CYCLE: u16 = 0xC00;
pub const INSTRET: u16 = 0xC02;
pub const CYCLEH: u16 = 0xC80;
pub const INSTRETH: u16 = 0xC82;
pub const MVENDORID: u16 = 0xF11;
pub const MARCHID: u16 = 0xF12;
pub const MIMPID: u16 = 0xF13;
pub const MHARTID: u16 = 0xF14;

pub const MSTATUS_MIE: u32 = 1 << 3;
pub const MSTATUS_MPIE: u32 = 1 << 7;
/// MPP is hard-wired to machine mode (0b11): this hart has no lower privilege mode in scope.
pub const MSTATUS_MPP_M: u32 = 0b11 << 11;
const MSTATUS_WRITABLE: u32 = MSTATUS_MIE | MSTATUS_MPIE;
/// MXL=1 (32-bit) | C | I | M.
const MISA_RV32IMC: u32 = (1 << 30) | (1 << 2) | (1 << 8) | (1 << 12);
/// CSR addresses whose bits [11:10] are 0b11 are read-only (Privileged spec §2.1).
const READ_ONLY_MASK: u16 = 0xC00;

/// CSRs that only one vendor's core implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VendorCsrs {
    /// A plain RV32IMC machine-mode hart (conformance tests).
    #[default]
    None,
    /// The `ESP32-C3` core: `ustatus` and the performance counter.
    Esp32C3,
}

#[derive(Clone, Debug, Default)]
pub struct CsrFile {
    pub mstatus: u32,
    pub mie: u32,
    pub mip: u32,
    pub mtvec: u32,
    pub mscratch: u32,
    pub mepc: u32,
    pub mcause: u32,
    pub mtval: u32,
    cycle_offset: u64,
    instret_offset: u64,
    /// Set by a software write to `minstret`/`minstreth` until the hart retires that instruction.
    instret_written: bool,
    /// Set by a software write to `mcycle`/`mcycleh` until the hart retires that instruction.
    cycle_written: bool,
    vendor: Option<Esp32C3Csrs>,
}

#[allow(clippy::cast_possible_truncation)] // deliberate: callers want the low 32 bits
const fn low(v: u64) -> u32 {
    v as u32
}
const fn high(v: u64) -> u32 {
    low(v >> 32)
}

impl CsrFile {
    /// A CSR file with the machine-mode CSRs and those of `vendor`.
    #[must_use]
    pub fn new(vendor: VendorCsrs) -> CsrFile {
        CsrFile {
            vendor: match vendor {
                VendorCsrs::None => None,
                VendorCsrs::Esp32C3 => Some(Esp32C3Csrs::default()),
            },
            ..CsrFile::default()
        }
    }

    /// Reads a CSR; `None` means "no such CSR" (illegal instruction).
    #[must_use]
    pub fn read(&self, addr: u16, cycles: Cycles, instret: u64) -> Option<u32> {
        let cyc = cycles.get().wrapping_add(self.cycle_offset);
        let ret = instret.wrapping_add(self.instret_offset);
        Some(match addr {
            MSTATUS => self.mstatus | MSTATUS_MPP_M,
            MISA => MISA_RV32IMC,
            MIE => self.mie,
            MIP => self.mip,
            MTVEC => self.mtvec,
            MSCRATCH => self.mscratch,
            MEPC => self.mepc,
            MCAUSE => self.mcause,
            MTVAL => self.mtval,
            MCYCLE | CYCLE => low(cyc),
            MCYCLEH | CYCLEH => high(cyc),
            MINSTRET | INSTRET => low(ret),
            MINSTRETH | INSTRETH => high(ret),
            MVENDORID | MARCHID | MIMPID | MHARTID => 0,
            _ => {
                return self
                    .vendor
                    .as_ref()
                    .and_then(|v| v.read(addr, cycles, instret));
            }
        })
    }

    /// Writes a CSR; returns `false` for unknown or read-only CSRs.
    /// Counter writes are stored as offsets so the hart's own counters stay monotonic.
    pub fn write_with_counters(
        &mut self,
        addr: u16,
        value: u32,
        cycles: Cycles,
        instret: u64,
    ) -> bool {
        if addr & READ_ONLY_MASK == READ_ONLY_MASK {
            return false;
        }
        match addr {
            MSTATUS => self.mstatus = value & MSTATUS_WRITABLE,
            MIE => self.mie = value,
            // `mip` pending bits are driven by the interrupt controller, not by software;
            // `misa` is WARL: writes are ignored, the ISA is fixed.
            MIP | MISA => {}
            MTVEC => self.mtvec = value & !0b10, // modes 0 (direct) and 1 (vectored) only
            MSCRATCH => self.mscratch = value,
            MEPC => self.mepc = value & !1,
            MCAUSE => self.mcause = value,
            MTVAL => self.mtval = value,
            MCYCLE => {
                self.cycle_written = true;
                self.cycle_offset =
                    replace_low(cycles.get().wrapping_add(self.cycle_offset), value)
                        .wrapping_sub(cycles.get());
            }
            MCYCLEH => {
                self.cycle_written = true;
                self.cycle_offset =
                    replace_high(cycles.get().wrapping_add(self.cycle_offset), value)
                        .wrapping_sub(cycles.get());
            }
            MINSTRET => {
                self.instret_written = true;
                self.instret_offset = replace_low(instret.wrapping_add(self.instret_offset), value)
                    .wrapping_sub(instret);
            }
            MINSTRETH => {
                self.instret_written = true;
                self.instret_offset =
                    replace_high(instret.wrapping_add(self.instret_offset), value)
                        .wrapping_sub(instret);
            }
            _ => {
                return self
                    .vendor
                    .as_mut()
                    .is_some_and(|v| v.write(addr, value, cycles, instret));
            }
        }
        true
    }

    /// Called when the instruction that just executed retires, having spent `cycles_spent` cycles.
    /// A software write to `minstret`/`minstreth` or to `mcycle`/`mcycleh` takes precedence over the
    /// increment of that same instruction (Privileged spec, machine counters), so the increment
    /// (one instruction, `cycles_spent` cycles) is cancelled in the offset.
    pub fn on_instruction_retired(&mut self, cycles_spent: u32) {
        if std::mem::take(&mut self.instret_written) {
            self.instret_offset = self.instret_offset.wrapping_sub(1);
        }
        if std::mem::take(&mut self.cycle_written) {
            self.cycle_offset = self.cycle_offset.wrapping_sub(u64::from(cycles_spent));
        }
    }

    /// Convenience for tests and reset code (counters at zero).
    pub fn write(&mut self, addr: u16, value: u32) -> bool {
        let known = self.write_with_counters(addr, value, Cycles::ZERO, 0);
        // Not executed by an instruction: there is no retirement increment to cancel.
        self.instret_written = false;
        self.cycle_written = false;
        known
    }
}

fn replace_low(v: u64, low32: u32) -> u64 {
    (v & !0xffff_ffff) | u64::from(low32)
}
fn replace_high(v: u64, high32: u32) -> u64 {
    (v & 0xffff_ffff) | (u64::from(high32) << 32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csr_esp32c3::PCER_USER;

    #[test]
    fn mstatus_keeps_mpp_at_machine_and_masks_unsupported_bits() {
        let mut csr = CsrFile::default();
        assert!(csr.write(MSTATUS, 0xffff_ffff));
        let status = csr.read(MSTATUS, Cycles::ZERO, 0).unwrap();
        assert_eq!(status, MSTATUS_MIE | MSTATUS_MPIE | MSTATUS_MPP_M);
    }

    #[test]
    fn mtvec_reserved_mode_bit_and_mepc_bit0_are_cleared() {
        let mut csr = CsrFile::default();
        assert!(csr.write(MTVEC, 0x1003));
        assert!(csr.write(MEPC, 0x2001));
        assert_eq!(csr.read(MTVEC, Cycles::ZERO, 0), Some(0x1001));
        assert_eq!(csr.read(MEPC, Cycles::ZERO, 0), Some(0x2000));
    }

    #[test]
    fn mip_and_misa_ignore_software_writes() {
        let mut csr = CsrFile::default();
        let misa = csr.read(MISA, Cycles::ZERO, 0).unwrap();
        assert!(csr.write(MIP, 0xffff_ffff));
        assert!(csr.write(MISA, 0));
        assert_eq!(csr.read(MIP, Cycles::ZERO, 0), Some(0));
        assert_eq!(csr.read(MISA, Cycles::ZERO, 0), Some(misa));
    }

    #[test]
    fn user_counters_alias_machine_counters_and_split_into_halves() {
        let csr = CsrFile::default();
        let cycles = Cycles(0x1_0000_0005);
        let instret = 0x2_0000_0007;
        assert_eq!(csr.read(CYCLE, cycles, instret), Some(5));
        assert_eq!(csr.read(CYCLEH, cycles, instret), Some(1));
        assert_eq!(csr.read(INSTRET, cycles, instret), Some(7));
        assert_eq!(csr.read(INSTRETH, cycles, instret), Some(2));
    }

    #[test]
    fn counter_writes_become_offsets_that_keep_counting() {
        let mut csr = CsrFile::default();
        assert!(csr.write_with_counters(MCYCLE, 100, Cycles(10), 0));
        assert!(csr.write_with_counters(MCYCLEH, 3, Cycles(10), 0));
        assert!(csr.write_with_counters(MINSTRET, 50, Cycles(10), 4));
        assert!(csr.write_with_counters(MINSTRETH, 9, Cycles(10), 4));
        // 10 more cycles and 6 more instructions elapse.
        assert_eq!(csr.read(MCYCLE, Cycles(20), 10), Some(110));
        assert_eq!(csr.read(MCYCLEH, Cycles(20), 10), Some(3));
        assert_eq!(csr.read(MINSTRET, Cycles(20), 10), Some(56));
        assert_eq!(csr.read(MINSTRETH, Cycles(20), 10), Some(9));
    }

    #[test]
    fn read_only_and_unknown_csrs_reject_writes() {
        let mut csr = CsrFile::default();
        for addr in [CYCLE, INSTRETH, MVENDORID, MARCHID, MIMPID, MHARTID, 0x7ff] {
            assert!(!csr.write(addr, 1), "csr {addr:#x} must reject writes");
        }
        assert_eq!(csr.read(0x7ff, Cycles::ZERO, 0), None);
    }

    #[test]
    fn scratch_style_csrs_roundtrip() {
        let mut csr = CsrFile::default();
        for addr in [MSCRATCH, MCAUSE, MTVAL, MIE] {
            assert!(csr.write(addr, 0xdead_beef));
            assert_eq!(csr.read(addr, Cycles::ZERO, 0), Some(0xdead_beef));
        }
    }

    #[test]
    fn retiring_a_cycle_counter_write_cancels_the_cycles_of_that_instruction() {
        let mut csr = CsrFile::default();
        // The write executes at cycle 10 and the instruction then spends 3 cycles.
        assert!(csr.write_with_counters(MCYCLE, 100, Cycles(10), 0));
        csr.on_instruction_retired(3);
        assert_eq!(csr.read(MCYCLE, Cycles(13), 1), Some(100));
        assert!(csr.write_with_counters(MCYCLEH, 7, Cycles(13), 1));
        csr.on_instruction_retired(2);
        assert_eq!(csr.read(MCYCLEH, Cycles(15), 2), Some(7));
        // Only the instruction that wrote is affected.
        csr.on_instruction_retired(4);
        assert_eq!(csr.read(MCYCLE, Cycles(19), 3), Some(104));
    }

    #[test]
    fn convenience_write_leaves_no_pending_cycle_cancellation() {
        let mut csr = CsrFile::default();
        assert!(csr.write(MCYCLE, 50));
        csr.on_instruction_retired(5);
        assert_eq!(csr.read(MCYCLE, Cycles(5), 1), Some(55));
    }

    #[test]
    fn vendor_csrs_exist_only_when_selected() {
        let mut generic = CsrFile::new(VendorCsrs::None);
        let mut c3 = CsrFile::new(VendorCsrs::Esp32C3);
        assert!(!generic.write(PCER_USER, 1));
        assert_eq!(generic.read(PCER_USER, Cycles::ZERO, 0), None);
        assert!(c3.write(PCER_USER, 1));
        assert_eq!(c3.read(PCER_USER, Cycles::ZERO, 0), Some(1));
    }
}
