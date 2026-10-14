//! Tests against the real mask ROMs. They need the ELF files of the pinned esp-rom-elfs archive:
//! `INKEMU_ROM_ELF_DIR=<dir> cargo test -p soc-esp32c3 --test real_rom -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

mod common;

use emu_core::Cycles;
use firmware::builder::{ImageSpec, build_image};
use sha2::{Digest, Sha256};
use soc_esp32c3::reset::ResetCause;
use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
use soc_esp32c3::{
    ChipRevision, ConsoleChannel, Diagnostic, Esp32C3, RomImage, SocConfig, StopReason, UsbHost,
};

/// The ROM ELF of `revision` in the directory `INKEMU_ROM_ELF_DIR` names; a failure says why and
/// what to do. The test reads a file the environment names; the crate's code does no host I/O
/// (`clippy.toml`).
#[allow(clippy::disallowed_methods)]
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
/// The device time of a boot to the banner: ten times what [`BANNER_BUDGET`] lasts at the 20 MHz
/// the ROM runs at. A bound for a CPU that would sleep, whose cycle counter stands still, so that
/// the first bound never comes; a bound, not an expectation.
const BANNER_TIME_BUDGET_NS: u64 = 10 * SECOND_NS;
const SLICE: u64 = 100_000;
const BOOT_LINE: &str = "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n";

struct Boot {
    uart0: String,
    usb: String,
    /// What the firmware wrote on USB-Serial-JTAG and the host did not take.
    usb_left: Vec<u8>,
    diagnostics: Vec<Diagnostic>,
    driven: Driven,
    trace_hash: Option<u64>,
    cycles: u64,
}

/// Runs the ROM of `revision` until its boot line is complete on UART0, or the budget runs out.
/// Carriage returns are dropped: the ROM expands `\n` to `\r\n` on the UART.
fn boot(revision: u16, config: SocConfig) -> Boot {
    let mut machine = Esp32C3::new(RomImage::from_elf(&elf(revision)).unwrap(), &config);
    let mut consoles = Consoles::default();
    let bounds = Bounds {
        slice: SLICE,
        max_cycles: BANNER_BUDGET,
        max_nanos: BANNER_TIME_BUDGET_NS,
    };
    let driven = drive(&mut machine, bounds, &mut consoles, |_, _, seen| {
        seen.text(ConsoleChannel::Uart0).contains(BOOT_LINE)
    });
    Boot {
        uart0: consoles.text(ConsoleChannel::Uart0),
        usb: consoles.text(ConsoleChannel::UsbSerialJtag),
        diagnostics: consoles.diagnostics,
        driven,
        trace_hash: machine.trace_hash(),
        cycles: machine.cycles().get(),
        usb_left: machine.flush_console(ConsoleChannel::UsbSerialJtag),
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

/// With no host, or a host whose port is closed, nobody takes the ROM's first USB packet: the ROM
/// waits for room 5,000 times, gives up and drops the rest of its USB output, and its banner on
/// UART0 goes on (report USJ §B.1, §F.3).
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev3_rom_keeps_its_first_usb_packet_without_a_host_that_takes_it_and_goes_on() {
    for usb_host in [UsbHost::Absent, UsbHost::Attached] {
        let boot = boot(
            3,
            SocConfig {
                usb_host,
                ..SocConfig::default()
            },
        );
        assert!(
            boot.uart0
                .starts_with(&banner("esp32c3-api1-20210207", "Feb  7 2021")),
            "{usb_host:?}: UART0 printed {:?}",
            boot.uart0
        );
        assert_eq!(boot.usb, "", "{usb_host:?}: the host took nothing");
        assert_eq!(
            String::from_utf8_lossy(&boot.usb_left),
            "ESP-ROM:esp32c3-api1-20210207\r\n",
            "{usb_host:?}: the first packet only"
        );
    }
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
        boot.driven,
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
/// The device time of a flash boot: ten times what [`FLASH_BOOT_BUDGET`] lasts at 20 MHz, for the
/// same reason as [`BANNER_TIME_BUDGET_NS`].
const FLASH_BOOT_TIME_BUDGET_NS: u64 = 20 * SECOND_NS;
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
    driven: Driven,
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
    let after = |consoles: &Consoles| {
        consoles
            .text(ConsoleChannel::Uart0)
            .strip_prefix(&banner_text)
            .map(str::to_owned)
            .unwrap_or_default()
    };
    let mut consoles = Consoles::default();
    let bounds = Bounds {
        slice: SLICE,
        max_cycles: FLASH_BOOT_BUDGET,
        max_nanos: FLASH_BOOT_TIME_BUDGET_NS,
    };
    let mut extra_slices = 1;
    let driven = drive(&mut machine, bounds, &mut consoles, |_, _, seen| {
        if !done(&after(seen)) {
            return false;
        }
        if extra_slices == 0 {
            return true;
        }
        extra_slices -= 1;
        false
    });
    FlashBoot {
        after_banner: after(&consoles),
        diagnostics: consoles.diagnostics,
        driven,
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
    assert_eq!(boot.driven, Driven::Done);
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
    assert_eq!(boot.driven, Driven::Done, "a loop, not a stop");
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

/// One second of device time, in nanoseconds.
const SECOND_NS: u64 = 1_000_000_000;
/// Where the watchdogs catch the ROM of a blank chip: in its UART routine
/// (`ROM3:uart_tx_one_char_uart.part.0@0x4004c0be`), in the loop that waits for room in the FIFO
/// while it prints `invalid header`. Its four instructions, as the rev3 ROM ELF disassembles:
/// `lw a4,0(a3)` reads `UART_STATUS`, `srli a4,a4,0x10` and `andi a4,a4,896` keep the top bits of
/// `TXFIFO_CNT`, `bnez a4,0x4004c0d4` loops while the FIFO holds 128 bytes or more. Which of the
/// four a watchdog expiry finds depends on the phase of the loop against the expiry, which
/// anything that changes the time the ROM spends elsewhere (its USB-Serial-JTAG output, for one)
/// moves: these tests pin the loop, and the trace hashes pin the run.
const ROM_UART_WAIT_LOOP: [u32; 4] = [0x4004_c0d4, 0x4004_c0d6, 0x4004_c0d8, 0x4004_c0dc];
/// Cycles between two looks at the machine in the watchdog tests: 50 us at the 20 MHz the ROM
/// runs at, the resolution the instant of a reset is observed with.
const WATCHDOG_SLICE: u64 = 1_000;
const WATCHDOG_SLICE_NS: u64 = 50_000;

/// What a run that is reset by its watchdogs looked like.
struct WatchdogRun {
    console: String,
    /// Device time, in nanoseconds, at the first look that found each reset done.
    reset_seen_at: Vec<u64>,
    /// The journal of the run: each diagnostic the first time it came.
    diagnostics: Vec<Diagnostic>,
    /// The diagnostics that came more than once, with their number of occurrences.
    repeated: Vec<(Diagnostic, u64)>,
}

/// The entry a reset leaves in the journal, for a comparison that leaves the stack pointer out:
/// the tests below pin the cause and the PC.
fn reset_entries(diagnostics: &[Diagnostic]) -> Vec<(ResetCause, u32)> {
    diagnostics
        .iter()
        .map(|diagnostic| match diagnostic {
            Diagnostic::Reset { cause, pc, .. } => (*cause, *pc),
            other => panic!("strict mode: nothing but resets is reported, got: {other}"),
        })
        .collect()
}

/// The rev3 ROM on `flash` (`None`: an erased chip), in strict mode, at power-on.
fn strict_machine(flash: Option<Vec<u8>>) -> Esp32C3 {
    let rom = RomImage::from_elf(&elf(3)).unwrap();
    match flash {
        Some(image) => Esp32C3::with_flash(rom, image, &strict()).unwrap(),
        None => Esp32C3::new(rom, &strict()),
    }
}

/// Runs `machine` until `resets` resets took place and `done` accepts the console, which is
/// everything UART0 sent since power-on.
///
/// Two bounds end a run that does not get there, and the failure says what was seen: a budget of CPU
/// cycles, and `limit_ns` of device time. Both are needed: the cycle counter stands still while the
/// CPU sleeps, so a machine that sleeps for ever would never exhaust the first.
fn run_to_resets(
    mut machine: Esp32C3,
    resets: u64,
    limit_ns: u64,
    done: impl Fn(&str) -> bool,
) -> WatchdogRun {
    let mut reset_seen_at = Vec::new();
    let mut consoles = Consoles::default();
    let bounds = Bounds {
        slice: WATCHDOG_SLICE,
        max_cycles: FLASH_BOOT_BUDGET * 4,
        max_nanos: limit_ns,
    };
    let driven = drive(&mut machine, bounds, &mut consoles, |machine, _, seen| {
        if machine.resets() > u64::try_from(reset_seen_at.len()).unwrap() {
            reset_seen_at.push(machine.device_time_nanos());
        }
        machine.resets() >= resets && done(&seen.text(ConsoleChannel::Uart0))
    });
    assert_eq!(
        driven,
        Driven::Done,
        "after {} resets, at {} ns of device time and {} cycles (limits: {limit_ns} ns, {} cycles); console:\n{}",
        machine.resets(),
        machine.device_time_nanos(),
        machine.cycles().get(),
        FLASH_BOOT_BUDGET * 4,
        consoles.text(ConsoleChannel::Uart0)
    );
    WatchdogRun {
        console: consoles.text(ConsoleChannel::Uart0),
        reset_seen_at,
        diagnostics: consoles.diagnostics,
        repeated: machine.most_repeated_diagnostics(8),
    }
}

/// Milestone point 4 of plan SP1b-3a. Nothing stops the watchdogs the reset values arm (TW §D.4):
/// main watchdog 0 after 26 000 000 cycles of the 20 MHz APB clock, 1.3 s, a core reset the ROM
/// reports as `rst:0x7` with the PC it was caught at; the RTC watchdog, which a core reset does
/// not restart, after 400 000 cycles of the 136 kHz slow clock, 2.941 s, a system reset reported
/// as `rst:0x10` without a saved PC.
///
/// The 1.3 s and the 2.941 s are the model's figures, not measurements: on silicon the logs give at
/// least 0.955 s between two resets (TW §H.2), and the RC oscillator behind the RTC watchdog is
/// modelled at its nominal 136 kHz (TW §I question 2).
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_blank_flash_is_reset_by_the_main_watchdog_then_by_the_rtc_watchdog() {
    // Three seconds of device time beyond the last reset: the whole run is a little over 2.9 s.
    let limit_ns = 2_941_176_471 + 3 * SECOND_NS;
    // Until the boot that follows the third reset has printed a whole line of its own.
    let run = run_to_resets(strict_machine(None), 3, limit_ns, |console| {
        console
            .split("rst:0x10 (RTCWDT_RTC_RST)")
            .nth(1)
            .is_some_and(|rest| rest.contains(INVALID_HEADER_BLANK))
    });
    // Strict mode: nothing unmodelled. The journal holds the resets and nothing else, one entry
    // each (spec §7), in the order they came: main watchdog 0 twice, then the RTC watchdog, each
    // in the loop where the ROM waits for room in the UART FIFO.
    let resets = reset_entries(&run.diagnostics);
    assert_eq!(
        resets.iter().map(|&(cause, _)| cause).collect::<Vec<_>>(),
        [
            ResetCause::Mwdt0Core,
            ResetCause::Mwdt0Core,
            ResetCause::RtcWatchdogSystem
        ],
        "journal {:?}",
        run.diagnostics
    );
    assert_eq!(run.repeated, Vec::new(), "no reset is a repeat of another");
    assert!(
        resets.iter().all(|(_, pc)| ROM_UART_WAIT_LOOP.contains(pc)),
        "every reset finds the ROM in its wait for room in the UART FIFO: {resets:?}"
    );
    // A reset is seen at the first look after it: the loop looks every `WATCHDOG_SLICE` cycles, 50 us
    // at 20 MHz, so a reset is seen up to 50 us after its instant. The window of 100 us is that
    // resolution (one look) and as much again as margin (a second look); it is not a modelling
    // error margin.
    assert_eq!(
        run.reset_seen_at.len(),
        3,
        "three resets: {:?}",
        run.reset_seen_at
    );
    for (seen, expired_ns, what) in [
        (run.reset_seen_at[0], 13 * SECOND_NS / 10, "main watchdog 0"),
        (
            run.reset_seen_at[1],
            26 * SECOND_NS / 10,
            "main watchdog 0 again",
        ),
        // 400 000 slow-clock cycles at 136 kHz: 2.941 176 5 s.
        (run.reset_seen_at[2], 2_941_176_471, "the RTC watchdog"),
    ] {
        assert!(
            (expired_ns..=expired_ns + 2 * WATCHDOG_SLICE_NS).contains(&seen),
            "{what} expires at {expired_ns} ns of device time; the reset was seen at {seen} ns"
        );
    }
    let lines: Vec<&str> = run.console.lines().collect();
    let boots: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("rst:"))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        boots.iter().map(|&i| lines[i]).collect::<Vec<_>>(),
        [
            "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)",
            "rst:0x7 (TG0WDT_SYS_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
            "rst:0x7 (TG0WDT_SYS_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
            "rst:0x10 (RTCWDT_RTC_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
        ]
    );
    // The ROM prints `invalid header` until the watchdog expires: every boot says it before the
    // reset that ends it, between its `rst:` line and the next one.
    for (boot, next) in boots.iter().zip(boots.iter().skip(1)) {
        assert!(
            lines[boot + 1..*next].contains(&INVALID_HEADER_BLANK.trim_end()),
            "the boot at line {boot} printed no `invalid header` before its reset:\n{}",
            lines[*boot..*next].join("\n")
        );
    }
    // The ROM was printing `invalid header` when it was reset: the line each boot after a reset
    // by main watchdog 0 prints names the instruction the journal names for that reset.
    for (boot, (_, pc)) in boots[1..3].iter().zip(&resets[..2]) {
        assert_eq!(lines[boot + 1], format!("Saved PC:{pc:#010x}"));
    }
    assert_eq!(
        lines[boots[3] + 1],
        INVALID_HEADER_BLANK.trim_end(),
        "a system reset clears the recorded PC: no `Saved PC` line (TW §D.2)"
    );
}

/// `lui`/`addi`/`sw` storing `value` at `addr` through `t0` and `t1`.
fn store(addr: u32, value: u32) -> [u32; 4] {
    use cpu_rv32::asm::{addi, lui, sw};
    const T0: u8 = 5;
    const T1: u8 = 6;
    // `addi` sign-extends its 12 bits: the upper part is rounded so the sum is the value.
    let split = |word: u32| {
        let upper = word.wrapping_add(0x800) & 0xFFFF_F000;
        (upper, word.wrapping_sub(upper).cast_signed())
    };
    let (page, offset) = split(addr);
    let (upper, lower) = split(value);
    [
        lui(T0, page),
        lui(T1, upper),
        addi(T1, T1, lower),
        sw(T1, T0, offset),
    ]
}

/// Milestone point 5 of plan SP1b-3a (spec §10.3 criterion 4). A firmware stops the two
/// watchdogs the reset values arm, starts main watchdog 1 with a 500 ms stage whose action is a
/// core reset, masks interrupts (`mstatus` ← 0) and hangs: its own watchdog resets it, 500 ms after
/// it enabled it, and the next boot names the watchdog and the instruction it hung in.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_firmware_that_hangs_with_interrupts_masked_is_reset_by_its_own_watchdog() {
    use cpu_rv32::asm::{csrrwi, jal};
    const MSTATUS: u16 = 0x300;
    /// The stage the program gives its watchdog: 500 counts of a millisecond.
    const STAGE_NS: u64 = 500 * SECOND_NS / 1000;
    let timg1 = |name| svd_regs::esp32c3::addr("TIMG1", name);
    let mut program = vec![
        // `mstatus.MIE` is already 0 after a reset; the program clears it itself so the test does
        // not rest on a reset value.
        csrrwi(0, MSTATUS, 0),
    ];
    // `WDTCONFIG0` of both protections: unlocked at reset, 0 stops them (TW §B.3, §C.2).
    program.extend(store(svd_regs::esp32c3::addr("TIMG0", "WDTCONFIG0"), 0));
    program.extend(store(svd_regs::esp32c3::addr("RTC_CNTL", "WDTCONFIG0"), 0));
    // Prescaler 20 000 on the 20 MHz APB clock (`mwdt_ll_set_prescaler`): one count a millisecond.
    program.extend(store(timg1("WDTCONFIG1"), 20_000 << 16));
    // Stage 0: 500 counts. Enable, stage 0 = reset of the core (`mwdt_ll_config_stage`).
    program.extend(store(timg1("WDTCONFIG2"), 500));
    program.extend(store(timg1("WDTCONFIG0"), (1 << 31) | (3 << 29)));
    program.push(jal(0, 0));
    // The last word, the `j .`.
    let hang_pc = STUB_ENTRY + 4 * u32::try_from(program.len() - 1).unwrap();
    let code: Vec<u8> = program.iter().flat_map(|word| word.to_le_bytes()).collect();
    let spec = ImageSpec {
        segments: vec![(STUB_ENTRY, code)],
        ..stub_spec()
    };
    // The instant the watchdog is enabled at is read from the run, so that the test does not
    // rest on the time the ROM takes to boot the image: one instruction a call, to the boundary
    // at which the enabling store, the instruction before the `j .`, is next. Device time
    // moves at the end of an instruction: the store executes at the time read here.
    let enabling_store_pc = hang_pc - 4;
    let mut machine = strict_machine(Some(build_image(&spec)));
    while machine.pc() != enabling_store_pc {
        assert!(
            machine.cycles().get() < FLASH_BOOT_BUDGET,
            "the program never reached the store at {enabling_store_pc:#010x} that enables its watchdog: at pc {:#010x} after {} cycles",
            machine.pc(),
            machine.cycles().get()
        );
        let stop = machine.run_until(Cycles(machine.cycles().get() + 1));
        assert_eq!(stop, StopReason::CycleBudget);
    }
    let enabled_ns = machine.device_time_nanos();
    // The reset takes place one stage, 500 counts of 1 ms, after that store. That a watchdog
    // counts its first stage from 0 when it is switched on is the model's choice, marked as an
    // assumption where the timer groups enable it: it is not observed on silicon.
    //
    // A prescaler half as large moves the reset by 250 ms and one twice as large by 500 ms, a
    // stage of 250 or 1 000 counts by 250 or 500 ms, an immediate expiry by 500 ms: the window
    // of 100 us (the resolution of the looks, as in the blank-flash test) is thousands of times
    // tighter than any of them.
    let run = run_to_resets(
        machine,
        1,
        enabled_ns + STAGE_NS + 3 * SECOND_NS,
        |console| {
            console
                .split("Saved PC:")
                .nth(1)
                .is_some_and(|rest| rest.contains('\n'))
        },
    );
    assert_eq!(
        reset_entries(&run.diagnostics),
        [(ResetCause::Mwdt1Core, hang_pc)],
        "the journal: the reset by the firmware's own watchdog, in its `j .`, and nothing else"
    );
    assert_eq!(run.repeated, Vec::new());
    let after_enabling = run.reset_seen_at[0] - enabled_ns;
    assert!(
        (STAGE_NS..=STAGE_NS + 2 * WATCHDOG_SLICE_NS).contains(&after_enabling),
        "main watchdog 1 expires {STAGE_NS} ns after it is enabled (at {enabled_ns} ns of device time); the reset was seen {after_enabling} ns after it"
    );
    let lines: Vec<&str> = run.console.lines().collect();
    let reset = lines
        .iter()
        .position(|line| line.starts_with("rst:0x8"))
        .unwrap_or_else(|| panic!("no reset by main watchdog 1:\n{}", run.console));
    assert_eq!(
        lines[reset],
        "rst:0x8 (TG1WDT_SYS_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)"
    );
    assert_eq!(
        lines[reset + 1],
        format!("Saved PC:{hang_pc:#010x}"),
        "the `j .` of the firmware"
    );
}
