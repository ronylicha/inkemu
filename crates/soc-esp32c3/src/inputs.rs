//! The journal of the inputs a machine accepted (G1): what a session records so that a replay
//! can hand the same inputs to a new machine.
//!
//! The journal is kept by the machine itself, not by whoever feeds it: a console script, the
//! command line and the keyboard of a graphical front end hand their inputs to
//! [`Esp32C3::send_console`](crate::Esp32C3::send_console) and
//! [`Esp32C3::set_usb_host`](crate::Esp32C3::set_usb_host), and the journal holds them as the
//! machine received them (ADR 0006), in the order it accepted them. An input the machine refuses
//! changes nothing and is not journaled.
use crate::machine::{ConsoleChannel, MAX_PENDING_HOST_BYTES, MAX_PENDING_HOST_CHANGES};
use crate::periph::UsbHost;
use emu_core::Cycles;

/// The most input the journal holds for one console channel over the whole run, each chunk
/// counting for its length and an empty chunk for one byte: the bound ADR 0006 puts on what a
/// channel holds pending, applied to the whole run. A session, which carries the journal, is
/// read within this bound; a replay hands every input over at power-on and so stays within the
/// machine's bound on what is pending.
pub const MAX_JOURNAL_CONSOLE_BYTES: usize = MAX_PENDING_HOST_BYTES;
/// The most changes of the USB host the journal holds over the whole run, for the same reason.
pub const MAX_JOURNAL_HOST_CHANGES: usize = MAX_PENDING_HOST_CHANGES;

/// An input the machine accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcceptedInput {
    /// The host sent `bytes` on `channel`, the first from device time `at`
    /// ([`Esp32C3::send_console`](crate::Esp32C3::send_console)).
    Console {
        channel: ConsoleChannel,
        at: Cycles,
        bytes: Vec<u8>,
    },
    /// From device time `at` the USB host is `state`
    /// ([`Esp32C3::set_usb_host`](crate::Esp32C3::set_usb_host)).
    UsbHost { at: Cycles, state: UsbHost },
}

/// The inputs a machine accepted since power-on, and the state of the USB host at power-on.
///
/// Its memory is bounded by [`MAX_JOURNAL_CONSOLE_BYTES`] per channel and
/// [`MAX_JOURNAL_HOST_CHANGES`]: an input past either bound is accepted by the machine all the
/// same, but the journal stops there and says it is incomplete ([`InputJournal::is_complete`]),
/// so that a session is never written from part of what the run received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputJournal {
    usb_host_at_power_on: UsbHost,
    inputs: Vec<AcceptedInput>,
    /// The input journaled on each channel, by its place in [`ConsoleChannel::ALL`], each chunk
    /// counting for `max(len, 1)`.
    console_bytes: [usize; 3],
    host_changes: usize,
    complete: bool,
}

impl InputJournal {
    /// The journal of a machine at power-on, its USB host `usb_host`.
    #[must_use]
    pub(crate) const fn new(usb_host: UsbHost) -> InputJournal {
        InputJournal {
            usb_host_at_power_on: usb_host,
            inputs: Vec::new(),
            console_bytes: [0; 3],
            host_changes: 0,
            complete: true,
        }
    }

    /// The USB host when the machine started.
    #[must_use]
    pub const fn usb_host_at_power_on(&self) -> UsbHost {
        self.usb_host_at_power_on
    }

    /// The inputs the machine accepted, in the order it accepted them.
    #[must_use]
    pub fn inputs(&self) -> &[AcceptedInput] {
        &self.inputs
    }

    /// Whether the journal holds every input the machine accepted: false once a bound was
    /// passed, after which it holds nothing more.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }

    /// The machine accepted `bytes` on `channel` from `at`.
    pub(crate) fn record_console(&mut self, channel: ConsoleChannel, at: Cycles, bytes: &[u8]) {
        if !self.complete {
            return;
        }
        let total = &mut self.console_bytes[channel.index()];
        let after = total.saturating_add(bytes.len().max(1));
        if after > MAX_JOURNAL_CONSOLE_BYTES {
            self.stop();
            return;
        }
        *total = after;
        self.inputs.push(AcceptedInput::Console {
            channel,
            at,
            bytes: bytes.to_vec(),
        });
    }

    /// The machine accepted that from `at` the USB host is `state`.
    pub(crate) fn record_usb_host(&mut self, at: Cycles, state: UsbHost) {
        if !self.complete {
            return;
        }
        if self.host_changes >= MAX_JOURNAL_HOST_CHANGES {
            self.stop();
            return;
        }
        self.host_changes += 1;
        self.inputs.push(AcceptedInput::UsbHost { at, state });
    }

    /// A bound was passed: what the journal holds is no longer the whole, so it lets it go.
    fn stop(&mut self) {
        self.complete = false;
        self.inputs = Vec::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_journal_holds_the_usb_host_at_power_on_and_no_input() {
        let journal = InputJournal::new(UsbHost::Attached);
        assert_eq!(journal.usb_host_at_power_on(), UsbHost::Attached);
        assert_eq!(journal.inputs(), []);
        assert!(journal.is_complete());
    }

    #[test]
    fn inputs_are_journaled_in_the_order_given() {
        let mut journal = InputJournal::new(UsbHost::Open);
        journal.record_console(ConsoleChannel::Uart0, Cycles(10), b"ab");
        journal.record_usb_host(Cycles(5), UsbHost::Absent);
        journal.record_console(ConsoleChannel::UsbSerialJtag, Cycles(7), b"");
        assert_eq!(
            journal.inputs(),
            [
                AcceptedInput::Console {
                    channel: ConsoleChannel::Uart0,
                    at: Cycles(10),
                    bytes: b"ab".to_vec()
                },
                AcceptedInput::UsbHost {
                    at: Cycles(5),
                    state: UsbHost::Absent
                },
                AcceptedInput::Console {
                    channel: ConsoleChannel::UsbSerialJtag,
                    at: Cycles(7),
                    bytes: Vec::new()
                },
            ]
        );
        assert!(journal.is_complete());
    }

    #[test]
    fn past_its_bound_on_a_channel_the_journal_stops_and_says_it_is_incomplete() {
        let mut journal = InputJournal::new(UsbHost::Open);
        journal.record_console(
            ConsoleChannel::Uart1,
            Cycles(0),
            &vec![b'x'; MAX_JOURNAL_CONSOLE_BYTES - 1],
        );
        // An empty chunk counts for one byte: the channel is now exactly full.
        journal.record_console(ConsoleChannel::Uart1, Cycles(1), b"");
        assert!(journal.is_complete());
        // Another channel has its own bound.
        journal.record_console(ConsoleChannel::Uart0, Cycles(1), b"y");
        assert!(journal.is_complete());
        journal.record_console(ConsoleChannel::Uart1, Cycles(2), b"");
        assert!(!journal.is_complete());
        assert_eq!(journal.inputs(), [], "an incomplete journal holds nothing");
        journal.record_usb_host(Cycles(3), UsbHost::Absent);
        assert_eq!(journal.inputs(), [], "nor anything after");
    }

    #[test]
    fn past_its_bound_on_host_changes_the_journal_stops() {
        let mut journal = InputJournal::new(UsbHost::Open);
        for at in 0..MAX_JOURNAL_HOST_CHANGES as u64 {
            journal.record_usb_host(Cycles(at), UsbHost::Open);
        }
        assert!(journal.is_complete());
        assert_eq!(journal.inputs().len(), MAX_JOURNAL_HOST_CHANGES);
        journal.record_usb_host(Cycles(u64::MAX), UsbHost::Absent);
        assert!(!journal.is_complete());
    }
}
