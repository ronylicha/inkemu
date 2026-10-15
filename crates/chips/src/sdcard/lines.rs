//! The card slot on its lines: what a board attaches to the SPI bus of a `SoC` (design of
//! sub-project 2d, task 4; design of sub-project 2e, §9; ADR 0012).
//!
//! [`SdLines`] holds an [`SdSlot`] and implements [`SpiTarget`]: the `SoC` tells it when its chip
//! select changes and hands it the bytes of each transfer while it is selected; the card answers
//! on MISO, which arrives filled with what the line reads undriven (an empty slot leaves it so).
//! The card has no D/C line and does nothing on its own between two transfers: it is not
//! [`emu_core::Clocked`]. Its time base is 1 GHz, so that an instant of the signals (`Nanos`) is an
//! instant of the card without conversion, as for the SSD1677 (2d-D2).
use emu_core::{Cycles, Level, Nanos, SpiTarget};

use super::{BlockStorage, SdSlot};

/// A card slot seen from its pins.
#[derive(Debug)]
pub struct SdLines<S: BlockStorage> {
    slot: SdSlot<S>,
}

impl<S: BlockStorage> SdLines<S> {
    /// The lines of `slot`, a card in it or none.
    #[must_use]
    pub fn new(slot: SdSlot<S>) -> SdLines<S> {
        SdLines { slot }
    }

    /// The slot, for what a board reads of the card.
    #[must_use]
    pub fn slot(&self) -> &SdSlot<S> {
        &self.slot
    }

    /// The slot, for what a board takes from it (the card at the end of a life).
    pub fn slot_mut(&mut self) -> &mut SdSlot<S> {
        &mut self.slot
    }
}

impl<S: BlockStorage> SpiTarget for SdLines<S> {
    fn select(&mut self, selected: bool, at: Nanos) {
        self.slot.select(Cycles(at.get()), selected);
    }

    /// The card shifts a byte out for each byte shifted in; a line it leaves undriven keeps what
    /// `miso` came with. A D/C level, which a card has no line for, is ignored.
    fn transfer(&mut self, _dc: Option<Level>, mosi: &[u8], miso: &mut [u8], at: Nanos) {
        self.slot.transfer(Cycles(at.get()), mosi, miso);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdcard::{BLOCK_BYTES, MemoryStorage, SdCard, SdCardConfig, crc7_byte};
    use emu_core::Frequency;

    fn card() -> SdCard<MemoryStorage> {
        let base = Frequency::from_hz(1_000_000_000).unwrap();
        let storage = MemoryStorage::new(128);
        assert_eq!(storage.bytes().len(), 128 * BLOCK_BYTES);
        SdCard::new(SdCardConfig::default(), storage, base).unwrap()
    }

    /// CMD0 then eight clocks to read R1, as a host sends them.
    fn go_idle() -> Vec<u8> {
        let first = 0x40;
        let mut bytes = vec![first, 0, 0, 0, 0, crc7_byte(&[first, 0, 0, 0, 0])];
        bytes.extend([0xFF; 8]);
        bytes
    }

    #[test]
    fn a_card_reached_by_transfers_answers_as_it_does_byte_by_byte() {
        let mut lines = SdLines::new(SdSlot::holding(card()));
        lines.select(true, Nanos(10));
        let mosi = go_idle();
        let mut miso = vec![0xFF; mosi.len()];
        lines.transfer(None, &mosi, &mut miso, Nanos(10));
        let mut direct = card();
        direct.select(Cycles(10), true);
        let expected: Vec<u8> = mosi
            .iter()
            .map(|&byte| direct.exchange(Cycles(10), byte).unwrap_or(0xFF))
            .collect();
        assert_eq!(miso, expected);
        assert!(miso.contains(&0x01), "R1 idle: {miso:02x?}");
    }

    #[test]
    fn an_empty_slot_leaves_miso_as_it_came_and_a_bare_selection_does_nothing() {
        let mut lines: SdLines<MemoryStorage> = SdLines::new(SdSlot::empty());
        lines.select(true, Nanos(0));
        let mut miso = [0xFF; 14];
        lines.transfer(Some(Level::High), &go_idle(), &mut miso, Nanos(0));
        assert_eq!(miso, [0xFF; 14]);
        let mut held = SdLines::new(SdSlot::holding(card()));
        for at in 0..4 {
            held.select(true, Nanos(at));
            held.select(false, Nanos(at));
        }
        let diagnostics = held.slot_mut().card_mut().unwrap().take_diagnostics();
        assert_eq!(diagnostics, [], "a selection alone is no command");
        assert!(held.slot().card().is_some());
    }
}
