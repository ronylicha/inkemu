//! The file of a session: `inkemu run --record` writes it, `inkemu replay` reads it. The format,
//! its checks and the replay belong to the `session` crate, so that a front end gets the same
//! answers; this module only reads the file, within the size a session may have, and writes it.
use crate::flash_file::{ReadFailure, read_regular_file};
use crate::flash_state::{same_file, temporary_path};
use crate::term::sanitize;
use session::{MAX_SESSION_BYTES, ReadError, Session};
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
    /// `detail` is what the [`ReadError`] says, its control characters escaped: a refusal can
    /// quote the file (a line of the text of its profile), and a session comes from anywhere.
    #[error("the session {path} is refused: {detail}")]
    Refused { path: String, detail: String },
    #[error("--record names {path}, which is {what}; give --record another path")]
    Claimed { path: String, what: String },
    #[error(
        "--record names {path}, whose session is written first to {temporary}, which is {what}; give --record another path"
    )]
    TemporaryClaimed {
        path: String,
        temporary: String,
        what: String,
    },
    #[error(
        "--record names {path}, which is a directory or a link, not a file; give the path of the file to write"
    )]
    NotAFileToWrite { path: String },
    #[error("--record names no file: `{path}`; give the path of the file to write")]
    NoFileName { path: String },
    #[error(
        "--record names {path}, in a directory that does not exist; create it, or give --record a path in a directory that exists"
    )]
    NoDirectory { path: String },
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
    Session::from_bytes(&bytes).map_err(|refusal: ReadError| SessionFileError::Refused {
        path: shown(path),
        detail: sanitize(&refusal.to_string()),
    })
}

/// A path the run reads or writes, which a session must not be written to: a file, or every file
/// of a directory. `what` says which, and what the run does with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claimed {
    File { path: PathBuf, what: String },
    Directory { path: PathBuf, what: String },
}

impl Claimed {
    /// Whether `path` is this file or a file of this directory, whether it exists or not.
    fn holds(&self, path: &Path) -> bool {
        match self {
            Claimed::File { path: file, .. } => same_place(path, file),
            Claimed::Directory { path: dir, .. } => directory_of(path)
                .zip(std::fs::canonicalize(dir).ok())
                .is_some_and(|(one, other)| one == other),
        }
    }

    fn what(&self) -> &str {
        match self {
            Claimed::File { what, .. } | Claimed::Directory { what, .. } => what,
        }
    }
}

/// The directory `path` is in, every link in it resolved; `None` when it does not exist.
fn directory_of(path: &Path) -> Option<PathBuf> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    std::fs::canonicalize(parent).ok()
}

/// Whether the two paths lead to one file, by whatever route, or would once it is created: the
/// same name in the same directory, every link in the directories resolved. The flash file of a
/// first run does not exist yet, and a session written at its path would replace it at the end.
pub(crate) fn same_place(one: &Path, other: &Path) -> bool {
    if same_file(one, other) {
        return true;
    }
    let resolved = |path: &Path| Some(directory_of(path)?.join(path.file_name()?));
    resolved(one)
        .zip(resolved(other))
        .is_some_and(|(one, other)| one == other)
}

/// Refuses, before a run, a `--record` path the run could not write a session to, or that names
/// a path the run reads or writes (`claimed`), the session or the temporary file it is written
/// through: whether that path exists yet or not.
///
/// # Errors
/// [`SessionFileError::NoFileName`] when the path names no file,
/// [`SessionFileError::NotAFileToWrite`] when a directory or a link is at the path,
/// [`SessionFileError::NoDirectory`] when its directory does not exist,
/// [`SessionFileError::Claimed`] and [`SessionFileError::TemporaryClaimed`] when the path or its
/// temporary file is one of `claimed`.
pub fn refuse_record_path(path: &Path, claimed: &[Claimed]) -> Result<(), SessionFileError> {
    check_output_path(path, claimed).map_err(|refusal| match refusal {
        OutputRefusal::NoFileName => SessionFileError::NoFileName { path: shown(path) },
        OutputRefusal::NotAFileToWrite => SessionFileError::NotAFileToWrite { path: shown(path) },
        OutputRefusal::NoDirectory => SessionFileError::NoDirectory { path: shown(path) },
        OutputRefusal::Claimed { what } => SessionFileError::Claimed {
            path: shown(path),
            what,
        },
        OutputRefusal::TemporaryClaimed { temporary, what } => SessionFileError::TemporaryClaimed {
            path: shown(path),
            temporary,
            what,
        },
    })
}

/// Why a path cannot take a file the run writes beside its own (a session, a capture of the
/// screen); each writer words it for its user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputRefusal {
    /// The path names no file (it ends in a separator, or is empty).
    NoFileName,
    /// A directory or a link stands at the path.
    NotAFileToWrite,
    /// The directory of the path does not exist.
    NoDirectory,
    /// The path is one the run reads or writes; `what` says which.
    Claimed { what: String },
    /// The temporary file the path is written through is one the run reads or writes.
    TemporaryClaimed { temporary: String, what: String },
}

/// Checks that a file written at `path` through its temporary file ([`write_through_temporary`])
/// replaces nothing in `claimed`, existing or not, and that it can be created there.
///
/// # Errors
/// [`OutputRefusal`] with the first reason found.
pub fn check_output_path(path: &Path, claimed: &[Claimed]) -> Result<(), OutputRefusal> {
    let ends_in_a_separator = path
        .as_os_str()
        .to_string_lossy()
        .ends_with(std::path::is_separator);
    let Some(temporary) = temporary_path(path).filter(|_| !ends_in_a_separator) else {
        return Err(OutputRefusal::NoFileName);
    };
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && !metadata.is_file()
    {
        return Err(OutputRefusal::NotAFileToWrite);
    }
    if directory_of(path).is_none() {
        return Err(OutputRefusal::NoDirectory);
    }
    for claim in claimed {
        if claim.holds(path) {
            return Err(OutputRefusal::Claimed {
                what: claim.what().to_owned(),
            });
        }
        if claim.holds(&temporary) {
            return Err(OutputRefusal::TemporaryClaimed {
                temporary: shown(&temporary),
                what: claim.what().to_owned(),
            });
        }
    }
    Ok(())
}

/// Writes `bytes` to `path` through a temporary file beside it, flushed to the disk then renamed:
/// the path holds the file of before or the new one, never part of one. On Unix a file created
/// here has `mode`, less what the process's umask removes.
///
/// # Errors
/// What the system said, its control characters escaped.
pub fn write_through_temporary(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let temporary = temporary_path(path).ok_or_else(|| "the path names no file".to_owned())?;
    // A link or anything else at the temporary name is removed, never followed.
    if std::fs::symlink_metadata(&temporary).is_ok() {
        std::fs::remove_file(&temporary).map_err(|e| sanitize(&e.to_string()))?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let written = options.open(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(e) = written.and_then(|()| std::fs::rename(&temporary, path)) {
        let _ = std::fs::remove_file(&temporary);
        return Err(sanitize(&e.to_string()));
    }
    Ok(())
}

/// Writes the session `bytes` to `path` ([`write_through_temporary`]). A session holds the whole
/// flash, what the firmware keeps among it (network credentials): on Unix a file created here is
/// readable and writable by its owner only (ADR 0004 point 5).
///
/// # Errors
/// [`SessionFileError::Write`] with what the system said.
pub fn write_session(path: &Path, bytes: &[u8]) -> Result<(), SessionFileError> {
    write_through_temporary(path, bytes, 0o600).map_err(|detail| SessionFileError::Write {
        path: shown(path),
        detail,
    })
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

    fn file(path: &Path, what: &str) -> Claimed {
        Claimed::File {
            path: path.to_path_buf(),
            what: what.to_owned(),
        }
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
        let error = refuse_record_path(&image, &[file(&image, "the file of --flash")])
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("flash.bin, which is the file of --flash; give --record another path"),
            "{error}"
        );
        let new = dir.path().join("new.inkrec");
        refuse_record_path(&new, &[file(&image, "the file of --flash")]).unwrap();
    }

    #[test]
    fn a_record_path_is_refused_as_the_flash_file_and_its_own_files_before_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        let flash = dir.path().join("x4.bin");
        let claimed: Vec<Claimed> = crate::flash_state::files_of_the_flash_file(&flash)
            .into_iter()
            .map(|(path, what)| file(&path, what))
            .collect();
        // Another spelling of the same place: through `.` in the directory.
        let spelled = dir.path().join(".").join("x4.bin");
        for (path, what) in [
            (spelled, "the flash file of --flash-file"),
            (
                dir.path().join("x4.bin.lock"),
                "the lock file of --flash-file",
            ),
            (
                dir.path().join(".x4.bin.tmp"),
                "the temporary file of --flash-file",
            ),
        ] {
            let error = refuse_record_path(&path, &claimed).unwrap_err().to_string();
            assert!(error.contains(&format!(", which is {what}")), "{error}");
        }
        // Beside the flash file under another name, the session and its temporary file are its
        // own.
        refuse_record_path(&dir.path().join("x4.inkrec"), &claimed).unwrap();
        assert!(names_in(dir.path()).is_empty(), "nothing is created");
    }

    #[test]
    fn a_record_path_whose_temporary_file_is_an_input_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join(".run.inkrec.tmp");
        std::fs::write(&script, b"script").unwrap();
        let error = refuse_record_path(
            &dir.path().join("run.inkrec"),
            &[file(&script, "the file of --console-script")],
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("run.inkrec, whose session is written first to ")
                && error.ends_with(
                    ".run.inkrec.tmp, which is the file of --console-script; give --record another path"
                ),
            "{error}"
        );
    }

    #[test]
    fn a_record_path_in_a_claimed_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Claimed::Directory {
            path: dir.path().to_path_buf(),
            what: "in the ROM cache".to_owned(),
        };
        assert!(matches!(
            refuse_record_path(&dir.path().join("run.inkrec"), std::slice::from_ref(&cache)),
            Err(SessionFileError::Claimed { .. })
        ));
        std::fs::create_dir(dir.path().join("sessions")).unwrap();
        refuse_record_path(&dir.path().join("sessions").join("run.inkrec"), &[cache]).unwrap();
    }

    #[test]
    fn a_record_path_that_names_no_file_or_no_directory_is_refused_before_the_run() {
        let dir = tempfile::tempdir().unwrap();
        for path in [PathBuf::new(), dir.path().join("run.inkrec/")] {
            assert!(
                matches!(
                    refuse_record_path(&path, &[]),
                    Err(SessionFileError::NoFileName { .. })
                ),
                "{}",
                path.display()
            );
        }
        let error = refuse_record_path(&dir.path().join("runs").join("s.inkrec"), &[])
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with(
                "s.inkrec, in a directory that does not exist; create it, or give --record a path in a directory that exists"
            ),
            "{error}"
        );
        // A bare name is in the current directory, which exists.
        refuse_record_path(Path::new("run.inkrec"), &[]).unwrap();
    }

    fn names_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
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
