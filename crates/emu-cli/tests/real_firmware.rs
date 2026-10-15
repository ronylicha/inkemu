//! Two firmwares of real Xteink X4 units through `inkemu run`, as a user starts them: `CrossPoint`
//! 1.6.5 (`crosspoint-1.6.5-x3-x4.bin`, an application image, given to `--firmware`) and the stock
//! firmware 3.0.2 EN of the X4 (a dump of the whole 16 MiB flash of a unit, given to `--flash`).
//! They show criterion 2 of the spec (`docs/superpowers/specs/2026-10-11-inkemu-design.md`,
//! §10.3): the ROM, the bootloader and the application run, the firmware's boot lines reach
//! standard output, and what the model still lacks is a warning that names it, never a silent
//! stop. Since sub-project 2b models the GPIO matrix, GP-SPI2, I2C0 and the SAR ADC, `CrossPoint`
//! runs to its idle loop with no warning at all, and the stock firmware reads its buttons and goes
//! back to deep sleep, waiting for the power button that sub-project 2d wires.
//!
//! Neither file is in this repository, is downloaded or is run anywhere but here, on the machine of
//! a user who has it: these tests run nowhere in the CI. Each one reads the path of its file from
//! a variable, checks the SHA-256 pinned below and refuses any other file before anything runs:
//!
//! ```text
//! export INKEMU_ROM_ELF_DIR=/path/to/rom          # inkemu rom fetch --all --rom-dir /path/to/rom
//! export INKEMU_CROSSPOINT_X3_X4=/path/to/crosspoint-1.6.5-x3-x4.bin
//! export INKEMU_STOCK_X4_FLASH=/path/to/x4-stock-3.0.2-en-flash.bin
//! cargo test --release -p emu-cli --test real_firmware -- --ignored
//! ```
//!
//! The dump of a unit holds its NVS, where a firmware keeps settings and network credentials: no
//! line here reads the flash but to hash it, and a failure quotes console and log lines, never a
//! byte of the flash. The panic handler of ESP-IDF prints the stack of the faulting task, which may
//! hold what the application read from NVS: a failure leaves those lines out of the console it
//! quotes ([`without_stack_dump`]).
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

use std::process::Output;

/// How long one run may take on the host before the test fails instead of hanging. It turns a
/// hang into a failure and says nothing of speed, so it is wide: a run of 5 s of device time took
/// about 45 s in a release build (measured 2026-10-14), and forty minutes is more than fifty times
/// that.
const HANG_LIMIT: std::time::Duration = std::time::Duration::from_mins(40);

/// `crosspoint-1.6.5-x3-x4.bin`, the release asset of `CrossPoint` 1.6.5 for the X3 and the X4
/// (measured 2026-10-14; `docs/superpowers/specs/2026-10-14-sp1b-3e-design.md`, §2).
const CROSSPOINT_X3_X4_SHA256: &str =
    "8eccf614b6d6f82284d3ec775a383fd421ec1880481ff863cf44a831e0385764";
/// The dump of the 16 MiB flash of an X4 running its stock firmware 3.0.2 EN (same section).
const STOCK_X4_FLASH_SHA256: &str =
    "03866a1fc23a6a624469f508da3a747159b2315303b98779fd8161276a989a88";

/// The device time each run is given: `CrossPoint` reaches its idle loop at 2.3 s, the stock
/// firmware goes to deep sleep at 0.64 s (measured 2026-10-15, release build).
const SECONDS: &str = "5";

/// The panic of the interrupt watchdog of ESP-IDF, the same text in v4.4 and v5.5
/// (`esp_system/panic.c`): both firmwares waited for the SAR ADC with their interrupts masked
/// until sub-project 2b modelled it; the wait now ends and no run panics.
const INTERRUPT_WATCHDOG_PANIC: &str =
    "Guru Meditation Error: Core  0 panic'ed (Interrupt wdt timeout on CPU0).";

/// The path `variable` holds, accepted only when the file there has the SHA-256 `sha256`. A
/// missing variable or file, or another file, fails the test before anything runs.
fn pinned_file(variable: &str, example: &str, sha256: &str) -> String {
    let path = std::env::var(variable).unwrap_or_else(|_| {
        panic!("{variable} is not set; export {variable}={example} (the file is not in this repository)")
    });
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("{variable}={path}: {e}; export {variable}={example}, the path of the file")
    });
    let read = {
        use sha2::Digest as _;
        emu_cli::term::hex(&sha2::Sha256::digest(&bytes))
    };
    assert!(
        read == sha256,
        "{variable}={path} is not the pinned file: its SHA-256 is {read}, {sha256} expected; nothing was run"
    );
    path
}

/// The ROM directory of the run.
fn rom_dir() -> String {
    std::env::var("INKEMU_ROM_ELF_DIR").unwrap_or_else(|_| {
        panic!(
            "INKEMU_ROM_ELF_DIR is not set; fetch the ROMs with `inkemu rom fetch --all --rom-dir <dir>` and export INKEMU_ROM_ELF_DIR=<dir>"
        )
    })
}

/// `inkemu run --device x4 --max-time 5` with `input` (`--firmware` or `--flash`) and `more`, on
/// the console the profile of the X4 names: what a user who types that command sees. A run still
/// going after [`HANG_LIMIT`] is killed and fails the test.
fn run(input: &str, file: &str, more: &[&str]) -> Output {
    let output = assert_cmd::Command::new(env!("CARGO_BIN_EXE_inkemu"))
        .args(["run", "--device", "x4", "--max-time", SECONDS])
        .args(["--rom-dir", &rom_dir(), input, file])
        .args(more)
        .timeout(HANG_LIMIT)
        .output()
        .unwrap();
    assert!(
        output.status.code().is_some(),
        "the run ended on a signal ({}): killed when it did not end within {HANG_LIMIT:?} on the host, or a crash\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Whether `line` is a line of the stack dump the panic handler of ESP-IDF prints: an address of
/// eight hex digits, then `: 0x` and the words there (`3fca8140: 0x00000000 0x3fca81a0 …`).
fn is_stack_dump_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 12
        && bytes[..8].iter().all(u8::is_ascii_hexdigit)
        && bytes[8..].starts_with(b": 0x")
}

/// `console` without the lines of the panic's stack dump, which may show RAM the application
/// filled from the flash, and with a line that says how many were left out.
fn without_stack_dump(console: &str) -> String {
    let mut kept = String::new();
    let mut dropped = 0usize;
    for line in console.lines() {
        if is_stack_dump_line(line) {
            dropped += 1;
        } else {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    if dropped == 0 {
        kept
    } else {
        format!("{kept}({dropped} lines of the panic's stack dump left out)\n")
    }
}

/// What a run printed: its exit code, its console (carriage returns dropped) and its log.
struct Run {
    code: Option<i32>,
    console: String,
    log: String,
}

impl Run {
    fn of(output: &Output) -> Run {
        Run {
            code: output.status.code(),
            console: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
            log: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Fails unless each of `lines` is on the console, in that order.
    fn assert_in_order(&self, lines: &[&str]) {
        let mut from = 0;
        for expected in lines {
            let at = self.console[from..].find(expected).unwrap_or_else(|| {
                panic!(
                    "`{expected}` is missing, or out of order:\n{}",
                    without_stack_dump(&self.console)
                )
            });
            from += at + expected.len();
        }
    }

    /// Fails when the log holds a warning, or the console the panic of the interrupt watchdog.
    fn assert_no_warning_and_no_panic(&self) {
        assert!(
            !self.log.lines().any(|line| line.starts_with("warning: ")),
            "{}",
            self.log
        );
        assert!(
            !self.console.contains(INTERRUPT_WATCHDOG_PANIC),
            "{}",
            without_stack_dump(&self.console)
        );
    }

    /// The warnings of the run, without their prefix.
    fn warnings(&self) -> Vec<&str> {
        self.log
            .lines()
            .filter_map(|line| line.strip_prefix("warning: "))
            .collect()
    }
}

/// `CrossPoint` 1.6.5 from its release asset, without a console option: the ROM, the bootloader
/// inkemu ships and the application; its own log on USB-Serial-JTAG, the console of the X4
/// profile; the I2C probe, whose address bytes get the NACK of an empty bus, so the board is an X4;
/// the SD card and the panel, which nothing answers yet (sub-project 2d attaches them); then the
/// idle loop, with no warning and no panic: the wait for the SAR ADC that the interrupt watchdog
/// ended before sub-project 2b now ends with a reading. Under `--strict-mmio` the run is the same
/// run: nothing stops it.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_CROSSPOINT_X3_X4 (a file of the user's)"]
fn crosspoint_1_6_5_x3_x4_runs_to_its_idle_loop_without_a_warning() {
    let file = pinned_file(
        "INKEMU_CROSSPOINT_X3_X4",
        "/path/to/crosspoint-1.6.5-x3-x4.bin",
        CROSSPOINT_X3_X4_SHA256,
    );
    let lenient = Run::of(&run("--firmware", &file, &[]));
    assert_eq!(lenient.code, Some(0), "{}", lenient.log);
    assert!(
        lenient.log.contains(
            "; console: usb-serial-jtag (the default of the device profile; --console shows another); "
        ),
        "{}",
        lenient.log
    );
    lenient.assert_in_order(&[
        "ESP-ROM:esp32c3-api1-20210207\n",
        "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        ") boot: ESP-IDF v5.5.2 2nd stage bootloader\n",
        // The bootloader's timed scan of the SAR ADC, which feeds the random number generator.
        ") boot: Enabling RNG early entropy source...\n",
        ") boot: Disabling RNG early entropy source...\n",
        // A NACK ends each probe as on an X4: ESP-IDF's driver reports it as an invalid state.
        "i2c_master_transmit_receive failed: [259] ESP_ERR_INVALID_STATE\n",
        "[INF] [HW] Xteink probe scores: pass1=0 pass2=0 verdict=0\n",
        "[INF] [MAIN] Hardware detect: X4\n",
        "[SD] SD card not detected (",
        "Wait complete:  CMD_SOFT_RESET (0 ms)\n",
    ]);
    assert!(
        lenient
            .log
            .contains("\ninkemu: device-time limit reached; "),
        "{}",
        lenient.log
    );
    lenient.assert_no_warning_and_no_panic();
    let strict = Run::of(&run("--firmware", &file, &["--strict-mmio"]));
    assert_eq!(strict.code, Some(0), "{}", strict.log);
    strict.assert_no_warning_and_no_panic();
}

/// The stock firmware of the X4 from a dump of a unit, without a console option: its bootloader
/// (silent) and its application (ESP-IDF v4.4.7), which reads the SAR ADC and the buttons, finds
/// the power button released and goes back to deep sleep with only the GPIO wake enabled, as an
/// X4 does when nothing holds its power button: the run ends there, the one warning naming the
/// wake that sub-project 2d models. Under `--strict-mmio` the run stops on that warning.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_STOCK_X4_FLASH (a file of the user's)"]
fn the_stock_x4_firmware_goes_back_to_deep_sleep_without_its_power_button() {
    const GPIO_WAKE: &str = "the chip went into deep sleep with wake sources inkemu does not model (WAKEUP_ENA 0x00004, pc ";
    let file = pinned_file(
        "INKEMU_STOCK_X4_FLASH",
        "/path/to/x4-stock-3.0.2-en-flash.bin",
        STOCK_X4_FLASH_SHA256,
    );
    let lenient = Run::of(&run("--flash", &file, &[]));
    assert_eq!(lenient.code, Some(0), "{}", lenient.log);
    lenient.assert_in_order(&[
        "ESP-ROM:esp32c3-api1-20210207\n",
        "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
    ]);
    assert!(
        lenient.log.contains(
            "\ninkemu: the device went into deep sleep and nothing inkemu models can wake it, so the run ends here\n"
        ),
        "{}",
        lenient.log
    );
    let warnings = lenient.warnings();
    assert!(
        warnings.len() == 1 && warnings[0].starts_with(GPIO_WAKE),
        "{}",
        lenient.log
    );
    assert!(
        !lenient.console.contains(INTERRUPT_WATCHDOG_PANIC),
        "{}",
        without_stack_dump(&lenient.console)
    );
    let strict = Run::of(&run("--flash", &file, &["--strict-mmio"]));
    assert_eq!(strict.code, Some(3), "{}", strict.log);
    let stop = strict
        .log
        .lines()
        .find_map(|line| line.strip_prefix("inkemu: stopped by --strict-mmio: "))
        .unwrap_or_else(|| panic!("no strict stop:\n{}", strict.log));
    // A strict run is the lenient run up to its first warning: the two runs agree on it.
    assert_eq!(stop, warnings[0], "{}", strict.log);
}

/// A failure quotes the console without the stack dump of the panic, and says how many lines it
/// left out; the lines a test looks for, addresses in a backtrace included, stay.
#[test]
fn a_quoted_console_leaves_out_the_stack_dump_of_the_panic() {
    let console = "\
Guru Meditation Error: Core  0 panic'ed (Interrupt wdt timeout on CPU0).
MEPC    : 0x42001234  RA      : 0x42005678
Stack memory:
3fca8140: 0x00000000 0x3fca81a0 0x42001234 0x00000001
3FCA8150: 0xdeadbeef 0x00000000 0x00000000 0x00000000
Backtrace: 0x42001234:0x3fca8140
3fca8160 is not a dump line
ELF file SHA256: 0000000000000000
";
    assert_eq!(
        without_stack_dump(console),
        "\
Guru Meditation Error: Core  0 panic'ed (Interrupt wdt timeout on CPU0).
MEPC    : 0x42001234  RA      : 0x42005678
Stack memory:
Backtrace: 0x42001234:0x3fca8140
3fca8160 is not a dump line
ELF file SHA256: 0000000000000000
(2 lines of the panic's stack dump left out)
"
    );
    assert_eq!(without_stack_dump("ok\n"), "ok\n");
}
