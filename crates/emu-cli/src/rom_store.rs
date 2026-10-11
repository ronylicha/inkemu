//! The ROM store: a pinned manifest of Espressif's ROM ELFs, a verified download of their archive and an
//! on-disk cache holding one ELF per ROM revision.
//!
//! inkemu never ships the mask ROM (ADR 0001, "Consequences"). The archive and every ELF are untrusted
//! network data until their SHA-256 matches the manifest, so:
//! - the archive hash is checked before anything is decompressed;
//! - no path taken from the archive is ever written to disk: only the entry whose name is exactly the
//!   expected one is read, into memory;
//! - the cache is written to a temporary file, created with `create_new` so a planted symlink is never
//!   followed, and renamed into place;
//! - a file already in the directory is read through a bound (a few MiB, regular files only) before it
//!   is hashed; one whose hash differs is reported, never loaded, replaced or deleted;
//! - an explicit directory ([`RomSource::Directory`]) is never downloaded into;
//! - however many ROMs are missing, the archive is downloaded once ([`ensure_elfs`]).
//!
//! Sources: `docs/research/esp32c3-rom-elfs.md`, §2 (archive content, hashes) and §9 (image hashes).
use crate::term::{hex, sanitize};
use etcetera::app_strategy::AppStrategy as _;
use sha2::{Digest, Sha256};
use soc_esp32c3::{ChipRevision, RomError, RomImage};
use std::cell::RefCell;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A release archive of ROM ELFs, pinned by its SHA-256 and exact size.
#[derive(Clone, Copy, Debug)]
pub struct RomArchive {
    pub release: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// One ROM revision: the ELF to extract, the hash of that ELF and the hash of the image built from it.
#[derive(Clone, Copy, Debug)]
pub struct RomEntry {
    /// ROM revision as `efuse_hal_chip_revision()` numbers it (`major * 100 + minor`).
    pub rom_revision: u16,
    pub elf_name: &'static str,
    pub elf_sha256: &'static str,
    pub image_sha256: &'static str,
}

/// The pinned `esp-rom-elfs` release (research report §2: 17 flat ELF files, 4 902 355 bytes).
pub const ESP_ROM_ELFS: RomArchive = RomArchive {
    release: "20260528",
    url: "https://github.com/espressif/esp-rom-elfs/releases/download/20260528/esp-rom-elfs-20260528.tar.gz",
    sha256: "caa463d3cbef2430a5a35847c1d9f2f152403b17a802050927ff60c8da54fe46",
    size: 4_902_355,
};

/// The three ESP32-C3 ROMs of the pinned release, by increasing revision (report §2 for the ELF hashes,
/// §9 for the image hashes).
pub const ESP32C3_ROMS: [RomEntry; 3] = [
    RomEntry {
        rom_revision: 0,
        elf_name: "esp32c3_rev0_rom.elf",
        elf_sha256: "11d565922e0a405b477756de748a14046f86dfb40d997766252a04cb378f0694",
        image_sha256: "832b9a17b4bca5fe307d2d84b3fcd9cdc75f6a8a51956a3e114b4d6713710565",
    },
    RomEntry {
        rom_revision: 3,
        elf_name: "esp32c3_rev3_rom.elf",
        elf_sha256: "19ac22e08707df926fb0cf4c54795d4067b983fae8635f396ded173a6d78fc3c",
        image_sha256: "0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99",
    },
    RomEntry {
        rom_revision: 101,
        elf_name: "esp32c3_rev101_rom.elf",
        elf_sha256: "9495e1453f36f7eec4112714ea1dca22d77808776adffa93dda7dda4393e8336",
        image_sha256: "8b41b9b114e110e373a58188e6e86b044719f3b4e6da0441c6e8b68b72986651",
    },
];

/// Plausibility cap on one ROM ELF: the genuine ones are far smaller, a larger entry is hostile or corrupt.
const MAX_ELF_BYTES: u64 = 4 * 1024 * 1024;
/// A whole download is abandoned after this long, so a stalled connection cannot hang the tool.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Size granularity of the download notice: 0.1 MB (decimal megabytes).
const TENTH_OF_A_MEGABYTE: u64 = 100_000;
/// Sub-directory of the user data directory that holds the cache.
const CACHE_SUBDIR: &str = "rom";
/// Application name under the user data directory.
const APP_NAME: &str = "inkemu";

#[derive(Debug, thiserror::Error)]
pub enum RomStoreError {
    #[error(
        "cannot download {url}: {detail}; check the network connection, or fetch that archive on another machine, extract `{elf_name}` and pass its directory with --rom-dir"
    )]
    Download {
        url: String,
        detail: String,
        elf_name: &'static str,
    },
    #[error(
        "{url} does not match the pinned archive (expected sha256 {expected}, got {found}); retry the download, or supply the ROM yourself with --rom-dir"
    )]
    ArchiveMismatch {
        url: String,
        expected: &'static str,
        found: String,
    },
    #[error(
        "the archive {url} holds no `{elf_name}`; the pinned release changed layout, report it and supply the ROM with --rom-dir meanwhile"
    )]
    MissingInArchive { url: String, elf_name: &'static str },
    #[error("{path} is not the expected ROM (expected sha256 {expected}, got {found}); {remedy}")]
    ElfMismatch {
        path: String,
        expected: &'static str,
        found: String,
        /// What to do about it: depends on whether the file sits in the cache, in a user directory
        /// or came out of the downloaded archive.
        remedy: String,
    },
    #[error(
        "{path} cannot be a ROM ELF ({reason}); replace it with the genuine file, or delete it and run `{fetch_command}`"
    )]
    UnsuitableFile {
        path: String,
        reason: String,
        /// The `inkemu rom fetch` invocation that restores the file where it lives.
        fetch_command: String,
    },
    #[error(
        "cannot read the verified archive {url}: {detail}; the pinned release does not have the expected layout: report it, and supply the ROM with --rom-dir meanwhile"
    )]
    Archive { url: String, detail: String },
    #[error(
        "{path} not found; --rom-dir must point at a directory holding the ROM ELF files of esp-rom-elfs release {release}"
    )]
    NotInDirectory { path: String, release: &'static str },
    #[error(
        "the ROM image built from {path} has sha256 {found}, expected {expected}; this is a bug in inkemu's ROM conversion, please report it"
    )]
    ImageMismatch {
        path: String,
        expected: &'static str,
        found: String,
    },
    #[error(
        "cannot {action} {path}: {detail}; check the permissions of the ROM directory or choose another one with --rom-dir"
    )]
    Io {
        action: &'static str,
        path: String,
        detail: String,
    },
    #[error(
        "this platform has no user data directory; pass the ROM directory explicitly with --rom-dir"
    )]
    NoDataDir,
    #[error("{path}: {source}")]
    Rom { path: String, source: RomError },
}

/// Downloads a URL into memory. The only code that touches the network sits behind this trait, so
/// every other part of the store is tested without it.
pub trait Fetch {
    /// # Errors
    /// A human-readable cause when the request fails. A body longer than `max_bytes` may be truncated
    /// to `max_bytes + 1` bytes; the caller detects it from the length.
    fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, String>;
}

/// [`Fetch`] over HTTPS (`ureq` with rustls and the Mozilla root store). Redirects are followed, which
/// GitHub release downloads need; plain HTTP is refused.
#[derive(Clone, Copy, Debug)]
pub struct HttpFetch;

impl Fetch for HttpFetch {
    fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_global(Some(DOWNLOAD_TIMEOUT))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = agent.get(url).call().map_err(|e| e.to_string())?;
        // One byte past the expected size, so a longer body is caught by the length check.
        let mut reader = response.into_body().into_reader().take(max_bytes + 1);
        let mut body = Vec::new();
        reader.read_to_end(&mut body).map_err(|e| e.to_string())?;
        Ok(body)
    }
}

/// A [`Fetch`] that says on `log` that the archive is being downloaded, at the moment the download
/// starts and not before: a refusal reached earlier (a tampered file, a directory that is never
/// downloaded into) never shows the notice, since nothing is downloaded then.
pub struct AnnouncingFetch<'a> {
    inner: &'a dyn Fetch,
    log: RefCell<&'a mut dyn std::io::Write>,
    notice: String,
}

impl<'a> AnnouncingFetch<'a> {
    /// `dir` is where the downloaded ROMs will be stored; it is only named in the notice.
    pub fn new(
        inner: &'a dyn Fetch,
        log: &'a mut dyn std::io::Write,
        archive: &RomArchive,
        dir: &Path,
    ) -> AnnouncingFetch<'a> {
        // Tenths of a megabyte, in integers: the size is only shown to one decimal.
        let tenths = archive.size / TENTH_OF_A_MEGABYTE;
        let notice = format!(
            "inkemu: downloading the ESP32-C3 mask ROM (esp-rom-elfs {}, {}.{} MB) into {}\n",
            archive.release,
            tenths / 10,
            tenths % 10,
            shown(dir)
        );
        AnnouncingFetch {
            inner,
            log: RefCell::new(log),
            notice,
        }
    }
}

impl std::fmt::Debug for AnnouncingFetch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnouncingFetch")
            .field("notice", &self.notice)
            .finish_non_exhaustive()
    }
}

impl Fetch for AnnouncingFetch<'_> {
    fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        // Best effort: a closed log must not turn a working download into a failure.
        let _ = self.log.borrow_mut().write_all(self.notice.as_bytes());
        self.inner.fetch(url, max_bytes)
    }
}

/// Where ROM files come from.
pub enum RomSource<'a> {
    /// The cache directory `dir`; a missing ELF is downloaded into it with `fetch`. `user_named` says
    /// the user chose `dir` (`rom fetch --rom-dir`) rather than the per-user default: a remedy that
    /// downloads must then name it again.
    Cache {
        dir: PathBuf,
        fetch: &'a dyn Fetch,
        user_named: bool,
    },
    /// A directory the user supplied (`--rom-dir`): read only, never downloaded into.
    Directory(PathBuf),
}

impl RomSource<'_> {
    /// The directory the ROM files are read from (and, for the cache, stored into).
    #[must_use]
    pub fn dir(&self) -> &Path {
        match self {
            RomSource::Cache { dir, .. } | RomSource::Directory(dir) => dir,
        }
    }
}

impl std::fmt::Debug for RomSource<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RomSource::Cache { dir, .. } => f
                .debug_struct("Cache")
                .field("dir", dir)
                .finish_non_exhaustive(),
            RomSource::Directory(dir) => f.debug_tuple("Directory").field(dir).finish(),
        }
    }
}

/// The ROM of the highest revision not above the chip's, as the `esp-rom-elfs` README prescribes
/// ("if the archive doesn't contain the ROM ELF file for your chip revision, use the next lower one").
#[must_use]
pub fn rom_entry_for(revision: ChipRevision) -> &'static RomEntry {
    let chip = revision.full();
    ESP32C3_ROMS
        .iter()
        .rev()
        .find(|entry| entry.rom_revision <= chip)
        // The oldest ROM suits every chip, so the search cannot come back empty.
        .unwrap_or(&ESP32C3_ROMS[0])
}

/// The per-user cache directory: the application data directory of the platform, plus `rom`.
///
/// etcetera's native strategy, with no qualifier or author (neither is needed to name one tool):
/// - Linux and other Unix: `$XDG_DATA_HOME/inkemu/rom`, or `~/.local/share/inkemu/rom`;
/// - macOS: `~/Library/Application Support/inkemu/rom`;
/// - Windows: `%APPDATA%\inkemu\data\rom`.
///
/// Nothing is created: the directory comes into being when the first ROM is stored.
///
/// # Errors
/// [`RomStoreError::NoDataDir`] when the home directory cannot be resolved.
pub fn default_cache_dir() -> Result<PathBuf, RomStoreError> {
    let strategy =
        etcetera::app_strategy::choose_native_strategy(etcetera::app_strategy::AppStrategyArgs {
            top_level_domain: String::new(),
            author: String::new(),
            app_name: APP_NAME.to_owned(),
        })
        .map_err(|_| RomStoreError::NoDataDir)?;
    Ok(strategy.data_dir().join(CACHE_SUBDIR))
}

/// The verified bytes of the ROM ELF described by `entry`: the one-entry case of [`ensure_elfs`].
///
/// # Errors
/// Any [`RomStoreError`] except `ImageMismatch` and `Rom`: a missing or mismatching file, a failed
/// download, an archive that does not match its pin, an I/O failure.
pub fn load_elf(
    source: &RomSource<'_>,
    archive: &RomArchive,
    entry: &RomEntry,
) -> Result<Vec<u8>, RomStoreError> {
    let mut elfs = ensure_elfs(source, archive, &[entry])?;
    let Some(elf) = elfs.pop() else {
        // INVARIANT: `ensure_elfs` returns exactly one ELF per entry it is given.
        unreachable!("ensure_elfs returned no ELF for one entry")
    };
    Ok(elf)
}

/// The verified bytes of every ROM ELF in `entries` (in the same order), downloading the pinned
/// `archive` **once** when the cache lacks some of them.
///
/// 1. Every file already present is read and hash-checked first: a mismatch is returned at once,
///    before any network access, and nothing is written.
/// 2. When nothing is missing, that is all.
/// 3. For a [`RomSource::Directory`] the first missing file is an error: that source never downloads.
/// 4. For a [`RomSource::Cache`] the archive is fetched once, verified, and each missing ELF is
///    extracted, hash-checked and stored.
///
/// # Errors
/// The errors of [`load_elf`].
pub fn ensure_elfs(
    source: &RomSource<'_>,
    archive: &RomArchive,
    entries: &[&RomEntry],
) -> Result<Vec<Vec<u8>>, RomStoreError> {
    let mut elfs = Vec::with_capacity(entries.len());
    for entry in entries {
        elfs.push(read_present(source, archive, entry)?);
    }
    let Some(first_missing) = elfs.iter().position(Option::is_none) else {
        return Ok(elfs.into_iter().flatten().collect());
    };
    let fetch = match source {
        RomSource::Cache { fetch, .. } => *fetch,
        RomSource::Directory(dir) => {
            return Err(RomStoreError::NotInDirectory {
                path: shown(&dir.join(entries[first_missing].elf_name)),
                release: archive.release,
            });
        }
    };
    let body = download(fetch, archive, entries[first_missing])?;
    for (slot, entry) in elfs.iter_mut().zip(entries) {
        if slot.is_none() {
            *slot = Some(extract_and_store(&body, archive, entry, source.dir())?);
        }
    }
    Ok(elfs.into_iter().flatten().collect())
}

/// The ELF of `entry` when `source` already holds it (hash-checked); `None` when the file is absent.
fn read_present(
    source: &RomSource<'_>,
    archive: &RomArchive,
    entry: &RomEntry,
) -> Result<Option<Vec<u8>>, RomStoreError> {
    let path = source.dir().join(entry.elf_name);
    let provenance = match source {
        RomSource::Cache {
            dir, user_named, ..
        } => Provenance::Cache {
            named: user_named.then(|| dir.clone()),
        },
        RomSource::Directory(dir) => Provenance::Directory {
            dir: dir.clone(),
            release: archive.release,
        },
    };
    let Some(bytes) = read_bounded(&path, &provenance)? else {
        return Ok(None);
    };
    check_elf(&bytes, entry, &path, &provenance)?;
    Ok(Some(bytes))
}

/// The content of the regular file at `path`, reading at most [`MAX_ELF_BYTES`] bytes; `None` when it
/// does not exist. Anything that is not a regular file, or is longer than the bound, is refused without
/// being hashed: the path may be a directory, a pipe that would block, or a huge file.
fn read_bounded(path: &Path, provenance: &Provenance) -> Result<Option<Vec<u8>>, RomStoreError> {
    let unsuitable = |reason: &str| RomStoreError::UnsuitableFile {
        path: shown(path),
        reason: reason.to_owned(),
        fetch_command: provenance.fetch_command(),
    };
    // Checked before opening: opening a pipe would wait for a writer.
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => return Err(unsuitable("it is not a regular file")),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error("read", path, &e)),
    }
    let file = std::fs::File::open(path).map_err(|e| io_error("read", path, &e))?;
    let metadata = file.metadata().map_err(|e| io_error("read", path, &e))?;
    if !metadata.is_file() {
        return Err(unsuitable("it is not a regular file"));
    }
    let mut bytes = Vec::new();
    // One byte past the bound, so a longer file is told from one of exactly the bound.
    file.take(MAX_ELF_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error("read", path, &e))?;
    if bytes.len() as u64 > MAX_ELF_BYTES {
        return Err(unsuitable(&format!(
            "it is longer than the {MAX_ELF_BYTES} byte (4 MiB) limit of a ROM ELF"
        )));
    }
    Ok(Some(bytes))
}

/// The ROM image built from the verified ELF, checked against the pinned image hash.
///
/// # Errors
/// Every error of [`load_elf`], plus `Rom` when the file is not a convertible ROM ELF and
/// `ImageMismatch` when the converted image differs from the pinned one.
pub fn load_rom(
    source: &RomSource<'_>,
    archive: &RomArchive,
    entry: &RomEntry,
) -> Result<RomImage, RomStoreError> {
    let elf = load_elf(source, archive, entry)?;
    let path = shown(&source.dir().join(entry.elf_name));
    let image = RomImage::from_elf(&elf).map_err(|source| RomStoreError::Rom {
        path: path.clone(),
        source,
    })?;
    let found = sha256_hex(image.bytes());
    if found != entry.image_sha256 {
        return Err(RomStoreError::ImageMismatch {
            path,
            expected: entry.image_sha256,
            found,
        });
    }
    Ok(image)
}

/// Fetches the pinned archive and verifies its size and hash. Which ELF the caller is after only
/// shows in the error message of a failed download.
fn download(
    fetch: &dyn Fetch,
    archive: &RomArchive,
    wanted: &RomEntry,
) -> Result<Vec<u8>, RomStoreError> {
    let body =
        fetch
            .fetch(archive.url, archive.size)
            .map_err(|detail| RomStoreError::Download {
                url: archive.url.to_owned(),
                detail: sanitize(&detail),
                elf_name: wanted.elf_name,
            })?;
    // The hash is checked before the archive is decompressed: hostile data never reaches the decoder.
    let found = sha256_hex(&body);
    if body.len() as u64 != archive.size || found != archive.sha256 {
        return Err(RomStoreError::ArchiveMismatch {
            url: archive.url.to_owned(),
            expected: archive.sha256,
            found,
        });
    }
    Ok(body)
}

/// Extracts `entry` from the verified archive `body`, checks it against the manifest and stores it.
fn extract_and_store(
    body: &[u8],
    archive: &RomArchive,
    entry: &RomEntry,
    dir: &Path,
) -> Result<Vec<u8>, RomStoreError> {
    let elf = extract_entry(body, archive, entry)?;
    check_elf(&elf, entry, &dir.join(entry.elf_name), &Provenance::Archive)?;
    store(dir, entry.elf_name, &elf)?;
    Ok(elf)
}

/// Reads the entry named exactly `entry.elf_name` out of the gzip-compressed tar `body`, in memory.
///
/// No other entry is looked at and no entry name is used as a path.
fn extract_entry(
    body: &[u8],
    archive: &RomArchive,
    entry: &RomEntry,
) -> Result<Vec<u8>, RomStoreError> {
    let read_error = |e: &std::io::Error| RomStoreError::Archive {
        url: sanitize(archive.url),
        detail: sanitize(&e.to_string()),
    };
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(body));
    for member in tar.entries().map_err(|e| read_error(&e))? {
        let member = member.map_err(|e| read_error(&e))?;
        if !member.header().entry_type().is_file()
            || member.path_bytes().as_ref() != entry.elf_name.as_bytes()
        {
            continue;
        }
        if member.size() > MAX_ELF_BYTES {
            return Err(RomStoreError::Archive {
                url: sanitize(archive.url),
                detail: format!(
                    "the entry `{}` is {} bytes, above the {MAX_ELF_BYTES} byte (4 MiB) limit of a ROM ELF",
                    entry.elf_name,
                    member.size()
                ),
            });
        }
        let mut bytes = Vec::new();
        member
            .take(MAX_ELF_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| read_error(&e))?;
        return Ok(bytes);
    }
    Err(RomStoreError::MissingInArchive {
        url: archive.url.to_owned(),
        elf_name: entry.elf_name,
    })
}

/// Where a ROM file under verification came from, which decides what the user can do about a mismatch.
enum Provenance {
    /// A cache: deleting the file makes the next download store it again. `named` is the directory
    /// when the user chose it (`rom fetch --rom-dir`), `None` for the per-user default.
    Cache { named: Option<PathBuf> },
    /// A directory the user supplied: nothing is downloaded there unless `inkemu rom fetch --rom-dir` is told to.
    Directory { dir: PathBuf, release: &'static str },
    /// The verified archive itself held something else.
    Archive,
}

impl Provenance {
    /// The `inkemu rom fetch` invocation that restores the ROM files where they live (`--all` is
    /// mandatory without `--device`, and a directory the user chose must be named again).
    fn fetch_command(&self) -> String {
        match self {
            Provenance::Cache { named: None } | Provenance::Archive => {
                "inkemu rom fetch --all".to_owned()
            }
            Provenance::Cache { named: Some(dir) } | Provenance::Directory { dir, .. } => {
                format!("inkemu rom fetch --all --rom-dir {}", rom_dir_word(dir))
            }
        }
    }

    fn remedy(&self) -> String {
        match self {
            Provenance::Cache { named: None } => "delete it to download it again, or point --rom-dir at a directory holding the genuine file".to_owned(),
            Provenance::Cache { named: Some(_) } => format!(
                "delete it and run `{}` to download it again, or replace it with the genuine file",
                self.fetch_command()
            ),
            Provenance::Directory { release, .. } => format!(
                "replace it with the genuine file of esp-rom-elfs release {release}, or delete it and run `{}`",
                self.fetch_command()
            ),
            Provenance::Archive => "retry the download, or supply the ROM yourself with --rom-dir".to_owned(),
        }
    }
}

/// What stands for a directory that cannot be written the same way in every shell.
const DIRECTORY_PLACEHOLDER: &str = "<directory>";

/// `dir` as it is written after `--rom-dir` in a printed command, so that the command runs as printed in
/// a POSIX shell, in cmd.exe and in PowerShell alike:
/// - bare, when every character is inert in all three;
/// - between double quotes, when only characters that double quotes neutralise in all three (a space,
///   mostly) stand in the way;
/// - as [`DIRECTORY_PLACEHOLDER`] otherwise: no single quoting is right in all three, and the message
///   that carries the command already names the path.
fn rom_dir_word(dir: &Path) -> String {
    let text = dir.display().to_string();
    let inert = |c: char| {
        c.is_alphanumeric()
            || std::path::is_separator(c)
            || matches!(c, '.' | '_' | '-' | ':' | '~')
    };
    let inert_in_quotes =
        |c: char| inert(c) || matches!(c, ' ' | '\'' | '(' | ')' | ',' | '+' | '=' | '@');
    // A leading `-` reads as an option whatever the quoting; a leading `~` expands in a POSIX shell.
    if text.is_empty() || text.starts_with(['-', '~']) {
        DIRECTORY_PLACEHOLDER.to_owned()
    } else if text.chars().all(inert) {
        text
    // A backslash before the closing quote would escape it.
    } else if text.chars().all(inert_in_quotes) && !text.ends_with('\\') {
        format!("\"{text}\"")
    } else {
        DIRECTORY_PLACEHOLDER.to_owned()
    }
}

fn check_elf(
    bytes: &[u8],
    entry: &RomEntry,
    path: &Path,
    provenance: &Provenance,
) -> Result<(), RomStoreError> {
    let found = sha256_hex(bytes);
    if found == entry.elf_sha256 {
        return Ok(());
    }
    Err(RomStoreError::ElfMismatch {
        path: shown(path),
        expected: entry.elf_sha256,
        found,
        remedy: provenance.remedy(),
    })
}

/// The temporary file `store` writes before the rename. One helper, so tests can plant something there.
fn temporary_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.{}.tmp", std::process::id()))
}

/// Creates `path` and writes `bytes` into it. The file is created with `create_new`, which fails rather
/// than follow a symlink or reuse a file planted at the predictable temporary name; such a leftover (a
/// killed run, or an attack) is removed (a symlink itself, never its target) and the creation retried once.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), RomStoreError> {
    let create = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    };
    let mut file = match create() {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path).map_err(|e| io_error("remove", path, &e))?;
            create()
        }
        other => other,
    }
    .map_err(|e| io_error("create", path, &e))?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|e| {
            // Best effort: the write error is the one worth reporting.
            let _ = std::fs::remove_file(path);
            io_error("write", path, &e)
        })
}

/// Writes `bytes` to `dir/name` through a temporary file and a rename, so a crash or a concurrent
/// reader never sees a partial ROM.
fn store(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), RomStoreError> {
    let target = dir.join(name);
    std::fs::create_dir_all(dir).map_err(|e| io_error("create", dir, &e))?;
    let temporary = temporary_path(dir, name);
    write_new(&temporary, bytes)?;
    std::fs::rename(&temporary, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        io_error("move into place", &target, &e)
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// A path as it may be printed: control characters escaped, since it can come from the command line.
fn shown(path: &Path) -> String {
    sanitize(&path.display().to_string())
}

fn io_error(action: &'static str, path: &Path, error: &std::io::Error) -> RomStoreError {
    io_error_at(action, &path.display().to_string(), error)
}

fn io_error_at(action: &'static str, path: &str, error: &std::io::Error) -> RomStoreError {
    RomStoreError::Io {
        action,
        path: sanitize(path),
        detail: sanitize(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn sha(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }
    fn leak(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    /// A gzip-compressed tar archive holding `files`.
    fn archive_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            // `append_data` would refuse `..`: write the raw name so the hostile case stays testable.
            header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            tar.append(&header, *bytes).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    struct FakeFetch {
        body: Result<Vec<u8>, String>,
        calls: Cell<u32>,
    }
    impl FakeFetch {
        fn serving(body: Vec<u8>) -> FakeFetch {
            FakeFetch {
                body: Ok(body),
                calls: Cell::new(0),
            }
        }
    }
    impl Fetch for FakeFetch {
        fn fetch(&self, _url: &str, _max_bytes: u64) -> Result<Vec<u8>, String> {
            self.calls.set(self.calls.get() + 1);
            self.body.clone()
        }
    }

    const ELF_NAME: &str = "esp32c3_rev3_rom.elf";

    /// An archive, and the manifest that pins it and the tiny valid ROM ELF inside.
    fn fixture() -> (Vec<u8>, RomArchive, RomEntry) {
        let elf = soc_esp32c3::test_support::rom_elf().build();
        let image = RomImage::from_elf(&elf).unwrap();
        let body = archive_of(&[("esp32s3_rev0_rom.elf", b"other chip"), (ELF_NAME, &elf)]);
        let archive = RomArchive {
            release: "test",
            url: "https://example.invalid/roms.tar.gz",
            sha256: leak(sha(&body)),
            size: body.len() as u64,
        };
        let entry = RomEntry {
            rom_revision: 3,
            elf_name: ELF_NAME,
            elf_sha256: leak(sha(&elf)),
            image_sha256: leak(sha(image.bytes())),
        };
        (body, archive, entry)
    }

    fn files_in(dir: &std::path::Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn downloads_once_then_serves_from_the_cache() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("rom");
        let fetch = FakeFetch::serving(body);
        let source = RomSource::Cache {
            dir: cache.clone(),
            fetch: &fetch,
            user_named: false,
        };
        let first = load_rom(&source, &archive, &entry).unwrap();
        let second = load_rom(&source, &archive, &entry).unwrap();
        assert_eq!(first, second);
        assert_eq!(fetch.calls.get(), 1, "the second load reads the cache");
        assert_eq!(
            files_in(&cache),
            [ELF_NAME],
            "only the requested ELF is stored, no temporary file left"
        );
    }

    #[test]
    fn archive_hash_mismatch_is_refused_and_nothing_is_written() {
        let (mut body, archive, entry) = fixture();
        *body.last_mut().unwrap() ^= 1;
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch::serving(body.clone());
        let source = RomSource::Cache {
            dir: dir.path().to_path_buf(),
            fetch: &fetch,
            user_named: false,
        };
        let err = load_elf(&source, &archive, &entry).unwrap_err();
        assert!(
            matches!(err, RomStoreError::ArchiveMismatch { .. }),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains(archive.sha256)
                && text.contains(&sha(&body))
                && text.contains("--rom-dir"),
            "{text}"
        );
        assert_eq!(files_in(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn truncated_or_oversized_download_is_refused() {
        let (body, archive, entry) = fixture();
        for served in [
            body[..body.len() / 2].to_vec(),
            [body.clone(), vec![0; 16]].concat(),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let fetch = FakeFetch::serving(served);
            let source = RomSource::Cache {
                dir: dir.path().to_path_buf(),
                fetch: &fetch,
                user_named: false,
            };
            assert!(matches!(
                load_elf(&source, &archive, &entry),
                Err(RomStoreError::ArchiveMismatch { .. })
            ));
            assert_eq!(files_in(dir.path()), Vec::<String>::new());
        }
    }

    #[test]
    fn tampered_cached_file_is_refused_with_both_hashes() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let tampered = b"not the ROM".to_vec();
        std::fs::write(dir.path().join(ELF_NAME), &tampered).unwrap();
        let fetch = FakeFetch::serving(body);
        let source = RomSource::Cache {
            dir: dir.path().to_path_buf(),
            fetch: &fetch,
            user_named: false,
        };
        let err = load_elf(&source, &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::ElfMismatch { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains(ELF_NAME)
                && text.contains(entry.elf_sha256)
                && text.contains(&sha(&tampered)),
            "{text}"
        );
        assert!(
            text.contains("delete it"),
            "the cache remedy is to delete: {text}"
        );
        assert_eq!(
            fetch.calls.get(),
            0,
            "a suspicious file is never silently replaced"
        );
        assert_eq!(
            std::fs::read(dir.path().join(ELF_NAME)).unwrap(),
            tampered,
            "and never deleted either"
        );
    }

    #[test]
    fn download_failure_explains_the_offline_route() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch {
            body: Err("dns error: no such host".into()),
            calls: Cell::new(0),
        };
        let source = RomSource::Cache {
            dir: dir.path().to_path_buf(),
            fetch: &fetch,
            user_named: false,
        };
        let text = load_elf(&source, &archive, &entry).unwrap_err().to_string();
        for part in [archive.url, "dns error", ELF_NAME, "--rom-dir"] {
            assert!(text.contains(part), "{part} missing from: {text}");
        }
        assert_eq!(files_in(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn explicit_directory_never_downloads() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let err = load_elf(
            &RomSource::Directory(dir.path().to_path_buf()),
            &archive,
            &entry,
        )
        .unwrap_err();
        assert!(matches!(err, RomStoreError::NotInDirectory { .. }), "{err}");
        assert!(
            err.to_string().contains(ELF_NAME) && err.to_string().contains("esp-rom-elfs"),
            "{err}"
        );
        std::fs::write(
            dir.path().join(ELF_NAME),
            soc_esp32c3::test_support::rom_elf().build(),
        )
        .unwrap();
        assert!(
            load_rom(
                &RomSource::Directory(dir.path().to_path_buf()),
                &archive,
                &entry
            )
            .is_ok()
        );
    }

    #[test]
    fn only_the_exact_entry_name_is_extracted() {
        let elf = soc_esp32c3::test_support::rom_elf().build();
        let body = archive_of(&[
            ("../esp32c3_rev3_rom.elf", b"escape"),
            ("sub/esp32c3_rev3_rom.elf", b"nested"),
            (ELF_NAME, &elf),
        ]);
        let archive = RomArchive {
            release: "test",
            url: "https://example.invalid/a.tar.gz",
            sha256: leak(sha(&body)),
            size: body.len() as u64,
        };
        let entry = RomEntry {
            rom_revision: 3,
            elf_name: ELF_NAME,
            elf_sha256: leak(sha(&elf)),
            image_sha256: "",
        };
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let fetch = FakeFetch::serving(body);
        assert_eq!(
            load_elf(
                &RomSource::Cache {
                    dir: cache.clone(),
                    fetch: &fetch,
                    user_named: false,
                },
                &archive,
                &entry
            )
            .unwrap(),
            elf
        );
        assert_eq!(files_in(&cache), [ELF_NAME]);
        assert_eq!(
            files_in(root.path()),
            ["cache"],
            "nothing escaped the cache directory"
        );
    }

    #[test]
    fn missing_entry_and_wrong_entry_content_are_reported() {
        let (_, _, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        for (files, expect_missing) in [
            (vec![("esp32s3_rev0_rom.elf", &b"x"[..])], true),
            (vec![(ELF_NAME, &b"wrong bytes"[..])], false),
        ] {
            let body = archive_of(&files);
            let archive = RomArchive {
                release: "test",
                url: "https://example.invalid/a.tar.gz",
                sha256: leak(sha(&body)),
                size: body.len() as u64,
            };
            let fetch = FakeFetch::serving(body);
            let err = load_elf(
                &RomSource::Cache {
                    dir: dir.path().to_path_buf(),
                    fetch: &fetch,
                    user_named: false,
                },
                &archive,
                &entry,
            )
            .unwrap_err();
            assert_eq!(
                matches!(err, RomStoreError::MissingInArchive { .. }),
                expect_missing,
                "{err}"
            );
            assert_eq!(
                matches!(err, RomStoreError::ElfMismatch { .. }),
                !expect_missing,
                "{err}"
            );
            assert_eq!(files_in(dir.path()), Vec::<String>::new());
        }
    }

    #[test]
    fn oversized_entry_is_refused_before_it_is_read() {
        // A header that declares 5 MiB for the expected name, with no body: a forged entry costs nothing.
        let mut header = tar::Header::new_gnu();
        header.set_path(ELF_NAME).unwrap();
        header.set_size(5 * 1024 * 1024);
        header.set_mode(0o644);
        header.set_cksum();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, header.as_bytes()).unwrap();
        let body = gz.finish().unwrap();
        let archive = RomArchive {
            release: "test",
            url: "https://example.invalid/a.tar.gz",
            sha256: leak(sha(&body)),
            size: body.len() as u64,
        };
        let (_, _, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch::serving(body);
        let err = load_elf(
            &RomSource::Cache {
                dir: dir.path().to_path_buf(),
                fetch: &fetch,
                user_named: false,
            },
            &archive,
            &entry,
        )
        .unwrap_err();
        assert!(matches!(err, RomStoreError::Archive { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains(ELF_NAME) && text.contains("4 MiB") && text.contains(archive.url),
            "{text}"
        );
        assert_eq!(files_in(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn default_cache_dir_is_the_rom_folder_of_the_inkemu_data_dir() {
        // The environment cannot be set in-process (`set_var` is unsafe, `unsafe_code` is forbidden),
        // so a machine with no home directory has nothing to check here.
        match default_cache_dir() {
            Ok(dir) => {
                assert_eq!(dir.file_name().unwrap(), "rom");
                assert!(
                    dir.parent().unwrap().to_string_lossy().contains("inkemu"),
                    "{}",
                    dir.display()
                );
            }
            Err(RomStoreError::NoDataDir) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn image_hash_is_checked_after_conversion() {
        let (body, archive, mut entry) = fixture();
        entry.image_sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch::serving(body);
        let err = load_rom(
            &RomSource::Cache {
                dir: dir.path().to_path_buf(),
                fetch: &fetch,
                user_named: false,
            },
            &archive,
            &entry,
        )
        .unwrap_err();
        assert!(matches!(err, RomStoreError::ImageMismatch { .. }), "{err}");
    }

    #[test]
    fn a_file_that_is_not_a_rom_reports_the_conversion_error_with_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"\x7fELF but not really".to_vec();
        std::fs::write(dir.path().join(ELF_NAME), &bytes).unwrap();
        let archive = RomArchive {
            release: "test",
            url: "",
            sha256: "",
            size: 0,
        };
        let entry = RomEntry {
            rom_revision: 3,
            elf_name: ELF_NAME,
            elf_sha256: leak(sha(&bytes)),
            image_sha256: "",
        };
        let err = load_rom(
            &RomSource::Directory(dir.path().to_path_buf()),
            &archive,
            &entry,
        )
        .unwrap_err();
        assert!(matches!(err, RomStoreError::Rom { .. }), "{err}");
        assert!(err.to_string().contains(ELF_NAME), "{err}");
    }

    #[test]
    fn rom_entry_follows_the_next_lower_revision_rule() {
        let rom_for = |text: &str| rom_entry_for(text.parse().unwrap()).rom_revision;
        assert_eq!((rom_for("v0.0"), rom_for("v0.2")), (0, 0));
        assert_eq!(
            (rom_for("v0.3"), rom_for("v0.4"), rom_for("v1.0")),
            (3, 3, 3)
        );
        assert_eq!((rom_for("v1.1"), rom_for("v3.15")), (101, 101));
    }

    #[test]
    fn manifest_is_well_formed() {
        let hex64 = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        };
        assert!(
            hex64(ESP_ROM_ELFS.sha256)
                && ESP_ROM_ELFS
                    .url
                    .starts_with("https://github.com/espressif/esp-rom-elfs/releases/download/")
        );
        assert!(ESP_ROM_ELFS.url.contains(ESP_ROM_ELFS.release));
        for (entry, revision) in ESP32C3_ROMS.iter().zip([0, 3, 101]) {
            assert_eq!(entry.rom_revision, revision);
            assert_eq!(entry.elf_name, format!("esp32c3_rev{revision}_rom.elf"));
            assert!(hex64(entry.elf_sha256) && hex64(entry.image_sha256));
        }
    }

    /// Three distinct tiny ROM ELFs (one per revision of the real manifest), the archive holding them
    /// and the manifest entries that pin them.
    fn trio() -> (Vec<u8>, RomArchive, [RomEntry; 3], [Vec<u8>; 3]) {
        const REVISIONS: [(u16, &str); 3] = [
            (0, "esp32c3_rev0_rom.elf"),
            (3, "esp32c3_rev3_rom.elf"),
            (101, "esp32c3_rev101_rom.elf"),
        ];
        let elfs = [0u8, 1, 2].map(|variant| {
            let mut elf = soc_esp32c3::test_support::rom_elf();
            elf.sections[0].2[0] = 10 + variant; // another initial-data byte: another file hash
            elf.build()
        });
        let files: Vec<(&str, &[u8])> = REVISIONS
            .iter()
            .zip(&elfs)
            .map(|(&(_, name), elf)| (name, elf.as_slice()))
            .collect();
        let body = archive_of(&files);
        let archive = RomArchive {
            release: "test",
            url: "https://example.invalid/trio.tar.gz",
            sha256: leak(sha(&body)),
            size: body.len() as u64,
        };
        let entries = [0, 1, 2].map(|i| RomEntry {
            rom_revision: REVISIONS[i].0,
            elf_name: REVISIONS[i].1,
            elf_sha256: leak(sha(&elfs[i])),
            image_sha256: "",
        });
        (body, archive, entries, elfs)
    }

    fn cache_of<'a>(dir: &std::path::Path, fetch: &'a dyn Fetch) -> RomSource<'a> {
        RomSource::Cache {
            dir: dir.to_path_buf(),
            fetch,
            user_named: false,
        }
    }

    /// A cache in a directory the user chose, as `rom fetch --rom-dir` builds it.
    fn named_cache_of<'a>(dir: &std::path::Path, fetch: &'a dyn Fetch) -> RomSource<'a> {
        RomSource::Cache {
            dir: dir.to_path_buf(),
            fetch,
            user_named: true,
        }
    }

    #[test]
    fn every_missing_rom_comes_from_a_single_download() {
        let (body, archive, entries, elfs) = trio();
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch::serving(body);
        let all: Vec<&RomEntry> = entries.iter().collect();
        let loaded = ensure_elfs(&cache_of(dir.path(), &fetch), &archive, &all).unwrap();
        assert_eq!(loaded, elfs);
        assert_eq!(fetch.calls.get(), 1);
        let mut stored = entries.map(|e| e.elf_name.to_owned());
        stored.sort();
        assert_eq!(files_in(dir.path()), stored);
    }

    #[test]
    fn a_rom_already_present_is_left_alone_and_the_others_still_cost_one_download() {
        let (body, archive, entries, elfs) = trio();
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join(entries[1].elf_name);
        std::fs::write(&present, &elfs[1]).unwrap();
        let modified = std::fs::metadata(&present).unwrap().modified().unwrap();
        let fetch = FakeFetch::serving(body);
        let all: Vec<&RomEntry> = entries.iter().collect();
        let loaded = ensure_elfs(&cache_of(dir.path(), &fetch), &archive, &all).unwrap();
        assert_eq!(loaded, elfs);
        assert_eq!(fetch.calls.get(), 1);
        assert_eq!(files_in(dir.path()).len(), 3);
        assert_eq!(
            std::fs::metadata(&present).unwrap().modified().unwrap(),
            modified,
            "the file that was already there is not rewritten"
        );
    }

    #[test]
    fn nothing_is_downloaded_when_every_rom_is_present() {
        let (body, archive, entries, elfs) = trio();
        let dir = tempfile::tempdir().unwrap();
        for (entry, elf) in entries.iter().zip(&elfs) {
            std::fs::write(dir.path().join(entry.elf_name), elf).unwrap();
        }
        let fetch = FakeFetch::serving(body);
        let all: Vec<&RomEntry> = entries.iter().collect();
        assert_eq!(
            ensure_elfs(&cache_of(dir.path(), &fetch), &archive, &all).unwrap(),
            elfs
        );
        assert_eq!(fetch.calls.get(), 0);
    }

    #[test]
    fn a_tampered_rom_stops_the_batch_before_any_download_or_write() {
        let (body, archive, entries, elfs) = trio();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(entries[0].elf_name), &elfs[0]).unwrap();
        std::fs::write(dir.path().join(entries[2].elf_name), b"tampered").unwrap();
        let fetch = FakeFetch::serving(body);
        let all: Vec<&RomEntry> = entries.iter().collect();
        let err = ensure_elfs(&cache_of(dir.path(), &fetch), &archive, &all).unwrap_err();
        assert!(matches!(err, RomStoreError::ElfMismatch { .. }), "{err}");
        assert_eq!(fetch.calls.get(), 0, "no network access before the check");
        assert_eq!(
            files_in(dir.path()),
            [entries[0].elf_name, entries[2].elf_name],
            "the missing ROM was not fetched, nothing was written"
        );
    }

    #[test]
    fn an_explicit_directory_reports_the_first_missing_rom_without_downloading() {
        let (_, archive, entries, elfs) = trio();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(entries[0].elf_name), &elfs[0]).unwrap();
        let all: Vec<&RomEntry> = entries.iter().collect();
        let err = ensure_elfs(
            &RomSource::Directory(dir.path().to_path_buf()),
            &archive,
            &all,
        )
        .unwrap_err();
        assert!(
            matches!(&err, RomStoreError::NotInDirectory { path, .. } if path.contains(entries[1].elf_name)),
            "{err}"
        );
    }

    #[test]
    fn a_mismatch_in_an_explicit_directory_does_not_promise_a_download() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ELF_NAME), b"not the ROM").unwrap();
        let err = load_elf(
            &RomSource::Directory(dir.path().to_path_buf()),
            &archive,
            &entry,
        )
        .unwrap_err();
        let text = err.to_string();
        let command = format!(
            "`inkemu rom fetch --all --rom-dir {}`",
            dir.path().display()
        );
        assert!(
            text.contains(&command) && text.contains("esp-rom-elfs release test"),
            "{text}"
        );
        assert!(!text.contains("download it again"), "{text}");
    }

    #[test]
    fn a_cache_mismatch_names_no_command_that_needs_arguments() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ELF_NAME), b"not the ROM").unwrap();
        let fetch = FakeFetch::serving(Vec::new());
        let err = load_elf(&cache_of(dir.path(), &fetch), &archive, &entry).unwrap_err();
        assert!(!err.to_string().contains("inkemu rom fetch"), "{err}");
    }

    #[test]
    fn an_unsuitable_file_in_the_cache_suggests_a_fetch_command_that_runs_as_printed() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(ELF_NAME)).unwrap();
        let fetch = FakeFetch::serving(body);
        let err = load_elf(&cache_of(dir.path(), &fetch), &archive, &entry).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("`inkemu rom fetch --all`"), "{text}");
        assert!(!text.contains("--rom-dir"), "{text}");
    }

    #[test]
    fn an_unsuitable_file_in_a_user_directory_names_that_directory_in_the_fetch_command() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(ELF_NAME)).unwrap();
        let err = load_elf(
            &RomSource::Directory(dir.path().to_path_buf()),
            &archive,
            &entry,
        )
        .unwrap_err();
        assert!(matches!(err, RomStoreError::UnsuitableFile { .. }), "{err}");
        let command = format!(
            "`inkemu rom fetch --all --rom-dir {}`",
            dir.path().display()
        );
        assert!(err.to_string().contains(&command), "{err}");
    }

    #[test]
    fn an_unsuitable_file_in_a_cache_the_user_named_names_that_directory_in_the_fetch_command() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(ELF_NAME)).unwrap();
        let fetch = FakeFetch::serving(body);
        let err = load_elf(&named_cache_of(dir.path(), &fetch), &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::UnsuitableFile { .. }), "{err}");
        let command = format!(
            "`inkemu rom fetch --all --rom-dir {}`",
            dir.path().display()
        );
        assert!(err.to_string().contains(&command), "{err}");
        assert_eq!(fetch.calls.get(), 0);
    }

    #[test]
    fn a_mismatch_in_a_cache_the_user_named_names_the_command_that_downloads_into_it() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ELF_NAME), b"not the ROM").unwrap();
        let fetch = FakeFetch::serving(Vec::new());
        let text = load_elf(&named_cache_of(dir.path(), &fetch), &archive, &entry)
            .unwrap_err()
            .to_string();
        let command = format!(
            "`inkemu rom fetch --all --rom-dir {}`",
            dir.path().display()
        );
        assert!(text.contains(&command), "{text}");
        assert!(!text.contains("point --rom-dir"), "{text}");
    }

    #[test]
    fn a_directory_with_a_space_is_quoted_in_the_fetch_command() {
        let (_, archive, entry) = fixture();
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("my roms");
        std::fs::create_dir_all(dir.join(ELF_NAME)).unwrap();
        let err = load_elf(&RomSource::Directory(dir.clone()), &archive, &entry).unwrap_err();
        let command = format!("`inkemu rom fetch --all --rom-dir \"{}\"`", dir.display());
        assert!(err.to_string().contains(&command), "{err}");
    }

    #[test]
    fn a_directory_is_written_bare_only_when_every_shell_reads_it_as_one_word() {
        let word = |text: &str| rom_dir_word(Path::new(text));
        assert_eq!(word("roms"), "roms");
        assert_eq!(word("/tmp/.tmpAb12/rom_dir-1"), "/tmp/.tmpAb12/rom_dir-1");
        assert_eq!(word("/home/zoé/données"), "/home/zoé/données");
        assert_eq!(word("/tmp/RUNNER~1/x"), "/tmp/RUNNER~1/x");
    }

    #[test]
    fn a_directory_that_double_quotes_protect_in_every_shell_is_quoted() {
        let word = |text: &str| rom_dir_word(Path::new(text));
        assert_eq!(word("/tmp/my roms"), "\"/tmp/my roms\"");
        assert_eq!(
            word("/tmp/rom's (v3), a+b=c@d"),
            "\"/tmp/rom's (v3), a+b=c@d\""
        );
    }

    #[test]
    fn a_directory_no_quoting_protects_in_every_shell_becomes_a_placeholder() {
        let word = |text: &str| rom_dir_word(Path::new(text));
        for hostile in [
            "",
            "-roms",
            "~/roms",
            "/tmp/$HOME",
            "/tmp/a`b`",
            "/tmp/a\"b",
            "/tmp/50%",
            "/tmp/a!b",
            "/tmp/a;b",
            "/tmp/a|b",
            "/tmp/a&b",
            "/tmp/a*b",
            "/tmp/a\nb",
            "/tmp/my roms\\",
        ] {
            assert_eq!(word(hostile), DIRECTORY_PLACEHOLDER, "{hostile:?}");
        }
    }

    /// The text the decorator writes when the download starts, for the pinned test archive.
    fn notice_count(log: &[u8]) -> usize {
        String::from_utf8_lossy(log)
            .matches("inkemu: downloading the ESP32-C3 mask ROM")
            .count()
    }

    #[test]
    fn the_download_notice_is_not_shown_when_a_tampered_file_stops_the_batch() {
        let (body, archive, entries, elfs) = trio();
        let dir = tempfile::tempdir().unwrap();
        // rev0 missing, rev3 valid, rev101 tampered: the check refuses before any download.
        std::fs::write(dir.path().join(entries[1].elf_name), &elfs[1]).unwrap();
        std::fs::write(dir.path().join(entries[2].elf_name), b"tampered").unwrap();
        let inner = FakeFetch::serving(body);
        let mut log = Vec::new();
        let announcing = AnnouncingFetch::new(&inner, &mut log, &archive, dir.path());
        let all: Vec<&RomEntry> = entries.iter().collect();
        let err = ensure_elfs(&cache_of(dir.path(), &announcing), &archive, &all).unwrap_err();
        assert!(matches!(err, RomStoreError::ElfMismatch { .. }), "{err}");
        assert_eq!(inner.calls.get(), 0);
        drop(announcing);
        assert_eq!(notice_count(&log), 0, "{}", String::from_utf8_lossy(&log));
    }

    #[test]
    fn the_download_notice_is_shown_once_when_the_archive_is_downloaded() {
        let (body, archive, entries, elfs) = trio();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(entries[1].elf_name), &elfs[1]).unwrap();
        let inner = FakeFetch::serving(body);
        let mut log = Vec::new();
        let announcing = AnnouncingFetch::new(&inner, &mut log, &archive, dir.path());
        let all: Vec<&RomEntry> = entries.iter().collect();
        let loaded = ensure_elfs(&cache_of(dir.path(), &announcing), &archive, &all).unwrap();
        assert_eq!(loaded, elfs);
        assert_eq!(inner.calls.get(), 1);
        drop(announcing);
        let text = String::from_utf8_lossy(&log).into_owned();
        assert_eq!(notice_count(&log), 1, "{text}");
        assert!(
            text.contains("esp-rom-elfs test") && text.contains(&dir.path().display().to_string()),
            "{text}"
        );
    }

    #[test]
    fn the_download_notice_is_not_shown_when_every_rom_is_present() {
        let (body, archive, entries, elfs) = trio();
        let dir = tempfile::tempdir().unwrap();
        for (entry, elf) in entries.iter().zip(&elfs) {
            std::fs::write(dir.path().join(entry.elf_name), elf).unwrap();
        }
        let inner = FakeFetch::serving(body);
        let mut log = Vec::new();
        let announcing = AnnouncingFetch::new(&inner, &mut log, &archive, dir.path());
        let all: Vec<&RomEntry> = entries.iter().collect();
        ensure_elfs(&cache_of(dir.path(), &announcing), &archive, &all).unwrap();
        assert_eq!(inner.calls.get(), 0);
        drop(announcing);
        assert!(log.is_empty(), "{}", String::from_utf8_lossy(&log));
    }

    #[test]
    fn the_download_notice_precedes_a_failed_download() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let inner = FakeFetch {
            body: Err("dns error".into()),
            calls: Cell::new(0),
        };
        let mut log = Vec::new();
        let announcing = AnnouncingFetch::new(&inner, &mut log, &archive, dir.path());
        let err = load_elf(&cache_of(dir.path(), &announcing), &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::Download { .. }), "{err}");
        drop(announcing);
        assert_eq!(
            notice_count(&log),
            1,
            "a download was attempted, so it was announced"
        );
    }

    #[test]
    fn a_file_longer_than_any_rom_elf_is_refused_without_being_hashed_or_replaced() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ELF_NAME);
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_ELF_BYTES + 1).unwrap();
        let fetch = FakeFetch::serving(body);
        let err = load_elf(&cache_of(dir.path(), &fetch), &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::UnsuitableFile { .. }), "{err}");
        assert!(err.to_string().contains("4 MiB"), "{err}");
        assert_eq!(fetch.calls.get(), 0);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), MAX_ELF_BYTES + 1);
    }

    #[test]
    fn a_file_of_exactly_the_limit_is_read_and_judged_by_its_hash() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join(ELF_NAME)).unwrap();
        file.set_len(MAX_ELF_BYTES).unwrap();
        let err = load_elf(
            &RomSource::Directory(dir.path().to_path_buf()),
            &archive,
            &entry,
        )
        .unwrap_err();
        assert!(matches!(err, RomStoreError::ElfMismatch { .. }), "{err}");
    }

    #[test]
    fn a_directory_where_the_rom_should_be_is_refused() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(ELF_NAME)).unwrap();
        let fetch = FakeFetch::serving(body);
        let err = load_elf(&cache_of(dir.path(), &fetch), &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::UnsuitableFile { .. }), "{err}");
        assert!(
            err.to_string().contains("`inkemu rom fetch --all`"),
            "{err}"
        );
        assert_eq!(fetch.calls.get(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_planted_at_the_temporary_name_is_never_followed() {
        let (body, archive, entry) = fixture();
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("rom");
        std::fs::create_dir(&cache).unwrap();
        let victim = root.path().join("victim");
        std::fs::write(&victim, b"precious").unwrap();
        std::os::unix::fs::symlink(&victim, temporary_path(&cache, ELF_NAME)).unwrap();
        let fetch = FakeFetch::serving(body);
        load_rom(&cache_of(&cache, &fetch), &archive, &entry).unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
        assert_eq!(
            files_in(&cache),
            [ELF_NAME],
            "ELF in place, no temporary left"
        );
        assert!(
            !std::fs::symlink_metadata(cache.join(ELF_NAME))
                .unwrap()
                .is_symlink()
        );
    }

    #[test]
    fn a_stale_temporary_file_does_not_stop_the_load() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temporary_path(dir.path(), ELF_NAME),
            b"left by a killed run",
        )
        .unwrap();
        let fetch = FakeFetch::serving(body);
        load_rom(&cache_of(dir.path(), &fetch), &archive, &entry).unwrap();
        assert_eq!(files_in(dir.path()), [ELF_NAME]);
    }

    #[test]
    fn a_cache_directory_that_is_a_regular_file_cannot_be_created() {
        let root = tempfile::tempdir().unwrap();
        let not_a_directory = root.path().join("rom");
        std::fs::write(&not_a_directory, b"x").unwrap();
        let err = store(&not_a_directory, ELF_NAME, b"elf").unwrap_err();
        assert!(
            matches!(
                err,
                RomStoreError::Io {
                    action: "create",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn a_target_that_is_a_directory_reports_the_rename_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(ELF_NAME)).unwrap();
        let err = store(dir.path(), ELF_NAME, b"elf").unwrap_err();
        assert!(
            matches!(
                err,
                RomStoreError::Io {
                    action: "move into place",
                    ..
                }
            ),
            "{err}"
        );
        assert_eq!(
            files_in(dir.path()),
            [ELF_NAME],
            "only the directory is left"
        );
    }

    #[test]
    fn an_archive_that_is_not_a_gzip_tar_is_reported_as_an_unexpected_layout() {
        let body = b"pinned by hash but not an archive".to_vec();
        let archive = RomArchive {
            release: "test",
            url: "https://example.invalid/a.tar.gz",
            sha256: leak(sha(&body)),
            size: body.len() as u64,
        };
        let (_, _, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch::serving(body);
        let err = load_elf(&cache_of(dir.path(), &fetch), &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::Archive { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains(archive.url)
                && text.contains("report it")
                && !text.contains("permissions"),
            "{text}"
        );
    }

    /// A gzip-compressed tar archive whose members are `(name, entry type, bytes)`; a symlink member
    /// points at `/etc/passwd`.
    fn archive_with_types(members: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for &(name, kind, bytes) in members {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            if kind == tar::EntryType::Symlink {
                header.set_link_name("/etc/passwd").unwrap();
            }
            header.set_cksum();
            tar.append(&header, bytes).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn a_symlink_member_with_the_expected_name_is_not_taken_for_the_rom() {
        let elf = soc_esp32c3::test_support::rom_elf().build();
        let (_, _, entry) = fixture();
        for (members, extracted) in [
            (
                vec![
                    (ELF_NAME, tar::EntryType::Symlink, &b""[..]),
                    (ELF_NAME, tar::EntryType::Regular, &elf[..]),
                ],
                true,
            ),
            (vec![(ELF_NAME, tar::EntryType::Symlink, &b""[..])], false),
        ] {
            let body = archive_with_types(&members);
            let archive = RomArchive {
                release: "test",
                url: "https://example.invalid/a.tar.gz",
                sha256: leak(sha(&body)),
                size: body.len() as u64,
            };
            let dir = tempfile::tempdir().unwrap();
            let fetch = FakeFetch::serving(body);
            let result = load_elf(&cache_of(dir.path(), &fetch), &archive, &entry);
            if extracted {
                assert_eq!(result.unwrap(), elf);
            } else {
                assert!(
                    matches!(result, Err(RomStoreError::MissingInArchive { .. })),
                    "{result:?}"
                );
            }
        }
    }
}
