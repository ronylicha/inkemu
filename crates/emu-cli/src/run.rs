//! `inkemu run`: drives a machine and relays its console and diagnostics.
use crate::exit::{EXIT_ERROR, EXIT_IDLE, EXIT_OK, EXIT_STRICT_MMIO};
use emu_core::Cycles;
use soc_esp32c3::diag::MAX_DISTINCT_DIAGNOSTICS;
use soc_esp32c3::{ConsoleChannel, Esp32C3, StopReason};
use std::io::{self, Write};

/// Cycles executed between two flushes of the console and the diagnostics: a fraction of a second
/// of host time, so output appears as the firmware prints it.
const SLICE_CYCLES: u64 = 1_000_000;

/// How many of the most repeated diagnostics the end-of-run summary lists.
const MAX_REPEATED_SHOWN: usize = 3;

/// Both consoles are drained every slice: the one that is not selected would otherwise grow without bound.
const CONSOLE_CHANNELS: [ConsoleChannel; 2] =
    [ConsoleChannel::Uart0, ConsoleChannel::UsbSerialJtag];

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
            RunEnd::Budget | RunEnd::ConsoleClosed => EXIT_OK,
            RunEnd::ConsoleError => EXIT_ERROR,
            RunEnd::StrictMmio => EXIT_STRICT_MMIO,
            RunEnd::Idle => EXIT_IDLE,
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

/// Takes the bytes of every console and relays those of `selected`; the others are discarded.
fn relay_selected_console(
    machine: &mut Esp32C3,
    selected: ConsoleChannel,
    console: &mut dyn Write,
) -> io::Result<bool> {
    let mut outcome = Ok(true);
    for channel in CONSOLE_CHANNELS {
        let bytes = machine.take_console(channel);
        if channel == selected {
            outcome = relay(console, &bytes);
        }
    }
    outcome
}

/// Says what the firmware kept doing: the warnings list each diagnostic once, so a register polled
/// a million times looks like a single line without this.
fn summarize_diagnostics(machine: &Esp32C3, log: &mut dyn Write) {
    for (diagnostic, count) in machine.most_repeated_diagnostics(MAX_REPEATED_SHOWN) {
        let _ = writeln!(log, "inkemu: repeated {count} times: {diagnostic}");
    }
    let suppressed = machine.suppressed_diagnostics();
    if suppressed > 0 {
        let _ = writeln!(
            log,
            "inkemu: {suppressed} further diagnostics were not listed (more than {MAX_DISTINCT_DIAGNOSTICS} distinct)"
        );
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
        let console_open = relay_selected_console(machine, options.console, console);
        // The stop line carries the violation: listing it as a warning too would print it twice.
        let violation = match &stop {
            StopReason::StrictMmio(diagnostic) => Some(diagnostic),
            _ => None,
        };
        for diagnostic in machine.drain_diagnostics() {
            if Some(&diagnostic) != violation {
                // A failing log writer must not hide the run's outcome: there is nowhere left to report it.
                let _ = writeln!(log, "warning: {diagnostic}");
            }
        }
        // A failure to write the console is logged whatever else ended the run in the same slice.
        if let Err(e) = &console_open {
            let _ = writeln!(
                log,
                "error: cannot write the console output: {e}; check where stdout is redirected"
            );
        }
        match (console_open, stop) {
            // `--strict-mmio` exists for CI: its exit code must not depend on the state of the console.
            (_, StopReason::StrictMmio(diagnostic)) => {
                let _ = writeln!(log, "inkemu: stopped by --strict-mmio: {diagnostic}");
                break RunEnd::StrictMmio;
            }
            (Ok(false), _) => break RunEnd::ConsoleClosed,
            (Err(_), _) => break RunEnd::ConsoleError,
            (Ok(true), StopReason::Idle) => {
                let _ = writeln!(
                    log,
                    "inkemu: the CPU waits for an interrupt and none can come: no interrupt line is pending and no device has anything scheduled; check that the firmware enables the interrupt it waits for"
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
            // The CPU sleeps and device time goes on: the next slice continues.
            (Ok(true), StopReason::CycleBudget | StopReason::Asleep) => {}
        }
    };
    // What the firmware had written and the device had not sent yet: without it the console
    // would end before the place the run stopped at.
    if !matches!(end, RunEnd::ConsoleClosed | RunEnd::ConsoleError) {
        let _ = relay(console, &machine.flush_console(options.console));
    }
    let _ = writeln!(
        log,
        "inkemu: stopped at pc {:#010x} after {} cycles ({} instructions)",
        machine.pc(),
        machine.cycles().get(),
        machine.instructions()
    );
    let _ = writeln!(
        log,
        "inkemu: {} us of device time; CPU clock at {}",
        machine.device_time_nanos() / 1_000,
        frequency_text(machine.cpu_frequency().hz())
    );
    summarize_diagnostics(machine, log);
    if let Some(hash) = machine.trace_hash() {
        let _ = writeln!(log, "trace-hash {hash:016x}");
    }
    end
}

/// A frequency in the largest unit that states it exactly: `80 MHz`, `625 kHz`, `39062 Hz`. Never a
/// rounded figure: the crystal divider can bring the CPU clock far below 1 MHz, where a number of
/// MHz would read 0.
fn frequency_text(hz: u64) -> String {
    const KILO: u64 = 1_000;
    const MEGA: u64 = 1_000_000;
    if hz.is_multiple_of(MEGA) {
        format!("{} MHz", hz / MEGA)
    } else if hz.is_multiple_of(KILO) {
        format!("{} kHz", hz / KILO)
    } else {
        format!("{hz} Hz")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boards::MachineOptions;
    use cpu_rv32::asm::{addi, jal, sw, wfi};
    use soc_esp32c3::test_support::{
        HANG, T0, T1, flat_rom, load_register, print_program, stop_watchdogs, unmodeled_register,
    };
    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    const USB: u32 = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");

    fn machine(program: &[u32], options: MachineOptions) -> Esp32C3 {
        boards::builtin("x4")
            .unwrap()
            .instantiate(flat_rom(program), options)
    }
    fn printing(base: u32, text: &[u8], tail: &[u32]) -> Vec<u32> {
        let mut program = print_program(base, text);
        program.extend_from_slice(tail);
        program
    }
    /// A program that reads the unmodelled register once, then parks.
    fn reading_unmodeled_register() -> Vec<u32> {
        let mut program = load_register(unmodeled_register().addr).to_vec();
        program.push(HANG);
        program
    }
    const STRICT: MachineOptions = MachineOptions {
        strict_mmio: true,
        trace_hash: false,
    };
    fn options(max_cycles: u64) -> RunOptions {
        RunOptions {
            max_cycles: Some(max_cycles),
            console: ConsoleChannel::Uart0,
        }
    }
    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn console_bytes_go_to_the_console_writer_and_nothing_else_does() {
        let mut m = machine(
            &printing(UART0, b"hello\n", &[HANG]),
            MachineOptions::default(),
        );
        let (mut console, mut log) = (Vec::new(), Vec::new());
        assert_eq!(
            // A byte takes 6 940 CPU cycles to leave the UART at the reset baud rate and clock.
            execute(&mut m, &options(60_000), &mut console, &mut log),
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
        assert!(
            log.contains("us of device time") && log.contains("CPU clock at 20 MHz"),
            "{log}"
        );
    }

    #[test]
    fn a_frequency_is_printed_exactly_and_never_as_zero() {
        for (hz, text) in [
            (160_000_000, "160 MHz"),
            (20_000_000, "20 MHz"),
            (1_000_000, "1 MHz"),
            (625_000, "625 kHz"),        // 40 MHz / 64
            (39_062, "39062 Hz"),        // 40 MHz / 1024, the lowest crystal division, rounded down
            (13_333_333, "13333333 Hz"), // 40 MHz / 3
            (1, "1 Hz"),
        ] {
            assert_eq!(frequency_text(hz), text);
        }
    }

    #[test]
    fn selected_console_channel_is_the_one_printed() {
        let program = printing(USB, b"usb", &[addi(T1, 0, 1), sw(T1, T0, 4), HANG]);
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
        let mut m = machine(&reading_unmodeled_register(), MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        assert_eq!(
            execute(&mut m, &options(10_000), &mut console, &mut log),
            RunEnd::Budget
        );
        let log = text(&log);
        assert_eq!(log.matches("warning: ").count(), 1, "{log}");
        assert!(log.contains("LEDC") && log.contains("0x40000004"), "{log}");
        assert!(
            !log.contains("repeated"),
            "a single access is not summarized as repeated: {log}"
        );
    }

    #[test]
    fn a_register_the_firmware_keeps_polling_is_summarized_with_its_count() {
        // lui, lw, then `jal -4` back to the lw: the same unmodelled read, over and over.
        let mut program = load_register(unmodeled_register().addr).to_vec();
        program.push(jal(0, -4));
        let mut m = machine(&program, MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        execute(&mut m, &options(10_000), &mut console, &mut log);
        let log = text(&log);
        assert_eq!(log.matches("warning: ").count(), 1, "{log}");
        let repeated: Vec<&str> = log
            .lines()
            .filter(|l| l.starts_with("inkemu: repeated "))
            .collect();
        assert_eq!(repeated.len(), 1, "{log}");
        assert!(repeated[0].contains("LEDC."), "{}", repeated[0]);
        let count: u64 = repeated[0]
            .trim_start_matches("inkemu: repeated ")
            .split(' ')
            .next()
            .and_then(|n| n.parse().ok())
            .expect("a count");
        assert!(count > 1, "{}", repeated[0]);
    }

    #[test]
    fn strict_mode_ends_with_its_own_exit_code_and_names_the_register() {
        let mut m = machine(&reading_unmodeled_register(), STRICT);
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(&mut m, &options(10_000), &mut console, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::StrictMmio, 3));
        let log = text(&log);
        assert!(
            log.contains("--strict-mmio") && log.contains("LEDC"),
            "{log}"
        );
        assert_eq!(
            log.matches("LEDC").count(),
            1,
            "the violation is printed once, in the stop line: {log}"
        );
        assert!(!log.contains("warning:"), "{log}");
    }

    #[test]
    fn a_strict_stop_wins_over_a_console_whose_reader_went_away() {
        // The console has bytes to relay in the very slice that hits the violation.
        let program = printing(UART0, b"x", &reading_unmodeled_register());
        let mut m = machine(&program, STRICT);
        let mut log = Vec::new();
        let end = execute(&mut m, &options(10_000), &mut Closed, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::StrictMmio, 3));
        assert!(
            text(&log).contains("stopped by --strict-mmio"),
            "{}",
            text(&log)
        );
    }

    #[test]
    fn the_console_that_is_not_selected_is_drained_too() {
        let mut program = printing(USB, b"jtag", &[addi(T1, 0, 1), sw(T1, T0, 4)]);
        program.extend(printing(UART0, b"uart", &[HANG]));
        let mut m = machine(&program, MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        execute(&mut m, &options(10_000), &mut console, &mut log);
        assert_eq!(console, b"uart");
        assert_eq!(
            m.take_console(ConsoleChannel::UsbSerialJtag),
            Vec::<u8>::new(),
            "the USB-Serial-JTAG bytes were discarded, not kept"
        );
    }

    #[test]
    fn idle_cpu_ends_the_run_and_says_why() {
        // With its watchdogs stopped, nothing on the device has anything scheduled.
        let mut program = stop_watchdogs();
        program.extend([wfi(), HANG]);
        let mut m = machine(&program, MachineOptions::default());
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
            text(&log).contains("waits for an interrupt and none can come"),
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

    #[test]
    fn the_console_shows_what_was_written_when_the_run_stopped() {
        // Five bytes need 34 700 cycles to leave at the reset baud rate; the run stops long
        // before, on the unmodelled register the firmware reads right after printing.
        let mut program = printing(UART0, b"hello", &[]);
        program.extend(reading_unmodeled_register());
        let mut m = machine(&program, STRICT);
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(&mut m, &options(10_000), &mut console, &mut log);
        assert_eq!(end, RunEnd::StrictMmio);
        assert_eq!(
            console, b"hello",
            "the bytes still in the UART are not lost"
        );
    }
}
