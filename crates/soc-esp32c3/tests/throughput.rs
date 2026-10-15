//! The throughput of the machine: instructions per second of host time, for three programs that
//! stress what the machine counts for a resource monitor (design `SP4b`, 4b-D11): pure
//! arithmetic, loads and stores in the SRAM (whose pages the RAM versions), and reads of a
//! peripheral register (each counted by block). It measures the host, so it is ignored by
//! default and says what it measured instead of asserting a speed; the same file built on two
//! commits compares them. Run it in release:
//!
//! ```text
//! cargo test --release -p soc-esp32c3 --test throughput -- --ignored --nocapture
//! ```
// A bench reads the host clock and prints what it measured, which the crate's `clippy.toml`
// refuses to the model.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use cpu_rv32::asm::{add, addi, jal, lui, lw, sub, sw};
use emu_core::Cycles;
use soc_esp32c3::memmap::SRAM_DBUS_BASE;
use soc_esp32c3::test_support::{T0, T1, T2, flat_rom, stop_watchdogs};
use soc_esp32c3::{Esp32C3, SocConfig, StopReason};
use std::time::Instant;

/// CPU cycles each program runs for, per pass.
const CYCLES: u64 = 200_000_000;
/// Passes of each program; the fastest is kept, the one least disturbed by the host.
const PASSES: usize = 5;

const T3: u8 = 28;

/// Twelve additions and subtractions on registers, then back.
fn arithmetic() -> Vec<u32> {
    let mut program = stop_watchdogs();
    program.extend([addi(T1, 0, 1), addi(T2, 0, 3)]);
    let body = [
        add(T1, T1, T2),
        sub(T2, T2, T1),
        add(T3, T3, T1),
        sub(T1, T1, T3),
    ];
    for _ in 0..3 {
        program.extend(body);
    }
    program.push(jal(0, -48));
    program
}

/// Four stores and loads in the SRAM, across two of its pages, then back.
fn sram() -> Vec<u32> {
    let mut program = stop_watchdogs();
    program.extend([lui(T0, SRAM_DBUS_BASE + 0x1000), addi(T1, 0, 0)]);
    program.push(addi(T1, T1, 1));
    for offset in [-0x800, -0x400, 0, 0x400] {
        program.extend([sw(T1, T0, offset), lw(T2, T0, offset)]);
    }
    program.push(jal(0, -36));
    program
}

/// Four reads of `SYSTIMER.UNIT0_VALUE_LO`, then back.
fn registers() -> Vec<u32> {
    let register = svd_regs::esp32c3::addr("SYSTIMER", "UNIT0_VALUE_LO");
    let page = register & 0xFFFF_F000;
    let offset = (register - page).cast_signed();
    let mut program = stop_watchdogs();
    program.push(lui(T0, page));
    for _ in 0..4 {
        program.push(lw(T1, T0, offset));
    }
    program.push(jal(0, -16));
    program
}

/// Millions of instructions per second of host time, the best of [`PASSES`] passes.
fn throughput(program: &[u32]) -> f64 {
    let mut best: f64 = 0.0;
    for _ in 0..PASSES {
        let mut machine = Esp32C3::new(flat_rom(program), &SocConfig::default());
        let start = Instant::now();
        let end = machine.run_until(Cycles(CYCLES));
        let seconds = start.elapsed().as_secs_f64();
        assert_eq!(end, StopReason::CycleBudget, "the program runs for ever");
        // A count of instructions below 2^52 is exact as `f64`.
        #[allow(clippy::cast_precision_loss)]
        let mips = machine.instructions() as f64 / seconds / 1e6;
        best = best.max(mips);
    }
    best
}

#[test]
#[ignore = "measures the host: run it in release, by hand"]
fn throughput_of_arithmetic_sram_and_register_reads() {
    for (name, program) in [
        ("arithmetic", arithmetic()),
        ("sram", sram()),
        ("registers", registers()),
    ] {
        println!(
            "{name}: {:.1} million instructions per second",
            throughput(&program)
        );
    }
}
