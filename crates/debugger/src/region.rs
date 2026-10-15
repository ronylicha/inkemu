//! The regions of the ESP32-C3's address space, as the debugger names them (memory map:
//! `soc_esp32c3::memmap`, `docs/research/esp32c3-boot-chain.md` §A.1).
use soc_esp32c3::memmap::{
    DROM_BASE, DROM_ROM_OFFSET, DROM_SIZE, FLASH_DBUS_BASE, FLASH_IBUS_BASE, FLASH_WINDOW_SIZE,
    PERIPH_BASE, PERIPH_SIZE, ROM_BASE, ROM_SIZE, RTC_RAM_BASE, RTC_RAM_SIZE, SRAM_DBUS_BASE,
    SRAM_IBUS_BASE, SRAM_SIZE, SRAM0_BASE, SRAM0_SIZE,
};

/// What lies at an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Region {
    /// The mask ROM, through the instruction bus or its data-bus alias.
    Rom,
    /// The 384 KiB SRAM, through either bus.
    Sram,
    /// SRAM0, the 16 KiB of the instruction bus only.
    Sram0,
    /// The 8 KiB of RTC memory.
    Rtc,
    /// A cache window of the flash, data bus or instruction bus.
    Flash,
    /// A byte of a flash window that no flash byte backs (an invalid MMU entry, or a page past the
    /// end of the chip): only a read of the machine says so, never [`Region::of`].
    Unbacked,
    /// The peripheral registers.
    Registers,
    /// Nothing.
    Unmapped,
}

/// Whether `address` lies in the `size` bytes from `base`.
fn within(address: u32, base: u32, size: u32) -> bool {
    address
        .checked_sub(base)
        .is_some_and(|offset| offset < size)
}

/// The flash windows: `(base, size)` of the data-bus one, then of the instruction-bus one.
pub(crate) const FLASH_WINDOWS: [(u32, u32); 2] = [
    (FLASH_DBUS_BASE, FLASH_WINDOW_SIZE),
    (FLASH_IBUS_BASE, FLASH_WINDOW_SIZE),
];

impl Region {
    /// The region `address` lies in.
    #[must_use]
    pub fn of(address: u32) -> Region {
        if within(address, ROM_BASE, ROM_SIZE) || within(address, DROM_BASE, DROM_SIZE) {
            Region::Rom
        } else if within(address, SRAM_DBUS_BASE, SRAM_SIZE)
            || within(address, SRAM_IBUS_BASE, SRAM_SIZE)
        {
            Region::Sram
        } else if within(address, SRAM0_BASE, SRAM0_SIZE) {
            Region::Sram0
        } else if within(address, RTC_RAM_BASE, RTC_RAM_SIZE) {
            Region::Rtc
        } else if FLASH_WINDOWS
            .iter()
            .any(|&(base, size)| within(address, base, size))
        {
            Region::Flash
        } else if within(address, PERIPH_BASE, PERIPH_SIZE) {
            Region::Registers
        } else {
            Region::Unmapped
        }
    }

    /// Whether the CPU can fetch an instruction here: memory, not registers nor nothing.
    #[must_use]
    pub const fn holds_code(self) -> bool {
        matches!(
            self,
            Region::Rom | Region::Sram | Region::Sram0 | Region::Rtc | Region::Flash
        )
    }

    /// The region in words, for a message.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Region::Rom => "the mask ROM",
            Region::Sram => "the SRAM",
            Region::Sram0 => "SRAM0",
            Region::Rtc => "the RTC memory",
            Region::Flash => "a flash window",
            Region::Unbacked => "a flash window no MMU entry backs",
            Region::Registers => "the peripheral registers",
            Region::Unmapped => "no memory nor device",
        }
    }
}

/// The instruction-bus address of the cell a data-bus alias names, when `address` is one: the
/// data-bus alias of the mask ROM, the data-bus view of the SRAM, or the data window of the
/// flash. The CPU fetches its instructions through the instruction bus, so its PC is never such
/// an address: a breakpoint there would never stop.
#[must_use]
pub fn instruction_bus_twin(address: u32) -> Option<u32> {
    let twin =
        |base: u32, size: u32, to: u32| within(address, base, size).then(|| to + (address - base));
    twin(DROM_BASE, DROM_SIZE, ROM_BASE + DROM_ROM_OFFSET)
        .or_else(|| twin(SRAM_DBUS_BASE, SRAM_SIZE, SRAM_IBUS_BASE))
        .or_else(|| twin(FLASH_DBUS_BASE, FLASH_WINDOW_SIZE, FLASH_IBUS_BASE))
}

/// Whether the instruction-bus window of the flash holds `address`: the code the CPU executes
/// from the flash, which the disassembly reads (4a-D6).
#[must_use]
pub fn in_flash_code(address: u32) -> bool {
    within(address, FLASH_IBUS_BASE, FLASH_WINDOW_SIZE)
}

/// Whether the `length` bytes from `address` (`length` ≥ 1, within the address space) overlap a
/// flash window.
#[must_use]
pub(crate) fn overlaps_flash(address: u32, length: u32) -> bool {
    let first = u64::from(address);
    let end = first + u64::from(length);
    FLASH_WINDOWS.iter().any(|&(base, size)| {
        let window = u64::from(base);
        first < window + u64::from(size) && window < end
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_region_of_the_memory_map_is_named() {
        assert_eq!(Region::of(0x4000_0000), Region::Rom);
        assert_eq!(Region::of(0x3FF1_0000), Region::Rom, "the data-bus alias");
        assert_eq!(Region::of(0x3FC8_0000), Region::Sram);
        assert_eq!(
            Region::of(0x403D_FFFF),
            Region::Sram,
            "the instruction-bus view"
        );
        assert_eq!(Region::of(0x4037_C000), Region::Sram0);
        assert_eq!(Region::of(0x5000_1FFF), Region::Rtc);
        assert_eq!(Region::of(0x5000_2000), Region::Unmapped);
        assert_eq!(Region::of(0x3C00_0000), Region::Flash);
        assert_eq!(Region::of(0x427F_FFFF), Region::Flash);
        assert_eq!(Region::of(0x6000_0000), Region::Registers);
        assert_eq!(Region::of(0), Region::Unmapped);
        assert_eq!(Region::of(u32::MAX), Region::Unmapped);
    }

    #[test]
    fn code_is_in_memory_and_never_in_registers() {
        assert!(Region::Flash.holds_code());
        assert!(Region::Rtc.holds_code());
        assert!(!Region::Registers.holds_code());
        assert!(!Region::Unmapped.holds_code());
    }

    #[test]
    fn each_data_bus_alias_has_its_instruction_bus_twin() {
        assert_eq!(
            instruction_bus_twin(DROM_BASE + 8),
            Some(ROM_BASE + DROM_ROM_OFFSET + 8)
        );
        assert_eq!(
            instruction_bus_twin(SRAM_DBUS_BASE + 0x40),
            Some(SRAM_IBUS_BASE + 0x40)
        );
        assert_eq!(
            instruction_bus_twin(FLASH_DBUS_BASE + 0x1_0000),
            Some(FLASH_IBUS_BASE + 0x1_0000)
        );
        assert_eq!(instruction_bus_twin(ROM_BASE), None);
        assert_eq!(instruction_bus_twin(SRAM_IBUS_BASE), None);
        assert_eq!(instruction_bus_twin(FLASH_IBUS_BASE), None);
        assert_eq!(instruction_bus_twin(RTC_RAM_BASE), None);
    }

    #[test]
    fn a_range_that_touches_a_flash_window_overlaps_it() {
        assert!(overlaps_flash(FLASH_DBUS_BASE - 1, 2));
        assert!(overlaps_flash(FLASH_IBUS_BASE + FLASH_WINDOW_SIZE - 1, 1));
        assert!(!overlaps_flash(FLASH_DBUS_BASE - 4, 4));
        assert!(!overlaps_flash(FLASH_IBUS_BASE + FLASH_WINDOW_SIZE, 4));
        assert!(in_flash_code(FLASH_IBUS_BASE));
        assert!(!in_flash_code(FLASH_DBUS_BASE));
    }
}
