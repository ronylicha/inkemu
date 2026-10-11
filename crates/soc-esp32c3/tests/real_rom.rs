//! Tests against the real mask ROMs. They need the ELF files of the pinned esp-rom-elfs archive:
//! `INKEMU_ROM_ELF_DIR=<dir> cargo test -p soc-esp32c3 --test real_rom -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

mod common;

use emu_core::Cycles;
use firmware::builder::{ImageSpec, build_image};
use sha2::{Digest, Sha256};
use soc_esp32c3::{
    ChipRevision, ConsoleChannel, Diagnostic, Esp32C3, RomImage, SocConfig, StopReason,
};

/// The ROM ELF of `revision` in the directory `INKEMU_ROM_ELF_DIR` names; a failure says why and
/// what to do.
fn elf(revision: u16) -> Vec<u8> {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").unwrap_or_else(|e| {
        panic!(
            "INKEMU_ROM_ELF_DIR is not usable ({e}); fetch the ROMs with `inkemu rom fetch --all --rom-dir <dir>` and set the variable to <dir>"
        )
    });
    let path = std::path::Path::new(&dir).join(format!("esp32c3_rev{revision}_rom.elf"));
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read the ROM ELF {} ({e}); fetch the ROMs with `inkemu rom fetch --all --rom-dir {dir}`",
            path.display()
        )
    })
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

/// Where the synthetic bootloader is loaded and entered: the start of the SRAM seen from the instruction bus.
const STUB_ENTRY: u32 = 0x4038_0000;
/// `j .`
const STUB_CODE: [u8; 4] = [0x6f, 0, 0, 0];
/// Where the ROM parks after `ets_run_flash_bootloader` failed: the `j .` of `main` (FB §H, "hang").
const ROM_HANG_PC: u32 = 0x4004_7ed2;
/// The ROM path from the banner to the bootloader entry is about 121,000 instructions (FB §1 item 1).
const FLASH_BOOT_BUDGET: u64 = 40_000_000;
const INVALID_HEADER_BLANK: &str = "invalid header: 0xffffffff\n";

fn stub_spec() -> ImageSpec {
    ImageSpec {
        chip: 5,
        entry: STUB_ENTRY,
        flash_size_nibble: 4,
        spi_mode: 2,
        segments: vec![(STUB_ENTRY, STUB_CODE.to_vec())],
        hash_appended: true,
    }
}

struct FlashBoot {
    /// UART0 output after the three banner lines, carriage returns dropped.
    after_banner: String,
    diagnostics: Vec<Diagnostic>,
    stop: StopReason,
    pc: u32,
    trace_hash: Option<u64>,
}

/// Boots the rev3 ROM on `flash` (`None`: an erased chip) until `done` accepts the text printed after
/// the banner, then one more slice so the ROM reaches the place it stops at.
fn flash_boot(flash: Option<Vec<u8>>, config: SocConfig, done: impl Fn(&str) -> bool) -> FlashBoot {
    let rom = RomImage::from_elf(&elf(3)).unwrap();
    let mut machine = match flash {
        Some(image) => Esp32C3::with_flash(rom, image, &config).unwrap(),
        None => Esp32C3::new(rom, &config),
    };
    let banner_text = banner("esp32c3-api1-20210207", "Feb  7 2021");
    let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
    let mut stop = StopReason::CycleBudget;
    let mut extra_slices = 1;
    let after = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes).replace('\r', "");
        text.strip_prefix(&banner_text)
            .map(str::to_owned)
            .unwrap_or_default()
    };
    while stop == StopReason::CycleBudget && machine.cycles().get() < FLASH_BOOT_BUDGET {
        if done(&after(&uart0)) {
            if extra_slices == 0 {
                break;
            }
            extra_slices -= 1;
        }
        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
        diagnostics.extend(machine.drain_diagnostics());
    }
    FlashBoot {
        after_banner: after(&uart0),
        diagnostics,
        stop,
        pc: machine.pc(),
        trace_hash: machine.trace_hash(),
    }
}

fn strict() -> SocConfig {
    SocConfig {
        strict_mmio: true,
        ..SocConfig::default()
    }
}

/// The hexadecimal number that follows `label` in `text`.
fn hex_after(text: &str, label: &str) -> u8 {
    let rest = &text[text
        .find(label)
        .unwrap_or_else(|| panic!("no `{label}` in {text:?}"))
        + label.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    u8::from_str_radix(&digits, 16).unwrap()
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rom_loads_a_synthetic_bootloader_and_jumps_to_its_entry() {
    let boot = flash_boot(Some(build_image(&stub_spec())), strict(), |text| {
        text.contains("entry 0x")
    });
    assert_eq!(
        boot.after_banner,
        "SPIWP:0xee\nmode:DIO, clock div:1\nload:0x40380000,len:0x4\nentry 0x40380000\n",
        "FB §G; no `SHA-256 comparison failed` line: the hardware digest equals the appended one"
    );
    assert_eq!(
        boot.diagnostics,
        Vec::new(),
        "nothing unmodelled, no exception, on the whole flash path"
    );
    assert_eq!(boot.stop, StopReason::CycleBudget);
    assert_eq!(boot.pc, STUB_ENTRY, "the ROM jumped into the image");
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn blank_flash_prints_invalid_header_forever() {
    // FB §H row 1: a tight loop, no SPIWP line, never exits.
    let boot = flash_boot(None, strict(), |text| {
        text.matches(INVALID_HEADER_BLANK).count() >= 3
    });
    assert!(
        boot.after_banner.matches(INVALID_HEADER_BLANK).count() >= 3,
        "{:?}",
        boot.after_banner
    );
    assert!(
        boot.after_banner
            .lines()
            .all(|line| line == INVALID_HEADER_BLANK.trim_end()
                || INVALID_HEADER_BLANK.starts_with(line)),
        "only that line, the last one possibly cut by the slice: {:?}",
        boot.after_banner
    );
    assert_eq!(boot.diagnostics, Vec::new());
    assert_eq!(boot.stop, StopReason::CycleBudget, "a loop, not a stop");
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_header_that_is_not_an_image_is_printed_byte_by_byte() {
    // FB §H rows 2 and 3: the four header bytes are printed as hdr[3] hdr[2] hdr[1] hdr[0].
    let zeros = flash_boot(Some(vec![0u8; 16]), strict(), |text| text.contains('\n'));
    assert!(
        zeros
            .after_banner
            .starts_with("invalid header: 0x00000000\n"),
        "{:?}",
        zeros.after_banner
    );
    assert_eq!(zeros.diagnostics, Vec::new());
    let mut image = build_image(&stub_spec());
    image[0] = 0x00;
    let no_magic = flash_boot(Some(image), strict(), |text| text.contains('\n'));
    assert!(
        no_magic
            .after_banner
            .starts_with("invalid header: 0x4f020100\n"),
        "{:?}",
        no_magic.after_banner
    );
    assert_eq!(no_magic.diagnostics, Vec::new());
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn an_image_for_another_chip_stops_the_rom_before_it_configures_the_flash() {
    // FB §H: chip id 9 -> one message, `ets_main.c 333 `, then `j .`; the check comes before SPIWP.
    let image = build_image(&ImageSpec {
        chip: 9,
        ..stub_spec()
    });
    let boot = flash_boot(Some(image), strict(), |text| {
        text.contains("ets_main.c 333 \n")
    });
    assert_eq!(
        boot.after_banner,
        "Invalid chip id. Expected 5 read 9. Bootloader for wrong chip?\nets_main.c 333 \n"
    );
    assert_eq!(boot.pc, ROM_HANG_PC);
    assert_eq!(boot.diagnostics, Vec::new());
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_flipped_data_bit_is_a_checksum_failure() {
    // FB §H: detected after the `load:` line; the ROM prints both checksums, then hangs.
    const FIRST_DATA_BYTE: usize = 24 + 8; // image header, segment header
    let mut image = build_image(&stub_spec());
    image[FIRST_DATA_BYTE] ^= 0x01;
    let stored = image[image.len() - 32 - 1];
    let boot = flash_boot(Some(image), strict(), |text| {
        text.contains("ets_main.c 333 \n")
    });
    assert!(
        boot.after_banner
            .contains("load:0x40380000,len:0x4\nChecksum failure. Calculated 0x"),
        "{:?}",
        boot.after_banner
    );
    assert_eq!(hex_after(&boot.after_banner, "stored 0x"), stored);
    assert_eq!(
        hex_after(&boot.after_banner, "Calculated 0x"),
        stored ^ 0x01
    );
    assert!(!boot.after_banner.contains("entry"));
    assert_eq!(boot.pc, ROM_HANG_PC);
    assert_eq!(boot.diagnostics, Vec::new());
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_wrong_digest_is_reported_and_the_rom_boots_anyway() {
    // FB §F step 7 and §H: a SHA mismatch is not fatal without secure boot. The digest the ROM prints
    // as "Calculated" comes out of the SHA accelerator model.
    let mut image = build_image(&stub_spec());
    let hashed = image.len() - 32;
    let calculated = common::hex(&Sha256::digest(&image[..hashed]));
    let last = image.len() - 1;
    image[last] ^= 0xFF;
    let expected = common::hex(&image[hashed..]);
    let boot = flash_boot(Some(image), strict(), |text| text.contains("entry 0x"));
    // The text of the ROM as it prints it (FB §H, "digest byte flipped"), digests in lower case.
    assert!(
        boot.after_banner.contains(&format!(
            "SHA-256 comparison failed:\nCalculated: {calculated}\nExpected: {expected}\nAttempting to boot anyway...\nentry 0x40380000\n"
        )),
        "{:?}",
        boot.after_banner
    );
    assert_eq!(boot.pc, STUB_ENTRY);
    assert_eq!(boot.diagnostics, Vec::new());
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn two_flash_boots_are_identical_down_to_the_trace_hash() {
    let config = SocConfig {
        trace_hash: true,
        ..SocConfig::default()
    };
    let run = || {
        flash_boot(Some(build_image(&stub_spec())), config, |text| {
            text.contains("entry 0x")
        })
    };
    let (first, second) = (run(), run());
    // Two runs that both stopped short of the bootloader would compare equal too.
    assert!(
        first.after_banner.contains("entry 0x40380000\n"),
        "{:?}",
        first.after_banner
    );
    assert!(first.trace_hash.is_some());
    assert_eq!(first.trace_hash, second.trace_hash);
    assert_eq!(first.after_banner, second.after_banner);
}
