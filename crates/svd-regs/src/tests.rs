extern crate std;

use crate::REGISTER_BYTES;
use crate::esp32c3::{lookup, registers};

/// Update these two numbers only after reading the generator diff of an SVD bump.
const REGISTER_COUNT: usize = 2605;
const UART_REGISTER_COUNT: usize = 33;

#[test]
fn table_is_sorted_unique_and_word_aligned() {
    assert_eq!(registers().len(), REGISTER_COUNT);
    for pair in registers().windows(2) {
        assert!(
            pair[0].addr + REGISTER_BYTES <= pair[1].addr,
            "{:#x} overlaps {:#x}",
            pair[0].addr,
            pair[1].addr
        );
    }
    assert!(registers().iter().all(|r| r.addr % REGISTER_BYTES == 0));
}

#[test]
fn every_byte_of_a_register_resolves_to_it_and_gaps_resolve_to_nothing() {
    for (i, r) in registers().iter().enumerate() {
        for byte in 0..REGISTER_BYTES {
            assert_eq!(lookup(r.addr + byte), Some(r));
        }
        let next = registers().get(i + 1).map_or(u32::MAX, |n| n.addr);
        if r.addr + REGISTER_BYTES < next {
            assert_eq!(
                lookup(r.addr + REGISTER_BYTES),
                None,
                "gap after {:#x}",
                r.addr
            );
        }
    }
    assert_eq!(lookup(0), None);
    assert_eq!(lookup(0x5fff_ffff), None);
}

fn expect(addr: u32, peripheral: &str, name: &str, reset: u32, explicit: bool) {
    let r = lookup(addr).unwrap_or_else(|| panic!("no register at {addr:#x}"));
    assert_eq!(
        (r.peripheral, r.name, r.reset_value, r.reset_explicit),
        (peripheral, name, reset, explicit),
        "{addr:#x}"
    );
}

/// Values cross-checked against the ESP-IDF v5.5 register headers (`docs/research/esp32c3-svd.md`, §4.3).
#[test]
fn known_registers_have_their_documented_reset_values() {
    expect(0x6000_001C, "UART0", "STATUS", 0xE000_C000, true);
    expect(0x6001_001C, "UART1", "STATUS", 0xE000_C000, true);
    expect(0x6002_3000, "SYSTIMER", "CONF", 0x4600_0000, true);
    expect(0x6000_8038, "RTC_CNTL", "RESET_STATE", 0x0000_3000, true);
    expect(0x600C_0008, "SYSTEM", "CPU_PER_CONF", 0x0000_000C, true);
}

#[test]
fn hardware_dependent_inputs_are_flagged_as_undocumented_zero() {
    expect(0x6000_4038, "GPIO", "STRAP", 0, false);
}

#[test]
fn the_single_address_collision_resolves_to_the_rng_register() {
    expect(0x6002_60B0, "RNG", "DATA", 0, false);
}

#[test]
fn derived_peripherals_have_the_same_layout_as_their_parent() {
    let offsets = |p: &str, base: u32| -> std::vec::Vec<(u32, &str)> {
        registers()
            .iter()
            .filter(|r| r.peripheral == p)
            .map(|r| (r.addr - base, r.name))
            .collect()
    };
    let uart0 = offsets("UART0", 0x6000_0000);
    assert_eq!(uart0.len(), UART_REGISTER_COUNT);
    assert_eq!(uart0, offsets("UART1", 0x6001_0000));
    assert_eq!(offsets("TIMG0", 0x6001_F000), offsets("TIMG1", 0x6002_0000));
}

#[test]
fn blocks_absent_from_the_svd_are_absent_from_the_table() {
    // The flash MMU table has no SVD description: `soc-esp32c3` models it by hand.
    assert_eq!(lookup(0x600C_5000), None);
}

#[test]
fn addr_resolves_names_to_the_addresses_the_reports_measured() {
    use crate::esp32c3::addr;
    assert_eq!(addr("UART0", "FIFO"), 0x6000_0000);
    assert_eq!(addr("EXTMEM", "ICACHE_SYNC_CTRL"), 0x600C_4028); // CM §2
    assert_eq!(addr("SHA", "M_MEM0"), 0x6003_B080); // FB §B.7
    assert_eq!(addr("SPI1", "W0"), 0x6000_2058); // FB §B.6
    assert_eq!(addr("RNG", "DATA"), 0x6002_60B0); // BA §2 row 88
    assert_eq!(addr("TIMG0", "RTCCALICFG"), 0x6001_F068); // BA §9
}

#[test]
fn addr_is_usable_in_a_constant() {
    const STRAP_OFFSET: u32 =
        crate::esp32c3::addr("GPIO", "STRAP") - crate::esp32c3::addr("GPIO", "BT_SELECT");
    assert_eq!(STRAP_OFFSET, 0x38);
}

#[test]
fn find_returns_the_whole_entry_or_none() {
    let reg = crate::esp32c3::find("SPI1", "CTRL").unwrap();
    assert_eq!(
        (reg.addr, reg.reset_value, reg.reset_explicit),
        (0x6000_2008, 0x002C_A000, true)
    );
    assert!(crate::esp32c3::find("SPI1", "NOPE").is_none());
    assert!(crate::esp32c3::find("NOPE", "CTRL").is_none());
}

#[test]
#[should_panic(expected = "no such register in the ESP32-C3 SVD")]
fn addr_of_an_unknown_register_panics_when_evaluated_at_run_time() {
    let name = std::hint::black_box("NOPE");
    let _ = crate::esp32c3::addr("SPI1", name);
}
