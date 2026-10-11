//! The ESP32-C3 address space. Memory map: `docs/research/esp32c3-boot-chain.md`, §A.1.
use crate::config::SocConfig;
use crate::diag::{Diagnostic, Diagnostics, Direction};
use crate::flash::FlashDevice;
use crate::memmap::{
    DROM_BASE, DROM_ROM_OFFSET, DROM_SIZE, PERIPH_BASE, PERIPH_SIZE, ROM_BASE, ROM_SIZE,
    RTC_RAM_BASE, RTC_RAM_SIZE, SRAM_DBUS_BASE, SRAM_IBUS_BASE, SRAM_SIZE, SRAM0_BASE, SRAM0_SIZE,
};
use crate::periph::Peripherals;
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
    pub periph: Peripherals,
    pub diag: Diagnostics,
    /// PC of the instruction being executed, for diagnostics.
    pc: u32,
}

#[derive(Clone, Copy)]
enum Region {
    /// Offset in the ROM image.
    Rom(u32),
    /// Data-bus address of the SRAM cell, whichever bus the access came through.
    Sram(u32),
    Sram0,
    Rtc,
    Periph,
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
    #[must_use]
    pub fn new(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) -> SocBus {
        SocBus {
            rom,
            sram: Ram::new(SRAM_DBUS_BASE, SRAM_SIZE as usize),
            sram0: Ram::new(SRAM0_BASE, SRAM0_SIZE as usize),
            rtc: Ram::new(RTC_RAM_BASE, RTC_RAM_SIZE as usize),
            periph: Peripherals::new(config, flash),
            diag: Diagnostics::new(config.strict_mmio),
            pc: 0,
        }
    }

    /// Tells the bus which instruction is running, so diagnostics can name it.
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
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
        self.diag.record(Diagnostic::UnmodeledMmio {
            addr: reg_addr,
            direction,
            pc: self.pc,
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

    /// Turns everything a device could not do into diagnostics that name the instruction.
    fn relay_unsupported(&mut self) {
        while let Some(what) = self.periph.take_unsupported() {
            self.diag
                .record(Diagnostic::Unsupported { what, pc: self.pc });
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
            .and_then(|(device, base)| device.read(reg_addr - base));
        self.relay_unsupported();
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
            self.diag.record(Diagnostic::SubWordWrite {
                addr,
                bytes: size.bytes(),
                pc: self.pc,
            });
        }
        let handled = self
            .periph
            .device(reg_addr)
            .is_some_and(|(device, base)| device.write(reg_addr - base, word));
        self.relay_unsupported();
        if !handled {
            self.unmodeled(reg_addr, Direction::Write, word);
        }
        Ok(0)
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
            // Decision D10: a store to the mask ROM raises a store access fault.
            Some(Region::Rom(_)) | None => Err(BusError::Unmapped {
                addr,
                kind: AccessKind::Store,
            }),
        }
    }

    fn page_version(&self, addr: u32) -> u64 {
        // The SRAM keeps one version per physical page: code decoded at an instruction-bus address is
        // invalidated by a store through the data bus, because both resolve to the same cell.
        match locate(addr, 1) {
            Some(Region::Sram(dbus_addr)) => self.sram.page_version(dbus_addr),
            Some(Region::Sram0) => self.sram0.page_version(addr),
            Some(Region::Rtc) => self.rtc.page_version(addr),
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
            0x3C00_0000,
            0x4200_0000,
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
        assert_eq!(b.diag.drain(), []);
    }

    #[test]
    fn repeated_access_warns_once_and_counts() {
        let mut b = bus();
        let reg = unmodeled();
        b.set_pc(0x4000_0040);
        for _ in 0..1000 {
            assert_eq!(load(&mut b, reg.addr), reg.reset_value);
        }
        let listed = b.diag.drain();
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
        assert_eq!(b.diag.occurrences(&listed[0]), 1000);
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
            b.diag.drain().len(),
            2,
            "one diagnostic for the write, one for the read"
        );
    }

    #[test]
    fn gap_in_peripheral_window_reads_zero_and_warns() {
        let mut b = bus();
        assert_eq!(load(&mut b, 0x600C_5200), 0); // right after the MMU table: nothing is documented there
        assert!(matches!(
            b.diag.drain()[..],
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
            b.diag.drain(),
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
            b.diag.take_violation(),
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
            b.diag.take_violation(),
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
            3 << 18,
            "EFUSE.RD_MAC_SPI_SYS_3 of a v0.3 chip"
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
        assert_eq!(b.diag.drain(), []);
    }

    #[test]
    fn efuse_reserved_hole_is_reported_as_unmodeled() {
        let mut b = bus();
        assert_eq!(load(&mut b, 0x6000_8800 + 0x194), 0);
        let reported = b.diag.drain();
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
        b.write(cmd, Word, 1 << 24).unwrap(); // sector erase
        let expected = Diagnostic::Unsupported {
            what: crate::diag::Unsupported::FlashCommand { opcode: 0x20 },
            pc: 0x4038_0010,
        };
        assert_eq!(b.diag.drain(), vec![expected.clone()]);
        assert_eq!(b.diag.take_violation(), Some(expected));
        assert_eq!(load(&mut b, cmd), 0);
    }

    #[test]
    fn every_refused_flash_command_of_one_write_is_reported_and_the_first_stops_a_strict_run() {
        let cmd = svd_regs::esp32c3::addr("SPI1", "CMD");
        let mut b = bus_with(SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        });
        b.set_pc(0x4038_0020);
        b.write(cmd, Word, (1 << 25) | (1 << 24)).unwrap(); // page program and sector erase
        let refused = |opcode| Diagnostic::Unsupported {
            what: crate::diag::Unsupported::FlashCommand { opcode },
            pc: 0x4038_0020,
        };
        assert_eq!(b.diag.drain(), vec![refused(0x02), refused(0x20)]);
        assert_eq!(b.diag.take_violation(), Some(refused(0x02)));
    }

    #[test]
    fn consoles_are_reachable_through_the_bus() {
        let mut b = bus();
        b.write(0x6000_0000, Word, u32::from(b'U')).unwrap();
        b.write(0x6004_3000, Word, u32::from(b'J')).unwrap();
        b.write(0x6004_3004, Word, 1).unwrap();
        b.write(0x6001_0010, Word, 1).unwrap(); // UART1.INT_CLR, written by the ROM's uartAttach
        assert_eq!(b.periph.uart0.take_tx(), b"U");
        assert_eq!(b.periph.usb_serial_jtag.take_tx(), b"J");
        assert_eq!(b.diag.drain(), []);
    }
}
