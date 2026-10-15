//! A blank SD image: an empty FAT32 volume on an MBR partition (design `SP2e`, 2e-D15).
//!
//! The layout is the SD Association's, as `SdFat` 2.3.1 writes it (research `sp2e-sd-card.md` §2
//! point 9): one partition of type `0Ch` starting at sector 8 192 (4 MiB), its data area on an
//! 8 192-sector boundary, two FATs, the boot sector and the `FSInfo` sector with their copies in
//! sectors 6 and 7, a root directory of one cluster holding the volume label. The clusters are the
//! largest power of two up to 32 KiB that leaves at least 65 525 + 16 of them, as FAT32 needs.
//! Nothing comes from the clock and the serial number is fixed: two creations of one size and
//! label give the same bytes (2e-R8). `CrossPoint` needs nothing on the volume to reach its home
//! screen; it creates its `/.crosspoint` directory itself (research §3).
//!
//! Its size is set, and only the structures are written; the rest reads zeros. On Unix
//! filesystems the file is then sparse; on NTFS, a file is not sparse unless marked so, and the
//! image takes its whole size on the disk from its creation. It is written under a temporary name, mode `0600` on Unix, flushed, then moved to its
//! name without ever replacing what took it meanwhile (`rename_no_replace`). A journal left
//! there by a former image is refused: the new image would apply it as its own.
use crate::flash_state::temporary_path;
use crate::sd_image::{journal_path, journal_temporary_path, sync_directory};
use crate::session_file::Claimed;
use crate::term::sanitize;
use chips::sdcard::{Capacity, MAX_CAPACITY_BYTES};
use std::fs::OpenOptions;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::Path;

const MIB: u64 = 1024 * 1024;
const SECTOR: u64 = 512;
/// The boundary unit of the SD layout: 8 192 sectors, 4 MiB.
const BOUNDARY: u64 = 8192;
/// The fewest reserved sectors: the boot sector, `FSInfo`, their copies, room around them.
const MIN_RESERVED: u64 = 32;
/// FAT32 needs at least 65 525 clusters; 16 more keep away from the boundary, where FAT
/// implementations disagree (Microsoft's FAT specification recommends the margin).
const MIN_CLUSTERS: u64 = 65_525 + 16;
/// The highest cluster count FAT32 numbers (clusters 2 to `0x0FFF_FFF6`).
const MAX_CLUSTERS: u64 = 0x0FFF_FFF5;
/// The volume serial number and the MBR's disk signature: fixed, the bytes "IKMU".
const VOLUME_ID: u32 = 0x554D_4B49;
/// 1980-01-01, the first date FAT can tell: the date of the volume label's entry.
const FIRST_FAT_DATE: u16 = 0x0021;

/// The smallest image: 64 MiB.
pub const MIN_MEBIBYTES: u64 = 64;
/// The largest image: the largest card the SPI mode addresses, 2 TiB − 128 MiB.
pub const MAX_MEBIBYTES: u64 = MAX_CAPACITY_BYTES / MIB;
/// The label of a volume when none is given.
pub const DEFAULT_LABEL: &str = "INKEMU";

/// Why a blank image cannot be made. The messages state the cause, then the remedy.
#[derive(Debug, thiserror::Error)]
pub enum SdFormatError {
    #[error(
        "an SD image of {mebibytes} MiB is too small: FAT32 needs at least {MIN_MEBIBYTES} MiB"
    )]
    TooSmall { mebibytes: u64 },
    #[error(
        "an SD image of {mebibytes} MiB is larger than any card the SPI mode addresses ({MAX_MEBIBYTES} MiB)"
    )]
    TooLarge { mebibytes: u64 },
    #[error("the volume label `{label}` cannot be used: {why}")]
    Label { label: String, why: &'static str },
    #[error("the SD image path {path} names no file; give the path of a file, like card.img")]
    NoFileName { path: String },
    #[error(
        "{path} already exists; a blank image never replaces a file: remove it, or give another path"
    )]
    Exists { path: String },
    #[error(
        "the SD image {path} would have its file {file}, which is {what}; give the image another path"
    )]
    Claimed {
        path: String,
        file: String,
        what: String,
    },
    #[error(
        "{journal} is the journal of an image that was at {path}; a blank image there would take it as its own: remove the journal, or give another path"
    )]
    JournalPresent { path: String, journal: String },
    #[error(
        "cannot create the SD image {path}: cannot {action} {target}: {detail}; check that the directory exists, is writable and has room"
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

/// The geometry of a blank image, in sectors of 512 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fat32Layout {
    /// The whole image.
    pub total_sectors: u64,
    /// Where the partition begins.
    pub partition_start: u64,
    /// The partition's length: to the end of the image.
    pub partition_sectors: u64,
    /// Before the first FAT, from the partition's start.
    pub reserved_sectors: u64,
    /// The length of each of the two FATs.
    pub fat_sectors: u64,
    pub sectors_per_cluster: u64,
    /// Where cluster 2 begins, from the start of the image.
    pub data_start: u64,
    pub clusters: u64,
    /// The volume label, padded with spaces.
    pub label: [u8; 11],
}

impl Fat32Layout {
    /// The layout of a blank image of `mebibytes` MiB labelled `label`.
    ///
    /// # Errors
    /// [`SdFormatError::TooSmall`], [`SdFormatError::TooLarge`], [`SdFormatError::Label`].
    pub fn new(mebibytes: u64, label: &str) -> Result<Fat32Layout, SdFormatError> {
        if mebibytes < MIN_MEBIBYTES {
            return Err(SdFormatError::TooSmall { mebibytes });
        }
        if mebibytes > MAX_MEBIBYTES {
            return Err(SdFormatError::TooLarge { mebibytes });
        }
        let label = volume_label(label)?;
        let total_sectors = mebibytes * MIB / SECTOR;
        let bytes = mebibytes * MIB;
        // Microsoft's table of default cluster sizes for FAT32, capped at 32 KiB as SD cards are.
        let mut sectors_per_cluster = match bytes {
            b if b <= 260 * MIB => 1,
            b if b <= 8 * 1024 * MIB => 8,
            b if b <= 16 * 1024 * MIB => 16,
            b if b <= 32 * 1024 * MIB => 32,
            _ => 64,
        };
        loop {
            if let Some((reserved_sectors, fat_sectors, data_start, clusters)) =
                geometry(total_sectors, sectors_per_cluster)
                && (MIN_CLUSTERS..=MAX_CLUSTERS).contains(&clusters)
            {
                return Ok(Fat32Layout {
                    total_sectors,
                    partition_start: BOUNDARY,
                    partition_sectors: total_sectors - BOUNDARY,
                    reserved_sectors,
                    fat_sectors,
                    sectors_per_cluster,
                    data_start,
                    clusters,
                    label,
                });
            }
            if sectors_per_cluster == 1 {
                return Err(SdFormatError::TooSmall { mebibytes });
            }
            sectors_per_cluster /= 2;
        }
    }

    /// The image's size in bytes.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.total_sectors * SECTOR
    }

    /// What the image is, in one line: its size, the kind of card it makes, its volume.
    #[must_use]
    pub fn describe(&self) -> String {
        let kind = Capacity::new(self.bytes()).map_or("no", |capacity| capacity.kind().name());
        format!(
            "{} MiB {kind} card, FAT32, {} clusters of {} bytes, label {}",
            self.bytes() / MIB,
            self.clusters,
            self.sectors_per_cluster * SECTOR,
            String::from_utf8_lossy(&self.label).trim_end()
        )
    }

    /// The first sector of the first FAT, from the start of the image.
    fn fat_start(&self) -> u64 {
        self.partition_start + self.reserved_sectors
    }

    /// Everything of the image that is not zero: `(byte offset, bytes)`, in order.
    #[must_use]
    pub fn regions(&self) -> Vec<(u64, Vec<u8>)> {
        let boot = self.boot_sector();
        let fs_info = self.fs_info();
        let mut boot_region = vec![0u8; 8 * 512];
        for (sector, bytes) in [(0, &boot), (1, &fs_info), (6, &boot), (7, &fs_info)] {
            if let Some(place) = boot_region.get_mut(sector * 512..(sector + 1) * 512) {
                place.copy_from_slice(bytes);
            }
        }
        // FAT entries 0 (the media descriptor), 1 (end of chain, the volume clean) and 2 (the
        // root directory, one cluster).
        let mut fat_head = Vec::with_capacity(12);
        for entry in [0x0FFF_FFF8u32, 0x0FFF_FFFF, 0x0FFF_FFFF] {
            fat_head.extend_from_slice(&entry.to_le_bytes());
        }
        vec![
            (0, self.mbr().to_vec()),
            (self.partition_start * SECTOR, boot_region),
            (self.fat_start() * SECTOR, fat_head.clone()),
            ((self.fat_start() + self.fat_sectors) * SECTOR, fat_head),
            (self.data_start * SECTOR, self.label_entry().to_vec()),
        ]
    }

    fn mbr(&self) -> [u8; 512] {
        let mut mbr = [0u8; 512];
        put(&mut mbr, 440, &VOLUME_ID.to_le_bytes());
        let mut entry = [0u8; 16];
        entry[1..4].copy_from_slice(&chs(self.partition_start));
        entry[4] = 0x0C;
        entry[5..8].copy_from_slice(&chs(self.total_sectors - 1));
        entry[8..12].copy_from_slice(&sectors32(self.partition_start));
        entry[12..16].copy_from_slice(&sectors32(self.partition_sectors));
        put(&mut mbr, 446, &entry);
        put(&mut mbr, 510, &[0x55, 0xAA]);
        mbr
    }

    fn boot_sector(&self) -> [u8; 512] {
        let mut boot = [0u8; 512];
        put(&mut boot, 0, &[0xEB, 0x58, 0x90]);
        put(&mut boot, 3, b"INKEMU  ");
        put(&mut boot, 11, &512u16.to_le_bytes());
        put(
            &mut boot,
            13,
            &[u8::try_from(self.sectors_per_cluster).unwrap_or(0)],
        );
        put(
            &mut boot,
            14,
            &u16::try_from(self.reserved_sectors)
                .unwrap_or(0)
                .to_le_bytes(),
        );
        put(&mut boot, 16, &[2]); // FATs
        put(&mut boot, 21, &[0xF8]); // fixed disk
        put(&mut boot, 24, &63u16.to_le_bytes()); // sectors per track
        put(&mut boot, 26, &255u16.to_le_bytes()); // heads
        put(&mut boot, 28, &sectors32(self.partition_start)); // hidden sectors
        put(&mut boot, 32, &sectors32(self.partition_sectors));
        put(&mut boot, 36, &sectors32(self.fat_sectors));
        put(&mut boot, 44, &2u32.to_le_bytes()); // root directory cluster
        put(&mut boot, 48, &1u16.to_le_bytes()); // FSInfo sector
        put(&mut boot, 50, &6u16.to_le_bytes()); // backup boot sector
        put(&mut boot, 64, &[0x80, 0x00, 0x29]); // drive, reserved, extended signature
        put(&mut boot, 67, &VOLUME_ID.to_le_bytes());
        put(&mut boot, 71, &self.label);
        put(&mut boot, 82, b"FAT32   ");
        put(&mut boot, 510, &[0x55, 0xAA]);
        boot
    }

    fn fs_info(&self) -> [u8; 512] {
        let mut info = [0u8; 512];
        put(&mut info, 0, &0x4161_5252u32.to_le_bytes());
        put(&mut info, 484, &0x6141_7272u32.to_le_bytes());
        // Free clusters: all but the root directory's; the next free one is 3.
        put(&mut info, 488, &sectors32(self.clusters - 1));
        put(&mut info, 492, &3u32.to_le_bytes());
        put(&mut info, 508, &0xAA55_0000u32.to_le_bytes());
        info
    }

    fn label_entry(&self) -> [u8; 32] {
        let mut entry = [0u8; 32];
        put(&mut entry, 0, &self.label);
        entry[11] = 0x08; // volume label
        put(&mut entry, 16, &FIRST_FAT_DATE.to_le_bytes()); // creation date
        put(&mut entry, 18, &FIRST_FAT_DATE.to_le_bytes()); // access date
        put(&mut entry, 24, &FIRST_FAT_DATE.to_le_bytes()); // write date
        entry
    }
}

/// The reserved sectors, the FAT's length, the data start and the cluster count of a partition
/// at [`BOUNDARY`] in an image of `total` sectors with clusters of `per_cluster` sectors; `None`
/// when no data area fits.
fn geometry(total: u64, per_cluster: u64) -> Option<(u64, u64, u64, u64)> {
    let mut fat = 1;
    loop {
        let data_start = (BOUNDARY + MIN_RESERVED + 2 * fat).div_ceil(BOUNDARY) * BOUNDARY;
        let data = total.checked_sub(data_start).filter(|&data| data > 0)?;
        let clusters = data / per_cluster;
        let needed = (clusters + 2).checked_mul(4)?.div_ceil(SECTOR);
        if needed <= fat {
            return Some((data_start - BOUNDARY - 2 * fat, fat, data_start, clusters));
        }
        fat = needed;
    }
}

/// A sector count as the 32 bits the structures hold it in. Every count here is below 2³².
fn sectors32(count: u64) -> [u8; 4] {
    u32::try_from(count).unwrap_or(u32::MAX).to_le_bytes()
}

fn put(into: &mut [u8], at: usize, bytes: &[u8]) {
    if let Some(place) = into.get_mut(at..at + bytes.len()) {
        place.copy_from_slice(bytes);
    }
}

/// The cylinder, head and sector of a sector in the MBR's form (255 heads, 63 sectors), or the
/// maximum the form tells when the sector is beyond it.
fn chs(lba: u64) -> [u8; 3] {
    const SECTORS: u64 = 63;
    const HEADS: u64 = 255;
    if lba >= 1024 * HEADS * SECTORS {
        return [0xFE, 0xFF, 0xFF];
    }
    let cylinder = lba / (HEADS * SECTORS);
    let head = lba / SECTORS % HEADS;
    let sector = lba % SECTORS + 1;
    // INVARIANT: head < 255, sector <= 63 and cylinder < 1024.
    let byte = |value: u64| u8::try_from(value & 0xFF).unwrap_or(0);
    [
        byte(head),
        byte(sector | (cylinder >> 2) & 0xC0),
        byte(cylinder),
    ]
}

/// The label as the boot sector holds it: upper-case, padded with spaces.
fn volume_label(label: &str) -> Result<[u8; 11], SdFormatError> {
    let refuse = |why| SdFormatError::Label {
        label: sanitize(label),
        why,
    };
    if label.is_empty() {
        return Err(refuse("it is empty"));
    }
    if label.len() > 11 {
        return Err(refuse("it is longer than 11 characters"));
    }
    if label.starts_with(' ') {
        return Err(refuse("it begins with a space"));
    }
    let mut bytes = [b' '; 11];
    for (place, character) in bytes.iter_mut().zip(label.bytes()) {
        let upper = character.to_ascii_uppercase();
        if !(upper.is_ascii_uppercase()
            || upper.is_ascii_digit()
            || b" !#$%&'()-@^_`{}~".contains(&upper))
        {
            return Err(refuse(
                "FAT takes letters, digits, spaces and ! # $ % & ' ( ) - @ ^ _ ` { } ~ only",
            ));
        }
        *place = upper;
    }
    Ok(bytes)
}

/// Creates a blank image of `mebibytes` MiB at `path`, labelled `label`, and gives its layout.
/// `claimed` are the files a run already reads or writes (empty from the command line): the
/// image, its temporary name and its journals must be none of them. `log` gets the warning of an
/// image created whose temporary name could not be removed.
///
/// # Errors
/// [`SdFormatError`]: a size or a label FAT32 cannot take, a path that names no file or where
/// something already is, a file the run already uses, a journal left by a former image, a file
/// that cannot be written (nothing is left then).
pub fn create_blank(
    path: &Path,
    mebibytes: u64,
    label: &str,
    claimed: &[Claimed],
    log: &mut dyn std::io::Write,
) -> Result<Fat32Layout, SdFormatError> {
    let layout = Fat32Layout::new(mebibytes, label)?;
    let shown_path = shown(path);
    let ends_in_a_separator = path
        .as_os_str()
        .to_string_lossy()
        .ends_with(std::path::is_separator);
    let (Some(temporary), Some(journal), Some(journal_temporary)) = (
        temporary_path(path).filter(|_| !ends_in_a_separator),
        journal_path(path),
        journal_temporary_path(path),
    ) else {
        return Err(SdFormatError::NoFileName { path: shown_path });
    };
    let own: [&Path; 4] = [path, &temporary, &journal, &journal_temporary];
    for claim in claimed {
        if let Some(file) = own.iter().find(|file| claim.holds(file)) {
            return Err(SdFormatError::Claimed {
                path: shown_path,
                file: shown(file),
                what: claim.what().to_owned(),
            });
        }
    }
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(SdFormatError::Exists { path: shown_path });
    }
    if let Some(left) = [&journal, &journal_temporary]
        .into_iter()
        .find(|file| std::fs::symlink_metadata(file).is_ok())
    {
        return Err(SdFormatError::JournalPresent {
            path: shown_path,
            journal: shown(left),
        });
    }
    let failed = |action: &'static str, target: &Path, e: &std::io::Error| {
        let _ = std::fs::remove_file(&temporary);
        SdFormatError::Write {
            path: shown_path.clone(),
            action,
            target: shown(target),
            detail: sanitize(&e.to_string()),
        }
    };
    // What sits at the temporary name is a creation that was interrupted: removed, never
    // followed.
    if std::fs::symlink_metadata(&temporary).is_ok() {
        std::fs::remove_file(&temporary).map_err(|e| failed("remove", &temporary, &e))?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|e| failed("create", &temporary, &e))?;
    file.set_len(layout.bytes())
        .map_err(|e| failed("size", &temporary, &e))?;
    for (offset, bytes) in layout.regions() {
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.write_all(&bytes))
            .map_err(|e| failed("write", &temporary, &e))?;
    }
    file.sync_all()
        .map_err(|e| failed("write", &temporary, &e))?;
    drop(file);
    match rename_no_replace(&temporary, path) {
        Ok(None) => {}
        // The image is in place under its name; the temporary one is a second name of it.
        Ok(Some(e)) => {
            let _ = writeln!(
                log,
                "inkemu: warning: the SD image {shown_path} is created, but {} could not be removed ({}); it is a second name of the same file (a hard link), so writing either writes the image: remove it",
                shown(&temporary),
                sanitize(&e.to_string())
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&temporary);
            return Err(SdFormatError::Exists { path: shown_path });
        }
        Err(e) => return Err(failed("move into place", path, &e)),
    }
    sync_directory(path);
    Ok(layout)
}

/// Moves `from` to `to`, and refuses with [`std::io::ErrorKind::AlreadyExists`] when something
/// is at `to`: what took the name meanwhile is never replaced. Linux (glibc) does it in one call,
/// `renameat2` with `RENAME_NOREPLACE`; elsewhere, or where the filesystem refuses the flag,
/// `to` is made a hard link to `from`, which fails when `to` exists, then `from` is removed. On
/// a filesystem without hard links, the last resort is a check then a rename, which leaves a
/// window where a file created at `to` would be replaced.
///
/// `Ok(Some(error))`: `to` is in place, but `from`, a second name of the same file after the
/// hard link, could not be removed; the caller says so.
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<Option<std::io::Error>> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use nix::errno::Errno;
        use nix::fcntl::{AT_FDCWD, RenameFlags, renameat2};
        match renameat2(AT_FDCWD, from, AT_FDCWD, to, RenameFlags::RENAME_NOREPLACE) {
            Ok(()) => return Ok(None),
            // The kernel or the filesystem does not take the flag: the links below.
            Err(Errno::EINVAL | Errno::ENOSYS | Errno::EOPNOTSUPP) => {}
            Err(e) => return Err(e.into()),
        }
    }
    link_into_place(from, to, |from| std::fs::remove_file(from))
}

/// The links of [`rename_no_replace`]: `to` made a hard link to `from`, then `from` removed by
/// `remove`. Once the link exists, `to` is in place: a failed removal is told, not an error.
fn link_into_place(
    from: &Path,
    to: &Path,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<Option<std::io::Error>> {
    match std::fs::hard_link(from, to) {
        Ok(()) => Ok(remove(from).err()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            if std::fs::symlink_metadata(to).is_ok() {
                return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
            }
            std::fs::rename(from, to).map(|()| None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, Write};

    const GIB: u64 = 1024;

    #[test]
    fn the_layout_aligns_the_data_and_keeps_enough_clusters() {
        for (mebibytes, per_cluster) in [
            (64, 1),
            (260, 1),
            (261, 4),
            (1024, 8),
            (4 * GIB, 8),
            (32 * GIB, 32),
            (64 * GIB, 64),
            (MAX_MEBIBYTES, 64),
        ] {
            let layout = Fat32Layout::new(mebibytes, DEFAULT_LABEL).unwrap();
            assert_eq!(layout.sectors_per_cluster, per_cluster, "{mebibytes} MiB");
            assert_eq!(layout.total_sectors, mebibytes * 2048);
            assert_eq!(layout.partition_start, 8192);
            assert_eq!(layout.data_start % 8192, 0, "data on a 4 MiB boundary");
            assert!(layout.reserved_sectors >= 32);
            assert_eq!(
                layout.partition_start + layout.reserved_sectors + 2 * layout.fat_sectors,
                layout.data_start
            );
            assert!(
                layout.clusters >= 65_541,
                "{mebibytes} MiB: {}",
                layout.clusters
            );
            assert!(
                layout.fat_sectors * 128 >= layout.clusters + 2,
                "the FAT holds every cluster"
            );
            assert!(layout.data_start + layout.clusters * per_cluster <= layout.total_sectors);
            assert_eq!(layout.bytes(), mebibytes * MIB);
        }
    }

    #[test]
    fn a_size_or_a_label_fat32_cannot_take_is_refused() {
        let message = |result: Result<Fat32Layout, SdFormatError>| result.unwrap_err().to_string();
        assert_eq!(
            message(Fat32Layout::new(63, "X")),
            "an SD image of 63 MiB is too small: FAT32 needs at least 64 MiB"
        );
        assert!(message(Fat32Layout::new(MAX_MEBIBYTES + 1, "X")).contains("larger than any card"));
        assert!(message(Fat32Layout::new(64, "")).contains("it is empty"));
        assert!(message(Fat32Layout::new(64, "TWELVE CHARS")).contains("longer than 11"));
        assert!(message(Fat32Layout::new(64, " LEAD")).contains("begins with a space"));
        assert!(message(Fat32Layout::new(64, "A/B")).contains("letters, digits"));
        assert!(message(Fat32Layout::new(64, "É")).contains("letters, digits"));
        assert_eq!(
            &Fat32Layout::new(64, "my card").unwrap().label,
            b"MY CARD    "
        );
        assert_eq!(
            Fat32Layout::new(64, "my card").unwrap().describe(),
            "64 MiB SDSC card, FAT32, 114688 clusters of 512 bytes, label MY CARD"
        );
        assert!(
            Fat32Layout::new(4096, DEFAULT_LABEL)
                .unwrap()
                .describe()
                .starts_with("4096 MiB SDHC/SDXC card, FAT32, 1044480 clusters of 4096 bytes")
        );
    }

    #[test]
    fn the_structures_are_where_the_specifications_put_them() {
        let layout = Fat32Layout::new(64, DEFAULT_LABEL).unwrap();
        let regions = layout.regions();
        let mbr = &regions[0].1;
        assert_eq!(mbr[510..], [0x55, 0xAA]);
        assert_eq!(mbr[446], 0x00, "not bootable");
        assert_eq!(mbr[450], 0x0C, "FAT32, LBA");
        assert_eq!(mbr[454..458], 8192u32.to_le_bytes());
        assert_eq!(
            mbr[458..462],
            u32::try_from(layout.partition_sectors)
                .unwrap()
                .to_le_bytes()
        );
        let (offset, boot) = &regions[1];
        assert_eq!(*offset, 8192 * 512);
        assert_eq!(boot[11..13], 512u16.to_le_bytes());
        assert_eq!(boot[16], 2);
        assert_eq!(boot[17..19], [0, 0], "no FAT16 root directory");
        assert_eq!(boot[22..24], [0, 0], "no FAT16 size");
        assert_eq!(boot[82..90], *b"FAT32   ");
        assert_eq!(boot[510..512], [0x55, 0xAA]);
        assert_eq!(
            boot[..512],
            boot[6 * 512..7 * 512],
            "the backup boot sector"
        );
        assert_eq!(boot[512..1024], boot[7 * 512..8 * 512], "the backup FSInfo");
        assert_eq!(
            boot[512 + 488..512 + 492],
            u32::try_from(layout.clusters - 1).unwrap().to_le_bytes()
        );
        assert_eq!(regions[2].1, regions[3].1, "two FATs, the same");
        assert_eq!(regions[3].0 - regions[2].0, layout.fat_sectors * 512);
        assert_eq!(regions[4].0, layout.data_start * 512);
        assert_eq!(regions[4].1[..11], *b"INKEMU     ");
        assert_eq!(regions[4].1[11], 0x08);
        assert_eq!(chs(0), [0, 1, 0]);
        assert_eq!(chs(8192), [130, 3, 0], "8 192 = 130 × 63 + 2");
        assert_eq!(
            chs(16_065 * 300),
            [0, 0x40 | 1, 0x2C],
            "cylinder 300: its bits 9:8 in the sector byte"
        );
        assert_eq!(chs(u64::MAX), [0xFE, 0xFF, 0xFF]);
    }

    #[test]
    fn a_blank_image_is_sparse_owned_deterministic_and_never_replaces_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.img");
        let two = dir.path().join("two.img");
        let layout = create_blank(&one, 64, DEFAULT_LABEL, &[], &mut Vec::new()).unwrap();
        create_blank(&two, 64, DEFAULT_LABEL, &[], &mut Vec::new()).unwrap();
        assert_eq!(std::fs::read(&one).unwrap(), std::fs::read(&two).unwrap());
        assert_eq!(std::fs::metadata(&one).unwrap().len(), layout.bytes());
        assert!(!dir.path().join(".one.img.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
            let metadata = std::fs::metadata(&one).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            assert!(
                metadata.blocks() * 512 < layout.bytes() / 4,
                "sparse: {} blocks",
                metadata.blocks()
            );
        }
        let error = create_blank(&one, 64, DEFAULT_LABEL, &[], &mut Vec::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("already exists"), "{error}");
        let error = create_blank(
            &dir.path().join("missing").join("x.img"),
            64,
            DEFAULT_LABEL,
            &[],
            &mut Vec::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.starts_with("cannot create the SD image"), "{error}");
        let error = create_blank(Path::new("dir/"), 64, DEFAULT_LABEL, &[], &mut Vec::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("names no file"), "{error}");
    }

    #[test]
    fn the_move_into_place_never_replaces_what_took_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&to, b"somebody's").unwrap();
        let error = rename_no_replace(&from, &to).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&from).unwrap(), b"new");
        assert_eq!(std::fs::read(&to).unwrap(), b"somebody's");
        std::fs::remove_file(&to).unwrap();
        assert!(rename_no_replace(&from, &to).unwrap().is_none());
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
        // The links, as elsewhere than Linux: the same refusal.
        std::fs::write(&from, b"newer").unwrap();
        let error = link_into_place(&from, &to, |from| std::fs::remove_file(from)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
    }

    #[test]
    fn a_link_in_place_whose_temporary_name_stays_is_a_success_told() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        std::fs::write(&from, b"image").unwrap();
        let left = link_into_place(&from, &to, |_| Err(std::io::Error::other("busy")))
            .unwrap()
            .unwrap();
        assert_eq!(left.to_string(), "busy");
        assert_eq!(std::fs::read(&to).unwrap(), b"image", "in place");
        assert!(from.exists(), "the second name, told to the caller");
    }

    #[test]
    fn a_journal_left_by_a_former_image_is_refused_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.img");
        for left in [".card.img.journal", ".card.img.journal.tmp"] {
            let journal = dir.path().join(left);
            std::fs::write(&journal, b"a former image's").unwrap();
            let error = create_blank(&path, 64, DEFAULT_LABEL, &[], &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert!(error.contains("is the journal of an image"), "{error}");
            assert!(error.contains(left), "{error}");
            assert!(!path.exists());
            assert_eq!(std::fs::read(&journal).unwrap(), b"a former image's");
            std::fs::remove_file(&journal).unwrap();
        }
    }

    #[test]
    fn a_file_the_run_already_uses_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.img");
        for (file, name) in [
            (path.clone(), "card.img"),
            (dir.path().join(".card.img.tmp"), ".card.img.tmp"),
            (dir.path().join(".card.img.journal"), ".card.img.journal"),
            (
                dir.path().join(".card.img.journal.tmp"),
                ".card.img.journal.tmp",
            ),
        ] {
            let claim = Claimed::File {
                path: file,
                what: "the session of --record".into(),
            };
            let error = create_blank(&path, 64, DEFAULT_LABEL, &[claim], &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("which is the session of --record"),
                "{error}"
            );
            assert!(error.contains(name), "{error}");
            assert!(!path.exists());
        }
    }

    /// The partition of an image, as a volume `fatfs` reads.
    struct Partition {
        file: std::fs::File,
        start: u64,
    }

    impl Read for Partition {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.file.read(buf)
        }
    }

    impl Write for Partition {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.file.write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }

    impl Seek for Partition {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            let position = match position {
                SeekFrom::Start(offset) => SeekFrom::Start(self.start + offset),
                other => other,
            };
            Ok(self.file.seek(position)? - self.start)
        }
    }

    #[test]
    fn another_fat_implementation_reads_the_volume_and_writes_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.img");
        let layout = create_blank(&path, 300, "My Card", &[], &mut Vec::new()).unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut partition = Partition {
            file,
            start: layout.partition_start * 512,
        };
        partition.seek(SeekFrom::Start(0)).unwrap();
        let fs = fatfs::FileSystem::new(partition, fatfs::FsOptions::new()).unwrap();
        assert_eq!(fs.fat_type(), fatfs::FatType::Fat32);
        assert_eq!(fs.volume_label(), "MY CARD");
        let counts = fs.stats().unwrap();
        assert_eq!(counts.cluster_size(), 4096, "8 sectors from 261 MiB");
        assert_eq!(u64::from(counts.total_clusters()), layout.clusters);
        assert_eq!(u64::from(counts.free_clusters()), layout.clusters - 1);
        let root = fs.root_dir();
        assert_eq!(root.iter().count(), 0, "an empty root directory");
        let directory = root.create_dir(".crosspoint").unwrap();
        let mut json = directory.create_file("state.json").unwrap();
        json.write_all(br#"{"showBootScreen":true}"#).unwrap();
        drop(json);
        let mut back = Vec::new();
        root.open_file(".crosspoint/state.json")
            .unwrap()
            .read_to_end(&mut back)
            .unwrap();
        assert_eq!(back, br#"{"showBootScreen":true}"#);
    }
}
