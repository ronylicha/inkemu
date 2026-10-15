//! `EXTMEM`: the flash-cache controller. Source: report CM §2 and §10 (ESP-IDF `extmem_reg.h`, the
//! ROM cache functions), FB §B.3 and §B.5.
//!
//! No cache line is modelled: the bus reads the flash through the MMU table at every access
//! (decision D4). The operations software waits for (invalidate, preload, autoload, freeze)
//! therefore complete at once, and the registers below only keep what software reads back.
//!
//! One thing a cache does is still followed, because it is where reading the flash live can differ
//! from a device: a line filled from the flash keeps its bytes when SPI1 programs or erases them,
//! until software invalidates it (CM §3, "Stale data"). `Extmem::window_read`,
//! `Extmem::flash_written` and `Extmem::invalidate` keep, per 64 KiB flash page, whether a
//! window read may now return other bytes than a device would. The bus reports such a read; it
//! still returns the bytes of the flash.
use super::mmu::{MMU_ENTRIES, MMU_PAGE_BYTES, MmuTable};
use super::{Accepted, EXTMEM_BASE, Storage};
use crate::ctx::{Ctx, Event, Source};
use crate::memmap::{FLASH_DBUS_BASE, FLASH_IBUS_BASE, FLASH_WINDOW_SIZE};
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("EXTMEM", name) - EXTMEM_BASE
}

const ICACHE_CTRL: u32 = reg("ICACHE_CTRL");
const ICACHE_CTRL1: u32 = reg("ICACHE_CTRL1");
const ICACHE_SYNC_CTRL: u32 = reg("ICACHE_SYNC_CTRL");
const ICACHE_SYNC_ADDR: u32 = reg("ICACHE_SYNC_ADDR");
const ICACHE_SYNC_SIZE: u32 = reg("ICACHE_SYNC_SIZE");
const ICACHE_PRELOAD_CTRL: u32 = reg("ICACHE_PRELOAD_CTRL");
const ICACHE_AUTOLOAD_CTRL: u32 = reg("ICACHE_AUTOLOAD_CTRL");
const ICACHE_FREEZE: u32 = reg("ICACHE_FREEZE");
const CACHE_STATE: u32 = reg("CACHE_STATE");
const CACHE_ILG_INT_ENA: u32 = reg("CACHE_ILG_INT_ENA");
const CACHE_ILG_INT_CLR: u32 = reg("CACHE_ILG_INT_CLR");
const CACHE_ILG_INT_ST: u32 = reg("CACHE_ILG_INT_ST");
const CORE0_ACS_CACHE_INT_ENA: u32 = reg("CORE0_ACS_CACHE_INT_ENA");
const CORE0_ACS_CACHE_INT_CLR: u32 = reg("CORE0_ACS_CACHE_INT_CLR");
const CORE0_ACS_CACHE_INT_ST: u32 = reg("CORE0_ACS_CACHE_INT_ST");
const CACHE_MMU_FAULT_CONTENT: u32 = reg("CACHE_MMU_FAULT_CONTENT");
const CACHE_MMU_FAULT_VADDR: u32 = reg("CACHE_MMU_FAULT_VADDR");

/// `ICACHE_SYNC_CTRL` bit 1, `SYNC_DONE`. ASSUMPTION: (decision D23) it reads set from reset. The
/// SVD gives reset 1 (`INVALIDATE_ENA` set, `DONE` clear), which reads as a tag initialisation that
/// ends before the ROM runs; the ROM sets bit 0 and waits for bit 1 either way (CM §9 item 5).
const SYNC_DONE: u32 = 1 << 1;
/// `ICACHE_SYNC_CTRL` bit 0, `INVALIDATE_ENA`: starts the invalidation of the range in
/// `ICACHE_SYNC_ADDR` and `ICACHE_SYNC_SIZE` (`ROM3:Cache_Invalidate_ICache_Items@0x40050f28`:
/// `SYNC_ADDR = addr`, `SYNC_SIZE[22:0] = n`, `SYNC_CTRL |= 1`; CM §4.1).
const SYNC_INVALIDATE: u32 = 1 << 0;
/// `ICACHE_SYNC_SIZE` `[22:0]`: the number of cache lines, not of bytes. `Cache_Invalidate_Addr`
/// called for `0x10000` bytes writes `0x800` (CM §4.1, measured).
const SYNC_LINES_MASK: u32 = 0x7F_FFFF;
/// A cache line (`ROM3:Cache_Get_ICache_Line_Size@0x40050e4c` returns 32; CM §3).
const LINE_BYTES: u64 = 32;
/// The 64 KiB pages of the largest flash an MMU entry can name: its page number has 8 bits.
const FLASH_PAGES: u32 = 256;
const PAGE_SET_WORDS: usize = 4;
const WORD_BITS: u32 = u64::BITS;

/// A set of 64 KiB flash pages, one bit each: constant size whatever the firmware does. The bus
/// keeps one too, of the pages the CPU fetched an instruction from (`SocBus::executed`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PageSet([u64; PAGE_SET_WORDS]);

impl PageSet {
    /// The word that holds `page` and the bit of the page in it; `None` past the 256 pages an
    /// MMU entry can name, so that a page number out of range is in no set and changes none.
    fn slot(&mut self, page: u32) -> Option<(&mut u64, u64)> {
        let word = self.0.get_mut(usize::try_from(page / WORD_BITS).ok()?)?;
        Some((word, 1 << (page % WORD_BITS)))
    }

    pub(crate) fn contains(&self, page: u32) -> bool {
        usize::try_from(page / WORD_BITS)
            .ok()
            .and_then(|word| self.0.get(word))
            .is_some_and(|word| word & (1 << (page % WORD_BITS)) != 0)
    }

    pub(crate) fn insert(&mut self, page: u32) {
        if let Some((word, bit)) = self.slot(page) {
            *word |= bit;
        }
    }

    /// Takes `page` out of the set; whether it was in it.
    pub(crate) fn remove(&mut self, page: u32) -> bool {
        self.slot(page).is_some_and(|(word, bit)| {
            let held = *word & bit != 0;
            *word &= !bit;
            held
        })
    }

    fn is_empty(&self) -> bool {
        self.0 == [0; PAGE_SET_WORDS]
    }
}

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
/// `CACHE_STATE` `[11:0]`: 1 is idle, the only value `Cache_Disable_ICache` accepts.
const CACHE_IDLE: u32 = 1;
/// `CACHE_ILG_INT_ST` bit 5: an access went through an invalid MMU entry.
const MMU_ENTRY_FAULT: u32 = 1 << 5;
/// `CACHE_MMU_FAULT_CONTENT`: `[9:0]` the entry, `[13:10]` the operation; 1 is a CPU access.
const FAULT_CONTENT_MASK: u32 = 0x3FF;
const FAULT_CODE_CPU: u32 = 1 << 10;
/// `CORE0_ACS_CACHE_INT_ST`: bit 1 a store through the instruction bus, bit 5 through the data bus.
const IBUS_WRITE: u32 = 1 << 1;
const DBUS_WRITE: u32 = 1 << 5;

/// The two cache error interrupts: the status bits a line is enabled for drive it (CM §2: "gated
/// by `ILG_INT_ENA` for the interrupt line (source 36)", and source 61 for the access errors). The
/// application enables them with `0x23` and `0x3F` and routes both sources to CPU line 25
/// (`esp_cache_err_int_init`, `IDF:esp_system/port/soc/esp32c3/cache_err_int.c`; report IR §3.2).
const ILLEGAL_SOURCE: Source = Source::of("CACHE_IA_INT_MAP");
const ACCESS_SOURCE: Source = Source::of("CACHE_CORE0_ACS_INT_MAP");
/// The bits of the two enable registers (CM §2): `CACHE_ILG_INT_ENA` 0, 1, 5, 7 and 8,
/// `CORE0_ACS_CACHE_INT_ENA` 0 to 5.
const ILLEGAL_BITS: u32 = 0x1A3;
const ACCESS_BITS: u32 = 0x3F;
/// Which `CACHE_ILG_INT_ST` bits each `CACHE_ILG_INT_ENA` bit enables, as (enable mask, status
/// mask). The two registers do not line up bit for bit: in `extmem_reg.h` (ESP-IDF v5.5.2) enable
/// bit 7 (`IBUS_CNT_OVF_INT_ENA`) is the one for status bits 7 (`IBUS_ACS_CNT_OVF_ST`) and 8
/// (`IBUS_ACS_MISS_CNT_OVF_ST`), enable bit 8 (`DBUS_CNT_OVF_INT_ENA`) the one for status bits 9
/// (`DBUS_ACS_CNT_OVF_ST`) and 10 (`DBUS_ACS_FLASH_MISS_CNT_OVF_ST`); bits 0
/// (`ICACHE_SYNC_OP_FAULT`), 1 (`ICACHE_PRELOAD_OP_FAULT`) and 5 (`MMU_ENTRY_FAULT`) pair with
/// themselves. ASSUMPTION: the pairing of the counter overflow bits follows from the field
/// names and the layout of the header alone; the register description of the TRM does not state
/// it. The model latches bit 5 only (the counters are not modelled), so only that pair is
/// reachable by software today.
const ILLEGAL_GATES: [(u32, u32); 5] = [
    (1 << 0, 1 << 0),
    (1 << 1, 1 << 1),
    (1 << 5, 1 << 5),
    (1 << 7, (1 << 7) | (1 << 8)),
    (1 << 8, (1 << 9) | (1 << 10)),
];

const WHY_OPERATION: &str = "cache operation: completes at once, there are no cache lines";
const WHY_INVALIDATE: &str = "cache invalidation: completes at once, there are no cache lines; it only ends the stale-read warning of the flash pages it covers";
const ACCEPTED: [Accepted; 12] = [
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
    Accepted::svd("EXTMEM", "ICACHE_SYNC_CTRL", WHY_INVALIDATE, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_SYNC_ADDR", WHY_INVALIDATE, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_SYNC_SIZE", WHY_INVALIDATE, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_PRELOAD_CTRL", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_AUTOLOAD_CTRL", WHY_OPERATION, "SP1b-4"),
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
    illegal_enable: u32,
    access_status: u32,
    access_enable: u32,
    fault_content: u32,
    fault_vaddr: u32,
    /// See [`Extmem::generation`]. A version does not saturate (it must move at every change)
    /// and cannot overflow: it moves at most once per register write, one instruction at least.
    generation: u64,
    /// The flash pages read through a window since the cache was last invalidated for them: the
    /// pages a device may hold lines of.
    ///
    /// INVARIANT: a page whose code the CPU keeps decoded is in the set. The CPU runs decoded
    /// code without a bus access, so the bus sees a page read once however often its code runs;
    /// that is enough only while the page stays in the set. [`Extmem::invalidate`] therefore
    /// moves the generation, which drops the decoded code, whenever it takes a page out.
    ///
    /// What a reset does to this set and to `stale`: they stand for the lines of the cache, so
    /// they follow the cache.
    /// - A CPU reset keeps both. It resets the CPU core and the permission registers only: the
    ///   registers of `EXTMEM` keep their values, and the internal memory, where the lines are
    ///   (CM §3: the cache occupies SRAM0), keeps its content (TRM §6.1.1, §6.1.3; report IR
    ///   §4.2). A page read and programmed before a CPU reset is still stale after it, until
    ///   the cache is invalidated for it.
    /// - A core or a system reset empties both, with the other registers of `EXTMEM`
    ///   (`Peripherals::reset`). ASSUMPTION: no line is valid after a reset of the digital
    ///   system. The memory of the lines keeps its content at such a reset too (TRM §6.1.1);
    ///   that their tags are cleared is read from the reset value the SVD gives
    ///   `ICACHE_SYNC_CTRL`, `INVALIDATE_ENA` set (the assumption on `SYNC_DONE`). No
    ///   firmware the ROM starts can tell: at every boot the ROM invalidates every line before
    ///   it reads a window (`ROM_Boot_Cache_Init` calls `Cache_Set_Default_Mode`, which calls
    ///   `Cache_Invalidate_ICache_All`; CM §4.1, §5.1). A debugger settles it: read a window,
    ///   program what it showed, reset the core, halt at the reset vector, enable the cache
    ///   without the ROM and read the window again.
    cached: PageSet,
    /// Those of `cached` a program or an erase has changed since: a device may still answer a
    /// window read of them with the bytes of before.
    stale: PageSet,
}

impl Extmem {
    #[must_use]
    pub fn new() -> Extmem {
        Extmem {
            cached: PageSet::default(),
            stale: PageSet::default(),
            registers: Storage::accepted(EXTMEM_BASE, &ACCEPTED),
            preload_order: 0,
            autoload: 0,
            freeze: 0,
            illegal_status: 0,
            illegal_enable: 0,
            access_status: 0,
            access_enable: 0,
            fault_content: 0,
            fault_vaddr: 0,
            generation: 0,
        }
    }

    /// The version of the code the CPU decoded through the flash windows, as far as the cache
    /// decides it. It changes when the cache or one of its buses is switched on or off (decision
    /// D6), and when an invalidation takes a page out of the cache: the code of that page is
    /// then fetched again the next time it runs, as a device fetches it from the flash again,
    /// and the fetch is a window read like any other (`Extmem::window_read`).
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// A read through a flash window reached the 64 KiB flash page `page`. Returns whether a
    /// device may answer with other bytes than the flash now holds: the page was read through a
    /// window, then programmed or erased, and the cache was not invalidated for it since.
    ///
    /// Constant cost: one bit set and one bit tested. The bus calls it once per flash page of an
    /// access, so twice at most for an access that straddles two pages. A page number past the
    /// 256 an MMU entry can name is in no set.
    pub(crate) fn window_read(&mut self, page: u32) -> bool {
        self.cached.insert(page);
        self.stale.contains(page)
    }

    /// SPI1 changed bytes of the flash pages `first_page` to `last_page`. The pages a window read
    /// since their last invalidation become stale; a page no window read is in no cache line.
    ///
    /// The two numbers come from an address the firmware wrote: the loop is bounded by
    /// [`FLASH_PAGES`], whatever they are ([`written_pages`]).
    pub(crate) fn flash_written(&mut self, first_page: u32, last_page: u32) {
        for page in written_pages(first_page, last_page) {
            if self.cached.contains(page) {
                self.stale.insert(page);
            }
        }
    }

    /// Software invalidated the cache lines of the range in `ICACHE_SYNC_ADDR` and
    /// `ICACHE_SYNC_SIZE`: the flash pages the range covers are in no line any more.
    ///
    /// - Address 0 with size 0 is every line (`ROM3:Cache_Invalidate_ICache_All@0x40051126` is
    ///   `Cache_Invalidate_ICache_Items(0, 0)`; `Cache_Disable_ICache` and
    ///   `Cache_Set_Default_Mode` call it; CM §4.1).
    /// - Otherwise the range is `size` lines of 32 bytes from an address of one of the two
    ///   windows, and names the MMU entries that lie whole in it. A page is dropped when a whole
    ///   entry that maps it lies in the range, as `spi_flash_check_and_flush_cache` asks
    ///   (`IDF:components/spi_flash/flash_mmap.c:306-321`: `cache_hal_invalidate_addr(vaddr,
    ///   SPI_FLASH_MMU_PAGE_SIZE)`). A range that covers part of an entry drops nothing: the model
    ///   keeps one bit per page, and the lines left in the cache would still be stale.
    ///
    /// ASSUMPTION: a size of 0 at another address than 0, and an address outside the two windows,
    /// invalidate nothing. No ROM or ESP-IDF function issues either (`Cache_Invalidate_Addr`
    /// returns 1 before it writes a register when the range is not inside one window, CM §4.1).
    /// Reading a window, programming it, issuing such a request and reading again on a device
    /// settles both.
    ///
    /// ASSUMPTION: a request drops the lines of a flash page whichever address they were filled
    /// through: an entry named at its address in one window drops what was read at its address
    /// in the other, and what was read through another entry that maps the same page. The model
    /// keeps one bit per flash page; a cache keeps lines, and what a line is tagged with is not
    /// established. CM §1 and §3 establish one MMU table and one cache memory for both buses,
    /// no more. ESP-IDF names one address per page, in the window the mapping is used through
    /// (`is_page_mapped_in_cache`, `IDF:components/spi_flash/flash_mmap.c:276-293`: the
    /// instruction window for a mapping that can execute, the data window otherwise), which
    /// shows nothing of the lines filled through the other address. If a line belongs to its
    /// address, a device keeps the lines of the address the request did not name, and a read
    /// there is stale on the device while nothing is reported here. Settled on a device: read
    /// a page at its `0x3C…` address, program it, invalidate its entry at its `0x42…` address
    /// only, read at `0x3C…` again; the same with two entries that map one page.
    ///
    /// ASSUMPTION: the lines a request names start at the address as `ICACHE_SYNC_ADDR` holds
    /// it, so a request that starts past the first byte of an entry leaves that entry out, even
    /// by one byte. What the cache does with the low 5 bits of the address, those inside a
    /// line, is not established: `Cache_Op_Addr` aligns the address to a line before it writes
    /// the register (CM §4.1), so no ROM or ESP-IDF path shows it. If silicon rounds the address
    /// down to its line, a request one byte into an entry for the `0x800` lines of a page drops
    /// that page on a device and nothing here: a read after it is then reported though the
    /// device reads the flash. Settled on a device: read a page, program it, invalidate `0x800`
    /// lines from one byte past the start of its entry, read again.
    ///
    /// A request that takes a page out of the cache moves [`Extmem::generation`]: the CPU may
    /// hold decoded code of that page, which it would go on running without the bus seeing a
    /// read, while a device fills the line again at the next fetch. The cost is one decode of
    /// the flash code that runs next; a request that drops nothing costs nothing.
    ///
    /// The loop is bounded by the 128 entries of the table, whatever the two registers hold
    /// ([`whole_entries`]).
    pub(crate) fn invalidate(&mut self, mmu: &MmuTable) {
        let addr = self.registers.read(ICACHE_SYNC_ADDR).unwrap_or(0);
        let lines = self.registers.read(ICACHE_SYNC_SIZE).unwrap_or(0) & SYNC_LINES_MASK;
        if lines == 0 {
            if addr == 0 {
                if !self.cached.is_empty() {
                    self.generation += 1;
                }
                self.cached = PageSet::default();
                self.stale = PageSet::default();
            }
            return;
        }
        let Some(start) = [FLASH_DBUS_BASE, FLASH_IBUS_BASE]
            .into_iter()
            .filter_map(|base| addr.checked_sub(base))
            .find(|offset| *offset < FLASH_WINDOW_SIZE)
        else {
            return;
        };
        let mut dropped = false;
        for entry in whole_entries(start, lines) {
            if let Some(page) = mmu.flash_page(entry) {
                dropped |= self.cached.remove(page);
                self.stale.remove(page);
            }
        }
        if dropped {
            self.generation += 1;
        }
    }

    /// Records an access through an invalid MMU entry (`CACHE_ILG_INT_ST`, `CACHE_MMU_FAULT_*`; CM §6.1).
    pub(crate) fn latch_mmu_fault(&mut self, ctx: &mut Ctx, vaddr: u32, content: u32) {
        self.illegal_status |= MMU_ENTRY_FAULT;
        self.fault_content = (content & FAULT_CONTENT_MASK) | FAULT_CODE_CPU;
        self.fault_vaddr = vaddr;
        self.drive(ctx);
    }

    /// Records a store to a flash window (`CORE0_ACS_CACHE_INT_ST`; CM §6.1).
    pub(crate) fn latch_store(&mut self, ctx: &mut Ctx, instruction_bus: bool) {
        self.access_status |= if instruction_bus {
            IBUS_WRITE
        } else {
            DBUS_WRITE
        };
        self.drive(ctx);
    }

    /// Each cache error interrupt is a level: a latched status bit whose enable bit is set.
    fn drive(&self, ctx: &mut Ctx) {
        let illegal = ILLEGAL_GATES.iter().any(|&(enable, status)| {
            self.illegal_enable & enable != 0 && self.illegal_status & status != 0
        });
        ctx.irq(ILLEGAL_SOURCE, illegal);
        // `CORE0_ACS_CACHE_INT_ENA` and `_ST` line up bit for bit, 0 to 5 (`extmem_reg.h`,
        // ESP-IDF v5.5.2: `CORE0_IBUS_ACS_MSK_IC_INT_ENA` and `CORE0_IBUS_ACS_MSK_ICACHE_ST` at
        // bit 0, `..._IBUS_WR_IC_INT_ENA` and `..._IBUS_WR_ICACHE_ST` at bit 1, and so on to
        // `CORE0_DBUS_WR_IC_INT_ENA` and `CORE0_DBUS_WR_ICACHE_ST` at bit 5).
        ctx.irq(
            ACCESS_SOURCE,
            self.access_status & self.access_enable & ACCESS_BITS != 0,
        );
    }
}

/// The flash pages a write of pages `first_page` to `last_page` reaches: those an MMU entry can
/// name, so at most the [`FLASH_PAGES`] of the largest flash, whatever the two numbers are.
pub(crate) fn written_pages(first_page: u32, last_page: u32) -> std::ops::RangeInclusive<u32> {
    first_page..=last_page.min(FLASH_PAGES - 1)
}

/// The MMU entries that lie whole in `lines` cache lines from `start` bytes into a window, at
/// most the [`MMU_ENTRIES`] of the table: the largest request, `0x7FFFFF` lines, covers 4,095
/// entries from its start, and only those of the table exist.
fn whole_entries(start: u32, lines: u32) -> std::ops::Range<usize> {
    let page_bytes = u64::from(MMU_PAGE_BYTES);
    let end = u64::from(start) + u64::from(lines) * LINE_BYTES;
    let entry =
        |bytes: u64| usize::try_from(bytes).map_or(MMU_ENTRIES, |entry| entry.min(MMU_ENTRIES));
    entry(u64::from(start).div_ceil(page_bytes))..entry(end / page_bytes)
}

impl Default for Extmem {
    fn default() -> Extmem {
        Extmem::new()
    }
}

impl Peripheral<Ctx> for Extmem {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
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
            CACHE_ILG_INT_ENA => self.illegal_enable,
            CORE0_ACS_CACHE_INT_ST => self.access_status,
            CORE0_ACS_CACHE_INT_ENA => self.access_enable,
            CACHE_MMU_FAULT_CONTENT => self.fault_content,
            CACHE_MMU_FAULT_VADDR => self.fault_vaddr,
            // Write-only registers read 0.
            CACHE_ILG_INT_CLR | CORE0_ACS_CACHE_INT_CLR => 0,
            _ => return self.registers.read(offset),
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        match offset {
            // There is no line to invalidate and `SYNC_DONE` already reads set. What the request
            // ends is the stale state of the pages it covers; which pages those are is told by
            // the MMU table, another device, so the request comes back as an event
            // (`Extmem::invalidate`).
            ICACHE_SYNC_CTRL => {
                if value & SYNC_INVALIDATE != 0 {
                    ctx.signal(Event::CacheInvalidated);
                }
            }
            ICACHE_PRELOAD_CTRL => self.preload_order = value & PRELOAD_ORDER,
            ICACHE_AUTOLOAD_CTRL => self.autoload = value & AUTOLOAD_CONFIG,
            ICACHE_FREEZE => self.freeze = value & (FREEZE_ENA | FREEZE_MODE),
            CACHE_ILG_INT_ENA => self.illegal_enable = value & ILLEGAL_BITS,
            // ASSUMPTION: the clear register clears the status bit for bit: bit n written 1
            // clears bit n of `CACHE_ILG_INT_ST`. It may instead be laid out as the enable
            // register is, one bit for the two overflow status bits of each bus
            // (`ILLEGAL_GATES`); the field names of `extmem_reg.h` do not settle it. This model
            // never sets status bits 7 to 10 (the access counters are not modelled), so the two
            // readings cannot differ today. A counter overflow on a chip, then a write of each
            // clear bit with `CACHE_ILG_INT_ST` read after it, settles it.
            CACHE_ILG_INT_CLR => self.illegal_status &= !value,
            CORE0_ACS_CACHE_INT_ENA => self.access_enable = value & ACCESS_BITS,
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
        self.drive(ctx);
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
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        assert_eq!(x.read(ctx, reg("ICACHE_CTRL")), Some(0));
        assert_eq!(
            x.read(ctx, reg("ICACHE_CTRL1")),
            Some(3),
            "CM §2: both buses shut"
        );
        assert_eq!(
            x.read(ctx, reg("ICACHE_SYNC_CTRL")),
            Some(2),
            "decision D23"
        );
        assert_eq!(
            x.read(ctx, reg("ICACHE_PRELOAD_CTRL")),
            Some(2),
            "Cache_Suspend_ICache spins on bit 1"
        );
        assert_eq!(
            x.read(ctx, reg("ICACHE_AUTOLOAD_CTRL")),
            Some(8),
            "Cache_Suspend_ICache_Autoload spins on bit 3"
        );
        assert_eq!(
            x.read(ctx, reg("CACHE_STATE")),
            Some(1),
            "Cache_Disable_ICache waits for exactly 1"
        );
        assert_eq!(x.read(ctx, reg("ICACHE_FREEZE")), Some(0));
    }

    #[test]
    fn invalidate_completes_at_once() {
        // CM §1.2: `R SYNC_CTRL=2 ; W SYNC_CTRL=3 ; R =2`, with SYNC_ADDR and SYNC_SIZE stored.
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        assert!(x.write(ctx, reg("ICACHE_SYNC_ADDR"), 0x3C55_0000));
        assert!(x.write(ctx, reg("ICACHE_SYNC_SIZE"), 0x800));
        assert!(x.write(ctx, reg("ICACHE_SYNC_CTRL"), 3));
        assert_eq!(x.read(ctx, reg("ICACHE_SYNC_CTRL")), Some(2));
        assert_eq!(x.read(ctx, reg("ICACHE_SYNC_ADDR")), Some(0x3C55_0000));
        assert_eq!(x.read(ctx, reg("ICACHE_SYNC_SIZE")), Some(0x800));
    }

    #[test]
    fn operation_registers_keep_their_configuration_bits_and_answer_done_whatever_is_written() {
        let ctx = &mut Ctx::default();
        // (register, value written, value read back): the enable and done bits written together,
        // then every bit set. A `DONE` bit is the model's answer, never what software wrote.
        for (name, written, read) in [
            ("ICACHE_SYNC_CTRL", 0x3, SYNC_DONE),
            ("ICACHE_SYNC_CTRL", u32::MAX, SYNC_DONE),
            ("ICACHE_SYNC_CTRL", 0, SYNC_DONE),
            ("ICACHE_PRELOAD_CTRL", 0x3, PRELOAD_DONE),
            (
                "ICACHE_PRELOAD_CTRL",
                u32::MAX,
                PRELOAD_DONE | PRELOAD_ORDER,
            ),
            ("ICACHE_PRELOAD_CTRL", 0, PRELOAD_DONE),
            (
                "ICACHE_FREEZE",
                FREEZE_ENA | FREEZE_DONE,
                FREEZE_ENA | FREEZE_DONE,
            ),
            (
                "ICACHE_FREEZE",
                u32::MAX,
                FREEZE_ENA | FREEZE_MODE | FREEZE_DONE,
            ),
            ("ICACHE_FREEZE", FREEZE_MODE | FREEZE_DONE, FREEZE_MODE),
            ("ICACHE_FREEZE", 0, 0),
        ] {
            let mut x = Extmem::new();
            assert!(x.write(ctx, reg(name), written), "{name} <- {written:#x}");
            assert_eq!(x.read(ctx, reg(name)), Some(read), "{name} <- {written:#x}");
        }
    }

    #[test]
    fn freeze_done_follows_enable() {
        // FB §B.5: enable, poll DONE set; then the ROM writes back the value without bit 0, poll DONE clear.
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        x.write(ctx, reg("ICACHE_FREEZE"), 1);
        let frozen = x.read(ctx, reg("ICACHE_FREEZE")).unwrap();
        assert_ne!(frozen & 4, 0);
        x.write(ctx, reg("ICACHE_FREEZE"), frozen & !1);
        assert_eq!(
            x.read(ctx, reg("ICACHE_FREEZE")),
            Some(0),
            "software cannot hold DONE"
        );
        x.write(ctx, reg("ICACHE_FREEZE"), 3);
        assert_eq!(x.read(ctx, reg("ICACHE_FREEZE")), Some(7), "MODE is stored");
    }

    #[test]
    fn autoload_enable_is_a_stored_bit_and_done_cannot_be_cleared() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        x.write(ctx, reg("ICACHE_AUTOLOAD_CTRL"), 4);
        assert_eq!(x.read(ctx, reg("ICACHE_AUTOLOAD_CTRL")), Some(0xC));
        assert_eq!(
            x.read(ctx, reg("ICACHE_AUTOLOAD_CTRL")),
            Some(0xC),
            "a read does not clear ENA (CM §2)"
        );
        x.write(ctx, reg("ICACHE_AUTOLOAD_CTRL"), 0);
        assert_eq!(x.read(ctx, reg("ICACHE_AUTOLOAD_CTRL")), Some(8));
        x.write(ctx, reg("ICACHE_PRELOAD_CTRL"), 5);
        assert_eq!(
            x.read(ctx, reg("ICACHE_PRELOAD_CTRL")),
            Some(6),
            "ENA self-clears, ORDER kept, DONE set"
        );
    }

    #[test]
    fn configuration_is_stored_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        for (name, reset) in [
            ("CACHE_MMU_OWNER", 0),
            ("CACHE_WRAP_AROUND_CTRL", 0),
            ("ICACHE_TAG_POWER_CTRL", 5),
            ("CACHE_MMU_POWER_CTRL", 5),
        ] {
            assert_eq!(x.read(ctx, reg(name)), Some(reset), "{name}");
            assert!(x.write(ctx, reg(name), 0xF));
            assert_eq!(x.read(ctx, reg(name)), Some(0xF), "{name}");
        }
    }

    #[test]
    fn bus_enable_writes_change_the_generation() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        let before = x.generation();
        x.write(ctx, reg("ICACHE_CTRL"), 1);
        assert_ne!(x.generation(), before);
        let before = x.generation();
        x.write(ctx, reg("ICACHE_CTRL1"), 0);
        assert_ne!(x.generation(), before);
        // The registers of a request are no switch: what a request drops is decided when it is
        // delivered (`an_invalidation_changes_the_generation_when_it_takes_a_page_out...`).
        let before = x.generation();
        x.write(ctx, reg("ICACHE_SYNC_ADDR"), FLASH_IBUS_BASE);
        x.write(ctx, reg("ICACHE_SYNC_SIZE"), 0x800);
        x.write(ctx, reg("ICACHE_SYNC_CTRL"), 1);
        assert_eq!(x.generation(), before);
    }

    #[test]
    fn an_invalid_mapping_is_latched_until_software_clears_it() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        x.latch_mmu_fault(ctx, 0x3C03_0010, 0x100);
        assert_eq!(
            x.read(ctx, reg("CACHE_ILG_INT_ST")),
            Some(1 << 5),
            "MMU_ENTRY_FAULT_ST (CM §6.1)"
        );
        assert_eq!(x.read(ctx, reg("CACHE_MMU_FAULT_VADDR")), Some(0x3C03_0010));
        assert_eq!(
            x.read(ctx, reg("CACHE_MMU_FAULT_CONTENT")),
            Some(0x100 | (1 << 10)),
            "content, code 1 = CPU miss"
        );
        assert!(
            x.write(ctx, reg("CACHE_ILG_INT_CLR"), 0xFFFF_FFFF),
            "Cache_Set_Default_Mode (FB §B.3)"
        );
        assert_eq!(x.read(ctx, reg("CACHE_ILG_INT_ST")), Some(0));
        assert!(
            !x.write(ctx, reg("CACHE_ILG_INT_ST"), 1),
            "status is read-only"
        );
    }

    #[test]
    fn a_store_to_a_window_is_latched_per_bus() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        x.latch_store(ctx, false);
        assert_eq!(
            x.read(ctx, reg("CORE0_ACS_CACHE_INT_ST")),
            Some(1 << 5),
            "DBUS_WR_ICACHE_ST"
        );
        x.latch_store(ctx, true);
        assert_eq!(
            x.read(ctx, reg("CORE0_ACS_CACHE_INT_ST")),
            Some((1 << 5) | (1 << 1)),
            "IBUS_WR_ICACHE_ST"
        );
        x.write(ctx, reg("CORE0_ACS_CACHE_INT_CLR"), 0x3F);
        assert_eq!(x.read(ctx, reg("CORE0_ACS_CACHE_INT_ST")), Some(0));
    }

    /// Whether the source is high in the context.
    fn level(ctx: &Ctx, source: Source) -> bool {
        ctx.sources() & (1 << source.number()) != 0
    }

    #[test]
    fn the_cache_error_enables_keep_the_bits_the_header_defines() {
        // `esp_cache_err_int_init` writes 0x23 and 0x3F; bits no field defines are not stored.
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        for (name, written, read) in [
            ("CACHE_ILG_INT_ENA", 0, 0),
            ("CACHE_ILG_INT_ENA", 0x23, 0x23),
            ("CACHE_ILG_INT_ENA", u32::MAX, ILLEGAL_BITS),
            ("CORE0_ACS_CACHE_INT_ENA", 0, 0),
            ("CORE0_ACS_CACHE_INT_ENA", 0x3F, 0x3F),
            ("CORE0_ACS_CACHE_INT_ENA", u32::MAX, ACCESS_BITS),
        ] {
            assert!(x.write(ctx, reg(name), written), "{name} <- {written:#x}");
            assert_eq!(x.read(ctx, reg(name)), Some(read), "{name} <- {written:#x}");
        }
    }

    #[test]
    fn an_invalid_mapping_raises_source_36_only_while_it_is_enabled() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        x.latch_mmu_fault(ctx, 0x3C03_0010, 0x100);
        assert!(!level(ctx, ILLEGAL_SOURCE), "latched, not enabled");
        assert!(x.write(ctx, reg("CACHE_ILG_INT_ENA"), 0x23));
        assert!(
            level(ctx, ILLEGAL_SOURCE),
            "enabling a latched status raises"
        );
        assert!(x.write(ctx, reg("CACHE_ILG_INT_ENA"), 0));
        assert!(!level(ctx, ILLEGAL_SOURCE), "disabling drops the level");
        assert!(x.write(ctx, reg("CACHE_ILG_INT_ENA"), 0x23));
        assert!(x.write(ctx, reg("CACHE_ILG_INT_CLR"), MMU_ENTRY_FAULT));
        assert!(!level(ctx, ILLEGAL_SOURCE), "clearing the status drops it");
        x.latch_mmu_fault(ctx, 0x3C03_0010, 0x100);
        assert!(level(ctx, ILLEGAL_SOURCE), "a new fault raises it again");
        assert!(
            !level(ctx, ACCESS_SOURCE),
            "the access errors are another source"
        );
    }

    #[test]
    fn a_store_to_a_window_raises_source_61_only_while_it_is_enabled() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        x.latch_store(ctx, true);
        assert!(!level(ctx, ACCESS_SOURCE), "latched, not enabled");
        assert!(x.write(ctx, reg("CORE0_ACS_CACHE_INT_ENA"), 0x3F));
        assert!(level(ctx, ACCESS_SOURCE));
        assert!(
            !level(ctx, ILLEGAL_SOURCE),
            "the illegal operations are another source"
        );
        assert!(x.write(ctx, reg("CORE0_ACS_CACHE_INT_CLR"), 0x3F));
        assert!(!level(ctx, ACCESS_SOURCE));
        // Only the enabled bus counts: ENA bit 5 (data bus) does not gate the instruction bus.
        assert!(x.write(ctx, reg("CORE0_ACS_CACHE_INT_ENA"), DBUS_WRITE));
        x.latch_store(ctx, true);
        assert!(
            !level(ctx, ACCESS_SOURCE),
            "IBUS_WR_ICACHE_ST is gated by bit 1"
        );
        x.latch_store(ctx, false);
        assert!(
            level(ctx, ACCESS_SOURCE),
            "DBUS_WR_ICACHE_ST is gated by bit 5"
        );
    }

    #[test]
    fn each_illegal_operation_status_bit_is_gated_by_the_enable_bit_the_header_pairs_it_with() {
        // IDF `extmem_reg.h` (v5.5.2): ENA bit 7 (IBUS_CNT_OVF) gates ST bits 7 (IBUS_ACS_CNT_OVF)
        // and 8 (IBUS_ACS_MISS_CNT_OVF); ENA bit 8 (DBUS_CNT_OVF) gates ST bits 9
        // (DBUS_ACS_CNT_OVF) and 10 (DBUS_ACS_FLASH_MISS_CNT_OVF). Bits 0, 1 and 5 pair with
        // themselves. The model only ever latches bit 5, so the counter overflow bits are set
        // here directly.
        let ctx = &mut Ctx::default();
        // (status bit, the one enable bit that raises the source).
        for (status, enable) in [(0, 0), (1, 1), (5, 5), (7, 7), (8, 7), (9, 8), (10, 8)] {
            for candidate in [0, 1, 5, 7, 8] {
                let mut x = Extmem::new();
                x.illegal_status = 1 << status;
                assert!(x.write(ctx, reg("CACHE_ILG_INT_ENA"), 1 << candidate));
                assert_eq!(
                    level(ctx, ILLEGAL_SOURCE),
                    candidate == enable,
                    "status bit {status} with only enable bit {candidate}"
                );
            }
        }
    }

    /// An MMU table whose entries map flash pages: `(entry, page)` pairs.
    fn mapping(pairs: &[(u32, u32)]) -> MmuTable {
        let ctx = &mut Ctx::default();
        let mut mmu = MmuTable::new();
        for &(entry, page) in pairs {
            assert!(mmu.write(ctx, 4 * entry, page));
        }
        mmu
    }

    /// Software's request, as `Cache_Invalidate_ICache_Items(addr, lines)` issues it, and the
    /// event it posts delivered as `Peripherals::deliver` does.
    fn invalidate(x: &mut Extmem, mmu: &MmuTable, addr: u32, lines: u32) {
        let ctx = &mut Ctx::default();
        assert!(x.write(ctx, reg("ICACHE_SYNC_ADDR"), addr));
        assert!(x.write(ctx, reg("ICACHE_SYNC_SIZE"), lines));
        assert!(x.write(ctx, reg("ICACHE_SYNC_CTRL"), 1));
        assert_eq!(ctx.take_signal(), Some(Event::CacheInvalidated));
        assert_eq!(ctx.take_signal(), None, "one request, one event");
        x.invalidate(mmu);
    }

    /// The lines of one 64 KiB MMU page: `Cache_Invalidate_Addr(vaddr, 0x10000)` writes `0x800`.
    const PAGE_LINES: u32 = 0x800;
    const PAGE: u32 = MMU_PAGE_BYTES;

    #[test]
    fn a_page_read_through_a_window_then_written_is_stale_until_the_cache_is_invalidated() {
        let mmu = mapping(&[(3, 5)]);
        let mut x = Extmem::new();
        assert!(!x.window_read(5), "a first read is what the flash holds");
        x.flash_written(5, 5);
        assert!(
            x.window_read(5),
            "read, then written: a device has the line"
        );
        assert!(x.window_read(5), "and keeps it: the next read is stale too");
        assert!(!x.window_read(4), "another page is untouched");
        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE, PAGE_LINES);
        assert!(!x.window_read(5), "the line was dropped: the read is fresh");
        // The read above filled the line again: a new write makes it stale again.
        x.flash_written(5, 5);
        assert!(x.window_read(5));
    }

    #[test]
    fn a_page_written_before_any_window_read_it_is_not_stale() {
        let mmu = mapping(&[(3, 5)]);
        let mut x = Extmem::new();
        x.flash_written(5, 5);
        assert!(!x.window_read(5), "no line held the bytes of before");
        // An invalidation forgets the read too: a write after it makes nothing stale.
        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 3 * PAGE, PAGE_LINES);
        x.flash_written(5, 5);
        assert!(!x.window_read(5));
    }

    #[test]
    fn a_write_marks_the_pages_of_its_range_that_were_read_and_no_other() {
        let mut x = Extmem::new();
        assert!(!x.window_read(2));
        assert!(!x.window_read(4));
        assert!(!x.window_read(5));
        x.flash_written(1, 4); // a 64 KiB block erase and more: pages 1 to 4
        assert!(x.window_read(2));
        assert!(x.window_read(4));
        assert!(!x.window_read(1), "in the range, never read");
        assert!(!x.window_read(3), "in the range, never read");
        assert!(!x.window_read(5), "read, past the range");
        assert!(!x.window_read(0), "before the range");
    }

    #[test]
    fn an_invalidation_drops_the_pages_of_the_whole_entries_it_covers_in_either_window() {
        let mmu = mapping(&[(3, 5), (4, 6), (5, 7)]);
        let stale = |x: &mut Extmem| [5, 6, 7].map(|page| x.window_read(page));
        let mut x = Extmem::new();
        let fill = |x: &mut Extmem| {
            for page in [5, 6, 7] {
                x.window_read(page);
            }
            x.flash_written(5, 7);
        };
        fill(&mut x);
        assert_eq!(stale(&mut x), [true, true, true]);
        // One entry, named through the data window, then another through the instruction window.
        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 4 * PAGE, PAGE_LINES);
        assert_eq!(stale(&mut x), [true, false, true]);
        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 3 * PAGE, PAGE_LINES);
        assert_eq!(stale(&mut x), [false, false, true]);
        // Two entries at once.
        fill(&mut x);
        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE, 2 * PAGE_LINES);
        assert_eq!(stale(&mut x), [false, false, true]);
        // A range that covers no whole entry leaves the lines it does not name: nothing is dropped.
        fill(&mut x);
        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE, PAGE_LINES - 1);
        assert_eq!(stale(&mut x), [true, true, true], "one line short");
        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE + 32, PAGE_LINES);
        assert_eq!(stale(&mut x), [true, true, true], "one line late");
        // One line late and long enough to cover the next entry: that one is dropped.
        invalidate(
            &mut x,
            &mmu,
            FLASH_DBUS_BASE + 3 * PAGE + 32,
            2 * PAGE_LINES,
        );
        assert_eq!(stale(&mut x), [true, false, true]);
    }

    #[test]
    fn an_invalidation_of_everything_drops_every_page() {
        // `Cache_Invalidate_ICache_All` is `Cache_Invalidate_ICache_Items(0, 0)` (CM §4.1).
        let mmu = MmuTable::new(); // nothing mapped: "everything" does not go through the table
        let mut x = Extmem::new();
        for page in [0, 63, 64, 255] {
            x.window_read(page);
        }
        x.flash_written(0, 255);
        assert!(x.window_read(0) && x.window_read(63) && x.window_read(64) && x.window_read(255));
        invalidate(&mut x, &mmu, 0, 0);
        for page in [0, 63, 64, 255] {
            assert!(!x.window_read(page), "{page}");
        }
    }

    #[test]
    fn a_request_that_names_no_line_of_a_window_drops_nothing() {
        let mmu = mapping(&[(0, 5), (127, 5)]);
        let mut x = Extmem::new();
        x.window_read(5);
        x.flash_written(5, 5);
        for (addr, lines) in [
            (FLASH_DBUS_BASE, 0),                              // no line, at an address
            (0, PAGE_LINES),                                   // lines, outside the windows
            (FLASH_DBUS_BASE - PAGE, 2 * PAGE_LINES),          // starts before the window
            (FLASH_IBUS_BASE + FLASH_WINDOW_SIZE, PAGE_LINES), // starts past its end
            (FLASH_DBUS_BASE + PAGE, PAGE_LINES),              // an entry that maps nothing
            (u32::MAX, u32::MAX),
        ] {
            invalidate(&mut x, &mmu, addr, lines);
            assert!(x.window_read(5), "{addr:#x} {lines:#x}");
        }
        // The size field has 23 bits: the largest request from the last entry ends past the
        // window and is still followed to the end of the table, no further.
        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 127 * PAGE, u32::MAX);
        assert!(!x.window_read(5));
    }

    #[test]
    fn an_invalidation_changes_the_generation_when_it_takes_a_page_out_of_the_cache_only() {
        // The CPU runs the code it decoded from a page without a bus access: the page must be
        // fetched again, and seen as read again, once the cache was invalidated for it.
        let mmu = mapping(&[(3, 5), (4, 6)]);
        let mut x = Extmem::new();
        let mut generation = x.generation();
        let mut moved = |x: &Extmem| {
            let moved = x.generation() != generation;
            generation = x.generation();
            moved
        };
        // Nothing was read: neither request takes anything out.
        invalidate(&mut x, &mmu, 0, 0);
        assert!(!moved(&x), "everything, nothing read");
        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 3 * PAGE, PAGE_LINES);
        assert!(!moved(&x), "an entry, its page not read");
        // A page that was read, and not written since, leaves the cache.
        x.window_read(5);
        x.window_read(6);
        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 3 * PAGE, PAGE_LINES);
        assert!(moved(&x), "page 5 was read");
        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 3 * PAGE, PAGE_LINES);
        assert!(!moved(&x), "the same request again: page 5 is out already");
        // Requests that drop nothing: one line short of the entry, outside the windows, no line
        // at an address, an entry that maps nothing.
        for (addr, lines) in [
            (FLASH_DBUS_BASE + 4 * PAGE, PAGE_LINES - 1),
            (0, PAGE_LINES),
            (FLASH_DBUS_BASE + 4 * PAGE, 0),
            (FLASH_DBUS_BASE + 5 * PAGE, PAGE_LINES),
        ] {
            invalidate(&mut x, &mmu, addr, lines);
            assert!(!moved(&x), "{addr:#x} {lines:#x}");
        }
        // Page 6 is still in: the request for everything takes it out, once.
        invalidate(&mut x, &mmu, 0, 0);
        assert!(moved(&x), "everything, page 6 read");
        invalidate(&mut x, &mmu, 0, 0);
        assert!(!moved(&x), "everything again");
        // A read does not move it, nor does a write of the flash: the chip has its own count.
        x.window_read(5);
        x.flash_written(5, 5);
        assert!(!moved(&x));
        // A stale page leaves the cache like any other.
        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE, PAGE_LINES);
        assert!(moved(&x), "page 5, read then written");
    }

    #[test]
    fn only_the_invalidate_bit_of_the_sync_register_asks_for_an_invalidation() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        for value in [0, SYNC_DONE, !SYNC_INVALIDATE] {
            assert!(x.write(ctx, reg("ICACHE_SYNC_CTRL"), value));
            assert_eq!(ctx.take_signal(), None, "{value:#x}");
        }
        for value in [SYNC_INVALIDATE, 3, u32::MAX] {
            assert!(x.write(ctx, reg("ICACHE_SYNC_CTRL"), value));
            assert_eq!(
                ctx.take_signal(),
                Some(Event::CacheInvalidated),
                "{value:#x}"
            );
        }
    }

    #[test]
    fn switching_the_cache_off_and_on_keeps_its_lines() {
        // `Cache_Suspend_ICache` and `Cache_Resume_ICache` around a flash operation write
        // `ICACHE_CTRL` and `ICACHE_CTRL1` and invalidate nothing (CM §4.1): the lines are still
        // there when the cache is back, stale if the operation changed their bytes.
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        x.window_read(5);
        x.write(ctx, reg("ICACHE_CTRL1"), 3);
        x.write(ctx, reg("ICACHE_CTRL"), 0);
        x.flash_written(5, 5);
        x.write(ctx, reg("ICACHE_CTRL"), 1);
        x.write(ctx, reg("ICACHE_CTRL1"), 0);
        assert_eq!(ctx.take_signal(), None);
        assert!(x.window_read(5));
    }

    #[test]
    fn page_numbers_no_entry_can_name_are_in_no_set() {
        let mut x = Extmem::new();
        for page in [256, 257, 1 << 16, u32::MAX] {
            assert!(!x.window_read(page), "{page}");
        }
        // A range the firmware's address made absurd is followed to page 255 and no further.
        x.window_read(255);
        x.flash_written(0, u32::MAX);
        assert!(x.window_read(255));
        for page in [256, u32::MAX] {
            assert!(!x.window_read(page), "{page}");
        }
        // An empty range (first past last) marks nothing.
        let mut x = Extmem::new();
        x.window_read(2);
        x.flash_written(3, 2);
        x.flash_written(300, 400);
        assert!(!x.window_read(2));
    }

    #[test]
    fn the_pages_of_a_write_stop_at_the_last_page_an_entry_can_name() {
        assert_eq!(written_pages(0, u32::MAX), 0..=255);
        assert_eq!(written_pages(255, u32::MAX), 255..=255);
        assert_eq!(written_pages(5, 7), 5..=7);
        assert!(written_pages(300, u32::MAX).is_empty());
        assert!(written_pages(3, 2).is_empty());
    }

    #[test]
    fn the_entries_of_an_invalidation_stop_at_the_end_of_the_table() {
        let last = FLASH_WINDOW_SIZE - 1;
        // The largest request the 23-bit size field can make, from the start, the middle and
        // the last byte of a window: 4,095 entries' worth, of which the table has 128.
        assert_eq!(whole_entries(0, SYNC_LINES_MASK), 0..128);
        assert_eq!(whole_entries(64 * PAGE, SYNC_LINES_MASK), 64..128);
        assert_eq!(whole_entries(last, SYNC_LINES_MASK), 128..128);
        assert_eq!(whole_entries(127 * PAGE, SYNC_LINES_MASK), 127..128);
        assert_eq!(whole_entries(127 * PAGE + 1, SYNC_LINES_MASK), 128..128);
        // What the value of the register can no longer hold, the function still bounds.
        assert_eq!(whole_entries(last, u32::MAX), 128..128);
        assert_eq!(whole_entries(0, u32::MAX), 0..128);
        // Ordinary requests: one page, two, one line short, one line late.
        assert_eq!(whole_entries(3 * PAGE, PAGE_LINES), 3..4);
        assert_eq!(whole_entries(3 * PAGE, 2 * PAGE_LINES), 3..5);
        assert!(whole_entries(3 * PAGE, PAGE_LINES - 1).is_empty());
        assert!(whole_entries(3 * PAGE + 32, PAGE_LINES).is_empty());
    }

    #[test]
    fn the_size_register_gives_its_low_23_bits_as_the_number_of_lines() {
        // `ICACHE_SYNC_SIZE` [22:0] is the line count; bits 31:23 are not part of it.
        let mmu = mapping(&[(3, 5)]);
        // Bits 31:23 set and a line count of 0, at address 0: the request for everything.
        for size in [0x0080_0000, 0xFF80_0000] {
            let mut x = Extmem::new();
            x.window_read(5);
            x.window_read(200);
            x.flash_written(0, 255);
            invalidate(&mut x, &mmu, 0, size);
            assert!(!x.window_read(5), "{size:#x}: everything was dropped");
            assert!(!x.window_read(200), "{size:#x}: everything was dropped");
        }
        // The same high bits at an address in a window: a count of 0 there drops nothing.
        for addr in [FLASH_DBUS_BASE + 3 * PAGE, FLASH_IBUS_BASE] {
            let mut x = Extmem::new();
            x.window_read(5);
            x.flash_written(5, 5);
            invalidate(&mut x, &mmu, addr, 0xFF80_0000);
            assert!(x.window_read(5), "{addr:#x}: no line was named");
        }
        // High bits above a real count: the count alone decides.
        let mut x = Extmem::new();
        x.window_read(5);
        x.flash_written(5, 5);
        invalidate(
            &mut x,
            &mmu,
            FLASH_DBUS_BASE + 3 * PAGE,
            0xFF80_0000 | PAGE_LINES,
        );
        assert!(!x.window_read(5), "the entry's page was dropped");
    }

    #[test]
    fn the_largest_requests_reach_the_last_page_and_the_last_entry() {
        // The worst a firmware can ask of the two loops, once each: a write of every page
        // number there is, an invalidation of every line the register can name from the first
        // entry. How far they go is the value of `written_pages` and `whole_entries`, pinned by
        // the two tests above on the same inputs: a removed bound changes a value, not a time.
        // Here the loops are seen to walk those ranges to their last element.
        let mmu = mapping(&[(0, 5), (127, 255)]);
        let mut x = Extmem::new();
        x.window_read(5);
        x.window_read(255);
        x.flash_written(0, u32::MAX);
        assert!(x.window_read(5), "the first page");
        assert!(x.window_read(255), "the last page");
        invalidate(&mut x, &mmu, FLASH_DBUS_BASE, u32::MAX);
        assert!(!x.window_read(5), "the first entry");
        assert!(!x.window_read(255), "the last entry");
    }

    #[test]
    fn registers_no_measured_path_touches_stay_unmodelled() {
        let ctx = &mut Ctx::default();
        let mut x = Extmem::new();
        for name in [
            "ICACHE_LOCK_CTRL",
            "ICACHE_PRELOAD_ADDR",
            "IBUS_PMS_TBL_LOCK",
            "CACHE_CONF_MISC",
            "REG_DATE",
        ] {
            assert_eq!(x.read(ctx, reg(name)), None, "{name}");
            assert!(!x.write(ctx, reg(name), 1), "{name}");
        }
    }
}
