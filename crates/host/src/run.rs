//! `inkemu run`: drives a machine and relays its console and diagnostics.
use crate::exit::{EXIT_ERROR, EXIT_IDLE, EXIT_OK, EXIT_SCRIPT_INCOMPLETE, EXIT_STRICT_MMIO};
use console_script::{ScriptRun, ScriptStatus};
use emu_core::Cycles;
use session::{ConsoleDigests, CutReason, Reached};
use soc_esp32c3::diag::{MAX_DISTINCT_DIAGNOSTICS, MAX_LISTED_RESETS};
use soc_esp32c3::{ConsoleChannel, DeepSleep, Diagnostic, Esp32C3, StopReason};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// Ctrl-C, as the run and the handler of the signal share it: whether the user asked the run to
/// stop, and whether the run has begun its summary.
///
/// The first request is left to the run, which reads it between two calls of the machine and
/// ends with its summary. A second one, while the run has not begun its summary, means that the
/// run does not come back, and the handler ends the process. From the first line of the summary
/// on, no request ends anything: what is left is the summary and the save of the flash file,
/// which a request would cut in two.
#[derive(Debug, Default)]
pub struct Interrupt {
    requested: AtomicBool,
    summarising: AtomicBool,
}

impl Interrupt {
    #[must_use]
    pub const fn new() -> Interrupt {
        Interrupt {
            requested: AtomicBool::new(false),
            summarising: AtomicBool::new(false),
        }
    }

    /// Records that the user asked the run to stop, and says whether the process must end at
    /// once: the request is not the first, and the run has not begun its summary.
    #[must_use]
    pub fn request(&self) -> bool {
        self.requested.swap(true, Ordering::SeqCst) && !self.summarising.load(Ordering::SeqCst)
    }

    /// Whether the user asked the run to stop.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Whether the run has begun its summary: from then on a request ends nothing.
    #[must_use]
    pub fn summary_has_begun(&self) -> bool {
        self.summarising.load(Ordering::SeqCst)
    }

    /// The run is about to write the first line of its summary.
    fn summary_begins(&self) {
        self.summarising.store(true, Ordering::SeqCst);
    }
}

/// How a pass of the run loop ends the run, if it does: what the console relay found and what
/// the machine stopped on. `cycle_limit_reached` tells a slice that ended on `--max-cycles` from
/// one that ended on its own length.
fn pass_end(
    console: &io::Result<bool>,
    stop: &StopReason,
    cycle_limit_reached: bool,
) -> Option<RunEnd> {
    match (console, stop) {
        // `--strict-mmio` exists for CI: its exit code must not depend on the state of the console.
        (_, StopReason::StrictMmio(_)) => Some(RunEnd::StrictMmio),
        (Ok(false), _) => Some(RunEnd::ConsoleClosed),
        (Err(_), _) => Some(RunEnd::ConsoleError),
        (Ok(true), StopReason::Idle) => Some(RunEnd::Idle),
        (Ok(true), StopReason::DeepSleep) => Some(RunEnd::DeepSleep),
        (Ok(true), StopReason::PowerOff) => Some(RunEnd::PowerOff),
        (Ok(true), StopReason::CycleBudget) if cycle_limit_reached => Some(RunEnd::Budget),
        (Ok(true), StopReason::TimeBudget) => Some(RunEnd::TimeLimit),
        // The end of a slice, or a CPU that sleeps while device time goes on: the next pass
        // continues. A sleep that ended on the very instant of the device-time limit comes
        // back as `Asleep` too; the next call then runs nothing and says `TimeBudget`.
        // A watched console spoke: the console script, if any, reads it, and the run goes on.
        (Ok(true), StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput) => {
            None
        }
    }
}

/// The first line of the summary: why the run ended. A console that failed has said so in the
/// line before; `stop` carries the violation of a strict stop.
pub fn say_why(log: &mut dyn Write, end: RunEnd, stop: Option<&StopReason>) {
    let _ = match end {
        RunEnd::Interrupted => writeln!(log, "inkemu: interrupted by the user"),
        RunEnd::StrictMmio => match stop {
            Some(StopReason::StrictMmio(diagnostic)) => {
                writeln!(log, "inkemu: stopped by --strict-mmio: {diagnostic}")
            }
            _ => Ok(()),
        },
        RunEnd::ConsoleClosed => {
            log_reader_gone(log);
            Ok(())
        }
        RunEnd::ConsoleError => Ok(()),
        RunEnd::Idle => writeln!(
            log,
            "inkemu: the CPU waits for an interrupt and none can come: no interrupt line is pending and no device has anything scheduled; check that the firmware enables the interrupt it waits for"
        ),
        // F5: the line after the device time says since when it sleeps and what it waits for.
        RunEnd::DeepSleep => writeln!(
            log,
            "inkemu: the device went into deep sleep and nothing inkemu models can wake it, so the run ends here"
        ),
        RunEnd::PowerOff => writeln!(
            log,
            "inkemu: the device powered off: its board cut its supply, so the run ends here; power it on again to go on"
        ),
        RunEnd::Budget => writeln!(
            log,
            "inkemu: cycle limit reached; raise --max-cycles to run longer"
        ),
        RunEnd::TimeLimit => writeln!(
            log,
            "inkemu: device-time limit reached; raise --max-time to run longer"
        ),
    };
}

/// Cycles executed between two flushes of the console and the diagnostics: a fraction of a second
/// of host time, so output appears as the firmware prints it.
pub const SLICE_CYCLES: u64 = 1_000_000;

/// How many of the most repeated diagnostics the end-of-run summary lists.
const MAX_REPEATED_SHOWN: usize = 3;

/// Passes that wrote nothing to the console between two questions about its reader, when they
/// execute little: a firmware that sleeps between far-apart wake-ups goes through a pass in a
/// fraction of a microsecond of host time, and a system call in each was about half the host
/// time of such a run, measured on one host. The reader's disappearance is still noticed after
/// this many silent passes at most ([`ReaderWatch`] says what bounds the delay); how long that
/// is in host time depends on the host and on what the firmware does.
const SILENT_PASSES_PER_QUESTION: u32 = 64;

/// Decides which of the passes that wrote nothing to the console ask whether its reader is still
/// there. It belongs to the run loop: the machine knows nothing of it, and it enters no trace.
///
/// The first such pass asks. After a question, the next one is asked by the
/// [`SILENT_PASSES_PER_QUESTION`]th pass that wrote nothing, or sooner, by the first of them at
/// which the CPU has executed a whole slice ([`SLICE_CYCLES`]) since the question: a pass that
/// took the host milliseconds does not feel a system call. A pass that wrote changes nothing
/// here: its write found the reader, or failed and ended the run. A firmware that prints in every
/// pass is therefore never asked about, and need not be.
///
/// The delay before a reader that left is noticed, counted from the last write or question: at
/// most [`SILENT_PASSES_PER_QUESTION`] passes, in which the CPU executes less than two slices of
/// cycles. A pass is one call of the machine, which ends on its slice of cycles or after
/// [`soc_esp32c3::machine::SLEEP_SLICE`] of device time or
/// [`soc_esp32c3::machine::MAX_FRUITLESS_WAKES`] sleeps.
///
/// A run that ends by itself inside that delay — on a limit, or because nothing can wake the
/// CPU — ends as what it is, not as a run whose reader left: the end the device reached is the
/// same on every host, and whether the reader had left a moment earlier is host timing. No last
/// question is asked when a run ends.
#[derive(Debug, Default)]
struct ReaderWatch {
    /// Passes that wrote nothing still to go before one asks; 0 at the start: the first asks.
    passes_left: u32,
    /// [`Esp32C3::cycles`] at the last question.
    asked_at: u64,
}

impl ReaderWatch {
    /// Whether a pass that wrote nothing, ended with the CPU at `cycles`, asks.
    fn asks(&mut self, cycles: u64) -> bool {
        let asks = self.passes_left == 0 || cycles.saturating_sub(self.asked_at) >= SLICE_CYCLES;
        if asks {
            self.passes_left = SILENT_PASSES_PER_QUESTION - 1;
            self.asked_at = cycles;
        } else {
            self.passes_left -= 1;
        }
        asks
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    /// Stop when the CPU cycle counter reaches this value; `None` is no limit. It counts the
    /// cycles the CPU executes, not device time: a sleeping CPU executes none, so a firmware that
    /// sleeps reaches it late, and one whose sleep nothing interrupts never does.
    pub max_cycles: Option<u64>,
    /// Stop when this many nanoseconds of device time have elapsed since power-on; `None` is no
    /// limit. Device time goes on while the CPU sleeps and across resets
    /// ([`Esp32C3::run_within`] says what "reached" means, to the cycle of the time base).
    pub max_device_nanos: Option<u64>,
    pub console: ConsoleChannel,
}

/// The value of `--max-time`: seconds of device time, as nanoseconds.
///
/// A decimal number, `DIGITS` optionally followed by `.` and one to nine `DIGITS`: ASCII digits
/// only, no sign, exponent, suffix, separator or space. It is read without floating point, so
/// that the limit of a run is the same number on every host: the whole part times 10⁹ plus the
/// fraction padded to nine digits.
///
/// # Errors
/// The text is not of that form, has a fraction finer than a nanosecond, or is more than a `u64`
/// of nanoseconds (584 years); the message states the cause, then the remedy.
pub fn parse_device_seconds(text: &str) -> Result<u64, String> {
    const NANOS_PER_SECOND: u64 = 1_000_000_000;
    const FRACTION_DIGITS: usize = 9;
    /// The value of a string of ASCII digits, when a `u64` holds it.
    fn number(digits: &str) -> Option<u64> {
        digits.bytes().try_fold(0u64, |value, byte| {
            value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))
        })
    }
    let is_digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    if !is_digits(whole) || fraction.is_some_and(|fraction| !is_digits(fraction)) {
        return Err(format!(
            "`{text}` is not a number of seconds; write digits with an optional fraction, like 30 or 0.25"
        ));
    }
    let fraction = fraction.unwrap_or_default();
    if fraction.len() > FRACTION_DIGITS {
        return Err(format!(
            "`{text}` has more than 9 digits after the point, which is finer than a nanosecond; give at most 9"
        ));
    }
    // The fraction padded with zeros on its right to nine digits is a number of nanoseconds.
    let nanos = number(&format!("{fraction:0<FRACTION_DIGITS$}"));
    number(whole)
        .and_then(|seconds| seconds.checked_mul(NANOS_PER_SECOND))
        .zip(nanos)
        .and_then(|(seconds, nanos)| seconds.checked_add(nanos))
        .ok_or_else(|| {
            format!(
                "`{text}` seconds is more than device time can count (584 years); give a smaller number"
            )
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunEnd {
    /// The cycle limit (`--max-cycles`) was reached.
    Budget,
    /// The device-time limit (`--max-time`) was reached.
    TimeLimit,
    /// Whoever was reading the console went away (`inkemu run | head`): a normal way to end.
    ConsoleClosed,
    /// The console could not be written for any other reason.
    ConsoleError,
    /// `--strict-mmio` stopped the run on something the emulator does not do as the device would.
    StrictMmio,
    /// The CPU waits for an interrupt and nothing on the device can raise one.
    Idle,
    /// The device went into deep sleep and nothing the model has can wake it (F5): what a
    /// firmware does when it turns the device off, so the run ended normally.
    DeepSleep,
    /// The user asked the run to stop (Ctrl-C). The request is read between two slices, at a
    /// place that depends on the host: the trace hash of such a run covers what ran up to the
    /// cut, and is not reproducible from one run to the next. The hash of a run ended by a limit
    /// is.
    Interrupted,
    /// The board's supply failed: the device powered off (ADR 0013), what a firmware does when it
    /// turns a battery device off, so the run ended normally.
    PowerOff,
}

impl RunEnd {
    /// Every way a run can end, for what must hold for each of them: a variant added to the enum
    /// is added here.
    pub const ALL: [RunEnd; 9] = [
        RunEnd::Budget,
        RunEnd::TimeLimit,
        RunEnd::ConsoleClosed,
        RunEnd::ConsoleError,
        RunEnd::StrictMmio,
        RunEnd::Idle,
        RunEnd::Interrupted,
        RunEnd::DeepSleep,
        RunEnd::PowerOff,
    ];

    /// The process exit code of a run that ended this way.
    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            RunEnd::Budget
            | RunEnd::TimeLimit
            | RunEnd::ConsoleClosed
            | RunEnd::Interrupted
            | RunEnd::DeepSleep
            | RunEnd::PowerOff => EXIT_OK,
            RunEnd::ConsoleError => EXIT_ERROR,
            RunEnd::StrictMmio => EXIT_STRICT_MMIO,
            RunEnd::Idle => EXIT_IDLE,
        }
    }

    /// The process exit code of a run that ended this way and whose flash file could then not be
    /// written: what the firmware wrote to the flash is lost, which is an error of the
    /// environment (2) whatever ended the run, a wait nothing can end included. One end keeps
    /// its own code, for the reason [`execute`] gives for the console: `--strict-mmio` exists
    /// for CI, and its exit code must not depend on the state of a directory. The message of
    /// the failed write is printed in every case.
    #[must_use]
    pub const fn exit_code_without_its_flash(self) -> u8 {
        match self {
            RunEnd::StrictMmio => EXIT_STRICT_MMIO,
            RunEnd::Budget
            | RunEnd::TimeLimit
            | RunEnd::ConsoleClosed
            | RunEnd::ConsoleError
            | RunEnd::Idle
            | RunEnd::Interrupted
            | RunEnd::DeepSleep
            | RunEnd::PowerOff => EXIT_ERROR,
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

/// Says in `log` that the console could not be written. A failing log writer must not hide the
/// run's outcome: there is nowhere left to report it.
fn log_console_error(log: &mut dyn Write, outcome: &io::Result<bool>) {
    if let Err(e) = outcome {
        let _ = writeln!(
            log,
            "error: cannot write the console output: {e}; check where stdout is redirected"
        );
    }
}

/// Takes the bytes of every console and relays those of `selected`; the others are discarded.
///
/// Every channel the machine has is taken on every pass ([`ConsoleChannel::ALL`], so that a
/// channel added to the model is drained here without an edit): the machine keeps what a
/// console sent until a caller takes it, and the ones nobody reads would otherwise grow for as
/// long as the firmware writes to them.
///
/// `Ok(false)` when the reader of the console is gone. A write finds that out by itself; when the
/// selected console has nothing to write, `reader_gone` is asked instead, once, if `watch` says
/// this pass is one that asks: a firmware that prints nothing would otherwise run on for a
/// reader that left.
///
/// The bytes of every channel are given back, for a console script to read.
fn relay_selected_console(
    machine: &mut Esp32C3,
    selected: ConsoleChannel,
    console: &mut dyn Write,
    watch: &mut ReaderWatch,
    reader_gone: &dyn Fn() -> bool,
) -> (io::Result<bool>, Vec<(ConsoleChannel, Vec<u8>)>) {
    let executed = machine.cycles().get();
    let mut outcome = Ok(true);
    let mut taken = Vec::with_capacity(ConsoleChannel::ALL.len());
    for channel in ConsoleChannel::ALL {
        let bytes = machine.take_console(channel);
        if channel == selected {
            outcome = if bytes.is_empty() {
                Ok(!(watch.asks(executed) && reader_gone()))
            } else {
                relay(console, &bytes)
            };
        }
        taken.push((channel, bytes));
    }
    (outcome, taken)
}

/// The exit code of a run that ended with `code` and ran a console script whose status is
/// `script`: a run that would exit 0 exits [`EXIT_SCRIPT_INCOMPLETE`] when the script is not
/// through. Any other code is kept: it says more about the run (a `--strict-mmio` stop, a CPU
/// that waits for ever, a console or a flash file that could not be written).
#[must_use]
pub fn exit_code_with_script(code: u8, script: Option<&ScriptStatus>) -> u8 {
    match script {
        Some(status) if code == EXIT_OK && *status != ScriptStatus::Complete => {
            EXIT_SCRIPT_INCOMPLETE
        }
        _ => code,
    }
}

/// The exit code of a run that ended as `end` with its console script at `script`, `saved`
/// telling whether what the run writes once it is over (its flash file, its session) was
/// written: when something was not, the code says so ([`RunEnd::exit_code_without_its_flash`])
/// whatever the script did.
#[must_use]
pub fn exit_code_of_run(end: RunEnd, script: Option<&ScriptStatus>, saved: bool) -> u8 {
    if saved {
        exit_code_with_script(end.exit_code(), script)
    } else {
        end.exit_code_without_its_flash()
    }
}

/// Says in `log`, when the journal of the inputs of `machine` has passed one of its bounds, that
/// the run will not be recorded: the user of a recorded run can stop it at once instead of
/// learning it at its end. Gives whether it said so. A machine told to keep no journal (a
/// replay) has nothing to say.
pub fn say_an_incomplete_journal(machine: &Esp32C3, log: &mut dyn Write) -> bool {
    let journal = machine.input_journal();
    if journal.is_complete() || journal.is_forgone() {
        return false;
    }
    let _ = writeln!(
        log,
        "warning: the run received more input than a session holds ({} bytes or {} chunks on a console, or {} changes of the USB host): it will not be recorded; stop it now, or record a run that sends less",
        soc_esp32c3::MAX_JOURNAL_CONSOLE_BYTES,
        soc_esp32c3::MAX_JOURNAL_CONSOLE_CHUNKS,
        soc_esp32c3::MAX_JOURNAL_HOST_CHANGES
    );
    true
}

/// The line of the summary that says how far the console script got.
fn say_script(log: &mut dyn Write, status: &ScriptStatus, steps: usize) {
    let _ = match status {
        ScriptStatus::Complete => {
            writeln!(log, "inkemu: console script through ({steps} steps)")
        }
        ScriptStatus::WaitingFor { step } => writeln!(
            log,
            "inkemu: console script not through: line {} ({step}) was still waiting; raise --max-time or --max-cycles, or check that the firmware prints that text on that console",
            step.line
        ),
        ScriptStatus::Ahead { step, at } => writeln!(
            log,
            "inkemu: console script not through: line {} ({step}) is dated at {} us of device time, after the end of the run; raise --max-time or --max-cycles",
            step.line,
            soc_esp32c3::clock::TIME_BASE.cycles_to_nanos(*at) / 1_000
        ),
    };
}

/// Says in `log` why a run ended as [`RunEnd::ConsoleClosed`].
fn log_reader_gone(log: &mut dyn Write) {
    let _ = writeln!(log, "inkemu: the reader of standard output went away");
}

/// What the line of `diagnostic` starts with. A reset is an event of the run, as the restart a
/// firmware asks for is: it is said in inkemu's own voice. Everything else is something the
/// emulation did not do as the device would, or a fault of the firmware: a warning.
#[must_use]
pub const fn journal_prefix(diagnostic: &Diagnostic) -> &'static str {
    match diagnostic {
        Diagnostic::Reset { .. } | Diagnostic::UnlistedResets => "inkemu",
        Diagnostic::UnmodeledMmio { .. }
        | Diagnostic::SubWordWrite { .. }
        | Diagnostic::Unsupported { .. }
        | Diagnostic::FlashMapping { .. }
        | Diagnostic::StaleFlashWindow { .. }
        | Diagnostic::Exception { .. }
        | Diagnostic::ClockedChipStuck { .. }
        | Diagnostic::Suppressed => "warning",
    }
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
    let unlisted_resets = machine.unlisted_resets();
    if unlisted_resets > 0 {
        let _ = writeln!(
            log,
            "inkemu: {unlisted_resets} further resets were not listed (more than {MAX_LISTED_RESETS})"
        );
    }
}

/// Runs `machine` until it stops, sending the firmware console to `console` and everything else
/// (warnings, then the summary, whose first line says why the run ended — or, when the console
/// failed, that it did) to `log`.
///
/// Output is relayed every `SLICE_CYCLES` cycles so a long run shows progress. Both limits of
/// `options` are given to every call of the machine: the cycle limit ends a run whose CPU
/// executes, the device-time limit also ends one whose CPU sleeps, and the first one reached
/// ends the run (the cycle limit when both are reached at the same instruction boundary).
///
/// The run takes two things from the host, and neither enters a trace: both are looked at between
/// two calls of the machine, and all they decide is where the run is cut.
/// - The request to stop. `interrupt` is read at the start of every pass of the loop, whatever
///   the previous pass ended on (a sleeping CPU included, which can go on for ever without the
///   cycle count advancing): once it is set (Ctrl-C), the run ends with the same summary as a run
///   that reached a limit. `interrupt` is told when the summary begins, before its first line,
///   whatever ends the run: from then on a request no longer ends the process ([`Interrupt`]).
/// - The state of its reader. A write to a console whose reader went away says so by failing;
///   the passes that had nothing to write ask `reader_gone` instead, the first of them and then
///   one in `SILENT_PASSES_PER_QUESTION` or one per slice of executed cycles (`ReaderWatch`),
///   and a yes ends the run as [`RunEnd::ConsoleClosed`]. A caller that cannot tell passes a
///   function that always says no: the limits then bound a run that prints nothing.
///
/// A run that is not interrupted and whose reader stays is therefore the same run on every host:
/// same console, same summary, same trace hash.
pub fn execute(
    machine: &mut Esp32C3,
    options: &RunOptions,
    interrupt: &Interrupt,
    reader_gone: &dyn Fn() -> bool,
    console: &mut dyn Write,
    log: &mut dyn Write,
) -> RunEnd {
    execute_with_script(
        machine,
        options,
        RunExtras::default(),
        interrupt,
        reader_gone,
        console,
        log,
    )
}

/// What a run of [`execute_with_script`] does beside showing its console: a console script to
/// hand to the machine, and the digests a session takes of every console (G3).
#[derive(Debug, Default)]
pub struct RunExtras<'a> {
    pub script: Option<&'a mut ScriptRun>,
    /// Every byte of every console goes into these, those the end of the run flushes included:
    /// the console a session compares.
    pub digests: Option<&'a mut ConsoleDigests>,
}

/// How a run that ended as `end` ends a session: a stop of the machine's own, or a cut of the
/// host (Ctrl-C, a reader that went away, a console that could not be written), which a session
/// records with its instant (G5).
///
/// # Errors
/// The [`CutReason`] of an end the host made.
pub const fn session_end(end: RunEnd) -> Result<Reached, CutReason> {
    match end {
        RunEnd::Budget => Ok(Reached::CycleLimit),
        RunEnd::TimeLimit => Ok(Reached::TimeLimit),
        RunEnd::Idle => Ok(Reached::Idle),
        RunEnd::DeepSleep => Ok(Reached::DeepSleep),
        RunEnd::PowerOff => Ok(Reached::PowerOff),
        RunEnd::StrictMmio => Ok(Reached::StrictMmio),
        RunEnd::Interrupted => Err(CutReason::Interrupted),
        RunEnd::ConsoleClosed => Err(CutReason::ReaderGone),
        RunEnd::ConsoleError => Err(CutReason::ConsoleError),
    }
}

/// [`execute`], with `extras`: a console script that started on `machine`
/// ([`ScriptRun::start`]), whose steps are handed to the machine at their dates as it reads the
/// bytes of every console after each call, the summary saying how far it got; and the digests of
/// a session, which take every byte of every console. A step the machine refuses is said in
/// `log` and ends the script there, its channels no longer watched ([`ScriptRun::stop`]); from a
/// file, only a `send` on a UART dated after about 114 years of device time is
/// (`InputError::BeyondTheLine`).
pub fn execute_with_script(
    machine: &mut Esp32C3,
    options: &RunOptions,
    extras: RunExtras<'_>,
    interrupt: &Interrupt,
    reader_gone: &dyn Fn() -> bool,
    console: &mut dyn Write,
    log: &mut dyn Write,
) -> RunEnd {
    let RunExtras {
        mut script,
        mut digests,
    } = extras;
    // Whether the script still reads the consoles: not after a step the machine refused.
    let mut observing = true;
    // Whether the console has taken everything it was given. A console that failed, or whose
    // reader went away, is not written to again: a strict stop goes on to its own end after it,
    // and must not report the same failure a second time.
    let mut console_usable = true;
    let mut reader = ReaderWatch::default();
    // Whether the log said that the run received more input than its session holds.
    let mut journal_said_incomplete = false;
    let mut end = loop {
        if interrupt.is_requested() {
            interrupt.summary_begins();
            say_why(log, RunEnd::Interrupted, None);
            break RunEnd::Interrupted;
        }
        let now = machine.cycles().get();
        let slice_end = now.saturating_add(SLICE_CYCLES);
        let limit = options
            .max_cycles
            .map_or(slice_end, |max| max.min(slice_end));
        let stop = machine.run_within(Cycles(limit), options.max_device_nanos);
        let (console_open, taken) =
            relay_selected_console(machine, options.console, console, &mut reader, reader_gone);
        if let Some(digests) = digests.as_deref_mut() {
            for (channel, bytes) in &taken {
                digests.add(*channel, bytes);
            }
        }
        let cycle_limit_reached = options
            .max_cycles
            .is_some_and(|max| machine.cycles().get() >= max);
        let pass = pass_end(&console_open, &stop, cycle_limit_reached);
        // A pass that ends the run hands the script's steps nothing more: what a `wait-for`
        // found in it would date inputs at the stop or after it, which the run never takes, and
        // the journal of a session holds only the inputs the run could still use.
        if pass.is_none()
            && observing
            && let Some(run) = script.as_deref_mut()
            && let Err(e) = run.observe(machine, &taken)
        {
            let _ = writeln!(log, "error: {e}");
            observing = false;
            // Nothing reads the consoles for the script any more: they go back to their pace.
            run.stop(machine);
        }
        if digests.is_some() && !journal_said_incomplete {
            journal_said_incomplete = say_an_incomplete_journal(machine, log);
        }
        console_usable = matches!(console_open, Ok(true));
        // The stop line carries the violation: listing it as a warning too would print it twice.
        let violation = match &stop {
            StopReason::StrictMmio(diagnostic) => Some(diagnostic),
            _ => None,
        };
        for diagnostic in machine.drain_diagnostics() {
            if Some(&diagnostic) != violation {
                // A failing log writer must not hide the run's outcome: there is nowhere left to report it.
                let _ = writeln!(log, "{}: {diagnostic}", journal_prefix(&diagnostic));
            }
        }
        let Some(end) = pass else {
            continue;
        };
        // The summary begins here: the warnings of the last slice are above, and from the next
        // line on a request to stop no longer ends the process.
        interrupt.summary_begins();
        // A failure to write the console is logged whatever else ended the run in the same slice.
        log_console_error(log, &console_open);
        say_why(log, end, Some(&stop));
        break end;
    };
    if console_usable {
        end = flush_shown_console(
            machine,
            options.console,
            end,
            digests.as_deref_mut(),
            console,
            log,
        );
    }
    if let Some(digests) = digests {
        let flushed = console_usable.then_some(options.console);
        flush_the_other_consoles(machine, flushed, digests);
    }
    say_where_the_run_stopped(machine, script.as_deref(), log);
    end
}

/// The lines of the summary after why the run ended: where the machine stopped, its device
/// time and clock, its deep sleep, the USB host, how far `script` got, the diagnostics and the
/// trace hash.
pub fn say_where_the_run_stopped(
    machine: &mut Esp32C3,
    script: Option<&ScriptRun>,
    log: &mut dyn Write,
) {
    let _ = writeln!(
        log,
        "inkemu: stopped at pc {:#010x} after {} cycles ({} instructions)",
        machine.pc(),
        machine.cycles().get(),
        machine.instructions()
    );
    // After a limit, Ctrl-C or a reader that went away, the `pc` above is the instruction after
    // a `wfi`, and nothing else says that the firmware was idle.
    let asleep = if machine.is_asleep() {
        "; the CPU is asleep, waiting for an interrupt"
    } else {
        ""
    };
    let _ = writeln!(
        log,
        "inkemu: {} us of device time; CPU clock at {}{asleep}",
        machine.device_time_nanos() / 1_000,
        frequency_text(machine.cpu_frequency().hz())
    );
    if let Some(sleep) = machine.deep_sleep() {
        let _ = writeln!(log, "{}", deep_sleep_line(&sleep));
    }
    let _ = writeln!(log, "inkemu: USB host {}", machine.usb_host().name());
    if let Some(run) = script {
        say_script(log, &run.status(machine.device_time()), run.step_count());
    }
    summarize_diagnostics(machine, log);
    if let Some(hash) = machine.trace_hash() {
        let _ = writeln!(log, "trace-hash {hash:016x}");
    }
}

/// Relays what the firmware had written on `shown` and the device had not sent yet: without it
/// the console would end before the place the run stopped at. The bytes go into the digests of
/// a session too. Gives the end of the run, which a console that fails here changes.
fn flush_shown_console(
    machine: &mut Esp32C3,
    shown: ConsoleChannel,
    end: RunEnd,
    digests: Option<&mut ConsoleDigests>,
    console: &mut dyn Write,
    log: &mut dyn Write,
) -> RunEnd {
    let bytes = machine.flush_console(shown);
    if let Some(digests) = digests {
        digests.add(shown, &bytes);
    }
    let flushed = relay(console, &bytes);
    log_console_error(log, &flushed);
    match (end, flushed) {
        // As in the loop: a `--strict-mmio` stop keeps its exit code whatever the console did.
        (RunEnd::StrictMmio, _) => RunEnd::StrictMmio,
        (_, Err(_)) => RunEnd::ConsoleError,
        (_, Ok(false)) => {
            log_reader_gone(log);
            RunEnd::ConsoleClosed
        }
        (other, Ok(true)) => other,
    }
}

/// A session compares every console to its last byte: the ones not shown, and the one shown when
/// it could not be written, are flushed into its digests only. `flushed` is the console the run
/// flushed already.
fn flush_the_other_consoles(
    machine: &mut Esp32C3,
    flushed: Option<ConsoleChannel>,
    digests: &mut ConsoleDigests,
) {
    for channel in ConsoleChannel::ALL {
        if flushed != Some(channel) {
            digests.add(channel, &machine.flush_console(channel));
        }
    }
}

/// The line of the summary of a run that ends with the device in deep sleep (F5, F9): since when
/// it sleeps, and when the RTC timer wakes it or what it waits for instead.
#[must_use]
pub fn deep_sleep_line(sleep: &DeepSleep) -> String {
    let since = sleep.since_nanos / 1_000;
    let waits = match sleep.timer_wake_nanos {
        Some(wake) => format!("the RTC timer wakes it at {} us", wake / 1_000),
        None if sleep.wake_sources == 0 => {
            "it waits for nothing: the firmware enabled no wake source".to_owned()
        }
        None => {
            // The RTC timer is modelled: enabled as a source without its alarm, it wakes
            // nothing, which is not the same as a source inkemu lacks.
            let unmodelled = sleep.wake_sources & !DeepSleep::TIMER;
            let waits = if unmodelled == 0 {
                "it waits for nothing".to_owned()
            } else {
                let gpio = if unmodelled & DeepSleep::GPIO != 0 {
                    " (GPIO, bit 2, belongs to sub-project 2d)"
                } else {
                    ""
                };
                format!("it waits for a wake source inkemu does not model{gpio}")
            };
            let timer = if sleep.wake_sources & DeepSleep::TIMER != 0 {
                "; the RTC timer is a wake source but its alarm is not enabled \
                 (SLP_TIMER1.MAIN_TIMER_ALARM_EN)"
            } else {
                ""
            };
            format!("{waits}: WAKEUP_ENA {:#07x}{timer}", sleep.wake_sources)
        }
    };
    format!("inkemu: the device is in deep sleep since {since} us of device time; {waits}")
}

/// A frequency in the largest unit that states its number of hertz exactly: `80 MHz`, `625 kHz`,
/// `39062 Hz`. This function never rounds: the crystal divider can bring the CPU clock far below
/// 1 MHz, where a number of MHz would read 0. The number of hertz it is given is itself a whole
/// number: a clock whose period does not divide the time base (40 MHz / 3) arrives truncated, as
/// `13333333 Hz`.
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
    use console_script::{Script, VERSION_LINE};
    use cpu_rv32::asm::{addi, beq, bne, jal, lbu, lui, lw, sw, wfi};
    use soc_esp32c3::UsbHost;
    use soc_esp32c3::test_support::{
        HANG, T0, T1, flat_rom, load_register, print_program, stop_watchdogs, store_register,
        unmodeled_register, within,
    };
    use std::time::Duration;
    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    const UART1: u32 = svd_regs::esp32c3::addr("UART1", "FIFO");
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
        usb_host: UsbHost::Open,
    };
    static NOT_INTERRUPTED: Interrupt = Interrupt::new();
    /// Ctrl-C pressed once before the run starts.
    fn requested() -> Interrupt {
        let interrupt = Interrupt::new();
        assert!(!interrupt.request(), "a first request ends nothing");
        interrupt
    }
    /// The reader of the console is there for the whole run.
    fn reader_stays() -> bool {
        false
    }

    /// A run on the UART0 console that no limit ends.
    const UNLIMITED: RunOptions = RunOptions {
        max_cycles: None,
        max_device_nanos: None,
        console: ConsoleChannel::Uart0,
    };
    const SECOND_NS: u64 = 1_000_000_000;
    /// A cycle limit for the runs that something else must end, and whose program executes: a
    /// run that lost its end reaches this limit after ten slices, ends as [`RunEnd::Budget`],
    /// and the assertion on its end fails, where a run without a limit would hang the suite.
    const BACKSTOP: u64 = 10 * SLICE_CYCLES;

    fn options(max_cycles: u64) -> RunOptions {
        RunOptions {
            max_cycles: Some(max_cycles),
            ..UNLIMITED
        }
    }
    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// A log that notes, for each of its lines, whether the run had begun its summary when the
    /// line began.
    struct Noting<'a> {
        interrupt: &'a Interrupt,
        lines: Vec<(bool, String)>,
        in_a_line: bool,
    }
    impl io::Write for Noting<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            for &byte in bytes {
                if !self.in_a_line {
                    let begun = self.interrupt.summary_has_begun();
                    self.lines.push((begun, String::new()));
                    self.in_a_line = true;
                }
                if let Some((_, line)) = self.lines.last_mut() {
                    line.push(char::from(byte));
                }
                self.in_a_line = byte != b'\n';
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The lines `program` logs in a run ended by `options`, each with whether the summary had
    /// begun when it began.
    fn noted(program: &[u32], options: &RunOptions, interrupt: &Interrupt) -> Vec<(bool, String)> {
        let mut m = machine(program, MachineOptions::default());
        let mut log = Noting {
            interrupt,
            lines: Vec::new(),
            in_a_line: false,
        };
        let _ = execute(
            &mut m,
            options,
            interrupt,
            &reader_stays,
            &mut Vec::new(),
            &mut log,
        );
        log.lines
    }

    /// Prints `ready` on UART0, then echoes what UART0 receives.
    fn ready_then_echo() -> Vec<u32> {
        printing(
            UART0,
            b"ready",
            &[
                lbu(T1, T0, 0x1C),
                beq(T1, 0, -4),
                lw(T1, T0, 0),
                sw(T1, T0, 0),
                jal(0, -16),
            ],
        )
    }

    /// Runs `program` with the console script `body` to 20 ms of device time: the console, the
    /// log, and the exit code of the run.
    fn scripted(program: &[u32], body: &str) -> (String, String, u8) {
        let mut m = machine(program, MachineOptions::default());
        let script = Script::parse(format!("{VERSION_LINE}\n{body}").as_bytes()).unwrap();
        let mut run = ScriptRun::start(&script, &mut m).unwrap();
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let options = RunOptions {
            max_device_nanos: Some(20_000_000),
            ..UNLIMITED
        };
        let end = execute_with_script(
            &mut m,
            &options,
            RunExtras {
                script: Some(&mut run),
                digests: None,
            },
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        let status = run.status(m.device_time());
        let code = exit_code_with_script(end.exit_code(), Some(&status));
        (text(&console), text(&log), code)
    }

    #[test]
    fn a_console_script_reads_the_consoles_and_its_summary_line_says_it_went_through() {
        let (console, log, code) = scripted(
            &ready_then_echo(),
            "wait-for uart0 \"ready\"\nsend uart0 \"ok\\r\"\n",
        );
        assert_eq!(console, "readyok\r");
        assert!(
            log.contains("inkemu: USB host open\ninkemu: console script through (2 steps)\n"),
            "{log}"
        );
        assert_eq!(code, EXIT_OK);
    }

    #[test]
    fn a_console_script_not_through_turns_an_exit_0_into_5_and_says_where_it_stopped() {
        let (_, log, code) = scripted(&ready_then_echo(), "wait-for uart0 \"never\"\n");
        assert!(
            log.contains(
                "inkemu: console script not through: line 2 (wait-for uart0 \"never\") was still waiting; raise --max-time or --max-cycles, or check that the firmware prints that text on that console\n"
            ),
            "{log}"
        );
        assert_eq!(code, EXIT_SCRIPT_INCOMPLETE);
        let (_, log, code) = scripted(&ready_then_echo(), "wait 1s\nsend uart0 \"late\"\n");
        assert!(
            log.contains(
                "inkemu: console script not through: line 3 (send uart0 \"late\") is dated at 1000000 us of device time, after the end of the run; raise --max-time or --max-cycles\n"
            ),
            "{log}"
        );
        assert_eq!(code, EXIT_SCRIPT_INCOMPLETE);
    }

    #[test]
    fn a_step_the_machine_refuses_ends_the_script_and_its_channels_go_back_to_their_pace() {
        // After `ready`, a send dated past the last instant UART0 times is refused: the log says
        // so, and UART0 is watched no more, so its next byte ends no call.
        let mut m = machine(&ready_then_echo(), MachineOptions::default());
        let script = Script::parse(
            format!(
                "{VERSION_LINE}\nwait-for uart0 \"ready\"\nwait 18446744073s\nsend uart0 \"x\"\n"
            )
            .as_bytes(),
        )
        .unwrap();
        let mut run = ScriptRun::start(&script, &mut m).unwrap();
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let options = RunOptions {
            max_device_nanos: Some(20_000_000),
            ..UNLIMITED
        };
        execute_with_script(
            &mut m,
            &options,
            RunExtras {
                script: Some(&mut run),
                digests: None,
            },
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert!(
            text(&log).contains("error: console script, line 4: uart0 cannot take an input dated"),
            "{}",
            text(&log)
        );
        assert_eq!(
            text(&log).matches("error: ").count(),
            1,
            "the refusal is said once: {}",
            text(&log)
        );
        m.send_console(ConsoleChannel::Uart0, m.device_time(), b"z")
            .unwrap();
        assert_ne!(
            m.run_until(m.cycles() + 1_000_000),
            StopReason::ConsoleOutput
        );
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"z");
    }

    /// A console that takes `left` bytes, then cannot be written.
    struct FailsAfter {
        left: usize,
    }
    impl io::Write for FailsAfter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.left == 0 {
                return Err(io::Error::other("disk full"));
            }
            let taken = bytes.len().min(self.left);
            self.left -= taken;
            Ok(taken)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_pass_that_ends_the_run_hands_the_script_nothing_more() {
        // The console fails on the `y` of `ready`, the pass in which the `wait-for` finds its
        // text: the run is over, and the send that follows is not handed to the machine.
        let mut m = machine(&ready_then_echo(), MachineOptions::default());
        let script = Script::parse(
            format!("{VERSION_LINE}\nwait-for uart0 \"ready\"\nsend uart0 \"x\"\n").as_bytes(),
        )
        .unwrap();
        let mut run = ScriptRun::start(&script, &mut m).unwrap();
        let mut log = Vec::new();
        let end = execute_with_script(
            &mut m,
            &options(BACKSTOP),
            RunExtras {
                script: Some(&mut run),
                digests: None,
            },
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut FailsAfter { left: 4 },
            &mut log,
        );
        assert_eq!(end, RunEnd::ConsoleError, "{}", text(&log));
        assert_eq!(
            m.input_journal().inputs(),
            &[],
            "no input handed after the end"
        );
        assert!(
            matches!(run.status(m.device_time()), ScriptStatus::WaitingFor { .. }),
            "{}",
            text(&log)
        );
    }

    /// Three bytes on UART1, four on UART0 (the console shown), four on USB-Serial-JTAG,
    /// committed, then the program parks: after 3 000 cycles no byte has left a UART.
    fn printing_on_every_console() -> Vec<u32> {
        let mut program = printing(UART1, b"one", &[]);
        program.extend(printing(UART0, b"zero", &[]));
        program.extend(printing(
            USB,
            b"jtag",
            &[addi(T1, 0, 1), sw(T1, T0, 4), HANG],
        ));
        program
    }

    #[test]
    fn the_digests_of_a_session_take_every_console_to_its_last_byte() {
        let mut expected = ConsoleDigests::default();
        for (channel, bytes) in [
            (ConsoleChannel::Uart0, &b"zero"[..]),
            (ConsoleChannel::Uart1, b"one"),
            (ConsoleChannel::UsbSerialJtag, b"jtag"),
        ] {
            expected.add(channel, bytes);
        }
        let expected = expected.finish();
        let gone = || true;
        let cases: [(u64, &dyn Fn() -> bool, bool, RunEnd); 4] = [
            // The limit ends the run: the end flushes the console shown, then the others.
            (3_000, &reader_stays, false, RunEnd::Budget),
            // The reader went away in the loop: every console is flushed into the digests only.
            (3_000, &gone, false, RunEnd::ConsoleClosed),
            // The console fails at the flush of the end: its bytes are in the digests all the
            // same, and the others are flushed after it.
            (3_000, &reader_stays, true, RunEnd::ConsoleError),
            // The console fails in the loop, every byte having left the UARTs.
            (BACKSTOP, &reader_stays, true, RunEnd::ConsoleError),
        ];
        for (max_cycles, reader_gone, failing, expected_end) in cases {
            let mut m = machine(&printing_on_every_console(), MachineOptions::default());
            let mut digests = ConsoleDigests::default();
            let (mut shown, mut log) = (Vec::new(), Vec::new());
            let console: &mut dyn io::Write = if failing { &mut Failing } else { &mut shown };
            let end = execute_with_script(
                &mut m,
                &options(max_cycles),
                RunExtras {
                    script: None,
                    digests: Some(&mut digests),
                },
                &NOT_INTERRUPTED,
                reader_gone,
                console,
                &mut log,
            );
            assert_eq!(end, expected_end, "{}", text(&log));
            assert_eq!(digests.finish(), expected, "{expected_end:?}");
        }
    }

    #[test]
    fn a_recorded_run_says_once_and_at_once_that_it_received_more_than_a_session_holds() {
        let run = |digests: Option<&mut ConsoleDigests>, forgo: bool| {
            let mut m = machine(&[HANG], MachineOptions::default());
            if forgo {
                m.forgo_input_journal();
            }
            for _ in 0..=soc_esp32c3::MAX_JOURNAL_CONSOLE_CHUNKS {
                m.send_console(ConsoleChannel::Uart0, m.device_time(), b"")
                    .unwrap();
            }
            let mut log = Vec::new();
            execute_with_script(
                &mut m,
                &options(3 * SLICE_CYCLES),
                RunExtras {
                    script: None,
                    digests,
                },
                &NOT_INTERRUPTED,
                &reader_stays,
                &mut Vec::new(),
                &mut log,
            );
            text(&log)
        };
        let log = run(Some(&mut ConsoleDigests::default()), false);
        let warning = "warning: the run received more input than a session holds (1048576 bytes or 16384 chunks on a console, or 16384 changes of the USB host): it will not be recorded; stop it now, or record a run that sends less\n";
        assert!(log.starts_with(warning), "{log}");
        assert_eq!(log.matches(warning).count(), 1, "{log}");
        // A run that is not recorded, and a replay, whose machine keeps no journal, say nothing.
        assert!(!run(None, false).contains("warning:"));
        assert!(!run(Some(&mut ConsoleDigests::default()), true).contains("warning:"));
    }

    #[test]
    fn a_script_not_through_changes_no_exit_code_but_0() {
        let waiting = ScriptStatus::Ahead {
            step: console_script::Step {
                line: 2,
                action: console_script::Action::Wait { nanos: 1 },
            },
            at: Cycles(1),
        };
        for end in RunEnd::ALL {
            let code = end.exit_code();
            let expected = if code == EXIT_OK {
                EXIT_SCRIPT_INCOMPLETE
            } else {
                code
            };
            assert_eq!(
                exit_code_with_script(code, Some(&waiting)),
                expected,
                "{end:?}"
            );
            assert_eq!(
                exit_code_with_script(code, Some(&ScriptStatus::Complete)),
                code
            );
            assert_eq!(exit_code_with_script(code, None), code);
        }
    }

    #[test]
    fn a_run_whose_flash_file_or_session_was_not_written_exits_as_one_without_its_flash() {
        let waiting = ScriptStatus::WaitingFor {
            step: console_script::Step {
                line: 2,
                action: console_script::Action::Wait { nanos: 1 },
            },
        };
        for end in RunEnd::ALL {
            for script in [None, Some(&waiting), Some(&ScriptStatus::Complete)] {
                assert_eq!(
                    exit_code_of_run(end, script, true),
                    exit_code_with_script(end.exit_code(), script),
                    "{end:?}"
                );
                assert_eq!(
                    exit_code_of_run(end, script, false),
                    end.exit_code_without_its_flash(),
                    "{end:?}: a script not through says nothing more than a lost write"
                );
            }
        }
    }

    #[test]
    fn the_summary_says_the_usb_host_at_the_end_of_the_run() {
        let mut m = machine(
            &[HANG],
            MachineOptions {
                usb_host: UsbHost::Attached,
                ..MachineOptions::default()
            },
        );
        let mut log = Vec::new();
        execute(
            &mut m,
            &options(1_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut Vec::new(),
            &mut log,
        );
        assert!(
            text(&log).contains("inkemu: USB host attached\n"),
            "{}",
            text(&log)
        );
    }

    #[test]
    fn a_second_request_ends_the_process_only_before_the_summary_begins() {
        let interrupt = Interrupt::new();
        assert!(!interrupt.request(), "the first is left to the run");
        assert!(interrupt.is_requested(), "which reads it at its next pass");
        assert!(interrupt.request(), "a second one ends the process");
        assert!(interrupt.request(), "and so does every one after it");
        interrupt.summary_begins();
        for _ in 0..3 {
            assert!(!interrupt.request(), "the summary has begun: nothing ends");
        }
        // A run that ended for another cause (a limit) has begun its summary: a first request
        // after it is read by nobody, and ends nothing either.
        let interrupt = Interrupt::new();
        interrupt.summary_begins();
        assert!(!interrupt.request());
        assert!(!interrupt.request());
    }

    #[test]
    fn the_summary_begins_before_its_first_line_and_after_the_warnings_of_the_run() {
        // A warning in the last slice, then the cycle limit: the warning is no part of the
        // summary, the line that says why the run ended is its first.
        let interrupt = Interrupt::new();
        let lines = noted(&reading_unmodeled_register(), &options(10_000), &interrupt);
        let first = lines
            .iter()
            .position(|(_, line)| line.starts_with("inkemu: cycle limit reached"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(first > 0, "a warning comes before it: {lines:?}");
        assert!(
            lines[..first].iter().all(|(begun, _)| !begun),
            "the warnings are written before the summary begins: {lines:?}"
        );
        assert!(
            lines[first..].iter().all(|(begun, _)| *begun),
            "every line of the summary is written once it has begun: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|(_, line)| line.starts_with("inkemu: stopped at pc "))
        );
        // Ctrl-C before the run: the summary is the whole log, from its first line.
        let interrupt = requested();
        let lines = noted(&[HANG], &options(10_000), &interrupt);
        assert_eq!(
            lines.first().map(|(_, line)| line.as_str()),
            Some("inkemu: interrupted by the user\n")
        );
        assert!(lines.iter().all(|(begun, _)| *begun), "{lines:?}");
    }

    #[test]
    fn the_console_error_line_is_the_first_of_the_summary() {
        // A console that fails ends the run, and the line that says so comes before the reason:
        // the summary has begun when it is written.
        let interrupt = Interrupt::new();
        let mut m = machine(
            &printing(UART0, b"hello\n", &[HANG]),
            MachineOptions::default(),
        );
        let mut log = Noting {
            interrupt: &interrupt,
            lines: Vec::new(),
            in_a_line: false,
        };
        let end = execute(
            &mut m,
            &options(BACKSTOP),
            &interrupt,
            &reader_stays,
            &mut Failing,
            &mut log,
        );
        assert_eq!(end, RunEnd::ConsoleError);
        let first = log
            .lines
            .iter()
            .position(|(_, line)| line.contains("disk full"))
            .unwrap_or_else(|| panic!("{:?}", log.lines));
        assert!(
            log.lines[first..].iter().all(|(begun, _)| *begun),
            "{:?}",
            log.lines
        );
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
            execute(
                &mut m,
                &options(60_000),
                &NOT_INTERRUPTED,
                &reader_stays,
                &mut console,
                &mut log
            ),
            RunEnd::Budget
        );
        assert_eq!(console, b"hello\n");
        assert!(!text(&log).contains("hello"));
    }

    #[test]
    fn max_cycles_ends_the_run_cleanly() {
        let mut m = machine(&[HANG], MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut m,
            &options(2_500_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
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
                console: ConsoleChannel::UsbSerialJtag,
                ..options(10_000)
            },
            &NOT_INTERRUPTED,
            &reader_stays,
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
            execute(
                &mut m,
                &options(10_000),
                &NOT_INTERRUPTED,
                &reader_stays,
                &mut console,
                &mut log
            ),
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
        execute(
            &mut m,
            &options(10_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
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
        let end = execute(
            &mut m,
            &options(10_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
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
        let end = execute(
            &mut m,
            &options(10_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut Closed,
            &mut log,
        );
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
        execute(
            &mut m,
            &options(10_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(console, b"uart");
        assert_eq!(
            m.take_console(ConsoleChannel::UsbSerialJtag),
            Vec::<u8>::new(),
            "the USB-Serial-JTAG bytes were discarded, not kept"
        );
    }

    /// Three bytes on UART1, then four on UART0, then the program parks. At the reset baud rate
    /// and clock a byte takes 6 940 cycles to leave either UART: 60 000 cycles see them all out.
    fn printing_on_both_uarts() -> Vec<u32> {
        let mut program = printing(UART1, b"one", &[]);
        program.extend(printing(UART0, b"zero", &[HANG]));
        program
    }

    #[test]
    fn a_console_no_one_reads_holds_nothing_at_the_end_of_a_run() {
        // Every channel is taken on every pass of the loop, the selected one or not: a firmware
        // that logs without end on a console nobody asked for must not fill the host's memory.
        let mut m = machine(&printing_on_both_uarts(), MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        execute(
            &mut m,
            &options(60_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(console, b"zero");
        for channel in ConsoleChannel::ALL {
            assert_eq!(
                m.flush_console(channel),
                Vec::<u8>::new(),
                "{channel:?} still holds bytes the run should have taken"
            );
        }
    }

    #[test]
    fn the_second_uart_is_relayed_as_the_run_goes_when_it_is_the_console() {
        // The console raises the interrupt flag when it is first written to: the run ends after
        // its first slice only if the bytes of UART1 are relayed by the loop, and not kept for
        // the flush at the end of the run.
        let mut m = machine(&printing_on_both_uarts(), MachineOptions::default());
        let interrupt = Interrupt::new();
        let mut console = RaisesInterrupt {
            interrupt: &interrupt,
            received: Vec::new(),
        };
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &RunOptions {
                console: ConsoleChannel::Uart1,
                ..options(3 * SLICE_CYCLES)
            },
            &interrupt,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(console.received, b"one");
        assert_eq!(end, RunEnd::Interrupted);
        assert!(m.cycles().get() < 2 * SLICE_CYCLES, "{:?}", m.cycles());
    }

    #[test]
    fn the_end_of_a_run_shows_what_the_second_uart_had_not_sent_yet() {
        // Stopped after 500 cycles, when no byte has left UART1: the flush at the end of the run
        // covers the selected channel, whichever it is.
        let mut m = machine(&printing(UART1, b"one", &[HANG]), MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        execute(
            &mut m,
            &RunOptions {
                console: ConsoleChannel::Uart1,
                ..options(500)
            },
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(console, b"one");
    }

    #[test]
    fn idle_cpu_ends_the_run_and_says_why() {
        // With its watchdogs stopped, nothing on the device has anything scheduled.
        let mut program = stop_watchdogs();
        program.extend([wfi(), HANG]);
        // A CPU that can never wake advances neither cycles nor device time: no limit of the
        // run could end it if it stopped ending by itself, only the bound on host time.
        let run = run_bounded(program, UNLIMITED);
        assert_eq!((run.end, run.end.exit_code()), (RunEnd::Idle, 4));
        assert!(
            run.log.contains("waits for an interrupt and none can come"),
            "{}",
            run.log
        );
    }

    /// A program that stops its watchdogs and goes into deep sleep (F1) with `wake_state` in
    /// `RTC_CNTL.WAKEUP_STATE`, after setting the alarm of the RTC timer at `target` counts.
    fn deep_sleeping(wake_state: u32, target: u32) -> Vec<u32> {
        let rtc = |name| svd_regs::esp32c3::addr("RTC_CNTL", name);
        let mut program = stop_watchdogs();
        for (register, value) in [
            ("SLP_TIMER0", target),
            ("SLP_TIMER1", 1 << 16),
            ("WAKEUP_STATE", wake_state),
            ("DIG_PWC", 1 << 31),
            ("STATE0", 1 << 31),
        ] {
            program.extend(store_register(rtc(register), value));
        }
        program.push(HANG);
        program
    }

    #[test]
    fn a_device_in_a_deep_sleep_nothing_can_end_ends_the_run_with_code_0_and_says_so() {
        // F5: no wake source at all, the last sleep of `deep_sleep_timer`.
        let run = run_bounded(deep_sleeping(0, 0), UNLIMITED);
        assert_eq!((run.end, run.end.exit_code()), (RunEnd::DeepSleep, 0));
        for line in [
            "inkemu: the device went into deep sleep and nothing inkemu models can wake it, so the run ends here\n",
            "\ninkemu: the device is in deep sleep since ",
            " us of device time; it waits for nothing: the firmware enabled no wake source\n",
        ] {
            assert!(run.log.contains(line), "`{line}` is missing:\n{}", run.log);
        }
        assert!(!run.log.contains("warning"), "{}", run.log);
    }

    #[test]
    fn a_gpio_wake_is_named_and_the_run_ends_asleep() {
        // F5: what CrossPoint arms before it sleeps, GPIO alone.
        let run = run_bounded(deep_sleeping(1 << 2 << 15, 0), UNLIMITED);
        assert_eq!(run.end, RunEnd::DeepSleep);
        for line in [
            "warning: the chip went into deep sleep with wake sources inkemu does not model (WAKEUP_ENA 0x00004, pc ",
            "it waits for a wake source inkemu does not model (GPIO, bit 2, belongs to sub-project 2d): WAKEUP_ENA 0x00004\n",
        ] {
            assert!(run.log.contains(line), "`{line}` is missing:\n{}", run.log);
        }
    }

    #[test]
    fn a_run_that_ends_on_its_limit_while_the_device_sleeps_says_when_the_timer_wakes_it() {
        // 272 000 counts of the 136 kHz slow clock: the alarm is at 2 s exactly.
        let run = run_bounded(
            deep_sleeping(1 << 3 << 15, 272_000),
            RunOptions {
                max_device_nanos: Some(SECOND_NS),
                ..UNLIMITED
            },
        );
        assert_eq!((run.end, run.device_nanos), (RunEnd::TimeLimit, SECOND_NS));
        assert!(
            run.log
                .contains(" us of device time; the RTC timer wakes it at 2000000 us\n"),
            "{}",
            run.log
        );
        // Run to 3 s, it wakes, sleeps again on the alarm already passed, and wakes again at once,
        // and so on: the journal says so at each wake.
        let run = run_bounded(
            deep_sleeping(1 << 3 << 15, 272_000),
            RunOptions {
                max_device_nanos: Some(3 * SECOND_NS),
                max_cycles: Some(2_000_000),
                ..UNLIMITED
            },
        );
        assert!(
            run.log.starts_with(
                "inkemu: woken from deep sleep by the RTC timer (rst:0x5), a core reset at 2000000 us of device time; the CPU had stopped at pc "
            ),
            "{}",
            run.log
        );
        assert!(
            run.log
                .matches("inkemu: woken from deep sleep by the RTC timer")
                .count()
                > 1
                && !run.log.contains("inkemu: repeated "),
            "{}",
            run.log
        );
    }

    #[test]
    fn the_deep_sleep_line_says_what_the_device_waits_for() {
        let line = |timer_wake_nanos, wake_sources| {
            deep_sleep_line(&DeepSleep {
                since_nanos: 1_500_000,
                timer_wake_nanos,
                wake_sources,
            })
        };
        assert_eq!(
            line(Some(3_500_000), DeepSleep::TIMER),
            "inkemu: the device is in deep sleep since 1500 us of device time; the RTC timer wakes it at 3500 us"
        );
        assert_eq!(
            line(None, 0),
            "inkemu: the device is in deep sleep since 1500 us of device time; it waits for nothing: the firmware enabled no wake source"
        );
        assert_eq!(
            line(None, 0x40),
            "inkemu: the device is in deep sleep since 1500 us of device time; it waits for a wake source inkemu does not model: WAKEUP_ENA 0x00040"
        );
        // The timer enabled as a source, its alarm not: a modelled source that wakes nothing.
        assert_eq!(
            line(None, DeepSleep::TIMER),
            "inkemu: the device is in deep sleep since 1500 us of device time; it waits for nothing: WAKEUP_ENA 0x00008; the RTC timer is a wake source but its alarm is not enabled (SLP_TIMER1.MAIN_TIMER_ALARM_EN)"
        );
        assert_eq!(
            line(None, DeepSleep::TIMER | DeepSleep::GPIO),
            "inkemu: the device is in deep sleep since 1500 us of device time; it waits for a wake source inkemu does not model (GPIO, bit 2, belongs to sub-project 2d): WAKEUP_ENA 0x0000c; the RTC timer is a wake source but its alarm is not enabled (SLP_TIMER1.MAIN_TIMER_ALARM_EN)"
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
                    usb_host: UsbHost::Open,
                },
            );
            let (mut console, mut log) = (Vec::new(), Vec::new());
            execute(
                &mut m,
                &options(5_000),
                &NOT_INTERRUPTED,
                &reader_stays,
                &mut console,
                &mut log,
            );
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
    fn a_console_whose_reader_went_away_ends_the_run_normally_and_says_why() {
        let mut m = machine(
            &printing(UART0, b"hello\n", &[HANG]),
            MachineOptions::default(),
        );
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &options(BACKSTOP),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut Closed,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleClosed, 0));
        let log = text(&log);
        assert!(
            log.contains("inkemu: the reader of standard output went away\ninkemu: stopped at pc "),
            "{log}"
        );
        assert!(!log.contains("cannot write"), "{log}");
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
            &options(BACKSTOP),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut Failing,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleError, 2));
        assert!(text(&log).contains("disk full"), "{}", text(&log));
    }

    /// Five bytes written, none of which has left the UART when the run stops: the console is
    /// written for the first time by the flush at the end of the run.
    fn bytes_still_in_the_uart(then: &[u32], options: MachineOptions) -> Esp32C3 {
        let mut program = printing(UART0, b"hello", &[]);
        program.extend_from_slice(then);
        machine(&program, options)
    }

    #[test]
    fn a_console_that_fails_at_the_final_flush_ends_the_run_as_a_console_error() {
        let mut m = bytes_still_in_the_uart(&[HANG], MachineOptions::default());
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &options(3_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut Failing,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleError, 2));
        let log = text(&log);
        assert!(
            log.contains(
                "error: cannot write the console output: disk full; check where stdout is redirected"
            ),
            "{log}"
        );
    }

    #[test]
    fn a_console_closed_at_the_final_flush_ends_the_run_as_console_closed() {
        let mut m = bytes_still_in_the_uart(&[HANG], MachineOptions::default());
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &options(3_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut Closed,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleClosed, 0));
        let log = text(&log);
        assert!(!log.contains("cannot write"), "{log}");
        assert!(
            log.contains(
                "inkemu: cycle limit reached; raise --max-cycles to run longer\ninkemu: the reader of standard output went away\n"
            ),
            "the limit ended the run, and its last bytes found nobody to read them: {log}"
        );
    }

    #[test]
    fn a_strict_mmio_stop_keeps_its_end_when_the_final_flush_fails() {
        let mut m = bytes_still_in_the_uart(&reading_unmodeled_register(), STRICT);
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &options(10_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut Failing,
            &mut log,
        );
        assert_eq!(end, RunEnd::StrictMmio);
        let log = text(&log);
        assert!(log.contains("cannot write the console output"), "{log}");
        assert!(log.contains("stopped by --strict-mmio"), "{log}");
    }

    /// Five bytes written to UART0, a wait of 4 096 turns of a two-instruction loop, then the
    /// read of the unmodelled register. A byte takes 6 940 cycles to leave at the reset baud rate
    /// and clock: when the read stops a strict run, some have left and some have not.
    fn printing_then_waiting_then_reading_the_unmodeled_register() -> Vec<u32> {
        let wait = [lui(T1, 0x1000), addi(T1, T1, -1), bne(T1, 0, -4)];
        let mut program = printing(UART0, b"hello", &wait);
        program.extend(reading_unmodeled_register());
        program
    }

    #[test]
    fn a_strict_stop_after_the_console_failed_says_so_once() {
        // The pass that ends on the violation has bytes to relay, and the UART still holds
        // others for the end of the run: a console that failed at the first is not written to
        // again, and its failure is one line of the log.
        let mut twin = machine(
            &printing_then_waiting_then_reading_the_unmodeled_register(),
            STRICT,
        );
        assert!(matches!(
            twin.run_until(Cycles(100_000)),
            StopReason::StrictMmio(_)
        ));
        assert_eq!(twin.take_console(ConsoleChannel::Uart0), b"he");
        assert_eq!(twin.flush_console(ConsoleChannel::Uart0), b"llo");

        let mut m = machine(
            &printing_then_waiting_then_reading_the_unmodeled_register(),
            STRICT,
        );
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &options(100_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut Failing,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::StrictMmio, 3));
        let log = text(&log);
        assert_eq!(
            log.matches("cannot write the console output").count(),
            1,
            "{log}"
        );
        assert!(log.contains("stopped by --strict-mmio"), "{log}");
    }

    #[test]
    fn a_strict_stop_after_the_reader_went_away_does_not_write_to_the_console_again() {
        let mut m = machine(
            &printing_then_waiting_then_reading_the_unmodeled_register(),
            STRICT,
        );
        let mut console = CountsWritesToAClosedPipe(0);
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &options(100_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(end, RunEnd::StrictMmio);
        assert_eq!(
            console.0, 1,
            "one write found the reader gone, none followed"
        );
    }

    /// A console whose reader went away, which counts the writes it refuses.
    struct CountsWritesToAClosedPipe(u32);
    impl io::Write for CountsWritesToAClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            self.0 += 1;
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_console_shows_what_was_written_when_the_run_stopped() {
        // Five bytes need 34 700 cycles to leave at the reset baud rate; the run stops long
        // before, on the unmodelled register the firmware reads right after printing.
        let mut program = printing(UART0, b"hello", &[]);
        program.extend(reading_unmodeled_register());
        let mut m = machine(&program, STRICT);
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut m,
            &options(10_000),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(end, RunEnd::StrictMmio);
        assert_eq!(
            console, b"hello",
            "the bytes still in the UART are not lost"
        );
    }

    #[test]
    fn a_cpu_asleep_while_a_watchdog_counts_does_not_end_the_run() {
        // Each life of this firmware is one byte and a `wfi`; main watchdog 0, armed by the reset
        // values, resets the chip every 1.3 s of device time (TW §D.4). The run goes on through
        // the sleeps and the resets until its cycle limit. Only the resets bring a sleeping CPU
        // to that limit: the device-time limit, twice the time the run takes, is there for a run
        // whose resets would stop, which would otherwise sleep for ever instead of failing.
        let mut m = machine(
            &printing(UART0, b"a", &[wfi(), HANG]),
            MachineOptions::default(),
        );
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut m,
            &RunOptions {
                max_device_nanos: Some(100 * SECOND_NS),
                ..options(300)
            },
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(end, RunEnd::Budget);
        // A life is four instructions (`lui`, `addi`, `sw`, `wfi`) and 6 cycles, then a sleep
        // that a watchdog ends: the 300 cycles of the limit are 50 lives. Measured when this was
        // written: 50 bytes, 49 resets (33 by main watchdog 0, 16 by the RTC watchdog) and
        // 48.36 s of device time. The bounds below ask for a tenth of that: more than 5 lives,
        // and more than 5 times the 1.3 s of the longest one. They are loose on purpose: the
        // cost of an instruction or the stage of a watchdog can change without this test, which
        // is about the run going on, having anything to say.
        assert!(
            console.len() > 5 && console.iter().all(|&byte| byte == b'a'),
            "one `a` per life: {console:?}"
        );
        // The byte of the life the limit interrupted may not have left the UART yet.
        let lives = u64::try_from(console.len()).unwrap();
        assert!(
            (m.resets()..=m.resets() + 1).contains(&lives),
            "{lives} bytes"
        );
        assert!(m.device_time_nanos() > 5 * 1_300_000_000);
        assert!(
            text(&log).contains("; the CPU is asleep, waiting for an interrupt\n"),
            "the limit is reached by the `wfi` of the last life: {}",
            text(&log)
        );
    }

    #[test]
    fn a_reset_is_reported_as_an_event_of_the_run_and_not_as_a_warning() {
        // The same firmware, under `--strict-mmio`: a watchdog reset is neither a fault of the
        // emulation nor a violation. Each reset is one line of the journal (spec §7), with its
        // device time, none is a repeat of another, and the run goes on to its cycle limit, with
        // the same device-time limit behind it for a run whose resets would stop.
        let mut m = machine(&printing(UART0, b"a", &[wfi(), HANG]), STRICT);
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut m,
            &RunOptions {
                max_device_nanos: Some(100 * SECOND_NS),
                ..options(300)
            },
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::Budget, 0));
        let log = text(&log);
        let resets: Vec<&str> = log
            .lines()
            .filter(|line| line.starts_with("inkemu: reset by "))
            .collect();
        assert_eq!(
            u64::try_from(resets.len()).unwrap(),
            m.resets(),
            "one line per reset: {log}"
        );
        // Main watchdog 0 resets the first lives, each 1.3 s after the reset before it.
        for (life, line) in (1..).zip(resets.iter().take(2)) {
            assert!(
                line.starts_with(
                    "inkemu: reset by main watchdog 0, a core reset (rst:0x7) at pc 0x40000010, sp 0x"
                ) && line.ends_with(&format!(", at {} us of device time", 1_300_000 * life)),
                "{log}"
            );
        }
        assert!(!log.contains("inkemu: repeated "), "{log}");
        assert!(!log.contains("warning:"), "{log}");
        assert!(m.resets() > 1);
    }

    #[test]
    fn an_interrupted_run_ends_with_the_summary_of_a_limit_stop() {
        // Ctrl-C sets the flag; the run ends at the next slice like a cycle limit.
        let mut m = machine(&[HANG], MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let interrupt = requested();
        let end = execute(
            &mut m,
            &options(BACKSTOP),
            &interrupt,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::Interrupted, 0));
        assert_eq!(m.cycles().get(), 0, "read before anything runs");
        let log = text(&log);
        assert!(log.contains("inkemu: interrupted by the user\n"), "{log}");
        assert!(
            log.contains("inkemu: stopped at pc ") && log.contains(" us of device time; "),
            "the same summary as any other end: {log}"
        );
    }

    /// A console that raises the interrupt flag when the run first writes to it, as a user
    /// pressing Ctrl-C once output has appeared.
    struct RaisesInterrupt<'a> {
        interrupt: &'a Interrupt,
        received: Vec<u8>,
    }
    impl io::Write for RaisesInterrupt<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            // A second request may come here: the run does not end the process, its handler does.
            let _ = self.interrupt.request();
            self.received.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// `SYSTIMER.CONF` as it resets (`0x4600_0000`: the clock gate and unit 0 at work) with
    /// `TARGET0_WORK_EN`, bit 24: the comparator of alarm 0 is on.
    const SYSTIMER_ALARM0_ON: u32 = 0x4600_0000 | (1 << 24);

    fn systimer(register: &str) -> u32 {
        svd_regs::esp32c3::addr("SYSTIMER", register)
    }

    /// Prints `x`, then sleeps under a period alarm every `period` counts of the 16 MHz counter
    /// whose interrupt is not enabled (`TARGET0_CONF`: bit 30 `PERIOD_MODE`, and the period):
    /// the CPU sleeps and wakes without an interrupt for ever, the machine returns `Asleep`
    /// again and again, and the cycle count never reaches a limit. Its watchdogs are left as
    /// they reset: main watchdog 0 resets the chip every 1.3 s of device time (TW §D.4), and the
    /// program starts again.
    fn sleeping_for_ever(period: u32) -> Vec<u32> {
        let mut tail = Vec::new();
        tail.extend(store_register(systimer("TARGET0_CONF"), (1 << 30) | period));
        tail.extend(store_register(systimer("COMP0_LOAD"), 1));
        tail.extend(store_register(systimer("CONF"), SYSTIMER_ALARM0_ON));
        tail.extend([wfi(), HANG]);
        printing(UART0, b"x", &tail)
    }

    /// What a run left, as plain data: a machine cannot leave the thread [`within`] runs it on.
    #[derive(Debug)]
    struct Outcome {
        end: RunEnd,
        console: Vec<u8>,
        log: String,
        cycles: u64,
        device_nanos: u64,
    }

    /// How long a run of these tests may take on the host before the test fails instead of
    /// hanging. It turns a hang into a failure and says nothing of speed, so it is wide: the
    /// longest run bounded by it, `a_device_time_limit_ends_a_run_whose_cpu_sleeps_for_ever`,
    /// took 12.4 s instrumented by `cargo llvm-cov` beside a release build and 16 spinning
    /// processes on 16 cores, and the limit is fifteen minutes, under the timeout of the CI
    /// jobs that run them.
    const HANG_LIMIT: Duration = Duration::from_mins(15);

    /// Runs `program` to the end of its run on a thread of its own, and fails after
    /// [`HANG_LIMIT`]. For the runs whose CPU sleeps: a sleep advances no cycle, so a run that
    /// lost its bound would otherwise hang the suite instead of failing.
    fn run_bounded(program: Vec<u32>, options: RunOptions) -> Outcome {
        within(
            HANG_LIMIT,
            "the run did not end within 15 minutes of host time: nothing bounds it any more",
            move || {
                let mut m = machine(&program, MachineOptions::default());
                let (mut console, mut log) = (Vec::new(), Vec::new());
                let end = execute(
                    &mut m,
                    &options,
                    &NOT_INTERRUPTED,
                    &reader_stays,
                    &mut console,
                    &mut log,
                );
                Outcome {
                    end,
                    console,
                    log: text(&log),
                    cycles: m.cycles().get(),
                    device_nanos: m.device_time_nanos(),
                }
            },
        )
    }

    #[test]
    fn a_device_time_limit_ends_a_run_whose_cpu_sleeps_for_ever() {
        // An alarm every 64 counts (4 us): half a million wake-ups in the two seconds, a hundred
        // thousand for each call of the machine, none of which can end the run.
        let run = run_bounded(
            sleeping_for_ever(64),
            RunOptions {
                max_device_nanos: Some(2 * SECOND_NS),
                ..UNLIMITED
            },
        );
        assert_eq!((run.end, run.end.exit_code()), (RunEnd::TimeLimit, 0));
        assert!(
            run.log
                .contains("inkemu: device-time limit reached; raise --max-time to run longer\n"),
            "{}",
            run.log
        );
        assert!(
            run.log.contains(
                "inkemu: 2000000 us of device time; CPU clock at 20 MHz; the CPU is asleep, waiting for an interrupt\n"
            ),
            "the limit instant exactly, and the word that says the firmware was idle: {}",
            run.log
        );
        assert_eq!(run.device_nanos, 2 * SECOND_NS);
        assert!(
            run.cycles < SLICE_CYCLES,
            "the CPU slept: no slice of cycles was ever used up ({} cycles)",
            run.cycles
        );
        // One life before the watchdog reset at 1.3 s, the start of a second one after it.
        assert_eq!(run.console, b"xx");
    }

    #[test]
    fn of_the_two_limits_the_first_one_reached_ends_the_run() {
        // `j .` at the 20 MHz of the reset clock: one millisecond is 20 000 cycles.
        let run = |max_cycles: Option<u64>, max_device_nanos: Option<u64>| {
            let mut m = machine(
                &[HANG],
                MachineOptions {
                    strict_mmio: false,
                    trace_hash: true,
                    usb_host: UsbHost::Open,
                },
            );
            let (mut console, mut log) = (Vec::new(), Vec::new());
            let end = execute(
                &mut m,
                &RunOptions {
                    max_cycles,
                    max_device_nanos,
                    ..UNLIMITED
                },
                &NOT_INTERRUPTED,
                &reader_stays,
                &mut console,
                &mut log,
            );
            let log = text(&log);
            assert!(!log.contains("the CPU is asleep"), "{log}");
            (end, m.cycles().get(), m.trace_hash(), log)
        };
        let (end, cycles, _, log) = run(Some(1_000), Some(SECOND_NS));
        assert_eq!(end, RunEnd::Budget);
        assert!((1_000..1_010).contains(&cycles), "{cycles} cycles");
        assert!(
            log.contains("cycle limit reached") && !log.contains("device-time limit"),
            "{log}"
        );
        let (end, cycles, by_time, log) = run(Some(1_000_000_000), Some(SECOND_NS / 1_000));
        assert_eq!((end, end.exit_code()), (RunEnd::TimeLimit, 0));
        assert!(
            (20_000..20_010).contains(&cycles),
            "the first instruction boundary at or past the millisecond: {cycles} cycles"
        );
        assert!(
            log.contains("device-time limit reached") && !log.contains("cycle limit"),
            "{log}"
        );
        // The limit that ended the run is not part of its trace.
        let (end, same, by_cycles, _) = run(Some(cycles), None);
        assert_eq!((end, same), (RunEnd::Budget, cycles));
        assert!(by_time.is_some());
        assert_eq!(by_time, by_cycles);
    }

    #[test]
    fn a_limit_of_no_time_at_all_stops_before_the_first_instruction() {
        let run = run_bounded(
            vec![HANG],
            RunOptions {
                max_device_nanos: Some(0),
                ..UNLIMITED
            },
        );
        assert_eq!(
            (run.end, run.cycles, run.device_nanos),
            (RunEnd::TimeLimit, 0, 0)
        );
    }

    /// Its watchdogs stopped, a one-shot alarm at count 32 000 000 of the 16 MHz counter (two
    /// seconds after power-on) whose interrupt is not enabled, then `wfi`.
    fn sleeping_to_an_alarm_two_seconds_after_power_on() -> Vec<u32> {
        let mut program = stop_watchdogs();
        program.extend(store_register(systimer("TARGET0_LO"), 32_000_000));
        program.extend(store_register(systimer("COMP0_LOAD"), 1));
        program.extend(store_register(systimer("CONF"), SYSTIMER_ALARM0_ON));
        program.extend([wfi(), HANG]);
        program
    }

    #[test]
    fn a_run_goes_on_through_a_long_sleep_and_ends_when_nothing_is_left_to_wake_the_cpu() {
        // The machine gives control back after the sleep to the alarm (`Asleep`: more than a
        // second slept), which is not an end; the alarm raises no interrupt and nothing else is
        // scheduled, so the next call finds a CPU nothing can wake.
        let run = run_bounded(sleeping_to_an_alarm_two_seconds_after_power_on(), UNLIMITED);
        assert_eq!((run.end, run.end.exit_code()), (RunEnd::Idle, 4));
        assert_eq!(
            run.device_nanos,
            2 * SECOND_NS,
            "the sleep ended on the alarm, and time stopped there"
        );
        assert!(
            run.log.contains("waits for an interrupt and none can come"),
            "{}",
            run.log
        );
        assert!(
            run.log
                .contains("inkemu: 2000000 us of device time; CPU clock at 20 MHz; the CPU is asleep, waiting for an interrupt\n"),
            "{}",
            run.log
        );
    }

    #[test]
    fn a_sleep_that_ends_on_the_instant_of_the_limit_is_followed_by_the_limit() {
        // The machine says `Asleep` for the call whose sleep of two seconds ended exactly on the
        // limit, and `TimeBudget` only for the next one, which runs nothing: the run must go
        // round once more and end on the limit, not go on or end as idle.
        let limit = Some(2 * SECOND_NS);
        let mut twin = machine(
            &sleeping_to_an_alarm_two_seconds_after_power_on(),
            MachineOptions::default(),
        );
        let far = Cycles(SLICE_CYCLES);
        assert_eq!(twin.run_within(far, limit), StopReason::Asleep);
        assert_eq!(twin.device_time_nanos(), 2 * SECOND_NS);
        assert_eq!(twin.run_within(far, limit), StopReason::TimeBudget);

        let run = run_bounded(
            sleeping_to_an_alarm_two_seconds_after_power_on(),
            RunOptions {
                max_device_nanos: limit,
                ..UNLIMITED
            },
        );
        assert_eq!((run.end, run.end.exit_code()), (RunEnd::TimeLimit, 0));
        assert_eq!(run.device_nanos, 2 * SECOND_NS);
        assert!(
            run.log.contains("device-time limit reached")
                && run.log.contains("the CPU is asleep")
                && !run.log.contains("none can come"),
            "{}",
            run.log
        );
    }

    #[test]
    fn seconds_of_device_time_are_read_without_floating_point() {
        for (text, nanos) in [
            ("30", 30 * SECOND_NS),
            ("2.5", 2_500_000_000),
            ("0.001", 1_000_000),
            ("0", 0),
            ("0.000000001", 1),
            ("007", 7 * SECOND_NS),
            ("1.000000000", SECOND_NS),
            ("0.1", 100_000_000),
            ("18446744073.709551615", u64::MAX),
        ] {
            assert_eq!(parse_device_seconds(text), Ok(nanos), "`{text}`");
        }
    }

    #[test]
    fn a_time_that_is_not_digits_with_an_optional_fraction_is_refused() {
        for text in [
            "", "abc", "-1", "+1", "1e3", "1.", ".5", " 1", "1 ", "1s", "1_000", "1,5", "1.2.3",
            "1.-5", "٣",
        ] {
            assert_eq!(
                parse_device_seconds(text),
                Err(format!(
                    "`{text}` is not a number of seconds; write digits with an optional fraction, like 30 or 0.25"
                ))
            );
        }
    }

    #[test]
    fn a_time_finer_than_a_nanosecond_is_refused() {
        for text in ["1.0000000001", "0.0000000000"] {
            assert_eq!(
                parse_device_seconds(text),
                Err(format!(
                    "`{text}` has more than 9 digits after the point, which is finer than a nanosecond; give at most 9"
                ))
            );
        }
    }

    #[test]
    fn a_time_device_time_cannot_count_is_refused() {
        // One nanosecond past the largest `u64` of nanoseconds, the next whole second, and a
        // number whose digits alone overflow.
        for text in [
            "18446744073.709551616",
            "18446744074",
            "99999999999999999999999999",
        ] {
            assert_eq!(
                parse_device_seconds(text),
                Err(format!(
                    "`{text}` seconds is more than device time can count (584 years); give a smaller number"
                ))
            );
        }
    }

    #[test]
    fn an_interrupt_ends_a_run_whose_cpu_sleeps_for_ever() {
        // An alarm at every count of the 16 MHz counter. The flag, raised once the byte printed
        // before the sleep has reached the console, must be read on the path of a sleeping CPU
        // too. A cycle limit could not end this run if the flag were no longer read; a quarter
        // of a second of device time does, as `TimeLimit`, and the assertion on the end fails.
        // The flag is raised by the first pass, 6.25 ms of device time into the run (a hundred
        // thousand alarms), and read at the start of the second: the limit is forty passes away,
        // and near enough for a run that lost its end to fail in seconds, not minutes.
        let mut m = machine(&sleeping_for_ever(1), MachineOptions::default());
        let interrupt = Interrupt::new();
        let mut console = RaisesInterrupt {
            interrupt: &interrupt,
            received: Vec::new(),
        };
        let mut log = Vec::new();
        let end = execute(
            &mut m,
            &RunOptions {
                max_device_nanos: Some(SECOND_NS / 4),
                ..UNLIMITED
            },
            &interrupt,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::Interrupted, 0));
        assert_eq!(console.received, b"x");
        assert!(
            m.cycles().get() < SLICE_CYCLES,
            "the CPU slept: no slice of cycles was ever used up ({} cycles)",
            m.cycles().get()
        );
        let log = text(&log);
        assert!(log.contains("inkemu: interrupted by the user\n"), "{log}");
        assert!(log.contains(" us of device time; "), "{log}");
    }

    #[test]
    fn an_interrupted_run_still_flushes_what_the_uart_holds() {
        // Five bytes written, none sent yet when the run stops: the interrupt ends the run like a
        // limit, and the end-of-run flush hands them to the console before the summary.
        let mut m = bytes_still_in_the_uart(&[HANG], MachineOptions::default());
        assert_eq!(m.run_until(Cycles(3_000)), StopReason::CycleBudget);
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let interrupt = requested();
        let end = execute(
            &mut m,
            &options(10_000),
            &interrupt,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(end, RunEnd::Interrupted);
        assert_eq!(console, b"hello");
    }

    #[test]
    fn a_console_that_fails_at_the_flush_of_an_interrupted_run_ends_it_as_a_console_error() {
        let mut m = bytes_still_in_the_uart(&[HANG], MachineOptions::default());
        assert_eq!(m.run_until(Cycles(3_000)), StopReason::CycleBudget);
        let mut log = Vec::new();
        let interrupt = requested();
        let end = execute(
            &mut m,
            &options(10_000),
            &interrupt,
            &reader_stays,
            &mut Failing,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleError, 2));
        let log = text(&log);
        assert!(log.contains("interrupted by the user"), "{log}");
        assert!(log.contains("cannot write the console output"), "{log}");
    }

    #[test]
    fn a_run_that_prints_nothing_ends_when_its_reader_goes_away() {
        // The firmware is silent, so no write ever fails: the run has to ask. The reader goes
        // away during the third pass; the cycle limit is only there to end a run that would no
        // longer ask.
        let mut m = machine(&[HANG], MachineOptions::default());
        let asked = std::cell::Cell::new(0u32);
        let gone_at_the_third_pass = || {
            asked.set(asked.get() + 1);
            asked.get() >= 3
        };
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut m,
            &options(10 * SLICE_CYCLES),
            &NOT_INTERRUPTED,
            &gone_at_the_third_pass,
            &mut console,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleClosed, 0));
        assert_eq!(asked.get(), 3, "asked once per pass");
        assert!(
            (3 * SLICE_CYCLES..4 * SLICE_CYCLES).contains(&m.cycles().get()),
            "{:?}",
            m.cycles()
        );
        let log = text(&log);
        assert!(
            log.contains("inkemu: the reader of standard output went away\ninkemu: stopped at pc "),
            "{log}"
        );
        assert!(!log.contains("cycle limit"), "{log}");
    }

    #[test]
    fn a_run_whose_cpu_sleeps_ends_when_its_reader_goes_away() {
        let run = within(
            HANG_LIMIT,
            "the run did not end within 15 minutes of host time: it no longer asks for its reader",
            || {
                // Half a million wake-ups a call of the machine cannot end the run; ten seconds of
                // device time would, were the question not asked.
                let mut m = machine(&sleeping_for_ever(64), MachineOptions::default());
                let (mut console, mut log) = (Vec::new(), Vec::new());
                let end = execute(
                    &mut m,
                    &RunOptions {
                        max_device_nanos: Some(10 * SECOND_NS),
                        ..UNLIMITED
                    },
                    &NOT_INTERRUPTED,
                    // The first pass relays the `x` the firmware printed before it slept.
                    &|| true,
                    &mut console,
                    &mut log,
                );
                (end, console, text(&log), m.device_time_nanos())
            },
        );
        let (end, console, log, device_nanos) = run;
        assert_eq!(end, RunEnd::ConsoleClosed);
        assert_eq!(console, b"x");
        assert!(device_nanos < SECOND_NS, "{device_nanos} ns");
        assert!(
            log.contains("inkemu: the reader of standard output went away\n")
                && log.contains("; the CPU is asleep, waiting for an interrupt\n"),
            "{log}"
        );
    }

    #[test]
    fn a_pass_that_wrote_to_the_console_does_not_ask_for_its_reader() {
        // A write that succeeds has just found the reader there. One slice, in which the byte
        // leaves the UART: the only pass of this run writes, and the answer is never asked for.
        let mut m = machine(&printing(UART0, b"x", &[HANG]), MachineOptions::default());
        let asked = std::cell::Cell::new(false);
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut m,
            &options(SLICE_CYCLES),
            &NOT_INTERRUPTED,
            &|| {
                asked.set(true);
                true
            },
            &mut console,
            &mut log,
        );
        assert_eq!(end, RunEnd::Budget);
        assert_eq!(console, b"x");
        assert!(!asked.get());
    }

    #[test]
    fn the_reader_is_asked_for_by_the_first_silent_pass_then_once_in_64_or_once_a_slice() {
        let mut watch = ReaderWatch::default();
        let mut asked = |cycles: u64| watch.asks(cycles);
        assert!(asked(100), "the first pass that wrote nothing asks");
        // Passes that execute next to nothing: 63 go by, the 64th asks.
        let quiet: Vec<bool> = (0..64).map(|pass| asked(101 + pass)).collect();
        assert_eq!(quiet.iter().filter(|asks| **asks).count(), 1);
        assert_eq!(quiet.last(), Some(&true), "{quiet:?}");
        // A whole slice executed since the question: the pass asks without waiting for its turn,
        // and the count of passes starts again from it.
        let last = 164;
        assert!(!asked(last + SLICE_CYCLES - 1));
        assert!(asked(last + SLICE_CYCLES));
        let after: Vec<bool> = (1..=64)
            .map(|pass| asked(last + SLICE_CYCLES + pass))
            .collect();
        assert_eq!(after.iter().filter(|asks| **asks).count(), 1);
        assert_eq!(after.last(), Some(&true), "{after:?}");
    }

    /// Sleeps without printing, its watchdogs stopped, under a period alarm every half second
    /// (8 000 000 counts of the 16 MHz counter) whose interrupt is not enabled: a call of the
    /// machine sleeps through a second of device time and returns `Asleep`, for ever, having
    /// executed nothing.
    fn sleeping_silently() -> Vec<u32> {
        const HALF_A_SECOND: u32 = 8_000_000;
        let mut program = stop_watchdogs();
        program.extend(store_register(
            systimer("TARGET0_CONF"),
            (1 << 30) | HALF_A_SECOND,
        ));
        program.extend(store_register(systimer("COMP0_LOAD"), 1));
        program.extend(store_register(systimer("CONF"), SYSTIMER_ALARM0_ON));
        program.extend([wfi(), HANG]);
        program
    }

    #[test]
    fn a_firmware_that_sleeps_without_printing_is_asked_about_its_reader_one_pass_in_64() {
        const LIMIT: RunOptions = RunOptions {
            max_device_nanos: Some(200 * SECOND_NS),
            ..UNLIMITED
        };
        // The passes of the run, counted on a twin driven as `execute` drives it.
        let passes = {
            let mut twin = machine(&sleeping_silently(), MachineOptions::default());
            let mut passes = 0u32;
            loop {
                passes += 1;
                assert!(passes < 1_000, "the device-time limit is never reached");
                let slice_end = Cycles(twin.cycles().get() + SLICE_CYCLES);
                match twin.run_within(slice_end, LIMIT.max_device_nanos) {
                    StopReason::Asleep => {}
                    StopReason::TimeBudget => break passes,
                    other => panic!("{other:?}"),
                }
            }
        };
        assert!(
            (190..=210).contains(&passes),
            "a pass for each second slept: {passes}"
        );

        let run = |gone_from: u32| {
            let mut m = machine(&sleeping_silently(), MachineOptions::default());
            let asked = std::cell::Cell::new(0u32);
            let (mut console, mut log) = (Vec::new(), Vec::new());
            let end = execute(
                &mut m,
                &LIMIT,
                &NOT_INTERRUPTED,
                &|| {
                    asked.set(asked.get() + 1);
                    asked.get() >= gone_from
                },
                &mut console,
                &mut log,
            );
            assert_eq!(console, b"");
            assert!(m.cycles().get() < SLICE_CYCLES, "{:?}", m.cycles());
            (end, asked.get(), m.device_time_nanos())
        };
        // A reader that stays: asked for by the first pass and by every 64th after it.
        let (end, asked, _) = run(u32::MAX);
        assert_eq!(end, RunEnd::TimeLimit);
        assert_eq!(asked, passes.div_ceil(SILENT_PASSES_PER_QUESTION));
        assert_eq!(asked, 4, "{passes} passes");
        // A reader gone from the start of the run, or from just after the first question: the
        // run ends at the next question, 64 passes of a second each later at most.
        let (end, asked, device_nanos) = run(1);
        assert_eq!((end, asked), (RunEnd::ConsoleClosed, 1));
        assert!(device_nanos < 2 * SECOND_NS, "{device_nanos} ns");
        let (end, asked, device_nanos) = run(2);
        assert_eq!((end, asked), (RunEnd::ConsoleClosed, 2));
        assert!(
            (64 * SECOND_NS..66 * SECOND_NS).contains(&device_nanos),
            "the 65th pass asks: {device_nanos} ns"
        );
    }

    #[test]
    fn a_strict_stop_keeps_its_exit_code_when_the_reader_of_a_silent_run_is_gone() {
        let mut m = machine(&reading_unmodeled_register(), STRICT);
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut m,
            &options(10_000),
            &NOT_INTERRUPTED,
            &|| true,
            &mut console,
            &mut log,
        );
        assert_eq!((end, end.exit_code()), (RunEnd::StrictMmio, 3));
    }

    #[test]
    fn an_interrupt_does_not_change_the_trace_of_the_run_it_ends() {
        // The request is read between two calls of the machine and never enters a trace. The
        // console raises the flag when the byte the firmware printed reaches it, at the end of
        // the first slice: the run executes that slice, and ends at the next pass. A second run,
        // limited to the cycle count the first one stopped at, must print the same hash.
        const TRACED: MachineOptions = MachineOptions {
            strict_mmio: false,
            trace_hash: true,
            usb_host: UsbHost::Open,
        };
        let program = printing(UART0, b"x", &[HANG]);
        let hash_line = |log: &[u8]| {
            text(log)
                .lines()
                .find(|line| line.starts_with("trace-hash "))
                .map(str::to_owned)
        };

        let mut interrupted = machine(&program, TRACED);
        let interrupt = Interrupt::new();
        let mut console = RaisesInterrupt {
            interrupt: &interrupt,
            received: Vec::new(),
        };
        let mut log = Vec::new();
        let end = execute(
            &mut interrupted,
            &options(BACKSTOP),
            &interrupt,
            &reader_stays,
            &mut console,
            &mut log,
        );
        assert_eq!(end, RunEnd::Interrupted);
        let cycles = interrupted.cycles().get();
        assert!(
            (SLICE_CYCLES..2 * SLICE_CYCLES).contains(&cycles),
            "one slice ran before the flag was read: {cycles} cycles"
        );

        let mut limited = machine(&program, TRACED);
        let (mut limited_console, mut limited_log) = (Vec::new(), Vec::new());
        let end = execute(
            &mut limited,
            &options(cycles),
            &NOT_INTERRUPTED,
            &reader_stays,
            &mut limited_console,
            &mut limited_log,
        );
        assert_eq!((end, limited.cycles().get()), (RunEnd::Budget, cycles));
        assert_eq!(limited_console, console.received);
        assert!(hash_line(&log).is_some());
        assert_eq!(hash_line(&log), hash_line(&limited_log));
    }
}
