//! The ESP32-C3 address space. Memory map: `docs/research/esp32c3-boot-chain.md`, §A.1.
use crate::config::SocConfig;
use crate::ctx::{Ctx, Event};
use crate::diag::{Diagnostic, Direction, MappingFault};
use crate::flash::FlashDevice;
use crate::memmap::{
    DROM_BASE, DROM_ROM_OFFSET, DROM_SIZE, FLASH_DBUS_BASE, FLASH_IBUS_BASE, FLASH_WINDOW_SIZE,
    PERIPH_BASE, PERIPH_SIZE, ROM_BASE, ROM_SIZE, RTC_RAM_BASE, RTC_RAM_SIZE, SRAM_DBUS_BASE,
    SRAM_IBUS_BASE, SRAM_SIZE, SRAM0_BASE, SRAM0_SIZE,
};
use crate::periph::Peripherals;
use crate::periph::mmu::{MMU_PAGE_BYTES, MmuMiss};
use crate::reset::ResetCause;
use crate::rom::RomImage;
use emu_core::{AccessKind, AccessSize, Bus, BusError, Ram, ReadOutcome};
use svd_regs::REGISTER_BYTES;

#[derive(Debug)]
pub struct SocBus {
    rom: RomImage,
    /// The 384 KiB SRAM, addressed here by its data-bus address.
    sram: Ram,
    sram0: Ram,
    rtc: Ram,
    /// Reached from outside the crate only through the bus: a direct write would bypass the
    /// diagnostics the bus records for an unmodelled or sub-word access.
    pub(crate) periph: Peripherals,
    /// The context handed to every peripheral access; it holds the diagnostics of the run.
    pub(crate) ctx: Ctx,
}

#[derive(Clone, Copy)]
enum Region {
    /// Offset in the ROM image.
    Rom(u32),
    /// Data-bus address of the SRAM cell, whichever bus the access came through.
    Sram(u32),
    Sram0,
    Rtc,
    /// A flash window: the offset inside it, and whether it is the instruction-bus one.
    Flash {
        offset: u32,
        instruction_bus: bool,
    },
    Periph,
}

/// Why one byte of a flash window is backed by no flash byte: the MMU entry it goes through, the
/// content of that entry, and what is wrong with it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct UnbackedByte {
    entry: u32,
    content: u32,
    fault: MappingFault,
}

/// Offset of `[addr, addr + len)` in the block at `base`, when it lies entirely inside it.
fn offset_in(addr: u32, len: u32, base: u32, size: u32) -> Option<u32> {
    let offset = addr.checked_sub(base)?;
    (offset.checked_add(len)? <= size).then_some(offset)
}

fn locate(addr: u32, len: u32) -> Option<Region> {
    let at = |base, size| offset_in(addr, len, base, size);
    if let Some(offset) = at(ROM_BASE, ROM_SIZE) {
        Some(Region::Rom(offset))
    } else if let Some(offset) = at(DROM_BASE, DROM_SIZE) {
        Some(Region::Rom(offset + DROM_ROM_OFFSET))
    } else if let Some(offset) =
        at(SRAM_DBUS_BASE, SRAM_SIZE).or_else(|| at(SRAM_IBUS_BASE, SRAM_SIZE))
    {
        Some(Region::Sram(SRAM_DBUS_BASE + offset))
    } else if at(SRAM0_BASE, SRAM0_SIZE).is_some() {
        Some(Region::Sram0)
    } else if at(RTC_RAM_BASE, RTC_RAM_SIZE).is_some() {
        Some(Region::Rtc)
    } else if let Some(offset) = at(FLASH_DBUS_BASE, FLASH_WINDOW_SIZE) {
        Some(Region::Flash {
            offset,
            instruction_bus: false,
        })
    } else if let Some(offset) = at(FLASH_IBUS_BASE, FLASH_WINDOW_SIZE) {
        Some(Region::Flash {
            offset,
            instruction_bus: true,
        })
    } else if at(PERIPH_BASE, PERIPH_SIZE).is_some() {
        Some(Region::Periph)
    } else {
        None
    }
}

const fn size_mask(size: AccessSize) -> u32 {
    match size {
        AccessSize::Byte => 0xff,
        AccessSize::Half => 0xffff,
        AccessSize::Word => u32::MAX,
    }
}

impl SocBus {
    /// The bus of a chip whose flash is `flash`.
    ///
    /// Only this crate can call it: [`Esp32C3::new`](crate::Esp32C3::new) and
    /// [`Esp32C3::with_flash`](crate::Esp32C3::with_flash) build the chip from
    /// [`SocConfig::flash`], so a machine never holds a flash whose size or id is another than
    /// the one its configuration states. A caller outside the crate hands over bytes, not a chip;
    /// the only thing that does not compile below is the call:
    ///
    /// ```compile_fail
    /// use soc_esp32c3::{FlashDevice, RomImage, SocConfig};
    /// fn a_bus_around_my_own_chip(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) {
    ///     let _ = soc_esp32c3::bus::SocBus::new(rom, flash, config);
    /// }
    /// ```
    #[must_use]
    pub(crate) fn new(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) -> SocBus {
        let mut ctx = Ctx::new(config.strict_mmio);
        let periph = Peripherals::new(config, flash, &mut ctx);
        SocBus {
            rom,
            sram: Ram::new(SRAM_DBUS_BASE, SRAM_SIZE as usize),
            sram0: Ram::new(SRAM0_BASE, SRAM0_SIZE as usize),
            rtc: Ram::new(RTC_RAM_BASE, RTC_RAM_SIZE as usize),
            periph,
            ctx,
        }
    }

    /// Calls the device `event` is for, then every device the call had something to tell.
    pub(crate) fn deliver(&mut self, event: Event) {
        self.periph.deliver(&mut self.ctx, event);
        self.settle();
    }

    /// Applies a reset to the peripherals. The memories keep their content (TRM §6.1.1).
    ///
    /// ASSUMPTION: the RTC memory keeps its content at a system reset too. The TRM says the
    /// memories are not reset (§6.1.1); report TW §I question 9 leaves open what a reset of the
    /// RTC domain does to them. Reading an RTC memory word on a chip after a `rst:0x10` settles it.
    pub(crate) fn reset(&mut self, cause: ResetCause, pc: u32, sp: u32) {
        self.periph.reset(&mut self.ctx, cause, pc, sp);
        self.settle();
    }

    /// One delivery pass of the wake-ups that are due (see [`Ctx::deliver_due`]), each followed by
    /// the events its call had something to tell.
    pub(crate) fn deliver_due(&mut self) {
        let SocBus { periph, ctx, .. } = self;
        ctx.deliver_due(|ctx, event| {
            periph.deliver(ctx, event);
            settle_signals(periph, ctx);
        });
    }

    /// Delivers what the devices posted for each other during a register access or a call: such
    /// an event reaches its device before the instruction ends.
    fn settle(&mut self) {
        settle_signals(&mut self.periph, &mut self.ctx);
    }

    /// Tells the bus which instruction is running, so diagnostics can name it.
    pub fn set_pc(&mut self, pc: u32) {
        self.ctx.set_pc(pc);
    }

    fn rom_read(&self, offset: u32, size: AccessSize) -> u32 {
        // INVARIANT: `locate` checked that the whole access lies inside the ROM.
        let start = offset as usize;
        self.rom.bytes()[start..start + size.bytes() as usize]
            .iter()
            .rev()
            .fold(0, |acc, &b| (acc << 8) | u32::from(b))
    }

    /// Spec §5.2: an unmodelled register reads as its SVD reset value; the access is reported once.
    fn unmodeled(&mut self, reg_addr: u32, direction: Direction, written: u32) -> u32 {
        let info = svd_regs::esp32c3::lookup(reg_addr);
        let reset = info.map_or(0, |r| r.reset_value);
        self.ctx.diag.record(Diagnostic::UnmodeledMmio {
            addr: reg_addr,
            direction,
            pc: self.ctx.pc(),
            register: info.map(|r| (r.peripheral, r.name)),
            value: if direction == Direction::Read {
                reset
            } else {
                written
            },
            reset_explicit: info.is_some_and(|r| r.reset_explicit),
        });
        reset
    }

    /// The flash offset one window byte maps to, or why no flash byte backs it: an invalid entry, or
    /// a page past the end of the chip.
    fn flash_offset(&self, window_offset: u32) -> Result<u32, UnbackedByte> {
        match self.periph.mmu.translate(window_offset) {
            Ok(offset) if offset < self.periph.spi1.flash().size() => Ok(offset),
            Ok(offset) => Err(UnbackedByte {
                entry: window_offset / MMU_PAGE_BYTES,
                content: offset / MMU_PAGE_BYTES,
                fault: MappingFault::BeyondFlash,
            }),
            Err(MmuMiss { entry, content }) => Err(UnbackedByte {
                entry,
                content,
                fault: MappingFault::InvalidEntry,
            }),
        }
    }

    /// Reads the flash **live** through the MMU table (decision D4): no cache line is modelled.
    ///
    /// ASSUMPTION: (decisions D4, D23) an access through an invalid entry, or to a page past the end
    /// of the flash, returns 0. What silicon returns is not documented (report CM §6.1: "wrong
    /// data"); 0 makes a fetch an illegal instruction, as ESP-IDF's panic handler expects. The fault
    /// registers are latched and the access is reported; the SP1b-4 lockstep may refine the value.
    fn flash_read(&mut self, vaddr: u32, window_offset: u32, size: AccessSize) -> ReadOutcome {
        let len = size.bytes();
        let mut bytes = [0u8; 4];
        // The fault of the previous byte. The bytes of one access that miss through the same entry
        // for the same reason are one event: the diagnostics count accesses, not bytes. An access is
        // at most 4 bytes, so it touches at most two entries, one after the other: remembering the
        // last fault is enough to report each (entry, fault) once.
        let mut reported: Option<UnbackedByte> = None;
        // The flash page of a byte a device may answer otherwise (`Extmem::window_read`). One
        // access is one event, as above: the first such page is the one reported.
        let mut stale: Option<u32> = None;
        // Byte by byte: a misaligned access may straddle two 64 KiB pages, each with its own entry.
        for (index, byte) in (0u32..len).zip(bytes.iter_mut()) {
            // INVARIANT: `locate` checked that the whole access lies inside the window, so
            // `window_offset + index` stays below `FLASH_WINDOW_SIZE` and cannot overflow.
            match self.flash_offset(window_offset + index) {
                Ok(offset) => {
                    self.periph
                        .spi1
                        .flash()
                        .read(offset, std::slice::from_mut(byte));
                    let page = offset / MMU_PAGE_BYTES;
                    if self.periph.extmem.window_read(page) {
                        stale.get_or_insert(page);
                    }
                }
                Err(unbacked) if reported == Some(unbacked) => {}
                Err(unbacked) => {
                    // The fault registers and the diagnostic name the address of the access.
                    self.deliver(Event::MmuFault {
                        vaddr,
                        content: unbacked.content,
                    });
                    self.ctx.diag.record(Diagnostic::FlashMapping {
                        vaddr,
                        entry: unbacked.entry,
                        content: unbacked.content,
                        fault: unbacked.fault,
                        pc: self.ctx.pc(),
                    });
                    reported = Some(unbacked);
                }
            }
        }
        if let Some(page) = stale {
            self.ctx.diag.record(Diagnostic::StaleFlashWindow {
                vaddr,
                page,
                pc: self.ctx.pc(),
            });
        }
        ReadOutcome {
            value: u32::from_le_bytes(bytes) & size_mask(size),
            stall: 0,
        }
    }

    fn mmio_read(&mut self, addr: u32, size: AccessSize) -> Result<ReadOutcome, BusError> {
        if !addr.is_multiple_of(size.bytes()) {
            return Err(BusError::Misaligned { addr, size });
        }
        let reg_addr = addr & !(REGISTER_BYTES - 1);
        let modelled = self
            .periph
            .device(reg_addr)
            .and_then(|(device, base)| device.read(&mut self.ctx, reg_addr - base));
        self.settle();
        let word = match modelled {
            Some(word) => word,
            None => self.unmodeled(reg_addr, Direction::Read, 0),
        };
        let shift = 8 * (addr - reg_addr);
        Ok(ReadOutcome {
            value: (word >> shift) & size_mask(size),
            stall: 0,
        })
    }

    fn mmio_write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError> {
        if !addr.is_multiple_of(size.bytes()) {
            return Err(BusError::Misaligned { addr, size });
        }
        let reg_addr = addr & !(REGISTER_BYTES - 1);
        let word = (value & size_mask(size)) << (8 * (addr - reg_addr));
        if size != AccessSize::Word {
            self.ctx.diag.record(Diagnostic::SubWordWrite {
                addr,
                bytes: size.bytes(),
                pc: self.ctx.pc(),
            });
        }
        let handled = self
            .periph
            .device(reg_addr)
            .is_some_and(|(device, base)| device.write(&mut self.ctx, reg_addr - base, word));
        self.settle();
        if !handled {
            self.unmodeled(reg_addr, Direction::Write, word);
        }
        Ok(0)
    }
}

/// Delivers the events devices posted with [`Ctx::signal`], oldest first.
///
/// The loop ends: the only code that posts a signal is a write to `SYSTEM.PERIP_RST_EN0/1`
/// (`System::write`: module resets), to `SPI1.CMD` (`Spi1::transfer`: a program or an erase
/// changed the flash) or to `EXTMEM.ICACHE_SYNC_CTRL` (`Extmem::write`: a cache invalidation),
/// which a register access reaches and a delivery does not. A delivery resets a module, or marks
/// or clears flash pages in the cache state: none writes one of those registers or posts a
/// signal. So the signals there are to deliver are those one register access posted (at most one
/// per module; for a flash command one per run of pages it changed, 128 at most among the 256
/// pages; one for an invalidation), and delivering them posts none.
fn settle_signals(periph: &mut Peripherals, ctx: &mut Ctx) {
    while let Some(event) = ctx.take_signal() {
        periph.deliver(ctx, event);
    }
}

impl Bus for SocBus {
    fn read(
        &mut self,
        addr: u32,
        size: AccessSize,
        kind: AccessKind,
    ) -> Result<ReadOutcome, BusError> {
        // ASSUMPTION: an instruction fetch through a data-bus address (the DROM window, the SRAM D-bus
        // view) is served like a load. The silicon behaviour is not established; the SP1b-4 lockstep
        // against a reference trace settles it.
        match locate(addr, size.bytes()) {
            Some(Region::Rom(offset)) => Ok(ReadOutcome {
                value: self.rom_read(offset, size),
                stall: 0,
            }),
            Some(Region::Sram(dbus_addr)) => self.sram.read(dbus_addr, size, kind),
            Some(Region::Sram0) => self.sram0.read(addr, size, kind),
            Some(Region::Rtc) => self.rtc.read(addr, size, kind),
            Some(Region::Flash { offset, .. }) => Ok(self.flash_read(addr, offset, size)),
            Some(Region::Periph) if kind != AccessKind::Fetch => self.mmio_read(addr, size),
            Some(Region::Periph) | None => Err(BusError::Unmapped { addr, kind }),
        }
    }

    fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError> {
        match locate(addr, size.bytes()) {
            Some(Region::Sram(dbus_addr)) => self.sram.write(dbus_addr, size, value),
            Some(Region::Sram0) => self.sram0.write(addr, size, value),
            Some(Region::Rtc) => self.rtc.write(addr, size, value),
            Some(Region::Periph) => self.mmio_write(addr, size, value),
            // ASSUMPTION: (decision D4) a store to a flash window raises a store access fault, as the
            // QEMU oracle does (CM §6.2); silicon latches the status bit, the fault is not established.
            Some(Region::Flash {
                instruction_bus, ..
            }) => {
                self.deliver(Event::FlashWindowStore { instruction_bus });
                Err(BusError::Unmapped {
                    addr,
                    kind: AccessKind::Store,
                })
            }
            // Decision D10: a store to the mask ROM raises a store access fault.
            Some(Region::Rom(_)) | None => Err(BusError::Unmapped {
                addr,
                kind: AccessKind::Store,
            }),
        }
    }

    fn fetch_is_memory(&self, addr: u32, len: u32) -> bool {
        match locate(addr, len) {
            // Decision D23 serves 0 for a flash window byte no flash byte backs: that is no
            // content to keep, and each such fetch is to be latched and reported.
            Some(Region::Flash { offset, .. }) => {
                self.flash_offset(offset).is_ok()
                    && self.flash_offset(offset + len.saturating_sub(1)).is_ok()
            }
            _ => true,
        }
    }

    fn page_version(&self, addr: u32) -> u64 {
        // The SRAM keeps one version per physical page: code decoded at an instruction-bus address is
        // invalidated by a store through the data bus, because both resolve to the same cell.
        match locate(addr, 1) {
            Some(Region::Sram(dbus_addr)) => self.sram.page_version(dbus_addr),
            Some(Region::Sram0) => self.sram0.page_version(addr),
            Some(Region::Rtc) => self.rtc.page_version(addr),
            // What an address of a flash window shows changes with the MMU table, with the cache
            // enables and with the content of the chip: a program or an erase that changes a byte
            // moves the generation of the chip, so the code decoded from the flash before it is
            // decoded again (spec §4.1: the decode cache is invalidated when the flash is
            // rewritten). An invalidation that takes a page out of the cache moves the second
            // counter too: it shows nothing new, but the code of that page must reach the bus
            // again to be seen as read again (`Extmem::generation`). The three counters only
            // grow, so their sum changes whenever one does (decision D6).
            // The first two restart at 0 when a core reset rebuilds the MMU and the
            // cache registers, so a version can repeat with another mapping: that is safe only
            // because `Hart::reset` builds a new decode cache, at every level (`Esp32C3::reset`).
            // The version is one number for both windows, not one per flash page: a write
            // anywhere in the chip drops all the decoded flash code, which costs a decode and
            // can never keep a stale instruction.
            Some(Region::Flash { .. }) => {
                self.periph.mmu.generation()
                    + self.periph.extmem.generation()
                    + self.periph.spi1.flash().generation()
            }
            Some(Region::Rom(_) | Region::Periph) | None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::STRAP_SPI_FAST_FLASH_BOOT;
    use crate::flash::NorFlash;
    use crate::test_support::unmodeled_register as unmodeled;
    use emu_core::{
        AccessKind::{Fetch, Load, Store},
        AccessSize::{Byte, Half, Word},
    };

    fn bus_with(config: SocConfig) -> SocBus {
        let mut rom = vec![0u8; ROM_SIZE as usize];
        rom[..4].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        rom[0x4_0000..0x4_0004].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        SocBus::new(
            RomImage::from_flat(rom).unwrap(),
            Box::new(NorFlash::blank(config.flash)),
            &config,
        )
    }
    fn bus() -> SocBus {
        bus_with(SocConfig::default())
    }
    fn load(bus: &mut SocBus, addr: u32) -> u32 {
        bus.read(addr, Word, Load).unwrap().value
    }

    #[test]
    fn rom_is_visible_on_the_instruction_bus_and_its_upper_part_on_the_data_bus() {
        let mut b = bus();
        assert_eq!(b.read(ROM_BASE, Word, Fetch).unwrap().value, 0x4433_2211);
        assert_eq!(
            load(&mut b, ROM_BASE),
            0x4433_2211,
            "_init reads its tables through the instruction bus"
        );
        assert_eq!(
            load(&mut b, DROM_BASE),
            0xDDCC_BBAA,
            "DROM is the ROM from offset 0x40000"
        );
        assert_eq!(load(&mut b, ROM_BASE + DROM_ROM_OFFSET), 0xDDCC_BBAA);
        assert_eq!(b.read(ROM_BASE + 1, Half, Load).unwrap().value, 0x3322);
    }

    #[test]
    fn a_fetch_through_a_data_bus_address_is_served_like_a_load() {
        // Pins the ASSUMPTION documented on `Bus::read`.
        let mut b = bus();
        assert_eq!(b.read(DROM_BASE, Word, Fetch).unwrap().value, 0xDDCC_BBAA);
        b.write(SRAM_DBUS_BASE + 0x40, Word, 0x0000_0013).unwrap();
        assert_eq!(
            b.read(SRAM_DBUS_BASE + 0x40, Word, Fetch).unwrap().value,
            0x13
        );
    }

    #[test]
    fn the_last_word_of_the_rom_reads_and_one_straddling_its_end_is_unmapped() {
        let mut rom = vec![0u8; ROM_SIZE as usize];
        let end = rom.len();
        rom[end - 4..].copy_from_slice(&[1, 2, 3, 4]);
        let mut b = SocBus::new(
            RomImage::from_flat(rom).unwrap(),
            Box::new(NorFlash::blank(SocConfig::default().flash)),
            &SocConfig::default(),
        );
        assert_eq!(load(&mut b, ROM_BASE + ROM_SIZE - 4), 0x0403_0201);
        let straddling = ROM_BASE + ROM_SIZE - 2;
        assert_eq!(
            b.read(straddling, Word, Load),
            Err(BusError::Unmapped {
                addr: straddling,
                kind: Load
            })
        );
    }

    #[test]
    fn store_to_rom_faults() {
        let mut b = bus();
        assert_eq!(
            b.write(ROM_BASE, Word, 0),
            Err(BusError::Unmapped {
                addr: ROM_BASE,
                kind: Store
            })
        );
        assert_eq!(
            b.write(DROM_BASE, Word, 0),
            Err(BusError::Unmapped {
                addr: DROM_BASE,
                kind: Store
            })
        );
    }

    #[test]
    fn sram_is_one_memory_seen_through_two_buses() {
        let mut b = bus();
        b.write(SRAM_DBUS_BASE + 0x100, Word, 0xCAFE_F00D).unwrap();
        assert_eq!(
            b.read(SRAM_IBUS_BASE + 0x100, Word, Fetch).unwrap().value,
            0xCAFE_F00D
        );
        b.write(SRAM_IBUS_BASE + 0x5_FFFC, Word, 7).unwrap();
        assert_eq!(load(&mut b, SRAM_DBUS_BASE + 0x5_FFFC), 7);
    }

    #[test]
    fn write_through_the_data_bus_invalidates_code_cached_at_the_instruction_bus_address() {
        let mut b = bus();
        let code = SRAM_IBUS_BASE + 0x2000;
        let before = b.page_version(code);
        b.write(SRAM_DBUS_BASE + 0x2000, Word, 0x13).unwrap();
        assert_ne!(b.page_version(code), before);
        assert_eq!(
            b.page_version(code),
            b.page_version(SRAM_DBUS_BASE + 0x2000)
        );
        let other = b.page_version(SRAM_IBUS_BASE + 0x4000);
        b.write(SRAM_DBUS_BASE + 0x2004, Word, 0x13).unwrap();
        assert_eq!(
            b.page_version(SRAM_IBUS_BASE + 0x4000),
            other,
            "another page is untouched"
        );
    }

    #[test]
    fn sram0_and_rtc_memory_are_plain_ram() {
        let mut b = bus();
        b.write(SRAM0_BASE, Word, 1).unwrap();
        b.write(RTC_RAM_BASE + RTC_RAM_SIZE - 4, Word, 2).unwrap();
        assert_eq!(
            (
                load(&mut b, SRAM0_BASE),
                load(&mut b, RTC_RAM_BASE + RTC_RAM_SIZE - 4)
            ),
            (1, 2)
        );
    }

    #[test]
    fn addresses_outside_every_region_are_unmapped() {
        let mut b = bus();
        for addr in [
            0,
            FLASH_DBUS_BASE - 4,
            FLASH_IBUS_BASE - 4,
            SRAM_DBUS_BASE - 4,
            ROM_BASE + ROM_SIZE,
            RTC_RAM_BASE + RTC_RAM_SIZE,
            PERIPH_BASE + PERIPH_SIZE,
        ] {
            assert_eq!(
                b.read(addr, Word, Load),
                Err(BusError::Unmapped { addr, kind: Load }),
                "{addr:#x}"
            );
        }
        // An access that starts inside a region and ends outside it is unmapped too.
        assert!(b.read(RTC_RAM_BASE + RTC_RAM_SIZE - 2, Word, Load).is_err());
        assert_eq!(
            b.read(PERIPH_BASE, Word, Fetch),
            Err(BusError::Unmapped {
                addr: PERIPH_BASE,
                kind: Fetch
            }),
            "peripherals hold no code"
        );
    }

    #[test]
    fn modeled_registers_answer_without_a_diagnostic() {
        let mut b = bus();
        assert_eq!(
            load(&mut b, 0x6000_4038),
            STRAP_SPI_FAST_FLASH_BOOT,
            "GPIO.STRAP"
        );
        b.write(0x6000_9054, Word, 0x1234).unwrap(); // IO_MUX.GPIO20
        assert_eq!(load(&mut b, 0x6000_9054), 0x1234);
        assert_eq!(b.ctx.diag.drain(), []);
    }

    #[test]
    fn repeated_access_warns_once_and_counts() {
        let mut b = bus();
        let reg = unmodeled();
        b.set_pc(0x4000_0040);
        for _ in 0..1000 {
            assert_eq!(load(&mut b, reg.addr), reg.reset_value);
        }
        let listed = b.ctx.diag.drain();
        assert_eq!(
            listed,
            [Diagnostic::UnmodeledMmio {
                addr: reg.addr,
                direction: Direction::Read,
                pc: 0x4000_0040,
                register: Some((reg.peripheral, reg.name)),
                value: reg.reset_value,
                reset_explicit: true,
            }]
        );
        assert_eq!(b.ctx.diag.occurrences(&listed[0]), 1000);
    }

    #[test]
    fn unmodeled_write_is_ignored_and_reported() {
        let mut b = bus();
        let reg = unmodeled();
        b.write(reg.addr, Word, 0xFFFF_FFFF).unwrap();
        assert_eq!(
            load(&mut b, reg.addr),
            reg.reset_value,
            "the write did not stick"
        );
        assert_eq!(
            b.ctx.diag.drain().len(),
            2,
            "one diagnostic for the write, one for the read"
        );
    }

    #[test]
    fn gap_in_peripheral_window_reads_zero_and_warns() {
        let mut b = bus();
        assert_eq!(load(&mut b, 0x600C_5200), 0); // right after the MMU table: nothing is documented there
        assert!(matches!(
            b.ctx.diag.drain()[..],
            [Diagnostic::UnmodeledMmio {
                register: None,
                value: 0,
                ..
            }]
        ));
    }

    #[test]
    fn sub_word_read_extracts_the_addressed_bytes() {
        let mut b = bus();
        b.write(0x6000_9054, Word, 0xA1B2_C3D4).unwrap();
        assert_eq!(b.read(0x6000_9054, Byte, Load).unwrap().value, 0xD4);
        assert_eq!(b.read(0x6000_9055, Byte, Load).unwrap().value, 0xC3);
        assert_eq!(b.read(0x6000_9056, Half, Load).unwrap().value, 0xA1B2);
    }

    #[test]
    fn sub_word_write_is_reported() {
        let mut b = bus();
        b.set_pc(0x4000_0100);
        b.write(0x6000_9054, Word, 0xFFFF_FFFF).unwrap();
        b.write(0x6000_9055, Byte, 0x7E).unwrap();
        assert_eq!(
            load(&mut b, 0x6000_9054),
            0x0000_7E00,
            "the whole register is written, other bytes as zero"
        );
        assert_eq!(
            b.ctx.diag.drain(),
            [Diagnostic::SubWordWrite {
                addr: 0x6000_9055,
                bytes: 1,
                pc: 0x4000_0100
            }]
        );
    }

    #[test]
    fn misaligned_mmio_access_is_a_bus_error() {
        let mut b = bus();
        assert_eq!(
            b.read(0x6000_9055, Word, Load),
            Err(BusError::Misaligned {
                addr: 0x6000_9055,
                size: Word
            })
        );
        assert_eq!(
            b.write(0x6000_9055, Half, 0),
            Err(BusError::Misaligned {
                addr: 0x6000_9055,
                size: Half
            })
        );
    }

    #[test]
    fn strict_mode_turns_an_unmodeled_access_into_a_violation() {
        let mut b = bus_with(SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        });
        let _ = load(&mut b, unmodeled().addr);
        assert!(matches!(
            b.ctx.diag.take_violation(),
            Some(Diagnostic::UnmodeledMmio { .. })
        ));
    }

    #[test]
    fn strict_mode_turns_an_unmodeled_write_into_a_violation() {
        let mut b = bus_with(SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        });
        b.write(unmodeled().addr, Word, 1).unwrap();
        assert!(matches!(
            b.ctx.diag.take_violation(),
            Some(Diagnostic::UnmodeledMmio {
                direction: Direction::Write,
                ..
            })
        ));
    }

    #[test]
    fn rtc_cntl_and_efuse_share_a_page_but_not_their_registers() {
        let mut b = bus();
        assert_eq!(load(&mut b, 0x6000_8038) & 0x3F, 1, "RTC_CNTL.RESET_STATE");
        assert_eq!(
            load(&mut b, 0x6000_8850),
            0x020C_0000,
            "EFUSE.RD_MAC_SPI_SYS_3 of the production v0.3 chip"
        );
        assert_eq!(
            load(&mut b, 0x600C_0058) >> 12 & 0x7F,
            40,
            "SYSTEM.SYSCLK_CONF"
        );
        assert_eq!(load(&mut b, 0x600C_2100), 1, "INTERRUPT_CORE0.CLOCK_GATE");
        assert_eq!(
            load(&mut b, 0x600C_E048),
            0,
            "ASSIST_DEBUG.CORE_0_RCD_PDEBUGPC"
        );
        assert_eq!(b.ctx.diag.drain(), []);
    }

    #[test]
    fn efuse_reserved_hole_is_reported_as_unmodeled() {
        let mut b = bus();
        assert_eq!(load(&mut b, 0x6000_8800 + 0x194), 0);
        let reported = b.ctx.diag.drain();
        assert!(
            matches!(
                reported[..],
                [Diagnostic::UnmodeledMmio {
                    addr: 0x6000_8994,
                    direction: Direction::Read,
                    register: None,
                    ..
                }]
            ),
            "{reported:?}"
        );
    }

    #[test]
    fn an_unmodelled_flash_command_becomes_a_named_diagnostic_and_a_strict_violation() {
        let cmd = svd_regs::esp32c3::addr("SPI1", "CMD");
        let mut b = bus_with(SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        });
        b.set_pc(0x4038_0010);
        b.write(cmd, Word, 1 << 19).unwrap(); // high-performance mode, which the chip does not know
        let expected = Diagnostic::Unsupported {
            what: crate::diag::Unsupported::FlashCommand { opcode: 0xA3 },
            pc: 0x4038_0010,
        };
        assert_eq!(b.ctx.diag.drain(), vec![expected.clone()]);
        assert_eq!(b.ctx.diag.take_violation(), Some(expected));
        assert_eq!(load(&mut b, cmd), 0);
    }

    #[test]
    fn a_write_of_several_flash_command_bits_is_one_named_diagnostic_and_a_strict_violation() {
        let cmd = svd_regs::esp32c3::addr("SPI1", "CMD");
        let mut b = bus_with(SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        });
        b.set_pc(0x4038_0020);
        b.write(cmd, Word, (1 << 25) | (1 << 24)).unwrap(); // page program and sector erase
        let refused = Diagnostic::Unsupported {
            what: crate::diag::Unsupported::FlashCommandBits { cmd: 0x0300_0000 },
            pc: 0x4038_0020,
        };
        assert_eq!(b.ctx.diag.drain(), vec![refused.clone()]);
        assert_eq!(b.ctx.diag.take_violation(), Some(refused));
    }

    #[test]
    fn consoles_are_reachable_through_the_bus() {
        let mut b = bus();
        b.write(0x6000_0000, Word, u32::from(b'U')).unwrap();
        b.write(0x6004_3000, Word, u32::from(b'J')).unwrap();
        b.write(0x6004_3004, Word, 1).unwrap();
        b.write(0x6001_0010, Word, 1).unwrap(); // UART1.INT_CLR, written by the ROM's uartAttach
        // A byte takes 10 bit times to leave the UART: 347 us at the reset baud rate (55 520 cycles
        // of the time base).
        b.ctx.sleep_until(emu_core::Cycles(60_000));
        assert_eq!(b.periph.uart0.take_tx(b.ctx.now()), b"U");
        assert_eq!(b.periph.usb_serial_jtag.take_tx(), b"J");
        assert_eq!(b.ctx.diag.drain(), []);
    }

    /// A bus whose flash holds 0x11 at offset 0, then `page << 4 | n` patterns at the start of pages 1 and 2.
    fn bus_with_flash(config: SocConfig) -> SocBus {
        let mut image = vec![0u8; 0x3_0000];
        image[..4].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        image[0x1_0000..0x1_0004].copy_from_slice(&[0xE9, 0x06, 0x02, 0x4F]);
        image[0x1_FFFC..0x2_0004]
            .copy_from_slice(&[0xA0, 0xA1, 0xA2, 0xA3, 0xB0, 0xB1, 0xB2, 0xB3]);
        let flash = NorFlash::with_image(image, config.flash).unwrap();
        SocBus::new(
            RomImage::from_flat(vec![0u8; ROM_SIZE as usize]).unwrap(),
            Box::new(flash),
            &config,
        )
    }
    fn map(b: &mut SocBus, entry: u32, page: u32) {
        b.write(crate::periph::mmu::MMU_TABLE_BASE + 4 * entry, Word, page)
            .unwrap();
    }

    #[test]
    fn one_entry_maps_the_same_flash_page_into_both_windows() {
        // CM §7 T2: write entry 3 = 1; 0x3C030000 and 0x42030000 both read flash[0x10000..].
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 3, 1);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE + 0x3_0000), 0x4F02_06E9);
        assert_eq!(load(&mut b, FLASH_IBUS_BASE + 0x3_0000), 0x4F02_06E9);
        assert_eq!(
            b.read(FLASH_IBUS_BASE + 0x3_0000, Half, Fetch)
                .unwrap()
                .value,
            0x06E9
        );
        assert_eq!(
            b.read(FLASH_DBUS_BASE + 0x3_0003, Byte, Load)
                .unwrap()
                .value,
            0x4F
        );
        assert_eq!(b.ctx.diag.drain(), Vec::new());
    }

    #[test]
    fn a_remap_is_visible_at_once() {
        // CM §7 T3: the window is a live view (no cache contents, decision D4).
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 1);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4F02_06E9);
        map(&mut b, 0, 0);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4433_2211);
    }

    #[test]
    fn what_the_image_does_not_hold_reads_erased() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 0x80);
        assert_eq!(
            load(&mut b, FLASH_DBUS_BASE),
            0xFFFF_FFFF,
            "page 0x80 = offset 8 MiB, inside the 16 MiB chip"
        );
    }

    #[test]
    fn invalid_entry_reads_zero_latches_the_fault_and_is_reported() {
        let mut b = bus_with_flash(SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        });
        b.set_pc(0x4004_a186);
        let vaddr = FLASH_DBUS_BASE + 0x5_0010;
        assert_eq!(load(&mut b, vaddr), 0);
        let expected = Diagnostic::FlashMapping {
            vaddr,
            entry: 5,
            content: 0x100,
            fault: crate::diag::MappingFault::InvalidEntry,
            pc: 0x4004_a186,
        };
        assert_eq!(b.ctx.diag.drain(), vec![expected.clone()]);
        assert_eq!(b.ctx.diag.take_violation(), Some(expected));
        let st = svd_regs::esp32c3::addr("EXTMEM", "CACHE_ILG_INT_ST");
        assert_eq!(load(&mut b, st), 1 << 5);
        assert_eq!(
            load(
                &mut b,
                svd_regs::esp32c3::addr("EXTMEM", "CACHE_MMU_FAULT_VADDR")
            ),
            vaddr
        );
        assert_eq!(
            b.read(FLASH_IBUS_BASE + 0x5_0000, Half, Fetch)
                .unwrap()
                .value,
            0,
            "a fetch decodes 0: illegal instruction"
        );
    }

    #[test]
    fn one_access_through_an_invalid_entry_is_one_occurrence_whatever_its_width() {
        // "repeated N times" counts accesses: a 4-byte load is not four events.
        for (size, bytes) in [(Word, 4u32), (Half, 2), (Byte, 1)] {
            let mut b = bus_with_flash(SocConfig::default());
            let vaddr = FLASH_DBUS_BASE + 0x5_0010;
            assert_eq!(b.read(vaddr, size, Load).unwrap().value, 0);
            let listed = b.ctx.diag.drain();
            assert_eq!(listed.len(), 1, "{bytes}-byte load: {listed:?}");
            assert_eq!(b.ctx.diag.occurrences(&listed[0]), 1, "{bytes}-byte load");
            assert_eq!(b.read(vaddr, size, Load).unwrap().value, 0);
            assert_eq!(b.ctx.diag.occurrences(&listed[0]), 2, "the second access");
        }
    }

    #[test]
    fn a_load_straddling_two_faulting_entries_reports_each_of_them_once() {
        let mut b = bus_with_flash(SocConfig::default());
        b.set_pc(0x4038_0040);
        let vaddr = FLASH_DBUS_BASE + 0x5_FFFE; // two bytes through entry 5, two through entry 6
        assert_eq!(load(&mut b, vaddr), 0);
        let fault = |entry| Diagnostic::FlashMapping {
            vaddr,
            entry,
            content: 0x100,
            fault: MappingFault::InvalidEntry,
            pc: 0x4038_0040,
        };
        assert_eq!(b.ctx.diag.drain(), vec![fault(5), fault(6)]);
        assert_eq!(b.ctx.diag.occurrences(&fault(5)), 1);
        assert_eq!(b.ctx.diag.occurrences(&fault(6)), 1);
        // One valid half, one faulting half: the valid bytes are read, the fault is one occurrence.
        map(&mut b, 5, 1); // window page 5 -> flash page 1, which ends with A0 A1 A2 A3
        assert_eq!(load(&mut b, vaddr), 0x0000_A3A2);
        assert_eq!(b.ctx.diag.drain(), Vec::new(), "entry 6 is already listed");
        assert_eq!(b.ctx.diag.occurrences(&fault(5)), 1);
        assert_eq!(b.ctx.diag.occurrences(&fault(6)), 2);
    }

    #[test]
    fn page_beyond_the_flash_is_reported() {
        // A 1 MiB chip answers capacity byte 0x14: the config type refuses any other pairing.
        let one_mib = crate::flash::JedecId {
            capacity: 0x14,
            ..crate::flash::JedecId::QEMU_ORACLE
        };
        let small = SocConfig {
            flash: crate::flash::FlashConfig::new(1 << 20, one_mib).unwrap(),
            ..SocConfig::default()
        };
        let flash = NorFlash::with_image(vec![0xAB; 16], small.flash).unwrap();
        let mut b = SocBus::new(
            RomImage::from_flat(vec![0u8; ROM_SIZE as usize]).unwrap(),
            Box::new(flash),
            &small,
        );
        map(&mut b, 0, 0x10); // 1 MiB: the first page past a 1 MiB chip
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0);
        assert!(matches!(
            b.ctx.diag.drain().as_slice(),
            [Diagnostic::FlashMapping {
                entry: 0,
                content: 0x10,
                fault: crate::diag::MappingFault::BeyondFlash,
                ..
            }]
        ));
        map(&mut b, 0, 0x0F);
        assert_eq!(
            load(&mut b, FLASH_DBUS_BASE),
            0xFFFF_FFFF,
            "the last page of the chip is fine"
        );
    }

    #[test]
    fn load_straddling_two_pages_reads_each_byte_through_its_own_entry() {
        // The machine performs misaligned loads; one that crosses a 64 KiB page uses two entries.
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 1); // window page 0 -> flash page 1, ends with A0 A1 A2 A3
        map(&mut b, 1, 0); // window page 1 -> flash page 0, starts with 11 22 33 44
        assert_eq!(load(&mut b, FLASH_DBUS_BASE + 0xFFFE), 0x2211_A3A2);
        map(&mut b, 1, 2); // -> flash page 2, starts with B0 B1 B2 B3
        assert_eq!(load(&mut b, FLASH_DBUS_BASE + 0xFFFE), 0xB1B0_A3A2);
    }

    #[test]
    fn the_instruction_window_serves_a_straddling_fetch_and_its_last_bytes() {
        // The same read path as the data window, reached through the other base and as a fetch.
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 1); // window page 0 -> flash page 1, which ends with A0 A1 A2 A3
        map(&mut b, 1, 2); // window page 1 -> flash page 2, which starts with B0 B1 B2 B3
        let straddling = FLASH_IBUS_BASE + 0xFFFE;
        assert_eq!(
            b.read(straddling, Word, Fetch).unwrap().value,
            0xB1B0_A3A2,
            "a 32-bit instruction across two 64 KiB pages"
        );
        assert_eq!(b.read(straddling, Half, Fetch).unwrap().value, 0xA3A2);
        map(&mut b, 127, 1);
        let last = FLASH_IBUS_BASE + FLASH_WINDOW_SIZE - 4;
        assert_eq!(b.read(last, Word, Fetch).unwrap().value, 0xA3A2_A1A0);
        assert_eq!(b.read(last + 2, Half, Fetch).unwrap().value, 0xA3A2);
        assert_eq!(b.read(last + 3, Byte, Fetch).unwrap().value, 0xA3);
        assert_eq!(
            b.read(last + 2, Word, Fetch),
            Err(BusError::Unmapped {
                addr: last + 2,
                kind: Fetch
            }),
            "a fetch that runs past the window"
        );
        assert_eq!(b.ctx.diag.drain(), Vec::new());
    }

    #[test]
    fn access_straddling_the_end_of_the_window_is_unmapped() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 127, 0);
        let last = FLASH_DBUS_BASE + FLASH_WINDOW_SIZE - 4;
        assert!(b.read(last, Word, Load).is_ok());
        for addr in [
            last + 2,
            FLASH_DBUS_BASE + FLASH_WINDOW_SIZE,
            FLASH_IBUS_BASE + FLASH_WINDOW_SIZE,
        ] {
            assert_eq!(
                b.read(addr, Word, Load),
                Err(BusError::Unmapped { addr, kind: Load }),
                "CM §6.2: load at 0x3C800000 faults"
            );
        }
    }

    #[test]
    fn a_store_to_a_window_faults_and_is_latched() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 0);
        for (addr, bit) in [(FLASH_DBUS_BASE, 1u32 << 5), (FLASH_IBUS_BASE, 1 << 1)] {
            assert_eq!(
                b.write(addr, Word, 0),
                Err(BusError::Unmapped { addr, kind: Store })
            );
            let st = svd_regs::esp32c3::addr("EXTMEM", "CORE0_ACS_CACHE_INT_ST");
            assert_ne!(load(&mut b, st) & bit, 0);
        }
        assert_eq!(
            load(&mut b, FLASH_DBUS_BASE),
            0x4433_2211,
            "the flash did not change"
        );
    }

    #[test]
    fn mapping_and_cache_enable_writes_change_the_version_of_both_windows() {
        let mut b = bus_with_flash(SocConfig::default());
        let versions = |b: &SocBus| {
            (
                b.page_version(FLASH_IBUS_BASE + 0x2000),
                b.page_version(FLASH_DBUS_BASE + 0x7F_0000),
            )
        };
        let start = versions(&b);
        assert_eq!(start.0, start.1);
        map(&mut b, 0, 0);
        let mapped = versions(&b);
        assert_ne!(mapped, start);
        b.write(svd_regs::esp32c3::addr("EXTMEM", "ICACHE_CTRL"), Word, 1)
            .unwrap();
        let enabled = versions(&b);
        assert_ne!(enabled, mapped);
        b.write(svd_regs::esp32c3::addr("EXTMEM", "ICACHE_CTRL1"), Word, 0)
            .unwrap();
        assert_ne!(versions(&b), enabled);
        let sram = b.page_version(SRAM_IBUS_BASE);
        map(&mut b, 1, 1);
        assert_eq!(
            b.page_version(SRAM_IBUS_BASE),
            sram,
            "RAM versions are untouched"
        );
    }

    #[test]
    fn a_program_or_an_erase_that_changes_the_flash_changes_the_version_of_both_windows() {
        // `SPI1.CMD` bits: write enable, page program, sector erase.
        const WREN: u32 = 1 << 30;
        const PP: u32 = 1 << 25;
        const SE: u32 = 1 << 24;
        let mut b = bus_with_flash(SocConfig::default());
        let versions = |b: &SocBus| {
            (
                b.page_version(FLASH_IBUS_BASE + 0x2000),
                b.page_version(FLASH_DBUS_BASE + 0x7F_0000),
            )
        };
        let spi1 = |b: &mut SocBus, register: &str, value: u32| {
            b.write(svd_regs::esp32c3::addr("SPI1", register), Word, value)
                .unwrap();
        };
        let start = versions(&b);
        let sram = b.page_version(SRAM_IBUS_BASE);
        // One byte, 0x00, at flash offset 0x10000, which holds 0xE9. Without the write-enable
        // latch the chip ignores the program: nothing changed, nothing is dropped.
        spi1(&mut b, "W0", 0);
        spi1(&mut b, "ADDR", 0x0101_0000);
        spi1(&mut b, "CMD", PP);
        assert_eq!(versions(&b), start);
        spi1(&mut b, "CMD", WREN);
        spi1(&mut b, "CMD", PP);
        let programmed = versions(&b);
        assert_ne!(programmed, start, "a program changed a byte");
        assert_eq!(programmed.0, programmed.1, "one version for both windows");
        // The same program again changes no byte.
        spi1(&mut b, "CMD", WREN);
        spi1(&mut b, "CMD", PP);
        assert_eq!(versions(&b), programmed);
        spi1(&mut b, "CMD", WREN);
        spi1(&mut b, "ADDR", 0x1_0000);
        spi1(&mut b, "CMD", SE);
        assert_ne!(versions(&b), programmed, "an erase changed a byte");
        assert_eq!(
            b.page_version(SRAM_IBUS_BASE),
            sram,
            "RAM versions are untouched"
        );
        assert_eq!(b.ctx.diag.drain(), Vec::new());
    }

    fn spi1(b: &mut SocBus, register: &str, value: u32) {
        b.write(svd_regs::esp32c3::addr("SPI1", register), Word, value)
            .unwrap();
    }
    /// Programs one byte through SPI1 as the ROM and ESP-IDF do: write enable, then page program.
    fn program_byte(b: &mut SocBus, flash_address: u32, byte: u8) {
        spi1(b, "CMD", 1 << 30);
        spi1(b, "W0", u32::from(byte));
        spi1(b, "ADDR", (1 << 24) | flash_address);
        spi1(b, "CMD", 1 << 25);
    }
    fn erase_sector(b: &mut SocBus, flash_address: u32) {
        spi1(b, "CMD", 1 << 30);
        spi1(b, "ADDR", flash_address);
        spi1(b, "CMD", 1 << 24);
    }
    /// `Cache_Invalidate_ICache_Items(addr, lines)` (CM §4.1).
    fn invalidate(b: &mut SocBus, addr: u32, lines: u32) {
        for (register, value) in [
            ("ICACHE_SYNC_ADDR", addr),
            ("ICACHE_SYNC_SIZE", lines),
            ("ICACHE_SYNC_CTRL", 1),
        ] {
            b.write(svd_regs::esp32c3::addr("EXTMEM", register), Word, value)
                .unwrap();
        }
    }
    fn stale(vaddr: u32, page: u32) -> Diagnostic {
        Diagnostic::StaleFlashWindow { vaddr, page, pc: 0 }
    }

    #[test]
    fn a_window_read_of_flash_programmed_since_it_was_read_is_reported_and_returns_the_flash() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 3, 1);
        map(&mut b, 0, 0);
        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
        assert_eq!(load(&mut b, vaddr), 0x4F02_06E9);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4433_2211);
        program_byte(&mut b, 0x1_0000, 0x00);
        assert_eq!(
            b.ctx.diag.drain(),
            Vec::new(),
            "the write itself is faithful"
        );
        assert_eq!(
            load(&mut b, vaddr),
            0x4F02_0600,
            "the window stays a view of the flash: the new byte is returned"
        );
        assert_eq!(b.ctx.diag.drain(), vec![stale(vaddr, 1)]);
        // The same page through the other window and at another address: the same line, counted.
        assert_eq!(load(&mut b, FLASH_IBUS_BASE + 0x3_FFFC), 0xA3A2_A1A0);
        assert_eq!(b.ctx.diag.drain(), Vec::new());
        assert_eq!(b.ctx.diag.occurrences(&stale(0, 1)), 2);
        // Page 0 was read too, and not written: its reads are those of a device.
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4433_2211);
        assert_eq!(b.ctx.diag.occurrences(&stale(0, 0)), 0);
    }

    #[test]
    fn an_invalidation_of_the_page_ends_the_report_until_it_is_written_again() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 3, 1);
        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
        load(&mut b, vaddr);
        program_byte(&mut b, 0x1_0000, 0x00);
        // What `spi_flash_check_and_flush_cache` does after a write to mapped flash.
        invalidate(&mut b, vaddr, 0x800);
        assert_eq!(load(&mut b, vaddr), 0x4F02_0600);
        assert_eq!(
            b.ctx.diag.drain(),
            Vec::new(),
            "a device reads the flash again too"
        );
        // That read filled the cache again; an erase makes it stale again.
        erase_sector(&mut b, 0x1_0000);
        assert_eq!(load(&mut b, vaddr), 0xFFFF_FFFF);
        assert_eq!(b.ctx.diag.drain(), vec![stale(vaddr, 1)]);
        // `Cache_Invalidate_ICache_All`, as `Cache_Disable_ICache` issues it.
        invalidate(&mut b, 0, 0);
        load(&mut b, vaddr);
        assert_eq!(b.ctx.diag.occurrences(&stale(0, 1)), 1, "no new occurrence");
    }

    #[test]
    fn an_invalidation_at_one_address_of_a_page_ends_the_report_of_its_reads_at_another() {
        // Pins the ASSUMPTION on `Extmem::invalidate`: the cache state is kept per flash page,
        // so a request drops the page whichever address it was read at.
        let other_window = FLASH_IBUS_BASE + 0x3_0000;
        let other_entry = FLASH_DBUS_BASE + 0x4_0000;
        for (read_at, invalidated_at) in [
            (FLASH_DBUS_BASE + 0x3_0000, other_window),
            (other_entry, FLASH_DBUS_BASE + 0x3_0000),
        ] {
            let mut b = bus_with_flash(SocConfig::default());
            map(&mut b, 3, 1);
            map(&mut b, 4, 1); // a second entry for the same flash page
            load(&mut b, read_at);
            program_byte(&mut b, 0x1_0000, 0x00);
            invalidate(&mut b, invalidated_at, 0x800);
            assert_eq!(load(&mut b, read_at), 0x4F02_0600);
            assert_eq!(
                b.ctx.diag.drain(),
                Vec::new(),
                "read at {read_at:#010x}, invalidated at {invalidated_at:#010x}"
            );
        }
    }

    #[test]
    fn a_request_one_byte_into_an_entry_or_at_an_entry_that_maps_nothing_keeps_the_report() {
        // The other ASSUMPTION on `Extmem::invalidate`: the range starts at the address as
        // written, so the entry it starts inside is not whole in it.
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 3, 1);
        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
        load(&mut b, vaddr);
        program_byte(&mut b, 0x1_0000, 0x00);
        invalidate(&mut b, vaddr + 1, 0x800);
        load(&mut b, vaddr);
        assert_eq!(b.ctx.diag.drain(), vec![stale(vaddr, 1)]);
        // A request issued while the entry maps nothing names no flash page, whatever page
        // number the invalid entry holds: the lines of the page it mapped are still there when
        // it maps it again.
        map(&mut b, 3, crate::periph::mmu::MMU_ENTRY_INVALID | 1);
        invalidate(&mut b, vaddr, 0x800);
        map(&mut b, 3, 1);
        load(&mut b, vaddr);
        assert_eq!(b.ctx.diag.occurrences(&stale(0, 1)), 2);
    }

    #[test]
    fn flash_written_before_a_window_read_it_or_left_as_it_was_is_not_reported() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 3, 1);
        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
        // Written first, read after: no cache line held the bytes of before.
        program_byte(&mut b, 0x1_0000, 0xE8);
        assert_eq!(load(&mut b, vaddr), 0x4F02_06E8);
        // Read, then a program that changes no byte (0xE8 AND 0xE8).
        program_byte(&mut b, 0x1_0000, 0xE8);
        assert_eq!(load(&mut b, vaddr), 0x4F02_06E8);
        // Read, then a write to a page no window read.
        program_byte(&mut b, 0x2_0000, 0x00);
        assert_eq!(load(&mut b, vaddr), 0x4F02_06E8);
        // Switching the cache off and on is no invalidation and no write.
        for (register, value) in [("ICACHE_CTRL", 0), ("ICACHE_CTRL", 1)] {
            b.write(svd_regs::esp32c3::addr("EXTMEM", register), Word, value)
                .unwrap();
        }
        assert_eq!(load(&mut b, vaddr), 0x4F02_06E8);
        assert_eq!(b.ctx.diag.drain(), Vec::new());
    }

    #[test]
    fn a_chip_erase_makes_stale_the_pages_it_changed_and_no_page_that_held_nothing() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 3, 1);
        map(&mut b, 5, 0x80); // past the image: erased already
        let held = FLASH_DBUS_BASE + 0x3_0000;
        let blank = FLASH_DBUS_BASE + 0x5_0000;
        assert_eq!(load(&mut b, held), 0x4F02_06E9);
        assert_eq!(load(&mut b, blank), 0xFFFF_FFFF);
        // Write enable, then `FLASH_CE`: the erase of the whole chip.
        spi1(&mut b, "CMD", 1 << 30);
        spi1(&mut b, "CMD", 1 << 22);
        assert_eq!(load(&mut b, blank), 0xFFFF_FFFF);
        assert_eq!(
            b.ctx.diag.drain(),
            Vec::new(),
            "the erase changed no byte of that page: a line a device kept holds what the flash does"
        );
        assert_eq!(load(&mut b, held), 0xFFFF_FFFF);
        assert_eq!(b.ctx.diag.drain(), vec![stale(held, 1)]);
        assert_eq!(b.ctx.diag.occurrences(&stale(0, 0x80)), 0);
    }

    #[test]
    fn one_load_that_straddles_a_stale_page_is_one_occurrence_at_its_address() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 3, 1);
        map(&mut b, 4, 2);
        let vaddr = FLASH_DBUS_BASE + 0x3_FFFE; // two bytes of flash page 1, two of page 2
        assert_eq!(load(&mut b, vaddr), 0xB1B0_A3A2);
        program_byte(&mut b, 0x2_0000, 0x00);
        assert_eq!(load(&mut b, vaddr), 0xB100_A3A2);
        assert_eq!(b.ctx.diag.drain(), vec![stale(vaddr, 2)]);
        assert_eq!(b.ctx.diag.occurrences(&stale(0, 2)), 1);
        assert_eq!(b.ctx.diag.occurrences(&stale(0, 1)), 0);
    }

    #[test]
    fn strict_mode_turns_a_window_read_that_may_differ_into_a_violation() {
        let mut b = bus_with_flash(SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        });
        map(&mut b, 3, 1);
        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
        load(&mut b, vaddr);
        program_byte(&mut b, 0x1_0000, 0x00);
        assert_eq!(
            b.ctx.diag.take_violation(),
            None,
            "the write is no violation"
        );
        load(&mut b, vaddr);
        assert_eq!(b.ctx.diag.take_violation(), Some(stale(vaddr, 1)));
    }

    #[test]
    fn a_cpu_reset_keeps_what_the_cache_holds_of_the_flash_and_a_core_reset_forgets_it() {
        // The lines of the cache are in internal memory and `EXTMEM` is no part of the CPU: a
        // CPU reset leaves both (TRM §6.1.1, §6.1.3). A core or a system reset rebuilds
        // `EXTMEM`: the ASSUMPTION on the fields of `Extmem`.
        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
        for (cause, kept) in [
            (ResetCause::SoftwareCpu, true),
            (ResetCause::Mwdt0Core, false),
            (ResetCause::RtcWatchdogSystem, false),
        ] {
            let reported = |b: &mut SocBus| {
                // A core reset rebuilt the MMU table; after a CPU reset this writes the entry
                // the value it holds.
                map(b, 3, 1);
                load(b, vaddr);
                b.ctx.diag.drain()
            };
            let expected = if kept {
                vec![stale(vaddr, 1)]
            } else {
                Vec::new()
            };
            // Read and written before the reset: stale across a CPU reset.
            let mut b = bus_with_flash(SocConfig::default());
            map(&mut b, 3, 1);
            load(&mut b, vaddr);
            program_byte(&mut b, 0x1_0000, 0x00);
            b.reset(cause, 0x4000_0000, 0);
            assert_eq!(reported(&mut b), expected, "written before {cause:?}");
            // Read before the reset, written after it: the line a device filled before a CPU
            // reset is still there for the write to make stale.
            let mut b = bus_with_flash(SocConfig::default());
            map(&mut b, 3, 1);
            load(&mut b, vaddr);
            b.reset(cause, 0x4000_0000, 0);
            program_byte(&mut b, 0x1_0000, 0x00);
            assert_eq!(reported(&mut b), expected, "written after {cause:?}");
        }
    }

    #[test]
    fn the_memories_keep_their_content_at_every_level_of_reset() {
        // TRM §6.1.1: a reset leaves the memories as they are. The RTC memory is the case the
        // manual states least plainly for a system reset: ASSUMPTION in `SocBus::reset`.
        let mut b = bus();
        let words = [
            (SRAM_DBUS_BASE + 0x40, 0x1111_1111),
            (SRAM0_BASE + 0x40, 0x2222_2222),
            (RTC_RAM_BASE + 0x40, 0x3333_3333),
        ];
        for (addr, value) in words {
            b.write(addr, Word, value).unwrap();
        }
        for cause in [
            ResetCause::SoftwareCpu,
            ResetCause::Mwdt0Core,
            ResetCause::RtcWatchdogSystem,
        ] {
            b.reset(cause, 0x4000_0000, 0);
            for (addr, value) in words {
                assert_eq!(load(&mut b, addr), value, "{addr:#010x} after {cause:?}");
            }
        }
    }
}
