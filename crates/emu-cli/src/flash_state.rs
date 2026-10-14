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
const LOCK_SUFFIX: &str = ".lock";

/// What to do about something at the name of the temporary file that cannot be removed, by the
/// cause the system gives. Removing a name asks for the right to change the directory, not the
/// file: a directory that refuses it is the cause, and removing the thing by hand would be
/// refused as well.
fn removal_remedy(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            "the directory does not let you remove a name in it (it is not writable for you, or it keeps each file for its owner): make the directory writable for you, or give the flash file a path in a directory of yours"
        }
        std::io::ErrorKind::ReadOnlyFilesystem => {
            "the directory is on a read-only filesystem: give the flash file a path on a writable one"
        }
        _ => "remove it, then run again",
    }
}

/// The mode of a flash file a run creates, on Unix: read and written by its owner, and nothing
/// for anyone else.
#[cfg(unix)]
const NEW_FLASH_FILE_MODE: u32 = 0o600;

/// Why a flash file cannot be used, read or written. The messages state the cause, then the
/// remedy; the paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum FlashStateError {
    #[error("the flash file path {path} names no file; give the path of a file, like flash-x4.bin")]
    NoFileName { path: String },
    #[error(
        "the flash file path {path} ends as the path of a directory does, with a separator or with `.`; give the path of the file itself, like flash-x4.bin"
    )]
    NamesADirectory { path: String },
    #[error("cannot create the lock file {lock} beside the flash file {path}: {detail}; {remedy}")]
    LockCreate {
        path: String,
        lock: String,
        detail: String,
        remedy: &'static str,
    },
    #[error(
        "{lock}, where the lock file of the flash file {path} goes, is not a regular file (a link is not followed); remove it or move it away, or give this run another --flash-file"
    )]
    LockNotAFile { path: String, lock: String },
    #[error(
        "cannot open for writing the lock file {lock} of the flash file {path}, which is there: {detail}; make it writable for you, or remove it once no run uses the flash file"
    )]
    LockOpen {
        path: String,
        lock: String,
        detail: String,
    },
    /// Found when the run starts, before anything runs: the file the content goes through when
    /// the run ends cannot be made now.
    #[error(
        "the flash file {path} cannot be written when this run ends: cannot {action} {target}: {detail}; {remedy}"
    )]
    Unwritable {
        path: String,
        action: &'static str,
        target: String,
        detail: String,
        remedy: &'static str,
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
    #[error("cannot read the flash file {path}: {detail}; {remedy}")]
    Read {
        path: String,
        detail: String,
        remedy: &'static str,
    },
    #[error(
        "the flash file {path} cannot be opened for writing: {detail}; make it writable for you, since a run replaces its flash file when it ends, or keep it as it is and give it to --flash, which is only read, with another path for --flash-file"
    )]
    NotWritable { path: String, detail: String },
    /// `seed` is the option the run would have started from, if any: it chooses the remedy.
    #[error(
        "the flash file {path} is {len} bytes and the flash of the device is {size} bytes: the file is not the flash of this device, and it is never padded or cut; {}",
        wrong_size_remedy(*seed)
    )]
    WrongSize {
        path: String,
        len: u64,
        size: u32,
        seed: Option<Input>,
    },
    #[error(
        "the flash file {path} changed while it was being read: it held {size} bytes, then {found}; make sure nothing else writes it, then run again"
    )]
    Changed {
        path: String,
        size: u32,
        found: String,
    },
    #[error(
        "both {input} and the flash file {path}, which exists, were given: the run cannot start from two flashes; drop {} to go on from the flash file, or add --reset-flash to start again from {} and replace the file",
        input.to_drop(), input.what_it_gives()
    )]
    BothGiven { path: String, input: Input },
    #[error(
        "{input} and --flash-file name the same file, {image} and {path}: {} of {input} is only read, and the flash file is replaced when the run ends; {}",
        input.noun(), input.same_file_remedy()
    )]
    SameFile {
        input: Input,
        image: String,
        path: String,
    },
    #[error(
        "{input} names {image}, which is the {role} of the flash file {path}: {why}, and {noun} of {input} is only read; copy {noun} to another name, then give that copy to {input}",
        noun = input.noun()
    )]
    ImageIsTheRunsOwn {
        input: Input,
        image: String,
        role: &'static str,
        path: String,
        why: &'static str,
    },
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

/// A file of `inkemu run` that the run only reads, named by its option in the refusals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// `--flash`: a whole flash image.
    Flash,
    /// `--firmware`: an application, which a flash is composed around.
    Firmware,
    /// `--bootloader`: the bootloader of that flash.
    Bootloader,
}

impl Input {
    /// What the refusals call the file of this input.
    const fn noun(self) -> &'static str {
        match self {
            Input::Flash => "the image",
            Input::Firmware | Input::Bootloader => "the file",
        }
    }

    /// What to drop from the command line for the run to go on from its flash file: a
    /// `--bootloader` cannot stay without its `--firmware`.
    const fn to_drop(self) -> &'static str {
        match self {
            Input::Flash => "--flash",
            Input::Firmware | Input::Bootloader => "--firmware, and --bootloader if it was given,",
        }
    }

    /// What a run starts from when this input is the seed and the flash file is reset.
    const fn what_it_gives(self) -> &'static str {
        match self {
            Input::Flash => "the image",
            Input::Firmware | Input::Bootloader => "the flash composed from the firmware",
        }
    }

    const fn same_file_remedy(self) -> &'static str {
        match self {
            Input::Flash => {
                "drop --flash to go on from the file, or give --flash-file another path to keep the image as it is"
            }
            Input::Firmware | Input::Bootloader => {
                "give --flash-file another path to keep the file as it is"
            }
        }
    }
}

impl std::fmt::Display for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Input::Flash => "--flash",
            Input::Firmware => "--firmware",
            Input::Bootloader => "--bootloader",
        })
    }
}

/// What to do about a flash file of another size than the flash, by the option the run would
/// have started from: with `--firmware` on the command line, `--flash` is no remedy, since the
/// two cannot be given together.
const fn wrong_size_remedy(seed: Option<Input>) -> &'static str {
    match seed {
        None | Some(Input::Flash) => {
            "use it with the device it was written for, pass a firmware image with --flash instead, or give a new path"
        }
        Some(Input::Firmware | Input::Bootloader) => {
            "use it with the device it was written for, or give --flash-file a new path"
        }
    }
}

/// The content a run starts from when it does not go on from its flash file, and the option
/// that gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seed {
    pub input: Input,
    pub content: Vec<u8>,
}

/// What to do about a file that cannot be created beside the flash file. The lock file and the
/// temporary file are both named after the flash file with five bytes more: a name too long
/// for one is too long for the other.
fn creation_remedy(error: &std::io::Error) -> &'static str {
    if error.kind() == std::io::ErrorKind::InvalidFilename {
        "give the flash file a shorter name: its lock file and its temporary file are named after it, with 5 bytes more"
    } else {
        "check that the directory exists and is writable, or give a path in one that is"
    }
}

/// The file beside `path` named after it: `prefix`, the name of `path`, then `suffix`. `None`
/// when `path` names no file, which [`FlashState::open`] refuses.
fn sibling(path: &Path, prefix: &str, suffix: &str) -> Option<PathBuf> {
    let name = path.file_name()?;
    let mut sibling = OsString::from(prefix);
    sibling.push(name);
    sibling.push(suffix);
    Some(path.with_file_name(sibling))
}

/// The lock file of the flash file at `path`.
fn lock_path(path: &Path) -> Option<PathBuf> {
    sibling(path, "", LOCK_SUFFIX)
}

/// The name the content of the flash file at `path` is written to before it takes the name of
/// the flash file. One name per flash file: the lock leaves a run alone with it.
fn temporary_path(path: &Path) -> Option<PathBuf> {
    sibling(path, ".", ".tmp")
}

/// Refuses `image`, the file given to `--flash`, as a file the run writes: the flash file, which
/// is replaced when the run ends, its temporary file, where an accepted run removes what it finds
/// and writes the content, and its lock file, which the run opens and locks. The image is only
/// read. Asked before anything is created: a refused run leaves no lock file beside the image,
/// and what sits at the temporary name is left as it is.
///
/// # Errors
/// [`FlashStateError::SameFile`] when the image is the flash file,
/// [`FlashStateError::ImageIsTheRunsOwn`] when it is the temporary file or the lock file of the
/// flash file, by whatever route the two paths lead to one file.
pub fn refuse_the_image_as_flash_file(
    input: Input,
    image: &Path,
    flash_file: &Path,
) -> Result<(), FlashStateError> {
    if same_file(image, flash_file) {
        return Err(FlashStateError::SameFile {
            input,
            image: shown(image),
            path: shown(flash_file),
        });
    }
    let owned = [
        (
            temporary_path(flash_file),
            "temporary file",
            "what is there is removed when the run starts, and the new content is written there when it ends",
        ),
        (
            lock_path(flash_file),
            "lock file",
            "the run opens it and holds it locked, and other runs open it too",
        ),
    ];
    for (path, role, why) in owned {
        if let Some(path) = path
            && same_file(image, &path)
        {
            return Err(FlashStateError::ImageIsTheRunsOwn {
                input,
                image: shown(image),
                role,
                path: shown(flash_file),
                why,
            });
        }
    }
    Ok(())
}

/// Whether the two paths lead to one file, by whatever route: another spelling of the path, a
/// symbolic link (followed on both sides: a link to the image is the image) or a hard link. A
/// path that leads to nothing is one file with no other: what is wrong with it is said by
/// whoever opens it.
#[cfg(unix)]
pub(crate) fn same_file(one: &Path, other: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (std::fs::metadata(one), std::fs::metadata(other)) {
        (Ok(one), Ok(other)) => one.dev() == other.dev() && one.ino() == other.ino(),
        _ => false,
    }
}

/// Where the standard library gives a file no identity, the two paths are compared once every
/// link in them is resolved. A hard link is then not seen; replacing the flash file leaves the
/// other name of the image with its content.
#[cfg(not(unix))]
pub(crate) fn same_file(one: &Path, other: &Path) -> bool {
    matches!(
        (std::fs::canonicalize(one), std::fs::canonicalize(other)),
        (Ok(one), Ok(other)) if one == other
    )
}

/// Opens the file at `path` as `options` say, and never anything else that would be there in its
/// place a moment after it was looked at. On Unix a link at the name fails the open instead of
/// leading it elsewhere (`O_NOFOLLOW`), and a pipe fails or opens at once instead of making the
/// process wait for its other end (`O_NONBLOCK`, which changes nothing for a regular file).
/// Elsewhere the standard library has no such request: the open is the plain one, and what was
/// opened is judged by its caller, as everywhere.
fn open_in_place(options: &mut OpenOptions, path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    options.open(path)
}

/// Opens the lock file at `lock_path`, creating it if there is none. Nothing is ever written to
/// it, and an existing one is not truncated.
///
/// What sits at the name and is no regular file is refused by name without being opened: a
/// link would have the file it names created, a pipe would be waited on. What was opened is
/// judged again, for a name that changed hands in between.
fn open_lock_file(
    lock_path: &Path,
    shown_path: &str,
    shown_lock: &str,
) -> Result<File, FlashStateError> {
    let not_a_file = || FlashStateError::LockNotAFile {
        path: shown_path.to_owned(),
        lock: shown_lock.to_owned(),
    };
    let is_there = match std::fs::symlink_metadata(lock_path) {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => return Err(not_a_file()),
        // Nothing there, or nothing that can be looked at: the open says which.
        Err(_) => false,
    };
    let lock = open_in_place(
        OpenOptions::new().write(true).create(true).truncate(false),
        lock_path,
    )
    .map_err(|e| {
        let detail = sanitize(&e.to_string());
        if is_there {
            FlashStateError::LockOpen {
                path: shown_path.to_owned(),
                lock: shown_lock.to_owned(),
                detail,
            }
        } else {
            FlashStateError::LockCreate {
                path: shown_path.to_owned(),
                lock: shown_lock.to_owned(),
                detail,
                remedy: creation_remedy(&e),
            }
        }
    })?;
    match lock.metadata() {
        Ok(metadata) if metadata.is_file() => Ok(lock),
        _ => Err(not_a_file()),
    }
}

/// Whether what is at `path` can be the flash file of a device whose flash is `size` bytes:
/// `Ok(false)` when nothing is there, `Ok(true)` for a regular file of exactly that size that
/// the user can write. What is no regular file is not opened: a link is not followed, and a
/// pipe is not waited on.
///
/// A flash file is replaced, not written in place, and replacing a file asks nothing of its
/// mode: a file its owner made read-only would be replaced all the same. A flash image is
/// often an only copy, so the mode is taken at its word and such a file is refused. Whether it
/// can be written is asked of the system, by opening it for writing (nothing is written, the
/// file is closed at once), not read from the mode: the system knows who runs, and what the
/// filesystem allows.
fn judge(path: &Path, shown: &str, size: u32) -> Result<bool, FlashStateError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(read_error(shown, &e)),
    };
    check(shown, size, &metadata)?;
    match open_in_place(OpenOptions::new().write(true), path) {
        Ok(_) => Ok(true),
        // Removed since the look: there is no flash file.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(FlashStateError::NotWritable {
            path: shown.to_owned(),
            detail: sanitize(&e.to_string()),
        }),
    }
}

fn check(shown: &str, size: u32, metadata: &std::fs::Metadata) -> Result<(), FlashStateError> {
    if !metadata.is_file() {
        return Err(FlashStateError::NotAFile {
            path: shown.to_owned(),
        });
    }
    if metadata.len() != u64::from(size) {
        return Err(FlashStateError::WrongSize {
            path: shown.to_owned(),
            len: metadata.len(),
            size,
            seed: None,
        });
    }
    Ok(())
}

fn read_error(shown: &str, error: &std::io::Error) -> FlashStateError {
    use std::io::ErrorKind;
    FlashStateError::Read {
        path: shown.to_owned(),
        detail: sanitize(&error.to_string()),
        remedy: match error.kind() {
            ErrorKind::NotADirectory => {
                "check the path: a part of it that should be a directory is not one"
            }
            // It was there when it was looked at: something removed it since.
            ErrorKind::NotFound => "make sure nothing else uses it, then run again",
            ErrorKind::PermissionDenied => {
                "check its read permissions, and those of the directories that lead to it"
            }
            _ => "check the path and its read permissions",
        },
    }
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

/// What a read of a file of `size` bytes found instead, `found` bytes, as the message of a file
/// that changed says it: the count when it is fewer, "more" when it is more, since no more than
/// one byte past `size` is ever read and the count would then be `size + 1` whatever the file
/// holds.
fn found_instead(found: u64, size: u32) -> String {
    if found > u64::from(size) {
        "more".to_owned()
    } else {
        format!("{found}")
    }
}

/// How many sectors of the flash a run modified, as a clause of its summary.
fn sectors_modified(modified: u32) -> String {
    let were = if modified == 1 { "was" } else { "were" };
    format!("{modified} of its 4 KiB sectors {were} modified")
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
    /// Whatever would keep the content from being written when the run ends, and can be known
    /// now, is refused now: a run must not go to its end to learn that what it wrote to the
    /// flash is lost for a cause that was there before it started. What sits at the path is
    /// judged first, so that a run refused for it creates nothing beside it; then, under the
    /// lock, the temporary file of this run is created and removed, to know that it can be.
    ///
    /// What already sits at the name of the temporary file is not touched here: the run may
    /// still be refused, and a refused run removes nothing. It is removed once the run is
    /// accepted ([`RunFlash::accept`]).
    ///
    /// # Errors
    /// [`FlashStateError`] when the path names no file, what is at the path cannot be the flash
    /// file of the device, the lock file or the temporary file cannot be created, or another
    /// run holds the lock.
    pub fn open(path: &Path, size: u32) -> Result<FlashState, FlashStateError> {
        let shown_path = shown(path);
        let Some(name) = path.file_name() else {
            return Err(FlashStateError::NoFileName { path: shown_path });
        };
        // `new.bin/` and `new.bin/.` have a last name too, and name a directory: the content
        // could not take that name. A path that names a file ends with the name of the file.
        if !path
            .as_os_str()
            .as_encoded_bytes()
            .ends_with(name.as_encoded_bytes())
        {
            return Err(FlashStateError::NamesADirectory { path: shown_path });
        }
        // Both exist: the path has a file name.
        let (Some(lock_path), Some(temporary)) = (lock_path(path), temporary_path(path)) else {
            return Err(FlashStateError::NoFileName { path: shown_path });
        };
        // Before the lock file exists. The judgement that counts is made again under the lock,
        // by whoever asks whether the file exists.
        judge(path, &shown_path, size)?;
        let shown_lock = shown(&lock_path);
        let lock = open_lock_file(&lock_path, &shown_path, &shown_lock)?;
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
        let state = FlashState {
            temporary,
            path: path.to_path_buf(),
            shown: shown_path,
            size,
            _lock: lock,
        };
        state.try_the_temporary_file()?;
        Ok(state)
    }

    fn create_temporary(&self) -> std::io::Result<File> {
        let mut options = OpenOptions::new();
        // `create_new` never follows a link and never reuses a file planted at the name.
        options.write(true).create_new(true);
        // A flash holds what the firmware keeps, the credentials of a network among it: on
        // Unix the file is for its owner alone from its first byte, and a flash file created
        // by a run stays so. Elsewhere a new file has the access rights its directory gives
        // it, as any file the standard library creates.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(NEW_FLASH_FILE_MODE);
        }
        options.open(&self.temporary)
    }

    /// Gives `temporary`, which holds the new content, the permission bits of the flash file it
    /// is about to replace, when there is one: a file the user opened to a group, or closed to
    /// everyone else, stays as the user set it. Only the read, write and execute bits are kept.
    /// With nothing to replace the file is a new one, and keeps the mode it was created with.
    #[cfg(unix)]
    fn keep_the_mode_of_the_replaced_file(&self, temporary: &File) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        const ACCESS_BITS: u32 = 0o777;
        let replaced = match std::fs::symlink_metadata(&self.path) {
            Ok(replaced) if replaced.is_file() => replaced.permissions().mode() & ACCESS_BITS,
            _ => return Ok(()),
        };
        // Not asked for when there is nothing to change: some filesystems refuse any request.
        if temporary.metadata()?.permissions().mode() & ACCESS_BITS == replaced {
            return Ok(());
        }
        temporary.set_permissions(std::fs::Permissions::from_mode(replaced))
    }

    /// What could not be done to the temporary file when the run starts, as the refusal of the
    /// run.
    fn unwritable(
        &self,
        action: &'static str,
        remedy: &'static str,
        error: &std::io::Error,
    ) -> FlashStateError {
        FlashStateError::Unwritable {
            path: self.shown.clone(),
            action,
            target: shown(&self.temporary),
            detail: sanitize(&error.to_string()),
            remedy,
        }
    }

    /// Creates and removes the temporary file of this run: the directory takes a new file and
    /// the name fits, now. When something already sits at the name, nothing is created and
    /// nothing is removed. The name fits, since it is taken; whether the directory still takes
    /// a change is learnt when the run is accepted and the name is cleared, which asks of the
    /// directory the right a creation asks.
    fn try_the_temporary_file(&self) -> Result<(), FlashStateError> {
        match self.create_temporary() {
            Ok(created) => drop(created),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(e) => return Err(self.unwritable("create", creation_remedy(&e), &e)),
        }
        std::fs::remove_file(&self.temporary)
            .map_err(|e| self.unwritable("remove", removal_remedy(&e), &e))
    }

    /// Removes what sits at the name of the temporary file, if anything does, and says whether
    /// something did. No run is writing it, since this one holds the lock: what a save that was
    /// interrupted left is there, or whatever else was put at that name. A link is removed
    /// itself, never what it names.
    ///
    /// For a run that is accepted, not for one that may still be refused: see
    /// [`RunFlash::accept`].
    fn clear_the_temporary_name(&self) -> Result<bool, FlashStateError> {
        match std::fs::remove_file(&self.temporary) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(self.unwritable("remove", removal_remedy(&e), &e)),
        }
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
        judge(&self.path, &self.shown, self.size)
    }

    /// Whether nothing at all is at the path of the flash file. What is there is not judged:
    /// a file that is no flash of the device is there.
    fn is_gone(&self) -> bool {
        matches!(
            std::fs::symlink_metadata(&self.path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound
        )
    }

    /// The content of the flash file: exactly the bytes of the flash of the device. The size is
    /// judged before anything is read, and no more than one byte past it is ever read.
    ///
    /// # Errors
    /// [`FlashStateError`] when the file cannot be read, is not a regular file of the size of
    /// the flash, or changes while it is read.
    pub fn load(&self) -> Result<Vec<u8>, FlashStateError> {
        let read_error = |e: std::io::Error| read_error(&self.shown, &e);
        // The file was judged under the lock, a moment ago. What took its name since is opened
        // as the lock file is: a pipe is not waited on, and a link fails the open instead of
        // having the file it names read as the flash of the device.
        let file = open_in_place(OpenOptions::new().read(true), &self.path).map_err(|e| {
            match std::fs::symlink_metadata(&self.path) {
                Ok(now_there) if !now_there.is_file() => FlashStateError::NotAFile {
                    path: self.shown.clone(),
                },
                _ => read_error(e),
            }
        })?;
        // What was opened is judged, not what the path named a moment ago.
        check(
            &self.shown,
            self.size,
            &file.metadata().map_err(read_error)?,
        )?;
        read_exactly(file, self.size)
            .map_err(read_error)?
            .map_err(|found| FlashStateError::Changed {
                path: self.shown.clone(),
                size: self.size,
                found: found_instead(found, self.size),
            })
    }

    /// Replaces the flash file with `content`, the whole flash of the device. The bytes go to a
    /// temporary file in the same directory, are flushed to the disk, and only then take the name
    /// of the flash file: whatever interrupts this, the path names the old content or the new
    /// one, never a part of either.
    ///
    /// What sits at the name of the temporary file is removed first, and `log` says so: the name
    /// was cleared when the run was accepted, and what was put there since is not this run's.
    /// A failing log writer changes nothing of the save.
    ///
    /// # Errors
    /// [`FlashStateError::Write`], which says what could not be done to which file; the flash
    /// file is then the one of before, and no temporary file is left.
    pub fn save(&self, content: &[u8], log: &mut dyn Write) -> Result<(), FlashStateError> {
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
        // The name was free when the run was accepted. Whatever was put there since is not
        // this run's: it is removed (a link itself, never what it points to), once, and said.
        let mut file = match self.create_temporary() {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                std::fs::remove_file(&self.temporary)
                    .map_err(|e| failed("remove", &self.temporary, &e))?;
                let _ = writeln!(
                    log,
                    "inkemu: removed {}, put while the run went on at the name where a save of the flash file {} puts its temporary file; nothing to do: the flash file is saved through that name now",
                    shown(&self.temporary),
                    self.shown
                );
                self.create_temporary()
            }
            other => other,
        }
        .map_err(|e| failed("create", &self.temporary, &e))?;
        file.write_all(content)
            .map_err(|e| failed("write", &self.temporary, &e))?;
        // Where a file has no mode there is none to keep: the new file has the access rights
        // its directory gives it.
        #[cfg(unix)]
        {
            self.keep_the_mode_of_the_replaced_file(&file)
                .map_err(|e| failed("set the permissions of", &self.temporary, &e))?;
        }
        file.sync_all()
            .map_err(|e| failed("write", &self.temporary, &e))?;
        drop(file);
        std::fs::rename(&self.temporary, &self.path)
            .map_err(|e| failed("move into place", &self.path, &e))
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
        seed: Option<Seed>,
        size: u32,
    ) -> Result<(RunFlash, Option<Vec<u8>>), FlashStateError> {
        let input = seed.as_ref().map(|seed| seed.input);
        Self::take(flash_file, reset, seed, size).map_err(|error| match error {
            // The file is judged before the seed is looked at: the remedy names what was given.
            FlashStateError::WrongSize {
                path, len, size, ..
            } => FlashStateError::WrongSize {
                path,
                len,
                size,
                seed: input,
            },
            other => other,
        })
    }

    fn take(
        flash_file: Option<&Path>,
        reset: bool,
        seed: Option<Seed>,
        size: u32,
    ) -> Result<(RunFlash, Option<Vec<u8>>), FlashStateError> {
        let state = flash_file
            .map(|path| FlashState::open(path, size))
            .transpose()?;
        // An existing file is judged even when it is about to be replaced: a file of another
        // size is not a flash of this device, and `--reset-flash` must not destroy it.
        let from_file = match &state {
            Some(state) if state.exists()? => {
                if let Some(seed) = &seed
                    && !reset
                {
                    return Err(FlashStateError::BothGiven {
                        path: state.shown.clone(),
                        input: seed.input,
                    });
                }
                !reset
            }
            _ => false,
        };
        let content = match &state {
            Some(state) if from_file => Some(state.load()?),
            _ => seed.map(|seed| seed.content),
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

    /// Whether the run starts from the content of its flash file.
    #[must_use]
    pub const fn starts_from_the_file(&self) -> bool {
        self.from_file
    }

    /// Clears the way for the save of a run that is accepted: every other refusal is behind
    /// it, its machine is built and about to run. What sits at the name of the temporary file
    /// is removed, and `log` says so. A run that is refused, for its flash file or for anything
    /// else, never comes here, and removes nothing.
    ///
    /// # Errors
    /// [`FlashStateError::Unwritable`] when something sits at the name and cannot be removed:
    /// the content could not go through that name when the run ends.
    pub fn accept(&self, log: &mut dyn Write) -> Result<(), FlashStateError> {
        let Some(state) = &self.state else {
            return Ok(());
        };
        if state.clear_the_temporary_name()? {
            let _ = writeln!(
                log,
                "inkemu: removed {}, found at the name where a save of the flash file {} puts its temporary file (an interrupted save leaves one there); nothing to do: the flash file itself is left as it is, and that name is kept for its saves",
                shown(&state.temporary),
                state.shown()
            );
        }
        Ok(())
    }

    /// Ends the run for its flash: says in `log` what it started from and what became of it,
    /// and writes the flash file when there is one and it does not hold the content the run
    /// ends with: it is absent (a first run, or a file removed while the run went on), it is
    /// being replaced, or the run modified the flash.
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
                    "inkemu: flash at end: {} and {} not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next",
                    sectors_modified(modified),
                    if modified == 1 { "is" } else { "are" }
                );
            }
            return true;
        };
        let unchanged = self.from_file && modified == 0;
        // The file the run started from held this content; whether it still does is for the
        // file to say. One that is gone holds nothing, and the content is in memory.
        let removed = unchanged && state.is_gone();
        if unchanged && !removed {
            let _ = writeln!(
                log,
                "inkemu: flash at end: not modified; {} is left as it is",
                state.shown()
            );
            return true;
        }
        let content = machine.flash_contents();
        match state.save(&content, log) {
            Ok(()) => {
                let sha256 = content_sha256(&content, self.size);
                let path = state.shown();
                let _ = if removed {
                    writeln!(
                        log,
                        "inkemu: flash at end: sha256 {sha256}; not modified; {path} was removed while the run went on and is written again"
                    )
                } else {
                    writeln!(
                        log,
                        "inkemu: flash at end: sha256 {sha256}; {}; saved to {path}",
                        sectors_modified(modified)
                    )
                };
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
            save(&state, &content(1)).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), content(1), "no header");
            assert_eq!(
                names(dir.path()),
                ["x4.bin", "x4.bin.lock"],
                "no temporary file is left"
            );
            assert!(state.exists().unwrap());
            assert_eq!(state.load().unwrap(), content(1));
            save(&state, &content(2)).unwrap();
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
    fn what_is_where_the_lock_file_goes_and_is_no_regular_file_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let refused = |name: &str| {
            let err = FlashState::open(&dir.path().join(name), SIZE).unwrap_err();
            assert!(matches!(err, FlashStateError::LockNotAFile { .. }), "{err}");
            let text = err.to_string();
            assert!(
                text.contains(&format!("{name}.lock"))
                    && text.contains("is not a regular file")
                    && text.contains("; remove it or move it away"),
                "{text}"
            );
        };
        std::fs::create_dir(dir.path().join("dir.bin.lock")).unwrap();
        refused("dir.bin");
        #[cfg(unix)]
        {
            // A link to nothing: followed, it would have the file it names created.
            let link = dir.path().join("link.bin.lock");
            std::os::unix::fs::symlink(dir.path().join("planted"), &link).unwrap();
            refused("link.bin");
            // A link to a file that is there: the lock of this run would be taken on that file.
            std::fs::remove_file(&link).unwrap();
            std::fs::write(dir.path().join("other.lock"), b"").unwrap();
            std::os::unix::fs::symlink(dir.path().join("other.lock"), &link).unwrap();
            refused("link.bin");
            std::fs::remove_file(dir.path().join("other.lock")).unwrap();
            std::fs::remove_file(&link).unwrap();
        }
        assert_eq!(names(dir.path()), ["dir.bin.lock"], "nothing was created");
    }

    /// What closes the instant between the look at a name and its opening: the open itself
    /// refuses a link, whatever the link names.
    #[cfg(unix)]
    #[test]
    fn an_open_in_place_never_follows_a_link_and_takes_a_regular_file_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let (file, link) = (dir.path().join("file"), dir.path().join("link"));
        let creating = || {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(false);
            options
        };
        std::os::unix::fs::symlink(&file, &link).unwrap();
        open_in_place(&mut creating(), &link).unwrap_err();
        assert!(!file.exists(), "what the link names was not created");
        std::fs::write(&file, b"kept").unwrap();
        open_in_place(&mut creating(), &link).unwrap_err();
        open_in_place(&mut creating(), &file).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"kept");
    }

    /// Makes a named pipe at `path`. Nobody ever opens its other end: an open that waits for
    /// one waits for ever.
    #[cfg(unix)]
    fn make_pipe(path: &Path) {
        nix::unistd::mkfifo(path, nix::sys::stat::Mode::S_IRWXU).unwrap();
    }

    /// What `work` returns, or `None` when it has not returned after five seconds: it waits
    /// for the other end of `pipe`. The test must then fail, not hang: both ends of the pipe
    /// are opened, which lets `work` return, before the answer is given.
    #[cfg(unix)]
    fn unless_it_waits_on<T: Send + 'static>(
        pipe: &Path,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        use std::os::unix::fs::OpenOptionsExt as _;
        let (answer, answered) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = answer.send(work());
        });
        let returned = answered
            .recv_timeout(std::time::Duration::from_secs(5))
            .ok();
        let both_ends = returned.is_none().then(|| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(pipe)
        });
        worker.join().unwrap();
        drop(both_ends);
        returned
    }

    /// What keeps a run from waiting for ever on a pipe put where one of its files goes: the
    /// open asks not to wait for the other end.
    #[cfg(unix)]
    #[test]
    fn an_open_in_place_does_not_wait_for_the_other_end_of_a_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("pipe");
        make_pipe(&pipe);
        // For writing, as the lock file and the flash file are opened: with nobody to read,
        // the open fails at once.
        let opened = unless_it_waits_on(&pipe, {
            let pipe = pipe.clone();
            move || {
                open_in_place(OpenOptions::new().write(true), &pipe).map_err(|e| e.raw_os_error())
            }
        });
        match opened {
            Some(Err(errno)) => assert_eq!(errno, Some(nix::libc::ENXIO)),
            Some(Ok(_)) => panic!("a pipe nobody reads was opened for writing"),
            None => panic!("the open for writing waited on the pipe"),
        }
        // For reading, as the flash file is loaded: the open returns at once, and what it
        // opened is for its caller to judge.
        let opened = unless_it_waits_on(&pipe, {
            let pipe = pipe.clone();
            move || open_in_place(OpenOptions::new().read(true), &pipe).map(|file| file.metadata())
        });
        match opened {
            Some(Ok(Ok(metadata))) => assert!(!metadata.is_file()),
            Some(other) => panic!("{other:?}"),
            None => panic!("the open for reading waited on the pipe"),
        }
    }

    /// The flash file is judged under the lock, then read. What takes its name in between is
    /// not read as the flash of the device, and is not waited on.
    #[cfg(unix)]
    #[test]
    fn what_took_the_place_of_the_flash_file_since_it_was_judged_is_refused_not_read() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = Arc::new(FlashState::open(&path, SIZE).unwrap());
        let not_a_file = |what: &str, loaded: Option<Result<Vec<u8>, FlashStateError>>| match loaded
        {
            None => panic!("the read waited on {what}"),
            Some(Ok(read)) => panic!("{what} was read as a flash of {} bytes", read.len()),
            Some(Err(err)) => assert!(
                matches!(err, FlashStateError::NotAFile { .. }),
                "{what}: {err}"
            ),
        };

        // A link to a file of the size of the flash: followed, it would be read as the flash.
        std::fs::write(dir.path().join("other.bin"), content(7)).unwrap();
        save(&state, &content(1)).unwrap();
        assert!(state.exists().unwrap());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("other.bin"), &path).unwrap();
        not_a_file("a link", Some(state.load()));

        // A pipe: opened as a file is, the read would wait for a writer that never comes.
        std::fs::remove_file(&path).unwrap();
        make_pipe(&path);
        let loading = Arc::clone(&state);
        not_a_file("a pipe", unless_it_waits_on(&path, move || loading.load()));
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_file_that_is_there_and_cannot_be_opened_is_not_said_to_be_impossible_to_create() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("x4.bin.lock");
        std::fs::write(&lock, b"").unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(
            OpenOptions::new().write(true).open(&lock).is_err(),
            "this test needs a user whom a read-only file stops; run it as one"
        );
        let err = FlashState::open(&dir.path().join("x4.bin"), SIZE).unwrap_err();
        assert!(matches!(err, FlashStateError::LockOpen { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains("cannot open for writing the lock file")
                && text.contains("x4.bin.lock")
                && text.contains("which is there")
                && text.contains("; make it writable for you"),
            "{text}"
        );
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_flash_file_that_a_run_creates_is_for_its_owner_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        save(&state, &content(1)).unwrap();
        assert_eq!(mode_of(&path), 0o600, "{:o}", mode_of(&path));
        // The same for a file that was removed while the run went on: it is a new file.
        set_mode(&path, 0o644);
        std::fs::remove_file(&path).unwrap();
        save(&state, &content(2)).unwrap();
        assert_eq!(mode_of(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_flash_file_that_is_replaced_keeps_the_mode_its_owner_gave_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        save(&state, &content(0)).unwrap();
        for (seed, mode) in [(1, 0o640), (2, 0o664), (3, 0o600), (4, 0o660)] {
            set_mode(&path, mode);
            save(&state, &content(seed)).unwrap();
            assert_eq!(mode_of(&path), mode, "{:o}", mode_of(&path));
            assert_eq!(std::fs::read(&path).unwrap(), content(seed));
        }
        // The access bits, and nothing a flash has no use for.
        set_mode(&path, 0o4640);
        save(&state, &content(5)).unwrap();
        assert_eq!(mode_of(&path), 0o640, "{:o}", mode_of(&path));
    }

    #[cfg(unix)]
    #[test]
    fn a_flash_file_the_user_cannot_write_is_refused_and_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let only_copy = vec![0x33u8; MIB as usize];
        std::fs::write(&path, &only_copy).unwrap();
        set_mode(&path, 0o400);
        assert!(
            OpenOptions::new().write(true).open(&path).is_err(),
            "this test needs a user whom a read-only file stops; run it as one"
        );
        // With or without a reset: the file would be replaced in both cases.
        for reset in [false, true] {
            let err = RunFlash::begin(Some(&path), reset, None, MIB).unwrap_err();
            assert!(
                matches!(err, FlashStateError::NotWritable { .. }),
                "{reset}: {err}"
            );
            let text = err.to_string();
            assert!(
                text.contains("x4.bin")
                    && text.contains("cannot be opened for writing")
                    && text.contains("; make it writable for you")
                    && text.contains("give it to --flash, which is only read"),
                "{text}"
            );
        }
        assert_eq!(names(dir.path()), ["x4.bin"], "not even a lock file");
        assert_eq!(mode_of(&path), 0o400);
        assert_eq!(std::fs::read(&path).unwrap(), only_copy);

        // Made read-only by a run before this one, between the look that comes before the
        // lock and the one that counts, under it: refused all the same.
        set_mode(&path, 0o600);
        let state = FlashState::open(&path, MIB).unwrap();
        set_mode(&path, 0o400);
        assert!(matches!(
            state.exists(),
            Err(FlashStateError::NotWritable { .. })
        ));
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
        let state = FlashState::open(&path, SIZE).unwrap();
        File::create(&path).unwrap().set_len(1 << 40).unwrap();
        assert!(matches!(
            state.load(),
            Err(FlashStateError::WrongSize { len, .. }) if len == 1 << 40
        ));
        drop(state);
        // And when a run starts, before anything else is done.
        assert!(matches!(
            FlashState::open(&path, SIZE),
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
        let err = FlashState::open(&path, SIZE).unwrap_err();
        assert!(matches!(err, FlashStateError::NotAFile { .. }), "{err}");
        assert!(err.to_string().contains("; give the path"), "{err}");
        assert_eq!(
            names(dir.path()),
            ["x4.bin"],
            "no lock file beside what was refused"
        );
        // A directory that took the place of the flash file while a run went on.
        std::fs::remove_dir(&path).unwrap();
        let state = FlashState::open(&path, SIZE).unwrap();
        std::fs::create_dir(&path).unwrap();
        for err in [state.exists().unwrap_err(), state.load().unwrap_err()] {
            assert!(matches!(err, FlashStateError::NotAFile { .. }), "{err}");
        }
    }

    #[test]
    fn a_file_that_is_not_a_flash_of_the_device_is_refused_before_a_lock_file_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("firmware.bin");
        std::fs::write(&path, [0xE9; 100]).unwrap();
        let err = FlashState::open(&path, SIZE).unwrap_err();
        assert!(
            matches!(err, FlashStateError::WrongSize { len: 100, .. }),
            "{err}"
        );
        assert_eq!(names(dir.path()), ["firmware.bin"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed_and_what_it_points_to_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("precious.bin");
        std::fs::write(&target, content(9)).unwrap();
        let path = dir.path().join("x4.bin");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let err = FlashState::open(&path, SIZE).unwrap_err();
        assert!(matches!(err, FlashStateError::NotAFile { .. }), "{err}");
        assert!(err.to_string().contains("a link is not followed"), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), content(9));
        assert_eq!(names(dir.path()), ["precious.bin", "x4.bin"]);
        // A link that took the place of the flash file while a run went on.
        std::fs::remove_file(&path).unwrap();
        let state = FlashState::open(&path, SIZE).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let err = state.exists().unwrap_err();
        assert!(matches!(err, FlashStateError::NotAFile { .. }), "{err}");
    }

    #[test]
    fn a_path_that_ends_as_the_path_of_a_directory_is_refused_and_nothing_is_created() {
        let dir = tempfile::tempdir().unwrap();
        for ending in ["x4.bin/", "x4.bin/.", "x4.bin//", "x4.bin/./."] {
            let path = format!("{}/{ending}", dir.path().display());
            let err = FlashState::open(Path::new(&path), SIZE).unwrap_err();
            assert!(
                matches!(err, FlashStateError::NamesADirectory { .. }),
                "{ending}: {err}"
            );
            let text = err.to_string();
            assert!(
                text.contains(ending) && text.contains("; give the path of the file itself"),
                "{text}"
            );
        }
        assert_eq!(names(dir.path()), [""; 0]);
        // Separators inside the path are no end of it: this one names a file.
        let doubled = format!("{}//x4.bin", dir.path().display());
        FlashState::open(Path::new(&doubled), SIZE).unwrap();
        assert_eq!(names(dir.path()), ["x4.bin.lock"]);
    }

    // 255 bytes: the longest name of a file on the filesystems a test runs on.
    #[cfg(unix)]
    #[test]
    fn the_longest_name_that_leaves_room_for_its_lock_file_is_saved_and_a_longer_one_refused() {
        let dir = tempfile::tempdir().unwrap();
        let longest = format!("{}.bin", "n".repeat(246));
        let path = dir.path().join(&longest);
        let state = FlashState::open(&path, SIZE).unwrap();
        save(&state, &content(1)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), content(1));
        assert_eq!(
            names(dir.path()),
            [longest.clone(), format!("{longest}.lock")]
        );

        let dir = tempfile::tempdir().unwrap();
        let too_long = dir.path().join(format!("{}.bin", "n".repeat(247)));
        let err = FlashState::open(&too_long, SIZE).unwrap_err();
        assert!(matches!(err, FlashStateError::LockCreate { .. }), "{err}");
        assert!(
            err.to_string()
                .contains("; give the flash file a shorter name"),
            "{err}"
        );
        assert_eq!(names(dir.path()), [""; 0], "nothing was created");
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_takes_no_new_file_is_refused_at_once_though_it_holds_the_lock_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("flash");
        std::fs::create_dir(&inner).unwrap();
        let path = inner.join("x4.bin");
        // An earlier run left its lock file: taking the lock needs no new file.
        drop(FlashState::open(&path, SIZE).unwrap());
        let mode = |mode| std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(mode));
        mode(0o555).unwrap();
        let administrator = File::create(inner.join("probe")).is_ok();
        let refused = FlashState::open(&path, SIZE);
        mode(0o755).unwrap();
        assert!(
            !administrator,
            "this test needs a user whom a read-only directory stops; run it as one"
        );
        let err = refused.unwrap_err();
        assert!(
            matches!(
                err,
                FlashStateError::Unwritable {
                    action: "create",
                    ..
                }
            ),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("x4.bin cannot be written when this run ends: cannot create ")
                && text.contains(".x4.bin.tmp")
                && text.contains("; check that the directory exists and is writable"),
            "{text}"
        );
        assert_eq!(names(&inner), ["x4.bin.lock"]);
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
        let err = save(&state, &content(1)).unwrap_err();
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
        let err = save(&state, &content(1)).unwrap_err();
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

    /// [`FlashState::save`] with a log nobody reads.
    fn save(state: &FlashState, content: &[u8]) -> Result<(), FlashStateError> {
        state.save(content, &mut std::io::sink())
    }

    fn accept(flash: &RunFlash) -> (Result<(), FlashStateError>, String) {
        let mut log = Vec::new();
        let accepted = flash.accept(&mut log);
        (accepted, String::from_utf8(log).unwrap())
    }

    #[test]
    fn what_an_interrupted_save_left_is_removed_by_a_run_that_is_accepted_which_says_so() {
        const LEFT: &[u8] = b"a part of a flash, left by a save that was killed";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let temporary = dir.path().join(".x4.bin.tmp");
        std::fs::write(&path, vec![0x11u8; MIB as usize]).unwrap();
        std::fs::write(&temporary, LEFT).unwrap();

        // A run that is refused, here for an image given next to a flash file that exists,
        // leaves the directory as it found it but for its lock file.
        let err = RunFlash::begin(Some(&path), false, Some(flash_seed()), MIB).unwrap_err();
        assert!(matches!(err, FlashStateError::BothGiven { .. }), "{err}");
        assert_eq!(std::fs::read(&temporary).unwrap(), LEFT, "not removed");

        // A run that takes the file does not touch it either until it is accepted: what comes
        // after the flash file (the ROM, the machine) may still refuse it.
        let (flash, _) = RunFlash::begin(Some(&path), false, None, MIB).unwrap();
        assert_eq!(std::fs::read(&temporary).unwrap(), LEFT, "not removed yet");
        let (accepted, log) = accept(&flash);
        accepted.unwrap();
        assert_eq!(names(dir.path()), ["x4.bin", "x4.bin.lock"], "removed");
        assert_eq!(
            log,
            format!(
                "inkemu: removed {}, found at the name where a save of the flash file {} puts its temporary file (an interrupted save leaves one there); nothing to do: the flash file itself is left as it is, and that name is kept for its saves\n",
                sanitize(&temporary.display().to_string()),
                sanitize(&path.display().to_string())
            )
        );
        // With nothing at the name there is nothing to remove, and nothing to say.
        let (accepted, log) = accept(&flash);
        accepted.unwrap();
        assert_eq!(log, "");
        // Nor without a flash file.
        let (no_file, _) = RunFlash::begin(None, false, None, MIB).unwrap();
        let (accepted, log) = accept(&no_file);
        accepted.unwrap();
        assert_eq!(log, "");
    }

    #[test]
    fn what_takes_the_temporary_name_while_the_run_goes_on_is_replaced_and_a_link_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let temporary = dir.path().join(".x4.bin.tmp");
        let state = FlashState::open(&path, SIZE).unwrap();
        assert_eq!(state.temporary, temporary, "one name per flash file");
        let saved = |n: u8| {
            let mut log = Vec::new();
            state.save(&content(n), &mut log).unwrap();
            String::from_utf8(log).unwrap()
        };
        let removal = format!(
            "inkemu: removed {}, put while the run went on at the name where a save of the flash file {} puts its temporary file; nothing to do: the flash file is saved through that name now\n",
            sanitize(&temporary.display().to_string()),
            sanitize(&path.display().to_string())
        );
        // Nothing at the name: nothing is removed, nothing is said.
        assert_eq!(saved(0), "");
        // What takes the name while the run goes on is not this run's: it is replaced, and said.
        std::fs::write(&temporary, b"planted").unwrap();
        assert_eq!(saved(1), removal);
        assert_eq!(std::fs::read(&path).unwrap(), content(1));
        #[cfg(unix)]
        {
            let victim = dir.path().join("victim");
            std::fs::write(&victim, b"untouched").unwrap();
            std::os::unix::fs::symlink(&victim, &temporary).unwrap();
            assert_eq!(saved(2), removal, "a link is said as anything else");
            assert_eq!(std::fs::read(&path).unwrap(), content(2));
            assert_eq!(std::fs::read(&victim).unwrap(), b"untouched");
        }
        assert!(!temporary.exists());
    }

    /// A directory that lets nothing be removed, with something at the temporary name: the
    /// cause given is the directory, which removing the thing by hand would meet as well.
    #[cfg(unix)]
    #[test]
    fn what_sits_at_the_temporary_name_of_a_directory_that_takes_no_change_names_the_directory() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let kept_in = dir.path().join("flash");
        std::fs::create_dir(&kept_in).unwrap();
        // An earlier run left its lock file, a killed save its temporary file.
        std::fs::write(kept_in.join("x4.bin.lock"), b"").unwrap();
        std::fs::write(kept_in.join(".x4.bin.tmp"), b"a part of a flash").unwrap();
        let mode = |mode| {
            std::fs::set_permissions(&kept_in, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        mode(0o555);
        let administrator = std::fs::File::create(kept_in.join("probe")).is_ok();
        let begun = RunFlash::begin(Some(&kept_in.join("x4.bin")), false, None, MIB);
        let accepted = begun.as_ref().map(|(flash, _)| accept(flash));
        mode(0o755);
        assert!(
            !administrator,
            "this test needs a user whom a read-only directory stops; run it as one"
        );
        let (accepted, log) = accepted.expect("the name is taken: nothing is tried at the start");
        assert_eq!(log, "", "nothing was removed");
        let text = accepted.unwrap_err().to_string();
        assert!(
            text.contains("cannot remove ")
                && text.contains(".x4.bin.tmp")
                && text.contains("; the directory does not let you remove a name in it")
                && text.contains("make the directory writable for you")
                && !text.contains("remove it, then run again"),
            "{text}"
        );
        assert_eq!(
            std::fs::read(kept_in.join(".x4.bin.tmp")).unwrap(),
            b"a part of a flash"
        );
    }

    #[test]
    fn what_sits_at_the_temporary_name_and_cannot_be_removed_refuses_the_run_before_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".x4.bin.tmp")).unwrap();
        let (flash, _) = RunFlash::begin(Some(&dir.path().join("x4.bin")), false, None, MIB)
            .expect("the name is taken: there is nothing to try, and nothing is removed yet");
        let (accepted, log) = accept(&flash);
        assert_eq!(log, "", "nothing was removed");
        let err = accepted.unwrap_err();
        assert!(
            matches!(
                err,
                FlashStateError::Unwritable {
                    action: "remove",
                    ..
                }
            ),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains(".x4.bin.tmp") && text.contains("; remove it, then run again"),
            "{text}"
        );
    }

    #[test]
    fn a_file_that_changed_while_it_was_read_is_said_to_hold_fewer_bytes_or_more() {
        // The read stops one byte past the size: a longer file is "more", never a count.
        assert_eq!(found_instead(u64::from(SIZE) + 1, SIZE), "more");
        assert_eq!(
            found_instead(u64::from(SIZE) - 1, SIZE),
            format!("{}", SIZE - 1)
        );
        assert_eq!(found_instead(0, SIZE), "0");
        let text = FlashStateError::Changed {
            path: "x4.bin".to_owned(),
            size: SIZE,
            found: found_instead(12, SIZE),
        }
        .to_string();
        assert!(
            text.contains(&format!(
                "it held {SIZE} bytes, then 12; make sure nothing else"
            )),
            "{text}"
        );
    }

    #[test]
    fn a_file_cut_between_the_look_and_the_read_is_refused_not_padded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        save(&state, &content(1)).unwrap();
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
        assert!(
            err.to_string()
                .contains("; make sure nothing else uses it, then run again"),
            "a file that is gone has no permissions to check: {err}"
        );
    }

    #[test]
    fn a_path_that_goes_through_a_file_is_refused_with_the_path_to_check_not_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("firmware.bin"), b"a file").unwrap();
        let path = dir.path().join("firmware.bin").join("x4.bin");
        let err = FlashState::open(&path, SIZE).unwrap_err();
        // Unix says so of the look at the flash file; elsewhere the creation of the lock file
        // fails, with its own remedy.
        #[cfg(unix)]
        {
            assert!(matches!(err, FlashStateError::Read { .. }), "{err}");
            let text = err.to_string();
            assert!(
                text.contains("; check the path") && !text.contains("permissions"),
                "{text}"
            );
        }
        assert_eq!(names(dir.path()), ["firmware.bin"], "{err}");
    }

    #[test]
    fn the_count_of_modified_sectors_agrees_with_its_verb() {
        assert_eq!(sectors_modified(0), "0 of its 4 KiB sectors were modified");
        assert_eq!(sectors_modified(1), "1 of its 4 KiB sectors was modified");
        assert_eq!(sectors_modified(2), "2 of its 4 KiB sectors were modified");
        assert_eq!(
            sectors_modified(4096),
            "4096 of its 4 KiB sectors were modified"
        );
    }

    #[test]
    fn a_flash_file_removed_while_the_run_went_on_is_written_again_though_nothing_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let held = vec![0x11u8; MIB as usize];
        std::fs::write(&path, &held).unwrap();
        let (flash, content) = RunFlash::begin(Some(&path), false, None, MIB).unwrap();
        assert!(flash.starts_from_the_file());
        std::fs::remove_file(&path).unwrap();
        let (kept, log) = finish(&flash, &ran(content, &[]));
        assert!(kept, "{log}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            held,
            "the flash of the device is in its file again"
        );
        let shown = sanitize(&path.display().to_string());
        assert!(
            log.ends_with(&format!(
                "inkemu: flash at end: sha256 {}; not modified; {shown} was removed while the run went on and is written again\n",
                hex(&Sha256::digest(&held))
            )),
            "{log}"
        );
        // Written again, it is there: the next end of a run that changed nothing leaves it.
        let before = identity(&path);
        let (kept, log) = finish(&flash, &ran(Some(held), &[]));
        assert!(kept && log.contains("not modified; "), "{log}");
        assert!(log.contains("is left as it is"), "{log}");
        assert_eq!(identity(&path), before);
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

    /// [`seed`], given by `--flash`.
    fn flash_seed() -> Seed {
        Seed {
            input: Input::Flash,
            content: seed(),
        }
    }

    #[test]
    fn without_a_flash_file_the_run_starts_from_the_seed_and_says_what_it_does_not_keep() {
        let (flash, content) = RunFlash::begin(None, false, Some(flash_seed()), MIB).unwrap();
        assert_eq!(content, Some(seed()));
        assert!(flash.state.is_none() && !flash.starts_from_the_file());
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
                "{start}inkemu: flash at end: 1 of its 4 KiB sectors was modified and is not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next\n"
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
        let (flash, content) =
            RunFlash::begin(Some(&path), false, Some(flash_seed()), MIB).unwrap();
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
                "inkemu: flash at end: sha256 {first_hash}; 1 of its 4 KiB sectors was modified; saved to {}\n",
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
        let err = RunFlash::begin(Some(&path), false, Some(flash_seed()), MIB).unwrap_err();
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
        let (flash, content) = RunFlash::begin(Some(&path), true, Some(flash_seed()), MIB).unwrap();
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
    fn the_image_is_refused_as_the_flash_file_by_every_route_to_it_and_by_no_other() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("x4.bin");
        std::fs::write(&image, seed()).unwrap();
        let refused = |route: &str, image: &Path, flash_file: &Path| {
            let err = refuse_the_image_as_flash_file(Input::Flash, image, flash_file).unwrap_err();
            assert!(
                matches!(err, FlashStateError::SameFile { .. }),
                "{route}: {err}"
            );
            let text = err.to_string();
            assert!(
                text.contains("name the same file")
                    && text.contains("; drop --flash to go on from the file")
                    && text.contains("give --flash-file another path")
                    && !text.contains("--reset-flash"),
                "{route}: {text}"
            );
        };
        refused("the same path", &image, &image);
        refused(
            "another spelling",
            &image,
            &dir.path().join(".").join("x4.bin"),
        );
        #[cfg(unix)]
        {
            let hard_link = dir.path().join("hard-link.bin");
            std::fs::hard_link(&image, &hard_link).unwrap();
            refused("a hard link", &hard_link, &image);
            let link = dir.path().join("link.bin");
            std::os::unix::fs::symlink(&image, &link).unwrap();
            refused("a link as the image", &link, &image);
            refused("a link as the flash file", &image, &link);
        }
        // Another file with the same bytes, a file that is not there yet, and none at all.
        let copy = dir.path().join("copy.bin");
        std::fs::write(&copy, seed()).unwrap();
        refuse_the_image_as_flash_file(Input::Flash, &image, &copy).unwrap();
        refuse_the_image_as_flash_file(Input::Flash, &image, &dir.path().join("new.bin")).unwrap();
        refuse_the_image_as_flash_file(Input::Flash, &dir.path().join("gone.bin"), &image).unwrap();
        refuse_the_image_as_flash_file(Input::Flash, &dir.path().join("a"), &dir.path().join("a"))
            .unwrap();
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
