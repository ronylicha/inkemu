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
//!
//! The `wait` steps after a `wait-for` (or after the start) are added up in nanoseconds, and the
//! sum is rounded up to a cycle of the time base once: a hundred and sixty `wait 1ns` are 160 ns,
//! 26 cycles, as one `wait 160ns` is. A step is never dated before the one before it.
use crate::parse::{Action, Script, Step};
use emu_core::Cycles;
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::{ConsoleChannel, Esp32C3, InputError};

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
    /// The device time the `wait` steps are counted from: the start of the script, or the
    /// instant the last `wait-for` found its text.
    anchor: Cycles,
    /// The nanoseconds of the `wait` steps handed over since the anchor, added up exactly: at
    /// most [`crate::MAX_STEPS`] times `u64::MAX`, far within a `u128`.
    waited: u128,
    /// What the channel of the current `wait-for` sent from the step time on: the last bytes,
    /// as many as its text has at most.
    seen: Vec<u8>,
    /// The step handed over last that is dated, and its date.
    last_dated: Option<(usize, Cycles)>,
    /// The channels the script still watches, each with the index of the last `wait-for` that
    /// names it: a channel no `wait-for` to come names goes back to its pace.
    watched: Vec<(ConsoleChannel, usize)>,
}

impl ScriptRun {
    /// Starts `script` on `machine` at its device time: watches every channel a `wait-for` of the
    /// script names, and hands over the steps before the first `wait-for`.
    ///
    /// # Errors
    /// [`RunError`] when the machine refuses an input. A script that [`Script::parse`] accepted
    /// stays within the bounds the machine puts on how much it holds; what a script cannot know
    /// is the device time its `wait-for` steps reach, so a `send` on a UART dated after about
    /// 114 years of device time is refused ([`InputError::BeyondTheLine`]).
    pub fn start(script: &Script, machine: &mut Esp32C3) -> Result<ScriptRun, RunError> {
        let mut watched: Vec<(ConsoleChannel, usize)> = Vec::new();
        for (index, step) in script.steps().iter().enumerate() {
            if let Action::WaitFor { channel, .. } = step.action {
                if let Some((_, last)) = watched.iter_mut().find(|(other, _)| *other == channel) {
                    *last = index;
                } else {
                    watched.push((channel, index));
                    machine.watch_console(channel, true);
                }
            }
        }
        let mut run = ScriptRun {
            steps: script.steps().to_vec(),
            next: 0,
            anchor: machine.device_time(),
            waited: 0,
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
    /// `wait-for` reads what remains of the same instant on its own channel, unless a `wait`
    /// between them put its step after now: bytes sent before the time of a step are never
    /// searched for it.
    ///
    /// # Errors
    /// [`RunError`] as [`ScriptRun::start`] says.
    pub fn observe(
        &mut self,
        machine: &mut Esp32C3,
        taken: &[(ConsoleChannel, Vec<u8>)],
    ) -> Result<(), RunError> {
        let now = machine.device_time();
        let mut rest: Vec<(ConsoleChannel, &[u8])> = taken
            .iter()
            .map(|(channel, bytes)| (*channel, bytes.as_slice()))
            .collect();
        // INVARIANT: each pass consumes a byte or a step, or ends the loop.
        while let Some((channel, text)) = self.waiting_for() {
            if now < self.step_time() {
                // Sent before the time of the current step: not searched, and neither is what
                // remains of the same instant.
                break;
            }
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
                self.anchor = now;
                self.waited = 0;
                self.last_dated = Some((self.next, now));
                self.next += 1;
                self.advance(machine)?;
            }
        }
        Ok(())
    }

    /// The device time of the next step: the anchor and the waits since, rounded up once.
    fn step_time(&self) -> Cycles {
        // The sum saturates: past the end of device time a step never comes.
        self.anchor + TIME_BASE.nanos_to_cycles(self.waited).get()
    }

    /// Stops watching every channel: the consoles go back to their pace. For a front end that
    /// stops showing them to the script, after a [`RunError`] for instance; the steps not handed
    /// over stay so, and [`ScriptRun::status`] still says where the script stopped.
    pub fn stop(&mut self, machine: &mut Esp32C3) {
        for (channel, _) in self.watched.drain(..) {
            machine.watch_console(channel, false);
        }
    }

    /// Stops watching the channels no `wait-for` from the next step on names.
    fn unwatch_the_channels_done_with(&mut self, machine: &mut Esp32C3) {
        let next = self.next;
        self.watched.retain(|&(channel, last)| {
            let needed = last >= next;
            if !needed {
                machine.watch_console(channel, false);
            }
            needed
        });
    }

    /// The channel and the text of the current `wait-for`.
    fn waiting_for(&self) -> Option<(ConsoleChannel, Vec<u8>)> {
        match &self.steps.get(self.next)?.action {
            Action::WaitFor { channel, text } => Some((*channel, text.clone())),
            _ => None,
        }
    }

    /// Hands the machine every step up to the next `wait-for`, and stops watching the channels
    /// no `wait-for` to come names: when nothing is waited for any more, none.
    fn advance(&mut self, machine: &mut Esp32C3) -> Result<(), RunError> {
        while let Some(step) = self.steps.get(self.next) {
            let at = self.step_time();
            let refused = |source| RunError {
                line: step.line,
                source,
            };
            match &step.action {
                Action::Send { channel, bytes } => {
                    machine.send_console(*channel, at, bytes).map_err(refused)?;
                }
                Action::UsbHost(state) => machine.set_usb_host(at, *state).map_err(refused)?,
                Action::Wait { nanos } => {
                    self.waited = self.waited.saturating_add(u128::from(*nanos));
                }
                Action::WaitFor { .. } => break,
            }
            self.last_dated = Some((self.next, self.step_time()));
            self.next += 1;
        }
        self.unwatch_the_channels_done_with(machine);
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
    use cpu_rv32::asm::{addi, beq, jal, lbu, lui, lw, sw, wfi};
    use soc_esp32c3::test_support::{HANG, T0, T1, flat_rom, print_program};
    use soc_esp32c3::{SocConfig, StopReason, UsbHost};

    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    const USB: u32 = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
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

    /// Sends `ab` to the USB host as one packet, then echoes what UART0 receives.
    fn usb_packet_then_echo() -> Vec<u32> {
        let mut program = print_program(USB, b"ab");
        // `EP1_CONF.WR_DONE`: the two bytes are one packet, taken by the host in one instant.
        program.extend([addi(T1, 0, 1), sw(T1, T0, 4), lui(T0, UART0)]);
        program.extend([
            lbu(T1, T0, 0x1C),
            beq(T1, 0, -4),
            lw(T1, T0, 0),
            sw(T1, T0, 0),
            jal(0, -16),
        ]);
        program
    }

    #[test]
    fn a_wait_for_after_a_wait_ignores_the_rest_of_the_instant_it_follows() {
        // `a` and `b` reach the host at one instant. Once `a` is found, the wait puts the next
        // wait-for 1 ms later: the `b` of that instant was sent before its step, and is not
        // found. Whatever the calls the run is cut into, `1` alone is sent, at the date the wait
        // gives it, and the script waits for `b`.
        let steps = script(
            "wait-for usb-serial-jtag \"a\"\nwait 1ms\nsend uart0 \"1\"\nwait-for usb-serial-jtag \"b\"\nsend uart0 \"2\"\n",
        );
        let outcome = |slice: u64| {
            let mut m = machine(&usb_packet_then_echo());
            let (console, run) = run(&mut m, &steps, slice, 10_000_000);
            let status = run.status(m.device_time());
            (console, status, run.last_dated, m.trace_hash())
        };
        let whole = outcome(u64::MAX);
        assert_eq!(whole.0, b"1");
        assert_eq!(
            whole.1,
            ScriptStatus::WaitingFor {
                step: steps.steps()[3].clone()
            }
        );
        let Some((2, sent_at)) = whole.2 else {
            panic!("{:?}", whole.2);
        };
        assert!(
            sent_at >= Cycles(TIME_BASE.hz() / 1_000),
            "1 ms after the packet: {sent_at:?}"
        );
        for slice in [1, 7, 1_000, 1_000_000] {
            assert_eq!(outcome(slice), whole, "slice {slice}");
        }
    }

    #[test]
    fn a_wait_for_on_the_same_instant_with_no_wait_between_reads_the_rest_of_it() {
        // The same packet, no wait between the two wait-fors: `b` is in what the channel sent
        // from the time of the second step on, and is found.
        let mut m = machine(&usb_packet_then_echo());
        let (console, run) = run(
            &mut m,
            &script(
                "wait-for usb-serial-jtag \"a\"\nwait-for usb-serial-jtag \"b\"\nsend uart0 \"2\"\n",
            ),
            1_000,
            10_000_000,
        );
        assert_eq!(console, b"2");
        assert_eq!(run.status(m.device_time()), ScriptStatus::Complete);
    }

    #[test]
    fn a_channel_no_wait_for_to_come_names_is_no_longer_watched() {
        // UART0 sends `abc` and the CPU sleeps; the script waits for `a` on UART0, then for a
        // text UART1 never sends. Once `a` is found, UART0 is back to its pace: `b` and `c` end
        // no call.
        let mut program = print_program(UART0, b"abc");
        program.extend([wfi(), HANG]);
        let mut m = machine(&program);
        let mut run = ScriptRun::start(
            &script("wait-for uart0 \"a\"\nwait-for uart1 \"z\"\n"),
            &mut m,
        )
        .unwrap();
        assert_eq!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
        let taken = vec![(ConsoleChannel::Uart0, m.take_console(ConsoleChannel::Uart0))];
        assert_eq!(taken[0].1, b"a");
        run.observe(&mut m, &taken).unwrap();
        assert_ne!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"bc");
    }

    #[test]
    fn no_channel_is_watched_once_the_last_step_is_handed_over() {
        let mut program = print_program(UART0, b"abc");
        program.extend([wfi(), HANG]);
        let mut m = machine(&program);
        let mut run = ScriptRun::start(&script("wait-for uart0 \"a\"\n"), &mut m).unwrap();
        assert_eq!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
        let taken = vec![(ConsoleChannel::Uart0, m.take_console(ConsoleChannel::Uart0))];
        run.observe(&mut m, &taken).unwrap();
        assert_eq!(run.status(m.device_time()), ScriptStatus::Complete);
        assert_ne!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"bc");
    }

    #[test]
    fn a_stopped_script_watches_no_channel() {
        // A front end stops the script after a refused step: the consoles go back to their pace.
        let mut program = print_program(UART0, b"abc");
        program.extend([wfi(), HANG]);
        let mut m = machine(&program);
        let mut run = ScriptRun::start(&script("wait-for uart0 \"zzz\"\n"), &mut m).unwrap();
        run.stop(&mut m);
        assert_ne!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"abc");
        assert!(matches!(
            run.status(m.device_time()),
            ScriptStatus::WaitingFor { .. }
        ));
    }

    #[test]
    fn a_run_that_reaches_the_date_of_the_last_step_is_complete_from_that_instant() {
        let mut m = machine(&[HANG]);
        let run = ScriptRun::start(&script("wait 1ms\nusb-host absent\n"), &mut m).unwrap();
        let at = Cycles(TIME_BASE.hz() / 1_000);
        assert_eq!(run.status(at), ScriptStatus::Complete);
        assert_eq!(
            run.status(Cycles(at.get() - 1)),
            ScriptStatus::Ahead {
                step: run.steps[1].clone(),
                at
            }
        );
    }

    #[test]
    fn a_change_of_the_usb_host_applies_before_a_send_on_usb_serial_jtag_at_its_instant() {
        // The format says so: whatever their order in the script, the host closes first, and
        // the packet waits for a port that never opens again.
        for body in [
            "send usb-serial-jtag \"x\"\nusb-host absent\n",
            "usb-host absent\nsend usb-serial-jtag \"x\"\n",
        ] {
            let mut m = machine(&[HANG]);
            let (_, run) = run(&mut m, &script(body), 10_000, 2_000_000);
            assert_eq!(run.status(m.device_time()), ScriptStatus::Complete);
            assert_eq!(m.usb_host(), UsbHost::Absent, "{body:?}");
            assert_eq!(
                m.input_pending(ConsoleChannel::UsbSerialJtag),
                1,
                "{body:?}: the packet never reached the device"
            );
        }
        // An open host at that instant takes it: the order of the two steps does not matter
        // either way.
        let mut m = machine(&[HANG]);
        let (_, _) = run(
            &mut m,
            &script("send usb-serial-jtag \"x\"\n"),
            10_000,
            2_000_000,
        );
        assert_eq!(m.input_pending(ConsoleChannel::UsbSerialJtag), 0);
    }

    #[test]
    fn a_send_dated_beyond_the_line_of_a_uart_is_refused_at_its_line() {
        // A script cannot know the device time its wait-fors reach: a send dated after the
        // last instant a UART times is refused by the machine, and names its step.
        let mut m = machine(&[HANG]);
        let error = ScriptRun::start(
            &script("wait 18446744073s\nwait 18446744073s\nsend uart1 \"x\"\n"),
            &mut m,
        )
        .unwrap_err();
        assert_eq!(error.line, 4);
        assert!(matches!(error.source, InputError::BeyondTheLine { .. }));
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

    /// The date the script gives its last step, a `usb-host absent` after `waits`, the run
    /// having gone nowhere.
    fn date_after(waits: &str) -> Cycles {
        let mut m = machine(&[HANG]);
        let run = ScriptRun::start(&script(&format!("{waits}usb-host absent\n")), &mut m).unwrap();
        match run.status(Cycles::ZERO) {
            ScriptStatus::Ahead { at, .. } => at,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_waits_are_added_up_and_rounded_up_to_a_cycle_of_the_time_base_once() {
        // 160 cycles a microsecond, 6.25 ns a cycle: 25 ns is 4 cycles, 1 ns is one.
        assert_eq!(date_after("wait 25ns\n"), Cycles(4));
        assert_eq!(date_after("wait 1ns\n"), Cycles(1));
        assert_eq!(date_after("wait 1s\n"), Cycles(TIME_BASE.hz()));
        // A hundred and sixty waits of 1 ns are 160 ns, 25.6 cycles: 26, not 160.
        assert_eq!(date_after(&"wait 1ns\n".repeat(160)), Cycles(26));
        assert_eq!(date_after("wait 3ns\nwait 3ns\n"), Cycles(1));
        // Seven of the longest waits add up beyond device time: the date saturates at its end.
        assert_eq!(
            date_after(&"wait 18446744073s\n".repeat(7)),
            Cycles(u64::MAX)
        );
    }

    #[test]
    fn the_waits_after_a_wait_for_are_counted_from_the_instant_it_found_its_text() {
        let mut program = print_program(UART0, b"y");
        program.extend([wfi(), HANG]);
        let mut m = machine(&program);
        let (_, run) = run(
            &mut m,
            &script("wait 1ns\nwait-for uart0 \"y\"\nwait 3ns\nwait 3ns\nusb-host absent\n"),
            u64::MAX,
            1_000_000,
        );
        assert_eq!(run.status(m.device_time()), ScriptStatus::Complete);
        assert!(
            run.anchor > Cycles::ZERO,
            "the anchor is the instant `y` came"
        );
        // 6 ns after the text: one cycle, the wait before the wait-for no longer counted.
        assert_eq!(run.last_dated, Some((4, run.anchor + 1)));
    }

    #[test]
    fn a_refused_input_names_the_line_of_its_step() {
        let mut m = machine(&[HANG]);
        m.run_until(Cycles(10));
        let mut run = ScriptRun {
            steps: script("send uart0 \"x\"\n").steps().to_vec(),
            next: 0,
            anchor: Cycles::ZERO,
            waited: 0,
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
