//! `inkemu run` and `inkemu rom fetch` through the real binary. Nothing here touches the network or the
//! default ROM cache: every run names its ROM directory.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use assert_cmd::Command;
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
