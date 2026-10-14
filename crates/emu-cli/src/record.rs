//! `inkemu run --record` and `inkemu replay` around [`crate::run::execute_with_script`]: what a
//! run keeps for its session, and the verdict of a replay. The session itself, its format and
//! its replay belong to the `session` crate; this module joins them to the run of the command
//! line, which shows a replay as it shows a run.
use crate::run::{RunEnd, RunOptions, session_end};
use session::{
    ConsoleDigests, Cut, CutReason, Divergence, End, Limits, NoTraceHash, Outcome, ProfileText,
    RomIdentity, Session, WriteError,
};
use soc_esp32c3::{ConsoleChannel, Esp32C3};

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
    limits: Limits,
    digests: ConsoleDigests,
}

impl Recorder {
    /// Begins the record of a run of `machine`, at power-on, which runs on the ROM `rom`, the
    /// profile `profile` and with `limits` (no cut yet).
    #[must_use]
    pub fn begin(
        rom: RomIdentity,
        profile: ProfileText,
        machine: &Esp32C3,
        limits: Limits,
    ) -> Recorder {
        Recorder {
            rom,
            profile,
            flash: machine.flash_contents(),
            limits,
            digests: ConsoleDigests::default(),
        }
    }

    /// The digests the run takes its consoles into.
    pub const fn digests(&mut self) -> &mut ConsoleDigests {
        &mut self.digests
    }

    /// The session of `machine`, whose run ended as `end`: a run the host cut is recorded with
    /// the device time it stopped at (G5).
    ///
    /// # Errors
    /// [`RecordError`] when the session cannot hold the run, or the machine keeps no trace hash.
    pub fn finish(self, machine: &Esp32C3, end: RunEnd) -> Result<Session, RecordError> {
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
        let outcome = Outcome::of(machine, end, self.digests)?;
        Ok(Session::record(
            self.rom,
            self.profile,
            self.flash,
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
/// into `digests` to the last byte.
///
/// # Errors
/// [`NoTraceHash`] when the machine keeps no trace hash.
pub fn verdict(
    session: &Session,
    machine: &Esp32C3,
    end: RunEnd,
    digests: ConsoleDigests,
) -> Result<Verdict, NoTraceHash> {
    let reached = match session_end(end) {
        Ok(reached) => reached,
        Err(reason) => return Ok(Verdict::Cut(reason)),
    };
    let end = session.limits.end_of(reached, machine.device_time());
    let replayed = Outcome::of(machine, end, digests)?;
    let differences = session.outcome.differences(&replayed);
    Ok(if differences.is_empty() {
        Verdict::Identical
    } else {
        Verdict::Diverged(Divergence(differences))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Interrupt, RunExtras, execute_with_script};
    use boards::MachineOptions;
    use cpu_rv32::asm::{beq, jal, lbu, lw, sw};
    use emu_core::Cycles;
    use session::{Quantity, Replay};
    use soc_esp32c3::test_support::{T0, T1, flat_rom, print_program};
    use soc_esp32c3::{RomImage, UsbHost};

    const UART0: u32 = 0x6000_0000;

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
        let mut recorder = Recorder::begin(identity, x4_text(), &machine, limits);
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
        recorder.finish(&machine, end).unwrap()
    }

    fn replayed(session: &Session) -> Verdict {
        let mut machine = Replay::start(session, echo()).unwrap().into_machine();
        let mut digests = ConsoleDigests::default();
        let end = run(
            &mut machine,
            &replay_options(session, ConsoleChannel::UsbSerialJtag),
            &mut digests,
        );
        verdict(session, &machine, end, digests).unwrap()
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
        let mut recorder = Recorder::begin(identity, x4_text(), &machine, Limits::default());
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
        let session = recorder.finish(&machine, end).unwrap();
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
                verdict(&session, &machine, end, ConsoleDigests::default()).unwrap(),
                Verdict::Cut(reason)
            );
        }
    }
}
