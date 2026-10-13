//! `inkemu run` and `inkemu rom fetch` through the real binary. Nothing here touches the network or the
//! default ROM cache: every run names its ROM directory.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use assert_cmd::Command;
use cpu_rv32::asm::wfi;
use firmware::builder::{ImageSpec, build_flash_image, build_image};
use predicates::prelude::*;
use predicates::str::contains;
use soc_esp32c3::test_support::{
    HANG, load_register, stop_watchdogs, store_register, unmodeled_register,
};

fn inkemu() -> Command {
    Command::new(env!("CARGO_BIN_EXE_inkemu"))
}

/// How long one command of these tests, or one wait on a run in progress, may take on the host
/// before the test fails instead of hanging. It turns a hang into a failure and says nothing of
/// speed, so it is wide. Measured beside a release build and 16 spinning processes on 16 cores:
/// a command refused before boot took up to 2.9 s instrumented by `cargo llvm-cov` (the hash of
/// a 16 MiB flash in a debug build), and the slowest command the CI runs in release, the third
/// run of the NVS reference firmware (12.5 s of device time), 7.9 s, at a load average of 20 to
/// 44 on 16 cores. The limit is fifteen minutes, more than a hundred times either, and under the
/// timeout of the CI jobs that run these tests.
const HANG_LIMIT: std::time::Duration = std::time::Duration::from_mins(15);

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

/// `/dev/full` refuses every write: the error message cannot be printed, and the exit code must
/// still say that the command was refused, not that the program crashed (101).
#[cfg(target_os = "linux")]
#[test]
fn an_error_that_cannot_be_written_to_standard_error_keeps_its_exit_code() {
    // The path of the binary is `$0` of the shell, so that no quoting of it is needed.
    for command in [
        r#""$0" run --device x9 2>/dev/full"#,
        r#""$0" inspect /nonexistent/firmware.bin 2>/dev/full"#,
        r#""$0" rvtest /nonexistent/test.elf 2>/dev/full"#,
    ] {
        Command::new("sh")
            .args(["-c", command, env!("CARGO_BIN_EXE_inkemu")])
            .assert()
            .code(2);
    }
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
    for code in [0, 2, 3, 4, 130] {
        assert!(
            out.contains(&format!("\n{code:>3}  ")),
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
    // A negative number included: it is a value of the option, refused for what it is, and not
    // an unknown flag.
    for value in ["abc", "-1", "1e3"] {
        inkemu()
            .args(["run", "--device", "x4", "--max-time", value])
            .assert()
            .code(2)
            .stdout(predicate::str::is_empty())
            .stderr(contains("'--max-time <SECONDS>'").and(contains(format!(
                "`{value}` is not a number of seconds; write digits with an optional fraction, like 30 or 0.25"
            ))));
    }
}

#[test]
fn a_time_limit_too_fine_or_too_long_exits_2_and_says_which() {
    for (value, message) in [
        (
            "1.0000000001",
            "has more than 9 digits after the point, which is finer than a nanosecond; give at most 9",
        ),
        (
            "18446744074",
            "seconds is more than device time can count (584 years); give a smaller number",
        ),
    ] {
        inkemu()
            .args(["run", "--device", "x4", "--max-time", value])
            .assert()
            .code(2)
            .stderr(
                contains("'--max-time <SECONDS>'").and(contains(format!("`{value}` {message}"))),
            );
    }
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
        "--max-cycles <MAX_CYCLES> Stop when the CPU has executed this many cycles of its clock. The limit counts executed cycles, not device time: a sleeping CPU executes none, so a firmware that sleeps reaches it late, and one whose sleep nothing interrupts never does; --max-time bounds both",
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
fn a_usb_host_state_the_port_does_not_have_exits_2_and_lists_the_three_it_has() {
    inkemu()
        .args(["run", "--device", "x4", "--usb-host", "closed"])
        .assert()
        .code(2)
        .stderr(
            contains("'--usb-host <USB_HOST>'")
                .and(contains("absent"))
                .and(contains("attached"))
                .and(contains("open")),
        );
}

/// A console script with `body` after its version line, in a directory that lives as long as
/// the value.
fn console_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("steps.txt");
    std::fs::write(&path, format!("inkemu-console-script 1\n{body}")).unwrap();
    (dir, path)
}

#[test]
fn a_refused_console_script_exits_2_at_its_line_and_column_before_any_rom_is_read() {
    let (dir, script) = console_script("wait 3\n");
    let empty_rom_dir = dir.path().join("roms");
    std::fs::create_dir(&empty_rom_dir).unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--rom-dir"])
        .arg(&empty_rom_dir)
        .arg("--console-script")
        .arg(&script)
        .assert()
        .code(2)
        .stderr(
            contains("steps.txt, line 2, column 6: `3` is no duration; write a whole number")
                .and(contains("esp32c3_rev3_rom.elf").not()),
        );
    inkemu()
        .args(["run", "--device", "x4", "--rom-dir"])
        .arg(&empty_rom_dir)
        .arg("--console-script")
        .arg(dir.path())
        .assert()
        .code(2)
        .stderr(contains(
            "is not a regular file; give the path of the file itself",
        ));
}

#[test]
fn run_help_documents_the_console_script_and_the_usb_host() {
    inkemu().args(["run", "--help"]).assert().code(0).stdout(
        contains("--console-script <FILE>")
            .and(contains("inkemu-console-script 1"))
            .and(contains("wait-for CHANNEL"))
            .and(contains("--usb-host <USB_HOST>"))
            .and(contains("[default: open]"))
            .and(contains("\n  5  ")),
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
            .timeout(HANG_LIMIT)
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

/// `--usb-host` reaches the machine: nothing in a run of a blank chip changes the host, so the
/// summary says at the end the state the option gave at power-on, and `open` without it.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn the_usb_host_given_is_the_one_the_summary_says() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    for (option, state) in [
        (Some("absent"), "absent"),
        (Some("attached"), "attached"),
        (Some("open"), "open"),
        (None, "open"),
    ] {
        let mut command = inkemu();
        command.args([
            "run",
            "--device",
            "x4",
            "--max-cycles",
            "1000000",
            "--rom-dir",
            &rom_dir,
        ]);
        if let Some(option) = option {
            command.args(["--usb-host", option]);
        }
        let output = command
            .timeout(HANG_LIMIT)
            .assert()
            .code(0)
            .get_output()
            .clone();
        let log = String::from_utf8_lossy(&output.stderr);
        assert!(
            log.contains(&format!("\ninkemu: USB host {state}\n")),
            "--usb-host {option:?}:\n{log}"
        );
    }
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
        .stderr(
            contains("no partition table at offset 0x8000")
                .and(contains("; use an application with --firmware"))
                .and(contains("merge_bin")),
        );
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

/// Runs `inkemu run --device x4 --max-cycles 1000 <args>` with an empty ROM directory, `pipe`
/// being a named pipe among the files `args` gives, and tells whether the command opened the pipe.
///
/// Opening a pipe for reading waits for a writer that never comes: a command that opened it would
/// wait for ever. No wall-clock limit decides the outcome. Between looks at whether the command
/// ended, the test opens the pipe for writing without waiting (`O_NONBLOCK`), which the system
/// refuses (`ENXIO`) while no reader holds it, and grants as soon as the command opened it to
/// read, even while that open still waits. Each look ends in one of two outcomes the command
/// decides: it ended, or it opened the pipe, and granting the write end lets its open return.
/// [`HANG_LIMIT`] only turns a command that does neither into a failure.
#[cfg(unix)]
fn run_on_a_pipe(
    pipe: &std::path::Path,
    rom_dir: &std::path::Path,
    args: &[&std::ffi::OsStr],
) -> (bool, std::process::Output) {
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_inkemu"))
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
        .arg(rom_dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + HANG_LIMIT;
    let opened = loop {
        if child.try_wait().unwrap().is_some() {
            break false;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
            .open(pipe)
        {
            // A reader holds the pipe: the command opened it.
            Ok(_writer) => break true,
            Err(e) if e.raw_os_error() == Some(nix::errno::Errno::ENXIO as i32) => {}
            Err(e) => panic!("opening {} to write: {e}", pipe.display()),
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the command neither ended nor opened {}", pipe.display());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    if opened {
        // Whatever it does next, its outcome is decided.
        let _ = child.kill();
    }
    (opened, child.wait_with_output().unwrap())
}

#[cfg(unix)]
#[test]
fn a_pipe_given_as_flash_firmware_or_bootloader_is_refused_without_being_opened() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = dir.path().join("pipe.bin");
    make_fifo(&pipe);
    let app = dir.path().join("app.bin");
    std::fs::write(&app, build_image(&ImageSpec::default())).unwrap();
    let rom_dir = dir.path().join("rom");
    for (option, args) in [
        (
            "--flash",
            vec![std::ffi::OsStr::new("--flash"), pipe.as_os_str()],
        ),
        (
            "--firmware",
            vec![std::ffi::OsStr::new("--firmware"), pipe.as_os_str()],
        ),
        (
            "--bootloader",
            vec![
                std::ffi::OsStr::new("--firmware"),
                app.as_os_str(),
                std::ffi::OsStr::new("--bootloader"),
                pipe.as_os_str(),
            ],
        ),
    ] {
        let (opened, output) = run_on_a_pipe(&pipe, &rom_dir, &args);
        let log = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!opened, "{option}: the pipe was opened: {log}");
        assert_eq!(output.status.code(), Some(2), "{option}: {log}");
        assert!(
            log.contains("pipe.bin")
                && log.contains("is not a regular file")
                && !log.contains(MISSING_ROM),
            "{option}: {log}"
        );
    }
}

#[test]
fn run_help_documents_the_flash_option_and_what_strict_stops_on() {
    let output = inkemu()
        .args(["run", "--help"])
        .assert()
        .code(0)
        .stdout(contains("--flash").and(contains("erased")))
        .get_output()
        .clone();
    let help = String::from_utf8(output.stdout).unwrap();
    // What a strict run stops on is said twice: by the option and by the list of exit codes.
    // Each text must say it on its own, whatever the width the help was wrapped at.
    let (options, exit_codes) = help
        .split_once("Exit codes:")
        .unwrap_or_else(|| panic!("no list of exit codes in:\n{help}"));
    // The help of the option: from the line that starts with its name (other options mention
    // it in their text) to the line that starts the next option.
    let strict_option: Vec<&str> = options
        .lines()
        .skip_while(|line| !line.trim_start().starts_with("--strict-mmio"))
        .enumerate()
        .take_while(|(index, line)| *index == 0 || !line.trim_start().starts_with('-'))
        .map(|(_, line)| line)
        .collect();
    // The text of exit code 3: from its number to the number of the next code.
    let exit_code_3 = exit_codes
        .split_once("\n  3  ")
        .and_then(|(_, from_3)| from_3.split_once("\n  4  "))
        .map_or_else(
            || panic!("no exit code 3 in:\n{exit_codes}"),
            |(text, _)| text,
        );
    for (text, section) in [
        (strict_option.join(" "), "the help of --strict-mmio"),
        (exit_code_3.to_owned(), "the help of exit code 3"),
    ] {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        for phrase in [
            "flash that is not mapped",
            "read through a flash window that may differ from the device",
        ] {
            assert!(
                text.contains(phrase),
                "`{phrase}` is not in {section}: `{text}`"
            );
        }
    }
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
        .timeout(HANG_LIMIT)
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
        .timeout(HANG_LIMIT)
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
            // A blank chip prints and resets for ever: were the limit to stop ending the run,
            // this ends the test.
            .timeout(HANG_LIMIT)
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
    // The journal says what the ROM's `rst:0x7` line does, with the stack pointer besides. The
    // watchdog finds the ROM in the loop of `uart_tx_one_char_uart.part.0` that waits for room in
    // the UART FIFO, at whichever of its four instructions the phase of the loop puts it on
    // (rev3 ROM ELF: `lw` 0x4004c0d4, `srli` 0x4004c0d6, `andi` 0x4004c0d8, `bnez` 0x4004c0dc);
    // the trace hash pins the run.
    let Some((reset_pc, _)) = log
        .split_once("\ninkemu: reset by main watchdog 0, a core reset (rst:0x7) at pc ")
        .and_then(|(_, rest)| rest.split_once(", sp 0x"))
    else {
        panic!("no reset by main watchdog 0 in the journal:\n{log}")
    };
    assert!(
        ["0x4004c0d4", "0x4004c0d6", "0x4004c0d8", "0x4004c0dc"].contains(&reset_pc),
        "{log}"
    );
    assert!(!log.contains("warning:"), "{log}");
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

/// A second stage that stops the watchdogs, prints `ready` on UART0, then echoes on UART0 what
/// it receives, polling `STATUS.RXFIFO_CNT`.
fn ready_then_echo() -> Vec<u32> {
    use cpu_rv32::asm::{beq, jal, lbu, lw, sw};
    use soc_esp32c3::test_support::{T0, T1, print_program};
    let uart0 = svd_regs::esp32c3::addr("UART0", "FIFO");
    let mut program = stop_watchdogs();
    program.extend(print_program(uart0, b"ready"));
    program.extend([
        lbu(T1, T0, 0x1C),
        beq(T1, 0, -4),
        lw(T1, T0, 0),
        sw(T1, T0, 0),
        jal(0, -16),
    ]);
    program
}

/// The run of the echo firmware with the console script `body`, to 50 ms of device time.
fn scripted_run(rom_dir: &str, body: &str) -> std::process::Output {
    let (_flash_dir, flash) = flash_image_of(&ready_then_echo());
    let (_script_dir, script) = console_script(body);
    inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--max-time",
            "0.05",
            "--rom-dir",
            rom_dir,
        ])
        .arg("--flash")
        .arg(&flash)
        .arg("--console-script")
        .arg(&script)
        .timeout(HANG_LIMIT)
        .output()
        .unwrap()
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_console_script_types_into_the_firmware_after_its_prompt() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let output = scripted_run(&rom_dir, "wait-for uart0 \"ready\"\nsend uart0 \"hi\\r\"\n");
    let log = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{log}");
    let console = String::from_utf8_lossy(&output.stdout);
    assert!(console.ends_with("readyhi\r"), "{console:?}");
    assert!(
        log.contains("inkemu: USB host open\ninkemu: console script through (2 steps)\n"),
        "{log}"
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_console_script_still_waiting_when_the_run_ends_exits_5() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let output = scripted_run(&rom_dir, "wait-for uart0 \"login:\"\n");
    let log = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(5), "{log}");
    // The firmware ran and printed its prompt: the script waited for a text that never came,
    // not for a firmware that never started.
    let console = String::from_utf8_lossy(&output.stdout);
    assert!(console.ends_with("ready"), "{console:?}");
    assert!(
        log.contains("inkemu: console script not through: line 2 (wait-for uart0 \"login:\") was still waiting; "),
        "{log}"
    );
}

/// Nothing interrupts the sleep of this firmware: it executes no cycle more, and only the
/// device-time limit ends its run.
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
        .timeout(HANG_LIMIT)
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

/// Exit code 3 through the real command: a second stage that reads a register the emulator does
/// not model, under `--strict-mmio`.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_strict_stop_exits_3_and_names_the_register() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let mut program = stop_watchdogs();
    program.extend(load_register(unmodeled_register().addr));
    program.push(HANG);
    let (_dir, flash) = flash_image_of(&program);
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--rom-dir",
            &rom_dir,
            "--flash",
        ])
        .arg(&flash)
        .timeout(HANG_LIMIT)
        .assert()
        .code(3)
        .stderr(contains("inkemu: stopped by --strict-mmio: ").and(contains("LEDC.")))
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    assert!(console.ends_with("entry 0x40380000\n"), "{console}");
}

/// Exit code 4 through the real command: a second stage that stops the watchdogs and waits for
/// an interrupt nothing can raise. No limit is given: the run ends by itself.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_wait_for_an_interrupt_nothing_can_raise_exits_4() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let mut program = stop_watchdogs();
    program.extend([wfi(), HANG]);
    let (_dir, flash) = flash_image_of(&program);
    inkemu()
        .args(["run", "--device", "x4", "--rom-dir", &rom_dir, "--flash"])
        .arg(&flash)
        .timeout(HANG_LIMIT)
        .assert()
        .code(4)
        .stderr(
            contains("inkemu: the CPU waits for an interrupt and none can come")
                .and(contains("; the CPU is asleep, waiting for an interrupt\n")),
        );
}

/// A run of the binary whose output is read while it goes on, for the tests that act on a run in
/// progress. Every wait has a limit, and a run still going when its test ends is killed.
#[cfg(unix)]
mod live {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    pub struct Live {
        child: Child,
        /// The lines of standard output, as they come.
        lines: Receiver<String>,
        /// The lines of standard error, as they come.
        log_lines: Receiver<String>,
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
        let stderr = child.stderr.take().unwrap();
        let (log_sender, log_lines) = channel();
        // Read to its end, whoever listens: a full pipe would stop the run.
        let stderr = std::thread::spawn(move || {
            let mut stderr = BufReader::new(stderr);
            let mut text = String::new();
            loop {
                let mut line = Vec::new();
                if stderr.read_until(b'\n', &mut line).unwrap_or(0) == 0 {
                    return text;
                }
                let line = String::from_utf8_lossy(&line).into_owned();
                text.push_str(&line);
                let _ = log_sender.send(line);
            }
        });
        Live {
            child,
            lines,
            log_lines,
            stderr: Some(stderr),
        }
    }

    /// Waits until a line of `lines` contains `text`; `what` names the stream in the failure.
    fn wait_for(lines: &Receiver<String>, what: &str, text: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        let mut seen = String::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match lines.recv_timeout(left) {
                Ok(line) if line.contains(text) => return,
                Ok(line) => seen.push_str(&line),
                Err(RecvTimeoutError::Timeout) => {
                    panic!("no line with `{text}` within {limit:?}; {what}:\n{seen}")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("{what} ended without a line with `{text}`:\n{seen}")
                }
            }
        }
    }

    impl Live {
        /// Sends the run the signal a terminal sends for Ctrl-C.
        pub fn interrupt(&self) {
            let sent = Command::new("kill")
                .args(["-INT", &self.child.id().to_string()])
                .status()
                .unwrap();
            assert!(sent.success(), "kill -INT {}: {sent}", self.child.id());
        }

        /// Waits until a line of standard output contains `text`.
        pub fn wait_for_line(&self, text: &str, limit: Duration) {
            wait_for(&self.lines, "standard output", text, limit);
        }

        /// Waits until a line of standard error contains `text`.
        pub fn wait_for_log_line(&self, text: &str, limit: Duration) {
            wait_for(&self.log_lines, "standard error", text, limit);
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

/// Ctrl-C through the real command, with a real `SIGINT`: a blank chip prints and resets for ever,
/// no limit is given, and the signal ends the run with its summary and exit code 0.
#[cfg(unix)]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn ctrl_c_ends_a_run_without_a_limit_with_its_summary_and_exit_code_0() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_inkemu"));
    command.args(["run", "--device", "x4", "--rom-dir", &rom_dir]);
    // The reader stays to the end: the run must not end for having lost it.
    let run = live::start(command, |_| false);
    let limit = HANG_LIMIT;
    // The banner is on standard output: the handler is installed and the run is under way.
    run.wait_for_line("ESP-ROM:esp32c3-api1-20210207", limit);
    run.interrupt();
    let (code, log) = run.finish(limit);
    assert_eq!(code, Some(0), "{log}");
    let interrupted = log
        .find("inkemu: interrupted by the user\n")
        .unwrap_or_else(|| panic!("no line for the interrupt in:\n{log}"));
    assert!(
        log[interrupted..].contains("\ninkemu: stopped at pc ")
            && log[interrupted..].contains(" us of device time; "),
        "the summary follows the interrupt: {log}"
    );
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
    let limit = HANG_LIMIT;
    run.wait_for_line(LAST_ROM_LINE, limit);
    let (code, log) = run.finish(limit);
    assert_eq!(code, Some(0), "{log}");
    assert!(
        log.contains("inkemu: the reader of standard output went away\ninkemu: stopped at pc "),
        "{log}"
    );
}

/// `flash_hello.bin` of `tools/reference-firmware/build.sh`, as
/// `tools/reference-firmware/SHA256SUMS.output` pins it.
const HELLO_IMAGE_SHA256: &str = "fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2";

/// The path the environment variable `variable` holds, accepted only when the file there is the
/// pinned image `file`. A run of another image fails later, on a console line or on a trace
/// hash, and says nothing of its cause.
fn pinned_image(variable: &str, file: &str, sha256: &str) -> String {
    let path = std::env::var(variable).unwrap_or_else(|_| panic!("set {variable}"));
    assert_eq!(
        sha256_of(std::path::Path::new(&path)),
        sha256,
        "{path} is not the pinned {file}; rebuild it with tools/reference-firmware/build.sh"
    );
    path
}

/// The reference `hello_world` image, named by `INKEMU_REF_FLASH`.
fn reference_image() -> String {
    pinned_image("INKEMU_REF_FLASH", "flash_hello.bin", HELLO_IMAGE_SHA256)
}

/// The milestone through the real command: ROM, bootloader, the whole life of the application,
/// its restart and the first line of the second boot, with nothing unmodelled.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_boots_the_reference_firmware_through_its_restart_into_a_second_boot() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let flash = reference_image();
    // One life of the application is 28 million cycles and 11 s of device time; 40 million reach
    // the second boot.
    let run = || {
        inkemu()
            .args([
                "run",
                "--device",
                "x4",
                "--strict-mmio",
                "--max-cycles",
                "40000000",
                "--trace-hash",
                "--rom-dir",
                &rom_dir,
                "--flash",
                &flash,
            ])
            .timeout(HANG_LIMIT)
            .assert()
            .code(0)
            .get_output()
            .clone()
    };
    let output = run();
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
    // The trace covers what ran before the restart as well as after it: a second process, given
    // the same image and the same limit, prints the same console and the same hash.
    let second = run();
    assert_eq!(
        output.stdout, second.stdout,
        "two runs print the same console"
    );
    assert!(trace_hash_line(&log).is_some(), "{log}");
    assert_eq!(
        trace_hash_line(&log),
        trace_hash_line(&String::from_utf8_lossy(&second.stderr))
    );
}

/// The application counts down a line a second and sleeps in between: five seconds of device
/// time end the run inside its countdown, with the CPU asleep. The name starts as the test above
/// does, so that the CI job that has the reference image runs both.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_boots_the_reference_firmware_and_a_device_time_limit_ends_its_countdown_asleep() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let flash = reference_image();
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
        .timeout(HANG_LIMIT)
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

/// The trace hash of the milestone run is pinned, with the device time it ends at.
///
/// The hash is the fingerprint of every PC the CPU executed, in order: here over one whole life
/// of the reference `hello_world` (ROM, bootloader, application, countdown, restart) and the
/// start of the second, 40,000,001 cycles in all. Two runs that agree on it took the same path,
/// instruction for instruction. The device time is how long that path lasted on the device.
///
/// Nothing but a change of the model moves either number: what an instruction or a register
/// does, or when an interrupt arrives, which the timing decides. A change that is meant to alter
/// the path or the timing changes them **on purpose**, and the commit that changes them here
/// says why. A change that was not meant to (a cache of the emulator, a diagnostic, the flash
/// file) and moves them has changed what the firmware sees: that is a regression to find, not a
/// value to pin again.
///
/// `reference_firmware` is in the name: it selects the tests of the CI job that has the image.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_runs_the_reference_firmware_along_its_pinned_trace() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let flash = reference_image();
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--max-cycles",
            "40000001",
            "--trace-hash",
            "--rom-dir",
            &rom_dir,
            "--flash",
            &flash,
        ])
        .timeout(HANG_LIMIT)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        trace_hash_line(&log),
        Some("trace-hash fb9ea628bc25b355"),
        "{log}"
    );
    assert_eq!(device_micros(&log), 11_396_706, "{log}");
    assert!(
        log.contains("after 40000001 cycles (26559249 instructions)"),
        "{log}"
    );
}

/// The flash of the X4, the device every test here runs.
const X4_FLASH_BYTES: u64 = 16 << 20;

/// `inkemu run --device x4 --flash-file <file> <more>` with a ROM directory that holds nothing:
/// a refusal of the flash file must come before any attempt to load a ROM.
fn run_with_flash_file(file: &std::path::Path, more: &[&str]) -> assert_cmd::assert::Assert {
    let empty = tempfile::tempdir().unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
        .arg(empty.path())
        .arg("--flash-file")
        .arg(file)
        .args(more)
        // A refusal takes a few seconds at most: a command that waits on what it was given ends
        // the test.
        .timeout(HANG_LIMIT)
        .assert()
}

/// What tells that a command was refused before it looked for a ROM: the name of the ROM file
/// the empty directory of [`run_with_flash_file`] does not hold.
const MISSING_ROM: &str = "esp32c3_rev3_rom.elf";

/// A file of `len` zero bytes, sparse: nothing that large is written to the disk.
fn sparse_file(path: &std::path::Path, len: u64) {
    std::fs::File::create(path).unwrap().set_len(len).unwrap();
}

/// The names in `dir`, sorted.
fn names_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Whether a file can be created in `dir`: the administrator creates one where the mode of the
/// directory says nobody can, and a test of a refusal on permissions cannot be made as that user.
#[cfg(unix)]
fn takes_a_new_file(dir: &std::path::Path) -> bool {
    let probe = dir.join("probe");
    let created = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    created
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

/// Makes a named pipe at `path`: opening one for writing waits for a reader that never comes.
#[cfg(unix)]
fn make_fifo(path: &std::path::Path) {
    nix::unistd::mkfifo(path, nix::sys::stat::Mode::S_IRWXU)
        .unwrap_or_else(|e| panic!("mkfifo {}: {e}", path.display()));
}

#[test]
fn run_help_documents_the_flash_file_its_lock_and_its_reset() {
    inkemu().args(["run", "--help"]).assert().code(0).stdout(
        contains("--flash-file <FILE>")
            .and(contains("FILE.lock"))
            .and(contains(".FILE.tmp"))
            .and(contains("by its owner only"))
            .and(contains("--reset-flash"))
            .and(contains("The image is only read"))
            .and(contains(
                "the flash file could not be written when the run ended",
            )),
    );
}

#[test]
fn a_reset_of_the_flash_without_a_flash_file_is_a_usage_error() {
    // An empty ROM directory and a limit: were the option accepted, the command would end on the
    // ROM it does not have, and never download one or run for ever.
    let empty = tempfile::tempdir().unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
        .arg(empty.path())
        .arg("--reset-flash")
        .timeout(HANG_LIMIT)
        .assert()
        .code(2)
        .stderr(
            contains("--flash-file <FILE>")
                .and(contains("required arguments were not provided"))
                .and(contains("esp32c3_rev3_rom.elf").not()),
        );
}

#[test]
fn a_flash_file_of_another_size_is_refused_before_boot_with_both_sizes_and_left_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x4.bin");
    // A firmware image given to the wrong option: with or without a reset, it must survive.
    let image = build_flash_image(&ImageSpec::default());
    std::fs::write(&file, &image).unwrap();
    for more in [&[][..], &["--reset-flash"][..]] {
        run_with_flash_file(&file, more).code(2).stderr(
            contains("x4.bin")
                .and(contains(format!("is {} bytes", image.len())))
                .and(contains("16777216 bytes"))
                .and(contains("never padded or cut"))
                .and(contains(
                    "; use it with the device it was written for, pass a firmware image with --flash instead, or give a new path\n",
                ))
                .and(contains("esp32c3_rev3_rom.elf").not()),
        );
        assert_eq!(std::fs::read(&file).unwrap(), image);
    }
}

#[test]
fn an_image_together_with_a_flash_file_that_exists_is_refused_before_boot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x4.bin");
    sparse_file(&file, X4_FLASH_BYTES);
    let image = dir.path().join("flash.bin");
    std::fs::write(&image, build_flash_image(&ImageSpec::default())).unwrap();
    run_with_flash_file(&file, &["--flash", image.to_str().unwrap()])
        .code(2)
        .stderr(
            contains("both --flash and the flash file")
                .and(contains("x4.bin"))
                .and(contains("; drop --flash to go on from the flash file"))
                .and(contains("--reset-flash"))
                .and(contains("esp32c3_rev3_rom.elf").not()),
        );
    assert_eq!(std::fs::metadata(&file).unwrap().len(), X4_FLASH_BYTES);
}

/// `--flash` is only read and the flash file is replaced: a file cannot be both. Whatever the
/// route by which the two options reach the same file, with or without a reset, the run is
/// refused before boot and the image is, byte for byte, the one it was.
#[test]
fn an_image_that_is_also_the_flash_file_is_refused_before_boot_by_every_route_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("x4.bin");
    // A dump of the whole flash: an image for `--flash` and, by its size, a flash file too.
    let mut bytes = build_flash_image(&ImageSpec::default());
    bytes.resize(usize::try_from(X4_FLASH_BYTES).unwrap(), 0xFF);
    std::fs::write(&image, &bytes).unwrap();
    let digest = sha256_of(&image);
    let mut routes = vec![
        ("the same path", image.clone(), image.clone()),
        (
            "another spelling",
            image.clone(),
            dir.path().join(".").join("x4.bin"),
        ),
    ];
    #[cfg(unix)]
    {
        let hard_link = dir.path().join("hard-link.bin");
        std::fs::hard_link(&image, &hard_link).unwrap();
        routes.push(("a hard link", hard_link, image.clone()));
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&image, &link).unwrap();
        routes.push(("a symbolic link", link, image.clone()));
    }
    let before = names_in(dir.path());
    for (route, flash, flash_file) in routes {
        for more in [&[][..], &["--reset-flash"][..]] {
            let mut arguments = vec!["--flash", flash.to_str().unwrap()];
            arguments.extend(more);
            run_with_flash_file(&flash_file, &arguments).code(2).stderr(
                contains("--flash and --flash-file name the same file")
                    .and(contains("; drop --flash to go on from the file"))
                    .and(contains("give --flash-file another path"))
                    // No remedy that would have the image replaced.
                    .and(contains("--reset-flash").not())
                    .and(contains(MISSING_ROM).not()),
            );
            assert_eq!(sha256_of(&image), digest, "{route} {more:?}");
            assert_eq!(
                names_in(dir.path()),
                before,
                "{route} {more:?}: no lock file"
            );
        }
    }
    // Two files that only hold the same bytes are two files: the refusal is the one of an
    // image beside a flash file that exists, whose remedies are sound here.
    let copy = dir.path().join("copy.bin");
    std::fs::copy(&image, &copy).unwrap();
    run_with_flash_file(&image, &["--flash", copy.to_str().unwrap()])
        .code(2)
        .stderr(contains("both --flash and the flash file").and(contains("--reset-flash")));
    run_with_flash_file(
        &image,
        &["--flash", copy.to_str().unwrap(), "--reset-flash"],
    )
    .code(2)
    .stderr(contains(MISSING_ROM));
}

/// The temporary name of a flash file is where an interrupted save leaves the flash, and the
/// one file a user would hand to `--flash` to get it back; an accepted run removes what is there.
/// The lock file is opened and locked by every run. An image at either name, by any route to it,
/// is refused before boot, and stays byte for byte what it was, with nothing created beside it.
#[test]
fn an_image_at_the_temporary_name_or_the_lock_file_of_the_flash_file_is_refused_before_boot() {
    // An image of a few kilobytes: the refusal comes before the image is read, whatever its size.
    let bytes = build_flash_image(&ImageSpec::default());
    for (owned, role) in [
        (".x4.bin.tmp", "temporary file"),
        ("x4.bin.lock", "lock file"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join(owned);
        std::fs::write(&image, &bytes).unwrap();
        let digest = sha256_of(&image);
        let mut routes = vec![
            ("the same path", image.clone()),
            ("another spelling", dir.path().join(".").join(owned)),
        ];
        #[cfg(unix)]
        {
            let hard_link = dir.path().join("hard-link.bin");
            std::fs::hard_link(&image, &hard_link).unwrap();
            routes.push(("a hard link", hard_link));
            let link = dir.path().join("link.bin");
            std::os::unix::fs::symlink(&image, &link).unwrap();
            routes.push(("a symbolic link", link));
        }
        let before = names_in(dir.path());
        for (route, flash) in routes {
            for more in [&[][..], &["--reset-flash"][..]] {
                let mut arguments = vec!["--flash", flash.to_str().unwrap()];
                arguments.extend(more);
                run_with_flash_file(&dir.path().join("x4.bin"), &arguments)
                    .code(2)
                    .stderr(
                        contains(format!("which is the {role} of the flash file"))
                            .and(contains("x4.bin"))
                            .and(contains("the image of --flash is only read"))
                            .and(contains(
                                "; copy the image to another name, then give that copy to --flash",
                            ))
                            .and(contains("inkemu: removed").not())
                            .and(contains(MISSING_ROM).not()),
                    );
                assert_eq!(sha256_of(&image), digest, "{owned}, {route} {more:?}");
                assert_eq!(
                    names_in(dir.path()),
                    before,
                    "{owned}, {route} {more:?}: nothing created, nothing removed"
                );
            }
        }
    }
}

#[test]
fn a_flash_file_that_another_run_holds_is_refused_before_boot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x4.bin");
    // This process is the other run: it holds the lock a run of inkemu takes.
    let held = emu_cli::flash_state::FlashState::open(&file, 16 << 20).unwrap();
    run_with_flash_file(&file, &[]).code(2).stderr(
        contains("is in use by another inkemu run")
            .and(contains("x4.bin.lock"))
            .and(contains("; wait for that run to end"))
            .and(contains("esp32c3_rev3_rom.elf").not()),
    );
    drop(held);
    // The lock is free again: the same command now gets as far as the ROM it does not have.
    // A lock lives as long as a copy of its descriptor does, and a test beside this one that
    // starts a process holds one for an instant, between its fork and its exec: the command is
    // tried again until that instant has passed.
    let deadline = std::time::Instant::now() + HANG_LIMIT;
    let freed = loop {
        let run = run_with_flash_file(&file, &[]);
        let still_held = String::from_utf8_lossy(&run.get_output().stderr).contains("in use");
        if !still_held || std::time::Instant::now() >= deadline {
            break run;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    freed
        .code(2)
        .stderr(contains("esp32c3_rev3_rom.elf").and(contains("in use").not()));
    assert!(!file.exists(), "a run that never started writes nothing");
}

#[test]
fn a_flash_file_in_a_directory_that_does_not_exist_is_refused_before_boot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("missing").join("x4.bin");
    run_with_flash_file(&file, &[]).code(2).stderr(
        contains("cannot create the lock file")
            .and(contains(
                "; check that the directory exists and is writable",
            ))
            .and(contains("esp32c3_rev3_rom.elf").not()),
    );
}

/// A name leaves room for the lock file and the temporary file, which are named after it with
/// five bytes more, or the run is refused: no run starts whose flash could not be saved.
#[cfg(unix)]
#[test]
fn a_flash_file_name_too_long_for_its_lock_file_is_refused_before_boot() {
    let dir = tempfile::tempdir().unwrap();
    // 255 bytes is the longest name of a file on the filesystems a test runs on.
    let longest = format!("{}.bin", "n".repeat(246));
    run_with_flash_file(&dir.path().join(&longest), &[])
        .code(2)
        .stderr(contains(MISSING_ROM));
    assert_eq!(
        names_in(dir.path()),
        [format!("{longest}.lock")],
        "taken: the lock file, and nothing left of the trial of the temporary file"
    );
    let dir = tempfile::tempdir().unwrap();
    let too_long = format!("{}.bin", "n".repeat(247));
    run_with_flash_file(&dir.path().join(too_long), &[])
        .code(2)
        .stderr(
            contains("cannot create the lock file")
                .and(contains("; give the flash file a shorter name"))
                .and(contains(MISSING_ROM).not()),
        );
    assert_eq!(names_in(dir.path()), [""; 0]);
}

/// `new.bin/` and `new.bin/.` name a directory: the content could not take that name when the
/// run ends, and a run that starts must be able to keep its flash.
#[test]
fn a_flash_file_path_that_ends_as_the_path_of_a_directory_is_refused_before_boot() {
    for ending in ["new.bin/", "new.bin/."] {
        let dir = tempfile::tempdir().unwrap();
        let path = format!("{}/{ending}", dir.path().display());
        run_with_flash_file(std::path::Path::new(&path), &[])
            .code(2)
            .stderr(
                contains("ends as the path of a directory does")
                    .and(contains("; give the path of the file itself"))
                    .and(contains(MISSING_ROM).not()),
            );
        assert_eq!(names_in(dir.path()), [""; 0], "{ending}: nothing created");
    }
}

/// A directory that takes no new file, and holds the lock file of an earlier run: the lock can
/// be taken, the flash could not be saved. Known when the run starts, said when the run starts.
#[cfg(unix)]
#[test]
fn a_flash_file_in_a_directory_that_takes_no_new_file_is_refused_before_boot() {
    let dir = tempfile::tempdir().unwrap();
    let kept_in = dir.path().join("flash");
    std::fs::create_dir(&kept_in).unwrap();
    std::fs::write(kept_in.join("x4.bin.lock"), b"").unwrap();
    set_mode(&kept_in, 0o555);
    let writable = takes_a_new_file(&kept_in);
    let refused = run_with_flash_file(&kept_in.join("x4.bin"), &[]);
    let left = names_in(&kept_in);
    set_mode(&kept_in, 0o755);
    assert!(
        !writable,
        "this test needs a user whom a read-only directory stops; run it as one"
    );
    refused.code(2).stderr(
        contains("x4.bin cannot be written when this run ends: cannot create ")
            .and(contains(".x4.bin.tmp"))
            .and(contains(
                "; check that the directory exists and is writable",
            ))
            .and(contains(MISSING_ROM).not()),
    );
    assert_eq!(left, ["x4.bin.lock"]);
}

/// The place of the lock file gets the care the flash file gets: what is there and is no
/// regular file is refused by name, without being opened. A pipe opened for writing would wait
/// for ever for a reader, and a link would have the file it names created.
#[cfg(unix)]
#[test]
fn what_is_where_the_lock_file_goes_and_is_no_regular_file_is_refused_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let refused = |name: &str| {
        // The limit: before this was refused, the command waited on the pipe for ever.
        inkemu()
            .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
            .arg(dir.path().join("no-rom"))
            .arg("--flash-file")
            .arg(dir.path().join(name))
            .timeout(std::time::Duration::from_secs(20))
            .assert()
            .code(2)
            .stderr(
                contains(format!("{name}.lock"))
                    .and(contains("is not a regular file"))
                    .and(contains("; remove it or move it away"))
                    .and(contains(MISSING_ROM).not()),
            );
    };
    make_fifo(&dir.path().join("pipe.bin.lock"));
    refused("pipe.bin");
    std::os::unix::fs::symlink(dir.path().join("planted"), dir.path().join("link.bin.lock"))
        .unwrap();
    refused("link.bin");
    std::fs::create_dir(dir.path().join("dir.bin.lock")).unwrap();
    refused("dir.bin");
    assert_eq!(
        names_in(dir.path()),
        ["dir.bin.lock", "link.bin.lock", "pipe.bin.lock"],
        "nothing was created: not what the link names, not a flash file"
    );
}

/// A lock file that is there and cannot be opened is not one that "cannot be created".
#[cfg(unix)]
#[test]
fn a_lock_file_that_cannot_be_opened_for_writing_is_said_to_be_there() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("x4.bin.lock");
    std::fs::write(&lock, b"").unwrap();
    set_mode(&lock, 0o444);
    let administrator = std::fs::OpenOptions::new().write(true).open(&lock).is_ok();
    assert!(
        !administrator,
        "this test needs a user whom a read-only file stops; run it as one"
    );
    run_with_flash_file(&dir.path().join("x4.bin"), &[])
        .code(2)
        .stderr(
            contains("cannot open for writing the lock file")
                .and(contains("x4.bin.lock"))
                .and(contains("which is there"))
                .and(contains("; make it writable"))
                .and(contains("cannot create").not())
                .and(contains(MISSING_ROM).not()),
        );
}

/// A flash file is replaced when a run ends, which the mode of the file does not prevent: one
/// its owner made read-only (an only copy) is refused, with or without a reset, and left alone.
#[cfg(unix)]
#[test]
fn a_flash_file_that_cannot_be_written_is_refused_before_boot_and_left_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x4.bin");
    sparse_file(&file, X4_FLASH_BYTES);
    set_mode(&file, 0o400);
    let administrator = std::fs::OpenOptions::new().write(true).open(&file).is_ok();
    assert!(
        !administrator,
        "this test needs a user whom a read-only file stops; run it as one"
    );
    for more in [&[][..], &["--reset-flash"][..]] {
        run_with_flash_file(&file, more).code(2).stderr(
            contains("x4.bin")
                .and(contains("cannot be opened for writing"))
                .and(contains("; make it writable for you"))
                .and(contains("--flash"))
                .and(contains(MISSING_ROM).not()),
        );
        assert_eq!(mode_of(&file), 0o400);
        assert_eq!(std::fs::metadata(&file).unwrap().len(), X4_FLASH_BYTES);
        assert_eq!(names_in(dir.path()), ["x4.bin"], "and no lock file");
    }
}

/// What sits at the name of the temporary file is removed by a run that runs, never by one that
/// is refused, whatever refuses it: here the flash file was taken and the ROM is missing. (The
/// removal by a run that runs, and what it says, is in the test of a save that was killed.)
#[test]
fn a_run_that_is_refused_leaves_what_sits_at_the_temporary_name_of_its_flash_file() {
    let dir = tempfile::tempdir().unwrap();
    let left = dir.path().join(".x4.bin.tmp");
    std::fs::write(&left, b"a part of a flash").unwrap();
    run_with_flash_file(&dir.path().join("x4.bin"), &[])
        .code(2)
        .stderr(contains(MISSING_ROM).and(contains("removed").not()));
    assert_eq!(names_in(dir.path()), [".x4.bin.tmp", "x4.bin.lock"]);
    assert_eq!(std::fs::read(&left).unwrap(), b"a part of a flash");
}

/// A run refused for what sits at the path of its flash file creates nothing beside it.
#[test]
fn a_run_refused_for_what_is_at_the_flash_file_path_leaves_no_lock_file_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x4.bin");
    let refused_as = |cause: &str, more: &[&str]| {
        let before = names_in(dir.path());
        run_with_flash_file(&file, more)
            .code(2)
            .stderr(contains(cause).and(contains(MISSING_ROM).not()));
        assert_eq!(names_in(dir.path()), before, "{cause}");
    };
    std::fs::create_dir(&file).unwrap();
    refused_as("is not a regular file", &[]);
    std::fs::remove_dir(&file).unwrap();
    // A firmware image given to the wrong option.
    std::fs::write(&file, build_flash_image(&ImageSpec::default())).unwrap();
    refused_as("never padded or cut", &[]);
    refused_as("never padded or cut", &["--reset-flash"]);
    std::fs::remove_file(&file).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("elsewhere.bin"), &file).unwrap();
        refused_as("a link is not followed", &[]);
        std::fs::remove_file(&file).unwrap();
        make_fifo(&file);
        refused_as("is not a regular file", &[]);
    }
}

/// Code that waits until the transmit FIFO of UART0 is empty. A second stage that prints must
/// start with it: the ROM has just filled the FIFO with its last lines, and a byte written to a
/// full FIFO is lost, on the device as here. `UART.STATUS` [25:16] is `TXFIFO_CNT`: the upper
/// half of the register, modulo 1024.
fn wait_for_uart0_to_drain() -> [u32; 5] {
    use cpu_rv32::asm::{addi, bne, lhu, lui, remu};
    use soc_esp32c3::test_support::{T0, T1};
    const T2: u8 = 7;
    let status = svd_regs::esp32c3::addr("UART0", "STATUS");
    let page = status & 0xFFFF_F000;
    [
        lui(T0, page),
        addi(T2, 0, 1024),
        lhu(T1, T0, (status - page + 2).cast_signed()),
        remu(T1, T1, T2),
        bne(T1, 0, -8),
    ]
}

/// A second stage that keeps in the flash the number of times it booted, and prints it: `boot 1`
/// on an erased flash, `boot 2` on the flash that boot left, and so on. The count is the byte at
/// flash offset 0x20000, past everything the images built here hold; each boot erases its sector
/// and programs the next value. The byte is read through the data window (MMU entry 0), once,
/// before it is written.
fn counting_its_boots() -> Vec<u32> {
    counting_its_boots_then(&[HANG])
}

/// [`counting_its_boots`] that goes on with `then` once the count is in the flash, instead of
/// spinning: a run that has modified its flash and ends the way `then` makes it end.
fn counting_its_boots_then(then: &[u32]) -> Vec<u32> {
    use cpu_rv32::asm::{addi, lbu, lui, sb, sw};
    use soc_esp32c3::test_support::{T0, flash_erase_sector, print_program};
    const A0: u8 = 10;
    const A1: u8 = 11;
    const S0: u8 = 8;
    const COUNT_AT: u32 = 0x2_0000;
    let spi1 = |register| svd_regs::esp32c3::addr("SPI1", register);
    let mut program = stop_watchdogs();
    program.extend(store_register(
        soc_esp32c3::periph::mmu::MMU_TABLE_BASE,
        COUNT_AT >> 16,
    ));
    program.extend([
        lui(S0, soc_esp32c3::memmap::FLASH_DBUS_BASE),
        lbu(A0, S0, 0), // the count the flash holds: 0xFF when erased
        addi(A0, A0, 1),
        // Through a byte of RAM, to keep the low byte: 0xFF + 1 is the count 0.
        lui(S0, soc_esp32c3::memmap::SRAM_DBUS_BASE),
        sb(A0, S0, 0),
        lbu(A0, S0, 0),
    ]);
    program.extend(wait_for_uart0_to_drain());
    program.extend(print_program(
        svd_regs::esp32c3::addr("UART0", "FIFO"),
        b"boot ",
    ));
    // `T0` still holds the address of the FIFO: the digit, then the end of the line.
    program.extend([
        addi(A1, A0, i32::from(b'1')),
        sw(A1, T0, 0),
        addi(A1, 0, i32::from(b'\n')),
        sw(A1, T0, 0),
    ]);
    program.extend(flash_erase_sector(COUNT_AT));
    // Write enable, the count in `W0`, one byte of it programmed at `COUNT_AT`.
    program.extend(store_register(spi1("CMD"), 1 << 30));
    let w0 = spi1("W0");
    let page = w0.wrapping_add(0x800) & 0xFFFF_F000;
    program.extend([
        lui(T0, page),
        sw(A0, T0, w0.wrapping_sub(page).cast_signed()),
    ]);
    program.extend(store_register(spi1("ADDR"), (1 << 24) | COUNT_AT));
    program.extend(store_register(spi1("CMD"), 1 << 25));
    program.extend_from_slice(then);
    program
}

/// What a run of [`counting_its_boots`] left: its exit code, its console and its log.
struct Counted {
    code: Option<i32>,
    console: String,
    log: String,
}

impl Counted {
    /// The hexadecimal digest of the summary line that starts with `prefix`.
    fn sha256(&self, prefix: &str) -> String {
        self.log
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("no line `{prefix}` in:\n{}", self.log))
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect()
    }
}

const FLASH_AT_START: &str = "inkemu: flash at start: sha256 ";
const FLASH_AT_END: &str = "inkemu: flash at end: sha256 ";

/// Runs `inkemu run --device x4 --strict-mmio --trace-hash` with the real ROM and `more`, for the
/// cycles the ROM and [`counting_its_boots`] need.
fn count_a_boot(more: &[&std::ffi::OsStr]) -> Counted {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let output = inkemu()
        .args(["run", "--device", "x4", "--strict-mmio", "--trace-hash"])
        .args(["--max-cycles", "5000000", "--rom-dir", &rom_dir])
        .args(more)
        .timeout(HANG_LIMIT)
        .output()
        .unwrap();
    Counted {
        code: output.status.code(),
        console: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
        log: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn sha256_of(path: &std::path::Path) -> String {
    use sha2::Digest as _;
    emu_cli::term::hex(&sha2::Sha256::digest(std::fs::read(path).unwrap()))
}

/// Persistence through the real command: the flash file is created by the first run, the second
/// run goes on from it, an image is refused next to it, and a reset starts again.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn the_flash_file_keeps_the_flash_of_the_device_from_one_run_to_the_next() {
    let (dir, image) = flash_image_of(&counting_its_boots());
    let image_bytes = std::fs::read(&image).unwrap();
    let file = dir.path().join("x4.bin");
    let (flash, flash_file) = (
        std::ffi::OsStr::new("--flash"),
        std::ffi::OsStr::new("--flash-file"),
    );

    let first = count_a_boot(&[flash, image.as_os_str(), flash_file, file.as_os_str()]);
    assert_eq!(first.code, Some(0), "{}", first.log);
    assert!(
        first.console.ends_with("entry 0x40380000\nboot 1\n"),
        "{}",
        first.console
    );
    assert!(
        first.log.contains("flash: flash.bin (") && first.log.contains("), kept in x4.bin\n"),
        "{}",
        first.log
    );
    assert_eq!(std::fs::metadata(&file).unwrap().len(), X4_FLASH_BYTES);
    assert_eq!(first.sha256(FLASH_AT_END), sha256_of(&file));
    assert!(
        first
            .log
            .contains("; 1 of its 4 KiB sectors was modified; saved to "),
        "{}",
        first.log
    );
    assert!(!first.log.contains("warning:"), "{}", first.log);
    assert_eq!(
        std::fs::read(&image).unwrap(),
        image_bytes,
        "--flash is only read"
    );

    // The second run is given the file alone: a power-on from the flash the first one left.
    let second = count_a_boot(&[flash_file, file.as_os_str()]);
    assert_eq!(second.code, Some(0), "{}", second.log);
    assert!(
        second.console.ends_with("entry 0x40380000\nboot 2\n"),
        "{}",
        second.console
    );
    assert!(
        second.log.contains("flash: x4.bin (flash file, 16 MiB)\n"),
        "{}",
        second.log
    );
    assert_eq!(
        second.sha256(FLASH_AT_START),
        first.sha256(FLASH_AT_END),
        "what the second run starts from is what the first one saved"
    );
    assert_eq!(second.sha256(FLASH_AT_END), sha256_of(&file));
    assert_ne!(second.sha256(FLASH_AT_END), second.sha256(FLASH_AT_START));
    let after_second = std::fs::read(&file).unwrap();
    assert_eq!(after_second[0x2_0000], 1, "the count of two boots, from 0");

    // The image next to the file that exists: refused, and nothing is written.
    let refused = count_a_boot(&[flash, image.as_os_str(), flash_file, file.as_os_str()]);
    assert_eq!(refused.code, Some(2), "{}", refused.log);
    assert!(
        refused.log.contains("both --flash and the flash file"),
        "{}",
        refused.log
    );
    assert!(
        refused.console.is_empty(),
        "nothing ran: {}",
        refused.console
    );
    assert_eq!(sha256_of(&file), second.sha256(FLASH_AT_END));

    // A reset starts again from the image and replaces the file.
    let reset = count_a_boot(&[
        flash,
        image.as_os_str(),
        flash_file,
        file.as_os_str(),
        std::ffi::OsStr::new("--reset-flash"),
    ]);
    assert_eq!(reset.code, Some(0), "{}", reset.log);
    assert!(reset.console.ends_with("boot 1\n"), "{}", reset.console);
    assert_eq!(reset.sha256(FLASH_AT_END), first.sha256(FLASH_AT_END));
    assert_eq!(sha256_of(&file), first.sha256(FLASH_AT_END));
    assert_eq!(
        std::fs::read(&image).unwrap(),
        image_bytes,
        "--flash is only read"
    );
    let mut names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["flash.bin", "x4.bin", "x4.bin.lock"],
        "and nothing else is left"
    );
}

/// Without a flash file nothing is kept, the summary says how much, and the image is not written.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_run_without_a_flash_file_says_what_it_does_not_keep_and_leaves_the_image_as_it_is() {
    let (dir, image) = flash_image_of(&counting_its_boots());
    let image_bytes = std::fs::read(&image).unwrap();
    let flash = std::ffi::OsStr::new("--flash");
    for _ in 0..2 {
        let run = count_a_boot(&[flash, image.as_os_str()]);
        assert_eq!(run.code, Some(0), "{}", run.log);
        assert!(
            run.console.ends_with("boot 1\n"),
            "every run is the first: {}",
            run.console
        );
        assert!(
            run.log.contains(
                "inkemu: flash at end: 1 of its 4 KiB sectors was modified and is not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next\n"
            ),
            "{}",
            run.log
        );
        assert!(!run.log.contains(FLASH_AT_END), "{}", run.log);
        assert_eq!(std::fs::read(&image).unwrap(), image_bytes);
    }
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "the image alone"
    );
}

/// Determinism with a flash file: the content a run starts from is an input, and nothing else
/// about the file is. Two runs from the same content have the same trace and leave the same file.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn the_same_starting_flash_file_gives_the_same_trace_and_the_same_final_file() {
    let (dir, image) = flash_image_of(&counting_its_boots());
    let (flash, flash_file) = (
        std::ffi::OsStr::new("--flash"),
        std::ffi::OsStr::new("--flash-file"),
    );
    // A flash file to start from: the one a first run leaves.
    let origin = dir.path().join("origin.bin");
    let made = count_a_boot(&[flash, image.as_os_str(), flash_file, origin.as_os_str()]);
    assert_eq!(made.code, Some(0), "{}", made.log);
    let run_from_a_copy = |name: &str| {
        let file = dir.path().join(name);
        std::fs::copy(&origin, &file).unwrap();
        let run = count_a_boot(&[flash_file, file.as_os_str()]);
        assert_eq!(run.code, Some(0), "{}", run.log);
        assert!(run.console.ends_with("boot 2\n"), "{}", run.console);
        (run, sha256_of(&file))
    };
    let (one, file_one) = run_from_a_copy("one.bin");
    let (two, file_two) = run_from_a_copy("two.bin");
    assert!(trace_hash_line(&one.log).is_some(), "{}", one.log);
    assert_eq!(trace_hash_line(&one.log), trace_hash_line(&two.log));
    assert_eq!(one.console, two.console);
    assert_eq!(file_one, file_two, "the same final flash file");
    assert_eq!(one.sha256(FLASH_AT_END), file_one);
    assert_eq!(one.sha256(FLASH_AT_START), sha256_of(&origin));
    // The first run of the image, with and without a flash file, is the same run: keeping the
    // flash is no part of the trace.
    let without = count_a_boot(&[flash, image.as_os_str()]);
    assert_eq!(trace_hash_line(&without.log), trace_hash_line(&made.log));
    assert_eq!(without.sha256(FLASH_AT_START), made.sha256(FLASH_AT_START));
    // The trace hash is the path the CPU took, and this firmware takes the same path whatever
    // count it reads: the second boot has the trace hash of the first. What tells the two runs
    // apart is the flash each started from, which is why the summary says it.
    assert_eq!(trace_hash_line(&without.log), trace_hash_line(&one.log));
    assert_ne!(without.sha256(FLASH_AT_START), one.sha256(FLASH_AT_START));
    assert_ne!(without.console, one.console);
}

/// A second stage that stops the watchdogs, prints `ready` and spins: a run that goes on until
/// something from outside ends it.
#[cfg(unix)]
fn ready_then_busy() -> Vec<u32> {
    let mut program = stop_watchdogs();
    program.extend(wait_for_uart0_to_drain());
    program.extend(soc_esp32c3::test_support::print_program(
        svd_regs::esp32c3::addr("UART0", "FIFO"),
        b"ready\n",
    ));
    program.push(HANG);
    program
}

/// [`run_of`] with a flash file.
#[cfg(unix)]
fn run_with_file(
    rom_dir: &str,
    image: &std::path::Path,
    file: &std::path::Path,
) -> std::process::Command {
    let mut command = run_of(rom_dir, image);
    command.arg("--flash-file").arg(file);
    command
}

/// Two processes on one flash file: the second is refused while the first runs, and the first
/// Ctrl-C ends the first with its flash file written.
#[cfg(unix)]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_second_run_on_a_flash_file_is_refused_while_the_first_goes_on_and_ctrl_c_saves_it() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (dir, image) = flash_image_of(&ready_then_busy());
    let file = dir.path().join("x4.bin");
    let limit = HANG_LIMIT;
    let first = live::start(run_with_file(&rom_dir, &image, &file), |_| false);
    first.wait_for_line("ready", limit);
    assert!(
        !file.exists(),
        "the flash file is not written while the run goes on"
    );

    // A limit: were the second run let in, it would end by itself, with exit code 0, instead of
    // spinning beside the first for ever.
    let second = run_with_file(&rom_dir, &image, &file)
        .args(["--max-cycles", "3000000"])
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(2));
    let refusal = String::from_utf8_lossy(&second.stderr).into_owned();
    assert!(
        refusal.contains("is in use by another inkemu run") && refusal.contains("x4.bin.lock"),
        "{refusal}"
    );
    assert!(second.stdout.is_empty(), "the second run never started");
    assert!(!file.exists(), "and it wrote nothing");

    first.interrupt();
    let (code, log) = first.finish(limit);
    assert_eq!(code, Some(0), "{log}");
    assert!(
        log.contains("inkemu: interrupted by the user\n")
            && log.contains("; 0 of its 4 KiB sectors were modified; saved to "),
        "{log}"
    );
    assert_eq!(std::fs::metadata(&file).unwrap().len(), X4_FLASH_BYTES);
    // The first run is over: its lock is free, and the file it left is taken as it is.
    let third = live::start(
        {
            let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_inkemu"));
            command
                .args([
                    "run",
                    "--device",
                    "x4",
                    "--rom-dir",
                    &rom_dir,
                    "--flash-file",
                ])
                .arg(&file);
            command
        },
        |_| false,
    );
    third.wait_for_line("ready", limit);
    third.interrupt();
    let (code, log) = third.finish(limit);
    assert_eq!(code, Some(0), "{log}");
    assert!(
        log.contains("inkemu: flash at end: not modified; "),
        "{log}"
    );
}

/// Once the run has returned, Ctrl-C ends nothing: a second `SIGINT`, sent as soon as the first
/// line after the run is printed, leaves the summary and the save of the flash file to complete,
/// and the exit code is the one of the first Ctrl-C. The flash file is new here, so the save
/// writes and flushes the whole 16 MiB after that line.
///
/// The test cannot fail on a sound binary, whenever the signal arrives: the line comes after the
/// run has returned. What it cannot do is make sure the signal arrives before the save ends; a
/// handler that ended the process there would fail it only when it does (the save takes tens of
/// milliseconds, the signal one or two). The rule itself is held by the unit tests of `run.rs`.
#[cfg(unix)]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_second_ctrl_c_once_the_run_has_returned_leaves_the_save_and_the_summary_to_complete() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (dir, image) = flash_image_of(&ready_then_busy());
    let file = dir.path().join("x4.bin");
    let limit = HANG_LIMIT;
    let run = live::start(run_with_file(&rom_dir, &image, &file), |_| false);
    run.wait_for_line("ready", limit);
    run.interrupt();
    run.wait_for_log_line("inkemu: flash at start: sha256 ", limit);
    run.interrupt();
    let (code, log) = run.finish(limit);
    assert_eq!(code, Some(0), "{log}");
    assert!(
        log.contains("inkemu: interrupted by the user\n")
            && log.contains("; 0 of its 4 KiB sectors were modified; saved to "),
        "{log}"
    );
    assert_eq!(std::fs::metadata(&file).unwrap().len(), X4_FLASH_BYTES);
    assert_eq!(
        names_in(dir.path()),
        ["flash.bin", "x4.bin", "x4.bin.lock"],
        "no temporary file is left"
    );
}

/// The flash file cannot be written when the run ends, because its directory is gone: the run
/// says so, says what is lost, and exits 2.
#[cfg(unix)]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_flash_file_whose_directory_vanished_during_the_run_exits_2_and_says_what_is_lost() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (dir, image) = flash_image_of(&ready_then_busy());
    let kept_in = dir.path().join("flash");
    std::fs::create_dir(&kept_in).unwrap();
    let limit = HANG_LIMIT;
    let run = live::start(
        run_with_file(&rom_dir, &image, &kept_in.join("x4.bin")),
        |_| false,
    );
    run.wait_for_line("ready", limit);
    std::fs::remove_dir_all(&kept_in).unwrap();
    run.interrupt();
    let (code, log) = run.finish(limit);
    assert_eq!(code, Some(2), "{log}");
    assert!(
        log.contains("\ninkemu: stopped at pc ")
            && log.contains("\nerror: cannot write the flash file ")
            && log.contains(
                "what this run wrote to the flash is lost; check that the directory still exists"
            ),
        "the summary, then the error: {log}"
    );
}

/// The same with a directory that became read-only: nothing can be created in it.
#[cfg(unix)]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_flash_file_whose_directory_became_read_only_exits_2_and_leaves_no_file() {
    use std::os::unix::fs::PermissionsExt as _;
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (dir, image) = flash_image_of(&ready_then_busy());
    let kept_in = dir.path().join("flash");
    std::fs::create_dir(&kept_in).unwrap();
    let read_only = |yes: bool| {
        let mode = if yes { 0o555 } else { 0o755 };
        std::fs::set_permissions(&kept_in, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    // The administrator writes where the mode says nobody can: the case cannot be made then.
    read_only(true);
    let probe = std::fs::File::create(kept_in.join("probe")).is_ok();
    read_only(false);
    assert!(
        !probe,
        "this test needs a user whom a read-only directory stops; run it as one"
    );
    let limit = HANG_LIMIT;
    let run = live::start(
        run_with_file(&rom_dir, &image, &kept_in.join("x4.bin")),
        |_| false,
    );
    run.wait_for_line("ready", limit);
    read_only(true);
    run.interrupt();
    let (code, log) = run.finish(limit);
    read_only(false);
    assert_eq!(code, Some(2), "{log}");
    assert!(
        log.contains("\nerror: cannot write the flash file ") && log.contains("cannot create "),
        "{log}"
    );
    let left: Vec<_> = std::fs::read_dir(&kept_in)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left, ["x4.bin.lock"], "no flash file, no temporary file");
}

/// What sits at the name of the temporary file is looked at once nothing else can refuse the
/// run. A directory there cannot be removed, the flash could not be saved through that name:
/// the run is refused before its first instruction, with what to remove.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_directory_at_the_temporary_name_refuses_the_run_before_its_first_instruction() {
    let (dir, image) = flash_image_of(&counting_its_boots());
    let file = dir.path().join("x4.bin");
    std::fs::create_dir(dir.path().join(".x4.bin.tmp")).unwrap();
    let refused = count_a_boot(&[
        "--flash".as_ref(),
        image.as_os_str(),
        "--flash-file".as_ref(),
        file.as_os_str(),
    ]);
    assert_eq!(refused.code, Some(2), "{}", refused.log);
    assert!(
        refused.console.is_empty(),
        "nothing ran: {}",
        refused.console
    );
    assert!(
        refused
            .log
            .contains("x4.bin cannot be written when this run ends: cannot remove ")
            && refused.log.contains(".x4.bin.tmp")
            && refused.log.contains("; remove it, then run again")
            && !refused.log.contains("inkemu: "),
        "the refusal and nothing of a run: {}",
        refused.log
    );
    assert_eq!(
        names_in(dir.path()),
        [".x4.bin.tmp", "flash.bin", "x4.bin.lock"],
        "the directory is left, and no flash file was written"
    );
}

/// `inkemu run --device x4` with the real ROM and `more`, to its end.
fn run_x4(more: &[&std::ffi::OsStr]) -> Counted {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let output = inkemu()
        .args(["run", "--device", "x4", "--rom-dir", &rom_dir])
        .args(more)
        .timeout(HANG_LIMIT)
        .output()
        .unwrap();
    Counted {
        code: output.status.code(),
        console: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
        log: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// What holds after the first run of [`counting_its_boots_then`] on a flash file, however that
/// run ended: the summary says the file was saved, the file is the flash the run left, and a
/// second run on it counts on.
fn assert_the_first_boot_was_kept(log: &str, file: &std::path::Path) {
    assert!(
        log.contains("; 1 of its 4 KiB sectors was modified; saved to "),
        "{log}"
    );
    let saved = std::fs::read(file).unwrap_or_else(|e| panic!("no flash file: {e}\n{log}"));
    assert_eq!(saved.len() as u64, X4_FLASH_BYTES);
    assert_eq!(saved[0x2_0000], 0, "the count of one boot, from 0");
    let said = log
        .lines()
        .find_map(|line| line.strip_prefix(FLASH_AT_END))
        .unwrap_or_else(|| panic!("no line `{FLASH_AT_END}` in:\n{log}"));
    assert!(said.starts_with(&sha256_of(file)), "{log}");
    let next = count_a_boot(&[std::ffi::OsStr::new("--flash-file"), file.as_os_str()]);
    assert!(next.console.contains("\nboot 2\n"), "{}", next.console);
}

/// A `--strict-mmio` stop is an end of the run like the others for its flash: what the firmware
/// wrote before the stop is saved, and the exit code is the one of the stop.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_strict_stop_saves_the_flash_file_and_exits_3() {
    let mut then = load_register(unmodeled_register().addr).to_vec();
    then.push(HANG);
    let (dir, image) = flash_image_of(&counting_its_boots_then(&then));
    let file = dir.path().join("x4.bin");
    let run = run_x4(&[
        "--strict-mmio".as_ref(),
        "--flash".as_ref(),
        image.as_os_str(),
        "--flash-file".as_ref(),
        file.as_os_str(),
    ]);
    assert_eq!(run.code, Some(3), "{}", run.log);
    assert!(
        run.log.contains("inkemu: stopped by --strict-mmio: "),
        "{}",
        run.log
    );
    assert!(run.console.ends_with("boot 1\n"), "{}", run.console);
    assert_the_first_boot_was_kept(&run.log, &file);
}

/// A CPU that waits for an interrupt nothing can raise ends the run by itself, with no limit
/// given: the flash is saved then too, and the exit code is the one of that end.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_cpu_that_waits_for_ever_saves_the_flash_file_and_exits_4() {
    let (dir, image) = flash_image_of(&counting_its_boots_then(&[wfi(), HANG]));
    let file = dir.path().join("x4.bin");
    let run = run_x4(&[
        "--flash".as_ref(),
        image.as_os_str(),
        "--flash-file".as_ref(),
        file.as_os_str(),
    ]);
    assert_eq!(run.code, Some(4), "{}", run.log);
    assert!(
        run.log
            .contains("inkemu: the CPU waits for an interrupt and none can come"),
        "{}",
        run.log
    );
    assert!(run.console.ends_with("boot 1\n"), "{}", run.console);
    assert_the_first_boot_was_kept(&run.log, &file);
}

/// `inkemu run --flash-file x4.bin | head`: the reader that goes away ends the run, and the
/// flash is saved. The firmware says `kept` once the count is in the flash, and the reader
/// leaves on that line.
#[cfg(unix)]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_reader_that_goes_away_ends_the_run_with_its_flash_file_saved() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let mut then = wait_for_uart0_to_drain().to_vec();
    then.extend(soc_esp32c3::test_support::print_program(
        svd_regs::esp32c3::addr("UART0", "FIFO"),
        b"kept\n",
    ));
    then.push(HANG);
    let (dir, image) = flash_image_of(&counting_its_boots_then(&then));
    let file = dir.path().join("x4.bin");
    let run = live::start(run_with_file(&rom_dir, &image, &file), |line| {
        line.contains("kept")
    });
    let limit = HANG_LIMIT;
    run.wait_for_line("kept", limit);
    let (code, log) = run.finish(limit);
    assert_eq!(code, Some(0), "{log}");
    assert!(
        log.contains("inkemu: the reader of standard output went away\n"),
        "{log}"
    );
    assert_the_first_boot_was_kept(&log, &file);
}

/// A console that cannot be written (`> /dev/full`) ends the run with exit code 2, and the
/// flash is saved all the same. The console of this run is UART1, on which the ROM prints
/// nothing: the first byte for standard output is the one the firmware sends once the count is
/// in the flash.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_console_that_cannot_be_written_ends_the_run_with_its_flash_file_saved() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let mut then =
        soc_esp32c3::test_support::print_program(svd_regs::esp32c3::addr("UART1", "FIFO"), b"x\n");
    then.push(HANG);
    let (dir, image) = flash_image_of(&counting_its_boots_then(&then));
    let file = dir.path().join("x4.bin");
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    // A limit: were the write to succeed, the firmware would spin for ever.
    let output = run_with_file(&rom_dir, &image, &file)
        .args(["--console", "uart1", "--max-cycles", "20000000"])
        .stdin(std::process::Stdio::null())
        .stdout(full)
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(2), "{log}");
    assert!(
        log.contains("error: cannot write the console output: ")
            && !log.contains("cycle limit reached"),
        "{log}"
    );
    assert_the_first_boot_was_kept(&log, &file);
}

/// A second stage that prints `ready` on the USB serial port, then some 128 000 bytes more,
/// then reads a register the emulator does not model. The bytes are twice what a pipe holds:
/// the run cannot reach the register, nor end, before its reader has read past `ready`.
#[cfg(unix)]
fn ready_then_much_output_then_an_unmodelled_register() -> Vec<u32> {
    use cpu_rv32::asm::{addi, bne};
    use soc_esp32c3::test_support::print_program;
    const S0: u8 = 8;
    /// A full-speed packet: the port sends it as soon as it is full.
    const PACKET: usize = 64;
    const PACKETS: i32 = 2000;
    let port = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
    let mut first = b"ready\n".to_vec();
    first.resize(PACKET - 1, b'.');
    first.push(b'\n');
    let mut filler = vec![b'.'; PACKET - 1];
    filler.push(b'\n');
    let mut program = stop_watchdogs();
    program.extend(print_program(port, &first));
    program.push(addi(S0, 0, PACKETS));
    let packet = print_program(port, &filler);
    // Back over the packet and the decrement, to the first instruction of the packet.
    let back = -4 * (i32::try_from(packet.len()).unwrap() + 1);
    program.extend(packet);
    program.extend([addi(S0, S0, -1), bne(S0, 0, back)]);
    program.extend(load_register(unmodeled_register().addr));
    program.push(HANG);
    program
}

/// A `--strict-mmio` stop keeps exit code 3 when the flash file cannot be written: that code is
/// for CI, and must not depend on the state of a directory. The error is printed all the same.
///
/// The directory of the flash file is removed by the reader of the run, when it reads `ready`
/// and before it reads anything else: the firmware then has more to print than a pipe holds,
/// so the run can neither stop nor save before the directory is gone.
#[cfg(unix)]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_strict_stop_whose_flash_file_cannot_be_written_keeps_exit_code_3_and_says_what_is_lost() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (dir, image) = flash_image_of(&ready_then_much_output_then_an_unmodelled_register());
    let kept_in = dir.path().join("flash");
    std::fs::create_dir(&kept_in).unwrap();
    let mut command = run_with_file(&rom_dir, &image, &kept_in.join("x4.bin"));
    command.args(["--strict-mmio", "--console", "usb-serial-jtag"]);
    let run = live::start(command, move |line| {
        if line.contains("ready") {
            std::fs::remove_dir_all(&kept_in).unwrap();
        }
        false
    });
    let (code, log) = run.finish(HANG_LIMIT);
    assert_eq!(code, Some(3), "{log}");
    assert!(
        log.contains("inkemu: stopped by --strict-mmio: ")
            && log.contains("\nerror: cannot write the flash file ")
            && log.contains(
                "what this run wrote to the flash is lost; check that the directory still exists"
            ),
        "the stop, then the error: {log}"
    );
}

/// A file-size limit of the host kills the process while it writes the flash file (`SIGXFSZ`):
/// no message can be printed. The flash file is then the one of before, the part that was
/// written stays under the temporary name, and the next run on the file removes it and goes on.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_save_killed_by_a_file_size_limit_leaves_the_flash_file_of_before_and_the_next_run_cleans_up() {
    use std::os::unix::process::ExitStatusExt as _;
    const SIGXFSZ: i32 = 25;
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (dir, image) = flash_image_of(&counting_its_boots());
    let file = dir.path().join("x4.bin");
    let flash_file = std::ffi::OsStr::new("--flash-file");
    let first = count_a_boot(&[
        "--flash".as_ref(),
        image.as_os_str(),
        flash_file,
        file.as_os_str(),
    ]);
    assert_eq!(first.code, Some(0), "{}", first.log);
    let before = sha256_of(&file);

    // 2048 blocks of at most 1024 bytes: far less than the 16 MiB of the flash, and more than
    // anything else the command writes. No core file either, whose size the limit would bound.
    let killed = std::process::Command::new("sh")
        .args(["-c", r#"ulimit -c 0; ulimit -f 2048; exec "$0" "$@""#])
        .arg(env!("CARGO_BIN_EXE_inkemu"))
        .args(["run", "--device", "x4", "--strict-mmio"])
        .args(["--max-cycles", "5000000", "--rom-dir", &rom_dir])
        .arg(flash_file)
        .arg(&file)
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&killed.stderr).into_owned();
    assert_eq!(killed.status.signal(), Some(SIGXFSZ), "{log}");
    assert!(
        log.contains(FLASH_AT_START) && !log.contains("flash at end"),
        "the run was over, its flash was being saved: {log}"
    );
    assert_eq!(sha256_of(&file), before, "the flash file of before");
    assert_eq!(
        names_in(dir.path()),
        [".x4.bin.tmp", "flash.bin", "x4.bin", "x4.bin.lock"],
        "the part that was written stays"
    );

    // What the killed run had written to its flash is lost: this one is the second boot again.
    let next = count_a_boot(&[flash_file, file.as_os_str()]);
    assert_eq!(next.code, Some(0), "{}", next.log);
    assert!(next.console.ends_with("boot 2\n"), "{}", next.console);
    assert_eq!(next.sha256(FLASH_AT_START), before);
    assert_eq!(next.sha256(FLASH_AT_END), sha256_of(&file));
    assert_eq!(
        names_in(dir.path()),
        ["flash.bin", "x4.bin", "x4.bin.lock"],
        "the part left by the killed save is gone"
    );
    // The run says what it removed and why, before anything else, and that nothing is to be done.
    let said = next.log.lines().next().unwrap_or_default();
    assert!(
        said.starts_with("inkemu: removed ")
            && said.contains(
                ".x4.bin.tmp, found at the name where a save of the flash file ",
            )
            && said.contains(
                "x4.bin puts its temporary file (an interrupted save leaves one there); nothing to do: ",
            ),
        "{}",
        next.log
    );
}

/// `flash_nvs.bin` of `tools/reference-firmware/build.sh` and the whole flash after the first and
/// the second life of its application (docs/research/esp-idf-storage-flash-traffic.md, §A.3, §B.2).
const NVS_IMAGE_SHA256: &str = "c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f";
const NVS_AFTER_LIFE_1: &str = "cfab4ffe5187eeea8ec2ee6e9794886289282f0242ce56aad2842144a6ffb17b";
const NVS_AFTER_LIFE_2: &str = "7ad0acf62e5a4e20aa6d47be0f1d922a7b89cb204941feac1ab2fea71363b709";

/// The milestone of the flash file through the real command, with a firmware nobody wrote for
/// the test: ESP-IDF's NVS example keeps a restart counter, and a second `inkemu run` on the
/// same flash file goes on counting where the first one stopped. One life of the example is
/// 11.2 s of device time; its writes are done in the first second.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_FLASH"]
fn the_nvs_reference_firmware_goes_on_counting_in_a_second_run_on_the_same_flash_file() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let image = std::path::PathBuf::from(pinned_image(
        "INKEMU_NVS_FLASH",
        "flash_nvs.bin",
        NVS_IMAGE_SHA256,
    ));
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x4.bin");
    let run = |seconds: &str, flash: &[&std::ffi::OsStr]| {
        let output = inkemu()
            .args(["run", "--device", "x4", "--strict-mmio", "--trace-hash"])
            .args(["--max-time", seconds, "--rom-dir", &rom_dir])
            .args(flash)
            .arg("--flash-file")
            .arg(&file)
            .timeout(HANG_LIMIT)
            .output()
            .unwrap();
        Counted {
            code: output.status.code(),
            console: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
            log: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    };

    // First run: from the image, five seconds of the first life.
    let first = run("5", &[std::ffi::OsStr::new("--flash"), image.as_os_str()]);
    assert_eq!(first.code, Some(0), "{}", first.log);
    assert!(
        first.console.contains("rst:0x1 (POWERON)")
            && first.console.contains(
                "Reading restart counter from NVS ... The value is not initialized yet!\n"
            )
            && !first.console.contains("Restart counter = "),
        "{}",
        first.console
    );
    assert_eq!(first.sha256(FLASH_AT_START), NVS_IMAGE_SHA256);
    assert_eq!(first.sha256(FLASH_AT_END), NVS_AFTER_LIFE_1);
    assert_eq!(sha256_of(&file), NVS_AFTER_LIFE_1);
    assert!(!first.log.contains("warning:"), "{}", first.log);

    // Second run: the file alone. A power-on, and the counter the first run wrote.
    let second = run("5", &[]);
    assert_eq!(second.code, Some(0), "{}", second.log);
    assert!(
        second.console.contains("rst:0x1 (POWERON)")
            && !second.console.contains("rst:0xc")
            && second
                .console
                .contains("Reading restart counter from NVS ... Done\nRestart counter = 1\n")
            && !second.console.contains("not initialized"),
        "{}",
        second.console
    );
    assert_eq!(second.sha256(FLASH_AT_START), NVS_AFTER_LIFE_1);
    assert_eq!(second.sha256(FLASH_AT_END), NVS_AFTER_LIFE_2);
    assert_eq!(sha256_of(&file), NVS_AFTER_LIFE_2);
    assert!(!second.log.contains("warning:"), "{}", second.log);
    // This firmware takes another path when it finds a counter: the two runs differ in their
    // trace as in the flash they started from.
    assert!(trace_hash_line(&first.log).is_some(), "{}", first.log);
    assert_ne!(trace_hash_line(&first.log), trace_hash_line(&second.log));

    // Third run: long enough for a restart. The counter goes on inside the run, then across it.
    let third = run("12.5", &[]);
    assert_eq!(third.code, Some(0), "{}", third.log);
    let counted = third
        .console
        .find("Restart counter = 2\n")
        .unwrap_or_else(|| panic!("no second count in:\n{}", third.console));
    let after_the_restart = &third.console[counted..];
    assert!(
        after_the_restart.contains("rst:0xc (RTC_SW_CPU_RST)")
            && after_the_restart.contains("Restart counter = 3\n"),
        "{}",
        third.console
    );
    assert_eq!(third.sha256(FLASH_AT_START), NVS_AFTER_LIFE_2);
    assert_eq!(third.sha256(FLASH_AT_END), sha256_of(&file));
    assert_eq!(sha256_of(&image), NVS_IMAGE_SHA256, "--flash is only read");
}

/// What the README says standard error reads for the runs of the reference images: the
/// `inkemu:` lines of its `text` blocks against the ones the binary prints. Each example is the
/// sentence that introduces a block and the command or commands that produce it, written as
/// the README writes them. An example that begins with `$ ` takes its commands from the block
/// itself, one run per paragraph. The images are copied under the names the README shows, and
/// the runs start in the directory that holds them, so that the file names the lines print are
/// the README's own. Numbers (program counter, cycles, instructions, device time, flash digests)
/// move with any change that touches what the firmware sees; this is the test that says so.
///
/// `reference_firmware` is in the name: it selects the tests of the CI job that has the images.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR, INKEMU_REF_FLASH and INKEMU_NVS_FLASH"]
fn the_readme_shows_what_the_binary_prints_for_the_reference_firmware() {
    const BINARY: &str = "target/release/inkemu ";
    /// The introduction of a block of the README, and what runs to produce it (empty: the
    /// commands are in the block).
    const EXAMPLES: [(&str, &[&str]); 3] = [
        (
            "With `--max-cycles 50000000`, standard error reads:\n\n",
            &["run --device x4 --flash flash_hello.bin --max-cycles 50000000"],
        ),
        (
            "`Restarting in 6 seconds...` and standard error reads:\n\n",
            &["run --device x4 --flash flash_hello.bin --max-time 5"],
        ),
        ("keeps a restart counter in NVS:\n\n", &[]),
    ];

    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let images = [
        ("flash_hello.bin", reference_image()),
        (
            "flash_nvs.bin",
            pinned_image("INKEMU_NVS_FLASH", "flash_nvs.bin", NVS_IMAGE_SHA256),
        ),
    ];
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"),
    )
    .unwrap();
    let inkemu_lines = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.starts_with("inkemu:"))
            .map(str::to_owned)
            .collect()
    };

    let mut disagreements = Vec::new();
    for (introduction, commands) in EXAMPLES {
        let after = readme
            .split_once(introduction)
            .unwrap_or_else(|| panic!("the README has no block after {introduction:?}"))
            .1;
        let block = after
            .strip_prefix("```text\n")
            .unwrap_or_else(|| panic!("no `text` block after {introduction:?}"))
            .split_once("\n```")
            .unwrap()
            .0;
        // One run, one paragraph of the block, when the block holds its commands.
        let runs: Vec<(String, String)> = if commands.is_empty() {
            block
                .split("\n\n")
                .map(|paragraph| {
                    let command = paragraph
                        .lines()
                        .next()
                        .and_then(|line| line.strip_prefix("$ "))
                        .and_then(|line| line.strip_prefix(BINARY))
                        .unwrap_or_else(|| panic!("no command opens:\n{paragraph}"));
                    (command.to_owned(), paragraph.to_owned())
                })
                .collect()
        } else {
            vec![(commands[0].to_owned(), block.to_owned())]
        };

        let dir = tempfile::tempdir().unwrap();
        for (name, source) in &images {
            std::fs::copy(source, dir.path().join(name)).unwrap();
        }
        for (command, shown) in runs {
            let output = inkemu()
                .current_dir(dir.path())
                .args(command.split_whitespace())
                .args(["--rom-dir", &rom_dir])
                .timeout(HANG_LIMIT)
                .output()
                .unwrap();
            let log = String::from_utf8_lossy(&output.stderr).into_owned();
            assert_eq!(output.status.code(), Some(0), "{command}\n{log}");
            let printed = inkemu_lines(&log);
            let shown = inkemu_lines(&shown);
            if printed != shown {
                disagreements.push(format!(
                    "`{BINARY}{command}` prints:\n{}\nthe README shows:\n{}",
                    printed.join("\n"),
                    shown.join("\n")
                ));
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "{}\n\nthe README shows other `inkemu:` lines than the binary prints; copy the lines \
         above that follow \"prints:\" into README.md, in place of the lines it shows",
        disagreements.join("\n\n")
    );
}

/// `inkemu run --device x4 --firmware <application> [--bootloader <bootloader>] <more>` with a ROM
/// directory that holds nothing: a refusal of what was given must come before any attempt to load
/// a ROM.
fn run_with_firmware(
    application: &[u8],
    bootloader: Option<&[u8]>,
    more: &[&str],
) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    let firmware = dir.path().join("app.bin");
    std::fs::write(&firmware, application).unwrap();
    let mut command = inkemu();
    command
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
        .arg(dir.path().join("rom"))
        .arg("--firmware")
        .arg(&firmware);
    if let Some(bytes) = bootloader {
        let path = dir.path().join("bootloader.bin");
        std::fs::write(&path, bytes).unwrap();
        command.arg("--bootloader").arg(path);
    }
    command.args(more).timeout(HANG_LIMIT).assert()
}

/// What `run_with_firmware` must refuse before boot, each with its cause and its remedy.
#[test]
fn every_input_that_cannot_be_booted_from_an_application_is_refused_before_boot() {
    use firmware::builder::{bootloader_spec, refresh_digest};
    let app = build_image(&ImageSpec::default());
    let bootloader = build_image(&bootloader_spec());
    let mut xota = b"XOTA".to_vec();
    xota.resize(512, 0);
    let s3 = build_image(&ImageSpec {
        chip: 9,
        ..ImageSpec::default()
    });
    let mut corrupt = app.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    // A data segment the size of the X4's slot, after the descriptor: the image is larger.
    let mut too_large = ImageSpec::default();
    too_large.segments.push((0x3C10_0000, vec![0; 0x64_0000]));
    let too_large = build_image(&too_large);
    let mut later_chip = bootloader.clone();
    later_chip[15..17].copy_from_slice(&4u16.to_le_bytes());
    refresh_digest(&mut later_chip);
    for (case, application, given, cause, remedy) in [
        (
            "not an image",
            vec![0u8; 64],
            None,
            "its first byte is 0x00, expected 0xe9",
            "; use the plain application .bin",
        ),
        (
            "an encrypted update",
            xota,
            None,
            "is an encrypted OTA update (.xota)",
            "; use the plain application .bin of a release",
        ),
        (
            "an S3 image",
            s3,
            None,
            "is built for ESP32-S3 and the device uses ESP32-C3",
            "; choose a device with that chip",
        ),
        (
            "larger than its slot",
            too_large,
            None,
            "and its slot, app0, holds 6553600",
            "; use an application that fits",
        ),
        (
            "a corrupt digest",
            corrupt,
            None,
            "the firmware is damaged: SHA-256 mismatch",
            "; re-download the file or rebuild the firmware",
        ),
        (
            "a whole flash image",
            build_flash_image(&ImageSpec::default()),
            None,
            "is a whole flash image",
            "; use it as the flash (`--flash`)",
        ),
        (
            "a bootloader as the firmware",
            bootloader.clone(),
            None,
            "the firmware is a 2nd-stage bootloader",
            "; give it as the bootloader (`--bootloader`)",
        ),
        (
            "an application as the bootloader",
            app.clone(),
            Some(app.clone()),
            "the bootloader is an application image",
            "; use it as the firmware (`--firmware`)",
        ),
        (
            "a bootloader for a later chip",
            app.clone(),
            Some(later_chip),
            "requires chip revision v0.4 or later and the device is v0.3: it would reset the chip for ever",
            "; give a bootloader built for v0.3",
        ),
    ] {
        assert_refused_before_boot(case, &application, given.as_deref(), cause, remedy);
    }
}

/// `image` with the 16-bit header field at `at` set to `value`, its digest refreshed: only that
/// field is wrong.
fn with_header_field(image: &[u8], at: usize, value: u16) -> Vec<u8> {
    let mut image = image.to_vec();
    image[at..at + 2].copy_from_slice(&value.to_le_bytes());
    firmware::builder::refresh_digest(&mut image);
    image
}

/// `image` whose header says 32 MB of flash (byte 3, high nibble 5), its digest refreshed.
fn with_32_mb_of_flash(image: &[u8]) -> Vec<u8> {
    let mut image = image.to_vec();
    image[3] = 0x5F;
    firmware::builder::refresh_digest(&mut image);
    image
}

/// The test application whose descriptor accepts the eFuse block revisions `min..=max`
/// (`min_efuse_blk_rev_full` and `max_efuse_blk_rev_full`, bytes 176..180); those of the X4 are
/// v1.2.
fn with_efuse_blocks(min: u16, max: u16) -> Vec<u8> {
    let mut desc = firmware::builder::app_desc_bytes("1.0.0", "demo", "v5.5.2");
    desc[176..178].copy_from_slice(&min.to_le_bytes());
    desc[178..180].copy_from_slice(&max.to_le_bytes());
    let mut spec = ImageSpec::default();
    spec.segments[0].1 = desc;
    build_image(&spec)
}

/// `image` whose checksum, the byte before the appended digest, is wrong, its digest refreshed so
/// that only the checksum is; and the cause the refusal states.
fn with_a_wrong_checksum(image: &[u8]) -> (Vec<u8>, String) {
    let mut image = image.to_vec();
    let at = image.len() - 33;
    let computed = image[at];
    image[at] ^= 1;
    firmware::builder::refresh_digest(&mut image);
    let cause = format!(
        "the firmware is damaged: checksum mismatch (stored {:#04x}, computed {computed:#04x})",
        computed ^ 1
    );
    (image, cause)
}

/// What the bootloader or the mask ROM would refuse in an application or a bootloader, refused
/// by `run_with_firmware` before boot, each message whole: its cause, then its remedy.
#[test]
fn what_the_bootloader_or_the_rom_would_refuse_is_refused_before_boot_in_whole_words() {
    use firmware::builder::bootloader_spec;
    let app = build_image(&ImageSpec::default());
    let bootloader = build_image(&bootloader_spec());
    // Code in a window of the flash and no descriptor: an application without its descriptor.
    let no_descriptor = build_image(&ImageSpec {
        segments: vec![(0x4200_0020, vec![0; 8]), (0x4038_0000, vec![0; 8])],
        ..ImageSpec::default()
    });
    let (bad_checksum, checksum_cause) = with_a_wrong_checksum(&app);
    // An application longer than the 0x8000 bytes below the partition table, given as the
    // bootloader.
    let mut long_application = ImageSpec::default();
    long_application
        .segments
        .push((0x3C01_0000, vec![0; 0x9000]));
    let long_application = build_image(&long_application);
    // A flash image, a bootloader at its start, whose partition table does not parse.
    let mut damaged_table = build_flash_image(&bootloader_spec());
    damaged_table[0x8010] ^= 1;
    let damaged_table_cause = format!(
        "the firmware is {} bytes and starts with a 2nd-stage bootloader, so it is a whole flash image, but its partition table at 0x8000 does not parse",
        damaged_table.len()
    );
    // Header bytes 15..17 are min_chip_rev_full, 17..19 max_chip_rev_full.
    for (case, application, given, cause, remedy) in [
        (
            "no application descriptor",
            no_descriptor,
            None,
            "the firmware has no application descriptor at 0x20 (esp_app_desc_t), which every ESP-IDF application carries and the bootloader reads",
            "; use the application .bin of an ESP-IDF build\n",
        ),
        (
            "a later chip revision required",
            with_header_field(&app, 15, 4),
            None,
            "the firmware requires chip revision v0.4 or later and the device is v0.3: the bootloader would refuse to start it",
            "; use a build for v0.3 (CONFIG_ESP32C3_REV_MIN), or choose a device of revision v0.4 or later\n",
        ),
        (
            "an earlier chip revision required",
            with_header_field(&app, 17, 2),
            None,
            "the firmware accepts chip revisions up to v0.2 and the device is v0.3: the bootloader would refuse to start it",
            "; use a build that accepts v0.3, or choose a device of revision v0.2 or earlier\n",
        ),
        (
            "a later eFuse block revision required",
            with_efuse_blocks(103, 199),
            None,
            "the firmware requires eFuse block revision v1.3 or later and the chip has v1.2: the bootloader would refuse to start it",
            "; use a build that accepts v1.2\n",
        ),
        (
            "an earlier eFuse block revision required",
            with_efuse_blocks(0, 101),
            None,
            "the firmware accepts eFuse block revisions up to v1.1 and the chip has v1.2: the bootloader would refuse to start it",
            "; use a build that accepts v1.2\n",
        ),
        (
            "more flash in the header of the firmware than the device has",
            with_32_mb_of_flash(&app),
            None,
            "the header of the firmware says 32 MB of flash and the device has 16 MB",
            "; use the firmware built for 16 MB, or choose a device with at least 32 MB\n",
        ),
        (
            "a wrong checksum",
            bad_checksum,
            None,
            checksum_cause.as_str(),
            "; re-download the file or rebuild the firmware\n",
        ),
        (
            "a flash image whose partition table does not parse",
            damaged_table,
            None,
            damaged_table_cause.as_str(),
            "; use an intact dump as the flash (`--flash`), or the application .bin of its build as the firmware\n",
        ),
        (
            "more flash in the header of the bootloader than the device has",
            app.clone(),
            Some(with_32_mb_of_flash(&bootloader)),
            "the header of the bootloader says 32 MB of flash and the device has 16 MB: the application would abort when it finds the smaller chip",
            "; give a bootloader built for 16 MB, or no bootloader to use the one inkemu ships\n",
        ),
        (
            "an application longer than a bootloader can be, as the bootloader",
            app.clone(),
            Some(long_application),
            "the bootloader is an application image (it carries an application descriptor at 0x20)",
            "; use it as the firmware (`--firmware`), and give no bootloader or the bootloader.bin of its build\n",
        ),
    ] {
        assert!(remedy.ends_with('\n'), "{case}: the remedy ends the line");
        assert_refused_before_boot(case, &application, given.as_deref(), cause, remedy);
    }
}

/// A file given as the bootloader that is no bootloader at all — not an image, an encrypted
/// update, a whole flash image — refused by `run_with_firmware` before boot, each message whole.
#[test]
fn what_is_no_bootloader_at_all_is_refused_as_the_bootloader_in_whole_words() {
    use firmware::builder::bootloader_spec;
    let app = build_image(&ImageSpec::default());
    let mut xota = b"XOTA".to_vec();
    xota.resize(512, 0);
    for (case, given, cause, remedy) in [
        (
            "not an image",
            vec![0x7F; 64],
            "the bootloader is not an ESP-IDF image: its first byte is 0x7f, expected 0xe9",
            "; give the bootloader.bin of a build, or no bootloader to use the one inkemu ships\n",
        ),
        (
            "an encrypted update",
            xota,
            "the bootloader is an encrypted OTA update (.xota), not a bootloader",
            "; give the bootloader.bin of a build, or no bootloader to use the one inkemu ships\n",
        ),
        (
            "a whole flash image",
            build_flash_image(&bootloader_spec()),
            "the bootloader is a whole flash image (a partition table follows it at 0x8000), not a bootloader alone",
            "; use it as the flash (`--flash`), and give no firmware (`--firmware`): the image holds its own application\n",
        ),
    ] {
        assert_refused_before_boot(case, &app, Some(&given), cause, remedy);
    }
}

/// `run_with_firmware(application, given)` exits 2 before it looks for the ROM, with an error
/// that holds `cause` and `remedy`; a `remedy` that ends the line follows `cause` at once, so
/// that the two are the whole message.
fn assert_refused_before_boot(
    what: &str,
    application: &[u8],
    given: Option<&[u8]>,
    cause: &str,
    remedy: &str,
) {
    let assert = run_with_firmware(application, given, &[])
        .code(2)
        .stdout(predicate::str::is_empty());
    let log = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        log.starts_with("error: ") && log.contains(cause) && log.contains(remedy),
        "{what}: {log}"
    );
    if remedy.ends_with('\n') {
        assert!(
            log.contains(&format!("{cause}{remedy}")),
            "{what}: the whole message: {log}"
        );
    }
    assert!(
        !log.contains(MISSING_ROM),
        "{what}: refused before the ROM: {log}"
    );
}

#[test]
fn a_firmware_and_a_bootloader_that_pass_are_taken_to_the_rom() {
    use firmware::builder::bootloader_spec;
    // The ROM directory is empty: getting as far as the ROM is getting past every check.
    let app = build_image(&ImageSpec::default());
    for given in [None, Some(build_image(&bootloader_spec()))] {
        run_with_firmware(&app, given.as_deref(), &[])
            .code(2)
            .stderr(contains(MISSING_ROM));
    }
}

#[test]
fn the_warnings_of_a_given_bootloader_are_printed_before_the_rom_is_looked_for() {
    use firmware::builder::{bootloader_spec, refresh_digest};
    // A header that says 4 MB of flash, and an appended digest that does not match: both are
    // warnings, and the run goes on to the ROM.
    let mut bootloader = build_image(&bootloader_spec());
    bootloader[3] = 0x2F;
    refresh_digest(&mut bootloader);
    let last = bootloader.len() - 1;
    bootloader[last] ^= 1;
    let assert = run_with_firmware(&build_image(&ImageSpec::default()), Some(&bootloader), &[])
        .code(2)
        .stdout(predicate::str::is_empty());
    let log = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(
        lines.get(..2),
        Some(
            &[
                "warning: the header of the bootloader says 4 MB of flash and the device has 16 MB; the application is told 4 MB and uses no more; rebuild the bootloader for 16 MB to use the whole flash",
                "warning: the SHA-256 appended to the bootloader does not match it; the mask ROM boots it anyway, as a chip without secure boot does, and prints four lines that say so; rebuild the bootloader if it was not changed on purpose",
            ][..]
        ),
        "{log}"
    );
    assert!(
        lines.get(2).is_some_and(|line| line.contains(MISSING_ROM)),
        "{log}"
    );
}

/// A run refused for its flash file prints no warning about a bootloader it will never boot: the
/// refusal is the whole of what it says.
#[test]
fn a_run_refused_for_its_flash_file_prints_no_warning_of_its_bootloader() {
    use firmware::builder::{bootloader_spec, refresh_digest};
    let mut bootloader = build_image(&bootloader_spec());
    bootloader[3] = 0x2F;
    refresh_digest(&mut bootloader);
    let dir = tempfile::tempdir().unwrap();
    let kept = dir.path().join("x4.bin");
    std::fs::write(&kept, vec![0xFF; 16 << 20]).unwrap();
    let assert = run_with_firmware(
        &build_image(&ImageSpec::default()),
        Some(&bootloader),
        &["--flash-file", kept.to_str().unwrap()],
    )
    .code(2)
    .stdout(predicate::str::is_empty());
    let log = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        log.starts_with("error: both --firmware and the flash file ")
            && log.contains("x4.bin, which exists, were given")
            && log.lines().count() == 1,
        "{log}"
    );
}

/// The header of a run names the bootloader it was given: by its descriptor and the SHA-256 of
/// its image, or by that hash alone when it has no descriptor. The header is printed once the ROM
/// is loaded.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn the_header_of_a_run_names_the_bootloader_it_was_given() {
    use firmware::builder::bootloader_spec;
    let rom = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let dir = tempfile::tempdir().unwrap();
    let firmware = dir.path().join("app.bin");
    std::fs::write(&firmware, build_image(&ImageSpec::default())).unwrap();
    // ESP-IDF before v5.1 writes no bootloader descriptor: its magic word is cleared here.
    let mut without_descriptor = bootloader_spec();
    without_descriptor.segments[0].1[0] = 0;
    for (image, named) in [
        (
            build_image(&bootloader_spec()),
            "behind a bootloader of ESP-IDF v5.5.2-test built Oct 13 2026 10:00:00, sha256 ",
        ),
        (
            build_image(&without_descriptor),
            "behind a bootloader without a descriptor, sha256 ",
        ),
    ] {
        let bootloader = dir.path().join("bootloader.bin");
        std::fs::write(&bootloader, &image).unwrap();
        let output = inkemu()
            .args([
                "run",
                "--device",
                "x4",
                "--max-cycles",
                "1000",
                "--rom-dir",
                &rom,
            ])
            .arg("--firmware")
            .arg(&firmware)
            .arg("--bootloader")
            .arg(&bootloader)
            .timeout(HANG_LIMIT)
            .assert()
            .code(0)
            .get_output()
            .clone();
        let log = String::from_utf8_lossy(&output.stderr).into_owned();
        let expected = format!(
            "{named}{}, under the partition table of the profile",
            sha256_of(&bootloader)
        );
        assert!(log.contains(&expected), "`{expected}` is missing:\n{log}");
    }
}

#[test]
fn a_firmware_is_a_seed_like_an_image_and_the_flash_file_rules_apply() {
    let app = build_image(&ImageSpec::default());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x4.bin");
    sparse_file(&file, X4_FLASH_BYTES);
    let firmware = dir.path().join("app.bin");
    std::fs::write(&firmware, &app).unwrap();
    let firmware_arg = firmware.to_str().unwrap();
    let bootloader = dir.path().join("bootloader.bin");
    std::fs::write(
        &bootloader,
        build_image(&firmware::builder::bootloader_spec()),
    )
    .unwrap();
    for more in [&[][..], &["--bootloader", bootloader.to_str().unwrap()][..]] {
        let args = [&["--firmware", firmware_arg][..], more].concat();
        run_with_flash_file(&file, &args).code(2).stderr(
            contains("both --firmware and the flash file")
                .and(contains(
                    "; drop --firmware, and --bootloader if it was given, to go on from the flash file",
                ))
                .and(contains(
                    "start again from the flash composed from the firmware",
                ))
                .and(contains(MISSING_ROM).not()),
        );
    }
    // Dropping both goes on from the flash file, as far as the ROM; with --reset-flash the run
    // starts from the firmware, as far as the ROM.
    run_with_flash_file(&file, &[])
        .code(2)
        .stderr(contains(MISSING_ROM));
    run_with_flash_file(&file, &["--firmware", firmware_arg, "--reset-flash"])
        .code(2)
        .stderr(contains(MISSING_ROM));
    // A flash file of another size is kept, and the remedy does not send a firmware to --flash.
    let small = dir.path().join("small.bin");
    sparse_file(&small, 1 << 20);
    run_with_flash_file(&small, &["--firmware", firmware_arg, "--reset-flash"])
        .code(2)
        .stderr(
            contains(
                "is 1048576 bytes and the flash of the device is 16777216 bytes: the file is not the flash of this device, and it is never padded or cut; use it with the device it was written for, or give --flash-file a new path\n",
            )
            .and(contains(MISSING_ROM).not()),
        );
    // The firmware cannot be the flash file, nor --bootloader. The remedy keeps the file: a reset
    // of the flash file would replace the firmware with the flash.
    let keeps_the_file = || {
        contains("; give --flash-file another path to keep the file as it is\n")
            .and(contains("--reset-flash").not())
    };
    run_with_flash_file(&firmware, &["--firmware", firmware_arg])
        .code(2)
        .stderr(contains("--firmware and --flash-file name the same file").and(keeps_the_file()));
    let other = dir.path().join("other.bin");
    run_with_flash_file(
        &firmware,
        &[
            "--firmware",
            other.to_str().unwrap(),
            "--bootloader",
            firmware_arg,
        ],
    )
    .code(2)
    .stderr(contains("--bootloader and --flash-file name the same file").and(keeps_the_file()));
    assert_eq!(std::fs::read(&firmware).unwrap(), app, "never written");
}

#[test]
fn a_firmware_with_an_image_or_a_bootloader_without_a_firmware_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.bin");
    std::fs::write(&path, b"x").unwrap();
    let path = path.to_str().unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--flash", path, "--firmware", path])
        .assert()
        .code(2)
        .stderr(contains(
            "'--flash <FLASH>' cannot be used with '--firmware <FILE>'",
        ));
    inkemu()
        .args(["run", "--device", "x4", "--bootloader", path])
        .assert()
        .code(2)
        .stderr(contains("--firmware <FILE>"));
}

#[test]
fn run_help_documents_the_firmware_and_the_bootloader() {
    let help = inkemu().args(["run", "--help"]).assert().success();
    let out = String::from_utf8_lossy(&help.get_output().stdout).into_owned();
    for expected in [
        "--firmware <FILE>",
        "the partition table of the device profile at 0x8000",
        "--bootloader <FILE>",
        "instead of the one inkemu ships",
    ] {
        assert!(out.contains(expected), "`{expected}` is missing:\n{out}");
    }
}

/// The help says that a flash file that exists refuses `--firmware` as it refuses `--flash`, and
/// the README shows the help as the binary prints it (white space aside: the help is wrapped to
/// the terminal).
#[test]
fn the_readme_shows_run_help_as_the_binary_prints_it() {
    fn words(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    let help = inkemu().args(["run", "--help"]).assert().success();
    let help = words(&String::from_utf8_lossy(&help.get_output().stdout));
    assert!(
        help.contains(
            "A run starts from the file when it exists (--flash or --firmware is then refused unless --reset-flash is given)"
        ),
        "{help}"
    );
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"),
    )
    .unwrap();
    let after = readme
        .split_once(
            "The options and the exit codes, as `inkemu run --help` prints them:\n\n```text\n",
        )
        .unwrap_or_else(|| panic!("no help block in the README"))
        .1;
    let block = after.split_once("\n```").unwrap().0;
    assert_eq!(words(block), help);
}

/// `hello_world.bin` and `nvs_rw_value_cxx.bin`, the application images the reference build
/// merges at 0x10000 (`tools/reference-firmware/SHA256SUMS.inputs`, `SHA256SUMS.inputs-nvs`).
const HELLO_APPLICATION_SHA256: &str =
    "4b82d73e37d10969d660eba95984cbe7f053212d39306cc54bfce6c366d0f04b";
const NVS_APPLICATION_SHA256: &str =
    "44523a2fd8f7dc0a3eaad17a8991c647ab663ad993dc0db2e31235806b516ff2";
/// The flash composed for the X4 from each application: the reference images merged under the
/// stock table of the X4, `flash_hello_ota.bin` (docs/research/sp1b-test-oracle.md, section 2) and
/// `flash_nvs_ota.bin` (`SHA256SUMS.output`).
const HELLO_COMPOSED_SHA256: &str =
    "9ddacb60b8b7338731b52edee1ecae47c7f45470d8db6fa8d1fbc5a20011bc89";
const NVS_COMPOSED_SHA256: &str =
    "87832c6d5d15672fb3892bfad281688fc7486b360a25b495bdb6a27d04f28fec";

/// The application image of the pinned merged image `variable` names, written to a file of its
/// own: the bytes the reference build merged at 0x10000, checked against `application_sha256`.
fn reference_application(
    variable: &str,
    file: &str,
    sha256: &str,
    application_sha256: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    use sha2::Digest as _;
    let merged = std::fs::read(pinned_image(variable, file, sha256)).unwrap();
    let from = &merged[0x1_0000..];
    let len = firmware::FirmwareImage::parse(from).unwrap().len;
    let application = &from[..len];
    assert_eq!(
        emu_cli::term::hex(&sha2::Sha256::digest(application)),
        application_sha256,
        "the application merged at 0x10000 of {file}"
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(file.replace("flash_", "app_"));
    std::fs::write(&path, application).unwrap();
    (dir, path)
}

/// A console with the timestamps of ESP-IDF's log lines masked: `I (123) boot: …` → `I (#) boot: …`.
fn masked(console: &str) -> String {
    console
        .lines()
        .map(|line| match line.split_once(") ") {
            Some((head, rest))
                if head.len() > 3
                    && head[..3].ends_with(" (")
                    && head[3..].bytes().all(|b| b.is_ascii_digit()) =>
            {
                format!("{} (#) {rest}", &head[..1])
            }
            _ => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first boot of the application behind the stock table of the X4: the lines of the QEMU
/// capture of the same flash (`golden_hello_ota_boot.log`, docs/research/sp1b-test-oracle.md,
/// section 2), with the two documented differences of inkemu's chip: eFuse block revision v1.2
/// and SPI mode DIO.
const OTA_FIRST_BOOT_LINES: [&str; 26] = [
    "I (#) boot: ESP-IDF v5.5.2 2nd stage bootloader",
    "I (#) boot: chip revision: v0.3",
    "I (#) boot: efuse block revision: v1.2",
    "I (#) boot.esp32c3: SPI Speed      : 80MHz",
    "I (#) boot.esp32c3: SPI Mode       : DIO",
    "I (#) boot.esp32c3: SPI Flash Size : 16MB",
    "I (#) boot: Enabling RNG early entropy source...",
    "I (#) boot: Partition Table:",
    "I (#) boot: ## Label            Usage          Type ST Offset   Length",
    "I (#) boot:  0 nvs              WiFi data        01 02 00009000 00005000",
    "I (#) boot:  1 otadata          OTA data         01 00 0000e000 00002000",
    "I (#) boot:  2 app0             OTA app          00 10 00010000 00640000",
    "I (#) boot:  3 app1             OTA app          00 11 00650000 00640000",
    "I (#) boot:  4 spiffs           Unknown data     01 82 00c90000 00360000",
    "I (#) boot:  5 coredump         Unknown data     01 03 00ff0000 00010000",
    "I (#) boot: End of partition table",
    "I (#) boot: No factory image, trying OTA 0",
    "I (#) esp_image: segment 0: paddr=00010020 vaddr=3c010020 size=0656ch ( 25964) map",
    "I (#) esp_image: segment 1: paddr=00016594 vaddr=3fc8b600 size=011d8h (  4568) load",
    "I (#) esp_image: segment 2: paddr=00017774 vaddr=40380000 size=088a4h ( 34980) load",
    "I (#) esp_image: segment 3: paddr=00020020 vaddr=42000020 size=0e6fch ( 59132) map",
    "I (#) esp_image: segment 4: paddr=0002e724 vaddr=403888a4 size=02c70h ( 11376) load",
    "I (#) esp_image: segment 5: paddr=0003139c vaddr=50000000 size=00020h (    32) load",
    "I (#) boot: Loaded app from partition at offset 0x10000",
    "I (#) boot: Set actual ota_seq=1 in otadata[0]",
    "I (#) boot: Disabling RNG early entropy source...",
];

/// The milestone of an application image: `--firmware hello_world.bin`, the application alone,
/// boots behind the bootloader inkemu ships and the stock table of the X4, in strict mode,
/// through its whole life, its restart and the second boot; the bootloader writes `otadata` on
/// the first boot and on that one only.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_boots_the_reference_firmware_from_its_application_image_through_a_second_boot() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (_dir, application) = reference_application(
        "INKEMU_REF_FLASH",
        "flash_hello.bin",
        HELLO_IMAGE_SHA256,
        HELLO_APPLICATION_SHA256,
    );
    // One life is 40 million cycles here; 44 million reach the application of the second boot.
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--max-cycles",
            "44000000",
        ])
        .args(["--rom-dir", &rom_dir, "--firmware"])
        .arg(&application)
        .timeout(HANG_LIMIT)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let console = masked(&String::from_utf8_lossy(&output.stdout).replace('\r', ""));
    let first_boot = OTA_FIRST_BOOT_LINES.join("\n");
    assert!(console.contains(&first_boot), "{console}");
    let mut from = 0;
    for expected in [
        "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "entry 0x403cbf1a\n",
        first_boot.as_str(),
        "I (#) cpu_start: Unicore app\n",
        "Hello world!\n",
        "Restarting in 0 seconds...\n",
        "Restarting now.\n",
        "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "I (#) boot: End of partition table\nI (#) esp_image: segment 0:",
        "I (#) boot: Loaded app from partition at offset 0x10000\nI (#) boot: Disabling RNG early entropy source...\n",
        "I (#) cpu_start: Unicore app\n",
    ] {
        let at = console[from..]
            .find(expected)
            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{console}"));
        from += at + expected.len();
    }
    // The second boot finds otadata written: it neither looks for a factory image nor writes.
    assert_eq!(
        console
            .matches("Set actual ota_seq=1 in otadata[0]")
            .count(),
        1
    );
    assert_eq!(console.matches("No factory image, trying OTA 0").count(), 1);
    assert_eq!(console.matches("2nd stage bootloader").count(), 2);
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        log.contains(
            "flash: app_hello.bin (hello_world 1, ESP-IDF v5.5.2) in app0 at 0x10000, behind the ESP-IDF v5.5.2 bootloader inkemu ships (esp-idf-v5.5.2-esp32c3-dio), under the partition table of the profile (16 MiB)\n"
        ),
        "{log}"
    );
    assert!(
        !log.contains("warning:") && !log.contains("stopped by --strict-mmio"),
        "nothing was reported in a whole life cycle: {log}"
    );
}

/// The trace hash of the application image's life, pinned like the one of the merged image
/// (`x4_runs_the_reference_firmware_along_its_pinned_trace`), with its device time and the flash
/// it starts from. The two differ from those of the merged image: the table is the X4's, so the
/// bootloader takes the OTA path and writes `otadata`.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_runs_the_reference_firmware_from_its_application_image_along_its_pinned_trace() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (_dir, application) = reference_application(
        "INKEMU_REF_FLASH",
        "flash_hello.bin",
        HELLO_IMAGE_SHA256,
        HELLO_APPLICATION_SHA256,
    );
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--max-cycles",
            "40000001",
        ])
        .args(["--trace-hash", "--rom-dir", &rom_dir, "--firmware"])
        .arg(&application)
        .timeout(HANG_LIMIT)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        trace_hash_line(&log),
        Some("trace-hash 9a6cf19d47136f33"),
        "{log}"
    );
    assert_eq!(device_micros(&log), 11_403_054, "{log}");
    assert!(
        log.contains("after 40000001 cycles (26635331 instructions)"),
        "{log}"
    );
    assert!(
        log.contains(&format!("{FLASH_AT_START}{HELLO_COMPOSED_SHA256}\n")),
        "the composed flash is the merged image under the X4 table: {log}"
    );
}

/// The NVS example from its application image, kept in a flash file: the first run composes the
/// flash (it is `flash_nvs_ota.bin`, byte for byte), the second goes on from the file and counts.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_FLASH"]
fn the_nvs_reference_firmware_counts_on_from_its_application_image_across_two_runs() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let (dir, application) = reference_application(
        "INKEMU_NVS_FLASH",
        "flash_nvs.bin",
        NVS_IMAGE_SHA256,
        NVS_APPLICATION_SHA256,
    );
    let file = dir.path().join("x4.bin");
    let run = |more: &[&std::ffi::OsStr]| {
        let output = inkemu()
            .args(["run", "--device", "x4", "--strict-mmio", "--max-time", "5"])
            .args(["--rom-dir", &rom_dir])
            .args(more)
            .arg("--flash-file")
            .arg(&file)
            .timeout(HANG_LIMIT)
            .output()
            .unwrap();
        Counted {
            code: output.status.code(),
            console: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
            log: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    };
    let first = run(&[std::ffi::OsStr::new("--firmware"), application.as_os_str()]);
    assert_eq!(first.code, Some(0), "{}", first.log);
    assert_eq!(first.sha256(FLASH_AT_START), NVS_COMPOSED_SHA256);
    assert!(
        first
            .console
            .contains("boot: Set actual ota_seq=1 in otadata[0]\n")
            && first.console.contains(
                "Reading restart counter from NVS ... The value is not initialized yet!\n"
            ),
        "{}",
        first.console
    );
    assert_eq!(first.sha256(FLASH_AT_END), sha256_of(&file));
    let second = run(&[]);
    assert_eq!(second.code, Some(0), "{}", second.log);
    assert_eq!(second.sha256(FLASH_AT_START), first.sha256(FLASH_AT_END));
    assert!(
        second.console.contains("rst:0x1 (POWERON)")
            && !second.console.contains("Set actual ota_seq")
            && second
                .console
                .contains("Reading restart counter from NVS ... Done\nRestart counter = 1\n"),
        "{}",
        second.console
    );
    for log in [&first.log, &second.log] {
        assert!(!log.contains("warning:"), "{log}");
    }
    assert_eq!(
        sha256_of(&application),
        NVS_APPLICATION_SHA256,
        "--firmware is only read"
    );
}

/// The bootloader inkemu ships is the one the reference build produces: the first bytes of the
/// pinned merged image, which the build checks against the same SHA-256.
#[test]
#[ignore = "needs INKEMU_REF_FLASH"]
fn the_shipped_bootloader_is_the_one_the_reference_firmware_build_produced() {
    let merged = std::fs::read(reference_image()).unwrap();
    let shipped = firmware::SHIPPED[0].bytes;
    assert_eq!(&merged[..shipped.len()], shipped);
    assert!(
        merged[shipped.len()..0x8000]
            .iter()
            .all(|&byte| byte == 0xFF),
        "nothing follows the bootloader before the table"
    );
}
