//! The image file of a device's SD card (design `SP2e`, 2e-D13, ADR 0014): the card's blocks, as
//! bytes and nothing else, from block 0, exactly the card's capacity.
//!
//! A run opens the image once and locks it (exclusively to write it, shared to only read it).
//! A life reads its blocks as the card asks for them ([`FileBlocks`]) and writes into an
//! [`Overlay`](chips::sdcard::Overlay) in memory; when the life ends, [`SdImage::commit`] writes
//! what changed **in place**, through a journal: the changes go to `.<name>.journal.tmp`, which is
//! flushed and renamed `.<name>.journal`, then applied to the image, which is flushed, and the
//! journal is removed. Whatever interrupts this, the image is the one before the life or the one
//! after it: a journal found when the image is opened again is checked and applied first. An
//! image of several gibibytes is never copied, and a life costs what it wrote.
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
use std::io::{Read as _, Seek as _, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The first line of a journal, its version.
const JOURNAL_MAGIC: &[u8] = b"inkemu-sd-journal 1\n";
const SHA256_BYTES: usize = 32;
const BLOCK: u64 = BLOCK_BYTES as u64;
/// The zeros an erased range is written with, a chunk at a time.
const ZERO_CHUNK: u64 = 1024 * 1024;

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
        "the SD image {path} has a journal, {journal}, left by a save that was interrupted, and is opened read-only; open it once for writing so that the save ends"
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
fn journal_path(path: &Path) -> Option<PathBuf> {
    sibling(path, ".", ".journal")
}

/// The name the journal is written to before it takes its own.
fn journal_temporary_path(path: &Path) -> Option<PathBuf> {
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

/// What a commit wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Committed {
    /// Blocks written.
    pub blocks: usize,
    /// Ranges erased.
    pub erased_ranges: usize,
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
    /// The image, open and locked. Closing it releases the lock.
    file: File,
}

impl SdImage {
    /// Opens and locks the image at `path`. A journal left by an interrupted save is applied
    /// (and `log` says so); a temporary journal left is removed (and said). Nothing else of the
    /// image is read.
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
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(SdImageError::NotAFile { path: shown_path }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SdImageError::Missing { path: shown_path });
            }
            Err(e) => {
                return Err(SdImageError::Open {
                    path: shown_path,
                    detail: sanitize(&e.to_string()),
                    remedy: "check the path and its permissions",
                });
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(access == Access::ReadWrite);
        let file = open_in_place(&mut options, path).map_err(|e| SdImageError::Open {
            path: shown_path.clone(),
            detail: sanitize(&e.to_string()),
            remedy: match (e.kind(), access) {
                (std::io::ErrorKind::PermissionDenied, Access::ReadWrite) => {
                    "make it writable for you, or open it read-only: the card is then write-protected"
                }
                _ => "check the path and its permissions",
            },
        })?;
        let metadata = file.metadata().map_err(|e| SdImageError::Open {
            path: shown_path.clone(),
            detail: sanitize(&e.to_string()),
            remedy: "check the path and its permissions",
        })?;
        if !metadata.is_file() {
            return Err(SdImageError::NotAFile { path: shown_path });
        }
        let capacity = Capacity::new(metadata.len()).map_err(|reason| SdImageError::Size {
            path: shown_path.clone(),
            reason: reason.to_string(),
        })?;
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
            file,
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

    /// The image as the source a life's card reads, one per life.
    ///
    /// # Errors
    /// [`SdImageError::Open`] when the system cannot give a second handle on the open file.
    pub fn source(&self) -> Result<FileBlocks, SdImageError> {
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

    /// Writes `changes`, what a life changed, into the image, through the journal. Nothing is
    /// written when nothing changed.
    ///
    /// # Errors
    /// [`SdImageError::Changed`] when the image's size changed since it was opened,
    /// [`SdImageError::ReadOnly`] for changes to an image opened read-only,
    /// [`SdImageError::Write`] when the journal or the image cannot be written. When the journal
    /// was written but its application failed, the next opening applies it.
    pub fn commit(&mut self, changes: &Changes<'_>) -> Result<Committed, SdImageError> {
        if changes.is_empty() {
            return Ok(Committed::default());
        }
        if self.access == Access::ReadOnly {
            return Err(SdImageError::ReadOnly {
                path: self.shown.clone(),
            });
        }
        self.check_size()?;
        let journal = encode_journal(self.capacity.blocks(), changes);
        self.write_journal(&journal)?;
        let committed = self.apply(&journal)?;
        self.remove_journal()?;
        Ok(committed)
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

    /// Writes the journal through its temporary name, flushed, then renamed.
    fn write_journal(&self, journal: &[u8]) -> Result<(), SdImageError> {
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
        let written = options.open(&self.temporary).and_then(|mut file| {
            file.write_all(journal)?;
            file.sync_all()
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

    /// Applies a checked journal to the image, then flushes the image.
    fn apply(&self, journal: &[u8]) -> Result<Committed, SdImageError> {
        let decoded = decode_journal(journal, self.capacity.blocks()).map_err(|why| {
            SdImageError::BadJournal {
                path: self.shown.clone(),
                journal: shown(&self.journal),
                why,
            }
        })?;
        let zeros = vec![0u8; usize::try_from(ZERO_CHUNK).unwrap_or(BLOCK_BYTES)];
        let mut file = &self.file;
        let write = |file: &mut &File, offset: u64, bytes: &[u8]| {
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(bytes)
        };
        for &(first, count) in &decoded.erased {
            let mut offset = first * BLOCK;
            let mut left = count * BLOCK;
            while left > 0 {
                let chunk = left.min(ZERO_CHUNK);
                let bytes = usize::try_from(chunk)
                    .ok()
                    .and_then(|chunk| zeros.get(..chunk))
                    .unwrap_or(&zeros);
                write(&mut file, offset, bytes)
                    .map_err(|e| self.failed("write", &self.path, &e))?;
                offset += chunk;
                left -= chunk;
            }
        }
        for (lba, block) in &decoded.blocks {
            write(&mut file, lba * BLOCK, block)
                .map_err(|e| self.failed("write", &self.path, &e))?;
        }
        self.file
            .sync_all()
            .map_err(|e| self.failed("write", &self.path, &e))?;
        Ok(Committed {
            blocks: decoded.blocks.len(),
            erased_ranges: decoded.erased.len(),
        })
    }

    fn remove_journal(&self) -> Result<(), SdImageError> {
        std::fs::remove_file(&self.journal)
            .map_err(|e| self.failed("remove", &self.journal, &e))?;
        sync_directory(&self.journal);
        Ok(())
    }

    /// What an interrupted save left: a temporary journal is removed, a journal is applied.
    fn recover(&mut self, log: &mut dyn Write) -> Result<(), SdImageError> {
        if std::fs::symlink_metadata(&self.temporary).is_ok() && self.access == Access::ReadWrite {
            std::fs::remove_file(&self.temporary)
                .map_err(|e| self.failed("remove", &self.temporary, &e))?;
            let _ = writeln!(
                log,
                "inkemu: removed {}, the journal of a save of the SD image {} that was interrupted before it was complete; the image is as the save before it left it",
                shown(&self.temporary),
                self.shown
            );
        }
        let journal = match std::fs::symlink_metadata(&self.journal) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(metadata) if !metadata.is_file() => {
                return Err(SdImageError::BadJournal {
                    path: self.shown.clone(),
                    journal: shown(&self.journal),
                    why: "it is not a regular file".to_owned(),
                });
            }
            _ => self.read_journal()?,
        };
        if self.access == Access::ReadOnly {
            return Err(SdImageError::PendingJournal {
                path: self.shown.clone(),
                journal: shown(&self.journal),
            });
        }
        let committed = self.apply(&journal)?;
        self.remove_journal()?;
        let _ = writeln!(
            log,
            "inkemu: finished the save of the SD image {} that was interrupted ({} blocks, {} erased ranges)",
            self.shown, committed.blocks, committed.erased_ranges
        );
        Ok(())
    }

    fn read_journal(&self) -> Result<Vec<u8>, SdImageError> {
        let bad = |why: String| SdImageError::BadJournal {
            path: self.shown.clone(),
            journal: shown(&self.journal),
            why,
        };
        let mut file = open_in_place(OpenOptions::new().read(true), &self.journal)
            .map_err(|e| bad(format!("it cannot be read: {}", sanitize(&e.to_string()))))?;
        let mut bytes = Vec::new();
        // The longest journal writes every block of the card once, with its number.
        let limit = self
            .capacity
            .blocks()
            .saturating_mul(BLOCK + 8)
            .saturating_add(4096);
        (&mut file)
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| bad(format!("it cannot be read: {}", sanitize(&e.to_string()))))?;
        Ok(bytes)
    }
}

/// Flushes the directory of `path`, so that a rename or a removal in it outlives a crash. Not
/// every system lets a directory be opened: best effort.
fn sync_directory(path: &Path) {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if let Ok(directory) = File::open(directory) {
        let _ = directory.sync_all();
    }
}

/// The bytes of a journal: the version line, the card's blocks (`u64`), the erased ranges
/// (`u64` count, then `u64` first and `u64` count each), the written blocks (`u64` count, then
/// `u64` number and 512 bytes each), then the SHA-256 of all that, little-endian.
fn encode_journal(blocks: u64, changes: &Changes<'_>) -> Vec<u8> {
    let erased: Vec<(u64, u64)> = changes.erased().collect();
    let mut bytes = Vec::with_capacity(
        JOURNAL_MAGIC.len() + 24 + erased.len() * 16 + changes.written_count() * (8 + BLOCK_BYTES),
    );
    bytes.extend_from_slice(JOURNAL_MAGIC);
    bytes.extend_from_slice(&blocks.to_le_bytes());
    let count = |len: usize| u64::try_from(len).unwrap_or(u64::MAX).to_le_bytes();
    bytes.extend_from_slice(&count(erased.len()));
    for (first, count) in erased {
        bytes.extend_from_slice(&first.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
    }
    bytes.extend_from_slice(&count(changes.written_count()));
    for (lba, block) in changes.blocks() {
        bytes.extend_from_slice(&lba.to_le_bytes());
        bytes.extend_from_slice(block);
    }
    let digest = Sha256::digest(&bytes);
    bytes.extend_from_slice(&digest);
    bytes
}

/// A journal, checked.
struct Journal {
    erased: Vec<(u64, u64)>,
    blocks: Vec<(u64, Block)>,
}

/// Checks and reads a journal for a card of `capacity` blocks; the reason it is refused.
fn decode_journal(bytes: &[u8], capacity: u64) -> Result<Journal, String> {
    let (body, digest) = bytes
        .split_at_checked(bytes.len().saturating_sub(SHA256_BYTES))
        .filter(|(body, _)| body.starts_with(JOURNAL_MAGIC))
        .ok_or_else(|| "it does not begin with the line of a journal".to_owned())?;
    if Sha256::digest(body).as_slice() != digest {
        return Err("its SHA-256 does not match: it is incomplete or damaged".to_owned());
    }
    // Every count is checked against the bytes left as it is read: a list grows by what the
    // file holds, never by a number it claims.
    let mut cursor = Cursor {
        rest: body.get(JOURNAL_MAGIC.len()..).unwrap_or_default(),
    };
    let blocks = cursor.u64("the size of the card")?;
    if blocks != capacity {
        return Err(format!(
            "it is for a card of {blocks} blocks, and the image holds {capacity}"
        ));
    }
    let ranges = cursor.u64("the number of erased ranges")?;
    let mut erased = Vec::new();
    for _ in 0..ranges {
        let first = cursor.u64("an erased range")?;
        let count = cursor.u64("an erased range")?;
        if count == 0 || first.checked_add(count).is_none_or(|end| end > capacity) {
            return Err(format!(
                "its erased range of {count} blocks from {first} is not on the card"
            ));
        }
        erased.push((first, count));
    }
    let written = cursor.u64("the number of written blocks")?;
    let mut blocks_written = Vec::new();
    for _ in 0..written {
        let lba = cursor.u64("a written block")?;
        if lba >= capacity {
            return Err(format!("its block {lba} is not on the card"));
        }
        blocks_written.push((lba, cursor.block()?));
    }
    if !cursor.rest.is_empty() {
        return Err("it has bytes after its last block".to_owned());
    }
    Ok(Journal {
        erased,
        blocks: blocks_written,
    })
}

/// What is left of a journal to read.
struct Cursor<'a> {
    rest: &'a [u8],
}

impl Cursor<'_> {
    fn u64(&mut self, what: &str) -> Result<u64, String> {
        let (head, tail) = self
            .rest
            .split_first_chunk::<8>()
            .ok_or_else(|| format!("it ends before {what}"))?;
        self.rest = tail;
        Ok(u64::from_le_bytes(*head))
    }

    fn block(&mut self) -> Result<Block, String> {
        let (block, tail) = self
            .rest
            .split_first_chunk::<BLOCK_BYTES>()
            .ok_or_else(|| "it ends in a written block".to_owned())?;
        self.rest = tail;
        Ok(*block)
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
    use chips::sdcard::{BlockStorage as _, Overlay};

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
        assert_eq!(
            committed,
            Committed {
                blocks: 2,
                erased_ranges: 1
            }
        );
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
        let image = open(&path);
        assert_eq!(read(&mut image.source().unwrap(), 7), [7; BLOCK_BYTES]);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), MIB, "never grown");
    }

    #[test]
    fn a_journal_left_after_its_rename_is_applied_at_the_next_opening_and_said() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[4]);
        {
            let image = open(&path);
            let mut overlay = Overlay::new(image.source().unwrap());
            overlay.write_block(4, &[0x44; BLOCK_BYTES]).unwrap();
            overlay.erase(10, 5).unwrap();
            let journal = encode_journal(image.capacity().blocks(), &overlay.changes());
            image.write_journal(&journal).unwrap();
            // The process dies here, before the journal is applied, or halfway through it.
            let mut file = &image.file;
            file.seek(SeekFrom::Start(4 * BLOCK)).unwrap();
            file.write_all(&[0x44; 100]).unwrap();
        }
        assert!(dir.path().join(".card.img.journal").exists());
        let mut log = Vec::new();
        let image = SdImage::open(&path, Access::ReadWrite, &[], &mut log).unwrap();
        let log = String::from_utf8(log).unwrap();
        assert!(log.contains("finished the save of the SD image"), "{log}");
        assert!(log.contains("(1 blocks, 1 erased ranges)"), "{log}");
        assert_eq!(read(&mut image.source().unwrap(), 4), [0x44; BLOCK_BYTES]);
        assert!(!dir.path().join(".card.img.journal").exists());
    }

    #[test]
    fn a_temporary_journal_left_before_its_rename_is_removed_and_the_image_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = image(dir.path(), "card.img", MIB, &[4]);
        std::fs::write(dir.path().join(".card.img.journal.tmp"), b"half a journal").unwrap();
        let mut log = Vec::new();
        let image = SdImage::open(&path, Access::ReadWrite, &[], &mut log).unwrap();
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
        let mut overlay = Overlay::new(chips::sdcard::SparseBlocks::new(MIB / BLOCK));
        overlay.write_block(1, &[1; BLOCK_BYTES]).unwrap();
        let good = encode_journal(MIB / BLOCK, &overlay.changes());
        let cases: [(Vec<u8>, &str); 4] = [
            (good[..good.len() - 1].to_vec(), "SHA-256 does not match"),
            (
                b"something else".to_vec(),
                "does not begin with the line of a journal",
            ),
            (
                encode_journal(MIB / BLOCK * 2, &overlay.changes()),
                "is for a card of 4096 blocks, and the image holds 2048",
            ),
            (
                {
                    let mut far = Overlay::new(chips::sdcard::SparseBlocks::new(1 << 20));
                    far.write_block(5000, &[1; BLOCK_BYTES]).unwrap();
                    encode_journal(MIB / BLOCK, &far.changes())
                },
                "its block 5000 is not on the card",
            ),
        ];
        for (bytes, why) in cases {
            std::fs::write(&journal, &bytes).unwrap();
            let error = SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert!(error.contains(why), "{error}");
            assert!(error.contains("nothing was applied"), "{error}");
            assert!(journal.exists(), "left for the user");
        }
        assert_eq!(std::fs::read(&path).unwrap(), vec![0; 1024 * 1024]);
    }

    #[test]
    fn what_is_no_card_image_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let open_error = |path: &Path| {
            SdImage::open(path, Access::ReadWrite, &[], &mut Vec::new())
                .unwrap_err()
                .to_string()
        };
        let missing = open_error(&dir.path().join("none.img"));
        assert!(missing.contains("inkemu sd create"), "{missing}");
        assert!(open_error(dir.path()).contains("not a regular file"));
        assert!(open_error(Path::new("/")).contains("names no file"));
        let odd = image(dir.path(), "odd.img", 1000, &[]);
        let error = open_error(&odd);
        assert!(
            error.contains("not a whole number of 512-byte blocks"),
            "{error}"
        );
        let prime = image(dir.path(), "prime.img", 4097 * 512, &[]);
        assert!(open_error(&prime).contains("cannot be told by the CSD"));
        #[cfg(unix)]
        {
            let real = image(dir.path(), "real.img", MIB, &[]);
            let link = dir.path().join("link.img");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(open_error(&link).contains("a link is not followed"));
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
        let error = SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("in use by another inkemu run"), "{error}");
        assert!(SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).is_err());
        drop(first);
        let one = SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).unwrap();
        let two = SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).unwrap();
        assert!(one.write_protected() && two.write_protected());
        assert!(SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new()).is_err());
    }

    #[test]
    fn a_read_only_image_commits_nothing_and_refuses_a_pending_journal() {
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
        std::fs::write(dir.path().join(".card.img.journal"), b"x").unwrap();
        let error = SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new())
            .unwrap_err()
            .to_string();
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
        let image = open(&path);
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
        image.write_journal(b"journal").unwrap();
        let mode = std::fs::metadata(dir.path().join(".card.img.journal"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}
