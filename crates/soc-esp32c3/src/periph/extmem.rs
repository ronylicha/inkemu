//! `EXTMEM`: the flash-cache controller. Only `ICACHE_CTRL1` is modelled so far, as plain storage
//! (report §C.1 and §J row 2); the cache itself (autoload, MMU, sync) arrives in SP1b-2. Every other
//! EXTMEM register returns `None`/`false`, so the bus reports it by name.
use super::Storage;
use emu_core::Peripheral;

pub(crate) const BASE: u32 = 0x600C_4000;
/// `ICACHE_CTRL1`: bit 0 `SHUT_IBUS`, bit 1 `SHUT_DBUS`. `Cache_Mask_All` sets both with a
/// read-modify-write, the first peripheral access of `boot_prepare` (report §C.1 and §J row 2,
/// `ROM3:Cache_Mask_All@0x40051138`). Plain read/write register; its SVD reset value is 3.
const ICACHE_CTRL1: u32 = 0x04;

#[derive(Clone, Debug)]
pub struct Extmem {
    registers: Storage,
}

impl Extmem {
    #[must_use]
    pub fn new() -> Extmem {
        Extmem {
            registers: Storage::for_offsets(BASE, &[ICACHE_CTRL1]),
        }
    }
}

impl Default for Extmem {
    fn default() -> Extmem {
        Extmem::new()
    }
}

impl Peripheral for Extmem {
    fn read(&mut self, offset: u32) -> Option<u32> {
        self.registers.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        self.registers.write(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icache_ctrl1_starts_at_its_documented_reset_value_and_remembers_writes() {
        let mut extmem = Extmem::new();
        let reset = svd_regs::esp32c3::lookup(BASE + ICACHE_CTRL1)
            .unwrap()
            .reset_value;
        assert_eq!(extmem.read(ICACHE_CTRL1), Some(reset));
        assert!(extmem.write(ICACHE_CTRL1, reset & !2));
        assert_eq!(
            extmem.read(ICACHE_CTRL1),
            Some(reset & !2),
            "Cache_UnMask_Dram0"
        );
    }

    #[test]
    fn every_other_cache_register_is_left_to_the_unmodeled_path() {
        let mut extmem = Extmem::new();
        for offset in [0x00, 0x28, 0x34, 0x40] {
            assert_eq!(extmem.read(offset), None, "{offset:#x}");
            assert!(!extmem.write(offset, 1), "{offset:#x}");
        }
    }
}
