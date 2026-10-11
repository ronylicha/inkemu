//! Helpers shared by the integration tests of this crate.

/// Lower-case hexadecimal of `bytes`: the form in which digests are pinned and printed.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}
