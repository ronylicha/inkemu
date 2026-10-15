//! The side of the bus a debugger sees (ADR 0016): the accesses of the CPU noted for its
//! watchpoints, and a read of memory without the effects of a load.
use super::{MMU_PAGE_BYTES, Region, SocBus, canonical, locate};
use crate::debug::{Access, Peeked, Watch, WatchHit};
use emu_core::AccessSize;

impl SocBus {
    /// Notes a load or a store of the CPU that completed, when it touches watched bytes and no
    /// access has been noted since a debugger last took one: an instruction makes one access,
    /// so the first is the one of the instruction a watchpoint stops after.
    #[inline]
    pub(super) fn note_access(&mut self, addr: u32, size: AccessSize, access: Access, value: u32) {
        if self.watches.is_empty() || self.watch_hit.is_some() {
            return;
        }
        let bytes = size.bytes();
        let at = canonical(addr);
        if self
            .watches
            .iter()
            .any(|watch| watch.covers(at, bytes, access))
        {
            let mask = match size {
                AccessSize::Byte => 0xff,
                AccessSize::Half => 0xffff,
                AccessSize::Word => u32::MAX,
            };
            self.watch_hit = Some(WatchHit {
                address: addr,
                // INVARIANT: an access is 1, 2 or 4 bytes.
                size: u8::try_from(bytes).unwrap_or(u8::MAX),
                access,
                value: value & mask,
                pc: self.ctx.pc(),
            });
        }
    }

    /// Watches the accesses `watches` cover from now on, in place of the ones before. An access
    /// noted and not taken yet stays only if a watchpoint of the new list covers it: a cleared
    /// watchpoint stops nothing any more.
    pub(crate) fn set_watches(&mut self, watches: &[Watch]) {
        self.watches = watches.to_vec();
        if let Some(hit) = self.watch_hit
            && !self
                .watches
                .iter()
                .any(|watch| watch.covers(canonical(hit.address), u32::from(hit.size), hit.access))
        {
            self.watch_hit = None;
        }
    }

    /// The access noted since the last call, if any; the next access to watched bytes is noted
    /// from now on.
    pub(crate) fn take_watch_hit(&mut self) -> Option<WatchHit> {
        self.watch_hit.take()
    }

    /// The byte at `addr` as it is, without the effects of a load: no peripheral is called, no
    /// cache state nor fault register changes, nothing is reported.
    pub(crate) fn peek_byte(&self, addr: u32) -> Peeked {
        let byte = |bytes: Result<&[u8], emu_core::BusError>| {
            bytes
                .ok()
                .and_then(|bytes| bytes.first().copied())
                .map_or(Peeked::Unmapped, Peeked::Byte)
        };
        match locate(addr, 1) {
            Some(Region::Rom(offset)) => self
                .rom
                .bytes()
                .get(offset as usize)
                .copied()
                .map_or(Peeked::Unmapped, Peeked::Byte),
            Some(Region::Sram(dbus_addr)) => byte(self.sram.bytes(dbus_addr, 1)),
            Some(Region::Sram0) => byte(self.sram0.bytes(addr, 1)),
            Some(Region::Rtc) => byte(self.rtc.bytes(addr, 1)),
            Some(Region::Flash { offset, .. }) => match self.flash_offset(offset) {
                Ok(flash_offset) if self.executed.contains(flash_offset / MMU_PAGE_BYTES) => {
                    let mut out = [0u8];
                    self.periph.spi1.flash().read(flash_offset, &mut out);
                    Peeked::Flash(out[0])
                }
                Ok(_) => Peeked::NotExecuted,
                Err(_) => Peeked::Unbacked,
            },
            Some(Region::Periph) => Peeked::Register,
            None => Peeked::Unmapped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SocConfig;
    use crate::ctx::Event;
    use crate::debug::WatchOn;
    use crate::flash::NorFlash;
    use crate::memmap::{
        DROM_BASE, DROM_ROM_OFFSET, FLASH_DBUS_BASE, FLASH_IBUS_BASE, PERIPH_BASE, ROM_BASE,
        RTC_RAM_BASE, SRAM_DBUS_BASE, SRAM_IBUS_BASE, SRAM0_BASE,
    };
    use crate::periph::mmu::{MMU_ENTRY_INVALID, MMU_TABLE_BASE};
    use crate::test_support::flat_rom;
    use emu_core::{AccessKind, Bus};

    fn bus() -> SocBus {
        let config = SocConfig::default();
        let flash = Box::new(NorFlash::blank(config.flash));
        SocBus::new(flat_rom(&[0x1234_5678]), flash, &config)
    }

    fn watch(address: u32, length: u32, on: WatchOn) -> Watch {
        Watch {
            address,
            length,
            on,
        }
    }

    #[test]
    fn a_store_to_watched_bytes_is_noted_with_its_value_and_its_instruction() {
        let mut b = bus();
        b.set_pc(0x4000_0010);
        b.set_watches(&[watch(SRAM_DBUS_BASE + 8, 4, WatchOn::Write)]);
        b.write(SRAM_DBUS_BASE, AccessSize::Word, 1).unwrap();
        assert_eq!(b.take_watch_hit(), None, "next to the watched bytes");
        b.write(SRAM_DBUS_BASE + 10, AccessSize::Half, 0x1_beef)
            .unwrap();
        assert_eq!(
            b.take_watch_hit(),
            Some(WatchHit {
                address: SRAM_DBUS_BASE + 10,
                size: 2,
                access: Access::Write,
                value: 0xbeef,
                pc: 0x4000_0010,
            })
        );
        assert_eq!(b.take_watch_hit(), None, "taken");
    }

    #[test]
    fn a_watchpoint_stops_only_on_the_accesses_it_names() {
        let mut b = bus();
        let at = SRAM_DBUS_BASE + 0x100;
        b.set_watches(&[watch(at, 1, WatchOn::Read)]);
        b.write(at, AccessSize::Byte, 7).unwrap();
        assert_eq!(b.take_watch_hit(), None, "a store, on a read watchpoint");
        b.read(at, AccessSize::Word, AccessKind::Load).unwrap();
        let hit = b.take_watch_hit().unwrap();
        assert_eq!((hit.access, hit.value, hit.size), (Access::Read, 7, 4));
        b.read(at, AccessSize::Word, AccessKind::Fetch).unwrap();
        assert_eq!(b.take_watch_hit(), None, "a fetch is no data access");
        b.set_watches(&[watch(at, 1, WatchOn::Access)]);
        b.write(at, AccessSize::Byte, 9).unwrap();
        assert!(b.take_watch_hit().is_some(), "access: stores too");
    }

    #[test]
    fn the_first_access_is_kept_until_it_is_taken() {
        let mut b = bus();
        b.set_watches(&[watch(SRAM_DBUS_BASE, 16, WatchOn::Write)]);
        b.write(SRAM_DBUS_BASE, AccessSize::Word, 1).unwrap();
        b.write(SRAM_DBUS_BASE + 4, AccessSize::Word, 2).unwrap();
        assert_eq!(b.take_watch_hit().map(|hit| hit.value), Some(1));
    }

    #[test]
    fn the_sram_is_watched_through_both_of_its_views() {
        let mut b = bus();
        b.set_watches(&[watch(SRAM_IBUS_BASE + 0x40, 4, WatchOn::Write)]);
        b.write(SRAM_DBUS_BASE + 0x40, AccessSize::Word, 5).unwrap();
        let hit = b.take_watch_hit().unwrap();
        assert_eq!(hit.address, SRAM_DBUS_BASE + 0x40, "the address as given");
        b.set_watches(&[watch(SRAM_DBUS_BASE + 0x40, 4, WatchOn::Read)]);
        b.read(SRAM_IBUS_BASE + 0x40, AccessSize::Word, AccessKind::Load)
            .unwrap();
        assert!(b.take_watch_hit().is_some());
    }

    #[test]
    fn the_rom_is_watched_through_its_data_bus_alias() {
        let mut b = bus();
        // The data-bus alias shows the ROM from its offset `DROM_ROM_OFFSET`.
        b.set_watches(&[watch(ROM_BASE + DROM_ROM_OFFSET + 8, 4, WatchOn::Read)]);
        b.read(DROM_BASE + 8, AccessSize::Word, AccessKind::Load)
            .unwrap();
        assert_eq!(
            b.take_watch_hit().map(|hit| hit.address),
            Some(DROM_BASE + 8)
        );
    }

    #[test]
    fn a_noted_access_a_cleared_watchpoint_covered_is_forgotten() {
        let mut b = bus();
        let kept = watch(SRAM_DBUS_BASE, 4, WatchOn::Write);
        b.set_watches(&[kept, watch(SRAM_DBUS_BASE + 16, 4, WatchOn::Write)]);
        b.write(SRAM_DBUS_BASE, AccessSize::Word, 1).unwrap();
        b.set_watches(&[kept]);
        assert!(b.take_watch_hit().is_some(), "still covered: kept");
        b.write(SRAM_DBUS_BASE, AccessSize::Word, 2).unwrap();
        b.set_watches(&[watch(SRAM_DBUS_BASE + 16, 4, WatchOn::Write)]);
        assert_eq!(b.take_watch_hit(), None, "its watchpoint cleared: gone");
        b.write(SRAM_DBUS_BASE + 16, AccessSize::Word, 3).unwrap();
        assert!(b.take_watch_hit().is_some(), "and the next access is noted");
    }

    #[test]
    fn a_watchpoint_at_the_top_of_the_address_space_does_not_overflow() {
        let mut b = bus();
        b.set_watches(&[watch(u32::MAX, u32::MAX, WatchOn::Access)]);
        b.write(SRAM_DBUS_BASE, AccessSize::Word, 1).unwrap();
        assert_eq!(b.take_watch_hit(), None);
    }

    #[test]
    fn a_faulting_access_is_not_noted() {
        let mut b = bus();
        b.set_watches(&[watch(ROM_BASE, 16, WatchOn::Write)]);
        assert!(b.write(ROM_BASE, AccessSize::Word, 1).is_err());
        assert_eq!(b.take_watch_hit(), None);
    }

    #[test]
    fn a_peek_reads_the_memories_and_reaches_no_register() {
        let mut b = bus();
        b.write(SRAM_DBUS_BASE + 3, AccessSize::Byte, 0xa5).unwrap();
        b.write(RTC_RAM_BASE, AccessSize::Byte, 0x5a).unwrap();
        assert_eq!(b.peek_byte(ROM_BASE), Peeked::Byte(0x78));
        assert_eq!(b.peek_byte(ROM_BASE + 3), Peeked::Byte(0x12));
        assert_eq!(b.peek_byte(SRAM_DBUS_BASE + 3), Peeked::Byte(0xa5));
        assert_eq!(
            b.peek_byte(SRAM_IBUS_BASE + 3),
            Peeked::Byte(0xa5),
            "the same cell"
        );
        assert_eq!(b.peek_byte(RTC_RAM_BASE), Peeked::Byte(0x5a));
        assert_eq!(b.peek_byte(SRAM0_BASE), Peeked::Byte(0));
        assert_eq!(b.peek_byte(PERIPH_BASE), Peeked::Register);
        assert_eq!(b.peek_byte(0x1000), Peeked::Unmapped);
        assert!(b.ctx.diag.drain().is_empty(), "nothing reported");
    }

    #[test]
    fn a_peek_of_a_flash_window_follows_the_mmu_without_a_report() {
        let config = SocConfig::default();
        let mut image = vec![0xffu8; 0x2_0000];
        image[0x1_0000..0x1_0004].copy_from_slice(&[0x13, 0x05, 0x15, 0x00]);
        let flash = Box::new(NorFlash::with_image(image, config.flash).unwrap());
        let mut b = SocBus::new(flat_rom(&[]), flash, &config);
        // Entry 2 is left invalid: nothing backs that page of either window.
        b.write(MMU_TABLE_BASE + 4, AccessSize::Word, 1).unwrap();
        b.write(MMU_TABLE_BASE + 8, AccessSize::Word, MMU_ENTRY_INVALID)
            .unwrap();
        b.read(
            FLASH_IBUS_BASE + 0x1_0000,
            AccessSize::Half,
            AccessKind::Fetch,
        )
        .unwrap();
        let generation = b.periph.extmem.generation();
        assert_eq!(b.peek_byte(FLASH_IBUS_BASE + 0x1_0000), Peeked::Flash(0x13));
        assert_eq!(b.peek_byte(FLASH_DBUS_BASE + 0x1_0002), Peeked::Flash(0x15));
        assert_eq!(b.peek_byte(FLASH_IBUS_BASE + 0x2_0000), Peeked::Unbacked);
        assert_eq!(b.ctx.diag.drain(), [], "no fault latched nor reported");
        assert_eq!(b.periph.extmem.generation(), generation, "no cache state");
    }

    /// A bus whose MMU entry 1 maps flash page 1, and entry 2 flash page 2.
    fn mapped_bus() -> SocBus {
        let config = SocConfig::default();
        let mut image = vec![0xffu8; 0x3_0000];
        image[0x1_0000..0x1_0004].copy_from_slice(&[0x13, 0x05, 0x15, 0x00]);
        image[0x2_0000..0x2_0004].copy_from_slice(&[0x93, 0x05, 0x16, 0x00]);
        let flash = Box::new(NorFlash::with_image(image, config.flash).unwrap());
        let mut b = SocBus::new(flat_rom(&[]), flash, &config);
        b.write(MMU_TABLE_BASE + 4, AccessSize::Word, 1).unwrap();
        b.write(MMU_TABLE_BASE + 8, AccessSize::Word, 2).unwrap();
        b
    }

    /// ADR 0016 §6: the flash a debugger may read is the code the CPU executed. A page read as
    /// data, through either window, gives no byte; a fetch from a page opens it, and only it.
    #[test]
    fn a_peek_gives_a_flash_byte_only_from_a_page_the_cpu_fetched_from() {
        let mut b = mapped_bus();
        b.read(
            FLASH_DBUS_BASE + 0x1_0000,
            AccessSize::Word,
            AccessKind::Load,
        )
        .unwrap();
        assert_eq!(b.peek_byte(FLASH_DBUS_BASE + 0x1_0000), Peeked::NotExecuted);
        assert_eq!(
            b.peek_byte(FLASH_IBUS_BASE + 0x1_0000),
            Peeked::NotExecuted,
            "a page of data, read through the instruction-bus alias"
        );
        b.read(
            FLASH_IBUS_BASE + 0x2_0000,
            AccessSize::Half,
            AccessKind::Fetch,
        )
        .unwrap();
        assert_eq!(b.peek_byte(FLASH_IBUS_BASE + 0x2_0000), Peeked::Flash(0x93));
        assert_eq!(b.peek_byte(FLASH_IBUS_BASE + 0x1_0000), Peeked::NotExecuted);
    }

    /// A program or an erase changes what a page holds: what the CPU executed there is gone, and
    /// the page is closed again until the CPU fetches from it anew.
    #[test]
    fn a_page_the_flash_rewrote_is_not_executed_any_more() {
        let mut b = mapped_bus();
        b.read(
            FLASH_IBUS_BASE + 0x1_0000,
            AccessSize::Half,
            AccessKind::Fetch,
        )
        .unwrap();
        b.read(
            FLASH_IBUS_BASE + 0x2_0000,
            AccessSize::Half,
            AccessKind::Fetch,
        )
        .unwrap();
        b.ctx.signal(Event::FlashWritten {
            first_page: 2,
            last_page: 2,
        });
        b.settle();
        assert_eq!(b.peek_byte(FLASH_IBUS_BASE + 0x2_0000), Peeked::NotExecuted);
        assert_eq!(b.peek_byte(FLASH_IBUS_BASE + 0x1_0000), Peeked::Flash(0x13));
        b.read(
            FLASH_IBUS_BASE + 0x2_0000,
            AccessSize::Half,
            AccessKind::Fetch,
        )
        .unwrap();
        assert_eq!(b.peek_byte(FLASH_IBUS_BASE + 0x2_0000), Peeked::Flash(0x93));
    }
}
