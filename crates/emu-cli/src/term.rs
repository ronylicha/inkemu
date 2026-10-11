//! Text that may reach a terminal.

/// Escapes control characters (C0, DEL, C1) so untrusted text cannot drive the terminal.
///
/// Firmware metadata, paths and device names are attacker-controlled; a raw ESC or CSI byte in them
/// could inject escape sequences. Printable text, non-ASCII letters included, is unchanged.
#[must_use]
pub fn sanitize(text: &str) -> String {
    text.chars()
        .fold(String::with_capacity(text.len()), |mut out, c| {
            if c.is_control() {
                out.extend(c.escape_default());
            } else {
                out.push(c);
            }
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_escapes_every_kind_of_control_character() {
        assert_eq!(sanitize("a\u{1b}[31m"), "a\\u{1b}[31m");
        assert_eq!(sanitize("\u{7}"), "\\u{7}");
        assert_eq!(sanitize("x\r"), "x\\r");
        assert_eq!(sanitize("\u{9b}"), "\\u{9b}");
        assert_eq!(sanitize("\u{7f}"), "\\u{7f}");
    }

    #[test]
    fn sanitize_leaves_printable_text_unchanged() {
        assert_eq!(sanitize("café 1.0"), "café 1.0");
    }
}
