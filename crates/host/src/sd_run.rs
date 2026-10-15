//! The SD image of a run (design of sub-project 2e, §5 and §9; design of sub-project 2d; ADR
//! 0014): opened and locked before the run, its card put in the slot of the wired device for each
//! life, and what a life changed committed to the image when the life ends.
//!
//! The command line and the desktop application share this: the image is never read again once
//! a life has its source, a run never accepts a journal written for another image (that is
//! `inkemu sd recover --accept-journal`), and a life's changes reach the image only through
//! [`SdImage::commit`], whose journal keeps the image whole whatever interrupts it.
use crate::sd_image::{SdImage, SdImageError, files_of_the_sd_image};
use crate::session_file::Claimed;
use boards::{SdHandle, SdStart};
use chips::sdcard::{BlockSource, Changes, Overlay, SdCard, SdCardConfig};
use session::{SdCardError, SdCardStart};
use std::io::Write;
use std::path::Path;

/// The files of the SD image at `path` as claims: what a session, a flash file or a capture of
/// the same run must not be.
#[must_use]
pub fn claims(path: Option<&Path>) -> Vec<Claimed> {
    path.map(files_of_the_sd_image)
        .unwrap_or_default()
        .into_iter()
        .map(|(path, what)| Claimed::File {
            path,
            what: what.to_owned(),
        })
        .collect()
}

/// The card of one life over `image`: its content read from the image as the card asks, the
/// write protection of the image's access.
///
/// # Errors
/// Those of [`SdImage::source`].
pub fn card_of_a_life(image: &mut SdImage) -> Result<SdStart, SdImageError> {
    Ok(SdStart {
        source: Box::new(image.source()?),
        config: SdCardConfig {
            write_protected: image.write_protected(),
            ..SdCardConfig::default()
        },
    })
}

/// The card of `image` as a session records it (design of sub-project 2e, §7), read whole before
/// the run. A refusal says the size found and the bound, then the remedy.
///
/// # Errors
/// The message of the [`SdImageError`] or the [`SdCardError`].
pub fn recorded_card(image: &mut SdImage) -> Result<SdCardStart, String> {
    let mut source = image.source().map_err(|e| e.to_string())?;
    SdCardStart::read(&mut source, image.write_protected()).map_err(|e| match e {
        SdCardError::TooFull { .. } => format!("the run is not recorded: {e}"),
        other => other.to_string(),
    })
}

/// What a life left on its card, taken out of the slot when the life ends.
#[derive(Debug, Default)]
pub struct LifeCard {
    storage: Option<Overlay<Box<dyn BlockSource>>>,
}

impl LifeCard {
    /// Takes the card out of the slot of `handle`; an empty slot, or none, leaves nothing.
    #[must_use]
    pub fn take(handle: Option<&SdHandle>) -> LifeCard {
        LifeCard {
            storage: handle
                .and_then(SdHandle::take_card)
                .map(SdCard::into_storage),
        }
    }

    /// What the life changed on the card; `None` without a card.
    #[must_use]
    pub fn changes(&self) -> Option<Changes<'_>> {
        self.storage.as_ref().map(Overlay::changes)
    }
}

/// Commits what `card` holds of the life to `image` and says it in `log`: the summary line, the
/// notice of the commit when it has one, or the error. `false` when the changes are not kept.
pub fn commit_and_say(image: &mut SdImage, card: &LifeCard, log: &mut dyn Write) -> bool {
    let Some(changes) = card.changes() else {
        return true;
    };
    let capacity = image.capacity();
    let written = changes.written_count();
    match image.commit(&changes) {
        Ok(committed) => {
            let _ = writeln!(
                log,
                "sd card: {} ({} MiB, {}), {written} blocks written",
                image.shown(),
                capacity.bytes() >> 20,
                capacity.kind().name()
            );
            if let Some(notice) = committed.notice() {
                let _ = writeln!(log, "warning: {notice}");
            }
            true
        }
        Err(e) => {
            let _ = writeln!(log, "error: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd_image::Access;
    use chips::sdcard::{BLOCK_BYTES, BlockStorage as _};

    #[test]
    fn the_files_of_the_image_are_claims_and_none_without_an_image() {
        assert_eq!(claims(None), []);
        let claimed = claims(Some(Path::new("dir/card.img")));
        assert_eq!(claimed.len(), 3);
        assert!(
            matches!(&claimed[0], Claimed::File { path, .. } if path == Path::new("dir/card.img"))
        );
    }

    #[test]
    fn a_life_s_changes_are_committed_and_said_and_a_new_life_reads_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.img");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(64 << 20).unwrap();
        drop(file);
        let mut image = SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new()).unwrap();
        let start = card_of_a_life(&mut image).unwrap();
        assert!(!start.config.write_protected);
        let mut overlay = Overlay::new(start.source);
        overlay.write_block(3, &[0x5A; BLOCK_BYTES]).unwrap();
        let card = LifeCard {
            storage: Some(overlay),
        };
        let mut log = Vec::new();
        assert!(commit_and_say(&mut image, &card, &mut log));
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.starts_with("sd card: ") && log.ends_with("(64 MiB, SDSC), 1 blocks written\n"),
            "{log}"
        );
        let recorded = recorded_card(&mut image).unwrap();
        assert_eq!(recorded.ranges().len(), 1);
        assert_eq!(recorded.ranges()[0].first, 3);
        assert!(
            commit_and_say(&mut image, &LifeCard::default(), &mut Vec::new()),
            "no card"
        );
    }
}
