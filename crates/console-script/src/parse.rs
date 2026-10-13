//! The text of a console script, and the steps it is made of.
//!
//! One step per line, in order; `#` begins a comment; blank lines are ignored. The first line is
//! [`VERSION_LINE`]. Strings are between double quotes, with the escapes `\r`, `\n`, `\t`, `\\`,
//! `\"` and `\xHH`; every other character is sent as its UTF-8 bytes. Nothing is translated: Enter
//! is `\r`, as a terminal sends it (report UR §10). Every value read from the text is bounded by a
//! constant before anything is allocated for it.
use soc_esp32c3::{ConsoleChannel, UsbHost};
use std::fmt;

/// The first line of every console script: the format and its version.
pub const VERSION_LINE: &str = "inkemu-console-script 1";
/// The longest script file, in bytes: a front end reads no more.
pub const MAX_SCRIPT_BYTES: usize = 1 << 20;
/// The longest line, in bytes: a send of [`MAX_SEND_BYTES`] written as `\xHH` escapes fits.
pub const MAX_LINE_BYTES: usize = 8192;
/// The most steps a script holds.
pub const MAX_STEPS: usize = 10_000;
/// The most bytes one `send` holds, its escapes resolved.
pub const MAX_SEND_BYTES: usize = 1024;
const _: () = assert!(4 * MAX_SEND_BYTES + "send usb-serial-jtag \"\"".len() < MAX_LINE_BYTES);
/// The longest text a `wait-for` waits for, in bytes.
pub const MAX_TEXT_BYTES: usize = 256;
/// The most bytes all the sends of a script hold together: with [`MAX_STEPS`] zero-length
/// packets, less than the most a channel of the machine holds
/// ([`soc_esp32c3::MAX_PENDING_HOST_BYTES`]), so that no send of a script is refused for it.
pub const MAX_TOTAL_BYTES: usize = 512 * 1024;
const _: () = assert!(MAX_TOTAL_BYTES + MAX_STEPS < soc_esp32c3::MAX_PENDING_HOST_BYTES);
// Nor is a change of the USB host refused: a script holds fewer than the machine keeps.
const _: () = assert!(MAX_STEPS < soc_esp32c3::MAX_PENDING_HOST_CHANGES);

/// What one step does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// `send <channel> "<bytes>"`: the host sends the bytes at the time of the step.
    Send {
        channel: ConsoleChannel,
        bytes: Vec<u8>,
    },
    /// `wait <duration>`: the time of the next steps is this many nanoseconds later.
    Wait { nanos: u64 },
    /// `wait-for <channel> "<text>"`: the time of the next steps is the device time at which the
    /// last byte of the first occurrence of the text reached the host on the channel, searched in
    /// what the channel sent from the time of the step on.
    WaitFor {
        channel: ConsoleChannel,
        text: Vec<u8>,
    },
    /// `usb-host absent|attached|open`: the USB host is in that state from the time of the step.
    UsbHost(UsbHost),
}

/// A step and the line it was written on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub line: usize,
    pub action: Action,
}

/// A console script, read and checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    steps: Vec<Step>,
}

/// A refusal of a script: where it is, then what is wrong and what to write instead.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {column}: {kind}")]
pub struct ScriptError {
    pub line: usize,
    pub column: usize,
    pub kind: ScriptErrorKind,
}

/// Why a script is refused. Each message gives the cause, then the remedy.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScriptErrorKind {
    #[error("the script is {len} bytes long, more than {MAX_SCRIPT_BYTES}; split it, or send less")]
    TooLong { len: usize },
    #[error(
        "the text is not UTF-8; save the script as UTF-8, and write other bytes as \\xHH between double quotes"
    )]
    NotUtf8,
    #[error("the first line is not `{VERSION_LINE}`; make it the first line of the script")]
    NoVersionLine,
    #[error(
        "the script is written for version `{found}` of the format, and inkemu reads version 1; write it for version 1"
    )]
    UnknownVersion { found: String },
    #[error("the line is {len} bytes long, more than {MAX_LINE_BYTES}; split the step in several")]
    LineTooLong { len: usize },
    #[error("the script has more than {MAX_STEPS} steps; split it in several scripts")]
    TooManySteps,
    #[error("`{word}` is no step; write send, wait, wait-for or usb-host")]
    UnknownStep { word: String },
    #[error("`{step}` needs {what}; write it after `{step}`")]
    Missing {
        step: &'static str,
        what: &'static str,
    },
    #[error("`{word}` is no channel; write uart0, uart1 or usb-serial-jtag")]
    UnknownChannel { word: String },
    #[error("`{word}` is no state of the USB host; write absent, attached or open")]
    UnknownHostState { word: String },
    #[error(
        "`{word}` is no duration; write a whole number and its unit, ns, us, ms or s, like 250ms"
    )]
    BadDuration { word: String },
    #[error(
        "`{word}` is more nanoseconds than device time can count (584 years); give a shorter wait"
    )]
    DurationTooLong { word: String },
    #[error("a string is expected here; write the bytes between double quotes")]
    NotAString,
    #[error("the string has no closing double quote; close it on the same line")]
    UnclosedString,
    #[error("`{escape}` is no escape; write \\r, \\n, \\t, \\\\, \\\" or \\x and two hex digits")]
    BadEscape { escape: String },
    #[error("the string holds more than {MAX_SEND_BYTES} bytes; split it in several sends")]
    SendTooLong,
    #[error("the text holds more than {MAX_TEXT_BYTES} bytes; wait for a shorter part of it")]
    TextTooLong,
    #[error("an empty text is found at once; give the text to wait for")]
    EmptyText,
    #[error(
        "the sends of the script hold more than {MAX_TOTAL_BYTES} bytes; send less, or split the script"
    )]
    TooManyBytes,
    #[error("`{text}` follows the step; remove it, or begin a comment with #")]
    Trailing { text: String },
}

impl Script {
    /// Reads a console script.
    ///
    /// # Errors
    /// The first thing wrong in the text, at its line and column ([`ScriptError`]).
    pub fn parse(bytes: &[u8]) -> Result<Script, ScriptError> {
        if bytes.len() > MAX_SCRIPT_BYTES {
            return Err(at(1, 1, ScriptErrorKind::TooLong { len: bytes.len() }));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| {
            let (before, _) = bytes.split_at(e.valid_up_to());
            // INVARIANT: `before` is the valid UTF-8 prefix the error reports.
            let before = std::str::from_utf8(before).unwrap_or_default();
            let line = before.matches('\n').count() + 1;
            let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
            at(
                line,
                before[line_start..].chars().count() + 1,
                ScriptErrorKind::NotUtf8,
            )
        })?;
        let mut lines = text
            .split('\n')
            .enumerate()
            .map(|(index, line)| (index + 1, line.strip_suffix('\r').unwrap_or(line)));
        match lines.next() {
            Some((_, VERSION_LINE)) => {}
            Some((_, first)) => {
                let kind = match first.strip_prefix("inkemu-console-script ") {
                    Some(found) => ScriptErrorKind::UnknownVersion {
                        found: found.trim().to_owned(),
                    },
                    None => ScriptErrorKind::NoVersionLine,
                };
                return Err(at(1, 1, kind));
            }
            None => return Err(at(1, 1, ScriptErrorKind::NoVersionLine)),
        }
        let mut steps = Vec::new();
        let mut total = 0usize;
        for (number, line) in lines {
            if line.len() > MAX_LINE_BYTES {
                return Err(at(
                    number,
                    1,
                    ScriptErrorKind::LineTooLong { len: line.len() },
                ));
            }
            let mut cursor = Cursor {
                line,
                pos: 0,
                number,
            };
            let Some(action) = cursor.step()? else {
                continue;
            };
            if steps.len() == MAX_STEPS {
                return Err(at(number, 1, ScriptErrorKind::TooManySteps));
            }
            if let Action::Send { bytes, .. } = &action {
                total += bytes.len();
                if total > MAX_TOTAL_BYTES {
                    return Err(at(number, 1, ScriptErrorKind::TooManyBytes));
                }
            }
            steps.push(Step {
                line: number,
                action,
            });
        }
        Ok(Script { steps })
    }

    /// The steps, in the order they run.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }
}

fn at(line: usize, column: usize, kind: ScriptErrorKind) -> ScriptError {
    ScriptError { line, column, kind }
}

/// Reads one line.
struct Cursor<'a> {
    line: &'a str,
    /// Byte offset of the next character.
    pos: usize,
    number: usize,
}

impl<'a> Cursor<'a> {
    /// The column of byte offset `pos`, counted in characters from 1.
    fn column(&self, pos: usize) -> usize {
        self.line[..pos].chars().count() + 1
    }

    fn refuse<T>(&self, pos: usize, kind: ScriptErrorKind) -> Result<T, ScriptError> {
        Err(at(self.number, self.column(pos), kind))
    }

    fn skip_spaces(&mut self) {
        let rest = &self.line[self.pos..];
        self.pos += rest.len() - rest.trim_start().len();
    }

    /// The next word and its offset, if the line has one before a comment.
    fn word(&mut self) -> Option<(usize, &'a str)> {
        self.skip_spaces();
        let line = self.line;
        let rest = &line[self.pos..];
        if rest.is_empty() || rest.starts_with('#') {
            return None;
        }
        let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let start = self.pos;
        self.pos += len;
        Some((start, &line[start..start + len]))
    }

    /// The step of the line, `None` for a blank or comment line.
    fn step(&mut self) -> Result<Option<Action>, ScriptError> {
        let Some((start, word)) = self.word() else {
            return Ok(None);
        };
        let action = match word {
            "send" => {
                let channel = self.channel("send")?;
                let bytes = self.string("send", MAX_SEND_BYTES, ScriptErrorKind::SendTooLong)?;
                Action::Send { channel, bytes }
            }
            "wait" => Action::Wait {
                nanos: self.duration()?,
            },
            "wait-for" => {
                let channel = self.channel("wait-for")?;
                self.skip_spaces();
                let string_at = self.pos;
                let text = self.string("wait-for", MAX_TEXT_BYTES, ScriptErrorKind::TextTooLong)?;
                if text.is_empty() {
                    return self.refuse(string_at, ScriptErrorKind::EmptyText);
                }
                Action::WaitFor { channel, text }
            }
            "usb-host" => Action::UsbHost(self.host_state()?),
            other => {
                return self.refuse(
                    start,
                    ScriptErrorKind::UnknownStep {
                        word: other.to_owned(),
                    },
                );
            }
        };
        self.skip_spaces();
        let rest = &self.line[self.pos..];
        if !rest.is_empty() && !rest.starts_with('#') {
            return self.refuse(
                self.pos,
                ScriptErrorKind::Trailing {
                    text: rest.to_owned(),
                },
            );
        }
        Ok(Some(action))
    }

    fn channel(&mut self, step: &'static str) -> Result<ConsoleChannel, ScriptError> {
        let Some((start, word)) = self.word() else {
            return self.refuse(
                self.pos,
                ScriptErrorKind::Missing {
                    step,
                    what: "a channel (uart0, uart1 or usb-serial-jtag)",
                },
            );
        };
        let found = ConsoleChannel::ALL
            .into_iter()
            .find(|channel| channel.name() == word);
        match found {
            Some(channel) => Ok(channel),
            None => self.refuse(
                start,
                ScriptErrorKind::UnknownChannel {
                    word: word.to_owned(),
                },
            ),
        }
    }

    fn host_state(&mut self) -> Result<UsbHost, ScriptError> {
        let Some((start, word)) = self.word() else {
            return self.refuse(
                self.pos,
                ScriptErrorKind::Missing {
                    step: "usb-host",
                    what: "a state (absent, attached or open)",
                },
            );
        };
        match UsbHost::ALL.into_iter().find(|state| state.name() == word) {
            Some(state) => Ok(state),
            None => self.refuse(
                start,
                ScriptErrorKind::UnknownHostState {
                    word: word.to_owned(),
                },
            ),
        }
    }

    /// `<digits><unit>`, in nanoseconds.
    fn duration(&mut self) -> Result<u64, ScriptError> {
        let Some((start, word)) = self.word() else {
            return self.refuse(
                self.pos,
                ScriptErrorKind::Missing {
                    step: "wait",
                    what: "a duration (a whole number and ns, us, ms or s)",
                },
            );
        };
        let digits = word.len() - word.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let (number, unit) = word.split_at(digits);
        let scale: u64 = match unit {
            "ns" => 1,
            "us" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            _ => 0,
        };
        let bad = || ScriptErrorKind::BadDuration {
            word: word.to_owned(),
        };
        if number.is_empty() || scale == 0 {
            return self.refuse(start, bad());
        }
        // The digits are ASCII: the fold only overflows.
        number
            .bytes()
            .try_fold(0u64, |value, digit| {
                value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
            })
            .and_then(|value| value.checked_mul(scale))
            .map_or_else(
                || {
                    self.refuse(
                        start,
                        ScriptErrorKind::DurationTooLong {
                            word: word.to_owned(),
                        },
                    )
                },
                Ok,
            )
    }

    /// A string between double quotes, its escapes resolved, at most `limit` bytes.
    fn string(
        &mut self,
        step: &'static str,
        limit: usize,
        too_long: ScriptErrorKind,
    ) -> Result<Vec<u8>, ScriptError> {
        self.skip_spaces();
        let open = self.pos;
        let rest = &self.line[open..];
        if rest.is_empty() || rest.starts_with('#') {
            return self.refuse(
                open,
                ScriptErrorKind::Missing {
                    step,
                    what: "a string between double quotes",
                },
            );
        }
        if !rest.starts_with('"') {
            return self.refuse(open, ScriptErrorKind::NotAString);
        }
        let mut bytes = Vec::new();
        let mut chars = rest.char_indices().skip(1);
        loop {
            let Some((offset, c)) = chars.next() else {
                return self.refuse(open, ScriptErrorKind::UnclosedString);
            };
            let start = open + offset;
            match c {
                '"' => {
                    self.pos = start + 1;
                    return Ok(bytes);
                }
                '\\' => {
                    let escaped = match chars.next() {
                        Some((_, 'r')) => b'\r',
                        Some((_, 'n')) => b'\n',
                        Some((_, 't')) => b'\t',
                        Some((_, '\\')) => b'\\',
                        Some((_, '"')) => b'"',
                        Some((_, 'x')) => {
                            let high = chars.next().and_then(|(_, c)| c.to_digit(16));
                            let low = chars.next().and_then(|(_, c)| c.to_digit(16));
                            match high
                                .zip(low)
                                .and_then(|(h, l)| u8::try_from(h * 16 + l).ok())
                            {
                                Some(byte) => byte,
                                None => return self.bad_escape(start),
                            }
                        }
                        _ => return self.bad_escape(start),
                    };
                    bytes.push(escaped);
                }
                other => {
                    let mut buffer = [0; 4];
                    bytes.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
                }
            }
            if bytes.len() > limit {
                return self.refuse(open, too_long);
            }
        }
    }

    /// Refuses the escape that begins at byte offset `start`: its backslash and what follows it,
    /// at most three characters.
    fn bad_escape<T>(&self, start: usize) -> Result<T, ScriptError> {
        let escape: String = self.line[start..].chars().take(4).collect();
        self.refuse(start, ScriptErrorKind::BadEscape { escape })
    }
}

impl fmt::Display for Step {
    /// The step as it is written in a script, for a front end that says where a script stopped.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.action {
            Action::Send { channel, bytes } => {
                write!(f, "send {} \"{}\"", channel.name(), quoted(bytes))
            }
            Action::Wait { nanos } => write!(f, "wait {nanos}ns"),
            Action::WaitFor { channel, text } => {
                write!(f, "wait-for {} \"{}\"", channel.name(), quoted(text))
            }
            Action::UsbHost(state) => write!(f, "usb-host {}", state.name()),
        }
    }
}

/// `bytes` as a script writes them between double quotes.
fn quoted(bytes: &[u8]) -> String {
    let mut text = String::new();
    for &byte in bytes {
        match byte {
            b'\r' => text.push_str("\\r"),
            b'\n' => text.push_str("\\n"),
            b'\t' => text.push_str("\\t"),
            b'\\' => text.push_str("\\\\"),
            b'"' => text.push_str("\\\""),
            0x20..=0x7E => text.push(char::from(byte)),
            other => {
                text.push_str("\\x");
                for nibble in [other >> 4, other & 0xF] {
                    // INVARIANT: a nibble is a hex digit: the fallback is never taken.
                    text.push(char::from_digit(u32::from(nibble), 16).unwrap_or('0'));
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(body: &str) -> Result<Script, ScriptError> {
        Script::parse(format!("{VERSION_LINE}\n{body}").as_bytes())
    }

    fn refusal(body: &str) -> (usize, usize, ScriptErrorKind) {
        let error = script(body).unwrap_err();
        (error.line, error.column, error.kind)
    }

    #[test]
    fn the_four_steps_are_read_in_order_with_their_lines() {
        let read = script(
            "# a comment\n\nsend uart0 \"a\"\n  wait 250ms   # half of it\nwait-for usb-serial-jtag \"ready\"\nusb-host attached\nsend uart1 \"\"\n",
        )
        .unwrap();
        assert_eq!(
            read.steps(),
            [
                Step {
                    line: 4,
                    action: Action::Send {
                        channel: ConsoleChannel::Uart0,
                        bytes: b"a".to_vec()
                    }
                },
                Step {
                    line: 5,
                    action: Action::Wait { nanos: 250_000_000 }
                },
                Step {
                    line: 6,
                    action: Action::WaitFor {
                        channel: ConsoleChannel::UsbSerialJtag,
                        text: b"ready".to_vec()
                    }
                },
                Step {
                    line: 7,
                    action: Action::UsbHost(UsbHost::Attached)
                },
                Step {
                    line: 8,
                    action: Action::Send {
                        channel: ConsoleChannel::Uart1,
                        bytes: Vec::new()
                    }
                },
            ]
        );
    }

    #[test]
    fn escapes_give_their_bytes_and_other_characters_their_utf8_bytes() {
        let read = script("send uart0 \"\\r\\n\\t\\\\\\\"\\x00\\xFf é\"\n").unwrap();
        assert_eq!(
            read.steps()[0].action,
            Action::Send {
                channel: ConsoleChannel::Uart0,
                bytes: b"\r\n\t\\\"\x00\xff \xc3\xa9".to_vec()
            }
        );
        assert_eq!(
            read.steps()[0].to_string(),
            "send uart0 \"\\r\\n\\t\\\\\\\"\\x00\\xff \\xc3\\xa9\""
        );
    }

    #[test]
    fn durations_take_their_units_and_carriage_returns_end_lines() {
        let read = script("wait 7ns\r\nwait 3us\r\nwait 2ms\r\nwait 5s\r\n").unwrap();
        let nanos: Vec<Action> = read.steps().iter().map(|s| s.action.clone()).collect();
        assert_eq!(
            nanos,
            [7, 3_000, 2_000_000, 5_000_000_000].map(|nanos| Action::Wait { nanos })
        );
    }

    #[test]
    fn the_version_line_must_come_first() {
        let first = |text: &str| Script::parse(text.as_bytes()).unwrap_err().kind;
        assert_eq!(first(""), ScriptErrorKind::NoVersionLine);
        assert_eq!(
            first("# comment\ninkemu-console-script 1\n"),
            ScriptErrorKind::NoVersionLine
        );
        assert_eq!(
            first("inkemu-console-script 2\n"),
            ScriptErrorKind::UnknownVersion {
                found: "2".to_owned()
            }
        );
        assert_eq!(
            Script::parse(b"inkemu-console-script 1").unwrap(),
            Script::default()
        );
        assert_eq!(
            Script::parse(b"inkemu-console-script 1\r\n").unwrap(),
            Script::default()
        );
    }

    #[test]
    fn every_refusal_names_its_line_and_column() {
        let channel = |word: &str| ScriptErrorKind::UnknownChannel {
            word: word.to_owned(),
        };
        for (body, line, column, kind) in [
            (
                "jump 3s",
                2,
                1,
                ScriptErrorKind::UnknownStep {
                    word: "jump".to_owned(),
                },
            ),
            ("send uart2 \"x\"", 2, 6, channel("uart2")),
            (
                "send",
                2,
                5,
                ScriptErrorKind::Missing {
                    step: "send",
                    what: "a channel (uart0, uart1 or usb-serial-jtag)",
                },
            ),
            (
                "send uart0   # nothing",
                2,
                14,
                ScriptErrorKind::Missing {
                    step: "send",
                    what: "a string between double quotes",
                },
            ),
            (
                "usb-host closed",
                2,
                10,
                ScriptErrorKind::UnknownHostState {
                    word: "closed".to_owned(),
                },
            ),
            (
                "usb-host open now",
                2,
                15,
                ScriptErrorKind::Trailing {
                    text: "now".to_owned(),
                },
            ),
            (
                "\n\nsend uart0 \"a\" \"b\"",
                4,
                16,
                ScriptErrorKind::Trailing {
                    text: "\"b\"".to_owned(),
                },
            ),
        ] {
            assert_eq!(refusal(body), (line, column, kind), "{body:?}");
        }
    }

    #[test]
    fn every_refusal_of_a_string_or_a_duration_names_its_line_and_column() {
        for (body, line, column, kind) in [
            ("send uart0 x", 2, 12, ScriptErrorKind::NotAString),
            ("send uart0 \"x", 2, 12, ScriptErrorKind::UnclosedString),
            (
                "send uart0 \"é\\q\"",
                2,
                14,
                ScriptErrorKind::BadEscape {
                    escape: "\\q\"".to_owned(),
                },
            ),
            (
                "send uart0 \"\\x4\"",
                2,
                13,
                ScriptErrorKind::BadEscape {
                    escape: "\\x4\"".to_owned(),
                },
            ),
            (
                "wait 3",
                2,
                6,
                ScriptErrorKind::BadDuration {
                    word: "3".to_owned(),
                },
            ),
            (
                "wait ms",
                2,
                6,
                ScriptErrorKind::BadDuration {
                    word: "ms".to_owned(),
                },
            ),
            (
                "wait 1.5s",
                2,
                6,
                ScriptErrorKind::BadDuration {
                    word: "1.5s".to_owned(),
                },
            ),
            (
                "wait 18446744074s",
                2,
                6,
                ScriptErrorKind::DurationTooLong {
                    word: "18446744074s".to_owned(),
                },
            ),
            (
                "wait",
                2,
                5,
                ScriptErrorKind::Missing {
                    step: "wait",
                    what: "a duration (a whole number and ns, us, ms or s)",
                },
            ),
            ("wait-for uart0 \"\"", 2, 16, ScriptErrorKind::EmptyText),
        ] {
            assert_eq!(refusal(body), (line, column, kind), "{body:?}");
        }
    }

    #[test]
    fn a_text_that_is_not_utf8_is_refused_where_it_stops_being_utf8() {
        let mut text = format!("{VERSION_LINE}\nsend uart0 \"é").into_bytes();
        text.push(0xFF);
        let error = Script::parse(&text).unwrap_err();
        assert_eq!((error.line, error.column), (2, 14));
        assert_eq!(error.kind, ScriptErrorKind::NotUtf8);
    }

    #[test]
    fn each_limit_is_held() {
        let long = "x".repeat(MAX_SEND_BYTES);
        assert!(script(&format!("send uart0 \"{long}\"")).is_ok());
        assert_eq!(
            refusal(&format!("send uart0 \"{long}x\"")),
            (2, 12, ScriptErrorKind::SendTooLong)
        );
        let escaped = "\\x41".repeat(MAX_SEND_BYTES);
        let read = script(&format!("send uart0 \"{escaped}\"")).unwrap();
        assert_eq!(
            read.steps()[0].action,
            Action::Send {
                channel: ConsoleChannel::Uart0,
                bytes: vec![b'A'; MAX_SEND_BYTES]
            },
            "an escape is one byte"
        );
        let text = "t".repeat(MAX_TEXT_BYTES);
        assert!(script(&format!("wait-for uart0 \"{text}\"")).is_ok());
        assert_eq!(
            refusal(&format!("wait-for uart0 \"{text}t\"")),
            (2, 16, ScriptErrorKind::TextTooLong)
        );
        let line = format!("# {}", "c".repeat(MAX_LINE_BYTES - 2));
        assert!(script(&line).is_ok());
        assert_eq!(
            refusal(&format!("{line}c")),
            (
                2,
                1,
                ScriptErrorKind::LineTooLong {
                    len: MAX_LINE_BYTES + 1
                }
            )
        );
        let steps = "wait 1ns\n".repeat(MAX_STEPS);
        assert_eq!(script(&steps).unwrap().steps().len(), MAX_STEPS);
        assert_eq!(
            refusal(&format!("{steps}wait 1ns\n")),
            (MAX_STEPS + 2, 1, ScriptErrorKind::TooManySteps)
        );
        let send = format!("send uart0 \"{long}\"\n");
        let sends = send.repeat(MAX_TOTAL_BYTES / MAX_SEND_BYTES);
        assert!(script(&sends).is_ok());
        let over = MAX_TOTAL_BYTES / MAX_SEND_BYTES + 2;
        assert_eq!(
            refusal(&format!("{sends}send uart0 \"x\"\n")),
            (over, 1, ScriptErrorKind::TooManyBytes)
        );
        let file = vec![b'#'; MAX_SCRIPT_BYTES + 1];
        assert_eq!(
            Script::parse(&file).unwrap_err(),
            at(
                1,
                1,
                ScriptErrorKind::TooLong {
                    len: MAX_SCRIPT_BYTES + 1
                }
            )
        );
    }

    #[test]
    fn every_message_states_the_cause_then_the_remedy() {
        let kinds = [
            ScriptErrorKind::TooLong { len: 9 },
            ScriptErrorKind::NotUtf8,
            ScriptErrorKind::NoVersionLine,
            ScriptErrorKind::UnknownVersion { found: "2".into() },
            ScriptErrorKind::LineTooLong { len: 9 },
            ScriptErrorKind::TooManySteps,
            ScriptErrorKind::UnknownStep { word: "x".into() },
            ScriptErrorKind::Missing {
                step: "send",
                what: "a channel",
            },
            ScriptErrorKind::UnknownChannel { word: "x".into() },
            ScriptErrorKind::UnknownHostState { word: "x".into() },
            ScriptErrorKind::BadDuration { word: "x".into() },
            ScriptErrorKind::DurationTooLong { word: "x".into() },
            ScriptErrorKind::NotAString,
            ScriptErrorKind::UnclosedString,
            ScriptErrorKind::BadEscape {
                escape: "\\q".into(),
            },
            ScriptErrorKind::SendTooLong,
            ScriptErrorKind::TextTooLong,
            ScriptErrorKind::EmptyText,
            ScriptErrorKind::TooManyBytes,
            ScriptErrorKind::Trailing { text: "x".into() },
        ];
        for kind in kinds {
            let text = at(3, 7, kind).to_string();
            let (cause, remedy) = text.split_once("; ").unwrap_or_else(|| panic!("{text}"));
            assert!(cause.starts_with("line 3, column 7: "), "{text}");
            assert!(!remedy.is_empty(), "{text}");
        }
    }
}
