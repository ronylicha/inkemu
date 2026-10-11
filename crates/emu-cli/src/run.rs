//! `inkemu run`: drives a machine and relays its console and diagnostics.
use emu_core::Cycles;
use soc_esp32c3::{ConsoleChannel, Esp32C3, StopReason};
use std::io::{self, Write};

/// Cycles executed between two flushes of the console and the diagnostics: a fraction of a second
/// of host time, so output appears as the firmware prints it.
const SLICE_CYCLES: u64 = 1_000_000;

#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    /// Stop when the CPU cycle counter reaches this value; `None` runs until the machine stops by itself.
    pub max_cycles: Option<u64>,
    pub console: ConsoleChannel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunEnd {
    Budget,
    /// Whoever was reading the console went away (`inkemu run | head`): a normal way to end.
    ConsoleClosed,
    /// The console could not be written for any other reason.
    ConsoleError,
    StrictMmio,
    Idle,
}

impl RunEnd {
    /// The process exit code of a run that ended this way.
    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            RunEnd::Budget | RunEnd::ConsoleClosed => 0,
            RunEnd::ConsoleError => 2,
            RunEnd::StrictMmio => 3,
            RunEnd::Idle => 4,
        }
    }
}

/// Writes `bytes` to `out` and flushes; `Ok(false)` when the reader is gone (a broken pipe).
///
/// # Errors
/// Any write or flush failure other than a broken pipe.
pub fn relay(out: &mut dyn Write, bytes: &[u8]) -> io::Result<bool> {
    if bytes.is_empty() {
        return Ok(true);
    }
    match out.write_all(bytes).and_then(|()| out.flush()) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

/// Runs `machine` until it stops, sending the firmware console to `console` and everything else
/// (warnings, the reason the run ended, the summary) to `log`.
///
/// Output is relayed every [`SLICE_CYCLES`] cycles so a long run shows progress.
pub fn execute(
    machine: &mut Esp32C3,
    options: &RunOptions,
    console: &mut dyn Write,
    log: &mut dyn Write,
) -> RunEnd {
    let end = loop {
        let now = machine.cycles().get();
        let slice_end = now.saturating_add(SLICE_CYCLES);
        let limit = options
            .max_cycles
            .map_or(slice_end, |max| max.min(slice_end));
        let stop = machine.run_until(Cycles(limit));
        let console_open = relay(console, &machine.take_console(options.console));
        for diagnostic in machine.drain_diagnostics() {
            // A failing log writer must not hide the run's outcome: there is nowhere left to report it.
            let _ = writeln!(log, "warning: {diagnostic}");
        }
        match (console_open, stop) {
            (Ok(false), _) => break RunEnd::ConsoleClosed,
            (Err(e), _) => {
                let _ = writeln!(
                    log,
                    "error: cannot write the console output: {e}; check where stdout is redirected"
                );
                break RunEnd::ConsoleError;
            }
            (Ok(true), StopReason::StrictMmio(diagnostic)) => {
                let _ = writeln!(log, "inkemu: stopped by --strict-mmio: {diagnostic}");
                break RunEnd::StrictMmio;
            }
            (Ok(true), StopReason::Idle) => {
                let _ = writeln!(
                    log,
                    "inkemu: the CPU is waiting for an interrupt and nothing can raise one; interrupt sources are not emulated yet"
                );
                break RunEnd::Idle;
            }
            (Ok(true), StopReason::CycleBudget)
                if options
                    .max_cycles
                    .is_some_and(|max| machine.cycles().get() >= max) =>
            {
                let _ = writeln!(
                    log,
                    "inkemu: cycle limit reached; raise --max-cycles to run longer"
                );
                break RunEnd::Budget;
            }
            (Ok(true), StopReason::CycleBudget) => {}
        }
    };
    let _ = writeln!(
        log,
        "inkemu: stopped at pc {:#010x} after {} cycles ({} instructions)",
        machine.pc(),
        machine.cycles().get(),
        machine.instructions()
    );
    if let Some(hash) = machine.trace_hash() {
        let _ = writeln!(log, "trace-hash {hash:016x}");
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use boards::MachineOptions;
    use cpu_rv32::asm::{addi, lui, lw, sw, wfi};
    use soc_esp32c3::test_support::flat_rom;

    const HANG: u32 = 0x0000_006f;
    const UART0: u32 = 0x6000_0000;
    const USB: u32 = 0x6004_3000;

    fn machine(program: &[u32], options: MachineOptions) -> Esp32C3 {
        boards::builtin("x4")
            .unwrap()
            .instantiate(flat_rom(program), options)
    }
    fn printing(base: u32, text: &[u8], tail: &[u32]) -> Vec<u32> {
        let mut program = vec![lui(5, base)];
        for &byte in text {
            program.extend([addi(6, 0, i32::from(byte)), sw(6, 5, 0)]);
        }
        program.extend_from_slice(tail);
        program
    }
    fn options(max_cycles: u64) -> RunOptions {
        RunOptions {
            max_cycles: Some(max_cycles),
            console: ConsoleChannel::Uart0,
        }
    }
    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }
    fn ledc_register() -> u32 {
        svd_regs::esp32c3::registers()
            .iter()
            .find(|r| r.peripheral == "LEDC")
            .unwrap()
            .addr
    }

    #[test]
    fn console_bytes_go_to_the_console_writer_and_nothing_else_does() {
        let mut m = machine(
            &printing(UART0, b"hello\n", &[HANG]),
            MachineOptions::default(),
        );
        let (mut console, mut log) = (Vec::new(), Vec::new());
        assert_eq!(
            execute(&mut m, &options(10_000), &mut console, &mut log),
            RunEnd::Budget
        );
        assert_eq!(console, b"hello\n");
        assert!(!text(&log).contains("hello"));
    }

    #[test]
    fn max_cycles_ends_the_run_cleanly() {
        let mut m = machine(&[HANG], MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(&mut m, &options(2_500_000), &mut console, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::Budget, 0));
        assert!(
            m.cycles().get() >= 2_500_000 && m.cycles().get() < 2_500_100,
            "the limit is honoured across slices"
        );
        let log = text(&log);
        assert!(
            log.contains("cycle limit")
                && log.contains("--max-cycles")
                && log.contains("0x40000000"),
            "{log}"
        );
    }

    #[test]
    fn selected_console_channel_is_the_one_printed() {
        let program = printing(USB, b"usb", &[addi(6, 0, 1), sw(6, 5, 4), HANG]);
        let mut m = machine(&program, MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        execute(
            &mut m,
            &RunOptions {
                max_cycles: Some(10_000),
                console: ConsoleChannel::UsbSerialJtag,
            },
            &mut console,
            &mut log,
        );
        assert_eq!(console, b"usb");
    }

    #[test]
    fn unmodeled_access_is_a_named_warning_and_the_run_goes_on() {
        let mut m = machine(
            &[lui(5, ledc_register()), lw(6, 5, 0), HANG],
            MachineOptions::default(),
        );
        let (mut console, mut log) = (Vec::new(), Vec::new());
        assert_eq!(
            execute(&mut m, &options(10_000), &mut console, &mut log),
            RunEnd::Budget
        );
        let log = text(&log);
        assert_eq!(log.matches("warning: ").count(), 1, "{log}");
        assert!(log.contains("LEDC") && log.contains("0x40000004"), "{log}");
    }

    #[test]
    fn strict_mode_ends_with_its_own_exit_code_and_names_the_register() {
        let mut m = machine(
            &[lui(5, ledc_register()), lw(6, 5, 0), HANG],
            MachineOptions {
                strict_mmio: true,
                trace_hash: false,
            },
        );
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(&mut m, &options(10_000), &mut console, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::StrictMmio, 3));
        let log = text(&log);
        assert!(
            log.contains("--strict-mmio") && log.contains("LEDC"),
            "{log}"
        );
    }

    #[test]
    fn idle_cpu_ends_the_run_and_says_why() {
        let mut m = machine(&[wfi(), HANG], MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut m,
            &RunOptions {
                max_cycles: None,
                console: ConsoleChannel::Uart0,
            },
            &mut console,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::Idle, 4));
        assert!(
            text(&log).contains("waiting for an interrupt"),
            "{}",
            text(&log)
        );
    }

    #[test]
    fn trace_hash_is_printed_when_enabled() {
        let run = || {
            let mut m = machine(
                &printing(UART0, b"x", &[HANG]),
                MachineOptions {
                    strict_mmio: false,
                    trace_hash: true,
                },
            );
            let (mut console, mut log) = (Vec::new(), Vec::new());
            execute(&mut m, &options(5_000), &mut console, &mut log);
            text(&log)
                .lines()
                .find(|l| l.starts_with("trace-hash "))
                .map(str::to_owned)
        };
        let line = run().expect("a trace-hash line");
        assert_eq!(line.len(), "trace-hash ".len() + 16);
        assert_eq!(Some(line), run());
    }

    /// A console whose reader went away, like `inkemu run | head -1`.
    struct Closed;
    impl io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn closed_stdout_does_not_panic() {
        let mut m = machine(
            &printing(UART0, b"hello\n", &[HANG]),
            MachineOptions::default(),
        );
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &RunOptions {
                max_cycles: None,
                console: ConsoleChannel::Uart0,
            },
            &mut Closed,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleClosed, 0));
    }

    struct Failing;
    impl io::Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn console_write_error_is_reported_and_ends_the_run_with_an_error_code() {
        let mut m = machine(
            &printing(UART0, b"hello\n", &[HANG]),
            MachineOptions::default(),
        );
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &RunOptions {
                max_cycles: None,
                console: ConsoleChannel::Uart0,
            },
            &mut Failing,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleError, 2));
        assert!(text(&log).contains("disk full"), "{}", text(&log));
    }
}
