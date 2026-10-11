//! CSRs specific to the ESP32-C3 core. Source: `docs/research/esp32c3-boot-chain.md`, §B.2.
use emu_core::Cycles;

/// User status. The ROM writes 0 then 1 in `_init`; nothing reads it back.
pub const USTATUS: u16 = 0x000;
/// Performance counter: event select, mode and count, machine addresses (ESP-IDF `rv_utils.h`).
pub const PCER_MACHINE: u16 = 0x7E0;
pub const PCMR_MACHINE: u16 = 0x7E1;
pub const PCCR_MACHINE: u16 = 0x7E2;
/// The same three registers at their user addresses, the ones the ROM uses.
pub const PCER_USER: u16 = 0x800;
pub const PCMR_USER: u16 = 0x801;
pub const PCCR_USER: u16 = 0x802;

/// `ustatus` bits of the RISC-V N extension draft.
/// ASSUMPTION: the silicon keeps only these two; a readback on a real chip would settle it.
pub const USTATUS_UIE: u32 = 1 << 0;
pub const USTATUS_UPIE: u32 = 1 << 4;

/// Event selection: CPU clock cycles (the value the ROM and ESP-IDF write).
pub const PCER_CYCLES: u32 = 1 << 0;
/// Event selection: retired instructions.
/// ASSUMPTION: bit position taken from the ESP32-C3 technical reference manual, not exercised by the ROM.
pub const PCER_INSTRUCTIONS: u32 = 1 << 1;
/// Mode register: global count enable.
pub const PCMR_ENABLE: u32 = 1 << 0;

/// Physical memory protection: 16 entries (ESP-IDF `SOC_CPU_PMP_REGION_GRANULARITY`, `cpu_region_protect.c`;
/// report BC §B.2). The registers are stored and the lock bit is honoured; **the protection is not
/// enforced** (decision D18): enforcement arrives with the application's memory protection in SP1b-3.
///
/// ASSUMPTION: a `pmpcfg` byte is stored verbatim. The privileged specification makes its fields
/// WARL — bits 5 and 6 are reserved, and so is the combination R = 0 with W = 1 — and what this core
/// keeps of them is not established; nothing reads them back on the measured paths. A readback on a
/// real chip, or the SP1b-4 lockstep, settles it. A lock is also never cleared, because no hart
/// reset exists yet: the reset of SP1b-3 must clear the whole PMP state.
pub const PMPCFG0: u16 = 0x3A0;
pub const PMPADDR0: u16 = 0x3B0;
pub const PMP_ENTRIES: usize = 16;
const PMPCFG_REGISTERS: u16 = 4;
const ENTRIES_PER_CFG: usize = 4;
/// `pmpcfg` byte: lock bit, and the address-matching field with its top-of-range value.
const PMP_LOCK: u8 = 0x80;
const PMP_MATCH_MASK: u8 = 0x18;
const PMP_MATCH_TOR: u8 = 0x08;

#[derive(Clone, Debug, Default)]
pub(crate) struct Esp32C3Csrs {
    ustatus: u32,
    pcer: u32,
    pcmr: u32,
    /// Counter value at the snapshot below.
    count: u32,
    snapshot_cycles: u64,
    snapshot_instret: u64,
    pmpcfg: [u8; PMP_ENTRIES],
    pmpaddr: [u32; PMP_ENTRIES],
}

#[allow(clippy::cast_possible_truncation)] // deliberate: the counter is 32 bits wide and wraps
const fn low(v: u64) -> u32 {
    v as u32
}

impl Esp32C3Csrs {
    fn counted(&self, cycles: Cycles, instret: u64) -> u32 {
        if self.pcmr & PCMR_ENABLE == 0 {
            return self.count;
        }
        // ASSUMPTION: when both the cycle and the instruction events are selected the counter counts
        // cycles, and any other event selection (hazards, branches, loads...) leaves it standing still;
        // the SP1b-4 lockstep against silicon-faithful traces settles it.
        let elapsed = if self.pcer & PCER_CYCLES != 0 {
            cycles.get().wrapping_sub(self.snapshot_cycles)
        } else if self.pcer & PCER_INSTRUCTIONS != 0 {
            instret.wrapping_sub(self.snapshot_instret)
        } else {
            0
        };
        self.count.wrapping_add(low(elapsed))
    }

    /// Freezes the value counted so far, so a configuration change only affects what follows.
    fn resnapshot(&mut self, cycles: Cycles, instret: u64) {
        self.count = self.counted(cycles, instret);
        self.snapshot_cycles = cycles.get();
        self.snapshot_instret = instret;
    }

    fn pmp_locked(&self, entry: usize) -> bool {
        self.pmpcfg
            .get(entry)
            .is_some_and(|cfg| cfg & PMP_LOCK != 0)
    }

    /// A locked entry freezes its own address; a locked top-of-range entry also freezes the address
    /// of the entry below, which is its lower bound.
    fn pmpaddr_locked(&self, entry: usize) -> bool {
        self.pmp_locked(entry)
            || self
                .pmpcfg
                .get(entry + 1)
                .is_some_and(|next| next & PMP_LOCK != 0 && next & PMP_MATCH_MASK == PMP_MATCH_TOR)
    }

    fn read_pmp(&self, addr: u16) -> Option<u32> {
        if let Some(index) = addr.checked_sub(PMPCFG0).filter(|i| *i < PMPCFG_REGISTERS) {
            let first = usize::from(index) * ENTRIES_PER_CFG;
            let bytes = self.pmpcfg.get(first..first + ENTRIES_PER_CFG)?;
            return Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
        }
        let entry = usize::from(addr.checked_sub(PMPADDR0)?);
        self.pmpaddr.get(entry).copied()
    }

    fn write_pmp(&mut self, addr: u16, value: u32) -> bool {
        if let Some(index) = addr.checked_sub(PMPCFG0).filter(|i| *i < PMPCFG_REGISTERS) {
            let first = usize::from(index) * ENTRIES_PER_CFG;
            for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
                if !self.pmp_locked(first + i)
                    && let Some(cfg) = self.pmpcfg.get_mut(first + i)
                {
                    *cfg = byte;
                }
            }
            return true;
        }
        let Some(entry) = addr
            .checked_sub(PMPADDR0)
            .map(usize::from)
            .filter(|e| *e < PMP_ENTRIES)
        else {
            return false;
        };
        if !self.pmpaddr_locked(entry) {
            // INVARIANT: `entry < PMP_ENTRIES` was checked just above.
            self.pmpaddr[entry] = value;
        }
        true
    }

    pub(crate) fn read(&self, addr: u16, cycles: Cycles, instret: u64) -> Option<u32> {
        Some(match addr {
            USTATUS => self.ustatus,
            PCER_MACHINE | PCER_USER => self.pcer,
            PCMR_MACHINE | PCMR_USER => self.pcmr,
            PCCR_MACHINE | PCCR_USER => self.counted(cycles, instret),
            _ => return self.read_pmp(addr),
        })
    }

    pub(crate) fn write(&mut self, addr: u16, value: u32, cycles: Cycles, instret: u64) -> bool {
        match addr {
            USTATUS => self.ustatus = value & (USTATUS_UIE | USTATUS_UPIE),
            PCER_MACHINE | PCER_USER => {
                self.resnapshot(cycles, instret);
                self.pcer = value;
            }
            PCMR_MACHINE | PCMR_USER => {
                self.resnapshot(cycles, instret);
                self.pcmr = value;
            }
            PCCR_MACHINE | PCCR_USER => {
                self.resnapshot(cycles, instret);
                self.count = value;
            }
            _ => return self.write_pmp(addr, value),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Cycles;

    #[test]
    fn counter_stays_frozen_until_it_is_enabled() {
        let c = Esp32C3Csrs::default();
        assert_eq!(c.read(PCCR_USER, Cycles(500), 40), Some(0));
    }

    #[test]
    fn counter_counts_cpu_cycles_once_the_cycle_event_and_the_enable_bit_are_set() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(PCER_USER, 1, Cycles(100), 10));
        assert!(c.write(PCMR_USER, 1, Cycles(110), 11));
        assert_eq!(c.read(PCCR_USER, Cycles(110), 11), Some(0));
        assert_eq!(c.read(PCCR_USER, Cycles(150), 20), Some(40));
    }

    #[test]
    fn machine_and_user_addresses_are_the_same_registers() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(PCER_MACHINE, 1, Cycles(0), 0));
        assert!(c.write(PCMR_MACHINE, 1, Cycles(0), 0));
        assert_eq!(c.read(PCER_USER, Cycles(0), 0), Some(1));
        assert_eq!(c.read(PCMR_USER, Cycles(0), 0), Some(1));
        assert_eq!(
            c.read(PCCR_MACHINE, Cycles(7), 0),
            c.read(PCCR_USER, Cycles(7), 0)
        );
    }

    #[test]
    fn writing_the_counter_sets_its_value_and_counting_resumes_from_there() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        assert!(c.write(PCCR_USER, 1000, Cycles(50), 5));
        assert_eq!(c.read(PCCR_USER, Cycles(60), 6), Some(1010));
    }

    #[test]
    fn disabling_freezes_the_value_reached() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 0, Cycles(30), 3);
        assert_eq!(c.read(PCCR_USER, Cycles(900), 90), Some(30));
    }

    #[test]
    fn instruction_event_counts_retired_instructions() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, PCER_INSTRUCTIONS, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        assert_eq!(c.read(PCCR_USER, Cycles(400), 25), Some(25));
    }

    #[test]
    fn selecting_both_events_counts_cycles() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, PCER_CYCLES | PCER_INSTRUCTIONS, Cycles(0), 0);
        c.write(PCMR_USER, PCMR_ENABLE, Cycles(0), 0);
        assert_eq!(c.read(PCCR_USER, Cycles(400), 25), Some(400));
    }

    #[test]
    fn counter_wraps_at_32_bits() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        c.write(PCCR_USER, u32::MAX, Cycles(0), 0);
        assert_eq!(c.read(PCCR_USER, Cycles(2), 0), Some(1));
    }

    #[test]
    fn ustatus_keeps_only_its_two_defined_bits() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(USTATUS, 0xffff_ffff, Cycles(0), 0));
        assert_eq!(
            c.read(USTATUS, Cycles(0), 0),
            Some(USTATUS_UIE | USTATUS_UPIE)
        );
    }

    #[test]
    fn other_addresses_are_not_vendor_csrs() {
        let mut c = Esp32C3Csrs::default();
        assert_eq!(c.read(0x7E3, Cycles(0), 0), None);
        assert!(!c.write(0x803, 1, Cycles(0), 0));
    }

    fn csrs() -> Esp32C3Csrs {
        Esp32C3Csrs::default()
    }
    const L: u32 = 0x80;
    const TOR: u32 = 0x08;

    #[test]
    fn pmp_registers_read_back_what_was_written() {
        let mut c = csrs();
        for i in 0..16u16 {
            assert_eq!(c.read(PMPADDR0 + i, Cycles(0), 0), Some(0));
            assert!(c.write(PMPADDR0 + i, 0x1000_0000 + u32::from(i), Cycles(0), 0));
            assert_eq!(
                c.read(PMPADDR0 + i, Cycles(0), 0),
                Some(0x1000_0000 + u32::from(i))
            );
        }
        for i in 0..4u16 {
            assert!(c.write(PMPCFG0 + i, 0x0F0B_0907, Cycles(0), 0));
            assert_eq!(c.read(PMPCFG0 + i, Cycles(0), 0), Some(0x0F0B_0907));
        }
        assert_eq!(
            c.read(0x3A4, Cycles(0), 0),
            None,
            "pmpcfg4 does not exist on a 16-entry PMP"
        );
        assert_eq!(
            c.read(0x3C0, Cycles(0), 0),
            None,
            "pmpaddr16 does not exist"
        );
    }

    #[test]
    fn a_locked_entry_ignores_later_writes_to_its_configuration_and_address() {
        let mut c = csrs();
        c.write(PMPADDR0 + 1, 0x111, Cycles(0), 0);
        // Entry 1 is byte 1 of pmpcfg0: lock it with R+W.
        c.write(PMPCFG0, (L | 0x03) << 8, Cycles(0), 0);
        // 0x7F in the three other bytes: every bit but the lock, so entries 0, 2 and 3 stay unlocked.
        c.write(PMPCFG0, 0x7F7F_7F7F, Cycles(0), 0);
        assert_eq!(
            c.read(PMPCFG0, Cycles(0), 0),
            Some(0x7F7F_007F | ((L | 0x03) << 8)),
            "byte 1 kept"
        );
        c.write(PMPADDR0 + 1, 0x222, Cycles(0), 0);
        assert_eq!(c.read(PMPADDR0 + 1, Cycles(0), 0), Some(0x111));
        c.write(PMPADDR0, 0x333, Cycles(0), 0);
        assert_eq!(
            c.read(PMPADDR0, Cycles(0), 0),
            Some(0x333),
            "entry 1 is not TOR: pmpaddr0 stays writable"
        );
    }

    #[test]
    fn a_locked_top_of_range_entry_also_locks_the_address_below_it() {
        let mut c = csrs();
        c.write(PMPADDR0 + 4, 0x444, Cycles(0), 0);
        // Entry 5 is byte 1 of pmpcfg1.
        c.write(PMPCFG0 + 1, (L | TOR | 0x01) << 8, Cycles(0), 0);
        c.write(PMPADDR0 + 4, 0x555, Cycles(0), 0);
        assert_eq!(c.read(PMPADDR0 + 4, Cycles(0), 0), Some(0x444));
    }

    /// Writes `cfg` as the configuration byte of `entry`, leaving the three other bytes of its
    /// `pmpcfg` register at 0.
    fn configure(c: &mut Esp32C3Csrs, entry: u16, cfg: u32) {
        assert!(c.write(PMPCFG0 + entry / 4, cfg << (8 * (entry % 4)), Cycles(0), 0));
    }
    fn addr(c: &Esp32C3Csrs, entry: u16) -> Option<u32> {
        c.read(PMPADDR0 + entry, Cycles(0), 0)
    }
    fn set_addr(c: &mut Esp32C3Csrs, entry: u16, value: u32) {
        assert!(c.write(PMPADDR0 + entry, value, Cycles(0), 0));
    }

    #[test]
    fn an_unlocked_top_of_range_entry_leaves_the_address_below_it_writable() {
        let mut c = csrs();
        set_addr(&mut c, 4, 0x444);
        configure(&mut c, 5, TOR | 0x01);
        set_addr(&mut c, 4, 0x555);
        assert_eq!(
            addr(&c, 4),
            Some(0x555),
            "only a *locked* TOR entry freezes it"
        );
        set_addr(&mut c, 5, 0x666);
        assert_eq!(addr(&c, 5), Some(0x666));
    }

    #[test]
    fn a_locked_top_of_range_entry_freezes_its_own_address_too() {
        let mut c = csrs();
        set_addr(&mut c, 5, 0x500);
        configure(&mut c, 5, L | TOR | 0x01);
        set_addr(&mut c, 5, 0x999);
        assert_eq!(addr(&c, 5), Some(0x500));
        set_addr(&mut c, 6, 0x600);
        assert_eq!(addr(&c, 6), Some(0x600), "the entry above is not concerned");
    }

    #[test]
    fn the_first_and_the_last_entry_follow_the_same_lock_rules() {
        // Entry 0, locked top-of-range: its lower bound is address 0, there is no entry below.
        let mut first = csrs();
        set_addr(&mut first, 0, 0x100);
        configure(&mut first, 0, L | TOR);
        set_addr(&mut first, 0, 0x200);
        assert_eq!(addr(&first, 0), Some(0x100));
        set_addr(&mut first, 1, 0x300);
        assert_eq!(addr(&first, 1), Some(0x300));
        // Entry 15, locked top-of-range: it freezes pmpaddr14 and pmpaddr15; no entry 16 exists
        // that could freeze pmpaddr15 from above.
        let mut last = csrs();
        set_addr(&mut last, 14, 0xE00);
        set_addr(&mut last, 15, 0xF00);
        configure(&mut last, 15, L | TOR);
        set_addr(&mut last, 14, 0x1);
        set_addr(&mut last, 15, 0x2);
        assert_eq!(
            (addr(&last, 14), addr(&last, 15)),
            (Some(0xE00), Some(0xF00))
        );
        // Entry 15 locked but not top-of-range: pmpaddr14 stays writable.
        let mut plain = csrs();
        configure(&mut plain, 15, L | 0x18); // NAPOT
        set_addr(&mut plain, 14, 0x3);
        set_addr(&mut plain, 15, 0x4);
        assert_eq!((addr(&plain, 14), addr(&plain, 15)), (Some(0x3), Some(0)));
    }

    #[test]
    fn a_word_write_changes_the_unlocked_bytes_and_leaves_the_locked_ones() {
        let mut c = csrs();
        // Lock entries 0 and 2 of pmpcfg0 (bytes 0 and 2); entries 1 and 3 stay unlocked.
        assert!(c.write(PMPCFG0, ((L | 0x01) << 16) | (L | 0x02), Cycles(0), 0));
        assert!(c.write(PMPCFG0, 0x1F1D_1B19, Cycles(0), 0));
        assert_eq!(
            c.read(PMPCFG0, Cycles(0), 0),
            Some(0x1F81_1B82),
            "bytes 3 and 1 written, bytes 2 and 0 kept"
        );
        // A byte written with its lock bit locks from the next write on.
        assert!(c.write(PMPCFG0, 0x0000_9B00, Cycles(0), 0));
        assert!(c.write(PMPCFG0, 0, Cycles(0), 0));
        assert_eq!(c.read(PMPCFG0, Cycles(0), 0), Some(0x0081_9B82));
    }

    #[test]
    fn a_configuration_byte_is_stored_verbatim_reserved_bits_included() {
        // Pins the ASSUMPTION documented on `PMPCFG0`: bits 5 and 6, and R = 0 with W = 1.
        let mut c = csrs();
        assert!(c.write(PMPCFG0, 0x0000_0062, Cycles(0), 0));
        assert_eq!(c.read(PMPCFG0, Cycles(0), 0), Some(0x62));
    }

    #[test]
    fn pmp_csrs_exist_only_on_the_esp32c3_hart() {
        use crate::csr::{CsrFile, VendorCsrs};
        let mut generic = CsrFile::new(VendorCsrs::None);
        assert_eq!(generic.read(PMPADDR0, Cycles(0), 0), None);
        assert!(!generic.write(PMPCFG0, 0));
        let mut c3 = CsrFile::new(VendorCsrs::Esp32C3);
        assert!(c3.write(PMPADDR0 + 15, 0x2000_0000));
        assert_eq!(c3.read(PMPADDR0 + 15, Cycles(0), 0), Some(0x2000_0000));
    }
}
