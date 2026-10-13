//! Console scripts: what the host sends to the serial consoles of a run, and when, written as a
//! small text (ADR 0006).
//!
//! ```text
//! inkemu-console-script 1
//! # Wait for the prompt, then type a command.
//! wait-for uart0 "ready"
//! wait 10ms
//! send uart0 "help\r"
//! usb-host absent
//! ```
//!
//! [`Script::parse`] reads the text, every refusal naming its line and its column;
//! [`ScriptRun`] hands the steps to a machine as dated inputs while it runs, and says how far the
//! script got. A front end (the command line, a graphical one) reads the file and drives the
//! machine; this crate performs no host I/O. The format is versioned by its first line, so that the
//! scenarios of a later plan can supersede it without ambiguity.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod parse;
mod run;

pub use parse::{
    Action, MAX_LINE_BYTES, MAX_SCRIPT_BYTES, MAX_SEND_BYTES, MAX_STEPS, MAX_TEXT_BYTES,
    MAX_TOTAL_BYTES, Script, ScriptError, ScriptErrorKind, Step, VERSION_LINE,
};
pub use run::{RunError, ScriptRun, ScriptStatus};
