//! Reads the file given to `inkemu run --console-script`. The format, its checks and the run of
//! its steps belong to the `console-script` crate, so that a front end gets the same answers;
//! this module only reads the file, within the size the format allows, and names it.
use crate::flash_file::{ReadFailure, read_regular_file};
use crate::term::sanitize;
use console_script::{MAX_SCRIPT_BYTES, Script, ScriptError};
use std::path::Path;

/// Why the console script cannot be used. The messages state the cause, then the remedy; the
/// paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum ScriptFileError {
    #[error(
        "cannot read the console script {path}: {detail}; check the path and its read permissions"
    )]
    Read { path: String, detail: String },
    #[error("the console script {path} is not a regular file; give the path of the file itself")]
    NotAFile { path: String },
    #[error(
        "the console script {path} is {len} bytes long, more than {MAX_SCRIPT_BYTES}; split it, or send less"
    )]
    TooLong { path: String, len: u64 },
    #[error(
        "the console script {path} held more than {MAX_SCRIPT_BYTES} bytes when it was read (it grew while it was being read); give a file that nothing else writes"
    )]
    Grew { path: String },
    #[error("the console script {path}, {source}")]
    Refused {
        path: String,
        #[source]
        source: ScriptError,
    },
}

/// The console script in the file `path`, read whole (at most [`MAX_SCRIPT_BYTES`] bytes, of
/// which no more than one more byte is read) and checked.
///
/// # Errors
/// [`ScriptFileError`]: the file cannot be read, or what it holds is no console script.
pub fn read_script(path: &Path) -> Result<Script, ScriptFileError> {
    let shown = || sanitize(&path.display().to_string());
    // INVARIANT: a mebibyte fits a `u32`.
    let limit = u32::try_from(MAX_SCRIPT_BYTES).unwrap_or(u32::MAX);
    let bytes = read_regular_file(path, limit).map_err(|failure| match failure {
        ReadFailure::Io(e) => ScriptFileError::Read {
            path: shown(),
            detail: sanitize(&e.to_string()),
        },
        ReadFailure::NotAFile => ScriptFileError::NotAFile { path: shown() },
        ReadFailure::TooLong(len) => ScriptFileError::TooLong { path: shown(), len },
        ReadFailure::Grew(_) => ScriptFileError::Grew { path: shown() },
    })?;
    Script::parse(&bytes).map_err(|source| ScriptFileError::Refused {
        path: shown(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_script::VERSION_LINE;

    #[test]
    fn a_script_file_is_read_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steps.txt");
        std::fs::write(&path, format!("{VERSION_LINE}\nsend uart0 \"x\"\n")).unwrap();
        assert_eq!(read_script(&path).unwrap().steps().len(), 1);
    }

    #[test]
    fn every_variant_names_the_file_and_states_its_cause_then_its_remedy() {
        let path = || "steps.txt".to_owned();
        let refused = Script::parse(b"inkemu-console-script 9\n").unwrap_err();
        for error in [
            ScriptFileError::Read {
                path: path(),
                detail: "permission denied".to_owned(),
            },
            ScriptFileError::NotAFile { path: path() },
            ScriptFileError::TooLong {
                path: path(),
                len: 2_000_000,
            },
            // A file that grew while it was read cannot be made in a test without a second
            // writer racing the reader: the variant is built.
            ScriptFileError::Grew { path: path() },
            ScriptFileError::Refused {
                path: path(),
                source: refused,
            },
        ] {
            let message = error.to_string();
            let (cause, remedy) = message
                .rsplit_once("; ")
                .unwrap_or_else(|| panic!("no remedy after a semicolon: {message}"));
            assert!(cause.contains("steps.txt"), "{message}");
            assert!(remedy.len() > 10, "{message}");
        }
        assert_eq!(
            ScriptFileError::Grew { path: path() }.to_string(),
            "the console script steps.txt held more than 1048576 bytes when it was read (it grew while it was being read); give a file that nothing else writes"
        );
    }

    #[test]
    fn each_refusal_names_the_file_and_states_its_remedy() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.txt");
        std::fs::write(&bad, format!("{VERSION_LINE}\nsend uart9 \"x\"\n")).unwrap();
        let refused = read_script(&bad).unwrap_err().to_string();
        assert!(
            refused.ends_with(
                "bad.txt, line 2, column 6: `uart9` is no channel; write uart0, uart1 or usb-serial-jtag"
            ),
            "{refused}"
        );
        let long = dir.path().join("long.txt");
        std::fs::File::create(&long)
            .unwrap()
            .set_len(u64::try_from(MAX_SCRIPT_BYTES).unwrap() + 1)
            .unwrap();
        let too_long = read_script(&long).unwrap_err().to_string();
        assert!(
            too_long.contains("is 1048577 bytes long, more than 1048576; "),
            "{too_long}"
        );
        let directory = read_script(dir.path()).unwrap_err().to_string();
        assert!(directory.contains("is not a regular file; "), "{directory}");
        let missing = read_script(&dir.path().join("missing.txt"))
            .unwrap_err()
            .to_string();
        assert!(
            missing.starts_with("cannot read the console script "),
            "{missing}"
        );
        assert!(
            missing.ends_with("; check the path and its read permissions"),
            "{missing}"
        );
    }
}
