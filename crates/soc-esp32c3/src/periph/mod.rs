//! The modelled peripherals of the ESP32-C3 and their place in the address space.
use crate::config::SocConfig;
use crate::diag::Unsupported;
use crate::flash::FlashDevice;
use emu_core::Peripheral;

pub mod assist_debug;
pub mod efuse;
pub mod extmem;
pub mod gpio;
pub mod intmatrix;
pub mod io_mux;
pub mod mmu;
pub mod rtc_cntl;
pub mod spi_mem;
pub mod system;
pub mod uart;
pub mod usb_serial_jtag;

pub use assist_debug::AssistDebug;
pub use efuse::Efuse;
pub use extmem::Extmem;
pub use gpio::Gpio;
pub use intmatrix::IntMatrix;
pub use io_mux::IoMux;
pub use mmu::MmuTable;
pub use rtc_cntl::RtcCntl;
pub use spi_mem::{Spi0, Spi1};
pub use system::System;
pub use uart::Uart;
pub use usb_serial_jtag::UsbSerialJtag;

/// A register whose value the emulator stores although the hardware effect is not modelled
/// (decision D12). The lists of these are what the storage of each model is built from, and what
/// `docs/registers-accepted-without-effect.md` is rendered from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub peripheral: &'static str,
    /// The register, or the first register of a block of `count` consecutive ones.
    pub register: &'static str,
    pub addr: u32,
    pub count: u32,
    pub not_modeled: &'static str,
    /// The plan that gives the register its effect.
    pub heir: &'static str,
}

impl Accepted {
    /// A register of the SVD; an unknown name fails the build when the list is a `const`.
    #[must_use]
    pub const fn svd(
        peripheral: &'static str,
        register: &'static str,
        not_modeled: &'static str,
        heir: &'static str,
    ) -> Accepted {
        Accepted::at(
            peripheral,
            register,
            svd_regs::esp32c3::addr(peripheral, register),
            not_modeled,
            heir,
        )
    }

    /// A register the SVD does not contain, at an address documented where the constant is defined.
    #[must_use]
    pub const fn at(
        peripheral: &'static str,
        register: &'static str,
        addr: u32,
        not_modeled: &'static str,
        heir: &'static str,
    ) -> Accepted {
        Accepted {
            peripheral,
            register,
            addr,
            count: 1,
            not_modeled,
            heir,
        }
    }

    /// The same row standing for `count` consecutive registers.
    #[must_use]
    pub const fn block(self, count: u32) -> Accepted {
        Accepted { count, ..self }
    }
}

/// Registers deliberately modelled as plain read/write memory: configuration the emulator
/// acknowledges but does not act on. Listing them here is a decision; anything else is reported.
#[derive(Clone, Debug)]
pub struct Storage {
    /// `(offset, value)`, sorted by offset.
    regs: Vec<(u32, u32)>,
}

impl Storage {
    /// Every register the SVD documents for `peripheral`, at its reset value.
    #[must_use]
    pub fn for_peripheral(peripheral: &str, base: u32) -> Storage {
        let regs = svd_regs::esp32c3::registers()
            .iter()
            .filter(|r| r.peripheral == peripheral)
            .map(|r| (r.addr - base, r.reset_value))
            .collect();
        Storage { regs }
    }

    /// Only the registers at `offsets`, at their SVD reset value (0 when the SVD has none).
    #[must_use]
    pub fn for_offsets(base: u32, offsets: &[u32]) -> Storage {
        let mut regs: Vec<(u32, u32)> = offsets
            .iter()
            .map(|&offset| {
                (
                    offset,
                    svd_regs::esp32c3::lookup(base + offset).map_or(0, |r| r.reset_value),
                )
            })
            .collect();
        regs.sort_unstable();
        regs.dedup();
        Storage { regs }
    }

    /// The single registers of `list` (rows with `count == 1`), at their SVD reset value, 0 without one.
    #[must_use]
    pub fn accepted(base: u32, list: &[Accepted]) -> Storage {
        let offsets: Vec<u32> = list
            .iter()
            .filter(|a| a.count == 1)
            .map(|a| a.addr - base)
            .collect();
        Storage::for_offsets(base, &offsets)
    }

    fn index(&self, offset: u32) -> Option<usize> {
        self.regs.binary_search_by_key(&offset, |&(o, _)| o).ok()
    }

    #[must_use]
    pub fn read(&self, offset: u32) -> Option<u32> {
        self.index(offset).map(|i| self.regs[i].1)
    }

    pub fn write(&mut self, offset: u32, value: u32) -> bool {
        match self.index(offset) {
            Some(i) => {
                self.regs[i].1 = value;
                true
            }
            None => false,
        }
    }
}

/// A block made only of registers accepted without effect.
#[derive(Clone, Debug)]
pub struct AcceptedBlock {
    registers: Storage,
}

impl AcceptedBlock {
    #[must_use]
    pub fn new(base: u32, list: &[Accepted]) -> AcceptedBlock {
        AcceptedBlock {
            registers: Storage::accepted(base, list),
        }
    }
}

impl Peripheral for AcceptedBlock {
    fn read(&mut self, offset: u32) -> Option<u32> {
        self.registers.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        self.registers.write(offset, value)
    }
}

/// Every register accepted without effect, in address order.
#[must_use]
pub fn accepted_without_effect() -> Vec<Accepted> {
    let mut rows = [
        io_mux::accepted(),
        rtc_cntl::accepted(),
        gpio::accepted(),
        SENSITIVE_ACCEPTED.to_vec(),
        system::accepted(),
        uart::accepted(),
        intmatrix::accepted(),
        assist_debug::accepted(),
        extmem::accepted(),
        spi_mem::accepted(),
    ]
    .concat();
    rows.sort_by_key(|row| row.addr);
    rows
}

/// The Markdown table of `docs/registers-accepted-without-effect.md`.
#[must_use]
pub fn render_accepted_table(rows: &[Accepted]) -> String {
    use std::fmt::Write as _;
    let mut table = String::from(
        "| Address | Register | Count | Hardware effect not modelled | Made real by |\n|---|---|---|---|---|\n",
    );
    for row in rows {
        // Writing to a String cannot fail.
        let _ = writeln!(
            table,
            "| `{:#010x}` | `{}.{}` | {} | {} | {} |",
            row.addr, row.peripheral, row.register, row.count, row.not_modeled, row.heir
        );
    }
    table
}

// Base address and range of every modelled block (`reg_base.h`; report §A.2), in one place. A range
// is `(base, end)`, the end being exclusive. The peripheral modules import their base from here, and
// ask the SVD for their registers relative to it.
// The two MSPI controllers (`reg_base.h` `DR_REG_SPI1_BASE`, `DR_REG_SPI0_BASE`; report BC §A.2).
pub(crate) const SPI1_BASE: u32 = 0x6000_2000;
const SPI1: (u32, u32) = (SPI1_BASE, 0x6000_3000);
pub(crate) const SPI0_BASE: u32 = 0x6000_3000;
const SPI0: (u32, u32) = (SPI0_BASE, 0x6000_4000);
pub(crate) const GPIO_BASE: u32 = 0x6000_4000;
const GPIO: (u32, u32) = (GPIO_BASE, 0x6000_4F00);
pub(crate) const IO_MUX_BASE: u32 = 0x6000_9000;
const IO_MUX: (u32, u32) = (IO_MUX_BASE, 0x6000_A000);
pub(crate) const RTC_CNTL_BASE: u32 = 0x6000_8000;
/// `RTC_CNTL` and the eFuse mirror share the page `0x6000_8000`: the first 2 KiB, then the last.
const RTC_CNTL: (u32, u32) = (RTC_CNTL_BASE, EFUSE_BASE);
pub(crate) const EFUSE_BASE: u32 = 0x6000_8800;
const EFUSE: (u32, u32) = (EFUSE_BASE, 0x6000_9000);
pub(crate) const SYSTEM_BASE: u32 = 0x600C_0000;
const SYSTEM: (u32, u32) = (SYSTEM_BASE, 0x600C_1000);
pub(crate) const SENSITIVE_BASE: u32 = 0x600C_1000;
const SENSITIVE: (u32, u32) = (SENSITIVE_BASE, 0x600C_2000);
/// `Cache_Occupy_ICache_MEMORY` gives SRAM0 to the cache by clearing bit 0 (CM §2.1). SRAM0 stays
/// plain RAM here: nothing uses it once the cache is on (CM §9 item 9).
const SENSITIVE_ACCEPTED: [Accepted; 1] = [Accepted::svd(
    "SENSITIVE",
    "INTERNAL_SRAM_USAGE_1",
    "SRAM0 handed to the cache as line storage",
    "SP1b-4",
)];
pub(crate) const INTMATRIX_BASE: u32 = 0x600C_2000;
const INTMATRIX: (u32, u32) = (INTMATRIX_BASE, 0x600C_3000);
pub(crate) const EXTMEM_BASE: u32 = 0x600C_4000;
const EXTMEM: (u32, u32) = (EXTMEM_BASE, mmu::MMU_TABLE_BASE);
const MMU_TABLE: (u32, u32) = (mmu::MMU_TABLE_BASE, mmu::MMU_TABLE_BASE + 0x200);
pub(crate) const ASSIST_DEBUG_BASE: u32 = 0x600C_E000;
const ASSIST_DEBUG: (u32, u32) = (ASSIST_DEBUG_BASE, 0x600C_F000);
// The two UARTs and the USB-Serial-JTAG device: the ROM's consoles (report §G).
const UART0_BASE: u32 = 0x6000_0000;
const UART0: (u32, u32) = (UART0_BASE, 0x6000_1000);
const UART1_BASE: u32 = 0x6001_0000;
const UART1: (u32, u32) = (UART1_BASE, 0x6001_1000);
const USB_SERIAL_JTAG_BASE: u32 = 0x6004_3000;
const USB_SERIAL_JTAG: (u32, u32) = (USB_SERIAL_JTAG_BASE, 0x6004_4000);

#[derive(Debug)]
pub struct Peripherals {
    pub gpio: Gpio,
    pub io_mux: IoMux,
    pub intmatrix: IntMatrix,
    pub rtc_cntl: RtcCntl,
    pub system: System,
    pub efuse: Efuse,
    pub extmem: Extmem,
    pub mmu: MmuTable,
    pub sensitive: AcceptedBlock,
    pub assist_debug: AssistDebug,
    pub spi0: Spi0,
    pub spi1: Spi1,
    pub uart0: Uart,
    pub uart1: Uart,
    pub usb_serial_jtag: UsbSerialJtag,
}

impl Peripherals {
    #[must_use]
    pub fn new(config: &SocConfig, flash: Box<dyn FlashDevice>) -> Peripherals {
        Peripherals {
            gpio: Gpio::new(config.strapping),
            io_mux: IoMux::new(),
            intmatrix: IntMatrix::new(),
            rtc_cntl: RtcCntl::new(),
            system: System::new(),
            efuse: Efuse::new(config.chip_revision),
            extmem: Extmem::new(),
            mmu: MmuTable::new(),
            sensitive: AcceptedBlock::new(SENSITIVE_BASE, &SENSITIVE_ACCEPTED),
            assist_debug: AssistDebug::new(),
            spi0: Spi0::new(),
            spi1: Spi1::new(flash),
            uart0: Uart::new(UART0_BASE),
            uart1: Uart::new(UART1_BASE),
            usb_serial_jtag: UsbSerialJtag::new(),
        }
    }

    /// The device that owns the register at `addr`, with its base address.
    pub(crate) fn device(&mut self, addr: u32) -> Option<(&mut dyn Peripheral, u32)> {
        let within = |block: (u32, u32)| (block.0..block.1).contains(&addr);
        if within(GPIO) {
            Some((&mut self.gpio, GPIO_BASE))
        } else if within(IO_MUX) {
            Some((&mut self.io_mux, IO_MUX_BASE))
        } else if within(RTC_CNTL) {
            Some((&mut self.rtc_cntl, RTC_CNTL_BASE))
        } else if within(EFUSE) {
            Some((&mut self.efuse, EFUSE_BASE))
        } else if within(SYSTEM) {
            Some((&mut self.system, SYSTEM_BASE))
        } else if within(SENSITIVE) {
            Some((&mut self.sensitive, SENSITIVE_BASE))
        } else if within(INTMATRIX) {
            Some((&mut self.intmatrix, INTMATRIX_BASE))
        } else if within(EXTMEM) {
            Some((&mut self.extmem, EXTMEM_BASE))
        } else if within(MMU_TABLE) {
            Some((&mut self.mmu, mmu::MMU_TABLE_BASE))
        } else if within(ASSIST_DEBUG) {
            Some((&mut self.assist_debug, ASSIST_DEBUG_BASE))
        } else if within(SPI1) {
            Some((&mut self.spi1, SPI1_BASE))
        } else if within(SPI0) {
            Some((&mut self.spi0, SPI0_BASE))
        } else if within(UART0) {
            Some((&mut self.uart0, UART0_BASE))
        } else if within(UART1) {
            Some((&mut self.uart1, UART1_BASE))
        } else if within(USB_SERIAL_JTAG) {
            Some((&mut self.usb_serial_jtag, USB_SERIAL_JTAG_BASE))
        } else {
            None
        }
    }

    /// The oldest thing a device was asked to do and cannot do faithfully, not taken yet: call until
    /// `None` to get all of them, in order.
    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.spi1.take_unsupported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_starts_at_the_svd_reset_values_and_only_answers_its_own_registers() {
        let mut s = Storage::for_peripheral("IO_MUX", 0x6000_9000);
        let gpio20 = svd_regs::esp32c3::lookup(0x6000_9054).unwrap();
        assert_eq!(s.read(0x54), Some(gpio20.reset_value));
        assert!(s.write(0x54, 5));
        assert_eq!(s.read(0x54), Some(5));
        assert_eq!(s.read(0x60), None, "no IO_MUX register at +0x60");
        assert!(!s.write(0x60, 1));
        let mut picked = Storage::for_offsets(0x6000_8000, &[0x8C, 0x84, 0x84]);
        assert_eq!(
            picked.read(0x84),
            Some(svd_regs::esp32c3::lookup(0x6000_8084).unwrap().reset_value)
        );
        assert_eq!(picked.read(0x88), None);
        assert!(picked.write(0x8C, 9));
    }

    #[test]
    fn accepted_storage_takes_its_registers_and_reset_values_from_the_list() {
        // `SPI1.CMD` is the first SPI1 register of the SVD: its address is the base of the block.
        const SPI1: u32 = svd_regs::esp32c3::addr("SPI1", "CMD");
        const CTRL: u32 = svd_regs::esp32c3::addr("SPI1", "CTRL") - SPI1;
        const CTRL1: u32 = svd_regs::esp32c3::addr("SPI1", "CTRL1") - SPI1;
        // `DR_REG_SPI0_BASE` (BC §A.2) and SPI0 `MISO_DLEN`, which the SVD omits (FB §B.2).
        const SPI0: u32 = 0x6000_3000;
        const MISO_DLEN: u32 = 0x28;
        const LIST: [Accepted; 2] = [
            Accepted::svd("SPI1", "CTRL", "read mode", "SP8"),
            Accepted::at(
                "SPI0",
                "MISO_DLEN",
                SPI0 + MISO_DLEN,
                "cache line length",
                "SP8",
            ),
        ];
        let mut s = Storage::accepted(SPI1, &LIST[..1]);
        assert_eq!(
            s.read(CTRL),
            Some(0x002C_A000),
            "SVD reset value of SPI1.CTRL (FB §D.2)"
        );
        assert_eq!(s.read(CTRL1), None, "not in the list");
        let mut other = Storage::accepted(SPI0, &LIST[1..]);
        assert_eq!(other.read(MISO_DLEN), Some(0), "no SVD entry: reset 0");
        assert!(other.write(MISO_DLEN, 0xFF));
        assert!(s.write(CTRL, 1));
    }

    #[test]
    fn an_accepted_block_is_storage_and_nothing_else() {
        // `SENSITIVE.ROM_TABLE_LOCK` is the first SENSITIVE register of the SVD: the base of the block.
        const BASE: u32 = svd_regs::esp32c3::addr("SENSITIVE", "ROM_TABLE_LOCK");
        const USAGE_1: u32 = svd_regs::esp32c3::addr("SENSITIVE", "INTERNAL_SRAM_USAGE_1") - BASE;
        const LIST: [Accepted; 1] = [Accepted::svd(
            "SENSITIVE",
            "INTERNAL_SRAM_USAGE_1",
            "SRAM0 given to the cache",
            "SP8",
        )];
        let mut block = AcceptedBlock::new(BASE, &LIST);
        assert_eq!(block.read(USAGE_1), Some(0xF), "CM §2.1");
        assert!(block.write(USAGE_1, 0xE));
        assert_eq!(block.read(USAGE_1), Some(0xE));
        assert_eq!(
            block.read(USAGE_1 + 4),
            None,
            "the next register is not in the list"
        );
        assert!(!block.write(USAGE_1 + 4, 0));
    }

    #[test]
    fn the_table_lists_every_register_once_in_address_order() {
        let rows = accepted_without_effect();
        assert!(
            rows.windows(2).all(|w| w[0].addr < w[1].addr),
            "sorted, no duplicate address"
        );
        assert!(
            rows.iter()
                .all(|r| r.count >= 1 && !r.not_modeled.is_empty() && r.heir.starts_with("SP"))
        );
        let io_mux = rows.iter().find(|r| r.peripheral == "IO_MUX").unwrap();
        assert_eq!(io_mux.count, 24, "every IO_MUX register of the SVD");
        let table = render_accepted_table(&rows);
        assert!(table.starts_with(
            "| Address | Register | Count | Hardware effect not modelled | Made real by |\n|---|---|---|---|---|\n"
        ));
        assert_eq!(table.lines().count(), rows.len() + 2);
    }

    #[test]
    fn the_mmu_table_sits_right_after_the_cache_registers() {
        let mut p = Peripherals::new(
            &SocConfig::default(),
            Box::new(crate::flash::NorFlash::blank(
                crate::flash::FlashConfig::default(),
            )),
        );
        let base_of = |p: &mut Peripherals, addr: u32| p.device(addr).map(|(_, base)| base);
        assert_eq!(base_of(&mut p, 0x600C_4FFC), Some(EXTMEM_BASE));
        assert_eq!(base_of(&mut p, 0x600C_5000), Some(mmu::MMU_TABLE_BASE));
        assert_eq!(base_of(&mut p, 0x600C_51FC), Some(mmu::MMU_TABLE_BASE));
        assert_eq!(
            base_of(&mut p, 0x600C_5200),
            None,
            "nothing is documented past the 128 entries"
        );
        assert_eq!(
            base_of(
                &mut p,
                svd_regs::esp32c3::addr("SENSITIVE", "INTERNAL_SRAM_USAGE_1")
            ),
            Some(SENSITIVE_BASE)
        );
    }

    #[test]
    fn the_page_shared_by_rtc_cntl_and_the_efuse_splits_at_the_last_two_kib() {
        let mut p = Peripherals::new(
            &SocConfig::default(),
            Box::new(crate::flash::NorFlash::blank(
                crate::flash::FlashConfig::default(),
            )),
        );
        let base_of = |p: &mut Peripherals, addr: u32| p.device(addr).map(|(_, base)| base);
        assert_eq!(base_of(&mut p, 0x6000_8000), Some(RTC_CNTL_BASE));
        assert_eq!(base_of(&mut p, 0x6000_87FC), Some(RTC_CNTL_BASE));
        assert_eq!(base_of(&mut p, 0x6000_8800), Some(EFUSE_BASE));
        assert_eq!(base_of(&mut p, 0x6000_8FFC), Some(EFUSE_BASE));
        assert_eq!(base_of(&mut p, 0x6000_9000), Some(IO_MUX_BASE));
    }
}
