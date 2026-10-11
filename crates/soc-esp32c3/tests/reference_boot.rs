//! The milestone of plan SP1b-2 on the reference firmware (ESP-IDF v5.5.2 bootloader + `hello_world`):
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_REF_FLASH=<flash_hello.bin> cargo test -p soc-esp32c3 --test reference_boot -- --ignored`
//! Build the image with `tools/reference-firmware/build.sh`.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

mod common;

use emu_core::Cycles;
use sha2::{Digest, Sha256};
use soc_esp32c3::diag::Unsupported;
use soc_esp32c3::memmap::{ROM_BASE, ROM_SIZE};
use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, RomImage, SocConfig, StopReason};

/// SHA-256 of the merged reference image (`docs/research/sp1b-test-oracle.md`, §2).
const REFERENCE_SHA256: &str = "fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2";
/// QEMU reaches the first application line after 1.69 M instructions (BA §1); far more than needed.
const BUDGET: u64 = 400_000_000;
const SLICE: u64 = 1_000_000;

/// The ROM lines of the reference image (FB §G, measured).
const ROM_LINES: &str = "ESP-ROM:esp32c3-api1-20210207\nBuild:Feb  7 2021\nrst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\nSPIWP:0xee\nmode:DIO, clock div:1\nload:0x3fcd5820,len:0x15a4\nload:0x403cbf10,len:0xc64\nload:0x403ce710,len:0x2fd0\nentry 0x403cbf1a\n";

/// The 2nd-stage bootloader lines of the golden log (TO §2), timestamps masked, with the two
/// documented differences from the QEMU capture:
/// - `efuse block revision: v1.2`, the production chip of decision D9 (QEMU's virtual chip is v1.3);
/// - `SPI Mode : DIO`, because SPI0 stores the mode the ROM programmed (QEMU does not model SPI0 and
///   prints `SLOW READ`; BA §2.1 item 3).
const BOOTLOADER_LINES: [&str; 21] = [
    "I (#) boot: ESP-IDF v5.5.2 2nd stage bootloader",
    "I (#) boot: chip revision: v0.3",
    "I (#) boot: efuse block revision: v1.2",
    "I (#) boot.esp32c3: SPI Speed      : 80MHz",
    "I (#) boot.esp32c3: SPI Mode       : DIO",
    "I (#) boot.esp32c3: SPI Flash Size : 16MB",
    "I (#) boot: Enabling RNG early entropy source...",
    "I (#) boot: Partition Table:",
    "I (#) boot: ## Label            Usage          Type ST Offset   Length",
    "I (#) boot:  0 nvs              WiFi data        01 02 00009000 00006000",
    "I (#) boot:  1 phy_init         RF data          01 01 0000f000 00001000",
    "I (#) boot:  2 factory          factory app      00 00 00010000 00100000",
    "I (#) boot: End of partition table",
    "I (#) esp_image: segment 0: paddr=00010020 vaddr=3c010020 size=0656ch ( 25964) map",
    "I (#) esp_image: segment 1: paddr=00016594 vaddr=3fc8b600 size=011d8h (  4568) load",
    "I (#) esp_image: segment 2: paddr=00017774 vaddr=40380000 size=088a4h ( 34980) load",
    "I (#) esp_image: segment 3: paddr=00020020 vaddr=42000020 size=0e6fch ( 59132) map",
    "I (#) esp_image: segment 4: paddr=0002e724 vaddr=403888a4 size=02c70h ( 11376) load",
    "I (#) esp_image: segment 5: paddr=0003139c vaddr=50000000 size=00020h (    32) load",
    "I (#) boot: Loaded app from partition at offset 0x10000",
    "I (#) boot: Disabling RNG early entropy source...",
];
const FIRST_APPLICATION_LINE: &str = "I (#) cpu_start: Unicore app";
/// `call_start_cpu0` of the application (BA §1).
const APPLICATION_ENTRY: u32 = 0x4038_0330;
/// The address ranges of the application's code, from the `esp_image` segment lines of the golden
/// log: IRAM is segment 2 (`vaddr=40380000 size=088a4h`) followed contiguously by segment 4
/// (`vaddr=403888a4 size=02c70h`), ending at 0x4038b514; the flash text window is segment 3
/// (`vaddr=42000020 size=0e6fch`), ending at 0x4200e71c. The bootloader runs at `0x403c_xxxx` and the
/// ROM below `0x4006_0000`, so neither is inside.
const APPLICATION_CODE: [std::ops::Range<u32>; 2] =
    [0x4038_0000..0x4038_b514, 0x4200_0020..0x4200_e71c];
/// The stock partition table of the Xteink X4 (repository fixture, built from `x4-stock.csv`):
/// `nvs`, `otadata`, two OTA slots — `app0` at 0x10000, where the reference image has its
/// application — `spiffs` and `coredump`; no factory application.
const X4_STOCK_TABLE: &[u8] = include_bytes!("../../../tests/fixtures/partitions/x4-stock.bin");
/// Flash offset of the partition table (ESP-IDF default, `CONFIG_PARTITION_TABLE_OFFSET`).
const PARTITION_TABLE_OFFSET: usize = 0x8000;
/// The instruction-bus addresses of the mask ROM (report BC §A.1).
const MASK_ROM: std::ops::Range<u32> = ROM_BASE..ROM_BASE + ROM_SIZE;

/// The ESP32-C3 (rev 3) mask ROM named by `INKEMU_ROM_ELF_DIR`; every failure says why and what to do.
fn rom() -> RomImage {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").unwrap_or_else(|e| {
        panic!(
            "INKEMU_ROM_ELF_DIR is not usable ({e}); fetch the ROMs with `inkemu rom fetch --all --rom-dir <dir>` and set the variable to <dir>"
        )
    });
    let path = std::path::Path::new(&dir).join("esp32c3_rev3_rom.elf");
    let elf = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read the ROM ELF {} ({e}); fetch the ROMs with `inkemu rom fetch --all --rom-dir {dir}`",
            path.display()
        )
    });
    RomImage::from_elf(&elf).unwrap_or_else(|e| {
        panic!(
            "{} is not a usable ESP32-C3 ROM ELF ({e}); delete it and fetch it again with `inkemu rom fetch --all --rom-dir {dir}`",
            path.display()
        )
    })
}

/// The reference flash image named by `INKEMU_REF_FLASH`, accepted only if it is the pinned one:
/// every test boots it, so a wrong image fails here with its real cause instead of as a
/// golden-log difference.
fn reference_flash() -> Vec<u8> {
    let path = std::env::var("INKEMU_REF_FLASH").unwrap_or_else(|e| {
        panic!(
            "INKEMU_REF_FLASH is not usable ({e}); build the image with `tools/reference-firmware/build.sh` and set the variable to the flash_hello.bin it prints"
        )
    });
    let image = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read the reference image {path} ({e}); build it with `tools/reference-firmware/build.sh`"
        )
    });
    let digest = common::hex(&Sha256::digest(&image));
    assert_eq!(
        digest, REFERENCE_SHA256,
        "{path} is not the pinned reference image; rebuild it with `tools/reference-firmware/build.sh`, which verifies the pins"
    );
    image
}

/// `I (123) tag: text` becomes `I (#) tag: text`: timestamps depend on the timing model.
fn mask_timestamp(line: &str) -> String {
    match (line.strip_prefix("I ("), line.find(") ")) {
        (Some(_), Some(close)) if line[3..close].bytes().all(|b| b.is_ascii_digit()) => {
            format!("I (#{}", &line[close..])
        }
        _ => line.to_owned(),
    }
}

struct Run {
    console: String,
    diagnostics: Vec<Diagnostic>,
    stop: StopReason,
    trace_hash: Option<u64>,
}

/// Runs the reference image until `done` accepts the console, the machine stops, or the budget ends.
fn run(config: SocConfig, done: impl Fn(&str) -> bool) -> Run {
    run_image(reference_flash(), config, |console, _| done(console))
}

/// Runs `image` until `done` accepts the console and the diagnostics seen so far, the machine
/// stops, or the budget ends.
fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic]) -> bool) -> Run {
    let mut machine = Esp32C3::with_flash(rom(), image, &config).unwrap();
    let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
    let mut stop = StopReason::CycleBudget;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
    while stop == StopReason::CycleBudget
        && machine.cycles().get() < BUDGET
        && !done(&text(&uart0), &diagnostics)
    {
        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
        diagnostics.extend(machine.drain_diagnostics());
    }
    Run {
        console: text(&uart0),
        diagnostics,
        stop,
        trace_hash: machine.trace_hash(),
    }
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn rom_bootloader_and_application_print_the_golden_log() {
    let boot = run(SocConfig::default(), |console| {
        console.contains("cpu_start: Unicore app\n")
    });
    assert!(
        boot.console.starts_with(ROM_LINES),
        "ROM part:\n{}",
        boot.console
    );
    let after_rom: Vec<String> = boot.console[ROM_LINES.len()..]
        .lines()
        .map(mask_timestamp)
        .collect();
    let mut expected: Vec<&str> = BOOTLOADER_LINES.to_vec();
    expected.push(FIRST_APPLICATION_LINE);
    assert_eq!(
        after_rom
            .iter()
            .map(String::as_str)
            .take(expected.len())
            .collect::<Vec<_>>(),
        expected,
        "bootloader and first application line\ndiagnostics: {:#?}",
        boot.diagnostics
    );
    assert!(
        !boot
            .diagnostics
            .iter()
            .any(|d| matches!(d, Diagnostic::Exception { .. })),
        "no CPU exception: {:#?}",
        boot.diagnostics
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn strict_run_reaches_the_application_and_ends_on_the_rtc_calibration() {
    let strict = SocConfig {
        strict_mmio: true,
        ..SocConfig::default()
    };
    let boot = run(strict, |_| false);
    // Milestone point 4: nothing unmodelled and no exception from reset to the application entry;
    // then the run ends on a *named* register, the first one plan SP1b-3 has to model (BA §9).
    let StopReason::StrictMmio(violation) = &boot.stop else {
        panic!(
            "expected a strict stop, got {:?}\nconsole:\n{}",
            boot.stop, boot.console
        );
    };
    assert!(
        matches!(
            violation,
            Diagnostic::UnmodeledMmio {
                register: Some(("TIMG0", "RTCCALICFG")),
                ..
            }
        ),
        "the run must end on TIMG0.RTCCALICFG, it ended on: {violation}\nconsole:\n{}",
        boot.console
    );
    assert_eq!(
        boot.diagnostics,
        vec![violation.clone()],
        "nothing else was reported before it"
    );
    // The first application line is printed by the application's own `system_early_init` (BA §3):
    // seeing it means the bootloader jumped and the application ran from the flash window.
    assert!(
        boot.console
            .lines()
            .map(mask_timestamp)
            .any(|line| line == FIRST_APPLICATION_LINE),
        "{}",
        boot.console
    );
    let Diagnostic::UnmodeledMmio { pc, .. } = violation else {
        panic!("expected an unmodelled register access, got: {violation}");
    };
    assert!(
        APPLICATION_CODE.iter().any(|code| code.contains(pc)),
        "the access must come from the application's own code {APPLICATION_CODE:x?} (entry {APPLICATION_ENTRY:#010x}), not from the bootloader or the ROM: pc {pc:#010x}"
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn without_strict_mode_the_wait_on_the_rtc_calibration_is_a_named_warning_not_a_silent_hang() {
    // Far past the first application line: the application then polls the calibration register.
    let mut machine = Esp32C3::with_flash(rom(), reference_flash(), &SocConfig::default()).unwrap();
    let stop = machine.run_until(Cycles(60_000_000));
    let diagnostics = machine.drain_diagnostics();
    assert_eq!(stop, StopReason::CycleBudget);
    assert!(
        diagnostics.iter().any(|d| matches!(
            d,
            Diagnostic::UnmodeledMmio {
                register: Some(("TIMG0", "RTCCALICFG")),
                ..
            }
        )),
        "{diagnostics:#?}"
    );
    assert!(
        machine
            .most_repeated_diagnostics(3)
            .iter()
            .any(|(d, count)| *count > 1
                && matches!(
                    d,
                    Diagnostic::UnmodeledMmio {
                        register: Some(("TIMG0", "RTCCALICFG")),
                        ..
                    }
                )),
        "the end-of-run summary must show TIMG0.RTCCALICFG as what the firmware keeps polling"
    );
}

/// Spec §5.1: "Reset → ROM → bootloader → table de partitions → `otadata` → application". The
/// reference image has a factory application; the target device has none. Behind the stock X4
/// table the unmodified bootloader finds a blank `otadata`, falls back to OTA slot 0 and records
/// that choice: the mask ROM's sector-erase and page-program functions run. The flash is read-only
/// in this plan, so both are refused, each once and by its own opcode, and nothing else is
/// reported before the application reaches the RTC calibration.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn the_stock_x4_ota_layout_boots_and_only_the_otadata_update_is_refused() {
    let mut image = reference_flash();
    image[PARTITION_TABLE_OFFSET..PARTITION_TABLE_OFFSET + X4_STOCK_TABLE.len()]
        .copy_from_slice(X4_STOCK_TABLE);
    let is_rtc_calibration = |d: &Diagnostic| {
        matches!(
            d,
            Diagnostic::UnmodeledMmio {
                register: Some(("TIMG0", "RTCCALICFG")),
                ..
            }
        )
    };
    let boot = run_image(image, SocConfig::default(), |_, diagnostics| {
        diagnostics.iter().any(is_rtc_calibration)
    });
    assert!(
        boot.console.starts_with(ROM_LINES),
        "ROM part:\n{}",
        boot.console
    );
    let lines: Vec<String> = boot.console.lines().map(mask_timestamp).collect();
    let mut from = 0;
    for expected in [
        "I (#) boot: No factory image, trying OTA 0",
        "I (#) boot: Loaded app from partition at offset 0x10000",
        "I (#) boot: Set actual ota_seq=1 in otadata[0]",
        FIRST_APPLICATION_LINE,
    ] {
        let at = lines[from..]
            .iter()
            .position(|line| line == expected)
            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{lines:#?}"));
        from += at + 1;
    }
    let reached = boot
        .diagnostics
        .iter()
        .position(is_rtc_calibration)
        .unwrap_or_else(|| {
            panic!(
                "the application never reached TIMG0.RTCCALICFG: {:#?}\nconsole:\n{}",
                boot.diagnostics, boot.console
            )
        });
    let before: Vec<(Unsupported, bool)> = boot.diagnostics[..reached]
        .iter()
        .map(|diagnostic| match diagnostic {
            Diagnostic::Unsupported { what, pc } => (*what, MASK_ROM.contains(pc)),
            other => panic!("unexpected diagnostic before the RTC calibration: {other}"),
        })
        .collect();
    assert_eq!(
        before,
        [
            (Unsupported::FlashCommand { opcode: 0x20 }, true),
            (Unsupported::FlashCommand { opcode: 0x02 }, true),
        ],
        "exactly the sector erase and the page program of the otadata update, each run by the mask ROM"
    );
    assert_eq!(
        boot.stop,
        StopReason::CycleBudget,
        "nothing stops a lenient run"
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn two_boots_of_the_reference_image_are_identical_down_to_the_trace_hash() {
    let config = SocConfig {
        trace_hash: true,
        ..SocConfig::default()
    };
    let boot = || {
        run(config, |console| {
            console.contains("cpu_start: Unicore app\n")
        })
    };
    let (first, second) = (boot(), boot());
    // Two runs that both exhausted the budget before the application would compare equal too.
    assert!(
        first
            .console
            .lines()
            .map(mask_timestamp)
            .any(|line| line == FIRST_APPLICATION_LINE),
        "the boot must reach the application:\n{}",
        first.console
    );
    assert!(first.trace_hash.is_some());
    assert_eq!(
        first.trace_hash, second.trace_hash,
        "spec §8: same session, same trace hash"
    );
    assert_eq!(first.console, second.console);
}
