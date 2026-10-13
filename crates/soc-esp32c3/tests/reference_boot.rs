//! The milestones of plans SP1b-2, SP1b-3a and SP1b-3b on the reference firmware, three images
//! behind the same ESP-IDF v5.5.2 bootloader:
//! - `hello_world`: the boot, then the whole life of the application, its restart and a second
//!   boot;
//! - `nvs_rw_value_cxx`, which keeps a restart counter in NVS: three lives and the flash writes
//!   of each, behind its own partition table and behind the stock table of the X4, where the
//!   bootloader writes `otadata` first.
//!
//! Each image is named by its own variable, and a test whose variable is missing fails by naming
//! it. The whole suite:
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_REF_FLASH=<flash_hello.bin> INKEMU_NVS_FLASH=<flash_nvs.bin> INKEMU_NVS_OTA_FLASH=<flash_nvs_ota.bin> cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored`
//! Build the three images with `tools/reference-firmware/build.sh`, which prints their paths.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

mod common;

use sha2::{Digest, Sha256};
use soc_esp32c3::memmap::{SRAM_DBUS_BASE, SRAM_SIZE};
use soc_esp32c3::reset::ResetCause;
use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, FlashWrite, RomImage, SocConfig};

/// SHA-256 of the merged reference image (`docs/research/sp1b-test-oracle.md`, §2).
const REFERENCE_SHA256: &str = "fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2";
/// The CPU cycles every driver may spend: far more than a whole life cycle needs. QEMU reaches the
/// first application line after 1.69 M instructions (BA §1), the application's first life is 28 M
/// cycles to the restart, and the second boot to its first application line a few million more.
const BUDGET: u64 = 400_000_000;
/// The device time every driver may spend, in nanoseconds: the CPU cycles do not move while the
/// CPU sleeps, so a regression that leaves it asleep with a device counting would never reach
/// `BUDGET`. A first life, its restart and the second boot take about 12 s; 60 s is far more.
const DEVICE_TIME_BUDGET_NS: u64 = 60 * 1_000_000_000;
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
/// What the application prints from its first line to its restart, timestamps masked. Up to the
/// heap size it is the QEMU capture of TO §2 line for line; the capture ends there. The eleven
/// `Restarting in` lines are `hello_world_main.c`'s loop from 10 down to 0.
const APPLICATION_LINES: [&str; 39] = [
    FIRST_APPLICATION_LINE,
    "I (#) cpu_start: GPIO 20 and 21 are used as console UART I/O pins",
    "I (#) cpu_start: Pro cpu start user code",
    "I (#) cpu_start: cpu freq: 160000000 Hz",
    "I (#) app_init: Application information:",
    "I (#) app_init: Project name:     hello_world",
    "I (#) app_init: App version:      1",
    "I (#) app_init: ELF file SHA256:  43d81da8d...",
    "I (#) app_init: ESP-IDF:          v5.5.2",
    "I (#) efuse_init: Min chip rev:     v0.3",
    "I (#) efuse_init: Max chip rev:     v1.99 ",
    "I (#) efuse_init: Chip rev:         v0.3",
    "I (#) heap_init: Initializing. RAM available for dynamic allocation:",
    "I (#) heap_init: At 3FC8D610 len 000329F0 (202 KiB): RAM",
    "I (#) heap_init: At 3FCC0000 len 0001C710 (113 KiB): Retention RAM",
    "I (#) heap_init: At 3FCDC710 len 00002950 (10 KiB): Retention RAM",
    "I (#) heap_init: At 50000020 len 00001FC8 (7 KiB): RTCRAM",
    "I (#) spi_flash: detected chip: issi",
    "I (#) spi_flash: flash io: dio",
    "I (#) sleep_gpio: Configure to isolate all GPIO pins in sleep state",
    "I (#) sleep_gpio: Enable automatic switching of GPIO sleep configuration",
    "I (#) main_task: Started on CPU0",
    "I (#) main_task: Calling app_main()",
    "Hello world!",
    "This is esp32c3 chip with 1 CPU core(s), WiFi/BLE, silicon revision v0.3, 16MB external flash",
    "Minimum free heap size: 332800 bytes",
    "Restarting in 10 seconds...",
    "Restarting in 9 seconds...",
    "Restarting in 8 seconds...",
    "Restarting in 7 seconds...",
    "Restarting in 6 seconds...",
    "Restarting in 5 seconds...",
    "Restarting in 4 seconds...",
    "Restarting in 3 seconds...",
    "Restarting in 2 seconds...",
    "Restarting in 1 seconds...",
    "Restarting in 0 seconds...",
    "Restarting now.",
    // The second boot starts here: the banner of the ROM.
    "ESP-ROM:esp32c3-api1-20210207",
];
/// The ROM lines of the boot that follows `esp_restart`. Two differences from the first boot: the
/// cause, `RTC_SW_CPU_RST` (IR §4.3), and the `Saved PC` line a real chip prints after a CPU reset
/// and the QEMU oracle never does (IR §4.4, §6 row 16).
///
/// The address is the instruction after the store that asked for the reset, in the ROM's
/// `software_reset_cpu`: the model resets at the end of that instruction (the assumption marked
/// on `AssistDebug::cpu_reset`; a real chip shows an application address).
const SECOND_BOOT_ROM_LINES: [&str; 9] = [
    "Build:Feb  7 2021",
    "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
    "Saved PC:0x40048b92",
    "SPIWP:0xee",
    "mode:DIO, clock div:1",
    "load:0x3fcd5820,len:0x15a4",
    "load:0x403cbf10,len:0xc64",
    "load:0x403ce710,len:0x2fd0",
    "entry 0x403cbf1a",
];
/// One second of device time, in nanoseconds.
const SECOND_NS: u64 = 1_000_000_000;
/// The stock partition table of the Xteink X4 (repository fixture, built from `x4-stock.csv`):
/// `nvs`, `otadata`, two OTA slots — `app0` at 0x10000, where the reference image has its
/// application — `spiffs` and `coredump`; no factory application.
const X4_STOCK_TABLE: &[u8] = include_bytes!("../../../tests/fixtures/partitions/x4-stock.bin");
/// Flash offset of the partition table (ESP-IDF default, `CONFIG_PARTITION_TABLE_OFFSET`).
const PARTITION_TABLE_OFFSET: usize = 0x8000;
/// Flash offset of the `otadata` partition in the stock X4 table, and of its first entry.
const OTADATA_OFFSET: u32 = 0xE000;

/// The ESP32-C3 (rev 3) mask ROM named by `INKEMU_ROM_ELF_DIR`; every failure says why and what to do.
/// The test reads a file the environment names; the crate's code does no host I/O (`clippy.toml`).
#[allow(clippy::disallowed_methods)]
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
    pinned_image("INKEMU_REF_FLASH", "flash_hello.bin", REFERENCE_SHA256)
}

/// The image `file` of `tools/reference-firmware/build.sh`, named by the environment variable
/// `variable` and accepted only if its SHA-256 is `sha256`. The test reads a file the environment
/// names; the crate's code does no host I/O (`clippy.toml`).
#[allow(clippy::disallowed_methods)]
fn pinned_image(variable: &str, file: &str, sha256: &str) -> Vec<u8> {
    let path = std::env::var(variable).unwrap_or_else(|e| {
        panic!(
            "{variable} is not usable ({e}); build the image with `tools/reference-firmware/build.sh` and set the variable to the {file} it prints"
        )
    });
    let image = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read the reference image {path} ({e}); build it with `tools/reference-firmware/build.sh`"
        )
    });
    let digest = common::hex(&Sha256::digest(&image));
    assert_eq!(
        digest, sha256,
        "{path} is not the pinned {file}; rebuild it with `tools/reference-firmware/build.sh`, which verifies the pins"
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
    /// Every program, erase and status write the flash chip performed, in order.
    flash_writes: Vec<FlashWrite>,
    /// The whole content of the flash when the run ended.
    flash: Vec<u8>,
    flash_modified_sectors: u32,
    /// How many flash commands of each opcode were sent to the chip, indexed by opcode.
    flash_commands: Vec<u64>,
    /// How the drive ended: a run that stopped (a strict stop, a CPU waiting for ever) did not
    /// reach what it waited for.
    driven: Driven,
    trace_hash: Option<u64>,
    resets: u64,
    cycles: u64,
    device_time_nanos: u64,
}

/// The instruction after the store with which the ROM's `software_reset_cpu` asks for the reset
/// (`ROM3:software_reset_cpu@0x40048b84`): the PC the restart finds, in the journal of the run as
/// in the `Saved PC` line of the second boot.
const RESTART_PC: u32 = 0x4004_8b92;

/// Asserts that the journal of a whole life cycle holds the restart and nothing else: one reset,
/// asked for by the firmware, found at [`RESTART_PC`] with a stack pointer in the data RAM.
fn assert_only_the_restart_is_reported(diagnostics: &[Diagnostic]) {
    let [
        Diagnostic::Reset {
            cause: ResetCause::SoftwareCpu,
            pc: RESTART_PC,
            sp,
        },
    ] = diagnostics
    else {
        panic!("expected the restart and nothing else, got: {diagnostics:#?}");
    };
    assert!(
        (SRAM_DBUS_BASE..SRAM_DBUS_BASE + SRAM_SIZE).contains(sp),
        "the stack pointer of the task that called `esp_restart` is in the data RAM: {sp:#010x}"
    );
}

/// How many times the application printed its first line: once per boot that reached it.
fn application_starts(console: &str) -> usize {
    console.matches("cpu_start: Unicore app\n").count()
}

/// Runs the reference image until `done` accepts the console, the machine stops, or the budget ends.
fn run(config: SocConfig, done: impl Fn(&str) -> bool) -> Run {
    run_image(reference_flash(), config, |console, _| done(console))
}

/// Runs `image` until `done` accepts the console and the diagnostics seen so far, the machine
/// stops, or the budget ends.
fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic]) -> bool) -> Run {
    let mut machine = Esp32C3::with_flash(rom(), image, &config).unwrap();
    let mut consoles = Consoles::default();
    let bounds = Bounds {
        slice: SLICE,
        max_cycles: BUDGET,
        max_nanos: DEVICE_TIME_BUDGET_NS,
    };
    let driven = drive(&mut machine, bounds, &mut consoles, |_, _, seen| {
        done(&seen.text(ConsoleChannel::Uart0), &seen.diagnostics)
    });
    assert_eq!(
        machine.flash_writes_dropped(),
        0,
        "the journal was taken at every slice: it never filled"
    );
    assert!(
        machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS || driven == Driven::Done,
        "device time ran out ({} ns) after {} CPU cycles before the run was done: the CPU sleeps and nothing wakes it?\nconsole:\n{}",
        machine.device_time_nanos(),
        machine.cycles().get(),
        consoles.text(ConsoleChannel::Uart0)
    );
    // The run is over: what the firmware wrote and the UART had not sent yet belongs to its console.
    let unsent = machine.flush_console(ConsoleChannel::Uart0);
    let mut console = consoles.bytes(ConsoleChannel::Uart0).to_vec();
    console.extend(unsent);
    Run {
        console: String::from_utf8_lossy(&console).replace('\r', ""),
        diagnostics: consoles.diagnostics,
        flash_writes: consoles.flash_writes,
        flash: machine.flash_contents(),
        flash_modified_sectors: machine.flash_modified_sectors(),
        flash_commands: (0..=u8::MAX)
            .map(|opcode| machine.flash_commands_sent(opcode))
            .collect(),
        driven,
        trace_hash: machine.trace_hash(),
        resets: machine.resets(),
        cycles: machine.cycles().get(),
        device_time_nanos: machine.device_time_nanos(),
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

/// Milestone point 1 of plan SP1b-3a: the application lives its whole life, restarts, and the
/// chip boots a second time.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn the_whole_life_cycle_prints_the_golden_log_and_boots_a_second_time() {
    let life = run(SocConfig::default(), |console| {
        application_starts(console) == 2
    });
    assert!(
        life.console.starts_with(ROM_LINES),
        "ROM part:\n{}",
        life.console
    );
    let after_rom: Vec<String> = life.console[ROM_LINES.len()..]
        .lines()
        .map(mask_timestamp)
        .collect();
    let mut expected: Vec<&str> = BOOTLOADER_LINES.to_vec();
    expected.extend(APPLICATION_LINES);
    expected.extend(SECOND_BOOT_ROM_LINES);
    expected.extend(BOOTLOADER_LINES);
    expected.push(FIRST_APPLICATION_LINE);
    assert_eq!(
        after_rom
            .iter()
            .map(String::as_str)
            .take(expected.len())
            .collect::<Vec<_>>(),
        expected,
        "diagnostics: {:#?}",
        life.diagnostics
    );
    assert_eq!(life.resets, 1, "one reset: the restart");
    assert_only_the_restart_is_reported(&life.diagnostics);
    assert_eq!(
        SECOND_BOOT_ROM_LINES[2],
        format!("Saved PC:{RESTART_PC:#010x}"),
        "the journal and the ROM's own line name the same instruction"
    );
}

/// Milestone point 3: in strict mode a whole life cycle, from power-on through `esp_restart` to
/// the first line of the second boot, touches no unmodelled register and takes no CPU exception.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn a_strict_run_crosses_the_restart_without_anything_unmodelled() {
    let strict = SocConfig {
        strict_mmio: true,
        ..SocConfig::default()
    };
    let life = run(strict, |console| application_starts(console) == 2);
    assert!(
        !matches!(life.driven, Driven::Stopped(_)),
        "nothing stops the run: {:?}\nconsole:\n{}",
        life.driven,
        life.console
    );
    // The restart is an entry of the journal, not a violation: strict mode lets it pass and
    // nothing else was reported.
    assert_only_the_restart_is_reported(&life.diagnostics);
    assert_eq!(application_starts(&life.console), 2, "{}", life.console);
    assert_eq!(life.resets, 1);
}

/// Milestone point 2: the eleven seconds the application spends in `vTaskDelay` are slept, not
/// executed. The lines of its countdown are one second of device time apart, and the CPU runs
/// through its first life in a fraction of the cycles eleven seconds would take at 160 MHz
/// (1 760 000 000).
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn the_countdown_lines_are_one_device_second_apart_and_the_cpu_sleeps_between_them() {
    /// CPU cycles one call to `run_until` executes before it returns, and the device time they
    /// take at the 160 MHz the application runs at (6.25 ns a cycle).
    ///
    /// The resolution of the times read below: while the CPU sleeps a call does not return, UART
    /// bytes leaving or not (a wake-up without an interrupt is not a reason to return). It
    /// returns `LOOK` cycles after the next tick interrupt wakes the CPU, so every look is a tick
    /// instant (a multiple of 10 ms) plus `LOOK_NS`, and the time of the look is the time the
    /// line was complete rounded up to the next tick, not the time it was printed. The gap between
    /// two looks is therefore a whole number of ticks within the difference of two such offsets,
    /// which is under `LOOK_NS`: measured, the nine gaps are 1 s - 1.08 us to 1 s + 0.26 us. One
    /// tick of error in the application's delay is 10 ms, four orders of magnitude more.
    const LOOK: u64 = 200;
    const LOOK_NS: u64 = 1_250;
    /// One tick of the application's scheduler (`CONFIG_FREERTOS_HZ` is 100).
    const TICK_NS: u64 = SECOND_NS / 100;
    let mut machine = Esp32C3::with_flash(rom(), reference_flash(), &SocConfig::default()).unwrap();
    let mut consoles = Consoles::default();
    // Device time, read from the machine, at the first look that found each countdown line
    // printed.
    let mut seen_at: Vec<u64> = Vec::new();
    let bounds = Bounds {
        slice: LOOK,
        max_cycles: BUDGET,
        max_nanos: DEVICE_TIME_BUDGET_NS,
    };
    let driven = drive(
        &mut machine,
        bounds,
        &mut consoles,
        |machine, taken, seen| {
            if taken[0].1.is_empty() {
                return false;
            }
            let console = seen.text(ConsoleChannel::Uart0);
            if console.matches("seconds...\n").count() > seen_at.len() {
                seen_at.push(machine.device_time_nanos());
            }
            console.contains("Restarting now.")
        },
    );
    assert_eq!(driven, Driven::Done);
    assert_eq!(
        seen_at.len(),
        11,
        "{}",
        consoles.text(ConsoleChannel::Uart0)
    );
    // `vTaskDelay(1000 / portTICK_PERIOD_MS)` waits 100 ticks counted from the next tick edge, so
    // the first delay, which starts inside a tick, lasts one second less the part of that tick
    // already spent: 0 to 10 ms shorter than a second, never longer. Which tick the first line is
    // seen at depends on that phase and also on the time the UART takes to send the lines before
    // it (a look is the first tick after the line is complete), so the first gap is bounded by
    // the tick on one side and by a second on the other, each with the resolution of the looks
    // (here 0.99 s).
    let first = seen_at[1] - seen_at[0];
    assert!(
        (SECOND_NS - TICK_NS - LOOK_NS..=SECOND_NS + LOOK_NS).contains(&first),
        "the first gap is {first} ns: {seen_at:?}"
    );
    // The nine others start on the tick edge the previous delay ended on: a second exactly, to
    // the resolution of the looks.
    for pair in seen_at[1..].windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap.abs_diff(SECOND_NS) <= LOOK_NS,
            "two countdown lines are {gap} ns apart: {seen_at:?}"
        );
    }
    assert!(
        machine.device_time_nanos() > 11 * SECOND_NS,
        "{} ns",
        machine.device_time_nanos()
    );
    assert!(
        machine.cycles().get() < 60_000_000,
        "the CPU executed {} cycles: it did not sleep",
        machine.cycles().get()
    );
}

/// The reference image with the stock partition table of the X4 in place of its own.
fn reference_image_behind_the_stock_x4_table() -> Vec<u8> {
    let mut image = reference_flash();
    image[PARTITION_TABLE_OFFSET..PARTITION_TABLE_OFFSET + X4_STOCK_TABLE.len()]
        .copy_from_slice(X4_STOCK_TABLE);
    image
}

/// The `otadata` entry the bootloader writes on the first boot of an OTA layout (report FW §E,
/// first row): sequence 1, a blank label, state valid (2), CRC.
fn the_otadata_entry() -> Vec<u8> {
    let mut entry = vec![0xFF; 32];
    entry[..4].copy_from_slice(&[1, 0, 0, 0]);
    entry[24..28].copy_from_slice(&[2, 0, 0, 0]);
    entry[28..].copy_from_slice(&[0x9A, 0x98, 0x43, 0x47]);
    entry
}

/// What the first boot of an OTA layout with a blank `otadata` writes (report FW §E, first row;
/// SF §B.1): one sector erase, then one program of the entry.
fn the_otadata_update() -> [FlashWrite; 2] {
    [
        FlashWrite::Erase {
            start: OTADATA_OFFSET,
            len: 0x1000,
        },
        FlashWrite::Program {
            address: OTADATA_OFFSET,
            data: the_otadata_entry(),
        },
    ]
}

/// Asserts that the first `otadata` sector of `flash`, the content of the chip when a run ended,
/// holds the entry of [`the_otadata_entry`] and erased bytes after it: what the chip holds, not
/// what it was asked to do.
fn assert_otadata_holds_the_entry_and_nothing_else(flash: &[u8]) {
    let entry = the_otadata_entry();
    let sector = OTADATA_OFFSET as usize;
    assert_eq!(
        flash[sector..sector + entry.len()],
        entry[..],
        "the entry, as measured (FW §E)"
    );
    let rest = &flash[sector + entry.len()..sector + 0x1000];
    assert_eq!(
        rest.iter().position(|&byte| byte != 0xFF),
        None,
        "the rest of the sector is erased"
    );
}

/// Asserts that `expected` are lines of `console`, in that order.
fn assert_lines_in_order(console: &str, expected: &[&str]) {
    let lines: Vec<String> = console.lines().map(mask_timestamp).collect();
    let mut from = 0;
    for expected in expected {
        let at = lines[from..]
            .iter()
            .position(|line| line == expected)
            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{lines:#?}"));
        from += at + 1;
    }
}

/// How many lines of `console` are `line`, timestamps masked.
fn count_lines(console: &str, line: &str) -> usize {
    console
        .lines()
        .map(mask_timestamp)
        .filter(|found| found == line)
        .count()
}

const NO_FACTORY_IMAGE: &str = "I (#) boot: No factory image, trying OTA 0";
const OTADATA_SET: &str = "I (#) boot: Set actual ota_seq=1 in otadata[0]";

/// Spec §5.1: "Reset → ROM → bootloader → table de partitions → `otadata` → application". The
/// reference image has a factory application; the target device has none. Behind the stock X4
/// table the unmodified bootloader finds a blank `otadata`, falls back to OTA slot 0 and records
/// that choice with the mask ROM's sector erase and page program. The chip takes both: the
/// operations are counted, the bytes are the measured ones, and the second boot, which reads
/// `otadata` through the cache window, finds the entry valid and writes nothing.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn the_stock_x4_ota_layout_boots_and_writes_otadata_on_its_first_boot_only() {
    let life = run_image(
        reference_image_behind_the_stock_x4_table(),
        SocConfig::default(),
        |console, _| application_starts(console) == 2,
    );
    assert!(
        life.console.starts_with(ROM_LINES),
        "ROM part:\n{}",
        life.console
    );
    assert_lines_in_order(
        &life.console,
        &[
            NO_FACTORY_IMAGE,
            "I (#) boot: Loaded app from partition at offset 0x10000",
            OTADATA_SET,
            FIRST_APPLICATION_LINE,
            "Hello world!",
            "Restarting now.",
            "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
            "I (#) boot: Loaded app from partition at offset 0x10000",
            FIRST_APPLICATION_LINE,
        ],
    );
    // SF §B.3: both lines are printed on the first boot only. A second boot that printed them
    // again would not have read back what the first one wrote.
    assert_eq!(count_lines(&life.console, NO_FACTORY_IMAGE), 1);
    assert_eq!(count_lines(&life.console, OTADATA_SET), 1);
    assert_eq!(
        life.flash_writes,
        the_otadata_update(),
        "two boots, one erase and one program: the update of the first boot"
    );
    assert_otadata_holds_the_entry_and_nothing_else(&life.flash);
    assert_eq!(life.flash_modified_sectors, 1, "the otadata sector");
    assert_only_the_restart_is_reported(&life.diagnostics);
    assert!(
        !matches!(life.driven, Driven::Stopped(_)),
        "nothing stops a lenient run: {:?}",
        life.driven
    );
}

/// The same layout in strict mode: the `otadata` update is nothing the model refuses any more.
/// The run crosses it without a stop and without a single report, and the application starts.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn a_strict_run_of_the_stock_x4_ota_layout_writes_otadata_and_reaches_the_application() {
    let strict = SocConfig {
        strict_mmio: true,
        ..SocConfig::default()
    };
    let boot = run_image(
        reference_image_behind_the_stock_x4_table(),
        strict,
        |console, _| console.contains("Hello world!\n"),
    );
    assert!(
        !matches!(boot.driven, Driven::Stopped(_)),
        "a strict run of this layout does not stop: {:?}\nconsole:\n{}",
        boot.driven,
        boot.console
    );
    assert_eq!(boot.diagnostics, [], "nothing unmodelled, no exception");
    assert_lines_in_order(
        &boot.console,
        &[
            NO_FACTORY_IMAGE,
            OTADATA_SET,
            FIRST_APPLICATION_LINE,
            "Hello world!",
        ],
    );
    assert_eq!(boot.flash_writes, the_otadata_update());
    assert_otadata_holds_the_entry_and_nothing_else(&boot.flash);
    assert_eq!(boot.resets, 0);
}

/// The same layout with an `otadata` sector that is not blank. Its first entry still reads
/// "never written" (a sequence number of `FFFFFFFF`), so the bootloader takes the same path; the
/// rest of the sector holds zeroes, which only an erase removes. Behind a blank sector the erase
/// of the bootloader changes nothing and nothing here could tell whether it took place; behind
/// this one, an erase that erased nothing would leave the entry AND-ed into the zeroes.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn the_bootloader_erases_an_otadata_sector_that_is_not_blank_before_it_programs_the_entry() {
    let mut image = reference_image_behind_the_stock_x4_table();
    let sector = OTADATA_OFFSET as usize;
    image[sector + 4..sector + 0x1000].fill(0x00);
    let life = run_image(image, SocConfig::default(), |console, _| {
        application_starts(console) == 2
    });
    assert_eq!(
        count_lines(&life.console, NO_FACTORY_IMAGE),
        1,
        "the first boot finds no valid entry, the second finds the one the first wrote:\n{}",
        life.console
    );
    assert_eq!(count_lines(&life.console, OTADATA_SET), 1);
    assert_eq!(life.flash_writes, the_otadata_update());
    assert_otadata_holds_the_entry_and_nothing_else(&life.flash);
    assert_eq!(life.flash_modified_sectors, 1, "the otadata sector");
    assert_only_the_restart_is_reported(&life.diagnostics);
}

/// Milestone point 6: two runs of a whole life cycle, the reset included, are the same run.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn two_life_cycles_are_identical_down_to_the_trace_hash_across_the_reset() {
    let config = SocConfig {
        trace_hash: true,
        ..SocConfig::default()
    };
    let life = || run(config, |console| application_starts(console) == 2);
    let (first, second) = (life(), life());
    // Two runs that both exhausted the budget before the second boot would compare equal too,
    // and so would two runs a watchdog reset: the reset crossed is the one `esp_restart` asks
    // for, and the second boot went as far as the first line of the application.
    assert_eq!(application_starts(&first.console), 2, "{}", first.console);
    assert_eq!(first.resets, 1);
    let (_, second_boot) = first
        .console
        .split_once("rst:0xc (RTC_SW_CPU_RST)")
        .unwrap_or_else(|| panic!("no software CPU reset in:\n{}", first.console));
    assert_eq!(application_starts(second_boot), 1, "{}", first.console);
    assert!(first.trace_hash.is_some());
    assert_eq!(
        (first.trace_hash, first.cycles, first.device_time_nanos),
        (second.trace_hash, second.cycles, second.device_time_nanos),
        "spec §8: same session, same trace hash"
    );
    assert_eq!(first.console, second.console);
}

// ---------------------------------------------------------------------------------------------
// The second reference firmware: ESP-IDF's `examples/storage/nvs/nvs_rw_value_cxx`, unmodified.
// Each of its lives reads a restart counter from NVS, writes it back incremented, counts down
// for eleven seconds and restarts (report SF §A.1). Only the second life proves that a write was
// kept: the first prints the same lines whether the flash took the bytes or not.
// ---------------------------------------------------------------------------------------------

/// `flash_nvs.bin`: the example behind its own single-app partition table (SF §A.3).
const NVS_SHA256: &str = "c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f";
/// `flash_nvs_ota.bin`: the same application behind the stock partition table of the X4.
const NVS_OTA_SHA256: &str = "87832c6d5d15672fb3892bfad281688fc7486b360a25b495bdb6a27d04f28fec";
/// The whole flash after each life of `flash_nvs.bin`, measured under QEMU and replayed (SF §B.2).
const NVS_AFTER_LIFE_1: &str = "cfab4ffe5187eeea8ec2ee6e9794886289282f0242ce56aad2842144a6ffb17b";
const NVS_AFTER_LIFE_2: &str = "7ad0acf62e5a4e20aa6d47be0f1d922a7b89cb204941feac1ab2fea71363b709";
const NVS_AFTER_LIFE_3: &str = "fc23c7ecb700e966c0ef9db3becc6b73e37324b5a21e5380519fe597542143f5";
/// The whole flash after three lives of `flash_nvs_ota.bin` (SF §B.2).
const NVS_OTA_AFTER_LIFE_3: &str =
    "6c668205970fc21e7330b840b44e47ca1a2a245f22d93ef1cb0f239a3b89de5b";

fn nvs_flash() -> Vec<u8> {
    pinned_image("INKEMU_NVS_FLASH", "flash_nvs.bin", NVS_SHA256)
}

fn nvs_ota_flash() -> Vec<u8> {
    pinned_image("INKEMU_NVS_OTA_FLASH", "flash_nvs_ota.bin", NVS_OTA_SHA256)
}

/// The configuration of every run of the example: nothing unmodelled is let through.
fn strict() -> SocConfig {
    SocConfig {
        strict_mmio: true,
        ..SocConfig::default()
    }
}

/// The last line of a life of the example: the restart follows.
const RESTARTING_NOW: &str = "Restarting now.\n";

/// Ends a run when the example has printed the last line of its `lives`-th life. Its writes are
/// done by then: the last one is eleven seconds before the restart (SF §B.2).
fn lives(lives: usize) -> impl Fn(&str, &[Diagnostic]) -> bool {
    move |console, _| console.matches(RESTARTING_NOW).count() >= lives
}

fn sha256(bytes: &[u8]) -> String {
    common::hex(&Sha256::digest(bytes))
}

fn program(address: u32, data: &[u8]) -> FlashWrite {
    FlashWrite::Program {
        address,
        data: data.to_vec(),
    }
}

/// The NVS entry that holds `restart_counter = value` (SF §B.2; `IDF:nvs_flash/src/nvs_page.cpp`:
/// namespace 1, type i32, span 1, chunk index `FF`, CRC, the key in 16 bytes, the value in 8).
fn counter_entry(crc: [u8; 4], value: u8) -> Vec<u8> {
    let mut entry = vec![0x01, 0x14, 0x01, 0xFF];
    entry.extend(crc);
    entry.extend(b"restart_counter\0");
    entry.extend([value, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
    entry
}

/// Where NVS keeps the state of the 126 entries of its first page, two bits each: the word the
/// example programs six times in three lives, with no erase in between (SF §B.2).
///
/// What the tests here tell of it: that it was programmed with the measured values in the
/// measured order, that nothing erased it, and that it ends as measured. What they cannot tell:
/// that a program stores the AND of the byte it is sent with the byte the cell holds. The six
/// values, `FE FA EA E2 A2 82`, each only clear bits of the one before, and every cell starts
/// at `FF`: a chip that stored the bytes as sent would leave the same word, the same journal
/// and the same hashes, and every test of this file would pass on it. The rule is held by unit
/// tests, which program bytes that would set bits again:
/// `flash::tests::a_program_only_clears_bits`,
/// `flash::tests::a_long_random_sequence_of_writes_leaves_what_the_plain_chip_holds_and_counts`
/// and
/// `machine::tests::the_content_of_the_flash_leaves_the_machine_whole_and_builds_the_same_flash_again`.
const ENTRY_STATES: u32 = 0x9020;

/// Every program of the three lives of the example on an erased NVS partition, in order: the
/// eleven rows of SF §B.2, measured under QEMU. Five on the first life (the page header, the
/// namespace entry, the counter), three on each life after (the new counter, then the old one
/// marked erased).
fn the_programs_of_the_three_lives() -> [Vec<FlashWrite>; 3] {
    let mut page_header = vec![0xFE, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0xFE];
    page_header.extend([0xFF; 19]);
    page_header.extend([0x84, 0x2D, 0xBA, 0xB9]);
    let mut namespace = vec![0x00, 0x01, 0x01, 0xFF, 0x09, 0xA9, 0x50, 0x07];
    namespace.extend(b"storage");
    namespace.extend([0; 9]);
    namespace.push(0x01);
    namespace.extend([0xFF; 7]);
    let states = |low: u8| program(ENTRY_STATES, &[low, 0xFF, 0xFF, 0xFF]);
    [
        vec![
            program(0x9000, &page_header),
            program(0x9040, &namespace),
            states(0xFE),
            program(0x9060, &counter_entry([0x81, 0xA8, 0xFF, 0x42], 1)),
            states(0xFA),
        ],
        vec![
            program(0x9080, &counter_entry([0x62, 0xAF, 0x70, 0xCC], 2)),
            states(0xEA),
            states(0xE2),
        ],
        vec![
            program(0x90A0, &counter_entry([0xFC, 0xAF, 0xDA, 0x00], 3)),
            states(0xA2),
            states(0x82),
        ],
    ]
}

/// What the first life of the example says of the counter: there is none yet.
const COUNTER_NOT_INITIALIZED: &str =
    "Reading restart counter from NVS ... The value is not initialized yet!";

/// The lines the application prints in one life, in order (SF §B.3): it opens NVS, says what it
/// read of the counter (`reading`), writes the counter back, counts down eleven lines from 10 to
/// 0 and restarts.
fn the_lines_of_a_life(reading: &[&str]) -> Vec<String> {
    let mut lines = vec!["Opening Non-Volatile Storage (NVS) handle... Done".to_owned()];
    lines.extend(reading.iter().map(|&line| line.to_owned()));
    lines.push("Updating restart counter in NVS ... Done".to_owned());
    lines.push("Committing updates in NVS ... Done".to_owned());
    lines.extend(
        (0..=10)
            .rev()
            .map(|seconds| format!("Restarting in {seconds} seconds...")),
    );
    lines.push(RESTARTING_NOW.trim_end().to_owned());
    lines
}

/// Asserts that `console` holds, in order, the lines of three lives of the example (SF §B.3,
/// §G): a power-on whose life finds no counter, then two restarts whose lives read the counter
/// the life before wrote, each life with every line of the first. `first_boot` are lines of the
/// bootloader of the first boot, between the ROM banner and the application.
fn assert_the_console_of_three_lives(console: &str, first_boot: &[&str]) {
    let mut lines = vec!["rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)".to_owned()];
    lines.extend(first_boot.iter().map(|&line| line.to_owned()));
    lines.extend(the_lines_of_a_life(&[COUNTER_NOT_INITIALIZED]));
    for counter in 1..=2 {
        lines.push("rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)".to_owned());
        lines.extend(the_lines_of_a_life(&[
            "Reading restart counter from NVS ... Done",
            &format!("Restart counter = {counter}"),
        ]));
    }
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert_lines_in_order(console, &lines);
}

/// What the journal of a strict run of the example may hold: the restart it asks for, and
/// nothing else. No unmodelled register, no refused flash command, no window read that may
/// differ from the device, and no CPU exception.
///
/// The restart is one entry however many lives the run crossed: the journal lists a reset once
/// per cause and PC. A run that ends on the last line of a life may or may not have reached the
/// restart that follows it, a few milliseconds later: no entry at all is accepted too.
fn assert_nothing_but_the_restart_is_reported(diagnostics: &[Diagnostic]) {
    assert!(diagnostics.len() <= 1, "{diagnostics:#?}");
    for diagnostic in diagnostics {
        assert!(
            matches!(
                diagnostic,
                Diagnostic::Reset {
                    cause: ResetCause::SoftwareCpu,
                    pc: RESTART_PC,
                    ..
                }
            ),
            "{diagnostic}"
        );
    }
}

/// The milestone of the flash writes: three lives of the example in strict mode. The counter the
/// second and the third life print is the one the life before wrote; the programs are the
/// measured ones, byte for byte and in order; the flash ends as the measured image.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_FLASH"]
fn the_nvs_example_counts_its_restarts_over_three_lives_and_programs_the_measured_bytes() {
    let run = run_image(nvs_flash(), strict(), lives(3));
    assert!(
        !matches!(run.driven, Driven::Stopped(_)),
        "a strict run of the example does not stop: {:?}\nconsole:\n{}",
        run.driven,
        run.console
    );
    assert_the_console_of_three_lives(&run.console, &[]);
    assert_eq!(
        count_lines(&run.console, COUNTER_NOT_INITIALIZED),
        1,
        "only the first life finds no counter"
    );
    assert_eq!(run.flash_writes, the_programs_of_the_three_lives().concat());
    assert_eq!(sha256(&run.flash), NVS_AFTER_LIFE_3);
    assert_eq!(run.flash_modified_sectors, 1, "the first page of NVS");
    assert_eq!(
        run.flash[ENTRY_STATES as usize], 0x82,
        "the last of the six values programmed there; the journal above holds no erase"
    );
    assert!(run.resets >= 2, "two restarts were crossed: {}", run.resets);
    assert_nothing_but_the_restart_is_reported(&run.diagnostics);
}

/// The same three lives behind the stock partition table of the X4: the bootloader writes
/// `otadata` on the first boot, and on that boot only; the application then writes the same NVS
/// bytes at the same addresses.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_OTA_FLASH"]
fn the_nvs_example_behind_the_stock_x4_table_writes_otadata_once_and_counts_its_restarts() {
    let run = run_image(nvs_ota_flash(), strict(), lives(3));
    assert!(
        !matches!(run.driven, Driven::Stopped(_)),
        "a strict run of the example does not stop: {:?}\nconsole:\n{}",
        run.driven,
        run.console
    );
    assert_the_console_of_three_lives(&run.console, &[NO_FACTORY_IMAGE, OTADATA_SET]);
    assert_eq!(count_lines(&run.console, COUNTER_NOT_INITIALIZED), 1);
    assert_eq!(count_lines(&run.console, NO_FACTORY_IMAGE), 1);
    assert_eq!(count_lines(&run.console, OTADATA_SET), 1);
    let mut expected = the_otadata_update().to_vec();
    expected.extend(the_programs_of_the_three_lives().concat());
    assert_eq!(run.flash_writes, expected);
    assert_eq!(sha256(&run.flash), NVS_OTA_AFTER_LIFE_3);
    assert_eq!(run.flash_modified_sectors, 2, "NVS and otadata");
    assert_nothing_but_the_restart_is_reported(&run.diagnostics);
}

/// The flash a first life leaves is the measured one, and a machine built from it goes on
/// counting: a power-on, not a restart, and the counter of the life before. This is what a
/// flash file kept between two runs relies on.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_FLASH"]
fn a_cold_start_from_the_flash_the_first_life_left_goes_on_counting() {
    let [first_life, second_life, _] = the_programs_of_the_three_lives();
    let first = run_image(nvs_flash(), strict(), lives(1));
    assert_eq!(first.flash_writes, first_life);
    assert_eq!(sha256(&first.flash), NVS_AFTER_LIFE_1);
    assert_nothing_but_the_restart_is_reported(&first.diagnostics);

    let second = run_image(first.flash, strict(), lives(1));
    // The ROM lines of the reference image: the bootloader is the same binary.
    assert!(
        second.console.starts_with(ROM_LINES),
        "a power-on:\n{}",
        second.console
    );
    assert_lines_in_order(
        &second.console,
        &[
            "Reading restart counter from NVS ... Done",
            "Restart counter = 1",
            "Restarting now.",
        ],
    );
    assert_eq!(count_lines(&second.console, COUNTER_NOT_INITIALIZED), 0);
    assert_eq!(second.flash_writes, second_life);
    assert_eq!(sha256(&second.flash), NVS_AFTER_LIFE_2);
    assert_nothing_but_the_restart_is_reported(&second.diagnostics);
}

/// Determinism with a flash that is written: two runs from the same content are the same run,
/// down to the trace hash, and leave the same flash.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_OTA_FLASH"]
fn three_lives_of_the_nvs_example_are_the_same_run_twice_and_leave_the_same_flash() {
    let config = SocConfig {
        trace_hash: true,
        ..strict()
    };
    let run = || run_image(nvs_ota_flash(), config, lives(3));
    let (first, second) = (run(), run());
    assert_eq!(first.console.matches(RESTARTING_NOW).count(), 3);
    assert!(first.trace_hash.is_some());
    assert_eq!(
        (first.trace_hash, first.cycles, first.device_time_nanos),
        (second.trace_hash, second.cycles, second.device_time_nanos)
    );
    assert_eq!(first.console, second.console);
    assert_eq!(first.flash_writes, second.flash_writes);
    assert_eq!(sha256(&first.flash), sha256(&second.flash));
    assert_eq!(sha256(&first.flash), NVS_OTA_AFTER_LIFE_3);
}

/// The flash commands of a first boot, by phase, against the list measured under QEMU (report FW
/// §A.4, SF §B.1): the bootloader's `otadata` update is two write enables, one sector erase and
/// one program; the application's five programs are five write enables and five programs.
///
/// QEMU's chip keeps its write-enable latch set after a program, so ESP-IDF sends it a write
/// disable (`04`) after each one: 142 of them in FW §A.4. A chip clears the latch when the
/// operation completes, and so does the model: the application sends none here. Status reads
/// (`05`) are not counted: how many there are depends on how fast the chip answers.
///
/// Why a count of `04` is asserted here, when the numbers of `05` and `04` measured under QEMU
/// are properties of QEMU's chip (SF §G assertion 1, §O question 1) and must not be pinned. The
/// number asserted is not one QEMU gave. It is zero, in the application phase alone: a
/// difference of two counts, which leaves out the write disable the bootloader sends whatever
/// the chip does (BA §2.2). And it follows from the code of ESP-IDF, not from a capture: its
/// driver sends a write disable after a program only when the chip, idle again, still shows the
/// latch set (`IDF:spi_flash/spi_flash_chip_generic.c:462-468`; FW key result 5). A chip whose
/// latch clears when the operation completes is sent none, however long its operations last and
/// however many status reads they take. That the latch clears is what the datasheet states (FW
/// §C, row "WEL cleared") and what ESP-IDF requires of a chip; it is read, not measured on a
/// device. This test is the only one on a real firmware that fails when the latch stays set:
/// the bytes written, the journal and the console are the same on both chips.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_OTA_FLASH"]
fn a_first_boot_sends_one_write_enable_per_write_and_the_application_no_write_disable() {
    const WRITE_ENABLE: usize = 0x06;
    const WRITE_DISABLE: usize = 0x04;
    const PROGRAM: usize = 0x02;
    const SECTOR_ERASE: usize = 0x20;
    const DUAL_IO_READ: usize = 0xBB;
    /// Erases of a block or of the chip, and the status write: none is sent by this firmware.
    const NEVER_SENT: [usize; 5] = [0xD8, 0x52, 0xC7, 0x60, 0x01];
    // The machine is deterministic: the first run is the beginning of the second.
    let bootloader = run_image(nvs_ota_flash(), strict(), |console, _| {
        console.contains(FIRST_APPLICATION_LINE.trim_start_matches("I (#) "))
    });
    let life = run_image(nvs_ota_flash(), strict(), lives(1));
    let (before, after) = (&bootloader.flash_commands, &life.flash_commands);
    assert_eq!(bootloader.flash_writes, the_otadata_update());
    assert_eq!(
        [before[WRITE_ENABLE], before[SECTOR_ERASE], before[PROGRAM]],
        [2, 1, 1],
        "ROM and bootloader: the otadata update"
    );
    let application = |opcode: usize| after[opcode] - before[opcode];
    assert_eq!(
        [
            application(WRITE_ENABLE),
            application(PROGRAM),
            application(SECTOR_ERASE),
            application(WRITE_DISABLE),
        ],
        [5, 5, 0, 0],
        "application: five times a write enable and a program, and no write disable"
    );
    assert!(application(DUAL_IO_READ) > 0, "it reads NVS through SPI1");
    for opcode in NEVER_SENT {
        assert_eq!(after[opcode], 0, "{opcode:#04x}");
    }
}
