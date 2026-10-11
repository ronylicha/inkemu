//! The modelled peripherals of the ESP32-C3 and their place in the address space.
use crate::config::SocConfig;
use emu_core::Peripheral;

pub mod assist_debug;
pub mod efuse;
pub mod gpio;
pub mod intmatrix;
pub mod io_mux;
pub mod rtc_cntl;
pub mod system;

pub use assist_debug::AssistDebug;
pub use efuse::Efuse;
pub use gpio::Gpio;
pub use intmatrix::IntMatrix;
pub use io_mux::IoMux;
pub use rtc_cntl::RtcCntl;
pub use system::System;

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

/// Address ranges of the modelled blocks (`reg_base.h`; report §A.2). Ends are exclusive.
const GPIO_BASE: u32 = 0x6000_4000;
const GPIO: (u32, u32) = (GPIO_BASE, 0x6000_4F00);
/// Shared with [`io_mux`], which asks the SVD for its registers relative to this base.
pub(crate) const IO_MUX_BASE: u32 = 0x6000_9000;
const IO_MUX: (u32, u32) = (IO_MUX_BASE, 0x6000_A000);
/// `RTC_CNTL` and the eFuse mirror share the page `0x6000_8000`: the first 2 KiB, then the last.
const RTC_CNTL: (u32, u32) = (rtc_cntl::BASE, EFUSE_BASE);
/// Shared with [`efuse`], which asks the SVD for its registers relative to this base.
pub(crate) const EFUSE_BASE: u32 = 0x6000_8800;
const EFUSE: (u32, u32) = (EFUSE_BASE, 0x6000_9000);
const SYSTEM: (u32, u32) = (system::BASE, 0x600C_1000);
const INTMATRIX_BASE: u32 = 0x600C_2000;
const INTMATRIX: (u32, u32) = (INTMATRIX_BASE, 0x600C_3000);
const ASSIST_DEBUG_BASE: u32 = 0x600C_E000;
const ASSIST_DEBUG: (u32, u32) = (ASSIST_DEBUG_BASE, 0x600C_F000);

#[derive(Debug)]
pub struct Peripherals {
    pub gpio: Gpio,
    pub io_mux: IoMux,
    pub intmatrix: IntMatrix,
    pub rtc_cntl: RtcCntl,
    pub system: System,
    pub efuse: Efuse,
    pub assist_debug: AssistDebug,
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
            assist_debug: AssistDebug::new(),
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
            Some((&mut self.rtc_cntl, rtc_cntl::BASE))
        } else if within(EFUSE) {
            Some((&mut self.efuse, EFUSE_BASE))
        } else if within(SYSTEM) {
            Some((&mut self.system, system::BASE))
        } else if within(INTMATRIX) {
            Some((&mut self.intmatrix, INTMATRIX_BASE))
        } else if within(ASSIST_DEBUG) {
            Some((&mut self.assist_debug, ASSIST_DEBUG_BASE))
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
}
