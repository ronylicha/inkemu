//! The file of a session: `inkemu run --record` writes it, `inkemu replay` reads it. The format,
//! its checks and the replay belong to the `session` crate, so that a front end gets the same
//! answers; this module only reads the file, within the size a session may have, and writes it.
use crate::flash_file::{ReadFailure, read_regular_file};
use crate::flash_state::same_file;
use crate::term::sanitize;
use session::{MAX_SESSION_BYTES, ReadError, Session};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Why a session file cannot be read or written. The messages state the cause, then the remedy;
/// the paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum SessionFileError {
    #[error("cannot read the session {path}: {detail}; check the path and its read permissions")]
    Read { path: String, detail: String },
    #[error("the session {path} is not a regular file; give the path of the file itself")]
    NotAFile { path: String },
    #[error(
        "the session {path} is {len} bytes long, more than any session ({MAX_SESSION_BYTES}); it is not a session of inkemu"
    )]
    TooLong { path: String, len: u64 },
    #[error(
        "the session {path} held more than {MAX_SESSION_BYTES} bytes when it was read (it grew while it was being read); give a file that nothing else writes"
    )]
    Grew { path: String },
    #[error("the session {path} is refused: {source}")]
    Refused {
        path: String,
        #[source]
        source: ReadError,
    },
    #[error(
        "--record names {path}, which is also the file of {option}: the run would replace it with its session; give --record another path"
    )]
    SameFile { path: String, option: &'static str },
    #[error(
        "--record names {path}, which is a directory or a link, not a file; give the path of the file to write"
    )]
    NotAFileToWrite { path: String },
    #[error(
        "cannot write the session {path}: {detail}; the run is not recorded: check that the directory exists and is writable"
    )]
    Write { path: String, detail: String },
}

fn shown(path: &Path) -> String {
    sanitize(&path.display().to_string())
}

/// The session in the file `path`, read whole (at most [`MAX_SESSION_BYTES`] bytes, of which no
/// more than one more byte is read) and checked.
///
/// # Errors
/// [`SessionFileError`]: the file cannot be read, or what it holds is no session this inkemu
/// reads.
pub fn read_session(path: &Path) -> Result<Session, SessionFileError> {
    // INVARIANT: the longest session, under 64 MiB, fits a `u32`.
    let limit = u32::try_from(MAX_SESSION_BYTES).unwrap_or(u32::MAX);
    let bytes = read_regular_file(path, limit).map_err(|failure| match failure {
        ReadFailure::Io(e) => SessionFileError::Read {
            path: shown(path),
            detail: sanitize(&e.to_string()),
        },
        ReadFailure::NotAFile => SessionFileError::NotAFile { path: shown(path) },
        ReadFailure::TooLong(len) => SessionFileError::TooLong {
            path: shown(path),
            len,
        },
        ReadFailure::Grew(_) => SessionFileError::Grew { path: shown(path) },
    })?;
    Session::from_bytes(&bytes).map_err(|source| SessionFileError::Refused {
        path: shown(path),
        source,
    })
}

/// Refuses, before a run, a `--record` path the run could not write a session to, or that names
/// a file the run reads: `inputs`, each with its option. A path that leads to nothing yet is
/// fine; the write at the end says what goes wrong then.
///
/// # Errors
/// [`SessionFileError::NotAFileToWrite`] when a directory or a link is at the path,
/// [`SessionFileError::SameFile`] when the path leads to one of the inputs.
pub fn refuse_record_path(
    path: &Path,
    inputs: &[(&'static str, Option<&Path>)],
) -> Result<(), SessionFileError> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && !metadata.is_file()
    {
        return Err(SessionFileError::NotAFileToWrite { path: shown(path) });
    }
    for (option, input) in inputs {
        if let Some(input) = input
            && same_file(path, input)
        {
            return Err(SessionFileError::SameFile {
                path: shown(path),
                option,
            });
        }
    }
    Ok(())
}

/// The name the session is written to before it takes the name `path`: `.<name>.tmp` beside it.
fn temporary_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let mut temporary = OsString::from(".");
    temporary.push(name);
    temporary.push(".tmp");
    Some(path.with_file_name(temporary))
}

/// Writes `bytes` to `path` through a temporary file beside it, flushed to the disk then renamed:
/// the path holds the session of before or the new one, never part of one. A session holds the
/// whole flash, what the firmware keeps among it (network credentials): on Unix a file created
/// here is readable and writable by its owner only (ADR 0004 point 5).
///
/// # Errors
/// [`SessionFileError::Write`] with what the system said.
pub fn write_session(path: &Path, bytes: &[u8]) -> Result<(), SessionFileError> {
    let failed = |detail: String| SessionFileError::Write {
        path: shown(path),
        detail: sanitize(&detail),
    };
    let temporary =
        temporary_path(path).ok_or_else(|| failed("the path names no file".to_owned()))?;
    // A link or anything else at the temporary name is removed, never followed.
    if std::fs::symlink_metadata(&temporary).is_ok() {
        std::fs::remove_file(&temporary).map_err(|e| failed(e.to_string()))?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = options.open(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(e) = written.and_then(|()| std::fs::rename(&temporary, path)) {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(e.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_written_through_its_temporary_file_and_replaces_the_one_before() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.inkrec");
        write_session(&path, b"first").unwrap();
        write_session(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(!dir.path().join(".run.inkrec.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        }
    }

    #[test]
    fn what_sits_at_the_temporary_name_is_removed_and_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.inkrec");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, b"keep").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(".run.inkrec.tmp")).unwrap();
        write_session(&path, b"session").unwrap();
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"keep");
        assert_eq!(std::fs::read(&path).unwrap(), b"session");
    }

    #[test]
    fn a_session_that_cannot_be_written_says_so_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("run.inkrec");
        let error = write_session(&path, b"x").unwrap_err().to_string();
        assert!(error.starts_with("cannot write the session "), "{error}");
        assert!(
            error.ends_with(
                "; the run is not recorded: check that the directory exists and is writable"
            ),
            "{error}"
        );
    }

    #[test]
    fn a_record_path_that_is_a_directory_or_an_input_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            refuse_record_path(dir.path(), &[]),
            Err(SessionFileError::NotAFileToWrite { .. })
        ));
        let image = dir.path().join("flash.bin");
        std::fs::write(&image, b"image").unwrap();
        assert!(matches!(
            refuse_record_path(&image, &[("--flash-file", None), ("--flash", Some(&image))]),
            Err(SessionFileError::SameFile {
                option: "--flash",
                ..
            })
        ));
        let new = dir.path().join("new.inkrec");
        refuse_record_path(&new, &[("--flash", Some(&image))]).unwrap();
    }

    #[test]
    fn a_file_that_is_no_session_is_refused_with_its_cause() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not.inkrec");
        std::fs::write(&path, b"hello").unwrap();
        let error = read_session(&path).unwrap_err().to_string();
        assert!(
            error.ends_with(
                "is refused: the file does not begin with `inkemu-session 1`; it is not a session of inkemu"
            ),
            "{error}"
        );
        assert!(matches!(
            read_session(dir.path()),
            Err(SessionFileError::NotAFile { .. })
        ));
        assert!(matches!(
            read_session(&dir.path().join("absent")),
            Err(SessionFileError::Read { .. })
        ));
    }

    #[test]
    fn a_file_longer_than_any_session_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.inkrec");
        let file = std::fs::File::create(&path).unwrap();
        // A sparse file: its length is all that is looked at.
        file.set_len(MAX_SESSION_BYTES as u64 + 1).unwrap();
        assert!(matches!(
            read_session(&path),
            Err(SessionFileError::TooLong { .. })
        ));
    }
}
