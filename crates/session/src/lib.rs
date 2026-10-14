//! Sessions: a run recorded so that it can be replayed, to the byte, on any host (ADR 0007).
//!
//! A session holds what a run is made of (ADR 0004 and ADR 0006): the identity of the mask ROM
//! (never the ROM itself), the device profile as text, the whole flash the run started from, the
//! options that change what the machine does or where the run is cut, the USB host at power-on,
//! and the inputs the machine accepted, dated in device time ([`soc_esp32c3::InputJournal`]). It
//! also holds what the run came to ([`Outcome`]), which a replay must give again. It holds no
//! path and no time of the host.
//!
//! [`Session::to_bytes`] and [`Session::from_bytes`] are the format `inkemu-session 1`; the
//! reader is bounded by constants and names the field it refuses. [`Replay`] rebuilds the machine
//! of a session, hands it every input, runs it to the same end and gives its [`Outcome`], which
//! [`Outcome::differences`] compares with the one recorded. A front end (the command line, a
//! graphical one, the web version) reads and writes the file; this crate performs no host I/O.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod format;
mod outcome;
mod replay;

pub use format::{
    Field, MAX_CONSOLE_BYTES, MAX_CONSOLE_CHUNKS, MAX_DEFLATED_FLASH_BYTES, MAX_HOST_CHANGES,
    MAX_INPUTS, MAX_INPUTS_MEMORY_BYTES, MAX_PROFILE_ID_BYTES, MAX_PROFILE_TEXT_BYTES,
    MAX_SESSION_BYTES, ProfileText, ReadError, RomIdentity, Session, VERSION_LINE, WriteError,
};
pub use outcome::{
    ConsoleDigests, Cut, CutReason, Difference, Divergence, End, Limits, NoTraceHash, Outcome,
    Quantity, Reached,
};
pub use replay::{Replay, ReplayError, Step};
