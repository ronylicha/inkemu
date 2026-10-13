//! A console script while a machine runs: its steps become dated inputs of the machine, and its
//! waits read what the consoles of the machine send (ADR 0006).
//!
//! The time of a step is fixed by the script and the run alone. A `wait-for` ends at the device
//! time at which the last byte of its text reached the host: the channels the script waits on are
//! watched ([`Esp32C3::watch_console`]), so each byte they send ends a call of the machine at the
//! instruction boundary it was sent at, and the bytes a caller takes after a call reached the
//! host at [`Esp32C3::device_time`]. The steps that follow are dated from that instant and handed
//! over at once, before the machine runs on. However a front end cuts the run into calls, the
//! machine gets the same inputs at the same dates.
use crate::parse::{Action, Script, Step};
use emu_core::Cycles;
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::{ConsoleChannel, Esp32C3, InputError};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A duration of `nanos` nanoseconds in cycles of the time base, rounded up: a wait never ends
/// before the time asked.
fn cycles_of(nanos: u64) -> u64 {
    let cycles = (u128::from(nanos) * u128::from(TIME_BASE.hz())).div_ceil(NANOS_PER_SECOND);
    // INVARIANT: a second has more nanoseconds than cycles of the time base, so the quotient is
    // at most `nanos`: the fallback, that same bound, is never taken.
    u64::try_from(cycles).unwrap_or(nanos)
}

/// An input the machine refused, and the line of the step that gave it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("console script, line {line}: {source}")]
pub struct RunError {
    pub line: usize,
    #[source]
    pub source: InputError,
}

/// How far a script got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptStatus {
    /// Every step was handed to the machine, and the run reached the time of the last one.
    Complete,
    /// The run waits for the text of this `wait-for`.
    WaitingFor { step: Step },
    /// Every step was handed to the machine, but this one is dated at `at`, which the run has not
    /// reached.
    Ahead { step: Step, at: Cycles },
}

/// A console script handed to a machine as it runs.
#[derive(Debug)]
pub struct ScriptRun {
    steps: Vec<Step>,
    /// The next step to hand to the machine.
    next: usize,
    /// The device time of the next step.
    step_time: Cycles,
    /// What the channel of the current `wait-for` sent from the step time on: the last bytes,
    /// as many as its text has at most.
    seen: Vec<u8>,
    /// The step handed over last that is dated, and its date.
    last_dated: Option<(usize, Cycles)>,
    /// The channels the script waits on and still watches.
    watched: Vec<ConsoleChannel>,
}

impl ScriptRun {
    /// Starts `script` on `machine` at its device time: watches every channel a `wait-for` of the
    /// script names, and hands over the steps before the first `wait-for`.
    ///
    /// # Errors
    /// [`RunError`] when the machine refuses an input; a script that [`Script::parse`] accepted
    /// stays within the bounds of the machine, so this does not happen from a script file.
    pub fn start(script: &Script, machine: &mut Esp32C3) -> Result<ScriptRun, RunError> {
        let mut watched = Vec::new();
        for step in script.steps() {
            if let Action::WaitFor { channel, .. } = step.action
                && !watched.contains(&channel)
            {
                watched.push(channel);
                machine.watch_console(channel, true);
            }
        }
        let mut run = ScriptRun {
            steps: script.steps().to_vec(),
            next: 0,
            step_time: machine.device_time(),
            seen: Vec::new(),
            last_dated: None,
            watched,
        };
        run.advance(machine)?;
        Ok(run)
    }

    /// The machine handed `taken` to the host since the last call, on each channel, at its device
    /// time now: the bytes a front end took from every channel after a call of
    /// [`Esp32C3::run_within`]. The current `wait-for` reads those of its channel; when its text
    /// is complete, the steps that follow are dated from now and handed over, and the next
    /// `wait-for` reads what remains of the same instant on its own channel.
    ///
    /// # Errors
    /// [`RunError`] as [`ScriptRun::start`] says.
    pub fn observe(
        &mut self,
        machine: &mut Esp32C3,
        taken: &[(ConsoleChannel, Vec<u8>)],
    ) -> Result<(), RunError> {
        let now = machine.device_time();
        if now < self.step_time {
            // Sent before the time of the current step: not searched.
            return Ok(());
        }
        let mut rest: Vec<(ConsoleChannel, &[u8])> = taken
            .iter()
            .map(|(channel, bytes)| (*channel, bytes.as_slice()))
            .collect();
        // INVARIANT: each pass consumes a byte or a step, or ends the loop.
        while let Some((channel, text)) = self.waiting_for() {
            let Some(entry) = rest
                .iter_mut()
                .find(|(c, bytes)| *c == channel && !bytes.is_empty())
            else {
                break;
            };
            let Some((&byte, after)) = entry.1.split_first() else {
                break;
            };
            entry.1 = after;
            self.seen.push(byte);
            if self.seen.len() > text.len() {
                self.seen.remove(0);
            }
            if self.seen == text {
                self.seen.clear();
                self.step_time = now;
                self.last_dated = Some((self.next, now));
                self.next += 1;
                self.advance(machine)?;
            }
        }
        Ok(())
    }

    /// The channel and the text of the current `wait-for`.
    fn waiting_for(&self) -> Option<(ConsoleChannel, Vec<u8>)> {
        match &self.steps.get(self.next)?.action {
            Action::WaitFor { channel, text } => Some((*channel, text.clone())),
            _ => None,
        }
    }

    /// Hands the machine every step up to the next `wait-for`.
    fn advance(&mut self, machine: &mut Esp32C3) -> Result<(), RunError> {
        while let Some(step) = self.steps.get(self.next) {
            let refused = |source| RunError {
                line: step.line,
                source,
            };
            match &step.action {
                Action::Send { channel, bytes } => machine
                    .send_console(*channel, self.step_time, bytes)
                    .map_err(refused)?,
                Action::UsbHost(state) => machine
                    .set_usb_host(self.step_time, *state)
                    .map_err(refused)?,
                Action::Wait { nanos } => {
                    self.step_time = self.step_time + cycles_of(*nanos);
                }
                Action::WaitFor { .. } => return Ok(()),
            }
            self.last_dated = Some((self.next, self.step_time));
            self.next += 1;
        }
        // Nothing is waited for any more: the consoles go back to their pace.
        for channel in self.watched.drain(..) {
            machine.watch_console(channel, false);
        }
        Ok(())
    }

    /// How many steps the script has.
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// How far the script got, the run being at device time `now`.
    #[must_use]
    pub fn status(&self, now: Cycles) -> ScriptStatus {
        if let Some(step) = self.steps.get(self.next) {
            return ScriptStatus::WaitingFor { step: step.clone() };
        }
        match self.last_dated {
            Some((index, at)) if at > now => ScriptStatus::Ahead {
                step: self.steps[index].clone(),
                at,
            },
            _ => ScriptStatus::Complete,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpu_rv32::asm::{beq, jal, lbu, lw, sw, wfi};
    use soc_esp32c3::test_support::{HANG, T0, T1, flat_rom, print_program};
    use soc_esp32c3::{SocConfig, StopReason, UsbHost};

    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    /// A byte at the reset settings of UART0, in cycles of the time base (`machine.rs`).
    const RESET_BYTE: u64 = 55_520;

    fn script(body: &str) -> Script {
        Script::parse(format!("{}\n{body}", crate::VERSION_LINE).as_bytes()).unwrap()
    }

    fn machine(program: &[u32]) -> Esp32C3 {
        Esp32C3::new(
            flat_rom(program),
            &SocConfig {
                trace_hash: true,
                ..SocConfig::default()
            },
        )
    }

    /// Prints `ready` on UART0, then echoes what UART0 receives.
    fn ready_then_echo() -> Vec<u32> {
        let mut program = print_program(UART0, b"ready");
        program.extend([
            lbu(T1, T0, 0x1C),
            beq(T1, 0, -4),
            lw(T1, T0, 0),
            sw(T1, T0, 0),
            jal(0, -16),
        ]);
        program
    }

    /// Runs `machine` with `script` to `limit_ns` of device time in calls of `slice` cycles, as a
    /// front end does: after each call it takes every console and shows it to the script.
    fn run(
        machine: &mut Esp32C3,
        script: &Script,
        slice: u64,
        limit_ns: u64,
    ) -> (Vec<u8>, ScriptRun) {
        let mut run = ScriptRun::start(script, machine).unwrap();
        let mut uart0 = Vec::new();
        loop {
            let stop = machine.run_within(machine.cycles() + slice, Some(limit_ns));
            let taken: Vec<(ConsoleChannel, Vec<u8>)> = ConsoleChannel::ALL
                .into_iter()
                .map(|channel| (channel, machine.take_console(channel)))
                .collect();
            uart0.extend(&taken[0].1);
            run.observe(machine, &taken).unwrap();
            match stop {
                StopReason::TimeBudget | StopReason::Idle => break,
                StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => {}
                StopReason::StrictMmio(diagnostic) => panic!("{diagnostic}"),
            }
        }
        (uart0, run)
    }

    #[test]
    fn a_wait_for_dates_the_next_steps_from_the_instant_its_text_reached_the_host() {
        let mut m = machine(&ready_then_echo());
        let (console, run) = run(
            &mut m,
            &script("wait-for uart0 \"dy\"\nsend uart0 \"ok\"\n"),
            u64::MAX,
            20_000_000,
        );
        assert_eq!(console, b"readyok");
        assert_eq!(run.status(m.device_time()), ScriptStatus::Complete);
        assert_eq!(run.step_count(), 2);
    }

    #[test]
    fn the_steps_are_dated_the_same_whatever_the_calls_the_run_is_cut_into() {
        // ADR 0006: the device time of every input is fixed by the inputs alone.
        let steps = script(
            "wait-for uart0 \"ready\"\nwait 1ms\nsend uart0 \"a\"\nwait-for uart0 \"a\"\nsend uart0 \"bc\"\nusb-host attached\nwait 2ms\nsend uart0 \"\\r\"\n",
        );
        let outcome = |slice: u64| {
            let mut m = machine(&ready_then_echo());
            let (console, run) = run(&mut m, &steps, slice, 30_000_000);
            assert_eq!(
                run.status(m.device_time()),
                ScriptStatus::Complete,
                "slice {slice}"
            );
            (console, m.trace_hash(), m.device_time(), m.instructions())
        };
        let whole = outcome(u64::MAX);
        assert_eq!(whole.0, b"readyabc\r");
        for slice in [1, 7, 1_000, 1_000_000] {
            assert_eq!(outcome(slice), whole, "slice {slice}");
        }
    }

    #[test]
    fn the_text_of_a_wait_for_is_searched_only_from_the_time_of_its_step() {
        // `ready` is sent before the wait ends: the wait-for after it never sees it.
        let mut m = machine(&ready_then_echo());
        let (_, run) = run(
            &mut m,
            &script("wait 20ms\nwait-for uart0 \"ready\"\n"),
            1_000,
            40_000_000,
        );
        assert_eq!(
            run.status(m.device_time()),
            ScriptStatus::WaitingFor {
                step: Step {
                    line: 3,
                    action: Action::WaitFor {
                        channel: ConsoleChannel::Uart0,
                        text: b"ready".to_vec()
                    }
                }
            }
        );
    }

    #[test]
    fn a_text_found_where_its_bytes_cross_the_calls_and_after_a_false_start_is_found() {
        let mut m = machine(&{
            let mut program = print_program(UART0, b"rereadyX");
            program.extend([wfi(), HANG]);
            program
        });
        let (_, run) = run(&mut m, &script("wait-for uart0 \"ready\"\n"), 3, 20_000_000);
        assert_eq!(run.status(m.device_time()), ScriptStatus::Complete);
    }

    #[test]
    fn a_wait_for_ends_at_the_instant_the_last_byte_of_its_text_reached_the_host() {
        // The program sleeps after its writes: the bytes reach the host at their very instants,
        // and the `y` of `ready` leaves the fifth byte time after the first. The send after the
        // wait-for is dated then, and its echo leaves after it lands and is sent back.
        let mut program = print_program(UART0, b"ready");
        program.extend([wfi(), HANG]);
        let mut m = machine(&program);
        let mut run_with_dates =
            ScriptRun::start(&script("wait-for uart0 \"y\"\nsend uart1 \"z\"\n"), &mut m).unwrap();
        let first = loop {
            assert_eq!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
            let bytes = m.take_console(ConsoleChannel::Uart0);
            if bytes == b"r" {
                break m.device_time();
            }
        };
        let found = loop {
            m.run_until(Cycles(1_000_000));
            let taken = vec![(ConsoleChannel::Uart0, m.take_console(ConsoleChannel::Uart0))];
            run_with_dates.observe(&mut m, &taken).unwrap();
            if taken[0].1 == b"y" {
                break m.device_time();
            }
        };
        assert_eq!(found, first + 4 * RESET_BYTE);
        assert_eq!(
            m.input_pending(ConsoleChannel::Uart1),
            1,
            "sent at {found:?}"
        );
        assert_eq!(run_with_dates.status(found), ScriptStatus::Complete);
    }

    #[test]
    fn a_run_that_ends_before_the_date_of_its_last_step_says_so() {
        let mut m = machine(&[HANG]);
        let (_, run) = run(
            &mut m,
            &script("wait 1s\nusb-host absent\n"),
            1_000_000,
            10_000_000,
        );
        assert_eq!(
            run.status(m.device_time()),
            ScriptStatus::Ahead {
                step: Step {
                    line: 3,
                    action: Action::UsbHost(UsbHost::Absent)
                },
                at: Cycles(TIME_BASE.hz()),
            }
        );
        assert_eq!(m.usb_host_changes_pending(), 1);
    }

    #[test]
    fn a_wait_is_rounded_up_to_a_cycle_of_the_time_base() {
        // 160 cycles a microsecond: 25 ns is 4 cycles, 1 ns is one.
        assert_eq!(cycles_of(25), 4);
        assert_eq!(cycles_of(1), 1);
        assert_eq!(cycles_of(0), 0);
        assert_eq!(cycles_of(1_000_000_000), TIME_BASE.hz());
        // 0.16 cycle a nanosecond: the longest wait, rounded up, without overflow.
        assert_eq!(cycles_of(u64::MAX), 2_951_479_051_793_528_259);
    }

    #[test]
    fn a_refused_input_names_the_line_of_its_step() {
        let mut m = machine(&[HANG]);
        m.run_until(Cycles(10));
        let mut run = ScriptRun {
            steps: script("send uart0 \"x\"\n").steps().to_vec(),
            next: 0,
            step_time: Cycles::ZERO,
            seen: Vec::new(),
            last_dated: None,
            watched: Vec::new(),
        };
        let error = run.advance(&mut m).unwrap_err();
        assert_eq!(error.line, 2);
        assert!(
            error
                .to_string()
                .starts_with("console script, line 2: the input is dated 0 ns")
        );
    }
}
