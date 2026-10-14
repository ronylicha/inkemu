//! The policy of the comparison on traces written from a machine: what QEMU would print for a
//! program, made by running the program on a second machine and writing its states as `-d cpu`
//! blocks. A trace of the same program compares to its end; a trace of a changed program, or a
//! trace with a value changed, diverges unless a row of the table admits the difference.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test: a panic is a test failure

use cpu_rv32::asm::{addi, csrrw, csrrwi, lui, lw};
use lockstep::{CpuState, End, Field, Lockstep, Target, TraceReader, WindowEnd, render_block};
use soc_esp32c3::memmap::{ROM_BASE, SRAM_DBUS_BASE};
use soc_esp32c3::test_support::{HANG, flat_rom, store_register};
use soc_esp32c3::{Esp32C3, SocConfig};

const T0: u8 = 5;
const T1: u8 = 6;
const NOP: u32 = 0x0000_0013;

fn machine(program: &[u32]) -> Esp32C3 {
    Esp32C3::new(flat_rom(program), &SocConfig::default())
}

/// One entry of a trace: a state, or the interrupt QEMU took before the next state.
#[derive(Clone, Copy, Debug)]
enum Entry {
    State(CpuState),
    Interrupt { epc: u32 },
}

/// The states `program` goes through, from power-on, for `steps` instructions, with the
/// interrupts taken between them.
fn record(program: &[u32], steps: usize) -> Vec<Entry> {
    let mut m = machine(program);
    let mut entries = vec![Entry::State(m.state())];
    for _ in 0..steps {
        let stepped = Target::step(&mut m);
        assert_eq!(stepped.stop, None);
        if stepped.interrupt.is_some() {
            entries.push(Entry::Interrupt {
                epc: m.csr(0x341).unwrap(),
            });
        }
        entries.push(Entry::State(m.state()));
    }
    entries
}

/// The text QEMU writes for these entries.
fn text(entries: &[Entry]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("Adding SPI flash device\n");
    for entry in entries {
        match entry {
            Entry::State(state) => out.push_str(&render_block(state)),
            Entry::Interrupt { epc } => {
                let _ = writeln!(
                    out,
                    "riscv_cpu_do_interrupt: hart:0, async:1, cause:0000000b, epc:{epc:#010x}, tval:0x00000000, desc=m_external"
                );
            }
        }
    }
    out
}

/// Compares `target` with the trace, ending after `limit` instructions when one is given.
fn compare(trace: &str, target: &mut Esp32C3, limit: Option<u64>) -> (End, lockstep::Counts) {
    let mut reader = TraceReader::new(trace.as_bytes());
    let mut lockstep = Lockstep::new(target, limit);
    while let Some(event) = reader.next_event().unwrap() {
        if let Some(end) = lockstep.feed(event, target) {
            return (end, lockstep.counts().clone());
        }
    }
    let end = lockstep.finish(target);
    (end, lockstep.counts().clone())
}

/// A program of `n` additions to `x1`, then one that sets `t0`, then a halt.
fn additions(n: usize, t0: i32) -> Vec<u32> {
    let mut program = vec![addi(1, 1, 1); n];
    program.extend([addi(T0, 0, t0), addi(T1, T0, 1), HANG]);
    program
}

#[test]
fn the_same_program_compares_to_the_end_of_its_trace() {
    let program = additions(10, 7);
    let trace = text(&record(&program, 20));
    let (end, counts) = compare(&trace, &mut machine(&program), None);
    assert_eq!(end, End::Window(WindowEnd::TraceEnd));
    assert_eq!(counts.compared, 20);
    // The halt is a branch to itself: its blocks are identical and every one is compared.
    assert_eq!(counts.duplicates, 0);
}

#[test]
fn a_changed_instruction_is_reported_with_both_states() {
    // Acceptance criterion 2: the model executes `addi t0, zero, 7` where the trace was written
    // by a program with `addi t0, zero, 8` at the same place, 70 instructions in.
    let model = additions(70, 7);
    let trace = text(&record(&additions(70, 8), 80));
    let (end, counts) = compare(&trace, &mut machine(&model), None);
    let End::Divergence(report) = end else {
        panic!("no divergence: {end:?}");
    };
    assert_eq!(report.index, 70);
    assert_eq!(counts.compared, 70);
    let executed = report.executed.as_ref().unwrap();
    assert_eq!(executed.pc, ROM_BASE + 4 * 70);
    assert_eq!(executed.instruction.unwrap().raw, addi(T0, 0, 7));
    assert_eq!(report.differing, vec![Field::X(T0)]);
    assert_eq!((report.qemu.x(T0), report.inkemu.x(T0)), (8, 7));
    assert_eq!(
        report.before.x(T0),
        0,
        "the state both sides started it from"
    );
    assert_eq!(report.history.len(), lockstep::HISTORY);
    assert_eq!(report.history.last().unwrap().pc, ROM_BASE + 4 * 69);
    let printed = report.to_string();
    assert!(
        printed.contains("x5/t0          00000000   00000008   00000007  <<"),
        "{printed}"
    );
    assert!(
        printed.contains("the 64 instructions before it"),
        "{printed}"
    );
}

#[test]
fn a_block_printed_twice_is_dropped_once() {
    let program = additions(5, 1);
    let mut entries = record(&program, 8);
    // QEMU logs a block again when its instruction budget runs out (oracle §1c pitfall 1).
    entries.insert(3, entries[2]);
    let (end, counts) = compare(&text(&entries), &mut machine(&program), None);
    assert_eq!(end, End::Window(WindowEnd::TraceEnd));
    assert_eq!((counts.compared, counts.duplicates), (8, 1));
}

/// Software interrupt 0 routed to line 3, `MIE` set, the source raised by a store; the vector
/// table at `ROM_BASE + 0x1000` parks every line.
fn software_interrupt() -> (Vec<u32>, usize) {
    let vectors = ROM_BASE + 0x1000;
    let reg = |p, r| svd_regs::esp32c3::addr(p, r);
    let store = store_register;
    let mut body = vec![lui(T0, vectors), csrrw(0, 0x305, T0)];
    body.extend(store(reg("INTERRUPT_CORE0", "CPU_INTR_FROM_CPU_0_MAP"), 3));
    body.extend(store(reg("INTERRUPT_CORE0", "CPU_INT_PRI_0") + 12, 1));
    body.extend(store(reg("INTERRUPT_CORE0", "CPU_INT_ENABLE"), 1 << 3));
    body.push(csrrwi(0, 0x300, 1 << 3));
    body.extend(store(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
    let steps = body.len() + 4;
    body.extend([NOP; 8]);
    body.resize(0x1000 / 4, HANG);
    body.extend([HANG; 32]);
    (body, steps)
}

#[test]
fn a_block_an_interrupt_preempted_is_dropped() {
    let (program, steps) = software_interrupt();
    let config = lockstep::table::configure(SocConfig::default(), lockstep::DIFFERENCES);
    let record_with = |config: &SocConfig| {
        let mut m = Esp32C3::new(flat_rom(&program), config);
        let mut entries = vec![Entry::State(m.state())];
        let mut preempted = false;
        for _ in 0..steps {
            let before = m.state();
            let stepped = Target::step(&mut m);
            let after = m.state();
            if stepped.interrupt.is_some() {
                // QEMU entered the next block, logged it, then took the interrupt: the state
                // the store left, at the PC the handler returns to.
                let mut entered = before;
                for n in 1..32 {
                    entered.set(Field::X(n), after.x(n));
                }
                entered.set(Field::Pc, after.get(Field::Mepc));
                entries.push(Entry::State(entered));
                entries.push(Entry::Interrupt {
                    epc: after.get(Field::Mepc),
                });
                preempted = true;
            }
            entries.push(Entry::State(after));
        }
        assert!(preempted, "the program takes its interrupt");
        entries
    };
    let entries = record_with(&config);
    let mut target = Esp32C3::new(flat_rom(&program), &config);
    let (end, counts) = compare(&text(&entries), &mut target, None);
    assert_eq!(end, End::Window(WindowEnd::TraceEnd));
    assert_eq!(counts.preempted, 1);
    assert_eq!(counts.interrupts, 1);
    assert_eq!(counts.compared, u64::try_from(steps).unwrap());
}

#[test]
fn qemu_back_at_the_first_instruction_of_the_rom_ends_the_window() {
    let program = additions(5, 1);
    let mut entries = record(&program, 4);
    let Entry::State(first) = entries[0] else {
        panic!("the trace starts with a state");
    };
    entries.push(Entry::State(first));
    let (end, counts) = compare(&text(&entries), &mut machine(&program), None);
    assert_eq!(
        end,
        End::Window(WindowEnd::Reset {
            qemu: true,
            inkemu: false
        })
    );
    assert_eq!(counts.compared, 4);
}

/// `lw t1, offset(t0)` with `t0` = `base`, then a halt.
fn load_from(base: u32, offset: i32) -> Vec<u32> {
    vec![lui(T0, base), lw(T1, T0, offset), NOP, HANG]
}

#[test]
fn an_access_to_a_block_qemu_lacks_ends_the_window_unless_a_row_covers_it() {
    // APB_SARADC: row 18 covers the registers of the bootloader's entropy source, up to
    // `CLKM_CONF` at 0x54; the block goes on to 0xFFF.
    let saradc = 0x6004_0000;
    let program = load_from(saradc, 0x100);
    let (end, _) = compare(&text(&record(&program, 3)), &mut machine(&program), None);
    assert_eq!(
        end,
        End::Window(WindowEnd::Unemulated {
            block: "APB_SARADC",
            address: saradc + 0x100,
            pc: ROM_BASE + 4,
        })
    );
    let program = load_from(saradc, 0x54);
    let (end, counts) = compare(&text(&record(&program, 3)), &mut machine(&program), None);
    assert_eq!(end, End::Window(WindowEnd::TraceEnd));
    assert_eq!(
        counts.rows.get(&18),
        Some(&1),
        "the covered access counts for its row"
    );
}

/// The trace of `program` with `x[reg]` set to `value` from block `from` on.
fn with_register(program: &[u32], steps: usize, from: usize, reg: u8, value: u32) -> String {
    let mut entries = record(program, steps);
    for entry in entries.iter_mut().skip(from) {
        if let Entry::State(state) = entry {
            state.set(Field::X(reg), value);
        }
    }
    text(&entries)
}

#[test]
fn a_load_from_a_rows_range_takes_qemus_value_and_any_other_load_does_not() {
    // Row 17: `RNG.DATA`. QEMU's host entropy gave 0x1234 where inkemu's seed gives another.
    let program = load_from(0x6002_6000, 0xB0);
    let trace = with_register(&program, 3, 2, T1, 0x1234);
    let mut target = machine(&program);
    let (end, counts) = compare(&trace, &mut target, None);
    assert_eq!(end, End::Window(WindowEnd::TraceEnd));
    assert_eq!(counts.rows.get(&17), Some(&1));
    assert_eq!(
        target.x()[usize::from(T1)],
        0x1234,
        "the machine holds QEMU's value"
    );
    // The same difference after a load from SRAM is a divergence.
    let program = load_from(SRAM_DBUS_BASE, 0);
    let trace = with_register(&program, 3, 2, T1, 0x1234);
    let (end, _) = compare(&trace, &mut machine(&program), None);
    assert!(
        matches!(&end, End::Divergence(report) if report.differing == [Field::X(T1)]),
        "{end:?}"
    );
}

#[test]
fn a_mask_settles_its_bits_and_no_other() {
    // Row 2: QEMU's `mie` resets to 0x0eee.
    let program = additions(3, 1);
    let mut entries = record(&program, 5);
    for entry in &mut entries {
        if let Entry::State(state) = entry {
            state.set(Field::Mie, 0x0EEE);
        }
    }
    let (end, counts) = compare(&text(&entries), &mut machine(&program), None);
    assert_eq!(end, End::Window(WindowEnd::TraceEnd));
    assert_eq!(
        counts.rows.get(&2),
        Some(&6),
        "the power-on state and five instructions"
    );
    if let Entry::State(state) = &mut entries[3] {
        state.set(Field::Mie, 0x1EEE);
    }
    let (end, _) = compare(&text(&entries), &mut machine(&program), None);
    let End::Divergence(report) = end else {
        panic!("bit 12 of mie is not masked: {end:?}");
    };
    assert_eq!(report.differing, vec![Field::Mie]);
    assert_eq!(report.nearest.map(|n| n.row), Some(2));
}

#[test]
fn the_limit_ends_the_window_after_that_many_instructions() {
    let program = additions(10, 1);
    let (end, counts) = compare(
        &text(&record(&program, 12)),
        &mut machine(&program),
        Some(5),
    );
    assert_eq!(end, End::Window(WindowEnd::Limit));
    assert_eq!(counts.compared, 5);
}

#[test]
fn the_rows_of_treatment_iv_are_used_from_the_start() {
    let target = machine(&[HANG]);
    let lockstep = Lockstep::new(&target, None);
    assert_eq!(lockstep.counts().rows.get(&24), Some(&1));
    let config = lockstep::table::configure(SocConfig::default(), lockstep::DIFFERENCES);
    assert!(config.qemu_compat.systimer_no_alarm_on_stopped_counter);
    assert_eq!(config.timing, soc_esp32c3::Timing::QemuIcount);
}
