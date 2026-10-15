//! The board around the pads, beyond its chips: the inputs of its user, which the machine dates,
//! journals and applies, and its supply (ADR 0013).
//!
//! **When an input acts.** An input dated `at` acts at the first instant of the time base after
//! `at`, as a level change on a pin reaches the chip through its input synchroniser. A front end
//! that presses a key at the present instant of a stopped machine, and a replay that hands every
//! input over at power-on, then give the same run: in both, the key acts at one cycle past its
//! date, once the instruction under way at that date has ended, never during it.
use std::collections::VecDeque;

use emu_core::{BoardInput, BoardInputError, BoardInputs, Cycles, Nanos, Supply};

use crate::clock::TIME_BASE;

/// The most inputs of the board that may wait for their instant: as many as a session holds
/// (`MAX_JOURNAL_BOARD_INPUTS`), so that a replay hands them all over at power-on.
pub const MAX_PENDING_BOARD_INPUTS: usize = 16_384;

/// Why the board does not take an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoardRefusal {
    /// No board inputs were attached.
    NoBoard,
    /// The board refused it.
    Board(BoardInputError),
    /// [`MAX_PENDING_BOARD_INPUTS`] inputs are waiting.
    TooMany,
}

/// The inputs and the supply of the board.
#[derive(Debug, Default)]
pub(super) struct Board {
    inputs: Option<Box<dyn BoardInputs>>,
    supply: Option<Box<dyn Supply>>,
    /// The inputs still to act, with the instant each acts at, in that order; inputs that act at
    /// one instant keep the order they were given in.
    pending: VecDeque<(Cycles, BoardInput)>,
    /// The instant the supply failed, once it has.
    power_lost: Option<Cycles>,
}

impl Board {
    pub(super) fn attach_inputs(&mut self, inputs: Box<dyn BoardInputs>) {
        self.inputs = Some(inputs);
    }

    pub(super) fn attach_supply(&mut self, supply: Box<dyn Supply>) {
        self.supply = Some(supply);
    }

    /// Takes `input`, dated `at`: it will act at the next instant of the time base.
    pub(super) fn queue(&mut self, at: Cycles, input: BoardInput) -> Result<(), BoardRefusal> {
        let board = self.inputs.as_ref().ok_or(BoardRefusal::NoBoard)?;
        board.check(input).map_err(BoardRefusal::Board)?;
        if self.pending.len() >= MAX_PENDING_BOARD_INPUTS {
            return Err(BoardRefusal::TooMany);
        }
        let acts = Cycles(at.get().saturating_add(1));
        let place = self
            .pending
            .partition_point(|(instant, _)| *instant <= acts);
        self.pending.insert(place, (acts, input));
        Ok(())
    }

    /// The inputs waiting for their instant.
    pub(super) fn pending(&self) -> usize {
        self.pending.len()
    }

    /// The instant the next input acts at.
    pub(super) fn next_due(&self) -> Option<Cycles> {
        self.pending.front().map(|(acts, _)| *acts)
    }

    /// Applies, in order, the inputs whose instant has come by `now`, asking the supply after each
    /// at its instant: a key let go that cuts the power cuts it there.
    pub(super) fn apply_due(&mut self, now: Cycles) {
        while let Some(&(acts, input)) = self.pending.front()
            && acts <= now
        {
            self.pending.pop_front();
            if let Some(board) = &mut self.inputs {
                board.apply(input, Nanos(TIME_BASE.cycles_to_nanos(acts)));
            }
            self.check_supply(acts);
        }
    }

    /// Asks the supply whether the chip still has power at `now`, once it has not stopped asking.
    pub(super) fn check_supply(&mut self, now: Cycles) {
        if self.power_lost.is_some() {
            return;
        }
        if let Some(supply) = &mut self.supply
            && !supply.powered(Nanos(TIME_BASE.cycles_to_nanos(now)))
        {
            self.power_lost = Some(now);
        }
    }

    /// The instant the supply failed, once it has.
    pub(super) const fn power_lost(&self) -> Option<Cycles> {
        self.power_lost
    }
}
