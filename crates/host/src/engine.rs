//! One life of a machine, from power-on to its end, without host I/O (design of sub-project 2c,
//! C3, C5, C7, C9, C10).
//!
//! The engine runs the machine in passes and steps, takes the inputs of a user at the present
//! instant of the device, reads the consoles and the diagnostics after each call, polls the panel,
//! and keeps what a session needs. It reads no file, no clock and starts no thread: the thread of
//! [`crate::device`] gives it the bound of each pass (its pace) and writes what it gives back. The
//! web version (sub-project 6) can put it in a Web Worker as it is.
//!
//! What ends a run of the command line does not always end a life here: a CPU that waits for an
//! interrupt no line can raise, and a deep sleep nothing modelled wakes, **halt** the machine
//! until an input arrives, since a user can still type or press (C5). A replay takes no input, and
//! ends on them as the command line does.
use crate::ports::{
    ButtonError, ButtonPort, ButtonSpec, Life, PanelDescription, PanelFrame, PanelSource,
};
use crate::record::{RecordError, Recorder, Verdict, verdict};
use crate::run::{self, RunEnd, SLICE_CYCLES};
use crate::utf8::Utf8Stream;
use emu_core::Cycles;
use session::{ConsoleDigests, NoTraceHash, Session};
use soc_esp32c3::{ConsoleChannel, DeepSleep, Esp32C3, InputError, StopReason, UsbHost};
use std::sync::Arc;

/// The limits of a run: what ends it on its own besides the firmware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// The CPU cycle counter value that ends the run.
    pub max_cycles: Option<u64>,
    /// Nanoseconds of device time since power-on that end the run.
    pub max_device_nanos: Option<u64>,
}

/// What a life keeps for a session.
#[derive(Debug)]
pub enum Mode {
    /// A run of the user, recorded or not.
    Run { recorder: Option<Recorder> },
    /// The replay of a session: no input, and a verdict at the end.
    Replay {
        session: Box<Session>,
        digests: ConsoleDigests,
    },
}

/// Why a machine waits for an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Halt {
    /// The CPU waits for an interrupt no line can raise and no device has anything scheduled.
    Idle,
    /// The chip is in a deep sleep nothing modelled wakes.
    DeepSleep,
}

/// Where a life is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It runs when asked to.
    Live,
    /// It waits for an input or a reset.
    Halted(Halt),
    /// It reached an end of its own: a limit, a strict stop, or, for a replay, a wait.
    Ended(RunEnd),
}

/// How much a notice matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// A line of the journal of a run: what the command line prints on its standard error.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub level: Level,
    pub text: String,
}

impl Notice {
    /// The lines of a log the command line's policy wrote, each with the level its prefix says:
    /// `error:` and `warning:` lines are what they say, every other line is information.
    #[must_use]
    pub fn from_log(bytes: &[u8]) -> Vec<Notice> {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| Notice {
                level: if line.starts_with("error:") {
                    Level::Error
                } else if line.starts_with("warning:") {
                    Level::Warning
                } else {
                    Level::Info
                },
                text: line.to_owned(),
            })
            .collect()
    }

    fn info(text: String) -> Notice {
        Notice {
            level: Level::Info,
            text,
        }
    }
}

/// What a call of the engine produced.
#[derive(Debug, Default)]
pub struct Output {
    /// The text each console gave, in the order of [`ConsoleChannel::ALL`]; only the ones that
    /// gave something.
    pub console: Vec<(ConsoleChannel, String)>,
    pub notices: Vec<Notice>,
    /// The panel completed a refresh.
    pub frame: Option<Arc<PanelFrame>>,
    /// The pass stopped on its pace: the machine is where the host clock allows it to be.
    pub caught_up: bool,
}

/// Why the engine did not do what it was asked.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("a replay takes no input")]
    Replaying,
    #[error("the run has ended; reset the device to run it again")]
    Ended,
    #[error("the machine waits for an input: there is nothing to step until one arrives")]
    NothingToStep,
    #[error("{0}")]
    Input(#[from] InputError),
    #[error("{0}")]
    Button(#[from] ButtonError),
}

/// The deep sleep of a machine, as an interface shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSleepStatus {
    pub since_nanos: u64,
    pub timer_wake_nanos: Option<u64>,
    /// `WAKEUP_ENA`: bit 3 the RTC timer, bit 2 GPIO.
    pub wake_sources: u32,
}

impl From<DeepSleep> for DeepSleepStatus {
    fn from(sleep: DeepSleep) -> DeepSleepStatus {
        DeepSleepStatus {
            since_nanos: sleep.since_nanos,
            timer_wake_nanos: sleep.timer_wake_nanos,
            wake_sources: sleep.wake_sources,
        }
    }
}

/// What the machine is at, for an interface.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineStatus {
    pub device_time_nanos: u64,
    pub cycles: u64,
    pub instructions: u64,
    pub pc: u32,
    pub cpu_hz: u64,
    /// The CPU sleeps in `wfi`, waiting for an interrupt.
    pub cpu_asleep: bool,
    pub deep_sleep: Option<DeepSleepStatus>,
    /// `absent`, `attached` or `open`.
    pub usb_host: &'static str,
    /// The changes of the USB host given and still to come.
    pub usb_host_changes_pending: usize,
    /// Whether the journal of a recorded run still holds every input.
    pub journal_complete: bool,
}

/// The end of a life: its machine, what its consoles still held, its summary, and what a session
/// makes of it.
#[derive(Debug)]
pub struct LifeEnd {
    pub machine: Esp32C3,
    pub end: RunEnd,
    pub output: Output,
    /// The verdict of a replay.
    pub verdict: Option<Result<Verdict, NoTraceHash>>,
    /// The session of a recorded run.
    pub session: Option<Result<Session, RecordError>>,
}

/// One life of a machine.
pub struct Engine {
    machine: Esp32C3,
    panel: Box<dyn PanelSource>,
    buttons: Box<dyn ButtonPort>,
    description: PanelDescription,
    mode: Mode,
    limits: Limits,
    state: State,
    decoders: [Utf8Stream; 3],
    frame: Option<Arc<PanelFrame>>,
    /// The journal was said to hold no more inputs.
    journal_said_incomplete: bool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("state", &self.state)
            .field("limits", &self.limits)
            .field("panel", &self.description)
            .finish_non_exhaustive()
    }
}

/// The place of `channel` in [`ConsoleChannel::ALL`].
fn place(channel: ConsoleChannel) -> usize {
    ConsoleChannel::ALL
        .iter()
        .position(|one| *one == channel)
        .unwrap_or(0)
}

impl Engine {
    /// The life of `life`, at power-on.
    #[must_use]
    pub fn new(life: Life, mode: Mode, limits: Limits) -> Engine {
        let Life {
            machine,
            panel,
            buttons,
        } = life;
        let description = panel.describe();
        Engine {
            machine,
            panel,
            buttons,
            description,
            mode,
            limits,
            state: State::Live,
            decoders: Default::default(),
            frame: None,
            journal_said_incomplete: false,
        }
    }

    /// The first image of the panel, at power-on.
    pub fn begin(&mut self) -> Output {
        let mut output = Output::default();
        self.poll_panel(&mut output);
        output
    }

    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    #[must_use]
    pub const fn machine(&self) -> &Esp32C3 {
        &self.machine
    }

    #[must_use]
    pub const fn panel(&self) -> PanelDescription {
        self.description
    }

    #[must_use]
    pub fn buttons(&self) -> &[ButtonSpec] {
        self.buttons.buttons()
    }

    /// Whether pressing a button changes what the firmware sees.
    #[must_use]
    pub fn buttons_reach_firmware(&self) -> bool {
        self.buttons.reaches_firmware()
    }

    /// The image of the last refresh of the panel.
    #[must_use]
    pub fn frame(&self) -> Option<Arc<PanelFrame>> {
        self.frame.clone()
    }

    #[must_use]
    pub const fn is_replay(&self) -> bool {
        matches!(self.mode, Mode::Replay { .. })
    }

    #[must_use]
    pub const fn is_recorded(&self) -> bool {
        matches!(self.mode, Mode::Run { recorder: Some(_) })
    }

    fn digests(&mut self) -> Option<&mut ConsoleDigests> {
        match &mut self.mode {
            Mode::Run {
                recorder: Some(recorder),
            } => Some(recorder.digests()),
            Mode::Run { recorder: None } => None,
            Mode::Replay { digests, .. } => Some(digests),
        }
    }

    /// Takes every console, into the digests of a session and into text.
    fn take_consoles(&mut self, output: &mut Output, flush: bool) {
        for channel in ConsoleChannel::ALL {
            let bytes = if flush {
                self.machine.flush_console(channel)
            } else {
                self.machine.take_console(channel)
            };
            if let Some(digests) = self.digests() {
                digests.add(channel, &bytes);
            }
            let decoder = &mut self.decoders[place(channel)];
            let mut text = decoder.push(&bytes);
            if flush {
                text.push_str(&decoder.finish());
            }
            if !text.is_empty() {
                output.console.push((channel, text));
            }
        }
    }

    /// Reads what the machine said since the last call: the diagnostics (but `violation`, which
    /// the end of the run states), and whether the journal of a recorded run just stopped holding
    /// every input.
    fn take_diagnostics(
        &mut self,
        output: &mut Output,
        violation: Option<&soc_esp32c3::Diagnostic>,
    ) {
        for diagnostic in self.machine.drain_diagnostics() {
            if Some(&diagnostic) == violation {
                continue;
            }
            let prefix = run::journal_prefix(&diagnostic);
            output.notices.push(Notice {
                level: if prefix == "warning" {
                    Level::Warning
                } else {
                    Level::Info
                },
                text: format!("{prefix}: {diagnostic}"),
            });
        }
        if self.is_recorded() && !self.journal_said_incomplete {
            let mut log = Vec::new();
            self.journal_said_incomplete = run::say_an_incomplete_journal(&self.machine, &mut log);
            output.notices.extend(Notice::from_log(&log));
        }
    }

    /// Takes the image of a refresh that completed. An image the port could not make is a fault
    /// of the port, said in the journal; the panel keeps its last image.
    fn poll_panel(&mut self, output: &mut Output) {
        match self.panel.poll(&mut self.machine) {
            Ok(Some(frame)) => {
                let frame = Arc::new(frame);
                self.frame = Some(Arc::clone(&frame));
                output.frame = Some(frame);
            }
            Ok(None) => {}
            Err(e) => output.notices.push(Notice {
                level: Level::Error,
                text: format!("error: the panel gave an image it cannot show: {e}"),
            }),
        }
    }

    /// Reads everything after a call of the machine.
    fn collect(&mut self, output: &mut Output, violation: Option<&soc_esp32c3::Diagnostic>) {
        self.take_consoles(output, false);
        self.take_diagnostics(output, violation);
        self.poll_panel(output);
    }

    /// Ends the run on `end`, with the line the command line says.
    fn end_on(&mut self, end: RunEnd, stop: Option<&StopReason>, output: &mut Output) {
        let mut log = Vec::new();
        run::say_why(&mut log, end, stop);
        output.notices.extend(Notice::from_log(&log));
        self.state = State::Ended(end);
    }

    /// Halts on `halt`, or ends a replay there.
    fn halt_on(&mut self, halt: Halt, stop: &StopReason, output: &mut Output) {
        if self.is_replay() {
            let end = match halt {
                Halt::Idle => RunEnd::Idle,
                Halt::DeepSleep => RunEnd::DeepSleep,
            };
            self.end_on(end, Some(stop), output);
            return;
        }
        let text = match halt {
            Halt::Idle => "inkemu: the CPU waits for an interrupt and none can come: the device waits for an input (a console, the USB host, a button) or a reset".to_owned(),
            Halt::DeepSleep => match self.machine.deep_sleep() {
                Some(sleep) => format!(
                    "{}; the device waits for an input or a reset",
                    run::deep_sleep_line(&sleep)
                ),
                None => "inkemu: the device is in deep sleep; it waits for an input or a reset"
                    .to_owned(),
            },
        };
        output.notices.push(Notice::info(text));
        self.state = State::Halted(halt);
    }

    /// What `stop` does to the state, after a pass (`paced`: its device-time bound was the pace,
    /// not the limit) or a step.
    fn after(&mut self, stop: &StopReason, paced: bool, output: &mut Output) {
        match stop {
            StopReason::StrictMmio(_) => self.end_on(RunEnd::StrictMmio, Some(stop), output),
            StopReason::Idle => self.halt_on(Halt::Idle, stop, output),
            StopReason::DeepSleep => self.halt_on(Halt::DeepSleep, stop, output),
            StopReason::PowerOff => self.end_on(RunEnd::PowerOff, Some(stop), output),
            StopReason::TimeBudget if paced => output.caught_up = true,
            StopReason::TimeBudget => self.end_on(RunEnd::TimeLimit, Some(stop), output),
            StopReason::CycleBudget
                if self
                    .limits
                    .max_cycles
                    .is_some_and(|max| self.machine.cycles().get() >= max) =>
            {
                self.end_on(RunEnd::Budget, Some(stop), output);
            }
            StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => {}
        }
    }

    /// Runs one pass: at most [`SLICE_CYCLES`] cycles, to the cycle limit, and to `pace` or the
    /// device-time limit, whichever comes first (nanoseconds of device time since power-on). A
    /// machine that is not live does nothing.
    pub fn pass(&mut self, pace: Option<u64>) -> Output {
        let mut output = Output::default();
        if self.state != State::Live {
            return output;
        }
        let now = self.machine.cycles().get();
        let slice_end = now.saturating_add(SLICE_CYCLES);
        let limit = self
            .limits
            .max_cycles
            .map_or(slice_end, |max| max.min(slice_end));
        let (deadline, paced) = match (pace, self.limits.max_device_nanos) {
            (Some(pace), Some(max)) if pace < max => (Some(pace), true),
            (Some(pace), None) => (Some(pace), true),
            (_, max) => (max, false),
        };
        let stop = self.machine.run_within(Cycles(limit), deadline);
        let violation = match &stop {
            StopReason::StrictMmio(diagnostic) => Some(diagnostic.clone()),
            _ => None,
        };
        self.collect(&mut output, violation.as_ref());
        self.after(&stop, paced, &mut output);
        output
    }

    /// Executes one instruction and goes to the boundary of the next ([`Esp32C3::step`]).
    ///
    /// # Errors
    /// [`EngineError::Ended`] after the end of the run, [`EngineError::NothingToStep`] while the
    /// machine waits for an input.
    pub fn step(&mut self) -> Result<Output, EngineError> {
        match self.state {
            State::Ended(_) => return Err(EngineError::Ended),
            State::Halted(_) => return Err(EngineError::NothingToStep),
            State::Live => {}
        }
        let mut output = Output::default();
        let stepped = self.machine.step();
        let violation = match &stepped.stop {
            Some(StopReason::StrictMmio(diagnostic)) => Some(diagnostic.clone()),
            _ => None,
        };
        self.collect(&mut output, violation.as_ref());
        if let Some(stop) = &stepped.stop {
            self.after(stop, false, &mut output);
        }
        if self.state == State::Live {
            if self
                .limits
                .max_cycles
                .is_some_and(|max| self.machine.cycles().get() >= max)
            {
                self.end_on(RunEnd::Budget, None, &mut output);
            } else if self
                .limits
                .max_device_nanos
                .is_some_and(|max| self.machine.device_time_nanos() >= max)
            {
                self.end_on(RunEnd::TimeLimit, None, &mut output);
            }
        }
        Ok(output)
    }

    /// Refuses an input the life cannot take.
    const fn accepts_input(&self) -> Result<(), EngineError> {
        if self.is_replay() {
            return Err(EngineError::Replaying);
        }
        if let State::Ended(_) = self.state {
            return Err(EngineError::Ended);
        }
        Ok(())
    }

    /// After an accepted input: a halted machine goes on, and the journal and the panel are
    /// read.
    fn after_input(&mut self) -> Output {
        if let State::Halted(_) = self.state {
            self.state = State::Live;
        }
        let mut output = Output::default();
        self.take_diagnostics(&mut output, None);
        self.poll_panel(&mut output);
        output
    }

    /// Sends `bytes` to `channel` at the present instant of the device, as a host does.
    ///
    /// # Errors
    /// [`EngineError::Replaying`], [`EngineError::Ended`], or the refusal of the machine.
    pub fn send_console(
        &mut self,
        channel: ConsoleChannel,
        bytes: &[u8],
    ) -> Result<Output, EngineError> {
        self.accepts_input()?;
        let now = self.machine.device_time();
        self.machine.send_console(channel, now, bytes)?;
        Ok(self.after_input())
    }

    /// Changes the USB host at the present instant of the device.
    ///
    /// # Errors
    /// [`EngineError::Replaying`], [`EngineError::Ended`], or the refusal of the machine.
    pub fn set_usb_host(&mut self, host: UsbHost) -> Result<Output, EngineError> {
        self.accepts_input()?;
        let now = self.machine.device_time();
        self.machine.set_usb_host(now, host)?;
        Ok(self.after_input())
    }

    /// Presses or releases the button `id` at the present instant of the device.
    ///
    /// # Errors
    /// [`ButtonError::Replaying`] during a replay, [`ButtonError::NotInSession`] for a button
    /// that reaches the firmware while the life is recorded, [`EngineError::Ended`], or the
    /// refusal of the port.
    pub fn button(&mut self, id: &str, pressed: bool) -> Result<Output, EngineError> {
        if self.is_replay() {
            return Err(ButtonError::Replaying.into());
        }
        let reaches = self.buttons.reaches_firmware();
        if reaches && self.is_recorded() {
            return Err(ButtonError::NotInSession { id: id.to_owned() }.into());
        }
        if let State::Ended(_) = self.state {
            return Err(EngineError::Ended);
        }
        self.buttons.set(&mut self.machine, id, pressed)?;
        if !reaches {
            // The firmware saw nothing: a halted machine has no reason to go on.
            let mut output = Output::default();
            self.poll_panel(&mut output);
            return Ok(output);
        }
        Ok(self.after_input())
    }

    /// What the machine is at.
    pub fn status(&mut self) -> MachineStatus {
        let journal = self.machine.input_journal();
        let journal_complete = journal.is_complete() || journal.is_forgone();
        MachineStatus {
            device_time_nanos: self.machine.device_time_nanos(),
            cycles: self.machine.cycles().get(),
            instructions: self.machine.instructions(),
            pc: self.machine.pc(),
            cpu_hz: self.machine.cpu_frequency().hz(),
            cpu_asleep: self.machine.is_asleep(),
            deep_sleep: self.machine.deep_sleep().map(DeepSleepStatus::from),
            usb_host: self.machine.usb_host().name(),
            usb_host_changes_pending: self.machine.usb_host_changes_pending(),
            journal_complete,
        }
    }

    /// The end that stopping this life now gives a session: the end it reached, the wait it is
    /// in, or a cut of the host.
    #[must_use]
    pub const fn end_now(&self) -> RunEnd {
        match self.state {
            State::Ended(end) => end,
            State::Halted(Halt::Idle) => RunEnd::Idle,
            State::Halted(Halt::DeepSleep) => RunEnd::DeepSleep,
            State::Live => RunEnd::Interrupted,
        }
    }

    /// Ends the life: flushes what the consoles still held, writes its summary as the command
    /// line does, and gives the verdict of a replay or the session of a recorded run.
    #[must_use]
    pub fn end(mut self) -> LifeEnd {
        let finished = self.finish();
        LifeEnd {
            machine: self.machine,
            end: finished.end,
            output: finished.output,
            verdict: finished.verdict,
            session: finished.session,
        }
    }

    /// [`Engine::end`], the machine left in the engine ([`Engine::machine`]): what holds the
    /// flash stays where a caller that must keep it can read it, whatever fails after. The
    /// engine is then done: it is not to run again.
    pub fn finish(&mut self) -> Finished {
        let end = self.end_now();
        let mut output = Output::default();
        self.take_consoles(&mut output, true);
        self.take_diagnostics(&mut output, None);
        let mut log = Vec::new();
        run::say_where_the_run_stopped(&mut self.machine, None, &mut log);
        output.notices.extend(Notice::from_log(&log));
        let mode = std::mem::replace(&mut self.mode, Mode::Run { recorder: None });
        let (verdict, session) = match mode {
            Mode::Replay { session, digests } => (
                Some(verdict(&session, &self.machine, end, digests, None)),
                None,
            ),
            Mode::Run {
                recorder: Some(recorder),
            } => (None, Some(recorder.finish(&self.machine, end, None))),
            Mode::Run { recorder: None } => (None, None),
        };
        Finished {
            end,
            output,
            verdict,
            session,
        }
    }
}

/// The end of a life without its machine ([`Engine::finish`]).
#[derive(Debug)]
pub struct Finished {
    pub end: RunEnd,
    pub output: Output,
    /// The verdict of a replay.
    pub verdict: Option<Result<Verdict, NoTraceHash>>,
    /// The session of a recorded run.
    pub session: Option<Result<Session, RecordError>>,
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::pattern;
    use crate::pattern::fixtures::{BUTTONS, PANEL};
    use boards::MachineOptions;
    use cpu_rv32::asm::{beq, jal, lbu, lw, sw, wfi};
    use soc_esp32c3::test_support::{HANG, T0, T1, flat_rom, print_program, stop_watchdogs};
    use soc_esp32c3::{RomImage, UsbHost};

    pub(crate) const UART0: u32 = 0x6000_0000;

    /// Prints `ready` on UART0, then echoes what UART0 receives.
    pub(crate) fn echo() -> RomImage {
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

    /// Prints `zz` on UART0 with its watchdogs stopped, then waits for an interrupt that no line
    /// can raise.
    pub(crate) fn idle() -> RomImage {
        let mut program = stop_watchdogs();
        program.extend(print_program(UART0, b"zz"));
        program.extend([wfi(), HANG]);
        flat_rom(&program)
    }

    pub(crate) fn options(recorded: bool) -> MachineOptions {
        MachineOptions {
            strict_mmio: false,
            trace_hash: recorded,
            usb_host: UsbHost::Open,
        }
    }

    /// A life of the X4 on `rom`, with the pattern's ports.
    pub(crate) fn life(rom: RomImage, recorded: bool) -> Life {
        let machine = boards::builtin("x4")
            .unwrap()
            .instantiate(rom, options(recorded));
        let (panel, buttons) = pattern::ports(PANEL, &BUTTONS);
        Life {
            machine,
            panel,
            buttons,
        }
    }

    /// The text `output` gave on `channel`.
    pub(crate) fn text(output: &Output, channel: ConsoleChannel) -> String {
        output
            .console
            .iter()
            .filter(|(one, _)| *one == channel)
            .map(|(_, text)| text.as_str())
            .collect()
    }

    /// Passes of at most a millisecond of device time each until `channel` has shown `wanted`, at
    /// most `passes` of them; the text shown.
    pub(crate) fn run_until(
        engine: &mut Engine,
        channel: ConsoleChannel,
        wanted: &str,
        passes: usize,
    ) -> String {
        let mut shown = String::new();
        for _ in 0..passes {
            let pace = engine.status().device_time_nanos + 1_000_000;
            shown += &text(&engine.pass(Some(pace)), channel);
            if shown.contains(wanted) {
                return shown;
            }
        }
        panic!("{wanted:?} not shown on {channel:?} in {passes} passes: {shown:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{echo, idle, life, run_until, text};
    use super::*;
    use crate::ports::{ButtonSpec, Edge, PanelFrame, RefreshKind};
    use crate::record::Recorder;
    use session::{Limits as SessionLimits, ProfileText, Replay, RomIdentity};

    const UART0: ConsoleChannel = ConsoleChannel::Uart0;

    /// A panel whose every refresh gives parts that are not an image.
    struct BrokenPanel(crate::ports::PanelDescription);

    impl crate::ports::PanelSource for BrokenPanel {
        fn describe(&self) -> crate::ports::PanelDescription {
            self.0
        }

        fn poll(
            &mut self,
            _: &mut Esp32C3,
        ) -> Result<Option<PanelFrame>, crate::ports::FrameError> {
            Err(crate::ports::FrameError::Generation)
        }
    }

    /// An image the port cannot make is said in the journal, and the panel keeps no image.
    #[test]
    fn a_panel_image_that_cannot_be_shown_is_said() {
        let mut broken = life(echo(), false);
        broken.panel = Box::new(BrokenPanel(broken.panel.describe()));
        let mut engine = Engine::new(broken, Mode::Run { recorder: None }, Limits::default());
        let output = engine.begin();
        assert!(output.frame.is_none());
        assert!(engine.frame().is_none());
        assert!(
            output
                .notices
                .iter()
                .any(|notice| notice.level == Level::Error
                    && notice
                        .text
                        .contains("the panel gave an image it cannot show")),
            "{:?}",
            output.notices
        );
    }

    #[test]
    fn a_life_begins_with_the_first_image_of_its_panel() {
        let mut engine = Engine::new(
            life(echo(), false),
            Mode::Run { recorder: None },
            Limits::default(),
        );
        let output = engine.begin();
        let frame = output.frame.expect("the first refresh");
        assert_eq!(
            (frame.generation(), frame.refresh()),
            (1, RefreshKind::Full)
        );
        assert_eq!(engine.frame().map(|f| f.generation()), Some(1));
        assert_eq!(engine.buttons().len(), 2);
        assert!(!engine.buttons_reach_firmware());
    }

    /// The consoles in both directions: what the firmware prints comes back as text, what is sent
    /// reaches it at the present instant.
    #[test]
    fn a_console_shows_what_the_firmware_prints_and_takes_what_is_sent() {
        let mut engine = Engine::new(
            life(echo(), false),
            Mode::Run { recorder: None },
            Limits::default(),
        );
        engine.begin();
        assert_eq!(run_until(&mut engine, UART0, "ready", 50), "ready");
        let before = engine.status().device_time_nanos;
        engine.send_console(UART0, "é\r".as_bytes()).unwrap();
        assert_eq!(
            engine.status().device_time_nanos,
            before,
            "sent at the present instant"
        );
        assert_eq!(run_until(&mut engine, UART0, "é\r", 50), "é\r");
        assert_eq!(engine.state(), State::Live);
    }

    /// A CPU that nothing can wake halts the life; an input sets it going again; a step is
    /// refused while it waits.
    #[test]
    fn a_wait_without_end_halts_until_an_input() {
        let mut engine = Engine::new(
            life(idle(), false),
            Mode::Run { recorder: None },
            Limits::default(),
        );
        engine.begin();
        let mut shown = String::new();
        for _ in 0..50 {
            let output = engine.pass(None);
            shown += &text(&output, UART0);
            if engine.state() != State::Live {
                assert!(
                    output
                        .notices
                        .iter()
                        .any(|n| n.text.contains("waits for an input")),
                    "{:?}",
                    output.notices
                );
                break;
            }
        }
        assert_eq!(shown, "zz");
        assert_eq!(engine.state(), State::Halted(Halt::Idle));
        assert_eq!(engine.step().unwrap_err(), EngineError::NothingToStep);
        assert_eq!(
            engine.pass(None).console.len(),
            0,
            "a halted life does not run"
        );
        engine.set_usb_host(UsbHost::Attached).unwrap();
        assert_eq!(engine.state(), State::Live);
        engine.pass(None);
        assert_eq!(
            engine.state(),
            State::Halted(Halt::Idle),
            "nothing woke the CPU"
        );
        assert_eq!(engine.end_now(), RunEnd::Idle);
    }

    /// A pace bounds a pass in device time; a pass already at its pace runs nothing.
    #[test]
    fn a_pass_stops_at_its_pace() {
        let mut engine = Engine::new(
            life(echo(), false),
            Mode::Run { recorder: None },
            Limits::default(),
        );
        engine.begin();
        let mut output = engine.pass(Some(100_000));
        while !output.caught_up {
            output = engine.pass(Some(100_000));
        }
        let at = engine.status().device_time_nanos;
        assert!((100_000..100_100).contains(&at), "{at}");
        let again = engine.pass(Some(100_000));
        assert!(again.caught_up);
        assert_eq!(engine.status().device_time_nanos, at);
        assert_eq!(engine.state(), State::Live, "a pace is no end");
    }

    /// A limit ends the life: the line of the command line says which, and nothing is taken any
    /// more.
    #[test]
    fn a_limit_ends_the_run_and_refuses_what_follows() {
        let limits = Limits {
            max_cycles: Some(2_000),
            max_device_nanos: None,
        };
        let mut engine = Engine::new(life(echo(), false), Mode::Run { recorder: None }, limits);
        engine.begin();
        let output = engine.pass(Some(u64::MAX));
        assert_eq!(engine.state(), State::Ended(RunEnd::Budget));
        assert!(
            output
                .notices
                .iter()
                .any(|n| n.text == "inkemu: cycle limit reached; raise --max-cycles to run longer"),
            "{:?}",
            output.notices
        );
        assert_eq!(
            engine.send_console(UART0, b"x").unwrap_err(),
            EngineError::Ended
        );
        assert_eq!(engine.step().unwrap_err(), EngineError::Ended);
        assert_eq!(
            engine.button("power", true).unwrap_err(),
            EngineError::Ended
        );
        let ended = engine.end();
        assert_eq!(ended.end, RunEnd::Budget);
        assert!(
            ended
                .output
                .notices
                .iter()
                .any(|n| n.text.starts_with("inkemu: stopped at pc "))
        );
    }

    #[test]
    fn a_step_executes_one_instruction() {
        let mut engine = Engine::new(
            life(echo(), false),
            Mode::Run { recorder: None },
            Limits::default(),
        );
        engine.begin();
        engine.step().unwrap();
        assert_eq!(engine.status().instructions, 1);
        engine.step().unwrap();
        assert_eq!(engine.status().instructions, 2);
    }

    /// The buttons of the pattern reach the panel and not the firmware: a press is a refresh,
    /// and an unknown name is refused.
    #[test]
    fn a_button_of_the_pattern_refreshes_the_panel() {
        let mut engine = Engine::new(
            life(echo(), false),
            Mode::Run { recorder: None },
            Limits::default(),
        );
        engine.begin();
        let output = engine.button("confirm", true).unwrap();
        let frame: Arc<PanelFrame> = output.frame.expect("a press is a refresh");
        assert_eq!(frame.generation(), 2);
        assert!(matches!(
            engine.button("home", true),
            Err(EngineError::Button(ButtonError::Unknown { .. }))
        ));
    }

    /// A port whose buttons reach the firmware, as sub-project 2d's will.
    struct Wired;

    impl ButtonPort for Wired {
        fn buttons(&self) -> &[ButtonSpec] {
            const ONE: [ButtonSpec; 1] = [ButtonSpec {
                id: "power",
                edge: Edge::Top,
                order: 0,
            }];
            &ONE
        }
        fn reaches_firmware(&self) -> bool {
            true
        }
        fn set(&mut self, _: &mut Esp32C3, _: &str, _: bool) -> Result<(), ButtonError> {
            Ok(())
        }
    }

    fn x4_text() -> ProfileText {
        ProfileText {
            id: "x4".to_owned(),
            text: boards::builtin_toml("x4").unwrap().to_owned(),
        }
    }

    /// A recorded life of [`echo`] to 20 ms of device time, typed into after `ready`.
    fn recorded() -> Session {
        let rom = echo();
        let identity = RomIdentity::of(3, &rom);
        let life = life(rom, true);
        let limits = SessionLimits {
            max_time_nanos: Some(20_000_000),
            ..SessionLimits::default()
        };
        let recorder = Recorder::begin(identity, x4_text(), &life.machine, None, limits);
        let mut engine = Engine::new(
            life,
            Mode::Run {
                recorder: Some(recorder),
            },
            Limits {
                max_cycles: None,
                max_device_nanos: Some(20_000_000),
            },
        );
        engine.begin();
        run_until(&mut engine, UART0, "ready", 50);
        engine.send_console(UART0, b"hi\r").unwrap();
        while engine.state() == State::Live {
            engine.pass(None);
        }
        assert_eq!(engine.state(), State::Ended(RunEnd::TimeLimit));
        let ended = engine.end();
        assert!(ended.verdict.is_none());
        ended.session.expect("a recorded life").expect("a session")
    }

    /// A recorded life replays to the same outcome; the replay takes no input.
    #[test]
    fn a_recorded_life_replays_identically_and_takes_no_input() {
        let session = recorded();
        assert_eq!(session.inputs.len(), 1);
        let machine = Replay::start(&session, echo()).unwrap().into_machine();
        let (panel, buttons) = crate::pattern::ports(
            crate::pattern::fixtures::PANEL,
            &crate::pattern::fixtures::BUTTONS,
        );
        let (max_cycles, max_device_nanos) = session.limits.run_limits();
        let mut engine = Engine::new(
            Life {
                machine,
                panel,
                buttons,
            },
            Mode::Replay {
                session: Box::new(session),
                digests: ConsoleDigests::default(),
            },
            Limits {
                max_cycles,
                max_device_nanos,
            },
        );
        engine.begin();
        assert!(engine.is_replay());
        assert_eq!(
            engine.send_console(UART0, b"x").unwrap_err(),
            EngineError::Replaying
        );
        assert_eq!(
            engine.button("power", true).unwrap_err(),
            EngineError::Button(ButtonError::Replaying)
        );
        while engine.state() == State::Live {
            engine.pass(None);
        }
        let ended = engine.end();
        assert!(
            matches!(ended.verdict, Some(Ok(Verdict::Identical))),
            "{:?}",
            ended.verdict
        );
    }

    /// A button that reaches the firmware cannot enter a session yet: refused while recording.
    #[test]
    fn a_button_that_reaches_the_firmware_is_refused_while_recording() {
        let rom = echo();
        let identity = RomIdentity::of(3, &rom);
        let mut wired = life(rom, true);
        wired.buttons = Box::new(Wired);
        let recorder = Recorder::begin(
            identity,
            x4_text(),
            &wired.machine,
            None,
            SessionLimits::default(),
        );
        let mut engine = Engine::new(
            wired,
            Mode::Run {
                recorder: Some(recorder),
            },
            Limits::default(),
        );
        assert_eq!(
            engine.button("power", true).unwrap_err(),
            EngineError::Button(ButtonError::NotInSession { id: "power".into() })
        );
        // Unrecorded, the same press is taken.
        let mut free = life(echo(), false);
        free.buttons = Box::new(Wired);
        let mut engine = Engine::new(free, Mode::Run { recorder: None }, Limits::default());
        engine.button("power", true).unwrap();
    }

    /// A user's stop of a running life is a cut of the host; of a waiting one, the wait.
    #[test]
    fn the_end_of_a_life_is_its_state() {
        let mut engine = Engine::new(
            life(echo(), false),
            Mode::Run { recorder: None },
            Limits::default(),
        );
        engine.begin();
        assert_eq!(engine.end_now(), RunEnd::Interrupted);
        run_until(&mut engine, UART0, "ready", 50);
        let ended = engine.end();
        assert_eq!(ended.end, RunEnd::Interrupted);
        assert!(ended.session.is_none());
    }

    #[test]
    fn a_log_becomes_notices_by_its_prefixes() {
        let notices = Notice::from_log(b"inkemu: a\nwarning: b\n\nerror: c\n");
        let levels: Vec<Level> = notices.iter().map(|n| n.level).collect();
        assert_eq!(levels, [Level::Info, Level::Warning, Level::Error]);
        assert_eq!(notices[2].text, "error: c");
    }
}
