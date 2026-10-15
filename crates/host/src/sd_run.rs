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
use chips::sdcard::{
    Block, BlockSource, BlockStorage as _, Changes, Overlay, SdCard, SdCardConfig, StorageError,
};
use session::{SdCardError, SdCardStart};
use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

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
    card.changes()
        .is_none_or(|changes| commit_changes(image, &changes, log).is_ok())
}

/// One layer of changes over the content a card starts from.
type Layer = Overlay<Box<dyn BlockSource>>;

/// The changes the image has not taken, read by the life that runs over them and kept by the run:
/// one overlay, two holders on the thread of the run.
#[derive(Debug, Clone)]
struct Shared(Rc<RefCell<Layer>>);

impl BlockSource for Shared {
    fn block_count(&self) -> u64 {
        self.0.borrow().block_count()
    }

    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError> {
        self.0.borrow_mut().read_block(lba, into)
    }
}

/// The card of a run across its lives, as [`crate::device::FlashKeeper`] keeps its flash (R1):
/// what its lives wrote and its image could not take yet stays in memory, an overlay on the
/// image; the next life starts from it, and each end of a life and the power-off write it again.
/// A life never starts on a card that misses what the last one wrote, nor waits for the image.
#[derive(Debug, Default)]
pub struct CardKeeper {
    unwritten: Option<Shared>,
}

impl CardKeeper {
    /// Whether changes are waiting for the image.
    #[must_use]
    pub fn is_unwritten(&self) -> bool {
        self.unwritten.is_some()
    }

    /// The card of the next life over `image` ([`card_of_a_life`]), the changes waiting read
    /// over it: the life does not read the image again then, so a save of it that did not end
    /// does not refuse the life (the overlay holds every block that save would change).
    ///
    /// # Errors
    /// Those of [`SdImage::source`], when no change is waiting.
    pub fn card_of_a_life(&self, image: &mut SdImage) -> Result<SdStart, SdImageError> {
        let Some(unwritten) = &self.unwritten else {
            return card_of_a_life(image);
        };
        Ok(SdStart {
            source: Box::new(unwritten.clone()),
            config: SdCardConfig {
                write_protected: image.write_protected(),
                ..SdCardConfig::default()
            },
        })
    }

    /// The card a session records ([`recorded_card`]), with the changes waiting.
    ///
    /// # Errors
    /// Those of [`recorded_card`].
    pub fn recorded_card(&self, image: &mut SdImage) -> Result<SdCardStart, String> {
        let Some(unwritten) = &self.unwritten else {
            return recorded_card(image);
        };
        SdCardStart::read(&mut unwritten.clone(), image.write_protected()).map_err(|e| match e {
            SdCardError::TooFull { .. } => format!("the run is not recorded: {e}"),
            other => other.to_string(),
        })
    }

    /// Adds what a life left on its card to the changes waiting. The life ran over them
    /// ([`CardKeeper::card_of_a_life`]): its own changes go on top, its erases first, then its
    /// writes, which is the content the life read.
    pub fn take_in(&mut self, card: LifeCard) {
        let Some(storage) = card.storage else {
            return;
        };
        let Some(unwritten) = &self.unwritten else {
            self.unwritten = Some(Shared(Rc::new(RefCell::new(storage))));
            return;
        };
        let changes = storage.changes();
        let mut layer = unwritten.0.borrow_mut();
        // INVARIANT: the life's overlay has the block count of the one under it, which checked
        // every range and block: neither can be refused here.
        for (first, count) in changes.erased() {
            let _ = layer.erase(first, count);
        }
        for (lba, block) in changes.blocks() {
            let _ = layer.write_block(lba, block);
        }
    }

    /// Writes the changes waiting to `image`, and says it in `log` as [`commit_and_say`] does.
    /// Changes the image could not take stay waiting, with a line that says where they are and
    /// what frees them. `false` then.
    pub fn commit_and_say(&mut self, image: &mut SdImage, log: &mut dyn Write) -> bool {
        let Some(unwritten) = &self.unwritten else {
            return true;
        };
        let committed = commit_changes(image, &unwritten.0.borrow().changes(), log);
        match committed {
            Ok(()) => self.unwritten = None,
            // The image holds them: its error says so and what frees the journal. They stay in
            // memory all the same, so that the next life does not wait for that journal (N4).
            Err(SdImageError::JournalNotRemoved { .. }) => {}
            Err(_) => {
                let _ = writeln!(
                    log,
                    "error: what the device wrote to its SD card is not in {} yet: it is kept in memory, the next life of the device starts from it, and it is written again when a life ends and when the device is powered off; free some room on the disk of the image, or make its directory writable",
                    image.shown()
                );
            }
        }
        self.unwritten.is_none()
    }
}

/// [`commit_and_say`] of `changes`: the error said in `log` is given back.
fn commit_changes(
    image: &mut SdImage,
    changes: &Changes<'_>,
    log: &mut dyn Write,
) -> Result<(), SdImageError> {
    let capacity = image.capacity();
    let written = changes.written_count();
    match image.commit(changes) {
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
            Ok(())
        }
        Err(e) => {
            let _ = writeln!(log, "error: {e}");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd_image::Access;
    use chips::sdcard::BLOCK_BYTES;

    /// The first byte of block `lba` of the image at `path`.
    fn byte_of(path: &Path, lba: usize) -> u8 {
        std::fs::read(path).unwrap()[lba * BLOCK_BYTES]
    }

    /// N4: a save the image holds whole, of which only the journal could not be removed, is said
    /// as that, with its remedy, never as changes missing from the image; they stay in memory
    /// for the next life, which does not wait for the journal.
    #[test]
    fn a_save_whose_journal_alone_stays_is_said_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.img");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(64 << 20)
            .unwrap();
        let mut image = SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new()).unwrap();
        let mut keeper = CardKeeper::default();
        let mut life = Overlay::new(keeper.card_of_a_life(&mut image).unwrap().source);
        life.write_block(3, &[0x5A; BLOCK_BYTES]).unwrap();
        keeper.take_in(LifeCard {
            storage: Some(life),
        });
        image.leave_the_journal();
        let mut log = Vec::new();
        assert!(!keeper.commit_and_say(&mut image, &mut log));
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.starts_with("error: the SD image ")
                && log.contains("holds what the device wrote, but its journal ")
                && log.ends_with("make the directory of the image writable\n"),
            "{log}"
        );
        assert!(!log.contains("not in"), "{log}");
        assert_eq!(byte_of(&path, 3), 0x5A, "the image holds it");
        assert!(keeper.is_unwritten());
    }

    /// R1: a life's changes the image did not take are read by the next life, which builds on
    /// them (its erase undoes a block of the last life, its write adds one); one commit writes
    /// them all, and the keeper waits for nothing after it.
    #[test]
    fn a_keeper_lays_the_next_life_over_the_unwritten_changes_then_writes_them_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.img");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(64 << 20)
            .unwrap();
        let mut image = SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new()).unwrap();
        let mut keeper = CardKeeper::default();
        let mut first = Overlay::new(keeper.card_of_a_life(&mut image).unwrap().source);
        first.write_block(3, &[0x5A; BLOCK_BYTES]).unwrap();
        first.write_block(5, &[0x77; BLOCK_BYTES]).unwrap();
        // Its commit did not happen (it failed): the changes wait.
        keeper.take_in(LifeCard {
            storage: Some(first),
        });
        assert!(keeper.is_unwritten());
        let mut second = Overlay::new(keeper.card_of_a_life(&mut image).unwrap().source);
        let mut block = [0; BLOCK_BYTES];
        second.read_block(3, &mut block).unwrap();
        assert_eq!(
            block[0], 0x5A,
            "the next life reads what the last one wrote"
        );
        assert_eq!(byte_of(&path, 3), 0, "the image does not hold it yet");
        second.erase(5, 1).unwrap();
        second.write_block(4, &[0x6B; BLOCK_BYTES]).unwrap();
        keeper.take_in(LifeCard {
            storage: Some(second),
        });
        let recorded = keeper.recorded_card(&mut image).unwrap();
        assert_eq!(recorded.ranges().len(), 1, "blocks 3 and 4, block 5 erased");
        let mut log = Vec::new();
        assert!(keeper.commit_and_say(&mut image, &mut log));
        assert!(!keeper.is_unwritten());
        assert!(
            String::from_utf8(log)
                .unwrap()
                .ends_with("2 blocks written\n")
        );
        assert_eq!(
            [byte_of(&path, 3), byte_of(&path, 4), byte_of(&path, 5)],
            [0x5A, 0x6B, 0]
        );
    }

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
