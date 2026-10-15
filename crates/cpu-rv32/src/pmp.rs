//! Physical memory protection: 16 entries, a granularity of 4 bytes (G = 0), address matching OFF,
//! TOR, NA4 and NAPOT (privileged specification, "Physical Memory Protection"; ESP32-C3 TRM §1.8;
//! report `docs/research/esp32c3-pmp-and-user-mode.md`, PM below).
//!
//! Two sets of rules share the registers and the decoding of the ranges (`PmpRules`): the
//! specification's, with the choices the Sail reference model makes as the conformance bench
//! configures it, and the ESP32-C3's, which the TRM documents as not conforming (no static
//! priority). An access is checked before it reaches the bus; a refused access raises the access
//! fault of its kind, as a bus error does.
use crate::csr::Privilege;
use emu_core::AccessKind;

/// `pmpcfg0`; `pmpcfg1` to `pmpcfg3` follow.
pub const PMPCFG0: u16 = 0x3A0;
/// `pmpaddr0`; `pmpaddr1` to `pmpaddr15` follow.
pub const PMPADDR0: u16 = 0x3B0;
/// Entries of the ESP32-C3 (TRM §1.8.2) and of the conformance bench (Sail's `memory.pmp.count`).
pub const PMP_ENTRIES: usize = 16;
const PMPCFG_REGISTERS: u16 = 4;
const ENTRIES_PER_CFG: usize = 4;

/// `pmpcfg` byte: permissions, address matching, reserved bits, lock.
const CFG_R: u8 = 0x01;
const CFG_W: u8 = 0x02;
const CFG_X: u8 = 0x04;
const CFG_RWX: u8 = CFG_R | CFG_W | CFG_X;
const CFG_A_MASK: u8 = 0x18;
const CFG_A_TOR: u8 = 0x08;
const CFG_A_NA4: u8 = 0x10;
const CFG_A_NAPOT: u8 = 0x18;
/// Bits 5 and 6 of a `pmpcfg` byte.
const CFG_RESERVED: u8 = 0x60;
const CFG_LOCK: u8 = 0x80;
/// The bits of `pmpaddr` a 32-bit physical address space gives meaning to: bits 31:30 would
/// encode addresses of 4 GiB and more.
const ADDR_BITS_OF_32_BIT_SPACE: u32 = 0x3FFF_FFFF;

/// Accesses are checked page by page when a page's verdict does not depend on where in the page
/// the access falls (`PageVerdicts`).
const PAGE_SHIFT: u32 = 12;
const PAGE_BYTES: u64 = 1 << PAGE_SHIFT;
const VERDICT_SLOTS: usize = 256;

/// How the entries combine, and what a write keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PmpRules {
    /// The privileged specification, with the choices Sail 0.15 makes as
    /// `tools/conformance/riscof/sail/inkemu_rv32imc.json` configures it (PM §4): static priority,
    /// the lowest-numbered entry that holds any byte of the access decides, and one that holds only
    /// some of them refuses it; bits 5–6 of a `pmpcfg` byte read 0; `pmpaddr` keeps the 30 bits of a
    /// 32-bit physical address space; a write of R = 0 with W = 1 is ignored (`PMP_Ignore`).
    Specification,
    /// The ESP32-C3 (TRM §1.8.2, PM §2): no static priority, an access succeeds when an entry
    /// that applies holds all of it and grants it, faults when an entry that applies holds any of
    /// it and none grants it. What a write keeps is QEMU's (PM §3): the whole byte, bits 5–6
    /// included, all 32 bits of `pmpaddr`, and a write of R = 0 with W = 1 ignored.
    ///
    /// ASSUMPTION: an entry holding only part of an access grants none of it (the TRM is silent;
    /// only a misaligned access can straddle the 4-byte grain), and a NAPOT range above 1 GB, which
    /// the TRM says the core does not support, is decoded as the specification does. A probe on a
    /// chip settles both.
    Esp32C3,
}

/// The 16 entries, their ranges decoded, and the verdicts of whole pages.
#[derive(Clone, Debug)]
pub(crate) struct Pmp {
    rules: PmpRules,
    cfg: [u8; PMP_ENTRIES],
    addr: [u32; PMP_ENTRIES],
    /// Byte range `[start, end)` of each entry; `None` when it is off or matches no address.
    ranges: [Option<(u64, u64)>; PMP_ENTRIES],
    /// One bit per entry that applies to user mode, and to machine mode: when none does, a check
    /// needs no look at the address.
    user_entries: u16,
    machine_entries: u16,
    verdicts: PageVerdicts,
}

/// What an access is checked for, by privilege: the permissions a page grants, when they are the
/// same for every access that stays inside the page.
#[derive(Clone, Debug)]
struct PageVerdicts {
    /// Bumped at every change of an entry: a verdict of another generation is stale. Never 0, the
    /// mark of an empty slot; when it would run past `u32::MAX` it starts again at 1 with every slot
    /// emptied, so that no verdict of an earlier round is taken for a fresh one (`advance`).
    generation: u32,
    slots: Box<[Slot; VERDICT_SLOTS]>,
}

impl PageVerdicts {
    const FIRST_GENERATION: u32 = 1;

    /// Every verdict is stale from here on.
    fn advance(&mut self) {
        if let Some(next) = self.generation.checked_add(1) {
            self.generation = next;
        } else {
            self.slots.fill(Slot::default());
            self.generation = Self::FIRST_GENERATION;
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    page: u32,
    /// The generation the slot was filled at: 0 is an empty slot.
    filled: u32,
    /// Permissions granted to user mode and to machine mode; `None` when they depend on where in
    /// the page the access falls.
    user: Option<u8>,
    machine: Option<u8>,
}

impl Pmp {
    pub(crate) fn new(rules: PmpRules) -> Pmp {
        Pmp {
            rules,
            cfg: [0; PMP_ENTRIES],
            addr: [0; PMP_ENTRIES],
            ranges: [None; PMP_ENTRIES],
            user_entries: 0,
            machine_entries: 0,
            verdicts: PageVerdicts {
                generation: PageVerdicts::FIRST_GENERATION,
                slots: Box::new([Slot::default(); VERDICT_SLOTS]),
            },
        }
    }

    /// The value of the PMP CSR at `addr`, `None` when it is not one.
    pub(crate) fn read(&self, addr: u16) -> Option<u32> {
        if let Some(index) = cfg_index(addr) {
            let first = usize::from(index) * ENTRIES_PER_CFG;
            let bytes = self.cfg.get(first..first + ENTRIES_PER_CFG)?;
            return Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
        }
        self.addr.get(addr_index(addr)?).copied()
    }

    /// Writes the PMP CSR at `addr`; `false` when it is not one. A locked part keeps its value.
    pub(crate) fn write(&mut self, addr: u16, value: u32) -> bool {
        if let Some(index) = cfg_index(addr) {
            let first = usize::from(index) * ENTRIES_PER_CFG;
            for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
                self.write_cfg(first + i, byte);
            }
        } else if let Some(entry) = addr_index(addr) {
            if !self.addr_locked(entry) {
                let kept = match self.rules {
                    PmpRules::Specification => value & ADDR_BITS_OF_32_BIT_SPACE,
                    PmpRules::Esp32C3 => value,
                };
                // INVARIANT: `addr_index` returns an index below `PMP_ENTRIES`.
                self.addr[entry] = kept;
            }
        } else {
            return false;
        }
        self.decode();
        true
    }

    fn write_cfg(&mut self, entry: usize, byte: u8) {
        if self.locked(entry) {
            return;
        }
        let byte = match self.rules {
            PmpRules::Specification => byte & !CFG_RESERVED,
            PmpRules::Esp32C3 => byte,
        };
        // R = 0 with W = 1 is reserved: both rules keep the entry as it was (PM §3, §4).
        if byte & (CFG_R | CFG_W) == CFG_W {
            return;
        }
        if let Some(cfg) = self.cfg.get_mut(entry) {
            *cfg = byte;
        }
    }

    fn locked(&self, entry: usize) -> bool {
        self.cfg.get(entry).is_some_and(|cfg| cfg & CFG_LOCK != 0)
    }

    /// A locked entry freezes its own address; a locked top-of-range entry also freezes the address
    /// of the entry below, which is its lower bound.
    fn addr_locked(&self, entry: usize) -> bool {
        self.locked(entry)
            || self
                .cfg
                .get(entry + 1)
                .is_some_and(|next| next & CFG_LOCK != 0 && next & CFG_A_MASK == CFG_A_TOR)
    }

    /// Decodes every range again and forgets every page verdict: a change of one entry can move the
    /// range of the next (its lower bound in TOR).
    fn decode(&mut self) {
        for entry in 0..PMP_ENTRIES {
            let previous = entry.checked_sub(1).map_or(0, |below| self.addr[below]);
            self.ranges[entry] = decode_range(self.cfg[entry], previous, self.addr[entry]);
        }
        self.user_entries = self.entries_applying(Privilege::User);
        self.machine_entries = self.entries_applying(Privilege::Machine);
        self.verdicts.advance();
    }

    /// One bit per entry that takes part in the checks of `privilege`.
    fn entries_applying(&self, privilege: Privilege) -> u16 {
        (0..PMP_ENTRIES)
            .filter(|&entry| self.applies(entry, privilege))
            .fold(0, |mask, entry| mask | (1 << entry))
    }

    /// Whether `entry` takes part in the checks of `privilege`: enabled, and for the ESP32-C3's
    /// machine mode, locked. When no entry does, every access of machine mode is allowed and every
    /// access of user mode refused, without a look at the address.
    fn applies(&self, entry: usize, privilege: Privilege) -> bool {
        self.ranges[entry].is_some()
            && match (self.rules, privilege) {
                // An unlocked entry matches machine mode, and grants it everything.
                (PmpRules::Specification, _) | (_, Privilege::User) => true,
                (PmpRules::Esp32C3, Privilege::Machine) => self.locked(entry),
            }
    }

    /// The permissions entry `entry` grants `privilege`.
    fn grants(&self, entry: usize, privilege: Privilege) -> u8 {
        let cfg = self.cfg[entry];
        if privilege == Privilege::Machine && cfg & CFG_LOCK == 0 {
            CFG_RWX
        } else {
            cfg & CFG_RWX
        }
    }

    /// Whether an access of `size` bytes at `addr` is allowed.
    pub(crate) fn allows(
        &mut self,
        addr: u32,
        size: u32,
        kind: AccessKind,
        privilege: Privilege,
    ) -> bool {
        let needed = permission(kind);
        let entries = match privilege {
            Privilege::User => self.user_entries,
            Privilege::Machine => self.machine_entries,
        };
        if entries == 0 {
            return default_permissions(privilege) & needed == needed;
        }
        let offset = u64::from(addr) & (PAGE_BYTES - 1);
        if offset + u64::from(size) > PAGE_BYTES {
            return self.allows_exactly(addr, size, needed, privilege);
        }
        let page = addr >> PAGE_SHIFT;
        let slot_index = usize::try_from(page).map_or(0, |p| p % VERDICT_SLOTS);
        let filled = self.verdicts.generation;
        let slot = self.verdicts.slots[slot_index];
        let slot = if slot.filled == filled && slot.page == page {
            slot
        } else {
            let fresh = Slot {
                page,
                filled,
                user: self.page_permissions(page, Privilege::User),
                machine: self.page_permissions(page, Privilege::Machine),
            };
            self.verdicts.slots[slot_index] = fresh;
            fresh
        };
        let verdict = match privilege {
            Privilege::User => slot.user,
            Privilege::Machine => slot.machine,
        };
        match verdict {
            Some(granted) => granted & needed == needed,
            None => self.allows_exactly(addr, size, needed, privilege),
        }
    }

    /// The permissions every access inside `page` gets, when they do not depend on where it falls:
    /// every entry that applies either holds the whole page or none of it (for the specification,
    /// the first entry that holds any of it holds all of it).
    fn page_permissions(&self, page: u32, privilege: Privilege) -> Option<u8> {
        let start = u64::from(page) << PAGE_SHIFT;
        let end = start + PAGE_BYTES;
        let mut granted = 0;
        let mut any = false;
        for entry in (0..PMP_ENTRIES).filter(|&e| self.applies(e, privilege)) {
            let Some(span) = self.ranges[entry] else {
                continue;
            };
            match overlap((start, end), span) {
                Overlap::None => continue,
                Overlap::Partial => return None,
                Overlap::Whole => {}
            }
            match self.rules {
                PmpRules::Specification => return Some(self.grants(entry, privilege)),
                PmpRules::Esp32C3 => {
                    granted |= self.grants(entry, privilege);
                    any = true;
                }
            }
        }
        Some(if any {
            granted
        } else {
            default_permissions(privilege)
        })
    }

    /// The verdict of the rules, entry by entry, for an access of `size` bytes at `addr` that
    /// needs `needed`.
    fn allows_exactly(&self, addr: u32, size: u32, needed: u8, privilege: Privilege) -> bool {
        let start = u64::from(addr);
        let access = (start, start + u64::from(size));
        let mut touched = false;
        for entry in (0..PMP_ENTRIES).filter(|&e| self.applies(e, privilege)) {
            let Some(span) = self.ranges[entry] else {
                continue;
            };
            match (self.rules, overlap(access, span)) {
                (_, Overlap::None) => {}
                (PmpRules::Specification, Overlap::Partial) => return false,
                (PmpRules::Specification, Overlap::Whole) => {
                    return self.grants(entry, privilege) & needed == needed;
                }
                (PmpRules::Esp32C3, Overlap::Partial) => touched = true,
                (PmpRules::Esp32C3, Overlap::Whole) => {
                    if self.grants(entry, privilege) & needed == needed {
                        return true;
                    }
                    touched = true;
                }
            }
        }
        !touched && default_permissions(privilege) & needed == needed
    }
}

/// What an access gets when no entry that applies holds any of it: everything in machine mode,
/// nothing in user mode (privileged specification; TRM §1.8.3).
const fn default_permissions(privilege: Privilege) -> u8 {
    match privilege {
        Privilege::Machine => CFG_RWX,
        Privilege::User => 0,
    }
}

const fn permission(kind: AccessKind) -> u8 {
    match kind {
        AccessKind::Fetch => CFG_X,
        AccessKind::Load => CFG_R,
        AccessKind::Store => CFG_W,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlap {
    None,
    Partial,
    /// The second range holds the whole of the first.
    Whole,
}

/// How much of `inner` lies in `outer`; both are half-open byte ranges.
fn overlap(inner: (u64, u64), outer: (u64, u64)) -> Overlap {
    if inner.1 <= outer.0 || outer.1 <= inner.0 {
        Overlap::None
    } else if outer.0 <= inner.0 && inner.1 <= outer.1 {
        Overlap::Whole
    } else {
        Overlap::Partial
    }
}

/// The byte range of an entry from its configuration, its address and the address of the entry
/// below (the lower bound in TOR, 0 for entry 0). A TOR entry whose lower bound is not below its
/// upper bound matches no address (specification; QEMU gives it address 0, PM §3).
fn decode_range(cfg: u8, previous: u32, addr: u32) -> Option<(u64, u64)> {
    let word = u64::from(addr);
    match cfg & CFG_A_MASK {
        CFG_A_TOR => {
            let (start, end) = (u64::from(previous) << 2, word << 2);
            (start < end).then_some((start, end))
        }
        CFG_A_NA4 => Some((word << 2, (word << 2) + 4)),
        CFG_A_NAPOT => {
            // The trailing ones of the address give the size: `a...a0` 8 bytes, `a...a01` 16...
            let mask = word ^ (word + 1);
            let begin = word & !mask;
            Some((begin << 2, (begin + mask + 1) << 2))
        }
        _ => None,
    }
}

fn cfg_index(addr: u16) -> Option<u16> {
    addr.checked_sub(PMPCFG0).filter(|i| *i < PMPCFG_REGISTERS)
}

fn addr_index(addr: u16) -> Option<usize> {
    addr.checked_sub(PMPADDR0)
        .map(usize::from)
        .filter(|e| *e < PMP_ENTRIES)
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: u32 = 0x80;
    const TOR: u32 = 0x08;
    const NA4: u32 = 0x10;
    const NAPOT: u32 = 0x18;
    const R: u32 = 0x01;
    const W: u32 = 0x02;
    const X: u32 = 0x04;
    use AccessKind::{Fetch, Load, Store};
    use Privilege::{Machine, User};

    fn c3() -> Pmp {
        Pmp::new(PmpRules::Esp32C3)
    }
    fn spec() -> Pmp {
        Pmp::new(PmpRules::Specification)
    }
    /// Writes `cfg` as the configuration byte of `entry`, keeping the three other bytes of its
    /// `pmpcfg` register.
    fn configure(p: &mut Pmp, entry: u16, cfg: u32) {
        let reg = PMPCFG0 + entry / 4;
        let shift = 8 * (entry % 4);
        let old = p.read(reg).unwrap_or(0);
        assert!(p.write(reg, (old & !(0xFF << shift)) | (cfg << shift)));
    }
    fn addr(p: &Pmp, entry: u16) -> Option<u32> {
        p.read(PMPADDR0 + entry)
    }
    fn set_addr(p: &mut Pmp, entry: u16, value: u32) {
        assert!(p.write(PMPADDR0 + entry, value));
    }
    /// `pmpaddr` of a byte address.
    const fn word(byte: u32) -> u32 {
        byte >> 2
    }

    #[test]
    fn registers_read_back_what_was_written() {
        for mut p in [c3(), spec()] {
            for i in 0..16u16 {
                assert_eq!(addr(&p, i), Some(0));
                set_addr(&mut p, i, 0x1000_0000 + u32::from(i));
                assert_eq!(addr(&p, i), Some(0x1000_0000 + u32::from(i)));
            }
            for i in 0..4u16 {
                assert!(p.write(PMPCFG0 + i, 0x0F0B_0907));
                assert_eq!(p.read(PMPCFG0 + i), Some(0x0F0B_0907));
            }
            assert_eq!(
                p.read(0x3A4),
                None,
                "pmpcfg4 does not exist on a 16-entry PMP"
            );
            assert_eq!(p.read(0x3C0), None, "pmpaddr16 does not exist");
            assert!(!p.write(0x3A4, 1));
            assert!(!p.write(0x3C0, 1));
        }
    }

    #[test]
    fn a_locked_entry_ignores_later_writes_to_its_configuration_and_address() {
        let mut p = c3();
        set_addr(&mut p, 1, 0x111);
        // Entry 1 is byte 1 of pmpcfg0: lock it with R+W.
        assert!(p.write(PMPCFG0, (L | R | W) << 8));
        // 0x7F in the three other bytes: every bit but the lock, so entries 0, 2 and 3 stay unlocked.
        assert!(p.write(PMPCFG0, 0x7F7F_7F7F));
        assert_eq!(
            p.read(PMPCFG0),
            Some(0x7F7F_007F | ((L | R | W) << 8)),
            "byte 1 kept"
        );
        set_addr(&mut p, 1, 0x222);
        assert_eq!(addr(&p, 1), Some(0x111));
        set_addr(&mut p, 0, 0x333);
        assert_eq!(
            addr(&p, 0),
            Some(0x333),
            "entry 1 is not TOR: pmpaddr0 stays writable"
        );
    }

    #[test]
    fn a_locked_top_of_range_entry_also_locks_the_address_below_it() {
        let mut p = c3();
        set_addr(&mut p, 4, 0x444);
        set_addr(&mut p, 5, 0x500);
        configure(&mut p, 5, L | TOR | R);
        set_addr(&mut p, 4, 0x555);
        assert_eq!(addr(&p, 4), Some(0x444));
        set_addr(&mut p, 5, 0x999);
        assert_eq!(addr(&p, 5), Some(0x500), "its own address is frozen too");
        set_addr(&mut p, 6, 0x600);
        assert_eq!(addr(&p, 6), Some(0x600), "the entry above is not concerned");
    }

    #[test]
    fn an_unlocked_top_of_range_entry_leaves_the_address_below_it_writable() {
        let mut p = c3();
        set_addr(&mut p, 4, 0x444);
        configure(&mut p, 5, TOR | R);
        set_addr(&mut p, 4, 0x555);
        assert_eq!(
            addr(&p, 4),
            Some(0x555),
            "only a *locked* TOR entry freezes it"
        );
    }

    #[test]
    fn the_first_and_the_last_entry_follow_the_same_lock_rules() {
        // Entry 0, locked top-of-range: its lower bound is address 0, there is no entry below.
        let mut first = c3();
        set_addr(&mut first, 0, 0x100);
        configure(&mut first, 0, L | TOR);
        set_addr(&mut first, 0, 0x200);
        assert_eq!(addr(&first, 0), Some(0x100));
        set_addr(&mut first, 1, 0x300);
        assert_eq!(addr(&first, 1), Some(0x300));
        // Entry 15, locked top-of-range: it freezes pmpaddr14 and pmpaddr15.
        let mut last = c3();
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
        let mut plain = c3();
        configure(&mut plain, 15, L | NAPOT);
        set_addr(&mut plain, 14, 0x3);
        set_addr(&mut plain, 15, 0x4);
        assert_eq!((addr(&plain, 14), addr(&plain, 15)), (Some(0x3), Some(0)));
    }

    #[test]
    fn a_word_write_changes_the_unlocked_bytes_and_leaves_the_locked_ones() {
        let mut p = c3();
        // Lock entries 0 and 2 of pmpcfg0 (bytes 0 and 2); entries 1 and 3 stay unlocked.
        assert!(p.write(PMPCFG0, ((L | R) << 16) | (L | W | R)));
        assert!(p.write(PMPCFG0, 0x1F1D_1B19));
        assert_eq!(
            p.read(PMPCFG0),
            Some(0x1F81_1B83),
            "bytes 3 and 1 written, bytes 2 and 0 kept"
        );
        // A byte written with its lock bit locks from the next write on.
        assert!(p.write(PMPCFG0, 0x0000_9B00));
        assert!(p.write(PMPCFG0, 0));
        assert_eq!(p.read(PMPCFG0), Some(0x0081_9B83));
    }

    #[test]
    fn a_write_of_r_0_with_w_1_leaves_its_entry_as_it_was_under_both_rules() {
        // QEMU ignores it (`pmp_write_cfg`); Sail is configured with `PMP_Ignore` (PM §3, §4).
        for mut p in [c3(), spec()] {
            configure(&mut p, 1, TOR | R);
            assert!(p.write(PMPCFG0, (W << 8) | (R | X)));
            assert_eq!(
                p.read(PMPCFG0),
                Some(((TOR | R) << 8) | (R | X)),
                "byte 1 kept, byte 0 written"
            );
        }
    }

    #[test]
    fn the_esp32c3_keeps_bits_5_and_6_and_all_32_address_bits_as_qemu_does() {
        let mut p = c3();
        assert!(p.write(PMPCFG0, 0x61));
        assert_eq!(p.read(PMPCFG0), Some(0x61));
        set_addr(&mut p, 0, 0xFFFF_FFFF);
        assert_eq!(addr(&p, 0), Some(0xFFFF_FFFF));
    }

    #[test]
    fn the_specification_rules_read_bits_5_and_6_as_0_and_keep_30_address_bits_as_sail_does() {
        let mut p = spec();
        assert!(p.write(PMPCFG0, 0x61));
        assert_eq!(p.read(PMPCFG0), Some(0x01));
        set_addr(&mut p, 0, 0xFFFF_FFFF);
        assert_eq!(addr(&p, 0), Some(0x3FFF_FFFF));
    }

    #[test]
    fn with_no_entry_enabled_machine_mode_may_do_anything_and_user_mode_nothing() {
        for mut p in [c3(), spec()] {
            for kind in [Fetch, Load, Store] {
                assert!(p.allows(0x4000_0000, 4, kind, Machine));
                assert!(!p.allows(0x4000_0000, 4, kind, User));
            }
        }
    }

    #[test]
    fn each_kind_of_access_needs_its_own_permission() {
        for (cfg, fetch, load, store) in [
            (R, false, true, false),
            (R | W, false, true, true),
            (R | X, true, true, false),
            (X, true, false, false),
            (R | W | X, true, true, true),
        ] {
            for mut p in [c3(), spec()] {
                set_addr(&mut p, 0, word(0x2000));
                configure(&mut p, 0, TOR | cfg);
                assert_eq!(p.allows(0x100, 4, Fetch, User), fetch, "{cfg:#x}");
                assert_eq!(p.allows(0x100, 4, Load, User), load, "{cfg:#x}");
                assert_eq!(p.allows(0x100, 4, Store, User), store, "{cfg:#x}");
            }
        }
    }

    #[test]
    fn tor_na4_and_napot_hold_the_bytes_the_specification_gives_them() {
        // TOR [0x1000, 0x2000): entry 1 above pmpaddr0.
        let mut p = spec();
        set_addr(&mut p, 0, word(0x1000));
        set_addr(&mut p, 1, word(0x2000));
        configure(&mut p, 1, TOR | R);
        assert!(!p.allows(0x0FFC, 4, Load, User));
        assert!(p.allows(0x1000, 4, Load, User));
        assert!(p.allows(0x1FFC, 4, Load, User));
        assert!(!p.allows(0x2000, 4, Load, User));
        // NA4 at 0x3000: four bytes.
        let mut p = spec();
        set_addr(&mut p, 0, word(0x3000));
        configure(&mut p, 0, NA4 | R);
        assert!(p.allows(0x3000, 4, Load, User));
        assert!(p.allows(0x3003, 1, Load, User));
        assert!(!p.allows(0x3004, 1, Load, User));
        assert!(!p.allows(0x2FFF, 1, Load, User));
        // NAPOT: `...0` is 8 bytes, `...01` 16 bytes, `...0111` 64 bytes.
        for (encoded, base, size) in [
            (word(0x4000), 0x4000, 8),
            (word(0x4000) | 0b1, 0x4000, 16),
            (word(0x4000) | 0b111, 0x4000, 64),
            // 24 trailing ones: 2^(24 + 3) bytes.
            (word(0x4000_0000) | 0x00FF_FFFF, 0x4000_0000, 0x0800_0000),
        ] {
            let mut p = spec();
            set_addr(&mut p, 0, encoded);
            configure(&mut p, 0, NAPOT | R);
            assert!(p.allows(base, 4, Load, User), "{encoded:#x}");
            assert!(p.allows(base + size - 4, 4, Load, User), "{encoded:#x}");
            assert!(!p.allows(base + size, 4, Load, User), "{encoded:#x}");
            assert!(!p.allows(base - 4, 4, Load, User), "{encoded:#x}");
        }
        // NAPOT of all ones covers the whole address space.
        let mut p = spec();
        set_addr(&mut p, 0, u32::MAX);
        configure(&mut p, 0, NAPOT | R);
        assert!(p.allows(0, 4, Load, User));
        assert!(p.allows(0xFFFF_FFFC, 4, Load, User));
    }

    #[test]
    fn a_tor_entry_whose_bounds_do_not_rise_matches_no_address() {
        for mut p in [c3(), spec()] {
            set_addr(&mut p, 0, word(0x2000));
            set_addr(&mut p, 1, word(0x2000));
            configure(&mut p, 1, L | TOR);
            assert!(
                p.allows(0, 4, Load, Machine),
                "not address 0, as QEMU has it"
            );
            assert!(p.allows(0x2000, 4, Load, Machine));
        }
    }

    #[test]
    fn only_a_locked_entry_restricts_machine_mode() {
        for mut p in [c3(), spec()] {
            set_addr(&mut p, 0, word(0x1000));
            configure(&mut p, 0, TOR);
            assert!(
                p.allows(0x800, 4, Store, Machine),
                "unlocked, no permission"
            );
            assert!(!p.allows(0x800, 4, Store, User));
            configure(&mut p, 0, L | TOR);
            assert!(!p.allows(0x800, 4, Store, Machine), "locked, no permission");
            assert!(p.allows(0x1000, 4, Store, Machine), "outside every entry");
        }
    }

    /// Entry 0, unlocked, holds [0, 0x1000); entry 1, locked and read-only, holds [0, 0x2000).
    fn unlocked_over_locked(p: &mut Pmp) {
        set_addr(p, 0, word(0x1000));
        configure(p, 0, TOR);
        // NAPOT at 0, ten trailing ones: 2^(10 + 3) = 0x2000 bytes.
        set_addr(p, 1, 0x3FF);
        configure(p, 1, L | NAPOT | R);
    }

    #[test]
    fn on_the_bench_an_unlocked_entry_takes_part_in_machine_mode_checks() {
        // Specification rules: every active entry matches machine mode; an unlocked one that holds
        // the access grants everything and hides the entries after it, and one that holds only part
        // of it refuses it.
        let mut p = spec();
        unlocked_over_locked(&mut p);
        assert!(
            p.allows(0x800, 4, Store, Machine),
            "entry 0 decides: everything, entry 1's read-only is hidden"
        );
        assert!(
            !p.allows(0x1800, 4, Store, Machine),
            "only entry 1 holds it: read-only"
        );
        assert!(
            !p.allows(0x0FFE, 4, Load, Machine),
            "entry 0 holds part of it: refused, though entry 1 holds it all and grants R"
        );
        // The ESP32-C3: an unlocked entry does not apply to machine mode at all.
        let mut c = c3();
        unlocked_over_locked(&mut c);
        assert!(!c.allows(0x800, 4, Store, Machine), "entry 1 decides");
        assert!(c.allows(0x0FFE, 4, Load, Machine), "entry 1 holds it all");
    }

    /// Entry 0 grants only reads of [0x1000, 0x2000), entry 1 reads and writes of [0, 0x4000):
    /// they overlap.
    fn overlapping(p: &mut Pmp, lock: u32) {
        set_addr(p, 0, word(0x1000) | 0x1FF);
        configure(p, 0, lock | NAPOT | R);
        set_addr(p, 1, word(0x4000));
        configure(p, 1, lock | TOR | R | W);
    }

    #[test]
    fn the_specification_lets_the_lowest_numbered_entry_decide() {
        let mut p = spec();
        overlapping(&mut p, 0);
        assert!(
            !p.allows(0x1800, 4, Store, User),
            "entry 0 decides: read only"
        );
        assert!(p.allows(0x2800, 4, Store, User), "only entry 1 holds it");
    }

    #[test]
    fn the_esp32c3_has_no_static_priority_one_entry_that_grants_suffices() {
        // TRM §1.8.2: "access will succeed if it matches at least one of enabled PMP entries".
        let mut p = c3();
        overlapping(&mut p, 0);
        assert!(p.allows(0x1800, 4, Store, User));
        let mut locked = c3();
        overlapping(&mut locked, L);
        assert!(locked.allows(0x1800, 4, Store, Machine));
        assert!(
            !locked.allows(0x1800, 4, Fetch, Machine),
            "neither grants X"
        );
    }

    #[test]
    fn an_access_that_straddles_the_edge_of_an_entry() {
        // [0x1000, 0x2000) read-only, [0x2000, 0x3000) read-write; a word at 0x1FFE straddles.
        let setup = |p: &mut Pmp| {
            set_addr(p, 0, word(0x1000));
            set_addr(p, 1, word(0x2000));
            configure(p, 1, TOR | R);
            set_addr(p, 2, word(0x3000));
            configure(p, 2, TOR | R | W);
        };
        let mut s = spec();
        setup(&mut s);
        assert!(!s.allows(0x1FFE, 4, Load, User), "partial match: refused");
        let mut c = c3();
        setup(&mut c);
        assert!(!c.allows(0x1FFE, 4, Load, User), "no entry holds all of it");
        // An entry that holds all of it grants it on the ESP32-C3, whatever a partial one says.
        set_addr(&mut c, 3, word(0x1F00) | 0x3F);
        configure(&mut c, 3, NAPOT | R);
        assert!(c.allows(0x1FFE, 4, Load, User));
        set_addr(&mut s, 3, word(0x1F00) | 0x3F);
        configure(&mut s, 3, NAPOT | R);
        assert!(
            !s.allows(0x1FFE, 4, Load, User),
            "entry 1 still comes first"
        );
    }

    #[test]
    fn an_access_across_a_page_end_is_checked_whole() {
        let mut p = c3();
        set_addr(&mut p, 0, word(0x2000));
        configure(&mut p, 0, TOR | R);
        assert!(p.allows(0x0FFE, 4, Load, User));
        assert!(!p.allows(0x1FFE, 4, Load, User));
    }

    #[test]
    fn a_change_of_an_entry_is_seen_by_the_next_check() {
        let mut p = c3();
        assert!(p.allows(0x4038_0000, 4, Fetch, Machine));
        set_addr(&mut p, 0, word(0x4038_0000));
        set_addr(&mut p, 1, word(0x4040_0000));
        configure(&mut p, 1, L | TOR | R | W);
        assert!(
            !p.allows(0x4038_0000, 4, Fetch, Machine),
            "a cached verdict is not reused"
        );
        assert!(p.allows(0x4038_0000, 4, Load, Machine));
    }

    #[test]
    fn a_verdict_of_an_earlier_round_of_the_generation_counter_is_not_reused() {
        let mut p = c3();
        set_addr(&mut p, 0, word(0x2000));
        configure(&mut p, 0, TOR | R);
        // The verdict of page 0 is taken at generation 1.
        p.verdicts.generation = PageVerdicts::FIRST_GENERATION;
        assert!(p.allows(0x100, 4, Load, User));
        // 2^32 − 2 changes later the counter is at its last value; the next change brings it back
        // to 1, the generation the stale verdict carries.
        p.verdicts.generation = u32::MAX;
        configure(&mut p, 0, TOR);
        assert_eq!(p.verdicts.generation, PageVerdicts::FIRST_GENERATION);
        assert!(
            !p.allows(0x100, 4, Load, User),
            "the verdict filled at generation 1 of the earlier round is gone"
        );
    }

    /// The verdict of every entry, without the page verdicts.
    fn exact(p: &Pmp, addr: u32, size: u32, kind: AccessKind, privilege: Privilege) -> bool {
        p.allows_exactly(addr, size, permission(kind), privilege)
    }

    #[test]
    fn page_verdicts_agree_with_the_rules_on_random_configurations() {
        // A deterministic xorshift: the same configurations on every run.
        let mut state: u64 = 0x2026_1015_0f0f_4f4f;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        #[allow(clippy::cast_possible_truncation)] // the low bits are the random bits wanted
        let mut next32 = move || next() as u32;
        let interesting = |r: u32| 0x4000_0000 + (r % 0x6000);
        for round in 0..400 {
            let rules = if round % 2 == 0 {
                PmpRules::Esp32C3
            } else {
                PmpRules::Specification
            };
            let mut p = Pmp::new(rules);
            let entries = next32() % 6;
            for entry in 0..entries {
                let entry = u16::try_from(entry).unwrap_or(0);
                set_addr(
                    &mut p,
                    entry,
                    word(interesting(next32())) | ((next32() % 4) * 0x3F),
                );
                configure(&mut p, entry, next32() & 0x9F);
            }
            for _ in 0..200 {
                let size = [1, 2, 4][usize::try_from(next32() % 3).unwrap_or(0)];
                let at = interesting(next32()) - 0x800;
                let kind = [Fetch, Load, Store][usize::try_from(next32() % 3).unwrap_or(0)];
                let privilege = if next32() % 2 == 0 { User } else { Machine };
                assert_eq!(
                    p.allows(at, size, kind, privilege),
                    exact(&p, at, size, kind, privilege),
                    "round {round}: {at:#x}+{size} {kind:?} {privilege:?} {:?} {:?}",
                    p.cfg,
                    p.addr
                );
            }
        }
    }
}
