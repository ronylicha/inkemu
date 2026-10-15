//! Exit-code contract of `inkemu rvtest`: 0 passed, 1 failed, 2 timeout or error.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/riscv-tests")
        .join(name)
}

fn rvtest() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_inkemu"));
    cmd.arg("rvtest");
    cmd
}

#[test]
fn passing_test_exits_zero() {
    rvtest()
        .arg(fixture("rv32ui-p-add"))
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Passed"));
}

/// Address of `tohost` in [`failing_elf`], inside the test RAM.
const TOHOST: u32 = 0x8000_1000;

/// A bare-metal RV32 ELF that reports the failure of its sub-test 1, as riscv-tests do: it
/// writes `(1 << 1) | 1` to `tohost`. Built here so that the test does not depend on a suite
/// test keeping a failure that inkemu may one day fix (the PMP of SP1b-4f did, for
/// `rv32mi-p-pmpaddr`).
fn failing_elf() -> Vec<u8> {
    const LOAD: u32 = 0x8000_0000;
    const EHDR: usize = 52;
    const PHDR: usize = 32;
    const SHDR: usize = 40;
    let code: Vec<u8> = [
        0x8000_12b7_u32, // lui t0, 0x80001: t0 = TOHOST
        0x0030_0313,     // addi t1, x0, 3
        0x0062_a023,     // sw t1, 0(t0)
        0x0000_006f,     // j .
    ]
    .iter()
    .flat_map(|w| w.to_le_bytes())
    .collect();
    let strtab = b"\0tohost\0".to_vec();
    let shstrtab = b"\0.symtab\0.strtab\0.shstrtab\0".to_vec();
    let mut symtab = vec![0u8; 16];
    symtab.extend(1u32.to_le_bytes()); // st_name: "tohost"
    symtab.extend(TOHOST.to_le_bytes()); // st_value
    symtab.extend(8u32.to_le_bytes()); // st_size
    symtab.extend([0x11, 0]); // st_info GLOBAL OBJECT, st_other
    symtab.extend(0xfff1u16.to_le_bytes()); // st_shndx SHN_ABS
    let code_at = EHDR + PHDR;
    let symtab_at = code_at + code.len();
    let strtab_at = symtab_at + symtab.len();
    let shstrtab_at = strtab_at + strtab.len();
    let sections_at = shstrtab_at + shstrtab.len();
    let word = |v: usize| u32::try_from(v).unwrap().to_le_bytes();
    let mut elf = vec![0x7f, b'E', b'L', b'F', 1, 1, 1];
    elf.resize(16, 0);
    elf.extend(2u16.to_le_bytes()); // ET_EXEC
    elf.extend(243u16.to_le_bytes()); // EM_RISCV
    elf.extend(1u32.to_le_bytes());
    elf.extend(LOAD.to_le_bytes()); // e_entry
    elf.extend(word(EHDR)); // e_phoff
    elf.extend(word(sections_at)); // e_shoff
    elf.extend(0u32.to_le_bytes());
    for half in [EHDR, PHDR, 1, SHDR, 4, 3] {
        elf.extend(u16::try_from(half).unwrap().to_le_bytes());
    }
    let load = usize::try_from(LOAD).unwrap();
    // type PT_LOAD, offset, vaddr, paddr, filesz, memsz, flags R+X, align
    for field in [1, code_at, load, load, code.len(), code.len(), 5, 4] {
        elf.extend(word(field));
    }
    elf.extend(&code);
    elf.extend(&symtab);
    elf.extend(&strtab);
    elf.extend(&shstrtab);
    elf.extend([0u8; SHDR]);
    // name, type, flags, addr, offset, size, link, info, addralign, entsize
    for header in [
        [1, 2, 0, 0, symtab_at, symtab.len(), 2, 1, 4, 16],
        [9, 3, 0, 0, strtab_at, strtab.len(), 0, 0, 1, 0],
        [17, 3, 0, 0, shstrtab_at, shstrtab.len(), 0, 0, 1, 0],
    ] {
        for field in header {
            elf.extend(word(field));
        }
    }
    elf
}

#[test]
fn failing_test_exits_one_and_names_the_sub_test() {
    let dir = std::env::temp_dir().join(format!("inkemu-fail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let elf = dir.join("fails-test-1.elf");
    std::fs::write(&elf, failing_elf()).unwrap();
    let assert = rvtest().arg(&elf).assert();
    std::fs::remove_dir_all(&dir).unwrap();
    assert
        .code(1)
        .stdout(predicate::str::contains("FAILED at test #1"));
}

#[test]
fn step_budget_exhaustion_exits_two() {
    rvtest()
        .arg(fixture("rv32ui-p-add"))
        .args(["--max-steps", "3"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no verdict after 3 steps"))
        .stderr(predicate::str::contains(
            "; raise --max-steps or check that the test writes to tohost",
        ));
}

#[test]
fn unreadable_elf_exits_two_with_a_remedy() {
    rvtest()
        .arg("/nonexistent/test.elf")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot read /nonexistent/test.elf",
        ))
        .stderr(predicate::str::contains(
            "; check the path and read permissions",
        ));
}

#[test]
fn unwritable_signature_path_exits_two_with_a_remedy() {
    rvtest()
        .arg(fixture("rv32ui-p-lw"))
        .args(["--signature", "/nonexistent/dir/lw.sig"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot write /nonexistent/dir/lw.sig",
        ))
        .stderr(predicate::str::contains(
            "; check that the directory exists and is writable",
        ));
}

#[test]
fn non_elf_input_exits_two_with_a_message() {
    rvtest()
        .arg(fixture("SHA256SUMS"))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not a valid ELF file"));
}

#[test]
fn allow_misaligned_flag_changes_the_misaligned_policy() {
    rvtest().arg(fixture("rv32ui-p-ma_data")).assert().code(1);
    rvtest()
        .arg("--allow-misaligned")
        .arg(fixture("rv32ui-p-ma_data"))
        .assert()
        .code(0);
}

#[test]
fn trace_hash_flag_prints_a_stable_hash() {
    let run = || {
        let out = rvtest()
            .arg(fixture("rv32ui-p-add"))
            .arg("--trace-hash")
            .assert()
            .code(0)
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };
    let first = run();
    assert!(first.contains("trace-hash "), "{first}");
    assert_eq!(first, run());
}

#[test]
fn signature_flag_writes_one_hex_word_per_line() {
    // rv32ui-p-lw: four data words in its signature region (little-endian view of its .data).
    let dir = std::env::temp_dir().join(format!("inkemu-sig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sig = dir.join("lw.sig");
    rvtest()
        .arg(fixture("rv32ui-p-lw"))
        .arg("--signature")
        .arg(&sig)
        .assert()
        .code(0);
    let text = std::fs::read_to_string(&sig).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(text, "00ff00ff\nff00ff00\n0ff00ff0\nf00ff00f\n");
}
