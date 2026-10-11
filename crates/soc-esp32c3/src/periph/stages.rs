//! The stage walk of a watchdog: what the main watchdogs (`TIMG0`, `TIMG1`) and the RTC watchdog
//! do alike once their counter has reached the timeout of the current stage. Sources: TRM
//! §12.2.2.2; report TW §B.3, §C.3, §D.5.
//!
//! A watchdog has four stages, each with a timeout and an action; the stages follow one another
//! and loop from the last to the first. What differs between the two kinds of watchdog stays in
//! the device: where a timeout comes from, what an action is, and when the counter runs.
use super::counter::Counter;
use emu_core::Cycles;

/// A watchdog has four stages (TRM §12.2.2.2).
pub(crate) const STAGES: usize = 4;

/// The most iterations one walk takes: four expiries, after which the count is reduced modulo a
/// turn of the four stages, at most three more expiries (the reduced count is below the sum of the
/// four timeouts, so the fourth stage from there cannot have expired), and the wait.
const WALK_STEPS: usize = 2 * STAGES;

/// Lets a watchdog go through the stages whose timeout its count has reached: calls
/// `expired(stage)` for each of them, in order, and returns the instant of the next expiry (`None`
/// when the counter is stopped). `stage` is the current stage and `counter` the counts since the
/// last feed or the start of that stage; both are left as they are at `now`.
/// `from_register_write` says that a write to a watchdog register called, not the passing of time.
///
/// The rules:
///
/// 1. A stage lasts at least one count: a timeout of 0 counts as 1 (each device marks that
///    assumption where it reads its timeout registers).
/// 2. A write that leaves the count at or past the timeout of the current stage expires that
///    stage at the write, and the next stage counts from 0 (TRM §12.2.2.2; TW §B.3: "the counter
///    returns to 0 and the next stage becomes current"): the first expiry of such a call carries
///    nothing.
/// 3. An expiry found because time passed (a wake-up, a clock change, a reset, the construction)
///    carries the counts elapsed since the expiry into the next stage (`count - timeout`). The
///    fraction of a count is lost at each transition: the counter is anchored at the present time
///    with a whole number of counts, so the loss is less than one count per transition, and
///    deterministic.
/// 4. The walk is bounded whatever firmware wrote: after four expiries in one call every stage
///    has acted once, so the remaining count is reduced modulo the sum of the four timeouts (whole
///    turns change neither the stage nor which actions fired), which leaves at most three more
///    expiries. One call costs at most [`WALK_STEPS`] iterations, with no trip count that depends
///    on a register value.
/// 5. The walk goes on after an expiry whose action asks for a reset: the reset does not stop the
///    watchdog, and the context keeps the deepest request, the first between equal levels
///    ([`crate::ctx::Ctx::request_reset`]).
///
/// ASSUMPTION: a stage expires when the counter is at or past its timeout (decision E27), not
/// only when it equals it. The hardware may
/// compare for equality, which would let a timeout lowered below the present count never expire
/// until the next feed; QEMU's model says so, no document does (TW §G row 7, §I question 5).
/// At-or-past makes such a watchdog fire, which is visible. Lowering a timeout below the count on
/// a chip, and seeing whether the stage fires, settles it.
///
/// ASSUMPTION: a stage whose action is "off" is waited for like the others and does nothing when
/// it ends: the walk does not look at the actions (QEMU's reading; TW §B.3, §I question 5). A
/// chip with stage 0 off and stage 1 resetting, timed, settles it.
pub(crate) fn walk(
    counter: &mut Counter,
    stage: &mut usize,
    timeouts: [u64; STAGES],
    now: Cycles,
    from_register_write: bool,
    mut expired: impl FnMut(usize),
) -> Option<Cycles> {
    let timeouts = timeouts.map(|timeout| timeout.max(1));
    // Four timeouts of at most 2^36 counts each (a 32-bit register, shifted by at most 4 for
    // stage 0 of the RTC watchdog): the sum fits; it saturates rather than wrap all the same.
    let turn = timeouts
        .iter()
        .fold(0u64, |sum, timeout| sum.saturating_add(*timeout));
    *stage %= STAGES;
    let mut count = counter.value(now);
    let mut carries = !from_register_write;
    let mut expiries = 0;
    for _ in 0..WALK_STEPS {
        let timeout = timeouts[*stage];
        if count < timeout {
            return counter.instant_in(timeout - count, now);
        }
        let ended = *stage;
        // The counter returns to 0 (or keeps the counts beyond the timeout, rule 3) and the next
        // stage becomes current; stages loop 0 to 3 (TRM §12.2.2.2).
        count = if carries { count - timeout } else { 0 };
        carries = true;
        *stage = (*stage + 1) % STAGES;
        expiries += 1;
        if expiries == STAGES {
            count %= turn;
        }
        counter.set(count, now);
        expired(ended);
    }
    // INVARIANT: unreachable. After the reduction the count is below the sum of the four
    // timeouts, so at most three more stages expire and the walk returns above. A wake-up one
    // count away keeps the watchdog alive if that were ever wrong.
    counter.instant_in(1, now)
}

#[cfg(test)]
mod tests {
    use super::super::counter::Rate;
    use super::*;

    /// One count every 10 cycles of the time base.
    const PERIOD: u64 = 10;

    /// A running counter that reads `count` at the present time, `NOW`.
    fn counter(count: u64) -> Counter {
        Counter::new(count, NOW, Rate::every(PERIOD), true)
    }
    const NOW: Cycles = Cycles(1_000);

    /// What one walk did: the stages that expired in order, the stage and the count it left, and
    /// the counts until the next expiry.
    #[derive(Debug, PartialEq, Eq)]
    struct Walked {
        expired: Vec<usize>,
        stage: usize,
        count: u64,
        counts_to_next: u64,
    }

    fn walked(
        count: u64,
        mut stage: usize,
        timeouts: [u64; STAGES],
        from_register_write: bool,
    ) -> Walked {
        let mut counter = counter(count);
        let mut expired = Vec::new();
        let next = walk(
            &mut counter,
            &mut stage,
            timeouts,
            NOW,
            from_register_write,
            |ended| expired.push(ended),
        )
        .expect("the counter runs");
        Walked {
            expired,
            stage,
            count: counter.value(NOW),
            counts_to_next: (next.get() - NOW.get()) / PERIOD,
        }
    }

    #[test]
    fn a_count_below_the_timeout_waits_for_what_is_left() {
        assert_eq!(
            walked(3, 1, [5, 9, 5, 5], false),
            Walked {
                expired: vec![],
                stage: 1,
                count: 3,
                counts_to_next: 6,
            }
        );
    }

    #[test]
    fn an_expiry_late_by_time_carries_the_counts_past_the_timeout_into_the_next_stage() {
        // Rule 3: the wake-up came 3 counts after stage 1 ended; stage 2 has counted them.
        assert_eq!(
            walked(12, 1, [5, 9, 7, 5], false),
            Walked {
                expired: vec![1],
                stage: 2,
                count: 3,
                counts_to_next: 4,
            }
        );
    }

    #[test]
    fn a_timeout_lowered_strictly_below_the_count_expires_at_the_write_and_carries_nothing() {
        // Rule 2: the count is 10, the write lowers the timeout of stage 2 to 4. The stage ends
        // at the write and stage 3 counts from 0, not from the 6 counts "past" the new timeout.
        assert_eq!(
            walked(10, 2, [5, 5, 4, 8], true),
            Walked {
                expired: vec![2],
                stage: 3,
                count: 0,
                counts_to_next: 8,
            }
        );
        // The same count met by the passing of time carries: 10 - 4 = 6 counts into stage 3.
        assert_eq!(
            walked(10, 2, [5, 5, 4, 8], false),
            Walked {
                expired: vec![2],
                stage: 3,
                count: 6,
                counts_to_next: 2,
            }
        );
    }

    #[test]
    fn a_write_that_lands_exactly_on_the_timeout_expires_the_stage_too() {
        assert_eq!(
            walked(4, 0, [4, 8, 5, 5], true),
            Walked {
                expired: vec![0],
                stage: 1,
                count: 0,
                counts_to_next: 8,
            }
        );
    }

    #[test]
    fn four_zero_timeouts_are_four_stages_of_one_count_and_the_walk_ends() {
        // Rule 1 and rule 4: a count of a million with every timeout at 0 is 250 000 turns of
        // four one-count stages. Each stage acts once, the whole turns are folded away, and the
        // walk waits for the one count of the stage it is left in.
        assert_eq!(
            walked(1_000_000, 0, [0; STAGES], false),
            Walked {
                expired: vec![0, 1, 2, 3],
                stage: 0,
                count: 0,
                counts_to_next: 1,
            }
        );
        // The same from a write: the first expiry carries nothing, so nothing is left to fold.
        assert_eq!(
            walked(1_000_000, 0, [0; STAGES], true),
            Walked {
                expired: vec![0],
                stage: 1,
                count: 0,
                counts_to_next: 1,
            }
        );
    }

    #[test]
    fn the_fold_from_another_stage_than_the_first_keeps_the_stage_and_the_order_of_the_actions() {
        // Unequal timeouts, a turn of 3 + 5 + 7 + 11 = 26 counts, the walk starting in stage 2.
        // A count of 7 + 11 + 3 + 5 (one turn from stage 2) + 2 * 26 (two more) + 7 + 11 + 2:
        // the four stages act once in the order 2, 3, 0, 1; the fold drops the two whole turns
        // (and the walk is back in stage 2); then stages 2 and 3 expire again and stage 0 has
        // counted 2 of its 3.
        let timeouts = [3, 5, 7, 11];
        let count = 26 + 2 * 26 + 7 + 11 + 2;
        assert_eq!(
            walked(count, 2, timeouts, false),
            Walked {
                expired: vec![2, 3, 0, 1, 2, 3],
                stage: 0,
                count: 2,
                counts_to_next: 1,
            }
        );
        // The most a walk does: the count is one short of two turns past the first, so three
        // stages expire after the fold and the fourth is one count from its end.
        assert_eq!(
            walked(26 + 26 - 1, 2, timeouts, false),
            Walked {
                expired: vec![2, 3, 0, 1, 2, 3, 0],
                stage: 1,
                count: 4,
                counts_to_next: 1,
            }
        );
    }

    /// The walk one count at a time, without the fold: what rule 4 must not change.
    fn reference(
        mut count: u64,
        mut stage: usize,
        timeouts: [u64; STAGES],
        from_register_write: bool,
    ) -> (Vec<usize>, usize, u64) {
        let timeouts = timeouts.map(|timeout| timeout.max(1));
        let mut expired = Vec::new();
        let mut carries = !from_register_write;
        while count >= timeouts[stage] {
            count = if carries { count - timeouts[stage] } else { 0 };
            carries = true;
            if !expired.contains(&stage) {
                expired.push(stage);
            }
            stage = (stage + 1) % STAGES;
        }
        (expired, stage, count)
    }

    #[test]
    fn the_fold_changes_neither_the_stage_nor_the_count_nor_which_stages_acted() {
        // Every start stage, a spread of timeouts (0 included), every count up to four turns and
        // more, by a write and by time: the bounded walk ends where the count-by-count walk ends,
        // with the same stages having acted, and never through its fallback (which would leave
        // the count at or past the timeout of the stage).
        let values = [0, 1, 2, 5];
        for start in 0..STAGES {
            for a in values {
                for b in values {
                    for c in values {
                        for d in values {
                            let timeouts = [a, b, c, d];
                            for count in 0..=60 {
                                for write in [false, true] {
                                    let got = walked(count, start, timeouts, write);
                                    let (acted, stage, left) =
                                        reference(count, start, timeouts, write);
                                    let label = format!(
                                        "start {start}, timeouts {timeouts:?}, count {count}, write {write}"
                                    );
                                    assert_eq!((got.stage, got.count), (stage, left), "{label}");
                                    let limit = timeouts[stage].max(1);
                                    assert!(got.count < limit, "{label}");
                                    assert_eq!(got.counts_to_next, limit - got.count, "{label}");
                                    let mut seen = Vec::new();
                                    for ended in &got.expired {
                                        if !seen.contains(ended) {
                                            seen.push(*ended);
                                        }
                                    }
                                    assert_eq!(seen, acted, "{label}");
                                    assert!(got.expired.len() < WALK_STEPS, "{label}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_stopped_counter_has_no_next_expiry_and_a_stage_out_of_range_is_brought_back() {
        let mut stopped = Counter::new(2, NOW, Rate::every(PERIOD), false);
        let mut stage = 6;
        let next = walk(&mut stopped, &mut stage, [5; STAGES], NOW, false, |_| {
            panic!("nothing expires")
        });
        assert_eq!((next, stage), (None, 2));
    }

    #[test]
    fn timeouts_at_the_top_of_their_range_do_not_overflow_the_turn() {
        // Four timeouts of `u64::MAX`: the sum saturates and the fold still leaves a count below
        // every timeout.
        let got = walked(u64::MAX - 1, 0, [u64::MAX; STAGES], false);
        assert_eq!((got.expired, got.stage), (vec![], 0));
        let got = walked(u64::MAX, 3, [u64::MAX; STAGES], false);
        assert_eq!((got.expired, got.stage, got.count), (vec![3], 0, 0));
    }
}
