//! A reset of both sides inside a window (SP1b-4d, decisions J1–J3): the window ends at it, or
//! goes on through it when the window crosses it, with nothing of the machine touched; what the
//! reset leaves different is held by the rows of the window, exactly as they declare it, and is a
//! divergence otherwise. The traces are written from a machine, as in `policy.rs`, then changed
//! where QEMU's reset of its whole machine leaves another state than the chip's CPU reset.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test: a panic is a test failure

use cpu_rv32::asm::{addi, csrrs, csrrw, lui, lw, sw};
use lockstep::table::PMP_RWX;
use lockstep::{
    Cause, CpuState, Difference, End, Field, KeptCsr, Location, Lockstep, Register, Site, Target,
    TraceReader, Treatment, Window, WindowEnd, render_block,
};
use soc_esp32c3::memmap::{ROM_BASE, SRAM_DBUS_BASE};
use soc_esp32c3::test_support::{HANG, flat_rom, store_register};
use soc_esp32c3::{Esp32C3, SocConfig};

const S1: u8 = 9;
const A0: u8 = 10;
const A1: u8 = 11;
const T2: u8 = 7;
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
fn kept(states: &mut [CpuState], reg: u8, value: u32, written: usize) {
    let first = resets(states)[0];
    for state in &mut states[first..written] {
        state.set(Field::X(reg), value);
    }
}

const KEPT_S1: &[Difference] = &[Difference {
    number: 1,
    summary: "s1 and a0, kept by QEMU at its reset",
    treatments: &[Treatment::KeptAtReset(&[Field::X(S1), Field::X(A0)])],
}];

#[test]
fn a_register_qemu_keeps_through_its_reset_is_held_until_written_and_a_read_of_it_diverges() {
    // Each life writes s1, the second one only after an addition that reads it.
    let program = lives(&[addi(A1, S1, 1), addi(S1, 0, 0x55)]);
    let mut states = record(&program, 16);
    let first = resets(&states)[0];
    let end = states.len();
    kept(&mut states, S1, 0x55, end);
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
    kept(&mut states, S1, 0x55, first + 1);
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
    kept(&mut states, S1, 0x56, first + 1);
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
    kept(&mut states, S1, 0x55, first + 4);
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
    kept(&mut states, S1, 0x55, first + 3);
    let (end, _, _) = compare(&text(&states), &program, &window(KEPT_S1, 1));
    assert!(
        matches!(&end, End::Divergence(report) if report.cause == Cause::ReadKeptAtReset { field: Field::X(S1) }),
        "{end:?}"
    );
}

const AFTER_RESET: &[Difference] = &[Difference {
    number: 1,
    summary: "UART0.CLKDIV, after a reset",
    treatments: &[Treatment::RegistersAfterReset(&[Register {
        address: 0x6000_0014,
        mask: 0x0000_00FF,
        qemu: None,
        after_command: None,
    }])],
}];

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
