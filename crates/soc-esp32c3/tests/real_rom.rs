//! Tests against the real mask ROMs. They need the ELF files of the pinned esp-rom-elfs archive:
//! `INKEMU_ROM_ELF_DIR=<dir> cargo test -p soc-esp32c3 --test real_rom -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

mod common;

use emu_core::Cycles;
use sha2::{Digest, Sha256};
use soc_esp32c3::{
    ChipRevision, ConsoleChannel, Diagnostic, Esp32C3, RomImage, SocConfig, StopReason,
};

fn elf(revision: u16) -> Vec<u8> {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR")
        .expect("set INKEMU_ROM_ELF_DIR to the directory holding the ROM ELF files");
    let path = std::path::Path::new(&dir).join(format!("esp32c3_rev{revision}_rom.elf"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn image_sha256(revision: u16) -> String {
    let rom = RomImage::from_elf(&elf(revision)).unwrap();
    common::hex(&Sha256::digest(rom.bytes()))
}

/// The chip a ROM revision really ships in: the eFuses must report a revision that ROM belongs to.
fn chip_of(rom_revision: u16) -> ChipRevision {
    match rom_revision {
        0 => ChipRevision::new(0, 0),
        3 => ChipRevision::new(0, 3),
        101 => ChipRevision::new(1, 1),
        other => panic!("no chip revision for ROM revision {other}"),
    }
    .unwrap()
}

fn config_for(rom_revision: u16) -> SocConfig {
    SocConfig {
        chip_revision: chip_of(rom_revision),
        ..SocConfig::default()
    }
}

/// rev3 is byte-identical to the reference ROM image shipped with Espressif's QEMU; rev0 and rev101
/// have no reference image and are pinned to what the same recipe produces
/// (`docs/research/esp32c3-rom-elfs.md`, §9).
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn converted_images_match_their_pinned_hashes() {
    assert_eq!(
        image_sha256(3),
        "0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99"
    );
    assert_eq!(
        image_sha256(0),
        "832b9a17b4bca5fe307d2d84b3fcd9cdc75f6a8a51956a3e114b4d6713710565"
    );
    assert_eq!(
        image_sha256(101),
        "8b41b9b114e110e373a58188e6e86b044719f3b4e6da0441c6e8b68b72986651"
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn eco_versions_identify_the_three_revisions() {
    let eco = |rev| RomImage::from_elf(&elf(rev)).unwrap().eco_version();
    assert_eq!((eco(0), eco(3), eco(101)), (0, 3, 7));
}

/// Far more than the ROM needs to reach its banner; a bound, not an expectation.
const BANNER_BUDGET: u64 = 20_000_000;
const SLICE: u64 = 100_000;
const BOOT_LINE: &str = "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n";

struct Boot {
    uart0: String,
    usb: String,
    diagnostics: Vec<Diagnostic>,
    stop: StopReason,
    trace_hash: Option<u64>,
    cycles: u64,
}

/// Runs the ROM of `revision` until its boot line is complete on UART0, or the budget runs out.
/// Carriage returns are dropped: the ROM expands `\n` to `\r\n` on the UART.
fn boot(revision: u16, config: SocConfig) -> Boot {
    let mut machine = Esp32C3::new(RomImage::from_elf(&elf(revision)).unwrap(), &config);
    let (mut uart0, mut usb, mut diagnostics) = (Vec::new(), Vec::new(), Vec::new());
    let mut stop = StopReason::CycleBudget;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
    while stop == StopReason::CycleBudget
        && machine.cycles().get() < BANNER_BUDGET
        && !text(&uart0).contains(BOOT_LINE)
    {
        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
        usb.extend(machine.take_console(ConsoleChannel::UsbSerialJtag));
        diagnostics.extend(machine.drain_diagnostics());
    }
    Boot {
        uart0: text(&uart0),
        usb: text(&usb),
        diagnostics,
        stop,
        trace_hash: machine.trace_hash(),
        cycles: machine.cycles().get(),
    }
}

fn banner(rom: &str, build: &str) -> String {
    format!("ESP-ROM:{rom}\nBuild:{build}\n{BOOT_LINE}")
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev3_rom_prints_its_banner_on_uart0() {
    let boot = boot(3, SocConfig::default());
    assert!(
        boot.uart0
            .starts_with(&banner("esp32c3-api1-20210207", "Feb  7 2021")),
        "UART0 printed: {:?}\ndiagnostics: {:#?}",
        boot.uart0,
        boot.diagnostics
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev3_rom_prints_the_same_banner_on_usb_serial_jtag() {
    let boot = boot(3, SocConfig::default());
    assert!(
        boot.usb
            .starts_with(&banner("esp32c3-api1-20210207", "Feb  7 2021")),
        "USB printed: {:?}",
        boot.usb
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev0_and_rev101_roms_print_their_own_banners() {
    assert!(
        boot(0, config_for(0))
            .uart0
            .starts_with(&banner("esp32c3-20200918", "Sep 18 2020"))
    );
    assert!(
        boot(101, config_for(101))
            .uart0
            .starts_with(&banner("esp32c3-eco7-20230720", "Jul 20 2023"))
    );
}

/// Everything the ROM touches up to its banner is modelled: in strict mode the banner is complete
/// before the machine stops, and no CPU exception occurred on the way. Each ROM boots on the chip
/// revision it ships in.
fn assert_strict_boot_reaches_the_banner(rom_revision: u16, banner_text: &str) {
    let boot = boot(
        rom_revision,
        SocConfig {
            strict_mmio: true,
            ..config_for(rom_revision)
        },
    );
    assert!(
        boot.uart0.starts_with(banner_text),
        "rev{rom_revision} stopped early: {:?}\nUART0: {:?}",
        boot.stop,
        boot.uart0
    );
    assert!(
        !boot
            .diagnostics
            .iter()
            .any(|d| matches!(d, Diagnostic::Exception { .. })),
        "rev{rom_revision}: {:#?}",
        boot.diagnostics
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev3_banner_is_reached_without_any_unmodeled_access_or_exception() {
    assert_strict_boot_reaches_the_banner(3, &banner("esp32c3-api1-20210207", "Feb  7 2021"));
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev0_banner_is_reached_without_any_unmodeled_access_or_exception() {
    assert_strict_boot_reaches_the_banner(0, &banner("esp32c3-20200918", "Sep 18 2020"));
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev101_banner_is_reached_without_any_unmodeled_access_or_exception() {
    assert_strict_boot_reaches_the_banner(101, &banner("esp32c3-eco7-20230720", "Jul 20 2023"));
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn two_boots_are_identical_down_to_the_trace_hash() {
    let config = SocConfig {
        trace_hash: true,
        ..SocConfig::default()
    };
    let (a, b) = (boot(3, config), boot(3, config));
    assert!(a.trace_hash.is_some());
    assert_eq!(
        (a.trace_hash, a.cycles, &a.uart0, &a.diagnostics),
        (b.trace_hash, b.cycles, &b.uart0, &b.diagnostics)
    );
}
