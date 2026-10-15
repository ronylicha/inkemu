//! The image file of a device's SD card (design `SP2e`, 2e-D13, ADR 0014): the card's blocks, as
//! bytes and nothing else, from block 0, exactly the card's capacity.
//!
//! A run opens the image once and locks it (exclusively to write it, shared to only read it).
//! A life reads its blocks as the card asks for them ([`FileBlocks`]) and writes into an
//! [`Overlay`](chips::sdcard::Overlay) in memory; when the life ends, [`SdImage::commit`] writes
//! what changed **in place**, through a journal: the changes go to `.<name>.journal.tmp`, which is
//! flushed and renamed `.<name>.journal`, then applied to the image, which is flushed, and the
//! journal is removed. Whatever interrupts this, the image is the one before the life or the one
//! after it:
//!
//! - a journal found when the image is opened is checked and applied first;
//! - a save whose application failed leaves its journal, and the image remembers it
//!   (`pending`): before the next life reads ([`SdImage::source`]) or saves
//!   ([`SdImage::commit`]), that journal is applied again, and when it still cannot be the life is
//!   refused ([`SdImageError::UnfinishedSave`]). A later save never writes over a journal that was
//!   not applied.
//!
//! A journal is bound to its image: its header holds the identity of the image file (device and
//! inode on Unix), which its SHA-256 covers. It is read in a stream, its length checked against
//! the longest journal the card can have before anything is read: nothing is allocated from what
//! the file says. An erased range is punched out of the file where the filesystem can (Linux), so
//! erasing a card frees its space instead of writing zeros.
//!
//! The image is never replaced, so the lock is on the image itself (no `.lock` file). The lock is
//! advisory: another program that writes the image during a run makes the run irreproducible;
//! its size is checked again at each commit.
use crate::flash_state::{open_in_place, sibling};
use crate::session_file::Claimed;
use crate::term::sanitize;
use chips::sdcard::{BLOCK_BYTES, Block, BlockSource, Capacity, Changes, StorageError};
use sha2::{Digest as _, Sha256};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufReader, BufWriter, Read, Seek as _, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The first line of a journal, its version.
const JOURNAL_MAGIC: &[u8] = b"inkemu-sd-journal 2\n";
/// The identity of the image file in a journal's header.
const IDENTITY_BYTES: usize = 16;
const SHA256_BYTES: usize = 32;
const BLOCK: u64 = BLOCK_BYTES as u64;
/// The magic, the identity, the card's blocks, the two counts and the digest.
const JOURNAL_FIXED_BYTES: u64 = JOURNAL_MAGIC.len() as u64 + IDENTITY_BYTES as u64 + 3 * 8 + 32;
/// An erased range in a journal: its first block and its count.
const RANGE_BYTES: u64 = 16;
/// A written block in a journal: its number and its bytes.
const WRITTEN_BYTES: u64 = 8 + BLOCK;
/// The zeros an erased range is written with, a chunk at a time, where no hole can be punched.
const ZERO_CHUNK: u64 = 1024 * 1024;
/// What a journal is read by, a chunk at a time.
const READ_CHUNK: usize = 64 * 1024;

/// The mode of a journal on Unix: the card's content is the user's alone (ADR 0004 point 5).
#[cfg(unix)]
const JOURNAL_MODE: u32 = 0o600;

/// How a run uses the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The card can be written; the image is locked exclusively and written when a life ends.
    ReadWrite,
    /// The card is write-protected (design 2e-D9); the image is locked shared and never written.
    ReadOnly,
}

/// Why an image cannot be used, read or written. The messages state the cause, then the remedy;
/// the paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum SdImageError {
    #[error("the SD image path {path} names no file; give the path of a file, like card.img")]
    NoFileName { path: String },
    #[error(
        "there is no SD image at {path}; create one with `inkemu sd create {path} --size 1024`, or give the path of an existing image"
    )]
    Missing { path: String },
    #[error(
        "the SD image {path} is not a regular file (a link is not followed: the image is written in place); give the path of the file itself"
    )]
    NotAFile { path: String },
    #[error("cannot open the SD image {path}: {detail}; {remedy}")]
    Open {
        path: String,
        detail: String,
        remedy: &'static str,
    },
    #[error("the SD image {path} cannot be a card: {reason}")]
    Size { path: String, reason: String },
    #[error("the SD image {path} is {what}; give the card another image")]
    Claimed { path: String, what: String },
    #[error(
        "the SD image {path} has its journal at {journal}, which is {what}; give the card another image"
    )]
    JournalClaimed {
        path: String,
        journal: String,
        what: String,
    },
    #[error(
        "the SD image {path} is in use by another inkemu run; wait for that run to end, or give this one another image"
    )]
    InUse { path: String },
    #[error(
        "cannot lock the SD image {path}: {detail}; give a path on a filesystem that supports file locks"
    )]
    Lock { path: String, detail: String },
    #[error(
        "the SD image {path} has a journal, {journal}, left by a save that did not end, and is opened read-only; open it once for writing so that the save ends"
    )]
    PendingJournal { path: String, journal: String },
    #[error(
        "the journal {journal} of the SD image {path} is not one inkemu wrote for it ({why}); nothing was applied: move the journal away, and the image is as the last complete save left it"
    )]
    BadJournal {
        path: String,
        journal: String,
        why: String,
    },
    #[error(
        "the journal {journal} of the SD image {path} is {len} bytes, more than any journal of this card ({max}); nothing was applied: move the journal away, and the image is as the last complete save left it"
    )]
    JournalTooLong {
        path: String,
        journal: String,
        len: u64,
        max: u64,
    },
    #[error(
        "the last save of the SD image {path} did not end, and its journal {journal} cannot be applied now: {detail}; the card is not used until it is: free the cause, then start again"
    )]
    UnfinishedSave {
        path: String,
        journal: String,
        detail: String,
    },
    #[error(
        "the changes of this life do not fit the SD image {path} ({why}); this is a fault of inkemu: nothing is written"
    )]
    InvalidChanges { path: String, why: String },
    #[error(
        "the SD image {path} was {was} bytes when the run opened it and is {now} now; something else writes it: what this life wrote is not saved"
    )]
    Changed { path: String, was: u64, now: String },
    #[error(
        "the SD image {path} is opened read-only, and the card changed; this is a fault of inkemu: nothing is written"
    )]
    ReadOnly { path: String },
    #[error(
        "cannot save the SD image {path}: cannot {action} {target}: {detail}; check that the directory is writable and has room, then run again"
    )]
    Write {
        path: String,
        action: &'static str,
        target: String,
        detail: String,
    },
}

fn shown(path: &Path) -> String {
    sanitize(&path.display().to_string())
}

/// The journal of the image at `path`.
pub(crate) fn journal_path(path: &Path) -> Option<PathBuf> {
    sibling(path, ".", ".journal")
}

/// The name the journal is written to before it takes its own.
pub(crate) fn journal_temporary_path(path: &Path) -> Option<PathBuf> {
    sibling(path, ".", ".journal.tmp")
}

/// The files a run writes or holds for the image at `path`, whether they exist yet or not,
/// each with what the run does with it: for the refusals of the other files of a run
/// (`--record`, `--flash-file`, a capture), which must not be any of them.
#[must_use]
pub fn files_of_the_sd_image(path: &Path) -> Vec<(PathBuf, &'static str)> {
    [
        (
            Some(path.to_path_buf()),
            "the SD image, which the run writes in place when a life ends",
        ),
        (
            journal_path(path),
            "the journal of the SD image, where a life's changes go before they are applied",
        ),
        (
            journal_temporary_path(path),
            "the temporary file of the journal of the SD image",
        ),
    ]
    .into_iter()
    .filter_map(|(file, what)| file.map(|file| (file, what)))
    .collect()
}

/// The longest journal a card of `blocks` blocks can have: at most one erased range every other
/// block (ranges are neither empty nor adjacent) and every block written once.
#[must_use]
pub fn max_journal_bytes(blocks: u64) -> u64 {
    let total = u128::from(JOURNAL_FIXED_BYTES)
        + u128::from(RANGE_BYTES) * u128::from(blocks.div_ceil(2))
        + u128::from(WRITTEN_BYTES) * u128::from(blocks);
    u64::try_from(total).unwrap_or(u64::MAX)
}

/// What a commit, or the application of a journal left, wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Committed {
    /// Blocks written.
    pub blocks: usize,
    /// Ranges erased.
    pub erased_ranges: usize,
    /// Bytes of zeros written for the erased ranges, where the filesystem could not punch a
    /// hole: 0 when it could.
    pub zeros_written: u64,
}

impl Committed {
    /// What the user should know: the filesystem could not free the erased blocks.
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        (self.zeros_written > 0).then(|| {
            format!(
                "the filesystem of the SD image cannot punch holes: {} bytes of zeros were written over the erased blocks that held data; the image keeps its size on the disk",
                self.zeros_written
            )
        })
    }
}

/// The image of one run, locked for as long as this value lives.
#[derive(Debug)]
pub struct SdImage {
    path: PathBuf,
    shown: String,
    journal: PathBuf,
    temporary: PathBuf,
    access: Access,
    capacity: Capacity,
    /// The identity of the image file, which binds its journals to it.
    identity: [u8; IDENTITY_BYTES],
    /// The image, open and locked. Closing it releases the lock.
    file: File,
    /// A save wrote its journal and did not apply it all: the journal is applied again before
    /// anything else reads or writes the card.
    pending: bool,
    /// The number of writes to the image after which a write fails: a fault the tests inject.
    /// Always `None` outside them.
    fail_after: Option<usize>,
}

/// What a walk through a journal does with its entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Walk {
    Check,
    Apply,
}

/// Why a walk through a journal stopped.
enum WalkError {
    /// The journal is not one this image takes.
    Bad(String),
    /// The journal cannot be read.
    Read(std::io::Error),
    /// The image cannot be written.
    Write(&'static str, std::io::Error),
}

impl SdImage {
    /// Opens and locks the image at `path`. A journal left by a save that did not end is applied
    /// (and `log` says so); a temporary journal left is removed (and said). Nothing else of the
    /// image is read. Read-only, nothing is removed or applied: a journal present is refused.
    ///
    /// # Errors
    /// [`SdImageError`]: the path names no file, is one the run already reads or writes
    /// (`claimed`), holds no regular file, cannot be opened, is no card's size, is locked by
    /// another run, or has a journal that cannot be applied.
    pub fn open(
        path: &Path,
        access: Access,
        claimed: &[Claimed],
        log: &mut dyn Write,
    ) -> Result<SdImage, SdImageError> {
        let shown_path = shown(path);
        let no_name = || SdImageError::NoFileName {
            path: shown_path.clone(),
        };
        let name = path.file_name().ok_or_else(no_name)?;
        if !path
            .as_os_str()
            .as_encoded_bytes()
            .ends_with(name.as_encoded_bytes())
        {
            return Err(no_name());
        }
        let (Some(journal), Some(temporary)) = (journal_path(path), journal_temporary_path(path))
        else {
            return Err(no_name());
        };
        for claim in claimed {
            if claim.holds(path) {
                return Err(SdImageError::Claimed {
                    path: shown_path,
                    what: claim.what().to_owned(),
                });
            }
            for own in [&journal, &temporary] {
                if claim.holds(own) {
                    return Err(SdImageError::JournalClaimed {
                        path: shown_path,
                        journal: shown(own),
                        what: claim.what().to_owned(),
                    });
                }
            }
        }
        let open_error = |e: &std::io::Error, remedy| SdImageError::Open {
            path: shown_path.clone(),
            detail: sanitize(&e.to_string()),
            remedy,
        };
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(SdImageError::NotAFile { path: shown_path }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SdImageError::Missing { path: shown_path });
            }
            Err(e) => return Err(open_error(&e, "check the path and its permissions")),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(access == Access::ReadWrite);
        let file = open_in_place(&mut options, path).map_err(|e| {
            open_error(
                &e,
                match (e.kind(), access) {
                    (std::io::ErrorKind::PermissionDenied, Access::ReadWrite) => {
                        "make it writable for you, or open it read-only: the card is then write-protected"
                    }
                    _ => "check the path and its permissions",
                },
            )
        })?;
        let metadata = file
            .metadata()
            .map_err(|e| open_error(&e, "check the path and its permissions"))?;
        if !metadata.is_file() {
            return Err(SdImageError::NotAFile { path: shown_path });
        }
        let capacity = Capacity::new(metadata.len()).map_err(|reason| SdImageError::Size {
            path: shown_path.clone(),
            reason: reason.to_string(),
        })?;
        let identity = identity_of(&file, path)
            .map_err(|e| open_error(&e, "check the path and its permissions"))?;
        let locked = match access {
            Access::ReadWrite => file.try_lock(),
            Access::ReadOnly => file.try_lock_shared(),
        };
        match locked {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(SdImageError::InUse { path: shown_path }),
            Err(TryLockError::Error(e)) => {
                return Err(SdImageError::Lock {
                    path: shown_path,
                    detail: sanitize(&e.to_string()),
                });
            }
        }
        let mut image = SdImage {
            path: path.to_path_buf(),
            shown: shown_path,
            journal,
            temporary,
            access,
            capacity,
            identity,
            file,
            pending: false,
            fail_after: None,
        };
        image.recover(log)?;
        Ok(image)
    }

    /// The path of the image, with its control characters escaped: for messages.
    #[must_use]
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// The capacity of the card the image is.
    #[must_use]
    pub fn capacity(&self) -> Capacity {
        self.capacity
    }

    /// Whether the card is write-protected: the image was opened read-only.
    #[must_use]
    pub fn write_protected(&self) -> bool {
        self.access == Access::ReadOnly
    }

    /// The image as the source a life's card reads, one per life. A save that did not end is
    /// finished first: a life never reads an image half written.
    ///
    /// # Errors
    /// [`SdImageError::UnfinishedSave`] when the journal of the last save cannot be applied,
    /// [`SdImageError::Open`] when the system cannot give a second handle on the open file.
    pub fn source(&mut self) -> Result<FileBlocks, SdImageError> {
        self.finish_pending()?;
        let file = self.file.try_clone().map_err(|e| SdImageError::Open {
            path: self.shown.clone(),
            detail: sanitize(&e.to_string()),
            remedy: "check the path and its permissions",
        })?;
        Ok(FileBlocks {
            file,
            blocks: self.capacity.blocks(),
        })
    }

    /// Writes `changes`, what a life changed, into the image, through the journal. A save that
    /// did not end is finished first; nothing is written when nothing changed.
    ///
    /// # Errors
    /// [`SdImageError::UnfinishedSave`] when the journal of the last save cannot be applied
    /// (this one is then not written), [`SdImageError::InvalidChanges`] for changes that do not
    /// fit the card, [`SdImageError::Changed`] when the image's size changed since it was
    /// opened, [`SdImageError::ReadOnly`] for changes to an image opened read-only,
    /// [`SdImageError::Write`] when the journal or the image cannot be written. When the journal
    /// was written but its application failed, it is applied again before the next life, and at
    /// the next opening.
    pub fn commit(&mut self, changes: &Changes<'_>) -> Result<Committed, SdImageError> {
        self.finish_pending()?;
        if changes.is_empty() {
            return Ok(Committed::default());
        }
        if self.access == Access::ReadOnly {
            return Err(SdImageError::ReadOnly {
                path: self.shown.clone(),
            });
        }
        self.check_changes(changes)?;
        self.check_size()?;
        self.write_journal(changes)?;
        self.pending = true;
        let committed = self.apply_changes(changes)?;
        self.file
            .sync_all()
            .map_err(|e| self.failed("write", &self.path, &e))?;
        self.remove_journal()?;
        self.pending = false;
        Ok(committed)
    }

    /// Applies the journal of a save that did not end, if there is one, and removes it.
    fn finish_pending(&mut self) -> Result<(), SdImageError> {
        let journal_there = std::fs::symlink_metadata(&self.journal).is_ok();
        if !self.pending && !journal_there {
            return Ok(());
        }
        if self.access == Access::ReadOnly {
            return Err(SdImageError::PendingJournal {
                path: self.shown.clone(),
                journal: shown(&self.journal),
            });
        }
        self.replay_journal()
            .and_then(|_| self.remove_journal())
            .map_err(|e| SdImageError::UnfinishedSave {
                path: self.shown.clone(),
                journal: shown(&self.journal),
                detail: e.to_string(),
            })?;
        self.pending = false;
        Ok(())
    }

    /// Changes are made by an overlay over this image: they fit it. Checked all the same, before
    /// anything is written.
    fn check_changes(&self, changes: &Changes<'_>) -> Result<(), SdImageError> {
        let blocks = self.capacity.blocks();
        let invalid = |why: String| SdImageError::InvalidChanges {
            path: self.shown.clone(),
            why,
        };
        for (first, count) in changes.erased() {
            if count == 0 || first.checked_add(count).is_none_or(|end| end > blocks) {
                return Err(invalid(format!(
                    "an erased range of {count} blocks from block {first}, on a card of {blocks} blocks"
                )));
            }
        }
        if let Some((lba, _)) = changes.blocks().find(|&(lba, _)| lba >= blocks) {
            return Err(invalid(format!(
                "block {lba} written, on a card of {blocks} blocks"
            )));
        }
        Ok(())
    }

    fn check_size(&self) -> Result<(), SdImageError> {
        let now = self
            .file
            .metadata()
            .map(|metadata| metadata.len())
            .map_err(|e| sanitize(&e.to_string()));
        match now {
            Ok(len) if len == self.capacity.bytes() => Ok(()),
            Ok(len) => Err(SdImageError::Changed {
                path: self.shown.clone(),
                was: self.capacity.bytes(),
                now: len.to_string(),
            }),
            Err(detail) => Err(SdImageError::Changed {
                path: self.shown.clone(),
                was: self.capacity.bytes(),
                now: format!("unknown ({detail})"),
            }),
        }
    }

    fn failed(&self, action: &'static str, target: &Path, error: &std::io::Error) -> SdImageError {
        SdImageError::Write {
            path: self.shown.clone(),
            action,
            target: shown(target),
            detail: sanitize(&error.to_string()),
        }
    }

    /// Writes the journal of `changes` through its temporary name, flushed, then renamed.
    fn write_journal(&self, changes: &Changes<'_>) -> Result<(), SdImageError> {
        // A temporary journal can only be the leftover of an interrupted save: this run holds
        // the lock. It is removed (a link itself, never what it names).
        if std::fs::symlink_metadata(&self.temporary).is_ok() {
            std::fs::remove_file(&self.temporary)
                .map_err(|e| self.failed("remove", &self.temporary, &e))?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(JOURNAL_MODE);
        }
        let written = options.open(&self.temporary).and_then(|file| {
            let mut out = BufWriter::new(file);
            encode_journal(&mut out, &self.identity, self.capacity.blocks(), changes)?;
            out.into_inner()
                .map_err(std::io::IntoInnerError::into_error)?
                .sync_all()
        });
        if let Err(e) = written {
            let _ = std::fs::remove_file(&self.temporary);
            return Err(self.failed("write", &self.temporary, &e));
        }
        std::fs::rename(&self.temporary, &self.journal).map_err(|e| {
            let _ = std::fs::remove_file(&self.temporary);
            self.failed("move into place", &self.journal, &e)
        })?;
        sync_directory(&self.journal);
        Ok(())
    }

    /// Applies checked changes to the image.
    fn apply_changes(&mut self, changes: &Changes<'_>) -> Result<Committed, SdImageError> {
        let mut committed = Committed::default();
        for (first, count) in changes.erased() {
            let zeros = self
                .erase_range(first, count)
                .map_err(|e| self.failed("erase blocks of", &self.path, &e))?;
            committed.zeros_written += zeros;
            committed.erased_ranges += 1;
        }
        for (lba, block) in changes.blocks() {
            self.write_at(lba * BLOCK, block)
                .map_err(|e| self.failed("write", &self.path, &e))?;
            committed.blocks += 1;
        }
        Ok(committed)
    }

    /// A fault the tests inject: the write fails once `fail_after` writes went through.
    fn injected_fault(&mut self) -> std::io::Result<()> {
        match self.fail_after {
            Some(0) => Err(std::io::Error::other("a write failure the test injects")),
            Some(left) => {
                self.fail_after = Some(left - 1);
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
        self.injected_fault()?;
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)
    }

    /// Makes `count` blocks from `first` read zeros: a hole punched where the filesystem can,
    /// zeros written over the data elsewhere. The bytes of zeros written.
    fn erase_range(&mut self, first: u64, count: u64) -> std::io::Result<u64> {
        self.injected_fault()?;
        let offset = first.saturating_mul(BLOCK);
        let len = count.saturating_mul(BLOCK);
        if punch_hole(&self.file, offset, len).is_ok() {
            return Ok(0);
        }
        zero_data(&self.file, offset, len)
    }

    fn remove_journal(&self) -> Result<(), SdImageError> {
        std::fs::remove_file(&self.journal)
            .map_err(|e| self.failed("remove", &self.journal, &e))?;
        sync_directory(&self.journal);
        Ok(())
    }

    /// What an interrupted save left: a temporary journal is removed, a journal is applied.
    /// Read-only, nothing is touched or read: a journal present is refused.
    fn recover(&mut self, log: &mut dyn Write) -> Result<(), SdImageError> {
        if self.access == Access::ReadOnly {
            if std::fs::symlink_metadata(&self.journal).is_ok() {
                return Err(SdImageError::PendingJournal {
                    path: self.shown.clone(),
                    journal: shown(&self.journal),
                });
            }
            return Ok(());
        }
        if std::fs::symlink_metadata(&self.temporary).is_ok() {
            std::fs::remove_file(&self.temporary)
                .map_err(|e| self.failed("remove", &self.temporary, &e))?;
            let _ = writeln!(
                log,
                "inkemu: removed {}, the journal of a save of the SD image {} that was interrupted before it was complete; the image is as the save before it left it",
                shown(&self.temporary),
                self.shown
            );
        }
        match std::fs::symlink_metadata(&self.journal) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(self.bad_journal(format!(
                    "it cannot be looked at: {}",
                    sanitize(&e.to_string())
                )));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(self
                    .bad_journal("it is not a regular file (a link is not followed)".to_owned()));
            }
            Ok(_) => {}
        }
        let committed = self.replay_journal()?;
        self.remove_journal()?;
        let _ = writeln!(
            log,
            "inkemu: finished the save of the SD image {} that was interrupted ({} blocks, {} erased ranges)",
            self.shown, committed.blocks, committed.erased_ranges
        );
        if let Some(notice) = committed.notice() {
            let _ = writeln!(log, "inkemu: {notice}");
        }
        Ok(())
    }

    fn bad_journal(&self, why: String) -> SdImageError {
        SdImageError::BadJournal {
            path: self.shown.clone(),
            journal: shown(&self.journal),
            why,
        }
    }

    /// Checks the journal on the disk whole, then applies it, reading it in a stream. Nothing
    /// is written unless all of it is checked.
    fn replay_journal(&mut self) -> Result<Committed, SdImageError> {
        let read_error = |image: &SdImage, e: &std::io::Error| {
            image.bad_journal(format!("it cannot be read: {}", sanitize(&e.to_string())))
        };
        // A pipe is not waited on and a link is not followed; what was opened is judged.
        let journal = open_in_place(OpenOptions::new().read(true), &self.journal)
            .map_err(|e| read_error(self, &e))?;
        let metadata = journal.metadata().map_err(|e| read_error(self, &e))?;
        if !metadata.is_file() {
            return Err(
                self.bad_journal("it is not a regular file (a link is not followed)".to_owned())
            );
        }
        let len = metadata.len();
        let max = max_journal_bytes(self.capacity.blocks());
        if len > max {
            return Err(SdImageError::JournalTooLong {
                path: self.shown.clone(),
                journal: shown(&self.journal),
                len,
                max,
            });
        }
        if len < JOURNAL_FIXED_BYTES {
            return Err(self.bad_journal("it is shorter than any journal".to_owned()));
        }
        let body = len - SHA256_BYTES as u64;
        if !digest_matches(&journal, body).map_err(|e| read_error(self, &e))? {
            return Err(self.bad_journal(
                "its SHA-256 does not match: it is incomplete or damaged".to_owned(),
            ));
        }
        let to_error = |image: &mut SdImage, error: WalkError| match error {
            WalkError::Bad(why) => image.bad_journal(why),
            WalkError::Read(e) => read_error(image, &e),
            WalkError::Write(action, e) => {
                image.pending = true;
                image.failed(action, &image.path, &e)
            }
        };
        if let Err(error) = self.walk(&journal, body, Walk::Check) {
            return Err(to_error(self, error));
        }
        let committed = match self.walk(&journal, body, Walk::Apply) {
            Ok(committed) => committed,
            Err(error) => return Err(to_error(self, error)),
        };
        self.file.sync_all().map_err(|e| {
            self.pending = true;
            self.failed("write", &self.path, &e)
        })?;
        Ok(committed)
    }

    /// Reads the `body` bytes of a journal (all but its digest) in order, checking each entry,
    /// and with [`Walk::Apply`] writing it to the image.
    fn walk(&mut self, journal: &File, body: u64, walk: Walk) -> Result<Committed, WalkError> {
        let mut file = journal;
        file.seek(SeekFrom::Start(0)).map_err(WalkError::Read)?;
        let mut reader = Entries {
            reader: BufReader::with_capacity(READ_CHUNK, file).take(body),
        };
        let mut magic = [0u8; JOURNAL_MAGIC.len()];
        reader.fill(&mut magic, "its first line")?;
        if magic != JOURNAL_MAGIC {
            return Err(WalkError::Bad(
                "it does not begin with the line of a journal".to_owned(),
            ));
        }
        let mut identity = [0u8; IDENTITY_BYTES];
        reader.fill(&mut identity, "the identity of its image")?;
        if identity != self.identity {
            return Err(WalkError::Bad(
                "it was written for another image file".to_owned(),
            ));
        }
        let capacity = self.capacity.blocks();
        let blocks = reader.u64("the size of the card")?;
        if blocks != capacity {
            return Err(WalkError::Bad(format!(
                "it is for a card of {blocks} blocks, and the image holds {capacity}"
            )));
        }
        let mut committed = Committed::default();
        let ranges = reader.u64("the number of erased ranges")?;
        if ranges > capacity.div_ceil(2) {
            return Err(WalkError::Bad(format!(
                "it counts {ranges} erased ranges, more than a card of {capacity} blocks has"
            )));
        }
        let mut previous_end: Option<u64> = None;
        for _ in 0..ranges {
            let first = reader.u64("an erased range")?;
            let count = reader.u64("an erased range")?;
            let end = first
                .checked_add(count)
                .filter(|&end| count > 0 && end <= capacity);
            let Some(end) = end else {
                return Err(WalkError::Bad(format!(
                    "its erased range of {count} blocks from {first} is not on the card"
                )));
            };
            if previous_end.is_some_and(|previous| first <= previous) {
                return Err(WalkError::Bad(
                    "its erased ranges are not in order, apart from each other".to_owned(),
                ));
            }
            previous_end = Some(end);
            if walk == Walk::Apply {
                committed.zeros_written += self
                    .erase_range(first, count)
                    .map_err(|e| WalkError::Write("erase blocks of", e))?;
            }
            committed.erased_ranges += 1;
        }
        let written = reader.u64("the number of written blocks")?;
        if written > capacity {
            return Err(WalkError::Bad(format!(
                "it counts {written} written blocks, more than the card's {capacity}"
            )));
        }
        let mut previous: Option<u64> = None;
        let mut block = [0u8; BLOCK_BYTES];
        for _ in 0..written {
            let lba = reader.u64("a written block")?;
            if lba >= capacity {
                return Err(WalkError::Bad(format!(
                    "its block {lba} is not on the card"
                )));
            }
            if previous.is_some_and(|previous| lba <= previous) {
                return Err(WalkError::Bad(
                    "its written blocks are not in order".to_owned(),
                ));
            }
            previous = Some(lba);
            reader.fill(&mut block, "a written block")?;
            if walk == Walk::Apply {
                self.write_at(lba * BLOCK, &block)
                    .map_err(|e| WalkError::Write("write", e))?;
            }
            committed.blocks += 1;
        }
        if reader.reader.limit() != 0 {
            return Err(WalkError::Bad(
                "it has bytes after its last block".to_owned(),
            ));
        }
        Ok(committed)
    }
}

/// The entries of a journal, read in a stream within its body.
struct Entries<'a> {
    reader: std::io::Take<BufReader<&'a File>>,
}

impl Entries<'_> {
    fn fill(&mut self, into: &mut [u8], what: &str) -> Result<(), WalkError> {
        self.reader.read_exact(into).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                WalkError::Bad(format!("it ends before {what}"))
            } else {
                WalkError::Read(e)
            }
        })
    }

    fn u64(&mut self, what: &str) -> Result<u64, WalkError> {
        let mut bytes = [0u8; 8];
        self.fill(&mut bytes, what)?;
        Ok(u64::from_le_bytes(bytes))
    }
}

/// Whether the SHA-256 of the first `body` bytes of `journal` is the 32 bytes after them, read
/// in a stream.
fn digest_matches(journal: &File, body: u64) -> std::io::Result<bool> {
    let mut file = journal;
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::with_capacity(READ_CHUNK, file);
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut left = body;
    while left > 0 {
        let take = usize::try_from(left.min(READ_CHUNK as u64)).unwrap_or(READ_CHUNK);
        let part = chunk.get_mut(..take).unwrap_or_default();
        reader.read_exact(part)?;
        hasher.update(&*part);
        left -= take as u64;
    }
    let mut digest = [0u8; SHA256_BYTES];
    reader.read_exact(&mut digest)?;
    Ok(hasher.finalize().as_slice() == digest)
}

/// The identity of the image file a journal is bound to: its device and inode on Unix; elsewhere,
/// where the standard library gives no identity, the SHA-256 of its canonical path.
fn identity_of(file: &File, path: &Path) -> std::io::Result<[u8; IDENTITY_BYTES]> {
    let mut identity = [0u8; IDENTITY_BYTES];
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let _ = path;
        let metadata = file.metadata()?;
        identity[..8].copy_from_slice(&metadata.dev().to_le_bytes());
        identity[8..].copy_from_slice(&metadata.ino().to_le_bytes());
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        let canonical = std::fs::canonicalize(path)?;
        let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
        identity.copy_from_slice(&digest[..IDENTITY_BYTES]);
    }
    Ok(identity)
}

/// Writes a journal: the version line, the identity of the image, the card's blocks (`u64`), the
/// erased ranges (`u64` count, then `u64` first and `u64` count each), the written blocks (`u64`
/// count, then `u64` number and 512 bytes each), then the SHA-256 of all that, little-endian.
fn encode_journal(
    out: &mut impl Write,
    identity: &[u8; IDENTITY_BYTES],
    blocks: u64,
    changes: &Changes<'_>,
) -> std::io::Result<()> {
    let mut hashing = Hashing {
        out,
        hasher: Sha256::new(),
    };
    let count = |len: usize| u64::try_from(len).unwrap_or(u64::MAX).to_le_bytes();
    hashing.write_all(JOURNAL_MAGIC)?;
    hashing.write_all(identity)?;
    hashing.write_all(&blocks.to_le_bytes())?;
    hashing.write_all(&count(changes.erased().count()))?;
    for (first, length) in changes.erased() {
        hashing.write_all(&first.to_le_bytes())?;
        hashing.write_all(&length.to_le_bytes())?;
    }
    hashing.write_all(&count(changes.written_count()))?;
    for (lba, block) in changes.blocks() {
        hashing.write_all(&lba.to_le_bytes())?;
        hashing.write_all(block)?;
    }
    let digest = hashing.hasher.finalize();
    hashing.out.write_all(&digest)
}

/// A writer that hashes what goes through it.
struct Hashing<'a, W: Write> {
    out: &'a mut W,
    hasher: Sha256,
}

impl<W: Write> Write for Hashing<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.out.write(bytes)?;
        self.hasher.update(bytes.get(..written).unwrap_or(bytes));
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// Frees `len` bytes from `offset` in `file`, which then read zeros, its size kept: Linux's
/// `fallocate(FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE)`.
#[cfg(target_os = "linux")]
fn punch_hole(file: &File, offset: u64, len: u64) -> std::io::Result<()> {
    use nix::fcntl::{FallocateFlags, fallocate};
    let to_off = |value: u64| {
        nix::libc::off_t::try_from(value)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    };
    fallocate(
        file,
        FallocateFlags::FALLOC_FL_PUNCH_HOLE | FallocateFlags::FALLOC_FL_KEEP_SIZE,
        to_off(offset)?,
        to_off(len)?,
    )
    .map_err(std::io::Error::from)
}

/// Elsewhere the call would need code the workspace forbids (`unsafe_code = "forbid"`): macOS's
/// `F_PUNCHHOLE` and Windows' `FSCTL_SET_ZERO_DATA` have no safe wrapper in the dependencies.
/// [`zero_data`] then writes zeros over the data only (ADR 0014).
#[cfg(not(target_os = "linux"))]
fn punch_hole(_file: &File, _offset: u64, _len: u64) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// Writes zeros over the parts of `len` bytes from `offset` that hold data, leaving the holes:
/// the file takes no more room on the disk than before. The bytes of zeros written.
fn zero_data(file: &File, offset: u64, len: u64) -> std::io::Result<u64> {
    let end = offset.saturating_add(len);
    let zeros = vec![0u8; usize::try_from(ZERO_CHUNK).unwrap_or(BLOCK_BYTES)];
    let mut written = 0;
    let mut writer = file;
    for (start, stop) in data_extents(file, offset, end)? {
        let mut at = start;
        while at < stop {
            let chunk = (stop - at).min(ZERO_CHUNK);
            let bytes = usize::try_from(chunk)
                .ok()
                .and_then(|chunk| zeros.get(..chunk))
                .unwrap_or(&zeros);
            writer.seek(SeekFrom::Start(at))?;
            writer.write_all(bytes)?;
            at += chunk;
            written += chunk;
        }
    }
    Ok(written)
}

/// The parts of `[start, end)` of `file` that hold data, by `SEEK_DATA` and `SEEK_HOLE`; the
/// whole range where the filesystem does not tell.
#[cfg(any(target_os = "linux", target_vendor = "apple", target_os = "freebsd"))]
fn data_extents(file: &File, start: u64, end: u64) -> std::io::Result<Vec<(u64, u64)>> {
    use nix::errno::Errno;
    use nix::unistd::{Whence, lseek};
    let to_off = |value: u64| {
        nix::libc::off_t::try_from(value)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    };
    let to_u64 = |value: nix::libc::off_t| u64::try_from(value).unwrap_or(0);
    let mut extents = Vec::new();
    let mut at = start;
    while at < end {
        let data = match lseek(file, to_off(at)?, Whence::SeekData) {
            Ok(data) => to_u64(data),
            // No data from `at` to the end of the file.
            Err(Errno::ENXIO) => break,
            // The filesystem does not tell its holes: the whole range is written.
            Err(Errno::EINVAL | Errno::EOPNOTSUPP) => return Ok(vec![(at, end)]),
            Err(e) => return Err(e.into()),
        };
        if data >= end {
            break;
        }
        let hole = to_u64(lseek(file, to_off(data)?, Whence::SeekHole)?).min(end);
        extents.push((data, hole.max(data)));
        at = hole.max(data + 1);
    }
    Ok(extents)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple", target_os = "freebsd")))]
fn data_extents(_file: &File, start: u64, end: u64) -> std::io::Result<Vec<(u64, u64)>> {
    Ok(vec![(start, end)])
}

/// Flushes the directory of `path`, so that a rename or a removal in it outlives a crash. Not
/// every system lets a directory be opened: best effort.
pub(crate) fn sync_directory(path: &Path) {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if let Ok(directory) = File::open(directory) {
        let _ = directory.sync_all();
    }
}

/// The image as a source of blocks: what a card reads during a life, straight from the file.
#[derive(Debug)]
pub struct FileBlocks {
    file: File,
    blocks: u64,
}

impl BlockSource for FileBlocks {
    fn block_count(&self) -> u64 {
        self.blocks
    }

    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError> {
        if lba >= self.blocks {
            return Err(StorageError::OutOfRange {
                lba,
                blocks: self.blocks,
            });
        }
        self.file
            .seek(SeekFrom::Start(lba * BLOCK))
            .and_then(|_| self.file.read_exact(into))
            .map_err(|e| StorageError::Failed {
                action: "read",
                lba,
                detail: sanitize(&e.to_string()),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chips::sdcard::{BlockStorage as _, Overlay, SparseBlocks};

    const MIB: u64 = 1024 * 1024;

    /// A sparse image of `bytes` bytes whose block `lba` holds `lba` in every byte, for the
    /// blocks listed.
    fn image(dir: &Path, name: &str, bytes: u64, numbered: &[u64]) -> PathBuf {
        let path = dir.join(name);
        let file = File::create(&path).unwrap();
        file.set_len(bytes).unwrap();
        let mut file = &file;
        for &lba in numbered {
            file.seek(SeekFrom::Start(lba * BLOCK)).unwrap();
            file.write_all(&[lba.to_le_bytes()[0]; BLOCK_BYTES])
                .unwrap();
        }
        path
    }

    fn open(path: &Path) -> SdImage {
        SdImage::open(path, Access::ReadWrite, &[], &mut Vec::new()).unwrap()
    }

    fn open_error(path: &Path, access: Access) -> String {
        SdImage::open(path, access, &[], &mut Vec::new())
            .unwrap_err()
            .to_string()
    }

    fn block_of(path: &Path, lba: u64) -> Vec<u8> {
        let bytes = std::fs::read(path).unwrap();
        let start = usize::try_from(lba * BLOCK).unwrap();
        bytes[start..start + BLOCK_BYTES].to_vec()
    }

    fn read(source: &mut impl BlockSource, lba: u64) -> Block {
        let mut block = [0xAA; BLOCK_BYTES];
        source.read_block(lba, &mut block).unwrap();
        block
    }

    /// The bytes of a journal of `changes` for `image`, as `commit` writes it.
    fn journal_of(image: &SdImage, changes: &Changes<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode_journal(
            &mut bytes,
            &image.identity,
            image.capacity.blocks(),
            changes,
        )
        .unwrap();
        bytes
    }

    #[test]
    fn a_life_reads_the_image_and_a_commit_writes_what_it_changed_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[1, 2, 3, 9]);
        let mut image = open(&path);
        assert_eq!(image.capacity().bytes(), MIB);
        assert!(!image.write_protected());
        let mut overlay = Overlay::new(image.source().unwrap());
        assert_eq!(read(&mut overlay, 3), [3; BLOCK_BYTES]);
        overlay.write_block(3, &[0x33; BLOCK_BYTES]).unwrap();
        overlay.erase(1, 2).unwrap();
        overlay.write_block(2, &[0x22; BLOCK_BYTES]).unwrap();
        let committed = image.commit(&overlay.changes()).unwrap();
        assert_eq!((committed.blocks, committed.erased_ranges), (2, 1));
        assert_eq!(block_of(&path, 1), [0; BLOCK_BYTES]);
        assert_eq!(block_of(&path, 2), [0x22; BLOCK_BYTES]);
        assert_eq!(block_of(&path, 3), [0x33; BLOCK_BYTES]);
        assert_eq!(block_of(&path, 9), [9; BLOCK_BYTES]);
        assert!(!dir.path().join(".card.img.journal").exists());
        assert!(!dir.path().join(".card.img.journal.tmp").exists());
        // The next life reads what the last one left.
        let mut next = Overlay::new(image.source().unwrap());
        assert_eq!(read(&mut next, 3), [0x33; BLOCK_BYTES]);
        assert_eq!(image.commit(&next.changes()).unwrap(), Committed::default());
        assert!(image.shown().ends_with("card.img"));
    }

    #[test]
    fn a_reopened_image_reads_what_the_last_run_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        {
            let mut image = open(&path);
            let mut overlay = Overlay::new(image.source().unwrap());
            overlay.write_block(7, &[7; BLOCK_BYTES]).unwrap();
            image.commit(&overlay.changes()).unwrap();
        }
        let mut image = open(&path);
        assert_eq!(read(&mut image.source().unwrap(), 7), [7; BLOCK_BYTES]);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), MIB, "never grown");
    }

    #[test]
    fn a_journal_left_after_its_rename_is_applied_at_the_next_opening_and_said() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[4]);
        {
            let mut image = open(&path);
            let mut overlay = Overlay::new(image.source().unwrap());
            overlay.write_block(4, &[0x44; BLOCK_BYTES]).unwrap();
            overlay.erase(10, 5).unwrap();
            image.write_journal(&overlay.changes()).unwrap();
            // The process dies here, before the journal is applied, or halfway through it.
            let mut file = &image.file;
            file.seek(SeekFrom::Start(4 * BLOCK)).unwrap();
            file.write_all(&[0x44; 100]).unwrap();
        }
        assert!(dir.path().join(".card.img.journal").exists());
        let mut log = Vec::new();
        let mut image = SdImage::open(&path, Access::ReadWrite, &[], &mut log).unwrap();
        let log = String::from_utf8(log).unwrap();
        assert!(log.contains("finished the save of the SD image"), "{log}");
        assert!(log.contains("(1 blocks, 1 erased ranges)"), "{log}");
        assert_eq!(read(&mut image.source().unwrap(), 4), [0x44; BLOCK_BYTES]);
        assert!(!dir.path().join(".card.img.journal").exists());
    }

    #[test]
    fn a_failed_application_is_finished_before_the_next_life_reads_and_never_mixed_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[1, 2, 3, 4]);
        let mut image = open(&path);
        let mut life = Overlay::new(image.source().unwrap());
        for lba in 1..=4u64 {
            life.write_block(lba, &[0x10 + lba.to_le_bytes()[0]; BLOCK_BYTES])
                .unwrap();
        }
        image.fail_after = Some(2);
        let error = image.commit(&life.changes()).unwrap_err().to_string();
        assert!(
            error.contains("a write failure the test injects"),
            "{error}"
        );
        assert!(image.pending);
        assert!(dir.path().join(".card.img.journal").exists());
        assert_eq!(block_of(&path, 3), [3; BLOCK_BYTES], "half applied");
        image.fail_after = None;
        // The next life finds the whole of the last one, not half of it.
        let mut next = Overlay::new(image.source().unwrap());
        assert!(!image.pending);
        assert!(!dir.path().join(".card.img.journal").exists());
        for lba in 1..=4u64 {
            assert_eq!(
                read(&mut next, lba),
                [0x10 + lba.to_le_bytes()[0]; BLOCK_BYTES]
            );
        }
        next.write_block(2, &[0x99; BLOCK_BYTES]).unwrap();
        image.commit(&next.changes()).unwrap();
        drop(image);
        assert_eq!(block_of(&path, 1), [0x11; BLOCK_BYTES]);
        assert_eq!(block_of(&path, 2), [0x99; BLOCK_BYTES]);
        assert_eq!(block_of(&path, 3), [0x13; BLOCK_BYTES]);
        assert_eq!(block_of(&path, 4), [0x14; BLOCK_BYTES]);
    }

    #[test]
    fn a_save_that_cannot_be_finished_refuses_the_next_life_and_keeps_its_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let mut image = open(&path);
        let mut life = Overlay::new(image.source().unwrap());
        life.write_block(5, &[5; BLOCK_BYTES]).unwrap();
        image.fail_after = Some(0);
        assert!(image.commit(&life.changes()).is_err());
        let journal = dir.path().join(".card.img.journal");
        let pending = std::fs::read(&journal).unwrap();
        image.fail_after = Some(0);
        let error = image.source().unwrap_err().to_string();
        assert!(error.contains("the last save of the SD image"), "{error}");
        let mut other = Overlay::new(SparseBlocks::new(MIB / BLOCK));
        other.write_block(6, &[6; BLOCK_BYTES]).unwrap();
        image.fail_after = Some(0);
        assert!(matches!(
            image.commit(&other.changes()),
            Err(SdImageError::UnfinishedSave { .. })
        ));
        assert_eq!(
            std::fs::read(&journal).unwrap(),
            pending,
            "never written over"
        );
        image.fail_after = None;
        image.commit(&other.changes()).unwrap();
        drop(image);
        assert_eq!(block_of(&path, 5), [5; BLOCK_BYTES]);
        assert_eq!(block_of(&path, 6), [6; BLOCK_BYTES]);
    }

    #[test]
    fn a_temporary_journal_left_before_its_rename_is_removed_and_the_image_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[4]);
        std::fs::write(dir.path().join(".card.img.journal.tmp"), b"half a journal").unwrap();
        let mut log = Vec::new();
        let mut image = SdImage::open(&path, Access::ReadWrite, &[], &mut log).unwrap();
        assert!(
            String::from_utf8(log)
                .unwrap()
                .contains("interrupted before it was complete")
        );
        assert!(!dir.path().join(".card.img.journal.tmp").exists());
        assert_eq!(read(&mut image.source().unwrap(), 4), [4; BLOCK_BYTES]);
    }

    #[test]
    fn a_journal_that_is_not_complete_or_not_for_this_image_is_refused_and_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let journal = dir.path().join(".card.img.journal");
        let image = open(&path);
        let mut overlay = Overlay::new(SparseBlocks::new(MIB / BLOCK));
        overlay.write_block(1, &[1; BLOCK_BYTES]).unwrap();
        let good = journal_of(&image, &overlay.changes());
        let mut far = Overlay::new(SparseBlocks::new(1 << 20));
        far.write_block(5000, &[1; BLOCK_BYTES]).unwrap();
        let mut beyond = Vec::new();
        encode_journal(&mut beyond, &image.identity, MIB / BLOCK, &far.changes()).unwrap();
        let mut bigger = Vec::new();
        encode_journal(
            &mut bigger,
            &image.identity,
            MIB / BLOCK * 2,
            &overlay.changes(),
        )
        .unwrap();
        drop(image);
        let cases: [(Vec<u8>, &str); 4] = [
            (good[..good.len() - 1].to_vec(), "SHA-256 does not match"),
            (b"something else".to_vec(), "shorter than any journal"),
            (
                bigger,
                "is for a card of 4096 blocks, and the image holds 2048",
            ),
            (beyond, "its block 5000 is not on the card"),
        ];
        for (bytes, why) in cases {
            std::fs::write(&journal, &bytes).unwrap();
            let error = open_error(&path, Access::ReadWrite);
            assert!(error.contains(why), "{error}");
            assert!(error.contains("nothing was applied"), "{error}");
            assert!(journal.exists(), "left for the user");
        }
        assert_eq!(std::fs::read(&path).unwrap(), vec![0; 1024 * 1024]);
    }

    #[test]
    fn a_journal_of_another_image_file_of_the_same_size_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let one = image(dir.path(), "one.img", MIB, &[]);
        let two = image(dir.path(), "two.img", MIB, &[]);
        let image = open(&one);
        let mut overlay = Overlay::new(SparseBlocks::new(MIB / BLOCK));
        overlay.write_block(1, &[1; BLOCK_BYTES]).unwrap();
        std::fs::write(
            dir.path().join(".two.img.journal"),
            journal_of(&image, &overlay.changes()),
        )
        .unwrap();
        let error = open_error(&two, Access::ReadWrite);
        assert!(error.contains("written for another image file"), "{error}");
        assert_eq!(std::fs::read(&two).unwrap(), vec![0; 1024 * 1024]);
    }

    #[test]
    fn a_journal_longer_than_any_of_its_card_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let journal = File::create(dir.path().join(".card.img.journal")).unwrap();
        // A tebibyte that takes nothing on the disk: reading it whole would abort the process.
        journal.set_len(1 << 40).unwrap();
        let error = open_error(&path, Access::ReadWrite);
        assert!(
            error.contains("1099511627776 bytes, more than any journal"),
            "{error}"
        );
        // 20 bytes of magic, 16 of identity, 3 counts of 8 and the 32 of the digest.
        assert_eq!(max_journal_bytes(2048), 92 + 16 * 1024 + 520 * 2048);
    }

    #[test]
    fn a_journal_with_as_many_ranges_and_blocks_as_a_card_can_have_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let blocks = MIB / BLOCK;
        {
            let mut image = open(&path);
            let mut overlay = Overlay::new(image.source().unwrap());
            for lba in (0..blocks).step_by(2) {
                overlay.erase(lba, 1).unwrap();
            }
            for lba in 0..blocks {
                overlay
                    .write_block(lba, &[lba.to_le_bytes()[0] | 1; BLOCK_BYTES])
                    .unwrap();
            }
            assert_eq!(overlay.changes().erased().count(), 1024);
            image.write_journal(&overlay.changes()).unwrap();
        }
        let len = std::fs::metadata(dir.path().join(".card.img.journal"))
            .unwrap()
            .len();
        assert_eq!(
            len,
            max_journal_bytes(blocks),
            "the longest journal of the card"
        );
        let mut image = open(&path);
        let mut source = image.source().unwrap();
        for lba in [0, 1, 2047] {
            assert_eq!(
                read(&mut source, lba),
                [lba.to_le_bytes()[0] | 1; BLOCK_BYTES]
            );
        }
    }

    #[test]
    fn changes_that_do_not_fit_the_card_are_refused_before_any_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let mut image = open(&path);
        let mut wider = Overlay::new(SparseBlocks::new(MIB / BLOCK * 2));
        wider.write_block(MIB / BLOCK, &[1; BLOCK_BYTES]).unwrap();
        let error = image.commit(&wider.changes()).unwrap_err().to_string();
        assert!(
            error.contains("block 2048 written, on a card of 2048 blocks"),
            "{error}"
        );
        let mut erased = Overlay::new(SparseBlocks::new(MIB / BLOCK * 2));
        erased.erase(2000, 100).unwrap();
        assert!(matches!(
            image.commit(&erased.changes()),
            Err(SdImageError::InvalidChanges { .. })
        ));
        assert!(!dir.path().join(".card.img.journal").exists());
        assert!(!dir.path().join(".card.img.journal.tmp").exists());
    }

    #[cfg(target_os = "linux")]
    fn allocated(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(path).unwrap().blocks() * 512
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn erasing_the_whole_card_frees_its_room_on_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.img");
        std::fs::write(&path, vec![0x55; usize::try_from(8 * MIB).unwrap()]).unwrap();
        assert!(allocated(&path) >= 8 * MIB);
        let mut image = open(&path);
        let mut overlay = Overlay::new(image.source().unwrap());
        overlay.erase(0, 8 * MIB / BLOCK).unwrap();
        let committed = image.commit(&overlay.changes()).unwrap();
        assert_eq!(committed.zeros_written, 0, "a hole, not zeros");
        assert_eq!(committed.notice(), None);
        assert!(
            allocated(&path) < MIB,
            "{} bytes allocated",
            allocated(&path)
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 8 * MIB);
        assert!(std::fs::read(&path).unwrap().iter().all(|&byte| byte == 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn where_no_hole_can_be_punched_zeros_go_over_the_data_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", 8 * MIB, &[0, 8 * MIB / BLOCK - 1]);
        let before = allocated(&path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let written = zero_data(&file, 0, 8 * MIB).unwrap();
        assert!(
            written > 0 && written <= 64 * 1024,
            "{written} bytes of zeros"
        );
        assert!(allocated(&path) <= before, "no room taken");
        assert!(std::fs::read(&path).unwrap().iter().all(|&byte| byte == 0));
        let notice = Committed {
            zeros_written: written,
            ..Committed::default()
        }
        .notice()
        .unwrap();
        assert!(notice.contains("cannot punch holes"), "{notice}");
    }

    #[cfg(unix)]
    #[test]
    fn a_journal_that_is_a_link_is_refused_and_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, b"keep").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(".card.img.journal")).unwrap();
        let error = open_error(&path, Access::ReadWrite);
        assert!(
            error.contains("not a regular file (a link is not followed)"),
            "{error}"
        );
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn a_temporary_journal_that_is_a_link_is_removed_and_what_it_names_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, b"keep").unwrap();
        let link = dir.path().join(".card.img.journal.tmp");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let _image = open(&path);
        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "the link is removed"
        );
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn a_journal_that_is_a_pipe_is_refused_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        nix::unistd::mkfifo(
            &dir.path().join(".card.img.journal"),
            nix::sys::stat::Mode::S_IRWXU,
        )
        .unwrap();
        let error = open_error(&path, Access::ReadWrite);
        assert!(error.contains("not a regular file"), "{error}");
        // A pipe that takes the journal's name after the look at it: what was opened is judged.
        std::fs::remove_file(dir.path().join(".card.img.journal")).unwrap();
        let mut image = open(&path);
        nix::unistd::mkfifo(
            &dir.path().join(".card.img.journal"),
            nix::sys::stat::Mode::S_IRWXU,
        )
        .unwrap();
        let error = image.replay_journal().unwrap_err().to_string();
        assert!(error.contains("not a regular file"), "{error}");
    }

    #[test]
    fn a_read_only_image_with_a_temporary_journal_is_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[3]);
        let temporary = dir.path().join(".card.img.journal.tmp");
        std::fs::write(&temporary, b"half").unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut image = SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).unwrap();
        let _source = image.source().unwrap();
        drop(image);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read(&temporary).unwrap(), b"half");
    }

    #[test]
    fn what_is_no_card_image_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let missing = open_error(&dir.path().join("none.img"), Access::ReadWrite);
        assert!(missing.contains("inkemu sd create"), "{missing}");
        assert!(open_error(dir.path(), Access::ReadWrite).contains("not a regular file"));
        assert!(open_error(Path::new("/"), Access::ReadWrite).contains("names no file"));
        let odd = image(dir.path(), "odd.img", 1000, &[]);
        let error = open_error(&odd, Access::ReadWrite);
        assert!(
            error.contains("not a whole number of 512-byte blocks"),
            "{error}"
        );
        let prime = image(dir.path(), "prime.img", 4097 * 512, &[]);
        assert!(open_error(&prime, Access::ReadWrite).contains("cannot be told by the CSD"));
        #[cfg(unix)]
        {
            let real = image(dir.path(), "real.img", MIB, &[]);
            let link = dir.path().join("link.img");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(open_error(&link, Access::ReadWrite).contains("a link is not followed"));
        }
    }

    #[test]
    fn a_path_the_run_already_uses_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let flash = Claimed::File {
            path: path.clone(),
            what: "the flash file of --flash-file".into(),
        };
        let error = SdImage::open(&path, Access::ReadWrite, &[flash], &mut Vec::new())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("is the flash file of --flash-file"),
            "{error}"
        );
        let session = Claimed::File {
            path: dir.path().join(".card.img.journal"),
            what: "the session of --record".into(),
        };
        let error = SdImage::open(&path, Access::ReadWrite, &[session], &mut Vec::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("has its journal at"), "{error}");
        let files = files_of_the_sd_image(&path);
        assert_eq!(files.len(), 3);
        assert_eq!(files[1].0, dir.path().join(".card.img.journal"));
        assert_eq!(files[2].0, dir.path().join(".card.img.journal.tmp"));
    }

    #[test]
    fn an_image_in_use_by_another_run_is_refused_and_two_readers_share_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let first = open(&path);
        let error = open_error(&path, Access::ReadWrite);
        assert!(error.contains("in use by another inkemu run"), "{error}");
        assert!(SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).is_err());
        drop(first);
        let one = SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).unwrap();
        let two = SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).unwrap();
        assert!(one.write_protected() && two.write_protected());
        assert!(SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new()).is_err());
    }

    #[test]
    fn a_read_only_image_commits_nothing_and_refuses_a_pending_journal_unread() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let mut image = SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).unwrap();
        let mut overlay = Overlay::new(image.source().unwrap());
        assert_eq!(
            image.commit(&overlay.changes()).unwrap(),
            Committed::default()
        );
        overlay.write_block(0, &[1; BLOCK_BYTES]).unwrap();
        assert!(matches!(
            image.commit(&overlay.changes()),
            Err(SdImageError::ReadOnly { .. })
        ));
        drop(image);
        // A journal no one can read is still refused by name: read-only, it is not read.
        let journal = dir.path().join(".card.img.journal");
        std::fs::write(&journal, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        let error = open_error(&path, Access::ReadOnly);
        assert!(error.contains("open it once for writing"), "{error}");
    }

    #[test]
    fn an_image_whose_size_changed_during_the_run_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let mut image = open(&path);
        let mut overlay = Overlay::new(image.source().unwrap());
        overlay.write_block(0, &[1; BLOCK_BYTES]).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(2 * MIB)
            .unwrap();
        let error = image.commit(&overlay.changes()).unwrap_err().to_string();
        assert!(
            error.contains("was 1048576 bytes when the run opened it and is 2097152 now"),
            "{error}"
        );
        assert!(!dir.path().join(".card.img.journal").exists());
    }

    #[test]
    fn a_block_past_the_image_is_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let mut image = open(&path);
        let mut source = image.source().unwrap();
        assert_eq!(source.block_count(), 2048);
        assert_eq!(
            source.read_block(2048, &mut [0; BLOCK_BYTES]).unwrap_err(),
            StorageError::OutOfRange {
                lba: 2048,
                blocks: 2048
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_journal_is_for_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[]);
        let image = open(&path);
        let mut overlay = Overlay::new(SparseBlocks::new(MIB / BLOCK));
        overlay.write_block(0, &[1; BLOCK_BYTES]).unwrap();
        image.write_journal(&overlay.changes()).unwrap();
        let mode = std::fs::metadata(dir.path().join(".card.img.journal"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}
