//! `inkemu run` and `inkemu rom fetch` through the real binary. Nothing here touches the network or the
//! default ROM cache: every run names its ROM directory.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use assert_cmd::Command;
use cpu_rv32::asm::wfi;
use firmware::builder::{ImageSpec, build_flash_image, build_image};
use predicates::prelude::*;
use predicates::str::contains;
use soc_esp32c3::test_support::{HANG, stop_watchdogs, store_register};

fn inkemu() -> Command {
    Command::new(env!("CARGO_BIN_EXE_inkemu"))
}

#[test]
fn unknown_device_exits_2_and_lists_the_known_ones() {
    inkemu()
        .args(["run", "--device", "x9"])
        .assert()
        .code(2)
        .stderr(contains("x9").and(contains("x3")).and(contains("x4")));
}

#[test]
fn unknown_device_name_is_echoed_with_control_characters_escaped() {
    inkemu()
        .args(["run", "--device", "x\u{1b}[31m9"])
        .assert()
        .code(2)
        .stderr(predicate::function(|err: &[u8]| !err.contains(&0x1b)))
        .stderr(contains("x\\u{1b}[31m9"));
}

#[test]
fn empty_rom_dir_exits_2_naming_the_file_and_never_downloads() {
    let dir = tempfile::tempdir().unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--rom-dir"])
        .arg(dir.path())
        .assert()
        .code(2)
        .stderr(contains("esp32c3_rev3_rom.elf").and(contains("esp-rom-elfs")));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn tampered_rom_is_refused_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("esp32c3_rev3_rom.elf"), b"not the ROM").unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
        .arg(dir.path())
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(
            contains("expected sha256 19ac22e0")
                .and(contains("got "))
                .and(contains(format!(
                    "`inkemu rom fetch --all --rom-dir {}`",
                    dir.path().display()
                )))
                .and(contains("download it again").not()),
        );
}

#[test]
fn run_help_lists_every_exit_code() {
    let help = inkemu().args(["run", "--help"]).assert().success();
    let out = String::from_utf8_lossy(&help.get_output().stdout).into_owned();
    for code in ["0", "2", "3", "4"] {
        assert!(
            out.contains(&format!("\n  {code}  ")),
            "exit code {code} missing from:\n{out}"
        );
    }
    assert!(
        !out.contains("x4 or x3"),
        "the help must not enumerate device ids:\n{out}"
    );
}

#[test]
fn a_time_limit_that_is_not_a_number_of_seconds_exits_2_and_says_what_to_write() {
    inkemu()
        .args(["run", "--device", "x4", "--max-time", "abc"])
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(
            contains("'--max-time <SECONDS>'").and(contains(
                "`abc` is not a number of seconds; write digits with an optional fraction, like 30 or 0.25",
            )),
        );
}

#[test]
fn run_help_tells_the_two_limits_apart() {
    let help = inkemu().args(["run", "--help"]).assert().success();
    // The help is wrapped to the terminal: compared with every run of white space as one space.
    let out = String::from_utf8_lossy(&help.get_output().stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for expected in [
        "--max-cycles <MAX_CYCLES> Stop when the CPU has executed this many cycles of its clock. A sleeping CPU executes none: a firmware that sleeps never reaches this limit, use --max-time to bound it",
        "--max-time <SECONDS> Stop when this many seconds of device time have elapsed since power-on (a decimal number: 30, 2.5, 0.001). Device time goes on while the CPU sleeps and across resets",
        "0 the run ended normally: the cycle limit or the device-time limit was reached, Ctrl-C was pressed, or the reader of standard output went away",
    ] {
        assert!(
            out.contains(expected),
            "`{expected}` is missing from:\n{out}"
        );
    }
}

#[test]
fn a_console_the_device_does_not_have_exits_2_and_lists_the_three_it_has() {
    inkemu()
        .args(["run", "--device", "x4", "--console", "uart2"])
        .assert()
        .code(2)
        .stderr(
            contains("'--console <CONSOLE>'")
                .and(contains("uart0"))
                .and(contains("uart1"))
                .and(contains("usb-serial-jtag")),
        );
}

#[test]
fn rom_fetch_requires_a_device_or_all() {
    inkemu().args(["rom", "fetch"]).assert().code(2);
}

/// The command the error messages print (`inkemu rom fetch --all --rom-dir <dir>`) must be accepted by the
/// argument parser. A tampered file present in the directory is refused before any download, so this
/// stays off the network.
#[test]
fn the_fetch_command_suggested_for_a_user_directory_is_accepted_and_refuses_a_tampered_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("esp32c3_rev3_rom.elf"), b"not the ROM").unwrap();
    inkemu()
        .args(["rom", "fetch", "--all", "--rom-dir"])
        .arg(dir.path())
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(
            contains("expected sha256 19ac22e0")
                .and(contains("got "))
                .and(contains("downloading").not())
                .and(contains("Usage:").not()),
        );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "nothing was downloaded or written"
    );
}

/// The command an error message prints, between its backticks.
fn printed_command(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let start = text.find("`inkemu rom fetch").expect("a printed command") + 1;
    let len = text[start..].find('`').expect("a closing backtick");
    text[start..start + len].to_owned()
}

/// `rom fetch --rom-dir X` downloads into X, so its remedy must name X again: the bare command would
/// download into the per-user cache instead.
#[test]
fn rom_fetch_into_a_named_directory_names_that_directory_in_its_remedy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("esp32c3_rev3_rom.elf")).unwrap();
    inkemu()
        .args(["rom", "fetch", "--device", "x4", "--rom-dir"])
        .arg(dir.path())
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(
            contains(format!(
                "`inkemu rom fetch --all --rom-dir {}`",
                dir.path().display()
            ))
            .and(contains("downloading").not()),
        );
}

/// The printed command, pasted into a real shell, for a directory whose name holds a space: the shell
/// must hand the directory to `inkemu` as one argument. The tampered file stops the command before any
/// download, so this stays off the network.
#[cfg(unix)]
#[test]
fn the_fetch_command_printed_for_a_directory_with_a_space_runs_as_printed_in_a_shell() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("my roms");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("esp32c3_rev3_rom.elf"), b"not the ROM").unwrap();
    let refused = inkemu()
        .args(["run", "--device", "x4", "--rom-dir"])
        .arg(&dir)
        .assert()
        .code(2);
    let command = printed_command(&refused.get_output().stderr);
    assert_eq!(
        command,
        format!("inkemu rom fetch --all --rom-dir \"{}\"", dir.display())
    );

    // `inkemu` resolves through PATH, as it does for the person pasting the command.
    let bin = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_inkemu"), bin.path().join("inkemu")).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    Command::new("sh")
        .arg("-c")
        .arg(&command)
        .env("PATH", path)
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(
            contains("expected sha256 19ac22e0")
                .and(contains("my roms"))
                .and(contains("downloading").not())
                .and(contains("Usage:").not()),
        );
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        1,
        "nothing was downloaded or written"
    );
}

#[test]
fn rom_fetch_refuses_a_device_together_with_all() {
    inkemu()
        .args(["rom", "fetch", "--device", "x4", "--all"])
        .assert()
        .code(2);
}

#[test]
fn rom_fetch_of_an_unknown_device_exits_2_and_lists_the_known_ones() {
    inkemu()
        .args(["rom", "fetch", "--device", "x9"])
        .assert()
        .code(2)
        .stderr(contains("x9").and(contains("x3")).and(contains("x4")));
}

/// The real ROM, through the real command. `INKEMU_ROM_ELF_DIR` must hold the three ROM ELF files.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn x4_boots_the_real_rom_to_its_banner() {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let run = || {
        inkemu()
            .args([
                "run",
                "--device",
                "x4",
                "--max-cycles",
                "5000000",
                "--trace-hash",
                "--rom-dir",
                &dir,
            ])
            .assert()
            .code(0)
            .get_output()
            .clone()
    };
    let first = run();
    let console = String::from_utf8_lossy(&first.stdout).replace('\r', "");
    assert!(
        console.starts_with(
            "ESP-ROM:esp32c3-api1-20210207\nBuild:Feb  7 2021\nrst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n"
        ),
        "{console}"
    );
    let log = String::from_utf8_lossy(&first.stderr).into_owned();
    assert!(
        log.contains("Xteink X4") && log.contains("v0.3") && log.contains("not calibrated"),
        "{log}"
    );
    assert_eq!(
        first.stdout,
        run().stdout,
        "two runs print the same console"
    );
    let hash = |out: &std::process::Output| {
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .find(|l| l.starts_with("trace-hash "))
            .map(str::to_owned)
    };
    assert!(hash(&first).is_some());
    assert_eq!(hash(&first), hash(&run()));
}

/// The command the `rom-boot` CI job and the README recommend, on ROM files that are already there:
/// no download, one line per file.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rom_fetch_all_prints_one_path_per_rom_and_downloads_nothing_when_they_are_present() {
    let source = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let names = [
        "esp32c3_rev0_rom.elf",
        "esp32c3_rev3_rom.elf",
        "esp32c3_rev101_rom.elf",
    ];
    let dir = tempfile::tempdir().unwrap();
    for name in names {
        std::fs::copy(
            std::path::Path::new(&source).join(name),
            dir.path().join(name),
        )
        .unwrap();
    }
    let assert = inkemu()
        .args(["rom", "fetch", "--all", "--rom-dir"])
        .arg(dir.path())
        .assert()
        .code(0)
        .stderr(contains("downloading").not());
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    for (line, name) in lines.iter().zip(names) {
        assert!(line.ends_with(name), "{line} should end with {name}");
    }
}

/// Runs `inkemu run --device x4 --flash <file with bytes>` with a ROM directory that holds nothing:
/// a refusal of the flash file must come before any attempt to load a ROM.
fn run_with_flash(bytes: &[u8]) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flash.bin");
    std::fs::write(&path, bytes).unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--flash"])
        .arg(&path)
        .arg("--rom-dir")
        .arg(dir.path())
        .assert()
}

#[test]
fn xota_is_refused_before_boot() {
    let mut bytes = b"XOTA".to_vec();
    bytes.resize(0x9000, 0);
    // The remedy must fit `--flash`: an application .bin, what `inspect` asks for, is refused here.
    run_with_flash(&bytes).code(2).stderr(
        contains("flash.bin")
            .and(contains("encrypted OTA"))
            .and(contains(
                "; use a full flash dump (`esptool.py read_flash`), or an image merged with `esptool.py merge_bin`",
            ))
            .and(contains("application .bin").not()),
    );
}

#[test]
fn a_file_without_the_image_magic_is_refused_before_boot() {
    run_with_flash(&vec![0u8; 0x9000]).code(2).stderr(
        contains("first byte is 0x00")
            .and(contains("expected 0xe9"))
            .and(contains("merge_bin")),
    );
}

#[test]
fn image_for_another_chip_is_refused() {
    run_with_flash(&build_flash_image(&ImageSpec {
        chip: 9,
        ..ImageSpec::default()
    }))
    .code(2)
    .stderr(
        contains("ESP32-S3")
            .and(contains("ESP32-C3"))
            .and(contains("; choose")),
    );
}

#[test]
fn image_declaring_more_flash_than_the_device_is_refused() {
    run_with_flash(&build_flash_image(&ImageSpec {
        flash_size_nibble: 5,
        ..ImageSpec::default()
    }))
    .code(2)
    .stderr(contains("32 MB").and(contains("16 MB")));
}

#[test]
fn an_application_image_given_as_flash_is_refused() {
    run_with_flash(&build_image(&ImageSpec::default()))
        .code(2)
        .stderr(contains("no partition table at offset 0x8000").and(contains("merge_bin")));
}

/// `inkemu run --device x4 --flash <path>` with an empty ROM directory: a refusal of the flash file
/// that came after the ROM lookup would show as the ROM error, and never as a download.
fn run_with_flash_path(path: &str) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--flash"])
        .arg(path)
        .arg("--rom-dir")
        .arg(dir.path())
        .assert()
}

#[test]
fn a_flash_file_that_does_not_exist_exits_2() {
    run_with_flash_path("/nonexistent/flash.bin")
        .code(2)
        .stderr(
            contains("/nonexistent/flash.bin")
                .and(contains("; check the path"))
                .and(contains("downloading").not())
                .and(contains("esp32c3_rev3_rom.elf").not()),
        );
}

#[test]
fn a_flash_path_with_control_characters_is_echoed_escaped() {
    run_with_flash_path("/nonexistent/\u{1b}[31mred.bin")
        .code(2)
        .stderr(predicate::function(|err: &[u8]| !err.contains(&0x1b)))
        .stderr(contains("/nonexistent/\\u{1b}[31mred.bin"));
}

#[cfg(unix)]
#[test]
fn a_device_node_given_as_flash_is_refused_at_once() {
    // `/dev/zero` never ends: the command must refuse it for what it is instead of reading it.
    run_with_flash_path("/dev/zero")
        .code(2)
        .stderr(contains("/dev/zero").and(contains("is not a regular file; use the path of")));
}

#[test]
fn run_help_documents_the_flash_option_and_what_strict_stops_on() {
    inkemu().args(["run", "--help"]).assert().code(0).stdout(
        contains("--flash")
            .and(contains("erased"))
            .and(contains("flash that is not mapped")),
    );
}

/// The real ROM on a full flash image, through the real command.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn x4_boots_a_flash_image_through_the_real_rom() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let stub = ImageSpec {
        entry: 0x4038_0000,
        segments: vec![(0x4038_0000, vec![0x6f, 0, 0, 0])],
        ..ImageSpec::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flash.bin");
    std::fs::write(&path, build_flash_image(&stub)).unwrap();
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--max-cycles",
            "5000000",
            "--rom-dir",
            &rom_dir,
            "--flash",
        ])
        .arg(&path)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    assert!(
        console.ends_with(
            "SPIWP:0xee\nmode:DIO, clock div:1\nload:0x40380000,len:0x4\nentry 0x40380000\n"
        ),
        "{console}"
    );
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        log.contains("flash: flash.bin") && log.contains("16 MiB") && !log.contains("warning:"),
        "{log}"
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn x4_without_a_flash_image_loops_on_the_invalid_header_of_an_erased_chip() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--max-cycles",
            "3000000",
            "--rom-dir",
            &rom_dir,
        ])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    assert!(
        console.matches("invalid header: 0xffffffff\n").count() >= 3,
        "{console}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("flash: erased"),
        "the header line says so"
    );
}

/// Where the ROM loads and enters the second stage of the images built here: the internal RAM.
const STUB_ENTRY: u32 = 0x4038_0000;

/// A flash image whose second stage is `program`, in a directory that lives as long as the value.
fn flash_image_of(program: &[u32]) -> (tempfile::TempDir, std::path::PathBuf) {
    let spec = ImageSpec {
        entry: STUB_ENTRY,
        segments: vec![(
            STUB_ENTRY,
            program.iter().flat_map(|word| word.to_le_bytes()).collect(),
        )],
        ..ImageSpec::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flash.bin");
    std::fs::write(&path, build_flash_image(&spec)).unwrap();
    (dir, path)
}

/// A second stage that stops the watchdogs the ROM left running, then sleeps for ever under a
/// period alarm of one millisecond (16 000 counts of the 16 MHz counter; `TARGET0_CONF` bit 30
/// is `PERIOD_MODE`, `CONF` bit 24 `TARGET0_WORK_EN`) whose interrupt is not enabled: the CPU
/// executes nothing more, and device time goes on.
fn sleeping_for_ever() -> Vec<u32> {
    const PERIOD_MODE: u32 = 1 << 30;
    const MILLISECOND: u32 = 16_000;
    let systimer = |register| svd_regs::esp32c3::addr("SYSTIMER", register);
    let mut program = stop_watchdogs();
    program.extend(store_register(
        systimer("TARGET0_CONF"),
        PERIOD_MODE | MILLISECOND,
    ));
    program.extend(store_register(systimer("COMP0_LOAD"), 1));
    program.extend(store_register(systimer("CONF"), 0x4600_0000 | (1 << 24)));
    program.extend([wfi(), HANG]);
    program
}

/// The microseconds of the summary line `inkemu: N us of device time; ...`.
fn device_micros(log: &str) -> u64 {
    log.lines()
        .find_map(|line| {
            line.strip_prefix("inkemu: ")?
                .split_once(" us of device time")?
                .0
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("no device-time line in:\n{log}"))
}

fn trace_hash_line(log: &str) -> Option<&str> {
    log.lines().find(|line| line.starts_with("trace-hash "))
}

/// A blank chip is reset by main watchdog 0 after 1.3 s of device time: a limit of 1.5 s shows
/// one reset through the real command, and the run it ends is the same run every time.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_device_time_limit_ends_the_run_of_a_blank_chip_after_its_first_watchdog_reset() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let run = || {
        inkemu()
            .args([
                "run",
                "--device",
                "x4",
                "--strict-mmio",
                "--max-time",
                "1.5",
                "--trace-hash",
                "--rom-dir",
                &rom_dir,
            ])
            .assert()
            .code(0)
            .get_output()
            .clone()
    };
    let first = run();
    let console = String::from_utf8_lossy(&first.stdout).replace('\r', "");
    assert_eq!(console.matches("rst:0x7 ").count(), 1, "{console}");
    let log = String::from_utf8_lossy(&first.stderr).into_owned();
    assert!(
        log.contains("inkemu: device-time limit reached; raise --max-time to run longer\n"),
        "{log}"
    );
    // The CPU executes when the limit is reached: the run stops at the first instruction
    // boundary at or past it.
    let micros = device_micros(&log);
    assert!((1_500_000..=1_500_010).contains(&micros), "{log}");
    assert!(!log.contains("the CPU is asleep"), "{log}");
    // The journal says what the ROM's `rst:0x7` line does, with the stack pointer besides.
    assert!(
        log.contains(
            "\ninkemu: reset by main watchdog 0, a core reset (rst:0x7) at pc 0x4004c0d4, sp 0x"
        ) && !log.contains("warning:"),
        "{log}"
    );
    let second = run();
    assert_eq!(
        first.stdout, second.stdout,
        "two runs print the same console"
    );
    assert!(trace_hash_line(&log).is_some(), "{log}");
    assert_eq!(
        trace_hash_line(&log),
        trace_hash_line(&String::from_utf8_lossy(&second.stderr))
    );
}

/// A firmware that sleeps executes no cycle: only the device-time limit ends its run.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_device_time_limit_ends_the_run_of_a_firmware_that_sleeps() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (_dir, flash) = flash_image_of(&sleeping_for_ever());
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--max-time",
            "2",
            "--rom-dir",
            &rom_dir,
            "--flash",
        ])
        .arg(&flash)
        .timeout(std::time::Duration::from_secs(60))
        .assert()
        .code(0)
        .get_output()
        .clone();
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        log.contains("inkemu: device-time limit reached; raise --max-time to run longer\n"),
        "{log}"
    );
    assert!(
        log.contains("inkemu: 2000000 us of device time; ")
            && log.contains("; the CPU is asleep, waiting for an interrupt\n"),
        "the limit instant exactly, with the CPU asleep: {log}"
    );
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    assert!(console.ends_with("entry 0x40380000\n"), "{console}");
}

/// A run of the binary whose output is read while it goes on, for the tests that act on a run in
/// progress. Every wait has a limit, and a run still going when its test ends is killed.
#[cfg(unix)]
mod live {
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    pub struct Live {
        child: Child,
        /// The lines of standard output, as they come.
        lines: Receiver<String>,
        stderr: Option<JoinHandle<String>>,
    }

    /// Starts `command`. Its standard output is read line by line on a thread of its own; the
    /// thread stops reading, and closes its end of the pipe, after the first line for which
    /// `reader_leaves_after` says yes: from then on the run has no reader.
    pub fn start(
        mut command: Command,
        reader_leaves_after: impl Fn(&str) -> bool + Send + 'static,
    ) -> Live {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                if stdout.read_until(b'\n', &mut line).unwrap_or(0) == 0 {
                    return;
                }
                let line = String::from_utf8_lossy(&line).into_owned();
                let leaves = reader_leaves_after(&line);
                // A test that stopped listening does not need the line.
                let _ = sender.send(line);
                if leaves {
                    return;
                }
            }
        });
        let mut stderr = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = stderr.read_to_end(&mut text);
            String::from_utf8_lossy(&text).into_owned()
        });
        Live {
            child,
            lines,
            stderr: Some(stderr),
        }
    }

    impl Live {
        /// Waits until a line of standard output contains `text`.
        pub fn wait_for_line(&self, text: &str, limit: Duration) {
            let deadline = Instant::now() + limit;
            let mut seen = String::new();
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match self.lines.recv_timeout(left) {
                    Ok(line) if line.contains(text) => return,
                    Ok(line) => seen.push_str(&line),
                    Err(RecvTimeoutError::Timeout) => {
                        panic!("no line with `{text}` within {limit:?}; standard output:\n{seen}")
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        panic!("standard output ended without a line with `{text}`:\n{seen}")
                    }
                }
            }
        }

        /// Waits for the run to end by itself: its exit code and what it wrote on standard error.
        pub fn finish(mut self, limit: Duration) -> (Option<i32>, String) {
            let deadline = Instant::now() + limit;
            let status = loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    Instant::now() < deadline,
                    "the run did not end within {limit:?}"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            let log = self.stderr.take().unwrap().join().unwrap();
            (status.code(), log)
        }
    }

    impl Drop for Live {
        fn drop(&mut self) {
            // Nothing to do for a run that has ended; one that has not must not outlive its test.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// `inkemu run --device x4` on `flash` with the real ROM, to be started by [`live::start`].
#[cfg(unix)]
fn run_of(rom_dir: &str, flash: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_inkemu"));
    command
        .args(["run", "--device", "x4", "--rom-dir", rom_dir, "--flash"])
        .arg(flash);
    command
}

/// `inkemu run firmware.bin | head`, with a firmware that prints nothing: no write ever fails,
/// and the run must still end once nobody reads it. No limit is given: only the reader that
/// went away can end this run.
#[cfg(unix)]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_run_that_prints_nothing_more_ends_when_the_reader_of_its_output_goes_away() {
    const LAST_ROM_LINE: &str = "entry 0x40380000";
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    // A second stage that stops the watchdogs and spins: silent and busy for ever.
    let mut program = stop_watchdogs();
    program.push(HANG);
    let (_dir, flash) = flash_image_of(&program);
    let run = live::start(run_of(&rom_dir, &flash), |line| {
        line.contains(LAST_ROM_LINE)
    });
    let limit = std::time::Duration::from_secs(60);
    run.wait_for_line(LAST_ROM_LINE, limit);
    let (code, log) = run.finish(limit);
    assert_eq!(code, Some(0), "{log}");
    assert!(
        log.contains("inkemu: the reader of standard output went away\ninkemu: stopped at pc "),
        "{log}"
    );
}

/// The milestone through the real command: ROM, bootloader, the whole life of the application,
/// its restart and the first line of the second boot, with nothing unmodelled.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_boots_the_reference_firmware_through_its_restart_into_a_second_boot() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let flash = std::env::var("INKEMU_REF_FLASH").expect("set INKEMU_REF_FLASH");
    // One life of the application is 28 million cycles and 11 s of device time; 40 million reach
    // the second boot.
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--max-cycles",
            "40000000",
            "--rom-dir",
            &rom_dir,
            "--flash",
            &flash,
        ])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    let mut from = 0;
    for expected in [
        "entry 0x403cbf1a\n",
        ") boot: ESP-IDF v5.5.2 2nd stage bootloader\n",
        ") cpu_start: Unicore app\n",
        "Hello world!\n",
        "Restarting in 10 seconds...\n",
        "Restarting in 0 seconds...\n",
        "Restarting now.\n",
        "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "Saved PC:0x40048b92\n",
        ") boot: ESP-IDF v5.5.2 2nd stage bootloader\n",
        ") cpu_start: Unicore app\n",
    ] {
        let at = console[from..]
            .find(expected)
            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{console}"));
        from += at + expected.len();
    }
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(log.contains("cycle limit reached"), "{log}");
    // The restart is an event of the run, said in inkemu's own voice with its cause, PC and
    // stack: not a warning.
    assert!(
        log.contains(
            "\ninkemu: reset by the firmware, a CPU reset (rst:0xc) at pc 0x40048b92, sp 0x"
        ),
        "{log}"
    );
    assert!(
        !log.contains("warning:") && !log.contains("stopped by --strict-mmio"),
        "nothing was reported in a whole life cycle: {log}"
    );
}

/// The application counts down a line a second and sleeps in between: five seconds of device
/// time end the run inside its countdown, with the CPU asleep. The name starts as the test above
/// does, so that the CI job that has the reference image runs both.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_boots_the_reference_firmware_and_a_device_time_limit_ends_its_countdown_asleep() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let flash = std::env::var("INKEMU_REF_FLASH").expect("set INKEMU_REF_FLASH");
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--max-time",
            "5",
            "--rom-dir",
            &rom_dir,
            "--flash",
            &flash,
        ])
        .timeout(std::time::Duration::from_secs(120))
        .assert()
        .code(0)
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    // The line of ten seconds is printed a quarter of a second after power-on, then one a
    // second: the last one due before five seconds is the line of six.
    for seconds in 6..=10 {
        assert!(
            console.contains(&format!("Restarting in {seconds} seconds...\n")),
            "{console}"
        );
    }
    assert!(
        console.ends_with("Restarting in 6 seconds...\n"),
        "{console}"
    );
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        log.contains("inkemu: device-time limit reached; raise --max-time to run longer\n"),
        "{log}"
    );
    let micros = device_micros(&log);
    assert!((5_000_000..=5_000_100).contains(&micros), "{log}");
    assert!(
        log.contains("; the CPU is asleep, waiting for an interrupt\n"),
        "{log}"
    );
    assert!(!log.contains("warning:"), "{log}");
}
