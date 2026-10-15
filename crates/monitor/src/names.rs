//! The names a user reads: the peripheral blocks of a part of the peripheral window, and the
//! interrupt sources, both from the SVD of the chip.
use soc_esp32c3::usage::{BLOCK_BYTES, SOURCES};

/// The first interrupt map register of `INTERRUPT_CORE0`: source `n` is routed by the register
/// 4 × `n` bytes after it.
const FIRST_MAP_REGISTER: u32 = svd_regs::esp32c3::addr("INTERRUPT_CORE0", "MAC_INTR_MAP");

/// The peripherals of the SVD that have a register in the [`BLOCK_BYTES`] bytes from `base`, in
/// address order: none for a part of the window the SVD documents nothing in, two for the one
/// where `RNG` is a register inside `APB_CTRL`'s block.
#[must_use]
pub fn block_peripherals(base: u32) -> Vec<&'static str> {
    let registers = svd_regs::esp32c3::registers();
    let first = registers.partition_point(|register| register.addr < base);
    let end = base.saturating_add(BLOCK_BYTES);
    let mut names: Vec<&'static str> = Vec::new();
    for register in registers
        .get(first..)
        .unwrap_or_default()
        .iter()
        .take_while(|register| register.addr < end)
    {
        if !names.contains(&register.peripheral) {
            names.push(register.peripheral);
        }
    }
    names
}

/// The name of interrupt source `source`: the name of its map register in `INTERRUPT_CORE0`
/// without `_MAP` (`UART_INTR` for source 21, `SYSTIMER_TARGET0_INT` for 37); `None` past the
/// last source.
#[must_use]
pub fn source_name(source: u32) -> Option<&'static str> {
    if usize::try_from(source).ok()? >= SOURCES {
        return None;
    }
    let register = svd_regs::esp32c3::lookup(FIRST_MAP_REGISTER + 4 * source)?;
    if register.peripheral != "INTERRUPT_CORE0" {
        return None;
    }
    Some(register.name.strip_suffix("_MAP").unwrap_or(register.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_named_by_the_peripherals_with_a_register_in_it() {
        assert_eq!(block_peripherals(0x6000_0000), ["UART0"]);
        assert_eq!(block_peripherals(0x6002_3000), ["SYSTIMER"]);
        assert_eq!(block_peripherals(0x6002_6000), ["APB_CTRL", "RNG"]);
        // The two halves of a 4 KiB page that holds two blocks.
        assert_eq!(block_peripherals(0x6000_8000), ["RTC_CNTL"]);
        assert_eq!(block_peripherals(0x6000_8800), ["EFUSE"]);
        assert_eq!(block_peripherals(0x6000_4000), ["GPIO"]);
        assert_eq!(block_peripherals(0x6000_4800), ["GPIO_SD"]);
        assert_eq!(block_peripherals(0x6000_1000), Vec::<&str>::new());
        assert_eq!(block_peripherals(0xFFFF_F800), Vec::<&str>::new());
    }

    #[test]
    fn a_source_is_named_by_its_map_register() {
        assert_eq!(source_name(0), Some("MAC_INTR"));
        assert_eq!(source_name(21), Some("UART_INTR"));
        assert_eq!(source_name(37), Some("SYSTIMER_TARGET0_INT"));
        assert_eq!(source_name(61), Some("CACHE_CORE0_ACS_INT"));
        assert_eq!(source_name(62), None);
    }
}
