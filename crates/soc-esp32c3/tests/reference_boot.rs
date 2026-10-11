//! The milestones of plans SP1b-2 and SP1b-3a on the reference firmware (ESP-IDF v5.5.2 bootloader +
//! `hello_world`): the boot, then the whole life of the application, its restart and a second boot.
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_REF_FLASH=<flash_hello.bin> cargo test -p soc-esp32c3 --test reference_boot -- --ignored`
//! Build the image with `tools/reference-firmware/build.sh`.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

mod common;

use emu_core::Cycles;
use sha2::{Digest, Sha256};
use soc_esp32c3::diag::Unsupported;
use soc_esp32c3::memmap::{ROM_BASE, ROM_SIZE, SRAM_DBUS_BASE, SRAM_SIZE};
use soc_esp32c3::reset::ResetCause;
use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, RomImage, SocConfig, StopReason};

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
/// `software_reset_cpu`: the model resets at the end of that instruction (`ASSUMPTION:` in
/// `AssistDebug::cpu_reset`; a real chip shows an application address).
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
    let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
    let mut stop = StopReason::CycleBudget;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
    // `Asleep` is the CPU waiting for its next interrupt: device time goes on, the run too.
    while matches!(stop, StopReason::CycleBudget | StopReason::Asleep)
        && machine.cycles().get() < BUDGET
        && machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS
        && !done(&text(&uart0), &diagnostics)
    {
        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
        diagnostics.extend(machine.drain_diagnostics());
    }
    assert!(
        machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS || done(&text(&uart0), &diagnostics),
        "device time ran out ({} ns) after {} CPU cycles before the run was done: the CPU sleeps and nothing wakes it?\nconsole:\n{}",
        machine.device_time_nanos(),
        machine.cycles().get(),
        text(&uart0)
    );
    // The run is over: what the firmware wrote and the UART had not sent yet belongs to its console.
    uart0.extend(machine.flush_console(ConsoleChannel::Uart0));
    Run {
        console: text(&uart0),
        diagnostics,
        stop,
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
        matches!(life.stop, StopReason::CycleBudget | StopReason::Asleep),
        "nothing stops the run: {:?}\nconsole:\n{}",
        life.stop,
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
    let mut uart0 = Vec::new();
    // Device time, read from the machine, at the first look that found each countdown line
    // printed.
    let mut seen_at: Vec<u64> = Vec::new();
    let mut restarting = false;
    while machine.cycles().get() < BUDGET
        && machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS
        && !restarting
    {
        let stop = machine.run_until(Cycles(machine.cycles().get() + LOOK));
        assert!(
            matches!(stop, StopReason::CycleBudget | StopReason::Asleep),
            "{stop:?}"
        );
        let fresh = machine.take_console(ConsoleChannel::Uart0);
        if fresh.is_empty() {
            continue;
        }
        uart0.extend(fresh);
        let console = String::from_utf8_lossy(&uart0);
        if console.matches("seconds...\r\n").count() > seen_at.len() {
            seen_at.push(machine.device_time_nanos());
        }
        restarting = console.contains("Restarting now.");
    }
    assert_eq!(seen_at.len(), 11, "{}", String::from_utf8_lossy(&uart0));
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

/// Spec §5.1: "Reset → ROM → bootloader → table de partitions → `otadata` → application". The
/// reference image has a factory application; the target device has none. Behind the stock X4
/// table the unmodified bootloader finds a blank `otadata`, falls back to OTA slot 0 and records
/// that choice: the mask ROM's sector-erase and page-program functions run. The flash is still
/// read-only (plan SP1b-3b makes it writable), so both are refused, each once and by its own
/// opcode, and nothing else is reported before the application says hello.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn the_stock_x4_ota_layout_boots_and_only_the_otadata_update_is_refused() {
    let boot = run_image(
        reference_image_behind_the_stock_x4_table(),
        SocConfig::default(),
        |console, _| console.contains("Hello world!\n"),
    );
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
        "Hello world!",
    ] {
        let at = lines[from..]
            .iter()
            .position(|line| line == expected)
            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{lines:#?}"));
        from += at + 1;
    }
    let reported: Vec<(Unsupported, bool)> = boot
        .diagnostics
        .iter()
        .map(|diagnostic| match diagnostic {
            Diagnostic::Unsupported { what, pc } => (*what, MASK_ROM.contains(pc)),
            other => panic!("unexpected diagnostic: {other}"),
        })
        .collect();
    assert_eq!(
        reported,
        [
            (Unsupported::FlashCommand { opcode: 0x20 }, true),
            (Unsupported::FlashCommand { opcode: 0x02 }, true),
        ],
        "exactly the sector erase and the page program of the otadata update, each run by the mask ROM"
    );
    assert!(
        matches!(boot.stop, StopReason::CycleBudget | StopReason::Asleep),
        "nothing stops a lenient run: {:?}",
        boot.stop
    );
}

/// The same layout in strict mode: the run stops on the first thing the model refuses, the sector
/// erase with which the bootloader starts its `otadata` update, and the stop names the operation
/// and the instruction. Nothing was reported before it, and the application did not start.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn a_strict_run_of_the_stock_x4_ota_layout_stops_at_the_first_flash_command_it_refuses() {
    let strict = SocConfig {
        strict_mmio: true,
        ..SocConfig::default()
    };
    let boot = run_image(
        reference_image_behind_the_stock_x4_table(),
        strict,
        |console, _| console.contains("Hello world!\n"),
    );
    let StopReason::StrictMmio(violation) = &boot.stop else {
        panic!(
            "a strict run of this layout must stop, got {:?}\nconsole:\n{}",
            boot.stop, boot.console
        );
    };
    let Diagnostic::Unsupported {
        what: Unsupported::FlashCommand { opcode: 0x20 },
        pc,
    } = violation
    else {
        panic!("the stop is not the sector erase: {violation}");
    };
    assert!(
        MASK_ROM.contains(pc),
        "the erase is run by the mask ROM: pc {pc:#010x}"
    );
    let text = violation.to_string();
    assert!(
        text.contains("flash command 0x20 is not modeled") && text.contains(&format!("{pc:#010x}")),
        "the stop names the operation and the instruction: {text}"
    );
    assert_eq!(
        boot.diagnostics,
        std::slice::from_ref(violation),
        "nothing was reported before the stop, and the stop is in the journal once"
    );
    assert!(
        boot.console
            .lines()
            .map(mask_timestamp)
            .any(|line| line == "I (#) boot: No factory image, trying OTA 0"),
        "the bootloader had chosen the OTA slot:\n{}",
        boot.console
    );
    assert_eq!(
        application_starts(&boot.console),
        0,
        "the run stopped in the bootloader:\n{}",
        boot.console
    );
    assert_eq!(boot.resets, 0);
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
