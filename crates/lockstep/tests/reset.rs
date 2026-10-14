//! A reset of both sides inside a window (SP1b-4d, decisions J1–J3): the window ends at it, or
//! goes on through it when the window crosses it, with nothing of the machine touched; what the
//! reset leaves different is held by the rows of the window, exactly as they declare it, and is a
//! divergence otherwise. The traces are written from a machine, as in `policy.rs`, then changed
//! where QEMU's reset of its whole machine leaves another state than the chip's CPU reset.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test: a panic is a test failure

use cpu_rv32::asm::{addi, bne, csrrc, csrrci, csrrs, csrrsi, csrrw, lui, lw, sw};
use lockstep::table::PMP_RWX;
use lockstep::{
    AfterReset, Cause, CpuState, Difference, End, Field, Kept, KeptCsr, Location, Lockstep,
    Register, Site, Target, TraceReader, Treatment, Window, WindowEnd, kept, render_block,
};
use soc_esp32c3::memmap::{ROM_BASE, SRAM_DBUS_BASE};
use soc_esp32c3::test_support::{HANG, flat_rom, store_register};
use soc_esp32c3::{Esp32C3, SocConfig};

const S1: u8 = 9;
const A0: u8 = 10;
const A1: u8 = 11;
const T2: u8 = 7;
const SP: u8 = 2;
const NOP: u32 = 0x0000_0013;
/// `RTC_CNTL.OPTIONS0` and its bit 5, `SW_PROCPU_RST`: the CPU reset `esp_restart` asks for.
const SW_PROCPU_RST: u32 = 1 << 5;
const PMPCFG0: u16 = 0x3A0;

fn options0() -> u32 {
    svd_regs::esp32c3::addr("RTC_CNTL", "OPTIONS0")
}

/// `body`, then the store that resets the CPU; every life runs the whole of it again.
fn lives(body: &[u32]) -> Vec<u32> {
    let mut program = body.to_vec();
    program.extend(store_register(options0(), SW_PROCPU_RST));
    program.push(HANG);
    program
}

/// The states QEMU would print for `program`, as the machine runs it for `steps` instructions.
fn record(program: &[u32], steps: usize) -> Vec<CpuState> {
    let mut m = Esp32C3::new(flat_rom(program), &SocConfig::default());
    let mut states = vec![m.state()];
    for _ in 0..steps {
        let stepped = Target::step(&mut m);
        assert_eq!(stepped.stop, None);
        assert_eq!(stepped.interrupt, None);
        states.push(m.state());
    }
    states
}

fn text(states: &[CpuState]) -> String {
    let mut out = String::new();
    for state in states {
        out.push_str(&render_block(state));
    }
    out
}

/// The indices of the states at the reset vector after the first: one per reset.
fn resets(states: &[CpuState]) -> Vec<usize> {
    states
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, state)| state.pc() == ROM_BASE)
        .map(|(i, _)| i)
        .collect()
}

fn compare(trace: &str, program: &[u32], window: &Window) -> (End, Lockstep, Esp32C3) {
    let mut target = Esp32C3::new(flat_rom(program), &SocConfig::default());
    let mut reader = TraceReader::new(trace.as_bytes());
    let mut lockstep = Lockstep::for_window(&target, window, None);
    while let Some(event) = reader.next_event().unwrap() {
        if let Some(end) = lockstep.feed(event, &mut target) {
            return (end, lockstep, target);
        }
    }
    let end = lockstep.finish(&mut target);
    (end, lockstep, target)
}

const fn window(rows: &'static [Difference], resets: u32) -> Window {
    Window {
        name: "test",
        firmware: "test",
        resets,
        bound: None,
        rows,
    }
}

const NO_ROWS: &[Difference] = &[];

#[test]
fn a_reset_of_both_sides_ends_a_window_that_crosses_none_and_is_crossed_otherwise() {
    let program = lives(&[NOP, NOP]);
    let states = record(&program, 20);
    let at = resets(&states);
    assert!(at.len() >= 3, "{at:?}");
    let trace = text(&states);
    let (end, lockstep, _) = compare(&trace, &program, &window(NO_ROWS, 0));
    assert_eq!(end, End::Window(WindowEnd::Reset));
    assert_eq!(lockstep.counts().resets_crossed, 0);
    assert_eq!(
        lockstep.counts().compared,
        u64::try_from(at[0] - 1).unwrap()
    );
    // Crossing the first reset, the window ends at the second; the instruction that asked for
    // the first one is compared, and so is every one of the second life.
    let (end, lockstep, _) = compare(&trace, &program, &window(NO_ROWS, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset));
    assert_eq!(lockstep.counts().resets_crossed, 1);
    assert_eq!(
        lockstep.counts().compared,
        u64::try_from(at[1] - 1).unwrap()
    );
}

/// QEMU's states of `states` after the first reset, with `value` in `reg` until the life after
/// it writes `reg`, at state `written` (exclusive), as QEMU keeps a register through its reset.
fn keep_in_qemu(states: &mut [CpuState], reg: u8, value: u32, written: usize) {
    let first = resets(states)[0];
    for state in &mut states[first..written] {
        state.set(Field::X(reg), value);
    }
}

const KEPT_S1: &[Difference] = &[Difference {
    number: 1,
    summary: "s1 and a0, kept by QEMU at its reset",
    treatments: &[Treatment::KeptAtReset(&[
        kept(Field::X(S1), 0),
        kept(Field::X(A0), 0),
    ])],
}];

#[test]
fn a_register_qemu_keeps_through_its_reset_is_held_until_written_and_a_read_of_it_diverges() {
    // Each life writes s1, the second one only after an addition that reads it.
    let program = lives(&[addi(A1, S1, 1), addi(S1, 0, 0x55)]);
    let mut states = record(&program, 16);
    let first = resets(&states)[0];
    let end = states.len();
    keep_in_qemu(&mut states, S1, 0x55, end);
    let (end, _, _) = compare(&text(&states), &program, &window(KEPT_S1, 1));
    let End::Divergence(report) = end else {
        panic!("a read of the held s1: {end:?}");
    };
    assert_eq!(
        report.cause,
        Cause::ReadKeptAtReset {
            field: Field::X(S1)
        }
    );
    assert_eq!(report.index, u64::try_from(first).unwrap());
    assert!(
        report
            .to_string()
            .contains("read x9/s1, which the last reset left different"),
        "{report}"
    );

    // Written before it is read, it is held until then, then compared again.
    let program = lives(&[addi(S1, 0, 0x55), addi(A1, S1, 1)]);
    let mut states = record(&program, 16);
    let first = resets(&states)[0];
    keep_in_qemu(&mut states, S1, 0x55, first + 1);
    let (end, lockstep, target) = compare(&text(&states), &program, &window(KEPT_S1, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset));
    assert_eq!(
        lockstep
            .usage(&target)
            .rows()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![1],
        "the hold is a use of its row"
    );
    // Held at another value than the one QEMU had before its reset, it is a divergence: the row
    // admits only what QEMU kept.
    let mut states = record(&program, 16);
    keep_in_qemu(&mut states, S1, 0x56, first + 1);
    let (end, _, _) = compare(&text(&states), &program, &window(KEPT_S1, 1));
    assert!(matches!(end, End::Divergence(_)), "{end:?}");
}

#[test]
fn a_held_register_stored_to_memory_holds_the_register_it_is_loaded_back_into() {
    // A prologue saves s1, which the reset left different, and an epilogue loads it into a0.
    let body = [
        lui(T2, SRAM_DBUS_BASE),
        sw(S1, T2, 0),
        lw(A0, T2, 0),
        addi(S1, 0, 0x55),
    ];
    let program = lives(&body);
    let mut states = record(&program, 30);
    let first = resets(&states)[0];
    // The second life's s1 is written by its fourth instruction; a0, loaded by its third, holds
    // QEMU's 0x55 until the end of the trace.
    keep_in_qemu(&mut states, S1, 0x55, first + 4);
    for state in &mut states[first + 3..] {
        if state.pc() != ROM_BASE || state.get(Field::X(A0)) != 0 {
            state.set(Field::X(A0), 0x55);
        }
    }
    let second = resets(&states)[1];
    let (end, lockstep, target) = compare(&text(&states[..=second]), &program, &window(KEPT_S1, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset));
    let usage = lockstep.usage(&target);
    let sites = &usage.rows()[&1].sites;
    assert!(sites.contains_key(&Site::at(Location::Integer(S1))));
    assert!(sites.contains_key(&Site::at(Location::Integer(A0))));
    // Loaded back through a half-word, it is a read.
    let body = [
        lui(T2, SRAM_DBUS_BASE),
        0x0093_9023 | (u32::from(S1) << 20), // sh s1, 0(t2)
        addi(S1, 0, 0x55),
    ];
    let program = lives(&body);
    let mut states = record(&program, 30);
    let first = resets(&states)[0];
    keep_in_qemu(&mut states, S1, 0x55, first + 3);
    let (end, _, _) = compare(&text(&states), &program, &window(KEPT_S1, 1));
    assert!(
        matches!(&end, End::Divergence(report) if report.cause == Cause::ReadKeptAtReset { field: Field::X(S1) }),
        "{end:?}"
    );
}

const CLKDIV: Register = Register {
    address: 0x6000_0014,
    mask: 0x0000_00FF,
    qemu: None,
    after_command: None,
};

/// `UART0.CLKDIV` after a reset of both sides: QEMU must read `0xB9` in the low byte, its reset
/// value in these traces (inkemu's is `0x2B6`, kept through the CPU reset).
const AFTER_RESET: &[Difference] = &[Difference {
    number: 1,
    summary: "UART0.CLKDIV, after a reset",
    treatments: &[Treatment::RegistersAfterReset(&[AfterReset {
        register: CLKDIV,
        inkemu: None,
        qemu: Some(0xB9),
    }])],
}];

/// The same row declaring another value for QEMU.
const AFTER_RESET_OTHER_VALUE: &[Difference] = &[Difference {
    number: 1,
    summary: "UART0.CLKDIV, after a reset, another value",
    treatments: &[Treatment::RegistersAfterReset(&[AfterReset {
        register: CLKDIV,
        inkemu: None,
        qemu: Some(0xB6),
    }])],
}];

/// `UART0.CLKDIV` after the machine resets UART0 (bit 2 of `PERIP_RST_EN0`), which QEMU
/// ignores: inkemu must read its reset value, `0x2B6`.
const AFTER_PULSE: &[Difference] = &[Difference {
    number: 1,
    summary: "UART0.CLKDIV, after a reset of UART0",
    treatments: &[Treatment::RegistersAfterReset(&[AfterReset {
        register: Register {
            mask: 0x0000_03FF,
            ..CLKDIV
        },
        inkemu: Some(0x2B6),
        qemu: None,
    }])],
}];

const AFTER_PULSE_OTHER_VALUE: &[Difference] = &[Difference {
    number: 1,
    summary: "UART0.CLKDIV, after a reset of UART0, another value",
    treatments: &[Treatment::RegistersAfterReset(&[AfterReset {
        register: Register {
            mask: 0x0000_03FF,
            ..CLKDIV
        },
        inkemu: Some(0x123),
        qemu: None,
    }])],
}];

#[test]
fn a_copy_after_a_reset_of_a_module_qemu_ignores_holds_only_from_inkemus_reset_value() {
    // The life sets CLKDIV, then pulses UART0's reset, then reads CLKDIV: inkemu reads its
    // reset value, QEMU, which ignores the pulse, the value the life set.
    let perip_rst_en0 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN0");
    let mut body = Vec::new();
    body.extend(store_register(0x6000_0014, 0x123));
    body.extend(store_register(perip_rst_en0, 1 << 2));
    body.extend(store_register(perip_rst_en0, 0));
    body.extend([lui(5, 0x6000_0000), lw(A1, 5, 0x14)]);
    let program = lives(&body);
    let states = record(&program, 60);
    let at = resets(&states);
    let load = body.len() - 1;
    let mut qemu = states;
    for state in &mut qemu[load + 1..at[0]] {
        state.set(Field::X(A1), 0x123);
    }
    let trace = text(&qemu[..at[0]]);
    let (end, lockstep, target) = compare(&trace, &program, &window(AFTER_PULSE, 0));
    assert_eq!(end, End::Window(WindowEnd::TraceEnd), "{end:?}");
    assert_eq!(lockstep.usage(&target).uses(1), 1);
    // A row that declares another value for inkemu does not admit it.
    let (end, _, _) = compare(&trace, &program, &window(AFTER_PULSE_OTHER_VALUE, 0));
    assert!(
        matches!(&end, End::Divergence(report) if report.index == u64::try_from(load).unwrap()),
        "{end:?}"
    );
}

#[test]
fn a_copy_after_a_reset_of_both_sides_holds_only_from_the_value_qemu_must_show() {
    let program = lives(&[lui(5, 0x6000_0000), lw(A1, 5, 0x14)]);
    let states = record(&program, 30);
    let at = resets(&states);
    let mut second = states;
    for state in &mut second[at[0] + 2..=at[1]] {
        let read = state.x(A1);
        state.set(Field::X(A1), read ^ 0x0F);
    }
    let (end, _, _) = compare(
        &text(&second[..=at[1]]),
        &program,
        &window(AFTER_RESET_OTHER_VALUE, 1),
    );
    assert!(
        matches!(&end, End::Divergence(report) if report.index == u64::try_from(at[0] + 1).unwrap()),
        "{end:?}"
    );
}

#[test]
fn a_register_of_the_rows_after_a_reset_is_copied_only_after_one() {
    let program = lives(&[lui(5, 0x6000_0000), lw(A1, 5, 0x14)]);
    let states = record(&program, 30);
    let at = resets(&states);
    // QEMU reads 0x0F more in the second life's load, the second instruction of it, and a1
    // keeps that value to the end of the life.
    let mut second = states.clone();
    for state in &mut second[at[0] + 2..=at[1]] {
        let read = state.x(A1);
        state.set(Field::X(A1), read ^ 0x0F);
    }
    let (end, lockstep, target) =
        compare(&text(&second[..=at[1]]), &program, &window(AFTER_RESET, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset));
    assert_eq!(lockstep.usage(&target).uses(1), 1);
    // The same in the first life is a divergence.
    let mut first = states;
    for state in &mut first[2..at[0]] {
        let read = state.x(A1);
        state.set(Field::X(A1), read ^ 0x0F);
    }
    let (end, _, _) = compare(&text(&first[..=at[1]]), &program, &window(AFTER_RESET, 1));
    assert!(matches!(end, End::Divergence(_)), "{end:?}");
}

const PMP: &[Difference] = &[Difference {
    number: 1,
    summary: "pmpcfg0, whose R, W, X bits QEMU keeps",
    treatments: &[Treatment::CsrKeptAtReset(&[KeptCsr {
        csr: PMPCFG0,
        kept: PMP_RWX,
        mask: 0x0000_0007,
    }])],
}];

#[test]
fn a_csr_qemu_partly_keeps_reads_as_inkemus_with_the_kept_bits_it_held_before_the_reset() {
    // Each life reads pmpcfg0 into a0, then writes 0x8F to it: locked, TOR, R, W, X.
    let program = lives(&[
        csrrs(A0, PMPCFG0, 0),
        addi(A1, 0, 0x8F),
        csrrw(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 30);
    let at = resets(&states);
    // QEMU's a0 holds what it read from the second life's first instruction to its end.
    let qemu = |read: u32| {
        let mut states = states.clone();
        for state in &mut states[at[0] + 1..=at[1]] {
            state.set(Field::X(A0), read);
        }
        text(&states[..=at[1]])
    };
    // QEMU cleared L and A and kept R, W, X: 0x07.
    let (end, lockstep, target) = compare(&qemu(0x07), &program, &window(PMP, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset));
    assert_eq!(lockstep.usage(&target).uses(1), 1);
    // Keeping L or A as well is not what the row says.
    let (end, _, _) = compare(&qemu(0x8F), &program, &window(PMP, 1));
    assert!(matches!(end, End::Divergence(_)), "{end:?}");
}

#[test]
fn a_csr_qemu_partly_keeps_is_compared_once_the_life_after_the_reset_writes_it() {
    // Each life writes 0 to pmpcfg0 and reads it into a0, then leaves R, W, X set: the second
    // life's read follows its own write, which the kept bits no longer explain.
    let program = lives(&[
        csrrw(0, PMPCFG0, 0),
        csrrs(A0, PMPCFG0, 0),
        addi(A1, 0, 0x07),
        csrrw(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 30);
    let at = resets(&states);
    let mut qemu = states.clone();
    // QEMU reads the R, W, X bits of the first life after the second life wrote 0: a model that
    // lost the write would read so, and the row must not admit it.
    for state in &mut qemu[at[0] + 2..=at[1]] {
        state.set(Field::X(A0), 0x07);
    }
    let (end, _, _) = compare(&text(&qemu[..=at[1]]), &program, &window(PMP, 1));
    assert!(
        matches!(&end, End::Divergence(report) if report.index == u64::try_from(at[0] + 1).unwrap()),
        "{end:?}"
    );
}

#[test]
fn the_bits_a_csrrs_or_csrrc_writes_after_the_reset_are_compared_and_the_others_still_kept() {
    // As ESP-IDF does (PMP_ENTRY_CFG_RESET, then PMP_ENTRY_CFG_SET): the second life clears the
    // R, W, X bits of entry 0 and sets R and X, then reads pmpcfg0; each life ends with R, W, X
    // set, which QEMU keeps through its reset.
    let program = lives(&[
        addi(A1, 0, 0x07),
        csrrc(0, PMPCFG0, A1),
        addi(A1, 0, 0x05),
        csrrs(0, PMPCFG0, A1),
        csrrs(A0, PMPCFG0, 0),
        addi(A1, 0, 0x07),
        csrrs(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 40);
    let at = resets(&states);
    let mut qemu = states.clone();
    // A model that lost the clear of W reads 0x07: the cleared bit is written, so compared.
    for state in &mut qemu[at[0] + 5..=at[1]] {
        state.set(Field::X(A0), 0x07);
    }
    let (end, _, _) = compare(&text(&qemu[..=at[1]]), &program, &window(PMP, 1));
    assert!(
        matches!(&end, End::Divergence(report) if report.index == u64::try_from(at[0] + 4).unwrap()),
        "{end:?}"
    );

    // A csrrs of entry 1 leaves entry 0's kept bits as the reset left them: still admitted.
    let program = lives(&[
        addi(A1, 0, 0x700),
        csrrs(0, PMPCFG0, A1),
        csrrs(A0, PMPCFG0, 0),
        addi(A1, 0, 0x07),
        csrrs(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 40);
    let at = resets(&states);
    let mut qemu = states;
    for state in &mut qemu[at[0] + 3..=at[1]] {
        state.set(Field::X(A0), 0x707);
    }
    let (end, lockstep, target) = compare(&text(&qemu[..=at[1]]), &program, &window(PMP, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset));
    assert_eq!(lockstep.usage(&target).uses(1), 1);
}

/// QEMU's `a0` in `states` set to `read` from state `from` to the end of the life (the state at
/// the next reset excluded: QEMU's register would be kept there, a row of another kind).
fn read_in_qemu(states: &mut [CpuState], from: usize, until: usize, read: u32) {
    for state in &mut states[from..until] {
        state.set(Field::X(A0), read);
    }
}

#[test]
fn a_csrrsi_writes_only_its_immediate() {
    // The second life sets W of entry 0 with an immediate, then reads pmpcfg0: R and X are still
    // the kept bits. sp holds every bit, so that a reading of the immediate as a register (x2)
    // would count them all written.
    let program = lives(&[
        addi(SP, 0, -1),
        csrrsi(0, PMPCFG0, 0x02),
        csrrs(A0, PMPCFG0, 0),
        addi(A1, 0, 0x07),
        csrrs(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 40);
    let at = resets(&states);
    let mut qemu = states;
    read_in_qemu(&mut qemu, at[0] + 3, at[1], 0x07);
    let (end, lockstep, target) = compare(&text(&qemu[..=at[1]]), &program, &window(PMP, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset), "{end:?}");
    assert_eq!(lockstep.usage(&target).uses(1), 1);
}

#[test]
fn a_csrrci_writes_its_immediate_which_is_then_compared() {
    // The second life clears W of entry 0 with an immediate: a model that lost the clear reads
    // R, W, X; one that kept it, R and X. sp holds 0, so that a reading of the immediate as a
    // register (x2) would count nothing written.
    let program = lives(&[
        addi(SP, 0, 0),
        csrrci(0, PMPCFG0, 0x02),
        csrrs(A0, PMPCFG0, 0),
        addi(A1, 0, 0x07),
        csrrs(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 40);
    let at = resets(&states);
    let read = |value: u32| {
        let mut qemu = states.clone();
        read_in_qemu(&mut qemu, at[0] + 3, at[1], value);
        compare(&text(&qemu[..=at[1]]), &program, &window(PMP, 1))
    };
    let (end, lockstep, target) = read(0x05);
    assert_eq!(end, End::Window(WindowEnd::Reset), "{end:?}");
    assert_eq!(lockstep.usage(&target).uses(1), 1);
    let (end, _, _) = read(0x07);
    assert!(
        matches!(&end, End::Divergence(report) if report.index == u64::try_from(at[0] + 2).unwrap()),
        "{end:?}"
    );
}

#[test]
fn a_csrrs_whose_register_holds_0_writes_nothing() {
    // rs1 is not x0, so the instruction writes; its operand is 0, so no bit: R, W, X are still
    // the kept bits.
    let program = lives(&[
        addi(A1, 0, 0),
        csrrs(0, PMPCFG0, A1),
        csrrs(A0, PMPCFG0, 0),
        addi(A1, 0, 0x07),
        csrrs(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 40);
    let at = resets(&states);
    let mut qemu = states;
    read_in_qemu(&mut qemu, at[0] + 3, at[1], 0x07);
    let (end, lockstep, target) = compare(&text(&qemu[..=at[1]]), &program, &window(PMP, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset), "{end:?}");
    assert_eq!(lockstep.usage(&target).uses(1), 1);
}

const MTVEC: u16 = 0x305;
const INSTRET: u16 = 0xC02;

/// `instret`, read-only (a write to it raises an illegal-instruction exception), as a CSR QEMU
/// keeps whole at its reset: inkemu's counter restarts there.
const INSTRET_KEPT: &[Difference] = &[Difference {
    number: 1,
    summary: "instret, kept by QEMU at its reset (a test row)",
    treatments: &[Treatment::CsrKeptAtReset(&[KeptCsr {
        csr: INSTRET,
        kept: u32::MAX,
        mask: u32::MAX,
    }])],
}];

#[test]
fn a_csr_instruction_that_raises_an_exception_writes_nothing() {
    // Each life points mtvec at 0x100 (the C3's vectored base), writes every bit of instret,
    // which traps there, reads instret into a0, then into s1 just before the store that resets
    // the CPU: the value before that store is s1 plus the length of the store's code.
    let mut body = vec![
        lui(T2, ROM_BASE),
        addi(T2, T2, 0x100),
        csrrw(0, MTVEC, T2),
        addi(A1, 0, -1),
        csrrw(0, INSTRET, A1),
    ];
    body.resize(0x100 / 4, NOP);
    body.extend([csrrs(A0, INSTRET, 0), csrrs(S1, INSTRET, 0)]);
    let read = 0x100 / 4;
    let program = lives(&body);
    let states = record(&program, 200);
    let at = resets(&states);
    let store_length = u32::try_from(store_register(options0(), SW_PROCPU_RST).len()).unwrap();
    let before_reset = states[at[0] - 1].x(S1) + store_length;
    // The second life traps after five instructions and reads at the vector.
    let mine = states[at[0] + 6].x(A0);
    assert_eq!(
        states[at[0] + 6].pc(),
        ROM_BASE + 4 * (u32::try_from(read).unwrap() + 1)
    );
    assert_ne!(
        before_reset & !mine,
        0,
        "the test needs bits QEMU keeps and inkemu does not show"
    );
    let mut qemu = states;
    read_in_qemu(&mut qemu, at[0] + 6, at[1], mine | before_reset);
    let (end, lockstep, target) =
        compare(&text(&qemu[..=at[1]]), &program, &window(INSTRET_KEPT, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset), "{end:?}");
    assert_eq!(lockstep.usage(&target).uses(1), 1);
}

#[test]
fn the_bits_written_in_a_life_are_forgotten_at_the_reset_that_ends_it() {
    // Each life reads pmpcfg0 first, then sets R, W, X: the life after a reset reads the kept
    // bits before writing any, through two resets.
    let program = lives(&[
        csrrs(A0, PMPCFG0, 0),
        addi(A1, 0, 0x07),
        csrrs(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 60);
    let at = resets(&states);
    let mut qemu = states;
    read_in_qemu(&mut qemu, at[0] + 1, at[1], 0x07);
    read_in_qemu(&mut qemu, at[1] + 1, at[2], 0x07);
    let (end, lockstep, target) = compare(&text(&qemu[..=at[2]]), &program, &window(PMP, 2));
    assert_eq!(end, End::Window(WindowEnd::Reset), "{end:?}");
    assert_eq!(lockstep.counts().resets_crossed, 2);
    assert_eq!(lockstep.usage(&target).uses(1), 2);
}

#[test]
fn the_bits_qemu_keeps_through_a_life_that_writes_none_are_still_kept_at_the_next_reset() {
    // A counter in SRAM tells the lives apart: the first sets R, W, X, the second writes
    // nothing, the third reads pmpcfg0. QEMU keeps the first life's bits through both resets.
    let program = lives(&[
        lui(T2, SRAM_DBUS_BASE),
        lw(A1, T2, 0),
        addi(A1, A1, 1),
        sw(A1, T2, 0),
        csrrs(A0, PMPCFG0, 0),
        addi(S1, 0, 1),
        bne(A1, S1, 12),
        addi(A1, 0, 0x07),
        csrrs(0, PMPCFG0, A1),
    ]);
    let states = record(&program, 80);
    let at = resets(&states);
    let read = |value: u32| {
        let mut qemu = states.clone();
        read_in_qemu(&mut qemu, at[0] + 5, at[1], 0x07);
        read_in_qemu(&mut qemu, at[1] + 5, at[2], value);
        compare(&text(&qemu[..=at[2]]), &program, &window(PMP, 2))
    };
    let (end, lockstep, target) = read(0x07);
    assert_eq!(end, End::Window(WindowEnd::Reset), "{end:?}");
    assert_eq!(lockstep.usage(&target).uses(1), 2);
    // Another value is not what QEMU keeps.
    let (end, _, _) = read(0x05);
    assert!(
        matches!(&end, End::Divergence(report) if report.index == u64::try_from(at[1] + 4).unwrap()),
        "{end:?}"
    );
}

/// `UART0.CLKDIV` after the machine reset UART0 and then a reset of both sides: inkemu, whose
/// CPU reset keeps the module, must show its reset value, whatever QEMU's reset gave.
const AFTER_PULSE_THEN_RESET: &[Difference] = &[Difference {
    number: 1,
    summary: "UART0.CLKDIV, after a reset of UART0 and a reset of both sides",
    treatments: &[Treatment::RegistersAfterReset(&[AfterReset {
        register: Register {
            mask: 0x0000_03FF,
            ..CLKDIV
        },
        inkemu: Some(0x2B6),
        qemu: Some(0xB9),
    }])],
}];

const AFTER_PULSE_THEN_RESET_OTHER_VALUE: &[Difference] = &[Difference {
    number: 1,
    summary: "UART0.CLKDIV, after a reset of UART0 and a reset of both sides, another value",
    treatments: &[Treatment::RegistersAfterReset(&[AfterReset {
        register: Register {
            mask: 0x0000_03FF,
            ..CLKDIV
        },
        inkemu: Some(0x123),
        qemu: Some(0x123),
    }])],
}];

#[test]
fn after_a_pulse_then_a_reset_of_both_sides_the_value_checked_is_inkemus() {
    // Each life reads CLKDIV, sets it, then pulses UART0's reset: the second life reads first
    // what the pulse left in inkemu (its reset value), and QEMU the value the life set.
    let perip_rst_en0 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN0");
    let mut body = vec![lui(5, 0x6000_0000), lw(A1, 5, 0x14)];
    body.extend(store_register(0x6000_0014, 0x123));
    body.extend(store_register(perip_rst_en0, 1 << 2));
    body.extend(store_register(perip_rst_en0, 0));
    let program = lives(&body);
    let states = record(&program, 80);
    let at = resets(&states);
    let mut qemu = states;
    for state in &mut qemu[at[0] + 2..=at[1]] {
        state.set(Field::X(A1), 0x123);
    }
    let trace = text(&qemu[..=at[1]]);
    // QEMU reads neither 0xB9 nor inkemu's value: the pulse decides, inkemu's value is checked.
    let (end, lockstep, target) = compare(&trace, &program, &window(AFTER_PULSE_THEN_RESET, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset), "{end:?}");
    assert_eq!(lockstep.usage(&target).uses(1), 1);
    // A row whose QEMU value matches but whose inkemu value does not is a divergence.
    let (end, _, _) = compare(
        &trace,
        &program,
        &window(AFTER_PULSE_THEN_RESET_OTHER_VALUE, 1),
    );
    assert!(
        matches!(&end, End::Divergence(report) if report.index == u64::try_from(at[0] + 1).unwrap()),
        "{end:?}"
    );
}

#[test]
fn a_register_of_the_rows_after_a_reset_is_compared_once_the_life_after_it_stores_to_it() {
    // The second life stores to UART0.CLKDIV, then loads it: the copy after the reset no longer
    // holds, and a difference is a divergence.
    let program = lives(&[lui(5, 0x6000_0000), sw(0, 5, 0x14), lw(A1, 5, 0x14)]);
    let states = record(&program, 30);
    let at = resets(&states);
    let mut qemu = states;
    for state in &mut qemu[at[0] + 3..=at[1]] {
        let read = state.x(A1);
        state.set(Field::X(A1), read ^ 0x0F);
    }
    let (end, _, _) = compare(&text(&qemu[..=at[1]]), &program, &window(AFTER_RESET, 1));
    assert!(
        matches!(&end, End::Divergence(report) if report.index == u64::try_from(at[0] + 2).unwrap()),
        "{end:?}"
    );
}

const KEPT_S1_AT_1: &[Difference] = &[Difference {
    number: 1,
    summary: "s1, kept by QEMU at its reset, declared 1 at inkemu's",
    treatments: &[Treatment::KeptAtReset(&[kept(Field::X(S1), 1)])],
}];

#[test]
fn a_hold_starts_only_from_the_reset_value_the_row_declares() {
    // inkemu's reset leaves s1 at 0; a row that declares 1 does not hold the difference.
    let program = lives(&[addi(S1, 0, 0x55), addi(A1, S1, 1)]);
    let mut states = record(&program, 16);
    let first = resets(&states)[0];
    keep_in_qemu(&mut states, S1, 0x55, first + 1);
    let (end, _, _) = compare(&text(&states), &program, &window(KEPT_S1_AT_1, 1));
    assert!(
        matches!(&end, End::Divergence(report) if report.cause == Cause::State),
        "{end:?}"
    );
}

const KEPT_MPIE: &[Difference] = &[Difference {
    number: 1,
    summary: "mstatus.MPIE, kept by QEMU at its reset",
    treatments: &[Treatment::KeptAtReset(&[Kept {
        field: Field::Mstatus,
        bits: 1 << 7,
        reset: 0,
    }])],
}];

#[test]
fn of_a_partly_held_csr_only_the_held_bits_may_differ() {
    // Each life writes MPIE at its end; QEMU keeps it through its reset, until the second life
    // writes it again (a write, not a read-modify-write, which would read the held bit).
    let program = lives(&[NOP, NOP, addi(A1, 0, 0x80), csrrw(0, 0x300, A1)]);
    let states = record(&program, 30);
    let at = resets(&states);
    let qemu = |bits: u32| {
        let mut states = states.clone();
        for state in &mut states[at[0]..at[0] + 4] {
            state.set(Field::Mstatus, state.get(Field::Mstatus) | bits);
        }
        text(&states[..=at[1]])
    };
    let (end, lockstep, target) = compare(&qemu(0x80), &program, &window(KEPT_MPIE, 1));
    assert_eq!(end, End::Window(WindowEnd::Reset));
    assert!(lockstep.usage(&target).uses(1) > 0);
    // MIE as well is not what the row holds.
    let (end, _, _) = compare(&qemu(0x88), &program, &window(KEPT_MPIE, 1));
    assert!(matches!(end, End::Divergence(_)), "{end:?}");
}

#[test]
fn a_window_ends_at_its_bound_and_holds_its_rows_whole_there() {
    let program = lives(&[NOP, NOP, NOP]);
    let states = record(&program, 10);
    let bounded = Window {
        bound: Some(2),
        ..window(KEPT_S1, 0)
    };
    let (end, lockstep, target) = compare(&text(&states), &program, &bounded);
    assert_eq!(end, End::Window(WindowEnd::Bound));
    assert_eq!(lockstep.counts().compared, 2);
    let check = lockstep.check_usage(&target, &WindowEnd::Bound);
    assert!(
        !check.passes(),
        "a bound is a whole window: the unused row fails it"
    );
}
