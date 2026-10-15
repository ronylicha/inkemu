//! What a run came to, the limits that ended it, and how a replay is compared with it (G3, G6).
use emu_core::Cycles;
use sha2::{Digest as _, Sha256};
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::{ConsoleChannel, Esp32C3};
use std::fmt::{self, Write as _};

/// Why the host cut a run before it reached an end of its own (ADR 0004: such an end depends on
/// the host). A session records the instant of the cut, which makes the run replayable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutReason {
    /// The user asked the run to stop (Ctrl-C).
    Interrupted,
    /// The reader of the console went away.
    ReaderGone,
    /// The console could not be written.
    ConsoleError,
}

impl CutReason {
    /// The cut as a summary names it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            CutReason::Interrupted => "Ctrl-C",
            CutReason::ReaderGone => "the reader of the console going away",
            CutReason::ConsoleError => "a console that could not be written",
        }
    }
}

/// Where the host cut a run: the device time at which it stopped, in cycles of the time base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    pub at: Cycles,
    pub reason: CutReason,
}

impl Cut {
    /// Whether a replay can stop exactly at the instant of the cut: whether the nanoseconds of
    /// the instant, the unit [`Esp32C3::run_within`] takes a device-time limit in, give it back.
    /// They do up to about 584 years of device time, the most a `u64` of nanoseconds counts;
    /// past it they saturate, and a replay would stop earlier. A session holds no later cut:
    /// such a run is refused when it is recorded, and such a file when it is read.
    #[must_use]
    pub fn is_replayable(self) -> bool {
        let nanos = TIME_BASE.cycles_to_nanos(self.at);
        TIME_BASE.nanos_to_cycles(u128::from(nanos)) == self.at
    }
}

/// The options of a run that change what the machine does or where the run ends (ADR 0004
/// point 1), and the cut of a run the host ended (G5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// `--strict-mmio`.
    pub strict_mmio: bool,
    /// `--max-cycles`: CPU cycles.
    pub max_cycles: Option<u64>,
    /// `--max-time`: nanoseconds of device time.
    pub max_time_nanos: Option<u64>,
    pub cut: Option<Cut>,
}

impl Limits {
    /// The cycle limit and the device-time limit a replay runs with, as
    /// [`Esp32C3::run_within`] takes them: those of the run, the cut being one more device-time
    /// limit, the earlier of the two applying.
    ///
    /// The cut is an instant in cycles of the time base, and `run_within` takes nanoseconds,
    /// which it rounds up to a cycle: the nanoseconds of the instant rounded down give that
    /// instant back exactly, a cycle of the time base (6.25 ns) being longer than a nanosecond,
    /// up to the 584 years a `u64` of nanoseconds counts, past which a session holds no cut
    /// ([`Cut::is_replayable`]).
    #[must_use]
    pub fn run_limits(&self) -> (Option<u64>, Option<u64>) {
        let cut = self.cut.map(|cut| TIME_BASE.cycles_to_nanos(cut.at));
        let time = match (self.max_time_nanos, cut) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (one, other) => one.or(other),
        };
        (self.max_cycles, time)
    }

    /// What ended a run, or its replay, that stopped as `reached` says at `device_time`. A stop
    /// on a limit at or after the instant of the cut is the cut: no instruction executes
    /// without moving device time, so every stop at that instant has the cycles of the run
    /// that was cut there.
    #[must_use]
    pub fn end_of(&self, reached: Reached, device_time: Cycles) -> End {
        match reached {
            Reached::CycleLimit | Reached::TimeLimit
                if self.cut.is_some_and(|cut| device_time >= cut.at) =>
            {
                End::Cut
            }
            Reached::CycleLimit => End::CycleLimit,
            Reached::TimeLimit => End::TimeLimit,
            Reached::Idle => End::Idle,
            Reached::DeepSleep => End::DeepSleep,
            Reached::StrictMmio => End::StrictMmio,
            Reached::PowerOff => End::PowerOff,
        }
    }
}

/// How a call of the machine ended a run of its own: the stops no further call changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reached {
    CycleLimit,
    TimeLimit,
    Idle,
    DeepSleep,
    StrictMmio,
    /// The board's supply failed (ADR 0013).
    PowerOff,
}

/// How a recorded run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// `--max-cycles` was reached.
    CycleLimit,
    /// `--max-time` was reached.
    TimeLimit,
    /// The CPU waits for an interrupt and none can come.
    Idle,
    /// The device is in a deep sleep nothing modelled can end.
    DeepSleep,
    /// `--strict-mmio` stopped the run.
    StrictMmio,
    /// The host cut the run ([`Limits::cut`] says where and why).
    Cut,
    /// The board's supply failed: the device powered off (ADR 0013).
    PowerOff,
}

impl End {
    /// Every end, in the order of its code in the file.
    pub const ALL: [End; 7] = [
        End::CycleLimit,
        End::TimeLimit,
        End::Idle,
        End::DeepSleep,
        End::StrictMmio,
        End::Cut,
        End::PowerOff,
    ];

    /// The end as a summary names it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            End::CycleLimit => "the cycle limit",
            End::TimeLimit => "the device-time limit",
            End::Idle => "a CPU waiting for an interrupt none can raise",
            End::DeepSleep => "a deep sleep nothing can end",
            End::StrictMmio => "a --strict-mmio stop",
            End::Cut => "the cut of the host",
            End::PowerOff => "the device powering off",
        }
    }
}

/// The SHA-256 of everything each console sent: what a session compares, rather than the bytes.
#[derive(Clone, Debug, Default)]
pub struct ConsoleDigests {
    digests: [Sha256; 3],
}

impl ConsoleDigests {
    /// `channel` sent `bytes`, after what it sent before.
    pub fn add(&mut self, channel: ConsoleChannel, bytes: &[u8]) {
        self.digests[place(channel)].update(bytes);
    }

    /// The SHA-256 of each channel, in [`ConsoleChannel::ALL`] order.
    #[must_use]
    pub fn finish(self) -> [[u8; 32]; 3] {
        self.digests.map(|digest| digest.finalize().into())
    }
}

/// `bytes` in lowercase hexadecimal, as `sha256sum` prints a digest.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        // Writing to a `String` cannot fail.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// The place of `channel` in [`ConsoleChannel::ALL`].
pub(crate) const fn place(channel: ConsoleChannel) -> usize {
    match channel {
        ConsoleChannel::Uart0 => 0,
        ConsoleChannel::UsbSerialJtag => 1,
        ConsoleChannel::Uart1 => 2,
    }
}

/// The machine was built without its trace hash ([`soc_esp32c3::SocConfig::trace_hash`]),
/// which an outcome needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "the machine keeps no trace hash, and the outcome of a session holds one; build it with the trace hash on"
)]
pub struct NoTraceHash;

/// What a run came to (G3): what its replay must give again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub end: End,
    /// Device time at the end, in cycles of the time base.
    pub device_time: Cycles,
    /// CPU cycles executed.
    pub cycles: u64,
    /// Instructions retired.
    pub instructions: u64,
    /// The trace hash: the fingerprint of the executed PC sequence.
    pub trace_hash: u64,
    /// The SHA-256 of the whole flash at the end.
    pub flash_sha256: [u8; 32],
    /// The SHA-256 of everything each console sent, in [`ConsoleChannel::ALL`] order: what was
    /// taken during the run and what the end flushed.
    pub consoles: [[u8; 32]; 3],
}

impl Outcome {
    /// The outcome of `machine`, whose run ended as `end`, its consoles taken into `consoles`
    /// to the last byte ([`Esp32C3::flush_console`] included).
    ///
    /// # Errors
    /// [`NoTraceHash`] when the machine keeps no trace hash.
    pub fn of(
        machine: &Esp32C3,
        end: End,
        consoles: ConsoleDigests,
    ) -> Result<Outcome, NoTraceHash> {
        let trace_hash = machine.trace_hash().ok_or(NoTraceHash)?;
        Ok(Outcome {
            end,
            device_time: machine.device_time(),
            cycles: machine.cycles().get(),
            instructions: machine.instructions(),
            trace_hash,
            flash_sha256: Sha256::digest(machine.flash_contents()).into(),
            consoles: consoles.finish(),
        })
    }

    /// Every quantity that differs between this outcome, the recorded one, and `replayed`, in
    /// the order of G3: the end, device time, cycles, instructions, the trace hash, the flash,
    /// then each console. The values are compared as they are; only those that differ are put
    /// into words.
    #[must_use]
    pub fn differences(&self, replayed: &Outcome) -> Vec<Difference> {
        let mut found = Vec::new();
        let mut compare = |quantity: Quantity, differs: bool, show: &dyn Fn(&Outcome) -> String| {
            if differs {
                found.push(Difference {
                    quantity,
                    recorded: show(self),
                    replayed: show(replayed),
                });
            }
        };
        compare(Quantity::End, self.end != replayed.end, &|o| {
            o.end.name().to_owned()
        });
        compare(
            Quantity::DeviceTime,
            self.device_time != replayed.device_time,
            &|o| device_time_text(o.device_time),
        );
        compare(Quantity::Cycles, self.cycles != replayed.cycles, &|o| {
            o.cycles.to_string()
        });
        compare(
            Quantity::Instructions,
            self.instructions != replayed.instructions,
            &|o| o.instructions.to_string(),
        );
        compare(
            Quantity::TraceHash,
            self.trace_hash != replayed.trace_hash,
            &|o| format!("{:016x}", o.trace_hash),
        );
        compare(
            Quantity::FinalFlash,
            self.flash_sha256 != replayed.flash_sha256,
            &|o| hex(&o.flash_sha256),
        );
        for channel in ConsoleChannel::ALL {
            let at = place(channel);
            compare(
                Quantity::Console(channel),
                self.consoles[at] != replayed.consoles[at],
                &|o| hex(&o.consoles[at]),
            );
        }
        found
    }
}

/// An instant of device time in words: in nanoseconds, or in cycles of the time base past the
/// 584 years a `u64` of nanoseconds counts, where two instants would show the same nanoseconds.
fn device_time_text(instant: Cycles) -> String {
    let nanos = TIME_BASE.cycles_to_nanos(instant);
    if TIME_BASE.nanos_to_cycles(u128::from(nanos)) == instant {
        format!("{nanos} ns of device time")
    } else {
        format!("{} cycles of the time base of device time", instant.get())
    }
}

/// A quantity of an outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    End,
    DeviceTime,
    Cycles,
    Instructions,
    TraceHash,
    FinalFlash,
    Console(ConsoleChannel),
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Quantity::End => f.write_str("the end"),
            Quantity::DeviceTime => f.write_str("the device time"),
            Quantity::Cycles => f.write_str("the cycles"),
            Quantity::Instructions => f.write_str("the instructions"),
            Quantity::TraceHash => f.write_str("the trace hash"),
            Quantity::FinalFlash => f.write_str("the SHA-256 of the final flash"),
            Quantity::Console(channel) => write!(f, "the SHA-256 of console {}", channel.name()),
        }
    }
}

/// A quantity the replay gives otherwise than the recording, with both values as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    pub quantity: Quantity,
    pub recorded: String,
    pub replayed: String,
}

/// How a replay diverges from its session: every quantity that differs, the first one first. It
/// says the first with its two values, then names the others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divergence(pub Vec<Difference>);

impl fmt::Display for Divergence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some((first, others)) = self.0.split_first() else {
            return f.write_str("nothing differs");
        };
        // The quantities are singular (the end) and plural (the cycles): no verb agrees with both.
        write!(
            f,
            "{}: {} when recorded, {} in the replay",
            first.quantity, first.recorded, first.replayed
        )?;
        if !others.is_empty() {
            f.write_str("; also different: ")?;
            for (index, other) in others.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{}", other.quantity)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome() -> Outcome {
        Outcome {
            end: End::TimeLimit,
            device_time: Cycles(160),
            cycles: 20,
            instructions: 19,
            trace_hash: 0xabcd,
            flash_sha256: [1; 32],
            consoles: [[2; 32], [3; 32], [4; 32]],
        }
    }

    #[test]
    fn the_same_outcome_has_no_difference() {
        assert_eq!(outcome().differences(&outcome()), []);
    }

    #[test]
    fn the_first_quantity_that_differs_is_named_with_both_values_and_the_others_after_it() {
        let replayed = Outcome {
            instructions: 18,
            trace_hash: 0xabce,
            consoles: [[2; 32], [9; 32], [4; 32]],
            ..outcome()
        };
        let differences = outcome().differences(&replayed);
        let quantities: Vec<Quantity> = differences.iter().map(|d| d.quantity).collect();
        assert_eq!(
            quantities,
            [
                Quantity::Instructions,
                Quantity::TraceHash,
                Quantity::Console(ConsoleChannel::UsbSerialJtag)
            ]
        );
        assert_eq!(
            Divergence(differences).to_string(),
            "the instructions: 19 when recorded, 18 in the replay; also different: the trace hash, the SHA-256 of console usb-serial-jtag"
        );
        let time = Outcome {
            device_time: Cycles(320),
            ..outcome()
        };
        assert_eq!(
            Divergence(outcome().differences(&time)).to_string(),
            "the device time: 1000 ns of device time when recorded, 2000 ns of device time in the replay"
        );
    }

    #[test]
    fn instants_whose_nanoseconds_saturate_are_compared_and_shown_as_they_are() {
        // Past 584 years both instants show u64::MAX nanoseconds: they differ all the same.
        let late = Outcome {
            device_time: Cycles(u64::MAX - 1),
            ..outcome()
        };
        let later = Outcome {
            device_time: Cycles(u64::MAX),
            ..outcome()
        };
        let differences = late.differences(&later);
        assert_eq!(differences.len(), 1);
        assert_eq!(differences[0].quantity, Quantity::DeviceTime);
        assert_eq!(
            differences[0].recorded,
            format!("{} cycles of the time base of device time", u64::MAX - 1)
        );
        assert_ne!(differences[0].recorded, differences[0].replayed);
    }

    #[test]
    fn a_cut_is_replayable_up_to_the_last_instant_whose_nanoseconds_count() {
        let cut = |at| Cut {
            at: Cycles(at),
            reason: CutReason::Interrupted,
        };
        // The last instant a limit of `u64::MAX` nanoseconds reaches, rounded up to a cycle of
        // the time base (6.25 ns): past it, the nanoseconds saturate there.
        let last = u64::try_from((u128::from(u64::MAX) * 4).div_ceil(25)).unwrap();
        assert!(cut(0).is_replayable());
        assert!(cut(last).is_replayable());
        assert!(!cut(last + 1).is_replayable());
        assert!(!cut(u64::MAX).is_replayable());
    }

    #[test]
    fn the_cut_is_one_more_time_limit_and_its_instant_comes_back_exact() {
        let cut = Cut {
            at: Cycles(1_000_003),
            reason: CutReason::Interrupted,
        };
        let limits = Limits {
            max_time_nanos: Some(1_000_000_000),
            cut: Some(cut),
            ..Limits::default()
        };
        let (cycles, nanos) = limits.run_limits();
        assert_eq!(cycles, None);
        // The nanoseconds of the cut, rounded up to a cycle as `run_within` does, are the cut.
        let nanos = nanos.unwrap();
        assert_eq!(TIME_BASE.nanos_to_cycles(u128::from(nanos)), cut.at);
        // Up to the 584 years a `u64` of nanoseconds counts, the longest limit of a run.
        for at in [1, 3, 7, 1_000_003, u64::MAX / 7] {
            let back = TIME_BASE.nanos_to_cycles(u128::from(TIME_BASE.cycles_to_nanos(Cycles(at))));
            assert_eq!(back, Cycles(at), "{at}");
        }
        let earlier = Limits {
            max_time_nanos: Some(5),
            ..limits
        };
        assert_eq!(earlier.run_limits().1, Some(5));
        assert_eq!(Limits::default().run_limits(), (None, None));
    }

    #[test]
    fn a_stop_on_a_limit_at_the_cut_is_the_cut_and_any_other_stop_is_itself() {
        let limits = Limits {
            max_cycles: Some(100),
            cut: Some(Cut {
                at: Cycles(50),
                reason: CutReason::ReaderGone,
            }),
            ..Limits::default()
        };
        assert_eq!(limits.end_of(Reached::CycleLimit, Cycles(50)), End::Cut);
        assert_eq!(limits.end_of(Reached::TimeLimit, Cycles(50)), End::Cut);
        assert_eq!(
            limits.end_of(Reached::CycleLimit, Cycles(49)),
            End::CycleLimit
        );
        assert_eq!(limits.end_of(Reached::Idle, Cycles(50)), End::Idle);
        let uncut = Limits::default();
        assert_eq!(uncut.end_of(Reached::TimeLimit, Cycles(50)), End::TimeLimit);
        assert_eq!(uncut.end_of(Reached::DeepSleep, Cycles(50)), End::DeepSleep);
        assert_eq!(
            uncut.end_of(Reached::StrictMmio, Cycles(50)),
            End::StrictMmio
        );
    }

    #[test]
    fn the_digests_of_the_consoles_are_those_of_their_bytes_whatever_the_takes() {
        let mut once = ConsoleDigests::default();
        once.add(ConsoleChannel::Uart1, b"hello");
        let mut twice = ConsoleDigests::default();
        twice.add(ConsoleChannel::Uart1, b"hel");
        twice.add(ConsoleChannel::Uart1, b"lo");
        let (once, twice) = (once.finish(), twice.finish());
        assert_eq!(once, twice);
        assert_eq!(once[2], <[u8; 32]>::from(Sha256::digest(b"hello")));
        assert_eq!(once[0], <[u8; 32]>::from(Sha256::digest(b"")));
    }
}
