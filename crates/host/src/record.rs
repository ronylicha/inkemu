//! `inkemu run --record` and `inkemu replay` around [`crate::run::execute_with_script`]: what a
//! run keeps for its session, and the verdict of a replay. The session itself, its format and
//! its replay belong to the `session` crate; this module joins them to the run of the command
//! line, which shows a replay as it shows a run.
use crate::exit::{EXIT_DIVERGED, EXIT_ERROR, EXIT_OK};
use crate::run::{RunEnd, RunOptions, session_end};
use crate::session_file;
use crate::term::sanitize;
use boards::Wired;
use chips::sdcard::Changes;
use session::{
    ConsoleDigests, Cut, CutReason, Divergence, End, Limits, NoTraceHash, Outcome, ProfileText,
    Replay, ReplayError, RomIdentity, SdCardStart, Session, WriteError,
};
use soc_esp32c3::{ConsoleChannel, Esp32C3, RomImage};
use std::io::Write;
use std::path::Path;

/// Why a run could not be recorded when it ended.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("{0}")]
    Write(#[from] WriteError),
    #[error("{0}")]
    NoTraceHash(#[from] NoTraceHash),
}

/// What a recorded run keeps beside its machine: what it started from, its options, and the
/// digests of its consoles, which [`crate::run::execute_with_script`] fills.
#[derive(Debug)]
pub struct Recorder {
    rom: RomIdentity,
    profile: ProfileText,
    flash: Vec<u8>,
    /// The SD card in the slot at power-on, as the session keeps it (boxed: a recorder sits in
    /// the mode of a life, beside the replay's).
    sd: Option<Box<SdCardStart>>,
    limits: Limits,
    digests: ConsoleDigests,
}

impl Recorder {
    /// Begins the record of a run of `machine`, at power-on, which runs on the ROM `rom`, the
    /// profile `profile`, with the SD card `sd` in its slot and with `limits` (no cut yet).
    #[must_use]
    pub fn begin(
        rom: RomIdentity,
        profile: ProfileText,
        machine: &Esp32C3,
        sd: Option<SdCardStart>,
        limits: Limits,
    ) -> Recorder {
        Recorder {
            rom,
            profile,
            flash: machine.flash_contents(),
            sd: sd.map(Box::new),
            limits,
            digests: ConsoleDigests::default(),
        }
    }

    /// The digests the run takes its consoles into.
    pub const fn digests(&mut self) -> &mut ConsoleDigests {
        &mut self.digests
    }

    /// The session of `machine`, whose run ended as `end` with `sd` the changes of the card in
    /// its slot: a run the host cut is recorded with the device time it stopped at (G5).
    ///
    /// # Errors
    /// [`RecordError`] when the session cannot hold the run, or the machine keeps no trace hash.
    pub fn finish(
        self,
        machine: &Esp32C3,
        end: RunEnd,
        sd: Option<&Changes<'_>>,
    ) -> Result<Session, RecordError> {
        let (limits, end) = match session_end(end) {
            Ok(reached) => (
                self.limits,
                self.limits.end_of(reached, machine.device_time()),
            ),
            Err(reason) => (
                Limits {
                    cut: Some(Cut {
                        at: machine.device_time(),
                        reason,
                    }),
                    ..self.limits
                },
                End::Cut,
            ),
        };
        let outcome = Outcome::of(machine, end, self.digests, sd)?;
        Ok(Session::record(
            self.rom,
            self.profile,
            self.flash,
            self.sd.map(|card| *card),
            limits,
            machine.input_journal(),
            outcome,
        )?)
    }
}

/// The options of a replay of `session` that shows `console`: the limits of the session, its cut
/// one more device-time limit.
#[must_use]
pub fn replay_options(session: &Session, console: ConsoleChannel) -> RunOptions {
    let (max_cycles, max_device_nanos) = session.limits.run_limits();
    RunOptions {
        max_cycles,
        max_device_nanos,
        console,
    }
}

/// How a replay compares with its session.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The replay gave the outcome recorded.
    Identical,
    /// The replay gave another outcome.
    Diverged(Divergence),
    /// The host cut the replay before the end of its session: nothing was compared.
    Cut(CutReason),
}

/// The verdict of the replay of `session` on `machine`, which ended as `end`, its consoles taken
/// into `digests` to the last byte, `sd` the changes of the card in its slot.
///
/// # Errors
/// [`NoTraceHash`] when the machine keeps no trace hash.
pub fn verdict(
    session: &Session,
    machine: &Esp32C3,
    end: RunEnd,
    digests: ConsoleDigests,
    sd: Option<&Changes<'_>>,
) -> Result<Verdict, NoTraceHash> {
    let reached = match session_end(end) {
        Ok(reached) => reached,
        Err(reason) => return Ok(Verdict::Cut(reason)),
    };
    let end = session.limits.end_of(reached, machine.device_time());
    let replayed = Outcome::of(machine, end, digests, sd)?;
    let differences = session.outcome.differences(&replayed);
    Ok(if differences.is_empty() {
        Verdict::Identical
    } else {
        Verdict::Diverged(Divergence(differences))
    })
}

/// Writes the session of the run of `machine`, which ended as `end` with `sd` the changes of the
/// card in its slot, to `path`, and says so in `log`; `false`, after saying why, when the run is
/// not recorded. A failed write of the file says its path and the remedy itself; a session that
/// cannot hold the run is said with the path it was not written to.
pub fn record_session(
    recorder: Recorder,
    machine: &Esp32C3,
    end: RunEnd,
    sd: Option<&Changes<'_>>,
    path: &Path,
    log: &mut dyn Write,
) -> bool {
    let not_recorded = |log: &mut dyn Write, cause: &dyn std::fmt::Display| {
        let _ = writeln!(
            log,
            "error: the run is not recorded in {}: {cause}",
            sanitize(&path.display().to_string())
        );
        false
    };
    let session = match recorder.finish(machine, end, sd) {
        Ok(session) => session,
        Err(e) => return not_recorded(log, &e),
    };
    let bytes = match session.to_bytes() {
        Ok(bytes) => bytes,
        Err(e) => return not_recorded(log, &e),
    };
    if let Err(e) = session_file::write_session(path, &bytes) {
        let _ = writeln!(log, "error: {e}");
        return false;
    }
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let _ = writeln!(
        log,
        "inkemu: session recorded in {} ({} inputs); `inkemu replay` replays it",
        sanitize(&name),
        session.inputs.len()
    );
    true
}

/// The wired device that replays `session` on `rom`, which this inkemu loads as ROM revision
/// `rom_revision` for the device of the session: its machine, and its card in its slot. A
/// refusal is given as its message, its control characters escaped (a session comes from
/// anywhere), with the remedy for a ROM that is not the one the session was recorded on.
///
/// # Errors
/// The message of the [`ReplayError`].
pub fn start_replay(session: &Session, rom: RomImage, rom_revision: u16) -> Result<Wired, String> {
    match Replay::start(session, rom) {
        Ok(replay) => Ok(replay.into_wired()),
        Err(e @ ReplayError::Rom { .. }) => Err(sanitize(&format!(
            "{e}: this inkemu loads ROM revision {rom_revision} for the device, from `inkemu rom fetch --device {}` or --rom-dir; a session recorded on another ROM replays with the inkemu that recorded it",
            session.profile.id
        ))),
        Err(e) => Err(sanitize(&e.to_string())),
    }
}

/// Says in `log` how the replay of `session` on `machine`, which ended as `end` with every
/// console taken into `digests` and `sd` the changes of the card in its slot, compares with what
/// the session recorded, and gives the exit code of the replay: [`EXIT_OK`] when identical,
/// [`EXIT_DIVERGED`] when it diverges, and [`EXIT_ERROR`] when nothing could be compared (a
/// replay the host cut, a machine without its trace hash).
pub fn report_replay(
    session: &Session,
    machine: &Esp32C3,
    end: RunEnd,
    digests: ConsoleDigests,
    sd: Option<&Changes<'_>>,
    log: &mut dyn Write,
) -> u8 {
    if let Some(cut) = session.limits.cut {
        let _ = writeln!(
            log,
            "inkemu: the session was cut by {} at {} us of device time when it was recorded; its replay stops there",
            cut.reason.name(),
            soc_esp32c3::clock::TIME_BASE.cycles_to_nanos(cut.at) / 1_000
        );
    }
    match verdict(session, machine, end, digests, sd) {
        Ok(Verdict::Identical) => {
            let _ = writeln!(
                log,
                "inkemu: the replay gives what the session recorded: the same end, device time, cycles, instructions, trace hash, final flash, SD card and consoles"
            );
            EXIT_OK
        }
        Ok(Verdict::Diverged(divergence)) => {
            let _ = writeln!(
                log,
                "inkemu: the replay diverges from the session: {}",
                sanitize(&divergence.to_string())
            );
            EXIT_DIVERGED
        }
        Ok(Verdict::Cut(reason)) => {
            let _ = writeln!(
                log,
                "error: the replay was cut by {} before the end of its session: nothing was compared; let it run to its end",
                reason.name()
            );
            EXIT_ERROR
        }
        Err(e) => {
            let _ = writeln!(log, "error: {e}");
            EXIT_ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Interrupt, RunExtras, execute_with_script};
    use boards::MachineOptions;
    use cpu_rv32::asm::{beq, jal, lbu, lw, sw, wfi};
    use emu_core::Cycles;
    use session::Quantity;
    use soc_esp32c3::UsbHost;
    use soc_esp32c3::test_support::{HANG, T0, T1, flat_rom, print_program, stop_watchdogs};

    const UART0: u32 = 0x6000_0000;
    const USB: u32 = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");

    /// Prints `ready` on UART0, then echoes what UART0 receives.
    fn echo() -> RomImage {
        let mut program = print_program(UART0, b"ready");
        program.extend([
            lbu(T1, T0, 0x1C),
            beq(T1, 0, -4),
            lw(T1, T0, 0),
            sw(T1, T0, 0),
            jal(0, -16),
        ]);
        flat_rom(&program)
    }

    fn x4_text() -> ProfileText {
        ProfileText {
            id: "x4".to_owned(),
            text: boards::builtin_toml("x4").unwrap().to_owned(),
        }
    }

    fn run(machine: &mut Esp32C3, options: &RunOptions, digests: &mut ConsoleDigests) -> RunEnd {
        execute_with_script(
            machine,
            options,
            RunExtras {
                script: None,
                digests: Some(digests),
            },
            &Interrupt::new(),
            &|| false,
            &mut Vec::new(),
            &mut Vec::new(),
        )
    }

    /// A run of [`echo`] to 20 ms, typed into at 5 ms, recorded.
    fn recorded() -> Session {
        let rom = echo();
        let identity = RomIdentity::of(3, &rom);
        let profile = boards::builtin("x4").unwrap();
        let options = MachineOptions {
            strict_mmio: false,
            trace_hash: true,
            usb_host: UsbHost::Attached,
        };
        let mut machine = profile.instantiate(rom, options);
        let limits = Limits {
            max_time_nanos: Some(20_000_000),
            ..Limits::default()
        };
        let mut recorder = Recorder::begin(identity, x4_text(), &machine, None, limits);
        machine
            .send_console(ConsoleChannel::Uart0, Cycles(800_000), b"hi\r")
            .unwrap();
        let options = RunOptions {
            max_cycles: None,
            max_device_nanos: Some(20_000_000),
            console: ConsoleChannel::Uart0,
        };
        let end = run(&mut machine, &options, recorder.digests());
        assert_eq!(end, RunEnd::TimeLimit);
        recorder.finish(&machine, end, None).unwrap()
    }

    fn replayed(session: &Session) -> Verdict {
        let mut machine = Replay::start(session, echo()).unwrap().into_machine();
        let mut digests = ConsoleDigests::default();
        let end = run(
            &mut machine,
            &replay_options(session, ConsoleChannel::UsbSerialJtag),
            &mut digests,
        );
        verdict(session, &machine, end, digests, None).unwrap()
    }

    #[test]
    fn a_recorded_run_replays_identically_through_the_run_of_the_command_line() {
        let session = recorded();
        assert_eq!(session.outcome.end, End::TimeLimit);
        assert_eq!(session.usb_host, UsbHost::Attached);
        // Through its bytes, as the file gives it back; the console shown changes nothing.
        let read = Session::from_bytes(&session.to_bytes().unwrap()).unwrap();
        assert_eq!(replayed(&read), Verdict::Identical);
    }

    #[test]
    fn a_changed_input_diverges_and_names_the_console() {
        let mut session = recorded();
        let Some(soc_esp32c3::AcceptedInput::Console { bytes, .. }) = session.inputs.first_mut()
        else {
            panic!("the run was typed into")
        };
        bytes[0] = b'H';
        let Verdict::Diverged(divergence) = replayed(&session) else {
            panic!("a changed input diverges")
        };
        assert_eq!(
            divergence.0[0].quantity,
            Quantity::Console(ConsoleChannel::Uart0)
        );
    }

    #[test]
    fn a_run_the_host_cut_is_recorded_with_its_instant_and_replays_to_it() {
        let rom = echo();
        let identity = RomIdentity::of(3, &rom);
        let mut machine = boards::builtin("x4").unwrap().instantiate(
            rom,
            MachineOptions {
                trace_hash: true,
                ..MachineOptions::default()
            },
        );
        // The limits the recorder keeps are those of the run below.
        let limits = Limits {
            max_cycles: Some(1_000),
            ..Limits::default()
        };
        let mut recorder = Recorder::begin(identity, x4_text(), &machine, None, limits);
        // Ctrl-C before the run: it stops at its first pass, at power-on.
        let interrupt = Interrupt::new();
        let _ = interrupt.request();
        let options = RunOptions {
            max_cycles: Some(1_000),
            max_device_nanos: None,
            console: ConsoleChannel::Uart0,
        };
        let end = execute_with_script(
            &mut machine,
            &options,
            RunExtras {
                script: None,
                digests: Some(recorder.digests()),
            },
            &interrupt,
            &|| false,
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert_eq!(end, RunEnd::Interrupted);
        let session = recorder.finish(&machine, end, None).unwrap();
        assert_eq!(session.outcome.end, End::Cut);
        assert_eq!(
            session.limits.cut,
            Some(Cut {
                at: Cycles(0),
                reason: CutReason::Interrupted
            })
        );
        assert_eq!(replayed(&session), Verdict::Identical);
    }

    #[test]
    fn the_ends_the_host_makes_are_cuts_and_the_others_are_stops_of_the_machine() {
        for end in RunEnd::ALL {
            let host = matches!(
                end,
                RunEnd::Interrupted | RunEnd::ConsoleClosed | RunEnd::ConsoleError
            );
            assert_eq!(session_end(end).is_err(), host, "{end:?}");
        }
    }

    /// A console that asks the run to stop when it is first written to: Ctrl-C pressed once the
    /// firmware has printed, somewhere else than at power-on.
    struct PressesCtrlC<'a> {
        interrupt: &'a Interrupt,
    }
    impl Write for PressesCtrlC<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let _ = self.interrupt.request();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A console that cannot be written.
    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// `rom` on the X4 with its trace hash, the recorder begun at power-on with `limits`.
    fn recording(rom: RomImage, limits: Limits) -> (Esp32C3, Recorder) {
        let identity = RomIdentity::of(3, &rom);
        let machine = boards::builtin("x4").unwrap().instantiate(
            rom,
            MachineOptions {
                trace_hash: true,
                ..MachineOptions::default()
            },
        );
        let recorder = Recorder::begin(identity, x4_text(), &machine, None, limits);
        (machine, recorder)
    }

    /// The run of `machine` to `limits`, recorded, its console `console`, Ctrl-C `interrupt`.
    fn record_through(
        machine: &mut Esp32C3,
        mut recorder: Recorder,
        (limits, shown): (Limits, ConsoleChannel),
        interrupt: &Interrupt,
        console: &mut dyn Write,
    ) -> (RunEnd, Session, String) {
        let (max_cycles, max_device_nanos) = limits.run_limits();
        let options = RunOptions {
            max_cycles,
            max_device_nanos,
            console: shown,
        };
        let mut log = Vec::new();
        let end = execute_with_script(
            machine,
            &options,
            RunExtras {
                script: None,
                digests: Some(recorder.digests()),
            },
            interrupt,
            &|| false,
            console,
            &mut log,
        );
        let session = recorder.finish(machine, end, None).unwrap();
        (end, session, String::from_utf8(log).unwrap())
    }

    #[test]
    fn a_run_cut_by_ctrl_c_once_it_has_printed_replays_to_its_cut() {
        // 200 ms are 4 million cycles of a CPU at 20 MHz, four slices of the run: Ctrl-C is
        // pressed when the first one relays `ready`, and the second pass never begins.
        let limits = Limits {
            max_time_nanos: Some(200_000_000),
            ..Limits::default()
        };
        let (mut machine, recorder) = recording(echo(), limits);
        machine
            .send_console(ConsoleChannel::Uart0, Cycles(800_000), b"hi\r")
            .unwrap();
        let interrupt = Interrupt::new();
        let (end, session, log) = record_through(
            &mut machine,
            recorder,
            (limits, ConsoleChannel::Uart0),
            &interrupt,
            &mut PressesCtrlC {
                interrupt: &interrupt,
            },
        );
        assert_eq!(end, RunEnd::Interrupted, "{log}");
        let cut = session.limits.cut.expect("a cut");
        assert_eq!(cut.reason, CutReason::Interrupted);
        assert!(
            cut.at > Cycles(0),
            "cut after the firmware printed: {cut:?}"
        );
        assert!(cut.at < Cycles(32_000_000), "cut before the limit: {cut:?}");
        let read = Session::from_bytes(&session.to_bytes().unwrap()).unwrap();
        assert_eq!(replayed(&read), Verdict::Identical);
    }

    #[test]
    fn a_run_whose_console_fails_at_the_final_flush_after_an_idle_end_replays_to_its_cut() {
        // `hello` is written to USB-Serial-JTAG and never committed: it is still in the device
        // when the CPU waits for an interrupt nothing can raise. The end of the run flushes it,
        // and the console fails there.
        let mut program = print_program(USB, b"hello");
        program.extend(stop_watchdogs());
        program.extend([wfi(), HANG]);
        let limits = Limits {
            max_time_nanos: Some(20_000_000),
            ..Limits::default()
        };
        let (mut machine, recorder) = recording(flat_rom(&program), limits);
        let (end, session, log) = record_through(
            &mut machine,
            recorder,
            (limits, ConsoleChannel::UsbSerialJtag),
            &Interrupt::new(),
            &mut Failing,
        );
        assert_eq!(end, RunEnd::ConsoleError, "{log}");
        assert!(
            log.contains("waits for an interrupt and none can come"),
            "the machine stopped on its own before the console failed: {log}"
        );
        assert_eq!(
            session.limits.cut.map(|cut| cut.reason),
            Some(CutReason::ConsoleError)
        );
        let mut machine = Replay::start(&session, flat_rom(&program))
            .unwrap()
            .into_machine();
        let mut digests = ConsoleDigests::default();
        let end = run(
            &mut machine,
            &replay_options(&session, ConsoleChannel::UsbSerialJtag),
            &mut digests,
        );
        assert_eq!(
            verdict(&session, &machine, end, digests, None).unwrap(),
            Verdict::Identical
        );
    }

    #[test]
    fn a_recorded_session_is_written_and_said_with_its_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run\u{1b}.inkrec");
        let limits = Limits {
            max_time_nanos: Some(1_000_000),
            ..Limits::default()
        };
        let (mut machine, recorder) = recording(echo(), limits);
        machine
            .send_console(ConsoleChannel::Uart0, Cycles(0), b"x")
            .unwrap();
        let mut log = Vec::new();
        assert!(record_session(
            recorder,
            &machine,
            RunEnd::TimeLimit,
            None,
            &path,
            &mut log
        ));
        assert_eq!(
            String::from_utf8(log).unwrap(),
            "inkemu: session recorded in run\\u{1b}.inkrec (1 inputs); `inkemu replay` replays it\n"
        );
        let session = crate::session_file::read_session(&path).unwrap();
        assert_eq!(session.inputs.len(), 1);
    }

    #[test]
    fn a_session_that_cannot_be_written_or_cannot_hold_the_run_says_why_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("run.inkrec");
        let (machine, recorder) = recording(echo(), Limits::default());
        let mut log = Vec::new();
        assert!(!record_session(
            recorder,
            &machine,
            RunEnd::Budget,
            None,
            &path,
            &mut log
        ));
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.starts_with("error: cannot write the session ")
                && log.ends_with(
                    "; the run is not recorded: check that the directory exists and is writable\n"
                ),
            "{log}"
        );
        assert_eq!(log.matches("run.inkrec").count(), 1, "the path once: {log}");
        // More chunks on a console than a session holds: the journal is incomplete.
        let (mut machine, recorder) = recording(echo(), Limits::default());
        for _ in 0..=soc_esp32c3::MAX_JOURNAL_CONSOLE_CHUNKS {
            machine
                .send_console(ConsoleChannel::Uart0, Cycles(0), b"")
                .unwrap();
        }
        let path = dir.path().join("run.inkrec");
        let mut log = Vec::new();
        assert!(!record_session(
            recorder,
            &machine,
            RunEnd::Budget,
            None,
            &path,
            &mut log
        ));
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.starts_with("error: the run is not recorded in ") && log.contains("run.inkrec: "),
            "{log}"
        );
        assert!(!path.exists(), "nothing is written");
    }

    #[test]
    fn a_replay_on_another_rom_is_refused_with_the_rom_to_fetch() {
        let session = recorded();
        let other = flat_rom(&[HANG]);
        let Err(message) = start_replay(&session, other, 3) else {
            panic!("another ROM is refused")
        };
        assert!(
            message.starts_with("the mask ROM given has SHA-256 ")
                && message.ends_with(
                    ": this inkemu loads ROM revision 3 for the device, from `inkemu rom fetch --device x4` or --rom-dir; a session recorded on another ROM replays with the inkemu that recorded it"
                ),
            "{message}"
        );
        start_replay(&session, echo(), 3).unwrap();
    }

    #[test]
    fn a_replay_says_how_it_compares_and_exits_with_its_code() {
        let session = recorded();
        let report = |session: &Session, end: RunEnd, trace_hash: bool| {
            let mut machine = if trace_hash {
                Replay::start(session, echo()).unwrap().into_machine()
            } else {
                boards::builtin("x4")
                    .unwrap()
                    .instantiate(echo(), MachineOptions::default())
            };
            let mut digests = ConsoleDigests::default();
            if end == RunEnd::TimeLimit {
                let ran = run(
                    &mut machine,
                    &replay_options(session, ConsoleChannel::Uart0),
                    &mut digests,
                );
                assert_eq!(ran, end);
            }
            let mut log = Vec::new();
            let code = report_replay(session, &machine, end, digests, None, &mut log);
            (code, String::from_utf8(log).unwrap())
        };
        let (code, log) = report(&session, RunEnd::TimeLimit, true);
        assert_eq!(code, EXIT_OK, "{log}");
        assert!(
            log.starts_with("inkemu: the replay gives what the session recorded: "),
            "{log}"
        );
        let mut changed = session.clone();
        changed.outcome.instructions += 1;
        let (code, log) = report(&changed, RunEnd::TimeLimit, true);
        assert_eq!(code, EXIT_DIVERGED, "{log}");
        assert!(
            log.starts_with("inkemu: the replay diverges from the session: the instructions: "),
            "{log}"
        );
        let (code, log) = report(&session, RunEnd::ConsoleClosed, true);
        assert_eq!(code, EXIT_ERROR);
        assert_eq!(
            log,
            "error: the replay was cut by the reader of the console going away before the end of its session: nothing was compared; let it run to its end\n"
        );
        let (code, log) = report(&session, RunEnd::Budget, false);
        assert_eq!(code, EXIT_ERROR);
        assert!(
            log.starts_with("error: the machine keeps no trace hash"),
            "{log}"
        );
        let mut cut = session;
        cut.limits.cut = Some(Cut {
            at: Cycles(160_000),
            reason: CutReason::Interrupted,
        });
        let (_, log) = report(&cut, RunEnd::Interrupted, true);
        assert!(
            log.starts_with(
                "inkemu: the session was cut by Ctrl-C at 1000 us of device time when it was recorded; its replay stops there\n"
            ),
            "{log}"
        );
    }

    /// Replaces `from` by `to`, of the same length, in `bytes`, once.
    fn replaced(bytes: &mut [u8], from: &[u8], to: &[u8]) {
        assert_eq!(from.len(), to.len());
        let at = bytes
            .windows(from.len())
            .position(|window| window == from)
            .expect("in the bytes");
        bytes[at..at + to.len()].copy_from_slice(to);
    }

    #[test]
    fn a_session_whose_profile_holds_control_characters_is_refused_with_them_escaped() {
        // A file inkemu did not write: its profile text holds a raw ESC, which TOML refuses and
        // whose line the refusal quotes. The text carries its own SHA-256, which is redone.
        use sha2::{Digest as _, Sha256};
        let session = recorded();
        let mut bytes = session.to_bytes().unwrap();
        let text = session.profile.text.clone();
        let hostile = text.replacen("name = \"Xteink X4\"", "name = \"\u{1b}[2Jnk X4\"", 1);
        assert_ne!(hostile, text, "the name was changed");
        replaced(&mut bytes, text.as_bytes(), hostile.as_bytes());
        replaced(
            &mut bytes,
            &Sha256::digest(text.as_bytes()),
            &Sha256::digest(hostile.as_bytes()),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hostile.inkrec");
        std::fs::write(&path, &bytes).unwrap();
        let error = crate::session_file::read_session(&path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is refused: "), "{error}");
        assert!(!error.contains('\u{1b}'), "{error:?}");
        assert!(error.contains("\\u{1b}[2Jnk X4"), "{error}");
    }

    #[test]
    fn a_replay_the_host_cuts_compares_nothing() {
        let session = recorded();
        let machine = Replay::start(&session, echo()).unwrap().into_machine();
        for (end, reason) in [
            (RunEnd::Interrupted, CutReason::Interrupted),
            (RunEnd::ConsoleClosed, CutReason::ReaderGone),
            (RunEnd::ConsoleError, CutReason::ConsoleError),
        ] {
            assert_eq!(
                verdict(&session, &machine, end, ConsoleDigests::default(), None).unwrap(),
                Verdict::Cut(reason)
            );
        }
    }
}
