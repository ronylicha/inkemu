//! The flash file of a device (`inkemu run --flash-file`): the content of its flash chip, kept
//! from one run to the next (spec §5.1).
//!
//! The file is the chip as bytes and nothing else: no header, exactly the size of the flash. A
//! run reads it when it starts and replaces it when it ends; a lock file beside it keeps two runs
//! from using it at the same time. The machine knows nothing of it (it performs no host I/O):
//! this module turns a file into the bytes a machine is built from, and the bytes a machine hands
//! over into a file.
use crate::term::{hex, sanitize};
use sha2::{Digest, Sha256};
use soc_esp32c3::Esp32C3;
use soc_esp32c3::flash::ERASED;
use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// What is appended to the name of a flash file to name its lock file.
pub const LOCK_SUFFIX: &str = ".lock";

/// Why a flash file cannot be used, read or written. The messages state the cause, then the
/// remedy; the paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum FlashStateError {
    #[error("the flash file path {path} names no file; give the path of a file, like flash-x4.bin")]
    NoFileName { path: String },
    #[error(
        "cannot create the lock file {lock} beside the flash file {path}: {detail}; check that the directory exists and is writable, or give a path in one that is"
    )]
    LockCreate {
        path: String,
        lock: String,
        detail: String,
    },
    #[error(
        "the flash file {path} is in use by another inkemu run, which holds the lock {lock}; wait for that run to end, or give this run another --flash-file"
    )]
    InUse { path: String, lock: String },
    #[error(
        "cannot lock the flash file {path} with {lock}: {detail}; give a path on a filesystem that supports file locks"
    )]
    Lock {
        path: String,
        lock: String,
        detail: String,
    },
    #[error(
        "the flash file {path} is not a regular file (a link is not followed: the file is replaced when the run ends, not written through); give the path of the file itself, or a new path"
    )]
    NotAFile { path: String },
    #[error("cannot read the flash file {path}: {detail}; check its read permissions")]
    Read { path: String, detail: String },
    #[error(
        "the flash file {path} is {len} bytes and the flash of the device is {size} bytes: the file is not the flash of this device, and it is never padded or cut; use it with the device it was written for, pass a firmware image with --flash instead, or give a new path"
    )]
    WrongSize { path: String, len: u64, size: u32 },
    #[error(
        "the flash file {path} changed while it was being read: it held {size} bytes, then {found}; make sure nothing else writes it, then run again"
    )]
    Changed {
        path: String,
        size: u32,
        found: String,
    },
    #[error(
        "both --flash and the flash file {path}, which exists, were given: the run cannot start from two flashes; drop --flash to go on from the flash file, or add --reset-flash to start again from the image and replace the file"
    )]
    BothGiven { path: String },
    #[error(
        "cannot write the flash file {path}: cannot {action} {target}: {detail}; what this run wrote to the flash is lost; check that the directory still exists, is writable and has {size} bytes free, then run again"
    )]
    Write {
        path: String,
        action: &'static str,
        target: String,
        detail: String,
        size: u32,
    },
}

fn shown(path: &Path) -> String {
    sanitize(&path.display().to_string())
}

/// The SHA-256, in hexadecimal, of a flash of `size` bytes that holds `image` from offset 0 and is
/// erased after it: what `sha256sum` says of the flash file of that content. Bytes of `image`
/// past `size` are not part of the flash and are left out.
///
/// The erased part is fed 4 KiB at a time: at most `size / 4096` rounds, and `size` is the flash
/// size of a device profile, 16 MiB at most.
#[must_use]
pub fn content_sha256(image: &[u8], size: u32) -> String {
    const ERASED_CHUNK: [u8; 4096] = [ERASED; 4096];
    // INVARIANT: a `u32` fits in `usize` on the hosts the CLI builds for; on one where it did
    // not, the whole image would be hashed and nothing added.
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    let held = image.get(..size).unwrap_or(image);
    let mut hasher = Sha256::new();
    hasher.update(held);
    let mut erased = size.saturating_sub(held.len());
    while erased > 0 {
        let round = ERASED_CHUNK.get(..erased).unwrap_or(&ERASED_CHUNK);
        hasher.update(round);
        erased -= round.len();
    }
    hex(&hasher.finalize())
}

/// Reads `source` to its end when it holds exactly `size` bytes; `Err(found)` says how many it
/// held otherwise. Never more than `size + 1` bytes are read, whatever the source is.
fn read_exactly(source: impl Read, size: u32) -> std::io::Result<Result<Vec<u8>, u64>> {
    let mut bytes = Vec::new();
    source.take(u64::from(size) + 1).read_to_end(&mut bytes)?;
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    Ok(if len == u64::from(size) {
        Ok(bytes)
    } else {
        Err(len)
    })
}

/// The flash file of one run, locked against other runs for as long as this value lives.
#[derive(Debug)]
pub struct FlashState {
    path: PathBuf,
    /// The file the content is written to before it takes the name of the flash file.
    temporary: PathBuf,
    shown: String,
    size: u32,
    /// The lock file, locked. Closing it releases the lock: at the end of the run, or when the
    /// process dies, whatever kills it. The file itself is never removed: removing it would let
    /// a run that already opened it and a run that creates it anew both hold "the" lock.
    _lock: File,
}

impl FlashState {
    /// Takes the flash file at `path` for a run of a device whose flash is `size` bytes: creates
    /// the lock file beside it if there is none, and locks it. Nothing of the flash file itself
    /// is read, created or changed.
    ///
    /// # Errors
    /// [`FlashStateError`] when the path names no file, the lock file cannot be created, or
    /// another run holds the lock.
    pub fn open(path: &Path, size: u32) -> Result<FlashState, FlashStateError> {
        let shown_path = shown(path);
        let Some(name) = path.file_name() else {
            return Err(FlashStateError::NoFileName { path: shown_path });
        };
        let sibling = |prefix: &str, suffix: &str| {
            let mut sibling = OsString::from(prefix);
            sibling.push(name);
            sibling.push(suffix);
            path.with_file_name(sibling)
        };
        let lock_path = sibling("", LOCK_SUFFIX);
        let shown_lock = shown(&lock_path);
        // Opened for writing so that the directory is known to be writable now, not at the end
        // of the run; nothing is ever written to it, and an existing one is not truncated.
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| FlashStateError::LockCreate {
                path: shown_path.clone(),
                lock: shown_lock.clone(),
                detail: sanitize(&e.to_string()),
            })?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(FlashStateError::InUse {
                    path: shown_path,
                    lock: shown_lock,
                });
            }
            Err(TryLockError::Error(e)) => {
                return Err(FlashStateError::Lock {
                    path: shown_path,
                    lock: shown_lock,
                    detail: sanitize(&e.to_string()),
                });
            }
        }
        Ok(FlashState {
            temporary: sibling(".", &format!(".{}.tmp", std::process::id())),
            path: path.to_path_buf(),
            shown: shown_path,
            size,
            _lock: lock,
        })
    }

    /// The path of the flash file, with its control characters escaped: for messages.
    #[must_use]
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// Whether the flash file exists. One that exists must be a flash of the device: a regular
    /// file of exactly the size of its flash.
    ///
    /// # Errors
    /// [`FlashStateError`] when what is at the path is not a regular file (a link is not
    /// followed), has another size, or cannot be looked at.
    pub fn exists(&self) -> Result<bool, FlashStateError> {
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => self.check(&metadata).map(|()| true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(self.read_error(&e)),
        }
    }

    /// The content of the flash file: exactly the bytes of the flash of the device. The size is
    /// judged before anything is read, and no more than one byte past it is ever read.
    ///
    /// # Errors
    /// [`FlashStateError`] when the file cannot be read, is not a regular file of the size of
    /// the flash, or changes while it is read.
    pub fn load(&self) -> Result<Vec<u8>, FlashStateError> {
        let file = File::open(&self.path).map_err(|e| self.read_error(&e))?;
        // What was opened is judged, not what the path named a moment ago.
        self.check(&file.metadata().map_err(|e| self.read_error(&e))?)?;
        read_exactly(file, self.size)
            .map_err(|e| self.read_error(&e))?
            .map_err(|found| FlashStateError::Changed {
                path: self.shown.clone(),
                size: self.size,
                found: if found > u64::from(self.size) {
                    "more".to_owned()
                } else {
                    format!("{found}")
                },
            })
    }

    /// Replaces the flash file with `content`, the whole flash of the device. The bytes go to a
    /// temporary file in the same directory, are flushed to the disk, and only then take the name
    /// of the flash file: whatever interrupts this, the path names the old content or the new
    /// one, never a part of either.
    ///
    /// # Errors
    /// [`FlashStateError::Write`], which says what could not be done to which file; the flash
    /// file is then the one of before, and no temporary file is left.
    pub fn save(&self, content: &[u8]) -> Result<(), FlashStateError> {
        let failed = |action: &'static str, target: &Path, e: &std::io::Error| {
            // Best effort: the failure being reported is the one worth knowing.
            let _ = std::fs::remove_file(&self.temporary);
            FlashStateError::Write {
                path: self.shown.clone(),
                action,
                target: shown(target),
                detail: sanitize(&e.to_string()),
                size: self.size,
            }
        };
        // `create_new` never follows a link and never reuses a file planted at the name. A
        // leftover of a killed run is removed (a link itself, never its target), once.
        let create = || {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.temporary)
        };
        let mut file = match create() {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                std::fs::remove_file(&self.temporary)
                    .map_err(|e| failed("remove", &self.temporary, &e))?;
                create()
            }
            other => other,
        }
        .map_err(|e| failed("create", &self.temporary, &e))?;
        file.write_all(content)
            .and_then(|()| file.sync_all())
            .map_err(|e| failed("write", &self.temporary, &e))?;
        drop(file);
        std::fs::rename(&self.temporary, &self.path)
            .map_err(|e| failed("move into place", &self.path, &e))
    }

    fn check(&self, metadata: &std::fs::Metadata) -> Result<(), FlashStateError> {
        if !metadata.is_file() {
            return Err(FlashStateError::NotAFile {
                path: self.shown.clone(),
            });
        }
        if metadata.len() != u64::from(self.size) {
            return Err(FlashStateError::WrongSize {
                path: self.shown.clone(),
                len: metadata.len(),
                size: self.size,
            });
        }
        Ok(())
    }

    fn read_error(&self, error: &std::io::Error) -> FlashStateError {
        FlashStateError::Read {
            path: self.shown.clone(),
            detail: sanitize(&error.to_string()),
        }
    }
}

/// The flash of one run of `inkemu run`: what it starts from, and what becomes of it when the
/// run ends.
///
/// The content a run starts from is an input of the run, as the ROM and the device profile are:
/// its SHA-256 is said in the summary. Writing the flash file is not part of the run: it happens
/// after the last instruction, and enters no trace.
#[derive(Debug)]
pub struct RunFlash {
    /// The flash file, locked, when the run was given one.
    state: Option<FlashState>,
    /// Whether the run starts from the content the flash file holds: the file then only has to
    /// be written if the run modified the flash.
    from_file: bool,
    start_sha256: String,
    size: u32,
}

impl RunFlash {
    /// Decides what a run starts from and takes its flash file, before any machine exists.
    ///
    /// - No flash file: the run starts from `seed` (the `--flash` image) or from an erased
    ///   flash, and nothing is kept when it ends.
    /// - A flash file that exists: the run goes on from it. `seed` is then refused, since the
    ///   run cannot start from both, unless `reset` asks to start again from the seed (or from
    ///   an erased flash) and to replace the file.
    /// - A flash file that does not exist yet: the run starts from `seed` or from an erased
    ///   flash, and the file is created when it ends.
    ///
    /// Returns the content to build the machine from; `None` is an erased flash.
    ///
    /// # Errors
    /// [`FlashStateError`]: the flash file cannot be used (see [`FlashState::open`],
    /// [`FlashState::exists`], [`FlashState::load`]), or it exists and `seed` was given without
    /// `reset`.
    pub fn begin(
        flash_file: Option<&Path>,
        reset: bool,
        seed: Option<Vec<u8>>,
        size: u32,
    ) -> Result<(RunFlash, Option<Vec<u8>>), FlashStateError> {
        let state = flash_file
            .map(|path| FlashState::open(path, size))
            .transpose()?;
        // An existing file is judged even when it is about to be replaced: a file of another
        // size is not a flash of this device, and `--reset-flash` must not destroy it.
        let from_file = match &state {
            Some(state) if state.exists()? => {
                if !reset && seed.is_some() {
                    return Err(FlashStateError::BothGiven {
                        path: state.shown.clone(),
                    });
                }
                !reset
            }
            _ => false,
        };
        let content = match &state {
            Some(state) if from_file => Some(state.load()?),
            _ => seed,
        };
        let start_sha256 = content_sha256(content.as_deref().unwrap_or_default(), size);
        Ok((
            RunFlash {
                state,
                from_file,
                start_sha256,
                size,
            },
            content,
        ))
    }

    /// The flash file of the run, when it has one.
    #[must_use]
    pub const fn state(&self) -> Option<&FlashState> {
        self.state.as_ref()
    }

    /// Whether the run starts from the content of its flash file.
    #[must_use]
    pub const fn starts_from_the_file(&self) -> bool {
        self.from_file
    }

    /// Ends the run for its flash: says in `log` what it started from and what became of it,
    /// and writes the flash file when there is one and it does not already hold the content
    /// (it did not exist, it is being replaced, or the run modified the flash).
    ///
    /// Returns `false` when the flash file had to be written and could not be: the error is in
    /// `log`, and what the run wrote to the flash is lost. A failing log writer does not keep
    /// the file from being written, nor change the answer.
    pub fn finish(&self, machine: &Esp32C3, log: &mut dyn Write) -> bool {
        let modified = machine.flash_modified_sectors();
        let _ = writeln!(log, "inkemu: flash at start: sha256 {}", self.start_sha256);
        let Some(state) = &self.state else {
            if modified > 0 {
                let _ = writeln!(
                    log,
                    "inkemu: flash at end: {modified} of its 4 KiB sectors were modified and are not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next"
                );
            }
            return true;
        };
        if self.from_file && modified == 0 {
            let _ = writeln!(
                log,
                "inkemu: flash at end: not modified; {} is left as it is",
                state.shown()
            );
            return true;
        }
        let content = machine.flash_contents();
        match state.save(&content) {
            Ok(()) => {
                let _ = writeln!(
                    log,
                    "inkemu: flash at end: sha256 {}; {modified} of its 4 KiB sectors were modified; saved to {}",
                    content_sha256(&content, self.size),
                    state.shown()
                );
                true
            }
            Err(e) => {
                let _ = writeln!(log, "error: {e}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flash small enough for a test to write many times; the module knows no minimum.
    const SIZE: u32 = 8192;

    fn content(seed: u8) -> Vec<u8> {
        (0..SIZE).map(|i| seed ^ i.to_le_bytes()[0]).collect()
    }

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn opening_creates_the_lock_file_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        assert_eq!(names(dir.path()), ["x4.bin.lock"]);
        assert_eq!(std::fs::read(dir.path().join("x4.bin.lock")).unwrap(), b"");
        assert!(!state.exists().unwrap(), "no flash file yet: a first run");
        assert!(state.shown().ends_with("x4.bin"));
    }

    #[test]
    fn what_is_saved_is_what_the_next_run_loads_and_the_file_is_the_content_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        {
            let state = FlashState::open(&path, SIZE).unwrap();
            state.save(&content(1)).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), content(1), "no header");
            assert_eq!(
                names(dir.path()),
                ["x4.bin", "x4.bin.lock"],
                "no temporary file is left"
            );
            assert!(state.exists().unwrap());
            assert_eq!(state.load().unwrap(), content(1));
            state.save(&content(2)).unwrap();
        }
        let next = FlashState::open(&path, SIZE).unwrap();
        assert!(next.exists().unwrap());
        assert_eq!(next.load().unwrap(), content(2), "the last one saved");
        assert_eq!(
            names(dir.path()),
            ["x4.bin", "x4.bin.lock"],
            "the lock file stays"
        );
    }

    #[test]
    fn a_second_run_on_the_same_file_is_refused_until_the_first_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let first = FlashState::open(&path, SIZE).unwrap();
        let err = FlashState::open(&path, SIZE).unwrap_err();
        assert!(matches!(err, FlashStateError::InUse { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains("x4.bin") && text.contains("x4.bin.lock") && text.contains("; wait for"),
            "{text}"
        );
        // Another file of the same directory is another device: no conflict.
        FlashState::open(&dir.path().join("x3.bin"), SIZE).unwrap();
        // The same file reached by another spelling of its path is the same lock.
        let other_spelling = dir.path().join(".").join("x4.bin");
        assert!(matches!(
            FlashState::open(&other_spelling, SIZE),
            Err(FlashStateError::InUse { .. })
        ));
        drop(first);
        FlashState::open(&path, SIZE).unwrap();
    }

    #[test]
    fn a_file_of_another_size_is_refused_with_both_sizes_and_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        for len in [0, 1, SIZE - 1, SIZE + 1, 2 * SIZE] {
            let bytes = vec![0xA5u8; len as usize];
            std::fs::write(&path, &bytes).unwrap();
            for err in [state.exists().unwrap_err(), state.load().unwrap_err()] {
                assert!(
                    matches!(err, FlashStateError::WrongSize { len: found, size: SIZE, .. } if found == u64::from(len)),
                    "{len}: {err}"
                );
                let text = err.to_string();
                assert!(
                    text.contains(&format!("is {len} bytes"))
                        && text.contains("8192 bytes")
                        && text.contains("never padded or cut")
                        && text.contains("; use it with"),
                    "{text}"
                );
            }
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{len}: untouched");
        }
    }

    #[test]
    fn a_huge_file_is_refused_on_its_size_without_being_read() {
        // A sparse file far larger than any flash: judged from its metadata.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        File::create(&path).unwrap().set_len(1 << 40).unwrap();
        let state = FlashState::open(&path, SIZE).unwrap();
        assert!(matches!(
            state.load(),
            Err(FlashStateError::WrongSize { len, .. }) if len == 1 << 40
        ));
    }

    #[test]
    fn reading_stops_one_byte_past_the_size_and_only_the_exact_size_is_a_content() {
        assert_eq!(read_exactly(std::io::repeat(0xFF), 16).unwrap(), Err(17));
        assert_eq!(read_exactly(&[7u8; 16][..], 16).unwrap(), Ok(vec![7; 16]));
        assert_eq!(read_exactly(&[7u8; 15][..], 16).unwrap(), Err(15));
        assert_eq!(read_exactly(&[7u8; 17][..], 16).unwrap(), Err(17));
        assert_eq!(read_exactly(std::io::empty(), 0).unwrap(), Ok(Vec::new()));
        let text = FlashStateError::Changed {
            path: "x4.bin".into(),
            size: 16,
            found: "more".into(),
        }
        .to_string();
        assert!(
            text.contains("held 16 bytes, then more") && text.contains("; make sure"),
            "{text}"
        );
    }

    #[test]
    fn what_is_not_a_regular_file_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        std::fs::create_dir(&path).unwrap();
        let state = FlashState::open(&path, SIZE).unwrap();
        for err in [state.exists().unwrap_err(), state.load().unwrap_err()] {
            assert!(matches!(err, FlashStateError::NotAFile { .. }), "{err}");
            assert!(err.to_string().contains("; give the path"), "{err}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed_and_what_it_points_to_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("precious.bin");
        std::fs::write(&target, content(9)).unwrap();
        let path = dir.path().join("x4.bin");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let state = FlashState::open(&path, SIZE).unwrap();
        let err = state.exists().unwrap_err();
        assert!(matches!(err, FlashStateError::NotAFile { .. }), "{err}");
        assert!(err.to_string().contains("a link is not followed"), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), content(9));
    }

    #[test]
    fn a_path_without_a_file_name_or_in_a_missing_directory_is_refused_before_the_run() {
        for path in ["", "/", ".."] {
            let err = FlashState::open(Path::new(path), SIZE).unwrap_err();
            assert!(
                matches!(err, FlashStateError::NoFileName { .. }),
                "{path:?}: {err}"
            );
            assert!(err.to_string().contains("; give the path of a file"));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("x4.bin");
        let err = FlashState::open(&path, SIZE).unwrap_err();
        assert!(matches!(err, FlashStateError::LockCreate { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains("x4.bin.lock") && text.contains("; check that the directory exists"),
            "{text}"
        );
        assert_eq!(names(dir.path()), [""; 0], "nothing was created");
    }

    // Unix: a directory that holds an open file (the lock) can be removed there, not everywhere.
    #[cfg(unix)]
    #[test]
    fn a_directory_that_vanished_fails_the_save_by_name_and_says_what_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("flash");
        std::fs::create_dir(&inner).unwrap();
        let state = FlashState::open(&inner.join("x4.bin"), SIZE).unwrap();
        std::fs::remove_dir_all(&inner).unwrap();
        let err = state.save(&content(1)).unwrap_err();
        assert!(
            matches!(
                err,
                FlashStateError::Write {
                    action: "create",
                    size: SIZE,
                    ..
                }
            ),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("cannot write the flash file")
                && text.contains("x4.bin")
                && text.contains("is lost")
                && text.contains("; check that the directory still exists"),
            "{text}"
        );
    }

    #[test]
    fn a_save_that_cannot_take_the_name_leaves_the_old_content_and_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        // A directory took the place of the flash file during the run: the rename fails.
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("kept"), b"old").unwrap();
        let err = state.save(&content(1)).unwrap_err();
        assert!(
            matches!(
                err,
                FlashStateError::Write {
                    action: "move into place",
                    ..
                }
            ),
            "{err}"
        );
        assert_eq!(std::fs::read(path.join("kept")).unwrap(), b"old");
        assert_eq!(
            names(dir.path()),
            ["x4.bin", "x4.bin.lock"],
            "no temporary file"
        );
    }

    #[test]
    fn a_leftover_at_the_temporary_name_is_replaced_and_a_link_planted_there_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        let temporary = dir
            .path()
            .join(format!(".x4.bin.{}.tmp", std::process::id()));
        assert_eq!(state.temporary, temporary);
        std::fs::write(&temporary, b"left by a killed run").unwrap();
        state.save(&content(1)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), content(1));
        #[cfg(unix)]
        {
            let victim = dir.path().join("victim");
            std::fs::write(&victim, b"untouched").unwrap();
            std::os::unix::fs::symlink(&victim, &temporary).unwrap();
            state.save(&content(2)).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), content(2));
            assert_eq!(std::fs::read(&victim).unwrap(), b"untouched");
        }
        assert!(!temporary.exists());
    }

    #[test]
    fn a_file_cut_between_the_look_and_the_read_is_refused_not_padded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        state.save(&content(1)).unwrap();
        assert!(state.exists().unwrap());
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(u64::from(SIZE) - 1)
            .unwrap();
        assert!(matches!(
            state.load(),
            Err(FlashStateError::WrongSize { .. })
        ));
        std::fs::remove_file(&path).unwrap();
        let err = state.load().unwrap_err();
        assert!(matches!(err, FlashStateError::Read { .. }), "{err}");
        assert!(err.to_string().contains("; check its read permissions"));
    }

    #[test]
    fn the_hash_of_a_content_is_the_hash_of_the_flash_it_makes() {
        let image = content(3);
        let half = &image[..4096];
        let mut padded = half.to_vec();
        padded.resize(SIZE as usize, 0xFF);
        assert_eq!(
            content_sha256(half, SIZE),
            hex(&Sha256::digest(&padded)),
            "erased after the image"
        );
        assert_eq!(content_sha256(&image, SIZE), hex(&Sha256::digest(&image)));
        assert_eq!(
            content_sha256(&[], SIZE),
            hex(&Sha256::digest(vec![0xFFu8; SIZE as usize])),
            "an erased flash"
        );
        assert_eq!(
            content_sha256(&image, 100),
            hex(&Sha256::digest(&image[..100])),
            "what is past the flash is not in it"
        );
        // An erased part that is no whole number of rounds, and none at all.
        assert_eq!(
            content_sha256(&[1, 2, 3], 5000),
            hex(&Sha256::digest(
                [&[1u8, 2, 3][..], &[0xFF; 4997][..]].concat()
            ))
        );
        assert_eq!(content_sha256(&[], 0), hex(&Sha256::digest([])));
        // What `sha256sum` says of an erased 16 MiB flash file.
        assert_eq!(
            content_sha256(&[], 16 << 20),
            "dffab0dd410657cb30c7b2fd7f2586a4792e8472e58882b3532581f8111a646d"
        );
    }

    #[test]
    fn a_path_with_control_characters_is_shown_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("\u{1b}[31mred.bin");
        let err = FlashState::open(&path, SIZE).unwrap_err().to_string();
        assert!(!err.contains('\u{1b}'), "{err:?}");
        assert!(err.contains("\\u{1b}[31mred.bin"), "{err}");
    }

    /// The flash of the machines below: 1 MiB, the smallest the model takes, so that a test
    /// writes and hashes little.
    const MIB: u32 = 1 << 20;

    /// A machine of a device with a 1 MiB flash that starts from `content` (`None`: erased) and
    /// runs `program` to its end.
    fn ran(content: Option<Vec<u8>>, program: &[u32]) -> Esp32C3 {
        use soc_esp32c3::test_support::{HANG, flat_rom};
        let config = soc_esp32c3::SocConfig {
            flash: soc_esp32c3::FlashConfig::new(
                MIB,
                soc_esp32c3::JedecId::from_u24(0x9D_6014).unwrap(),
            )
            .unwrap(),
            ..soc_esp32c3::SocConfig::default()
        };
        let mut program = program.to_vec();
        program.push(HANG);
        let rom = flat_rom(&program);
        let mut machine = match content {
            Some(content) => Esp32C3::with_flash(rom, content, &config).unwrap(),
            None => Esp32C3::new(rom, &config),
        };
        machine.run_until(emu_core::Cycles(400));
        machine
    }

    /// A program that changes one byte of the flash sector at 0x3000.
    fn writing(byte: u8) -> Vec<u32> {
        soc_esp32c3::test_support::flash_program(0x3000, &[byte])
    }

    fn finish(flash: &RunFlash, machine: &Esp32C3) -> (bool, String) {
        let mut log = Vec::new();
        let kept = flash.finish(machine, &mut log);
        (kept, String::from_utf8(log).unwrap())
    }

    /// A seed shorter than the flash, as a firmware image is.
    fn seed() -> Vec<u8> {
        vec![0x5A; 0x4000]
    }

    #[test]
    fn without_a_flash_file_the_run_starts_from_the_seed_and_says_what_it_does_not_keep() {
        let (flash, content) = RunFlash::begin(None, false, Some(seed()), MIB).unwrap();
        assert_eq!(content, Some(seed()));
        assert!(flash.state().is_none() && !flash.starts_from_the_file());
        let start = format!(
            "inkemu: flash at start: sha256 {}\n",
            content_sha256(&seed(), MIB)
        );
        // A run that writes nothing has nothing more to say.
        let (kept, log) = finish(&flash, &ran(content.clone(), &[]));
        assert!(kept);
        assert_eq!(log, start);
        let (kept, log) = finish(&flash, &ran(content, &writing(0x00)));
        assert!(kept, "nothing was to be kept");
        assert_eq!(
            log,
            format!(
                "{start}inkemu: flash at end: 1 of its 4 KiB sectors were modified and are not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next\n"
            )
        );
        // No seed: an erased flash, whose hash is the one of a file of erased bytes.
        let (flash, content) = RunFlash::begin(None, false, None, MIB).unwrap();
        assert_eq!(content, None);
        let (_, log) = finish(&flash, &ran(None, &[]));
        assert_eq!(
            log,
            format!(
                "inkemu: flash at start: sha256 {}\n",
                hex(&Sha256::digest(vec![0xFFu8; MIB as usize]))
            )
        );
    }

    #[test]
    fn a_first_run_creates_the_flash_file_and_the_next_one_goes_on_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        // First run: no file yet, the seed, a write.
        let (flash, content) = RunFlash::begin(Some(&path), false, Some(seed()), MIB).unwrap();
        assert_eq!(content, Some(seed()));
        assert!(!flash.starts_from_the_file());
        let machine = ran(content, &writing(0x00));
        let (kept, log) = finish(&flash, &machine);
        assert!(kept);
        let first = std::fs::read(&path).unwrap();
        assert_eq!(first.len(), MIB as usize, "the whole flash, not the seed");
        assert_eq!(first[0x3000], 0x00, "what the run wrote");
        assert_eq!(first[0x3001], 0x5A, "what it started from");
        assert_eq!(first[0x4000], 0xFF, "erased past the seed");
        let first_hash = hex(&Sha256::digest(&first));
        assert!(
            log.ends_with(&format!(
                "inkemu: flash at end: sha256 {first_hash}; 1 of its 4 KiB sectors were modified; saved to {}\n",
                sanitize(&path.display().to_string())
            )),
            "{log}"
        );
        drop(flash);

        // Second run: from the file, and it writes nothing: the file is not touched.
        let (flash, content) = RunFlash::begin(Some(&path), false, None, MIB).unwrap();
        assert!(flash.starts_from_the_file());
        assert_eq!(content.as_deref(), Some(&first[..]));
        let before = identity(&path);
        let (kept, log) = finish(&flash, &ran(content.clone(), &[]));
        assert!(kept);
        assert_eq!(
            log,
            format!(
                "inkemu: flash at start: sha256 {first_hash}\ninkemu: flash at end: not modified; {} is left as it is\n",
                sanitize(&path.display().to_string())
            )
        );
        // A program that leaves every byte as it was (0x00 over 0x00) modified nothing either.
        let (kept, log) = finish(&flash, &ran(content.clone(), &writing(0x00)));
        assert!(kept && log.contains("not modified"), "{log}");
        assert_eq!(std::fs::read(&path).unwrap(), first);
        assert_eq!(identity(&path), before, "the file was not written again");
        // A run that changes a byte: the file becomes the new content, what the first run wrote
        // included.
        let another_write = soc_esp32c3::test_support::flash_program(0x5000, &[0x0F]);
        let (kept, log) = finish(&flash, &ran(content, &another_write));
        assert!(kept && log.contains("; saved to "), "{log}");
        let second = std::fs::read(&path).unwrap();
        assert_eq!((second[0x3000], second[0x5000]), (0x00, 0x0F));
        assert_ne!(identity(&path), before, "replaced, not written in place");
    }

    /// What tells a file that was replaced from one that was left alone: its inode where there
    /// is one, its modification time elsewhere.
    #[cfg(unix)]
    fn identity(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(path).unwrap().ino()
    }
    #[cfg(not(unix))]
    fn identity(path: &Path) -> std::time::SystemTime {
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    #[test]
    fn a_seed_and_an_existing_flash_file_are_refused_unless_the_file_is_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let kept_content = vec![0x11u8; MIB as usize];
        std::fs::write(&path, &kept_content).unwrap();
        let err = RunFlash::begin(Some(&path), false, Some(seed()), MIB).unwrap_err();
        assert!(matches!(err, FlashStateError::BothGiven { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains("both --flash and the flash file")
                && text.contains("x4.bin")
                && text.contains("; drop --flash")
                && text.contains("--reset-flash"),
            "{text}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), kept_content, "untouched");

        // The refusal released the lock. A reset starts from the seed and replaces the file,
        // even when the run modified nothing.
        let (flash, content) = RunFlash::begin(Some(&path), true, Some(seed()), MIB).unwrap();
        assert_eq!(content, Some(seed()));
        assert!(!flash.starts_from_the_file());
        let (kept, log) = finish(&flash, &ran(content, &[]));
        assert!(
            kept && log.contains("0 of its 4 KiB sectors were modified; saved to"),
            "{log}"
        );
        let replaced = std::fs::read(&path).unwrap();
        assert_eq!(replaced[..0x4000], seed()[..]);
        assert!(replaced[0x4000..].iter().all(|&byte| byte == 0xFF));
        drop(flash);

        // A reset without a seed starts from an erased flash.
        let (flash, content) = RunFlash::begin(Some(&path), true, None, MIB).unwrap();
        assert_eq!(content, None);
        let (kept, _) = finish(&flash, &ran(None, &[]));
        assert!(kept);
        assert!(
            std::fs::read(&path)
                .unwrap()
                .iter()
                .all(|&byte| byte == 0xFF)
        );
    }

    #[test]
    fn a_reset_does_not_replace_a_file_that_is_not_a_flash_of_the_device() {
        // A firmware image given to --flash-file by mistake must survive --reset-flash.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("firmware.bin");
        std::fs::write(&path, seed()).unwrap();
        for reset in [false, true] {
            let err = RunFlash::begin(Some(&path), reset, None, MIB).unwrap_err();
            assert!(
                matches!(err, FlashStateError::WrongSize { .. }),
                "{reset}: {err}"
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), seed());
    }

    #[cfg(unix)]
    #[test]
    fn a_flash_file_that_cannot_be_written_is_said_in_the_log_and_answered_by_no() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("flash");
        std::fs::create_dir(&inner).unwrap();
        let (flash, content) =
            RunFlash::begin(Some(&inner.join("x4.bin")), false, None, MIB).unwrap();
        std::fs::remove_dir_all(&inner).unwrap();
        let (kept, log) = finish(&flash, &ran(content, &writing(0x00)));
        assert!(!kept);
        assert!(
            log.starts_with("inkemu: flash at start: sha256 ")
                && log.contains("\nerror: cannot write the flash file ")
                && log.contains("what this run wrote to the flash is lost"),
            "{log}"
        );
    }

    #[test]
    fn a_log_that_cannot_be_written_does_not_keep_the_flash_from_being_saved() {
        struct Failing;
        impl Write for Failing {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let (flash, content) = RunFlash::begin(Some(&path), false, None, MIB).unwrap();
        let machine = ran(content, &writing(0x00));
        assert!(flash.finish(&machine, &mut Failing));
        assert_eq!(std::fs::read(&path).unwrap()[0x3000], 0x00);
    }
}
