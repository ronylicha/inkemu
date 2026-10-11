//! `inkemu run` and `inkemu rom fetch` through the real binary. Nothing here touches the network or the
//! default ROM cache: every run names its ROM directory.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use assert_cmd::Command;
use firmware::builder::{ImageSpec, build_flash_image, build_image};
use predicates::prelude::*;
use predicates::str::contains;

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
    run_with_flash(&bytes).code(2).stderr(
        contains("flash.bin")
            .and(contains("encrypted OTA"))
            .and(contains("; use")),
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

#[test]
fn a_flash_file_that_does_not_exist_exits_2() {
    inkemu()
        .args(["run", "--device", "x4", "--flash", "/nonexistent/flash.bin"])
        .assert()
        .code(2)
        .stderr(contains("/nonexistent/flash.bin").and(contains("; check the path")));
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

/// The milestone through the real command: ROM, bootloader, first application line, strict stop.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_boots_the_reference_firmware_to_its_first_application_line() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let flash = std::env::var("INKEMU_REF_FLASH").expect("set INKEMU_REF_FLASH");
    let output = inkemu()
        .args([
            "run",
            "--device",
            "x4",
            "--strict-mmio",
            "--rom-dir",
            &rom_dir,
            "--flash",
            &flash,
        ])
        .assert()
        .code(3)
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    assert!(console.contains("entry 0x403cbf1a\n"), "{console}");
    assert!(
        console.contains(") boot: ESP-IDF v5.5.2 2nd stage bootloader\n"),
        "{console}"
    );
    assert!(
        console.contains(") boot.esp32c3: SPI Mode       : DIO\n"),
        "{console}"
    );
    assert!(
        console.contains(") boot: Disabling RNG early entropy source...\n"),
        "{console}"
    );
    assert!(console.contains(") cpu_start: Unicore app\n"), "{console}");
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        log.contains("stopped by --strict-mmio") && log.contains("TIMG0.RTCCALICFG"),
        "{log}"
    );
    assert!(
        !log.contains("warning:"),
        "nothing was reported before the stop: {log}"
    );
}
