//! The replay of a session on a machine (G2, G6, G7).
//!
//! The machine is rebuilt from the session alone: the profile from its text, the flash from its
//! content, the options and the USB host at power-on as they were. Every input is handed to it
//! at once, at power-on, in the order the recorded machine accepted them: a run depends on the
//! date, the bytes and the order of its inputs, not on the instant a front end hands them over
//! (ADR 0006, [`Esp32C3::send_console`]), and a session holds no more input than the machine
//! takes ahead ([`crate::MAX_CONSOLE_BYTES`], [`crate::MAX_HOST_CHANGES`]). The replay then runs
//! to the limits of the session, its cut included, and gives the [`Outcome`] its recording gave.
//!
//! The machine of a replay keeps no journal of its inputs ([`Esp32C3::forgo_input_journal`]):
//! the session holds them, and the machine's consoles hold what is still to reach the device, so
//! the inputs are in memory twice at most, never three times.
use crate::format::Session;
use crate::outcome::{ConsoleDigests, End, Limits, NoTraceHash, Outcome, Reached, hex};
use boards::{MachineOptions, ProfileError};
use emu_core::Cycles;
use sha2::{Digest as _, Sha256};
use soc_esp32c3::{
    AcceptedInput, ConsoleChannel, Esp32C3, FlashImageError, InputError, RomImage, StopReason,
};

/// Why a session cannot be replayed, before its first instruction.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    /// The ROM given is not the one the session was recorded on.
    #[error(
        "the mask ROM given has SHA-256 {found}, and the session was recorded on the ROM of revision {revision} whose SHA-256 is {recorded}; give that ROM"
    )]
    Rom {
        revision: u16,
        recorded: String,
        found: String,
    },
    #[error("{source}")]
    Profile { source: ProfileError },
    #[error("{source}")]
    Flash { source: FlashImageError },
    /// The machine refused an input of the session, which the recorded machine had accepted:
    /// the file was not written by inkemu, or was changed.
    #[error("the machine refuses input {index} of the session: {source}")]
    Input { index: usize, source: InputError },
    /// [`Replay::finish`] was called before the replay reached an end.
    #[error("the replay has not reached the end of its session; step it on")]
    Unfinished,
    #[error("{0}")]
    NoTraceHash(#[from] NoTraceHash),
}

/// What one call of a replay did: what each console sent, and the end when it was reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The bytes each console sent in the call, in [`ConsoleChannel::ALL`] order.
    pub taken: Vec<(ConsoleChannel, Vec<u8>)>,
    pub end: Option<End>,
}

/// A session being replayed.
#[derive(Debug)]
pub struct Replay {
    machine: Esp32C3,
    limits: Limits,
    consoles: ConsoleDigests,
    end: Option<End>,
}

impl Replay {
    /// The machine of `session` on `rom`, at power-on, every input handed over, keeping no
    /// journal of them.
    ///
    /// # Errors
    /// [`ReplayError::Rom`] when `rom` is not the ROM of the session, [`ReplayError::Profile`]
    /// or [`ReplayError::Flash`] when the device cannot be rebuilt, [`ReplayError::Input`] when
    /// the machine refuses an input.
    pub fn start(session: &Session, rom: RomImage) -> Result<Replay, ReplayError> {
        let found: [u8; 32] = Sha256::digest(rom.bytes()).into();
        if found != session.rom.sha256 {
            return Err(ReplayError::Rom {
                revision: session.rom.revision,
                recorded: hex(&session.rom.sha256),
                found: hex(&found),
            });
        }
        let device = session
            .profile
            .device()
            .map_err(|source| ReplayError::Profile { source })?;
        let options = MachineOptions {
            strict_mmio: session.limits.strict_mmio,
            // The outcome holds the trace hash, which changes nothing the machine does.
            trace_hash: true,
            usb_host: session.usb_host,
        };
        let mut machine = device
            .instantiate_with_flash(rom, session.flash.clone(), options)
            .map_err(|source| ReplayError::Flash { source })?;
        machine.forgo_input_journal();
        for (index, input) in session.inputs.iter().enumerate() {
            let handed = match input {
                AcceptedInput::Console { channel, at, bytes } => {
                    machine.send_console(*channel, *at, bytes)
                }
                AcceptedInput::UsbHost { at, state } => machine.set_usb_host(*at, *state),
                AcceptedInput::Board { at, input } => machine.send_board_input(*at, *input),
            };
            handed.map_err(|source| ReplayError::Input { index, source })?;
        }
        Ok(Replay {
            machine,
            limits: session.limits,
            consoles: ConsoleDigests::default(),
            end: None,
        })
    }

    /// The machine being replayed: its diagnostics, its state, for a front end to show.
    #[must_use]
    pub const fn machine(&self) -> &Esp32C3 {
        &self.machine
    }

    /// The machine being replayed, for what a front end takes from it
    /// ([`Esp32C3::drain_diagnostics`]). Handing it inputs makes the replay another run.
    pub const fn machine_mut(&mut self) -> &mut Esp32C3 {
        &mut self.machine
    }

    /// The machine, for a front end that drives it with its own loop (the command line, which
    /// shows the replay as it shows a run): it runs with [`Limits::run_limits`], tells the end
    /// with [`Limits::end_of`] and gives the outcome with [`Outcome::of`].
    #[must_use]
    pub fn into_machine(self) -> Esp32C3 {
        self.machine
    }

    /// The end the replay reached, if it has.
    #[must_use]
    pub const fn end(&self) -> Option<End> {
        self.end
    }

    /// Runs at most `slice` CPU cycles (1 at least) towards the end of the session, and gives
    /// what each console sent and the end when it is reached; after the end, nothing more runs.
    /// The outcome is the same however the replay is cut into steps.
    pub fn step(&mut self, slice: u64) -> Step {
        if self.end.is_some() {
            return Step {
                taken: Vec::new(),
                end: self.end,
            };
        }
        let (max_cycles, max_nanos) = self.limits.run_limits();
        let next = self.machine.cycles().get().saturating_add(slice.max(1));
        let limit = max_cycles.map_or(next, |max| max.min(next));
        let stop = self.machine.run_within(Cycles(limit), max_nanos);
        let taken: Vec<(ConsoleChannel, Vec<u8>)> = ConsoleChannel::ALL
            .into_iter()
            .map(|channel| {
                let bytes = self.machine.take_console(channel);
                self.consoles.add(channel, &bytes);
                (channel, bytes)
            })
            .collect();
        let reached = match stop {
            StopReason::CycleBudget
                if max_cycles.is_some_and(|max| self.machine.cycles().get() >= max) =>
            {
                Some(Reached::CycleLimit)
            }
            StopReason::TimeBudget => Some(Reached::TimeLimit),
            StopReason::Idle => Some(Reached::Idle),
            StopReason::DeepSleep => Some(Reached::DeepSleep),
            StopReason::StrictMmio(_) => Some(Reached::StrictMmio),
            StopReason::PowerOff => Some(Reached::PowerOff),
            StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => None,
        };
        self.end = reached.map(|reached| self.limits.end_of(reached, self.machine.device_time()));
        Step {
            taken,
            end: self.end,
        }
    }

    /// The outcome of the replay, once it reached its end: what the consoles still held is
    /// taken too, as the end of a run takes it.
    ///
    /// # Errors
    /// [`ReplayError::Unfinished`] before the end.
    pub fn finish(mut self) -> Result<Outcome, ReplayError> {
        let end = self.end.ok_or(ReplayError::Unfinished)?;
        for channel in ConsoleChannel::ALL {
            let flushed = self.machine.flush_console(channel);
            self.consoles.add(channel, &flushed);
        }
        Ok(Outcome::of(&self.machine, end, self.consoles)?)
    }
}
