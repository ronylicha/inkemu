//! `inkemu licenses` through the real binary.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use assert_cmd::Command;

#[test]
fn licenses_prints_the_notice_then_every_part_with_its_licence() {
    let output = Command::new(env!("CARGO_BIN_EXE_inkemu"))
        .arg("licenses")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(
        text,
        boards::notices::notices_text(),
        "the library's text, as it is"
    );
    for expected in [
        "This product includes the ESP-IDF 2nd-stage bootloader",
        "== ESP-IDF 2nd-stage bootloader (third_party/esp-idf-bootloader) ==",
        "== ESP32-C3 register descriptions (Espressif CMSIS-SVD) (third_party/espressif-svd) ==",
        "Copyright (c) 2018-2019, The libfemto authors",
        "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
    ] {
        assert!(text.contains(expected), "`{expected}` is missing");
    }
}

#[test]
fn the_help_lists_the_licenses_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_inkemu"))
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(output).unwrap();
    assert!(
        help.contains("licenses") && help.contains("third-party parts built into inkemu"),
        "{help}"
    );
}
