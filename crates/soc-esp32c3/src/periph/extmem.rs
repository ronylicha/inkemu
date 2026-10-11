//! `EXTMEM`: the flash-cache controller. Source: report CM §2 and §10 (ESP-IDF `extmem_reg.h`, the
//! ROM cache functions), FB §B.3 and §B.5.
//!
//! No cache line is modelled: the bus reads the flash through the MMU table at every access
//! (decision D4). The operations software waits for (invalidate, preload, autoload, freeze)
//! therefore complete at once, and the registers below only keep what software reads back.
use super::{Accepted, EXTMEM_BASE, Storage};
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("EXTMEM", name) - EXTMEM_BASE
}

const ICACHE_CTRL: u32 = reg("ICACHE_CTRL");
const ICACHE_CTRL1: u32 = reg("ICACHE_CTRL1");
const ICACHE_SYNC_CTRL: u32 = reg("ICACHE_SYNC_CTRL");
const ICACHE_PRELOAD_CTRL: u32 = reg("ICACHE_PRELOAD_CTRL");
const ICACHE_AUTOLOAD_CTRL: u32 = reg("ICACHE_AUTOLOAD_CTRL");
const ICACHE_FREEZE: u32 = reg("ICACHE_FREEZE");
const CACHE_STATE: u32 = reg("CACHE_STATE");
const CACHE_ILG_INT_CLR: u32 = reg("CACHE_ILG_INT_CLR");
const CACHE_ILG_INT_ST: u32 = reg("CACHE_ILG_INT_ST");
const CORE0_ACS_CACHE_INT_CLR: u32 = reg("CORE0_ACS_CACHE_INT_CLR");
const CORE0_ACS_CACHE_INT_ST: u32 = reg("CORE0_ACS_CACHE_INT_ST");
const CACHE_MMU_FAULT_CONTENT: u32 = reg("CACHE_MMU_FAULT_CONTENT");
const CACHE_MMU_FAULT_VADDR: u32 = reg("CACHE_MMU_FAULT_VADDR");

/// `ICACHE_SYNC_CTRL` bit 1, `SYNC_DONE`. ASSUMPTION: (decision D23) it reads set from reset. The
/// SVD gives reset 1 (`INVALIDATE_ENA` set, `DONE` clear), which reads as a tag initialisation that
/// ends before the ROM runs; the ROM sets bit 0 and waits for bit 1 either way (CM §9 item 5).
const SYNC_DONE: u32 = 1 << 1;
/// `ICACHE_PRELOAD_CTRL`: bit 1 `DONE` (always set: nothing is ever preloading), bit 2 `ORDER` (stored).
const PRELOAD_DONE: u32 = 1 << 1;
const PRELOAD_ORDER: u32 = 1 << 2;
/// `ICACHE_AUTOLOAD_CTRL`: bit 3 `DONE` (read-only, set); bits 0, 1, 2, 4, 5, 6 are configuration.
const AUTOLOAD_DONE: u32 = 1 << 3;
const AUTOLOAD_CONFIG: u32 = 0x77;
/// `ICACHE_FREEZE`: bit 0 `ENA`, bit 1 `MODE`, bit 2 `DONE` which mirrors `ENA`.
const FREEZE_ENA: u32 = 1 << 0;
const FREEZE_MODE: u32 = 1 << 1;
const FREEZE_DONE: u32 = 1 << 2;
/// `CACHE_STATE` [11:0]: 1 is idle, the only value `Cache_Disable_ICache` accepts.
const CACHE_IDLE: u32 = 1;
/// `CACHE_ILG_INT_ST` bit 5: an access went through an invalid MMU entry.
const MMU_ENTRY_FAULT: u32 = 1 << 5;
/// `CACHE_MMU_FAULT_CONTENT`: [9:0] the entry, [13:10] the operation; 1 is a CPU access.
const FAULT_CONTENT_MASK: u32 = 0x3FF;
const FAULT_CODE_CPU: u32 = 1 << 10;
/// `CORE0_ACS_CACHE_INT_ST`: bit 1 a store through the instruction bus, bit 5 through the data bus.
const IBUS_WRITE: u32 = 1 << 1;
const DBUS_WRITE: u32 = 1 << 5;

const WHY_OPERATION: &str = "cache operation: completes at once, there are no cache lines";
const WHY_FAULT_IRQ: &str = "cache error interrupt: the status is latched, no interrupt is raised";
const ACCEPTED: [Accepted; 14] = [
    Accepted::svd(
        "EXTMEM",
        "ICACHE_CTRL",
        "cache enable: the flash windows are always readable (D5)",
        "SP1b-4",
    ),
    Accepted::svd(
        "EXTMEM",
        "ICACHE_CTRL1",
        "bus gating of the flash windows (SHUT_IBUS, SHUT_DBUS; D5)",
        "SP1b-4",
    ),
    Accepted::svd("EXTMEM", "ICACHE_TAG_POWER_CTRL", "tag memory power", "SP8"),
    Accepted::svd("EXTMEM", "ICACHE_SYNC_CTRL", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_SYNC_ADDR", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_SYNC_SIZE", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_PRELOAD_CTRL", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_AUTOLOAD_CTRL", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "CACHE_ILG_INT_ENA", WHY_FAULT_IRQ, "SP1b-3"),
    Accepted::svd("EXTMEM", "CORE0_ACS_CACHE_INT_ENA", WHY_FAULT_IRQ, "SP1b-3"),
    Accepted::svd(
        "EXTMEM",
        "CACHE_WRAP_AROUND_CTRL",
        "wrap-around read mode",
        "SP1b-4",
    ),
    Accepted::svd("EXTMEM", "CACHE_MMU_POWER_CTRL", "MMU memory power", "SP8"),
    Accepted::svd("EXTMEM", "CACHE_MMU_OWNER", "MMU ownership bits", "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_FREEZE", WHY_OPERATION, "SP1b-4"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

#[derive(Clone, Debug)]
pub struct Extmem {
    registers: Storage,
    preload_order: u32,
    autoload: u32,
    freeze: u32,
    illegal_status: u32,
    access_status: u32,
    fault_content: u32,
    fault_vaddr: u32,
    generation: u64,
}

impl Extmem {
    #[must_use]
    pub fn new() -> Extmem {
        Extmem {
            registers: Storage::accepted(EXTMEM_BASE, &ACCEPTED),
            preload_order: 0,
            autoload: 0,
            freeze: 0,
            illegal_status: 0,
            access_status: 0,
            fault_content: 0,
            fault_vaddr: 0,
            generation: 0,
        }
    }

    /// Changes when the cache or one of its buses is switched on or off (decision D6).
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Records an access through an invalid MMU entry (`CACHE_ILG_INT_ST`, `CACHE_MMU_FAULT_*`; CM §6.1).
    pub fn latch_mmu_fault(&mut self, vaddr: u32, content: u32) {
        self.illegal_status |= MMU_ENTRY_FAULT;
        self.fault_content = (content & FAULT_CONTENT_MASK) | FAULT_CODE_CPU;
        self.fault_vaddr = vaddr;
    }

    /// Records a store to a flash window (`CORE0_ACS_CACHE_INT_ST`; CM §6.1).
    pub fn latch_store(&mut self, instruction_bus: bool) {
        self.access_status |= if instruction_bus {
            IBUS_WRITE
        } else {
            DBUS_WRITE
        };
    }
}

impl Default for Extmem {
    fn default() -> Extmem {
        Extmem::new()
    }
}

impl Peripheral for Extmem {
    fn read(&mut self, offset: u32) -> Option<u32> {
        Some(match offset {
            ICACHE_SYNC_CTRL => SYNC_DONE,
            ICACHE_PRELOAD_CTRL => PRELOAD_DONE | self.preload_order,
            ICACHE_AUTOLOAD_CTRL => AUTOLOAD_DONE | self.autoload,
            ICACHE_FREEZE => {
                let done = if self.freeze & FREEZE_ENA == 0 {
                    0
                } else {
                    FREEZE_DONE
                };
                self.freeze | done
            }
            CACHE_STATE => CACHE_IDLE,
            CACHE_ILG_INT_ST => self.illegal_status,
            CORE0_ACS_CACHE_INT_ST => self.access_status,
            CACHE_MMU_FAULT_CONTENT => self.fault_content,
            CACHE_MMU_FAULT_VADDR => self.fault_vaddr,
            // Write-only registers read 0.
            CACHE_ILG_INT_CLR | CORE0_ACS_CACHE_INT_CLR => 0,
            _ => return self.registers.read(offset),
        })
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            // The invalidation has nothing to invalidate; `SYNC_DONE` already reads set.
            ICACHE_SYNC_CTRL => {}
            ICACHE_PRELOAD_CTRL => self.preload_order = value & PRELOAD_ORDER,
            ICACHE_AUTOLOAD_CTRL => self.autoload = value & AUTOLOAD_CONFIG,
            ICACHE_FREEZE => self.freeze = value & (FREEZE_ENA | FREEZE_MODE),
            CACHE_ILG_INT_CLR => self.illegal_status &= !value,
            CORE0_ACS_CACHE_INT_CLR => self.access_status &= !value,
            ICACHE_CTRL | ICACHE_CTRL1 => {
                self.generation += 1;
                return self.registers.write(offset, value);
            }
            // Read-only: the write is reported as unmodelled.
            CACHE_STATE
            | CACHE_ILG_INT_ST
            | CORE0_ACS_CACHE_INT_ST
            | CACHE_MMU_FAULT_CONTENT
            | CACHE_MMU_FAULT_VADDR => return false,
            _ => return self.registers.write(offset, value),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(name: &str) -> u32 {
        svd_regs::esp32c3::addr("EXTMEM", name) - EXTMEM_BASE
    }

    #[test]
    fn reset_state_lets_every_rom_poll_terminate() {
        let mut x = Extmem::new();
        assert_eq!(x.read(reg("ICACHE_CTRL")), Some(0));
        assert_eq!(
            x.read(reg("ICACHE_CTRL1")),
            Some(3),
            "CM §2: both buses shut"
        );
        assert_eq!(x.read(reg("ICACHE_SYNC_CTRL")), Some(2), "decision D23");
        assert_eq!(
            x.read(reg("ICACHE_PRELOAD_CTRL")),
            Some(2),
            "Cache_Suspend_ICache spins on bit 1"
        );
        assert_eq!(
            x.read(reg("ICACHE_AUTOLOAD_CTRL")),
            Some(8),
            "Cache_Suspend_ICache_Autoload spins on bit 3"
        );
        assert_eq!(
            x.read(reg("CACHE_STATE")),
            Some(1),
            "Cache_Disable_ICache waits for exactly 1"
        );
        assert_eq!(x.read(reg("ICACHE_FREEZE")), Some(0));
    }

    #[test]
    fn invalidate_completes_at_once() {
        // CM §1.2: `R SYNC_CTRL=2 ; W SYNC_CTRL=3 ; R =2`, with SYNC_ADDR and SYNC_SIZE stored.
        let mut x = Extmem::new();
        assert!(x.write(reg("ICACHE_SYNC_ADDR"), 0x3C55_0000));
        assert!(x.write(reg("ICACHE_SYNC_SIZE"), 0x800));
        assert!(x.write(reg("ICACHE_SYNC_CTRL"), 3));
        assert_eq!(x.read(reg("ICACHE_SYNC_CTRL")), Some(2));
        assert_eq!(x.read(reg("ICACHE_SYNC_ADDR")), Some(0x3C55_0000));
        assert_eq!(x.read(reg("ICACHE_SYNC_SIZE")), Some(0x800));
    }

    #[test]
    fn freeze_done_follows_enable() {
        // FB §B.5: enable, poll DONE set; then the ROM writes back the value without bit 0, poll DONE clear.
        let mut x = Extmem::new();
        x.write(reg("ICACHE_FREEZE"), 1);
        let frozen = x.read(reg("ICACHE_FREEZE")).unwrap();
        assert_ne!(frozen & 4, 0);
        x.write(reg("ICACHE_FREEZE"), frozen & !1);
        assert_eq!(
            x.read(reg("ICACHE_FREEZE")),
            Some(0),
            "software cannot hold DONE"
        );
        x.write(reg("ICACHE_FREEZE"), 3);
        assert_eq!(x.read(reg("ICACHE_FREEZE")), Some(7), "MODE is stored");
    }

    #[test]
    fn autoload_enable_is_a_stored_bit_and_done_cannot_be_cleared() {
        let mut x = Extmem::new();
        x.write(reg("ICACHE_AUTOLOAD_CTRL"), 4);
        assert_eq!(x.read(reg("ICACHE_AUTOLOAD_CTRL")), Some(0xC));
        assert_eq!(
            x.read(reg("ICACHE_AUTOLOAD_CTRL")),
            Some(0xC),
            "a read does not clear ENA (CM §2)"
        );
        x.write(reg("ICACHE_AUTOLOAD_CTRL"), 0);
        assert_eq!(x.read(reg("ICACHE_AUTOLOAD_CTRL")), Some(8));
        x.write(reg("ICACHE_PRELOAD_CTRL"), 5);
        assert_eq!(
            x.read(reg("ICACHE_PRELOAD_CTRL")),
            Some(6),
            "ENA self-clears, ORDER kept, DONE set"
        );
    }

    #[test]
    fn configuration_is_stored_from_its_reset_value() {
        let mut x = Extmem::new();
        for (name, reset) in [
            ("CACHE_MMU_OWNER", 0),
            ("CACHE_WRAP_AROUND_CTRL", 0),
            ("ICACHE_TAG_POWER_CTRL", 5),
            ("CACHE_MMU_POWER_CTRL", 5),
            ("CACHE_ILG_INT_ENA", 0),
            ("CORE0_ACS_CACHE_INT_ENA", 0),
        ] {
            assert_eq!(x.read(reg(name)), Some(reset), "{name}");
            assert!(x.write(reg(name), 0xF));
            assert_eq!(x.read(reg(name)), Some(0xF), "{name}");
        }
    }

    #[test]
    fn bus_enable_writes_change_the_generation() {
        let mut x = Extmem::new();
        let before = x.generation();
        x.write(reg("ICACHE_CTRL"), 1);
        assert_ne!(x.generation(), before);
        let before = x.generation();
        x.write(reg("ICACHE_CTRL1"), 0);
        assert_ne!(x.generation(), before);
        let before = x.generation();
        x.write(reg("ICACHE_SYNC_CTRL"), 1);
        assert_eq!(x.generation(), before, "an invalidation maps nothing new");
    }

    #[test]
    fn an_invalid_mapping_is_latched_until_software_clears_it() {
        let mut x = Extmem::new();
        x.latch_mmu_fault(0x3C03_0010, 0x100);
        assert_eq!(
            x.read(reg("CACHE_ILG_INT_ST")),
            Some(1 << 5),
            "MMU_ENTRY_FAULT_ST (CM §6.1)"
        );
        assert_eq!(x.read(reg("CACHE_MMU_FAULT_VADDR")), Some(0x3C03_0010));
        assert_eq!(
            x.read(reg("CACHE_MMU_FAULT_CONTENT")),
            Some(0x100 | (1 << 10)),
            "content, code 1 = CPU miss"
        );
        assert!(
            x.write(reg("CACHE_ILG_INT_CLR"), 0xFFFF_FFFF),
            "Cache_Set_Default_Mode (FB §B.3)"
        );
        assert_eq!(x.read(reg("CACHE_ILG_INT_ST")), Some(0));
        assert!(!x.write(reg("CACHE_ILG_INT_ST"), 1), "status is read-only");
    }

    #[test]
    fn a_store_to_a_window_is_latched_per_bus() {
        let mut x = Extmem::new();
        x.latch_store(false);
        assert_eq!(
            x.read(reg("CORE0_ACS_CACHE_INT_ST")),
            Some(1 << 5),
            "DBUS_WR_ICACHE_ST"
        );
        x.latch_store(true);
        assert_eq!(
            x.read(reg("CORE0_ACS_CACHE_INT_ST")),
            Some((1 << 5) | (1 << 1)),
            "IBUS_WR_ICACHE_ST"
        );
        x.write(reg("CORE0_ACS_CACHE_INT_CLR"), 0x3F);
        assert_eq!(x.read(reg("CORE0_ACS_CACHE_INT_ST")), Some(0));
    }

    #[test]
    fn registers_no_measured_path_touches_stay_unmodelled() {
        let mut x = Extmem::new();
        for name in [
            "ICACHE_LOCK_CTRL",
            "ICACHE_PRELOAD_ADDR",
            "IBUS_PMS_TBL_LOCK",
            "CACHE_CONF_MISC",
            "REG_DATE",
        ] {
            assert_eq!(x.read(reg(name)), None, "{name}");
            assert!(!x.write(reg(name), 1), "{name}");
        }
    }
}
