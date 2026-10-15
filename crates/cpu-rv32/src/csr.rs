//! The CSR file (Zicsr) of the [`Core`] a hart models: the machine-mode CSRs, and on the cores
//! that have them the user-mode rules, the PMP and the vendor's CSRs.
use crate::csr_esp32c3::Esp32C3Csrs;
use crate::pmp::{Pmp, PmpRules};
use emu_core::{AccessKind, Cycles};

pub const MSTATUS: u16 = 0x300;
pub const MISA: u16 = 0x301;
pub const MIE: u16 = 0x304;
pub const MTVEC: u16 = 0x305;
/// Which counters user mode may read (the conformance bench only, [`Core::MachineUser`]).
pub const MCOUNTEREN: u16 = 0x306;
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
/// `MPP` = machine mode (0b11). A plain hart has machine mode only: its `MPP` reads this, always.
pub const MSTATUS_MPP_M: u32 = 0b11 << 11;
/// The low bit of `MPP`, the one software writes on the ESP32-C3 core: "only lower bit is
/// writable, write to the higher bit is ignored as it is directly tied to the lower bit" (TRM
/// §1.5, register 1.5, as quoted in report IR §2.3). `MPP` is 0 (user) or 3 (machine).
const MSTATUS_MPP_LOW: u32 = 1 << 11;
/// `MPRV`: loads and stores of machine mode are checked as `MPP`'s mode (the conformance bench;
/// the ESP32-C3 has no such field, TRM register 1.5).
const MSTATUS_MPRV: u32 = 1 << 17;
/// `TW` (timeout wait): `wfi` in user mode is an illegal instruction (TRM register 1.5).
const MSTATUS_TW: u32 = 1 << 21;
const MSTATUS_WRITABLE: u32 = MSTATUS_MIE | MSTATUS_MPIE;
/// `mtvec.MODE` = 1, vectored.
const MTVEC_VECTORED: u32 = 1;
/// The bits of `mtvec` below `BASE` on the ESP32-C3 core.
const MTVEC_C3_LOW: u32 = 0xFF;
/// MXL=1 (32-bit) | C | I | M.
const MISA_RV32IMC: u32 = (1 << 30) | (1 << 2) | (1 << 8) | (1 << 12);
/// `misa.U`, user mode implemented.
const MISA_U: u32 = 1 << 20;
/// CSR addresses whose bits `[11:10]` are 0b11 are read-only (Privileged spec §2.1).
const READ_ONLY_MASK: u16 = 0xC00;
/// Bits `[9:8]` of a CSR address: the lowest privilege that may access it (Privileged spec §2.1).
const CSR_PRIVILEGE_SHIFT: u16 = 8;
/// `mcounteren` bits for `cycle` and `instret`.
const COUNTEREN_CY: u32 = 1 << 0;
const COUNTEREN_IR: u32 = 1 << 2;

/// The privilege mode a hart executes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Privilege {
    User,
    #[default]
    Machine,
}

impl Privilege {
    /// The encoding of the mode in `MPP` and in bits `[9:8]` of a CSR address.
    const fn level(self) -> u16 {
        match self {
            Privilege::User => 0,
            Privilege::Machine => 3,
        }
    }
}

/// The core a hart models: its privilege modes, its PMP and its vendor's CSRs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Core {
    /// A plain RV32IMC hart: machine mode only, no PMP, no vendor CSR.
    #[default]
    MachineOnly,
    /// The conformance bench (`inkemu rvtest`, riscv-tests, riscv-arch-test against Sail): machine
    /// and user modes and a 16-entry PMP as the privileged specification has them, with the
    /// choices the Sail reference model makes as `tools/conformance/riscof/sail` configures it.
    /// `MPP` keeps machine or user and turns any other value into user, `MPRV` and `TW` exist,
    /// `mcounteren` tells which counters user mode may read; the PMP follows the specification's
    /// rules (`pmp::PmpRules::Specification`). Report `docs/research/esp32c3-pmp-and-user-mode.md`
    /// §4.
    MachineUser,
    /// The ESP32-C3 core (TRM chapter 1): machine and user modes, `misa` `0x40101104`, its PMP
    /// with the TRM's rules (`pmp::PmpRules::Esp32C3`), `ustatus`, the performance counter.
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
    core: Core,
    vendor: Option<Esp32C3Csrs>,
    pmp: Option<Pmp>,
    /// [`MCOUNTEREN`], on the conformance bench; every bit is writable, as in Sail's default
    /// configuration (`base.mcounteren_writable_bits`).
    mcounteren: u32,
}

#[allow(clippy::cast_possible_truncation)] // deliberate: callers want the low 32 bits
const fn low(v: u64) -> u32 {
    v as u32
}
const fn high(v: u64) -> u32 {
    low(v >> 32)
}

impl CsrFile {
    /// The CSR file of `core` at reset.
    #[must_use]
    pub fn new(core: Core) -> CsrFile {
        CsrFile {
            mtvec: match core {
                Core::MachineOnly | Core::MachineUser => 0,
                Core::Esp32C3 => MTVEC_VECTORED,
            },
            core,
            vendor: match core {
                Core::MachineOnly | Core::MachineUser => None,
                Core::Esp32C3 => Some(Esp32C3Csrs::default()),
            },
            pmp: match core {
                Core::MachineOnly => None,
                Core::MachineUser => Some(Pmp::new(PmpRules::Specification)),
                Core::Esp32C3 => Some(Pmp::new(PmpRules::Esp32C3)),
            },
            ..CsrFile::default()
        }
    }

    /// The ESP32-C3 cycle counter counts from here on the time base [`CsrFile::set_time_base`]
    /// gives instead of the hart's cycles; nothing on a hart without the ESP32-C3 CSRs.
    pub fn count_cycles_on_time_base(&mut self) {
        if let Some(vendor) = &mut self.vendor {
            vendor.count_on_time_base();
        }
    }

    /// The instant, in cycles of the 160 MHz time base, of the instruction about to execute: what
    /// the counter of [`CsrFile::count_cycles_on_time_base`] counts.
    pub fn set_time_base(&mut self, now: u64) {
        if let Some(vendor) = &mut self.vendor {
            vendor.set_time_base(now);
        }
    }

    /// Reads of the cycle counter that [`CsrFile::count_cycles_on_time_base`] answered.
    #[must_use]
    pub fn time_base_reads(&self) -> u64 {
        self.vendor.as_ref().map_or(0, Esp32C3Csrs::time_base_reads)
    }

    /// An instruction is about to read `addr`. A read has no effect on any CSR of the chip: it is
    /// counted for [`CsrFile::time_base_reads`].
    pub fn note_read(&mut self, addr: u16) {
        if let Some(vendor) = &mut self.vendor {
            vendor.note_read(addr);
        }
    }

    /// Reads a CSR; `None` means "no such CSR" (illegal instruction).
    #[must_use]
    pub fn read(&self, addr: u16, cycles: Cycles, instret: u64) -> Option<u32> {
        let cyc = cycles.get().wrapping_add(self.cycle_offset);
        let ret = instret.wrapping_add(self.instret_offset);
        Some(match addr {
            MSTATUS => self.mstatus | self.mpp_wired(),
            MISA => match self.core {
                Core::MachineOnly => MISA_RV32IMC,
                // TRM register 1.6 for the ESP32-C3: `0x40101104`.
                Core::MachineUser | Core::Esp32C3 => MISA_RV32IMC | MISA_U,
            },
            MCOUNTEREN if self.core == Core::MachineUser => self.mcounteren,
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
                return self.pmp.as_ref().and_then(|p| p.read(addr)).or_else(|| {
                    self.vendor
                        .as_ref()
                        .and_then(|v| v.read(addr, cycles, instret))
                });
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
            MSTATUS => {
                self.mstatus = match self.core {
                    Core::MachineOnly => value & MSTATUS_WRITABLE,
                    // Sail's `legalize_mstatus` with U and without S: `MPP` keeps machine or user
                    // and turns 1 and 2, modes the hart does not have, into user.
                    Core::MachineUser => {
                        let mpp = if value & MSTATUS_MPP_M == MSTATUS_MPP_M {
                            MSTATUS_MPP_M
                        } else {
                            0
                        };
                        (value & (MSTATUS_WRITABLE | MSTATUS_TW | MSTATUS_MPRV)) | mpp
                    }
                    // The ESP32-C3 core: `MPP` is WARL {0, 3}, set by its low bit, and `TW` is
                    // stored (TRM register 1.5). Reset value 0 (TRM §1.4.2).
                    Core::Esp32C3 => {
                        let mpp = if value & MSTATUS_MPP_LOW == 0 {
                            0
                        } else {
                            MSTATUS_MPP_M
                        };
                        (value & (MSTATUS_WRITABLE | MSTATUS_TW)) | mpp
                    }
                };
            }
            MCOUNTEREN if self.core == Core::MachineUser => self.mcounteren = value,
            MIE => self.mie = value,
            // `mip` pending bits are driven by the interrupt controller, not by software;
            // `misa` is WARL: writes are ignored, the ISA is fixed.
            MIP | MISA => {}
            MTVEC => {
                self.mtvec = match self.core {
                    // The ESP32-C3 core only has the vectored mode: `MODE` reads 1, `BASE` is
                    // bits 31:8 (TRM §1.5.2, register 1.7; report IR §2.2).
                    // ASSUMPTION: bits 7:2 read 0. The TRM draws `BASE` as [31:8] and says
                    // nothing of the bits below it; every value the ROM and ESP-IDF write has
                    // them clear. Writing `0xFFFFFFFF` to `mtvec` on a chip and reading it back
                    // settles it.
                    Core::Esp32C3 => (value & !MTVEC_C3_LOW) | MTVEC_VECTORED,
                    // A plain hart: modes 0 (direct) and 1 (vectored) only.
                    Core::MachineOnly | Core::MachineUser => value & !0b10,
                };
            }
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
                return self.pmp.as_mut().is_some_and(|p| p.write(addr, value))
                    || self
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

    /// From here on `mcycle` and `minstret` read what a new hart reads: 0 now, then what the hart
    /// executes. For a reset of a hart whose own totals (`cycles`, `instret`) go on across it:
    /// software that was just reset must not read the age of the run in them. That the counters
    /// of the chip restart with the core is assumed: the marker is on `Hart::reset`, with what
    /// settles it.
    pub(crate) fn restart_counters(&mut self, cycles: Cycles, instret: u64) {
        self.cycle_offset = 0u64.wrapping_sub(cycles.get());
        self.instret_offset = 0u64.wrapping_sub(instret);
    }

    /// The `MPP` a plain hart reads whatever is stored: machine mode, its only mode. The cores
    /// with a user mode store their `MPP` in [`CsrFile::mstatus`].
    const fn mpp_wired(&self) -> u32 {
        match self.core {
            Core::MachineOnly => MSTATUS_MPP_M,
            Core::MachineUser | Core::Esp32C3 => 0,
        }
    }

    /// The mode `MPP` holds.
    const fn mpp(&self) -> Privilege {
        if (self.mstatus | self.mpp_wired()) & MSTATUS_MPP_M == MSTATUS_MPP_M {
            Privilege::Machine
        } else {
            Privilege::User
        }
    }

    /// A trap is taken from `from`: `MPP` takes that mode (a plain hart's stays machine mode).
    pub(crate) fn save_privilege_on_trap(&mut self, from: Privilege) {
        if self.core != Core::MachineOnly {
            self.mstatus = match from {
                Privilege::Machine => self.mstatus | MSTATUS_MPP_M,
                Privilege::User => self.mstatus & !MSTATUS_MPP_M,
            };
        }
    }

    /// `mret`: returns the mode `MPP` holds, the one the hart goes on in, and gives `MPP` the least
    /// privileged mode the core has (privileged specification, `mret`): user mode on a core that
    /// has one, machine mode on a plain hart. On the conformance bench `MPRV` is cleared when the
    /// hart leaves machine mode (Sail's `MRET`, privileged specification 1.12).
    pub(crate) fn return_privilege(&mut self) -> Privilege {
        let to = self.mpp();
        if self.core != Core::MachineOnly {
            self.mstatus &= !MSTATUS_MPP_M;
        }
        if to != Privilege::Machine {
            self.mstatus &= !MSTATUS_MPRV;
        }
        to
    }

    /// The mode the loads and stores of an instruction executing in `current` are checked as:
    /// `MPP`'s when `MPRV` is set in machine mode (the conformance bench only).
    pub(crate) const fn data_privilege(&self, current: Privilege) -> Privilege {
        match current {
            Privilege::Machine if self.mstatus & MSTATUS_MPRV != 0 => self.mpp(),
            other => other,
        }
    }

    /// Whether `privilege` may access the CSR at `addr`: bits `[9:8]` of the address name the
    /// lowest mode that may (privileged specification §2.1; TRM §1.8.3 for the PMP's), and on the
    /// conformance bench `mcounteren` tells whether user mode may read `cycle` and `instret`.
    /// Whether the CSR exists is the business of [`CsrFile::read`].
    #[must_use]
    pub fn accessible(&self, addr: u16, privilege: Privilege) -> bool {
        if (addr >> CSR_PRIVILEGE_SHIFT) & 0b11 > privilege.level() {
            return false;
        }
        if privilege == Privilege::User && self.core == Core::MachineUser {
            return match addr {
                CYCLE | CYCLEH => self.mcounteren & COUNTEREN_CY != 0,
                INSTRET | INSTRETH => self.mcounteren & COUNTEREN_IR != 0,
                _ => true,
            };
        }
        true
    }

    /// Whether `wfi` in user mode is an illegal instruction (`mstatus.TW`, TRM register 1.5).
    pub(crate) const fn user_wfi_traps(&self) -> bool {
        self.mstatus & MSTATUS_TW != 0
    }

    /// Whether the PMP lets an access of `size` bytes at `addr` through; a core without one lets
    /// every access through (it executes in machine mode only).
    pub(crate) fn pmp_allows(
        &mut self,
        addr: u32,
        size: u32,
        kind: AccessKind,
        privilege: Privilege,
    ) -> bool {
        self.pmp
            .as_mut()
            .is_none_or(|pmp| pmp.allows(addr, size, kind, privilege))
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
    fn esp32c3_mstatus_resets_to_0_and_takes_mpp_from_its_low_bit() {
        let mut csr = CsrFile::new(Core::Esp32C3);
        assert_eq!(csr.read(MSTATUS, Cycles::ZERO, 0), Some(0));
        assert!(csr.write(MSTATUS, 0xffff_ffff));
        assert_eq!(
            csr.read(MSTATUS, Cycles::ZERO, 0),
            Some(MSTATUS_MIE | MSTATUS_MPIE | MSTATUS_MPP_M | MSTATUS_TW)
        );
        // The high bit of MPP is tied to the low one: a write of it alone is user mode.
        assert!(csr.write(MSTATUS, 1 << 12));
        assert_eq!(csr.read(MSTATUS, Cycles::ZERO, 0), Some(0));
        assert!(csr.write(MSTATUS, 1 << 11));
        assert_eq!(csr.read(MSTATUS, Cycles::ZERO, 0), Some(MSTATUS_MPP_M));
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
    fn the_esp32_c3_mtvec_is_vectored_only_with_a_256_byte_aligned_base() {
        // IR §2.2: the ROM writes 0x40001D00 and reads back 0x40001D01; the application writes
        // 0x40380001.
        let mut csr = CsrFile::new(Core::Esp32C3);
        assert_eq!(
            csr.read(MTVEC, Cycles::ZERO, 0),
            Some(1),
            "vectored at reset"
        );
        assert!(csr.write(MTVEC, 0x4000_1D00));
        assert_eq!(csr.read(MTVEC, Cycles::ZERO, 0), Some(0x4000_1D01));
        assert!(csr.write(MTVEC, 0x4038_0001));
        assert_eq!(csr.read(MTVEC, Cycles::ZERO, 0), Some(0x4038_0001));
        assert!(csr.write(MTVEC, 0x4038_00FE));
        assert_eq!(
            csr.read(MTVEC, Cycles::ZERO, 0),
            Some(0x4038_0001),
            "the bits under the base read 0, the mode reads 1"
        );
        // The generic cores keep the two standard modes.
        for core in [Core::MachineOnly, Core::MachineUser] {
            let mut generic = CsrFile::new(core);
            assert_eq!(generic.read(MTVEC, Cycles::ZERO, 0), Some(0));
            assert!(generic.write(MTVEC, 0x1000));
            assert_eq!(
                generic.read(MTVEC, Cycles::ZERO, 0),
                Some(0x1000),
                "direct mode"
            );
        }
    }

    #[test]
    fn vendor_csrs_exist_only_on_the_esp32c3_core() {
        let mut c3 = CsrFile::new(Core::Esp32C3);
        assert!(c3.write(PCER_USER, 1));
        assert_eq!(c3.read(PCER_USER, Cycles::ZERO, 0), Some(1));
        for core in [Core::MachineOnly, Core::MachineUser] {
            let mut generic = CsrFile::new(core);
            assert!(!generic.write(PCER_USER, 1));
            assert_eq!(generic.read(PCER_USER, Cycles::ZERO, 0), None);
        }
    }

    #[test]
    fn pmp_csrs_exist_on_the_cores_with_a_user_mode() {
        use crate::pmp::{PMPADDR0, PMPCFG0};
        let mut plain = CsrFile::new(Core::MachineOnly);
        assert_eq!(plain.read(PMPADDR0, Cycles::ZERO, 0), None);
        assert!(!plain.write(PMPCFG0, 0));
        for core in [Core::MachineUser, Core::Esp32C3] {
            let mut csr = CsrFile::new(core);
            assert!(csr.write(PMPADDR0 + 15, 0x2000_0000));
            assert_eq!(csr.read(PMPADDR0 + 15, Cycles::ZERO, 0), Some(0x2000_0000));
        }
    }

    #[test]
    fn misa_says_user_mode_on_the_cores_that_have_it() {
        let misa = |core| CsrFile::new(core).read(MISA, Cycles::ZERO, 0);
        assert_eq!(misa(Core::MachineOnly), Some(0x4000_1104));
        assert_eq!(misa(Core::MachineUser), Some(0x4010_1104));
        assert_eq!(misa(Core::Esp32C3), Some(0x4010_1104), "TRM register 1.6");
    }

    #[test]
    fn the_bench_keeps_mpp_machine_or_user_and_has_mprv_and_tw() {
        // Sail's `legalize_mstatus` with U and without S.
        let mut csr = CsrFile::new(Core::MachineUser);
        assert_eq!(csr.read(MSTATUS, Cycles::ZERO, 0), Some(0));
        assert!(csr.write(MSTATUS, 0xffff_ffff));
        assert_eq!(
            csr.read(MSTATUS, Cycles::ZERO, 0),
            Some(MSTATUS_MIE | MSTATUS_MPIE | MSTATUS_MPP_M | MSTATUS_MPRV | MSTATUS_TW)
        );
        for (mpp, kept) in [(0, 0), (1, 0), (2, 0), (3, MSTATUS_MPP_M)] {
            assert!(csr.write(MSTATUS, mpp << 11));
            assert_eq!(csr.read(MSTATUS, Cycles::ZERO, 0), Some(kept), "MPP {mpp}");
        }
    }

    #[test]
    fn mret_returns_the_mode_mpp_holds_and_leaves_user_mode_in_it() {
        for core in [Core::MachineUser, Core::Esp32C3] {
            let mut csr = CsrFile::new(core);
            csr.save_privilege_on_trap(Privilege::Machine);
            assert_eq!(csr.return_privilege(), Privilege::Machine, "{core:?}");
            assert_eq!(csr.read(MSTATUS, Cycles::ZERO, 0), Some(0), "{core:?}");
            assert_eq!(csr.return_privilege(), Privilege::User, "{core:?}");
            csr.save_privilege_on_trap(Privilege::User);
            assert_eq!(csr.return_privilege(), Privilege::User, "{core:?}");
        }
        let mut plain = CsrFile::new(Core::MachineOnly);
        plain.save_privilege_on_trap(Privilege::Machine);
        assert_eq!(plain.return_privilege(), Privilege::Machine);
        assert_eq!(
            plain.read(MSTATUS, Cycles::ZERO, 0),
            Some(MSTATUS_MPP_M),
            "wired"
        );
    }

    #[test]
    fn mprv_makes_loads_and_stores_of_machine_mode_take_mpps_mode_until_mret_leaves_it() {
        let mut csr = CsrFile::new(Core::MachineUser);
        assert!(csr.write(MSTATUS, MSTATUS_MPRV));
        assert_eq!(csr.data_privilege(Privilege::Machine), Privilege::User);
        assert_eq!(csr.data_privilege(Privilege::User), Privilege::User);
        assert!(csr.write(MSTATUS, MSTATUS_MPRV | MSTATUS_MPP_M));
        assert_eq!(csr.data_privilege(Privilege::Machine), Privilege::Machine);
        assert!(csr.write(MSTATUS, MSTATUS_MPRV));
        assert_eq!(csr.return_privilege(), Privilege::User);
        assert_eq!(
            csr.read(MSTATUS, Cycles::ZERO, 0),
            Some(0),
            "mret into user mode cleared MPRV"
        );
        // The ESP32-C3 has no MPRV (TRM register 1.5).
        let mut c3 = CsrFile::new(Core::Esp32C3);
        assert!(c3.write(MSTATUS, MSTATUS_MPRV));
        assert_eq!(c3.read(MSTATUS, Cycles::ZERO, 0), Some(0));
        assert_eq!(c3.data_privilege(Privilege::Machine), Privilege::Machine);
    }

    #[test]
    fn user_mode_reaches_only_the_csrs_whose_address_allows_it() {
        for core in [Core::MachineUser, Core::Esp32C3] {
            let csr = CsrFile::new(core);
            for addr in [MSTATUS, MEPC, crate::pmp::PMPCFG0, 0x7E2] {
                assert!(!csr.accessible(addr, Privilege::User), "{addr:#x}");
                assert!(csr.accessible(addr, Privilege::Machine), "{addr:#x}");
            }
            assert!(csr.accessible(PCER_USER, Privilege::User), "{core:?}");
        }
    }

    #[test]
    fn on_the_bench_mcounteren_tells_which_counters_user_mode_may_read() {
        let mut csr = CsrFile::new(Core::MachineUser);
        for addr in [CYCLE, CYCLEH, INSTRET, INSTRETH] {
            assert!(!csr.accessible(addr, Privilege::User), "{addr:#x}");
            assert!(csr.accessible(addr, Privilege::Machine), "{addr:#x}");
        }
        assert!(csr.write(MCOUNTEREN, COUNTEREN_CY));
        assert_eq!(csr.read(MCOUNTEREN, Cycles::ZERO, 0), Some(COUNTEREN_CY));
        assert!(csr.accessible(CYCLE, Privilege::User));
        assert!(csr.accessible(CYCLEH, Privilege::User));
        assert!(!csr.accessible(INSTRET, Privilege::User));
        // Only the bench has it: the ESP32-C3's CSR list does not name it (TRM table 1.4.1).
        let mut c3 = CsrFile::new(Core::Esp32C3);
        assert_eq!(c3.read(MCOUNTEREN, Cycles::ZERO, 0), None);
        assert!(!c3.write(MCOUNTEREN, 1));
    }
}
