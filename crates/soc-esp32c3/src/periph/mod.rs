//! The modelled peripherals of the ESP32-C3 and their place in the address space.
use crate::config::SocConfig;
use emu_core::Peripheral;

pub mod assist_debug;
pub mod efuse;
pub mod extmem;
pub mod gpio;
pub mod intmatrix;
pub mod io_mux;
pub mod rtc_cntl;
pub mod system;
pub mod uart;
pub mod usb_serial_jtag;

pub use assist_debug::AssistDebug;
pub use efuse::Efuse;
pub use extmem::Extmem;
pub use gpio::Gpio;
pub use intmatrix::IntMatrix;
pub use io_mux::IoMux;
pub use rtc_cntl::RtcCntl;
pub use system::System;
pub use uart::Uart;
pub use usb_serial_jtag::UsbSerialJtag;

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

// Base address and range of every modelled block (`reg_base.h`; report §A.2), in one place. A range
// is `(base, end)`, the end being exclusive. The peripheral modules import their base from here, and
// ask the SVD for their registers relative to it.
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
pub(crate) const INTMATRIX_BASE: u32 = 0x600C_2000;
const INTMATRIX: (u32, u32) = (INTMATRIX_BASE, 0x600C_3000);
pub(crate) const EXTMEM_BASE: u32 = 0x600C_4000;
const EXTMEM: (u32, u32) = (EXTMEM_BASE, 0x600C_5000);
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
    pub assist_debug: AssistDebug,
    pub uart0: Uart,
    pub uart1: Uart,
    pub usb_serial_jtag: UsbSerialJtag,
}

impl Peripherals {
    #[must_use]
    pub fn new(config: &SocConfig) -> Peripherals {
        Peripherals {
            gpio: Gpio::new(config.strapping),
            io_mux: IoMux::new(),
            intmatrix: IntMatrix::new(),
            rtc_cntl: RtcCntl::new(),
            system: System::new(),
            efuse: Efuse::new(config.chip_revision),
            extmem: Extmem::new(),
            assist_debug: AssistDebug::new(),
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
        } else if within(INTMATRIX) {
            Some((&mut self.intmatrix, INTMATRIX_BASE))
        } else if within(EXTMEM) {
            Some((&mut self.extmem, EXTMEM_BASE))
        } else if within(ASSIST_DEBUG) {
            Some((&mut self.assist_debug, ASSIST_DEBUG_BASE))
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
    fn the_page_shared_by_rtc_cntl_and_the_efuse_splits_at_the_last_two_kib() {
        let mut p = Peripherals::new(&SocConfig::default());
        let base_of = |p: &mut Peripherals, addr: u32| p.device(addr).map(|(_, base)| base);
        assert_eq!(base_of(&mut p, 0x6000_8000), Some(RTC_CNTL_BASE));
        assert_eq!(base_of(&mut p, 0x6000_87FC), Some(RTC_CNTL_BASE));
        assert_eq!(base_of(&mut p, 0x6000_8800), Some(EFUSE_BASE));
        assert_eq!(base_of(&mut p, 0x6000_8FFC), Some(EFUSE_BASE));
        assert_eq!(base_of(&mut p, 0x6000_9000), Some(IO_MUX_BASE));
    }
}
