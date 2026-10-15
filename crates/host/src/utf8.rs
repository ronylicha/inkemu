//! The text of a console, decoded as its bytes arrive (design of sub-project 2c, C12).
//!
//! A console delivers bytes in the slices a pass of the machine cuts, and a UTF-8 sequence can be
//! cut in two: the decoder keeps the start of an unfinished sequence for the next slice instead of
//! turning it into a replacement character. Bytes that are not UTF-8 become U+FFFD, as
//! [`String::from_utf8_lossy`] makes them.

/// A UTF-8 decoder over a stream of slices.
#[derive(Debug, Default, Clone)]
pub struct Utf8Stream {
    /// The start of a sequence the last slice cut: at most three bytes.
    pending: Vec<u8>,
}

/// How many bytes the sequence that `lead` starts has; `None` when `lead` starts none.
const fn sequence_len(lead: u8) -> Option<usize> {
    match lead {
        0x00..=0x7F => Some(1),
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}

impl Utf8Stream {
    #[must_use]
    pub const fn new() -> Utf8Stream {
        Utf8Stream {
            pending: Vec::new(),
        }
    }

    /// The text of `bytes` after what earlier slices left, without the start of a sequence that
    /// `bytes` ends in the middle of.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut all = std::mem::take(&mut self.pending);
        all.extend_from_slice(bytes);
        let keep = Self::unfinished_tail(&all);
        let cut = all.len() - keep;
        self.pending = all.split_off(cut);
        String::from_utf8_lossy(&all).into_owned()
    }

    /// The text of what is still pending: the start of a sequence that never ended.
    pub fn finish(&mut self) -> String {
        let pending = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&pending).into_owned()
    }

    /// How many bytes at the end of `bytes` start a valid sequence that is not complete yet.
    fn unfinished_tail(bytes: &[u8]) -> usize {
        // A sequence is at most four bytes: its lead is among the last three.
        for back in 1..=bytes.len().min(3) {
            let lead = bytes[bytes.len() - back];
            if lead & 0xC0 == 0x80 {
                // A continuation byte: the lead is further back.
                continue;
            }
            return match sequence_len(lead) {
                // The sequence needs more than what is there, and what is there fits it.
                Some(len) if len > back && Self::continues(&bytes[bytes.len() - back..]) => back,
                _ => 0,
            };
        }
        0
    }

    /// Whether the bytes after the lead are all continuation bytes, and the second fits the lead
    /// (the ranges UTF-8 allows after `E0`, `ED`, `F0` and `F4`).
    fn continues(sequence: &[u8]) -> bool {
        let Some((lead, rest)) = sequence.split_first() else {
            return false;
        };
        if !rest.iter().all(|byte| byte & 0xC0 == 0x80) {
            return false;
        }
        match (lead, rest.first()) {
            (0xE0, Some(second)) => *second >= 0xA0,
            (0xED, Some(second)) => *second < 0xA0,
            (0xF0, Some(second)) => *second >= 0x90,
            (0xF4, Some(second)) => *second < 0x90,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through() {
        let mut stream = Utf8Stream::new();
        assert_eq!(
            stream.push(b"I (31) boot: ESP-IDF\r\n"),
            "I (31) boot: ESP-IDF\r\n"
        );
        assert_eq!(stream.finish(), "");
    }

    /// Every cut of a text in every place gives the text back.
    #[test]
    fn a_sequence_cut_anywhere_is_kept_whole() {
        let text = "é€😀 fin";
        let bytes = text.as_bytes();
        for cut in 0..=bytes.len() {
            let mut stream = Utf8Stream::new();
            let mut out = stream.push(&bytes[..cut]);
            out += &stream.push(&bytes[cut..]);
            out += &stream.finish();
            assert_eq!(out, text, "cut at {cut}");
        }
        // One byte at a time.
        let mut stream = Utf8Stream::new();
        let out: String = bytes.iter().map(|b| stream.push(&[*b])).collect();
        assert_eq!(out, text);
    }

    #[test]
    fn bytes_that_are_not_utf8_become_replacement_characters() {
        let mut stream = Utf8Stream::new();
        assert_eq!(stream.push(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
        // A lead followed by something that is not a continuation is not kept back.
        assert_eq!(stream.push(&[0xC3, b'x']), "\u{FFFD}x");
        // A surrogate start is not a valid sequence: not kept back either.
        assert_eq!(stream.push(&[0xED, 0xA0]), "\u{FFFD}\u{FFFD}");
    }

    /// A sequence that never ends is given at the end, as replacement characters.
    #[test]
    fn a_sequence_that_never_ends_is_given_by_finish() {
        let mut stream = Utf8Stream::new();
        assert_eq!(stream.push(&[b'z', 0xE2, 0x82]), "z");
        assert_eq!(stream.finish(), "\u{FFFD}");
        assert_eq!(stream.finish(), "");
    }
}
