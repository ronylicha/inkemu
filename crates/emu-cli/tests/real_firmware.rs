//! Two firmwares of real Xteink X4 units through `inkemu run`, as a user starts them: `CrossPoint`
//! 1.6.5 (`crosspoint-1.6.5-x3-x4.bin`, an application image, given to `--firmware`) and the stock
//! firmware 3.0.2 EN of the X4 (a dump of the whole 16 MiB flash of a unit, given to `--flash`).
//! They show criterion 2 of the spec (`docs/superpowers/specs/2026-10-11-inkemu-design.md`,
//! §10.3): the ROM, the bootloader and the application run, the firmware's boot lines reach
//! standard output, and the first accesses to what later sub-projects model (GPIO, I2C0, SPI2, the
//! SAR ADC) are warnings that name their register, never a silent stop.
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
//! byte of the flash.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

use std::process::Output;

/// `crosspoint-1.6.5-x3-x4.bin`, the release asset of `CrossPoint` 1.6.5 for the X3 and the X4
/// (measured 2026-10-14; `docs/superpowers/specs/2026-10-14-sp1b-3e-design.md`, §2).
const CROSSPOINT_X3_X4_SHA256: &str =
    "8eccf614b6d6f82284d3ec775a383fd421ec1880481ff863cf44a831e0385764";
/// The dump of the 16 MiB flash of an X4 running its stock firmware 3.0.2 EN (same section).
const STOCK_X4_FLASH_SHA256: &str =
    "03866a1fc23a6a624469f508da3a747159b2315303b98779fd8161276a989a88";

/// The device time each run is given: the two lives of `CrossPoint`, and several of the stock
/// firmware, end before it (measured 2026-10-14, release build).
const SECONDS: &str = "5";

/// The panic of the interrupt watchdog of ESP-IDF, the same text in v4.4 and v5.5
/// (`esp_system/panic.c`): both firmwares wait for the SAR ADC with their interrupts masked.
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
/// the console the profile of the X4 names: what a user who types that command sees.
fn run(input: &str, file: &str, more: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_inkemu"))
        .args(["run", "--device", "x4", "--max-time", SECONDS])
        .args(["--rom-dir", &rom_dir(), input, file])
        .args(more)
        .output()
        .unwrap()
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
                    self.console
                )
            });
            from += at + expected.len();
        }
    }

    /// Fails unless a warning names a register of `peripheral`: a firmware that touches a block
    /// the model does not have is told so by name.
    fn assert_named_warning(&self, peripheral: &str) {
        let named = format!(": {peripheral}.");
        assert!(
            self.log
                .lines()
                .any(|line| line.starts_with("warning: unmodeled register ")
                    && line.contains(&named)),
            "no warning names a register of {peripheral}:\n{}",
            self.log
        );
    }

    /// Fails when an access went to an address where no register is documented.
    fn assert_nothing_undocumented(&self) {
        assert!(
            !self.log.contains("no register is documented there"),
            "{}",
            self.log
        );
    }

    /// The first warning of the run, without its prefix: what `--strict-mmio` stops on. A CPU
    /// exception is a warning that does not stop a strict run (it is what the firmware does on
    /// the device), so it is passed over.
    fn first_warning(&self) -> &str {
        self.log
            .lines()
            .filter_map(|line| line.strip_prefix("warning: "))
            .find(|diagnostic| !diagnostic.starts_with("CPU exception: "))
            .unwrap_or_else(|| panic!("no warning:\n{}", self.log))
    }
}

/// The same run under `--strict-mmio`: exit code 3 at the first warning of the lenient run, which
/// names `register` (`PERIPHERAL.NAME`) and its address.
fn assert_strict_stop(input: &str, file: &str, lenient: &Run, register: &str, address: &str) {
    let strict = Run::of(&run(input, file, &["--strict-mmio"]));
    assert_eq!(strict.code, Some(3), "{}", strict.log);
    let stop = strict
        .log
        .lines()
        .find_map(|line| line.strip_prefix("inkemu: stopped by --strict-mmio: "))
        .unwrap_or_else(|| panic!("no strict stop:\n{}", strict.log));
    assert!(
        stop.starts_with("unmodeled register ")
            && stop.contains(&format!(": {register} at {address} ")),
        "the stop names {register} at {address}: {stop}"
    );
    // A strict run is the lenient run up to its first warning: the two runs agree on it.
    assert_eq!(stop, lenient.first_warning(), "{}", lenient.log);
}

/// `CrossPoint` 1.6.5 from its release asset, without a console option: the ROM, the bootloader
/// inkemu ships and the application; its own log on USB-Serial-JTAG, the console of the X4
/// profile; the board identified as an X4; the wait for the SAR ADC with interrupts masked, which
/// the interrupt watchdog ends with a panic and a CPU reset; and a second life, which finds the
/// verdict X4 in NVS, skips the I2C probe and reaches the SD card and the panel before the same
/// wait (`docs/superpowers/specs/2026-10-14-sp1b-3e-design.md`, §2). Under `--strict-mmio` the run
/// stops at the first register a later sub-project models.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_CROSSPOINT_X3_X4 (a file of the user's)"]
fn crosspoint_1_6_5_x3_x4_starts_and_names_what_the_model_lacks() {
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
        // The first life probes the board over I2C, finds nothing, which means X4, and stores the
        // verdict in NVS before the wait for the ADC.
        "[INF] [HW] Xteink probe scores: pass1=0 pass2=0 verdict=0\n",
        INTERRUPT_WATCHDOG_PANIC,
        "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        // The second life reads the verdict back: the NVS write survived the CPU reset.
        "[INF] [HW] Using cached device type: X4\n",
        "[INF] [MAIN] Hardware detect: X4\n",
        "[SD] SD card not detected (",
        "Wait complete:  CMD_SOFT_RESET (0 ms)\n",
    ]);
    assert!(
        lenient
            .log
            .contains("\ninkemu: reset by the firmware, a CPU reset (rst:0xc) at pc "),
        "{}",
        lenient.log
    );
    for peripheral in ["GPIO", "I2C0", "SPI2", "APB_SARADC"] {
        lenient.assert_named_warning(peripheral);
    }
    lenient.assert_nothing_undocumented();
    assert_strict_stop(
        "--firmware",
        &file,
        &lenient,
        "APB_SARADC.APB_TSENS_CTRL",
        "0x60040058",
    );
}

/// The stock firmware of the X4 from a dump of a unit, without a console option: its bootloader
/// (silent), its application (ESP-IDF v4.4.7), the same wait for the SAR ADC, a panic of the
/// interrupt watchdog and a core reset (`rst:0x3`), again and again. Under `--strict-mmio` the run
/// stops at the pad of GPIO13, the power latch of the board, which the application sets up before
/// it touches any other block inkemu lacks.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_STOCK_X4_FLASH (a file of the user's)"]
fn the_stock_x4_firmware_starts_and_names_what_the_model_lacks() {
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
        INTERRUPT_WATCHDOG_PANIC,
        "rst:0x3 (RTC_SW_SYS_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        INTERRUPT_WATCHDOG_PANIC,
    ]);
    assert!(
        lenient
            .log
            .contains("\ninkemu: reset by the firmware, a core reset (rst:0x3) at pc "),
        "{}",
        lenient.log
    );
    for peripheral in ["GPIO", "APB_SARADC"] {
        lenient.assert_named_warning(peripheral);
    }
    // ESP-IDF v4.4 polls SPI1 `FSM` (`0x60002054`) before each status read of the flash.
    lenient.assert_nothing_undocumented();
    assert_strict_stop("--flash", &file, &lenient, "GPIO.PIN13", "0x600040a8");
}
