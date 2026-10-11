//! Deterministic event queue: ordered by cycle, then by insertion order.
use crate::time::Cycles;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeSet, BinaryHeap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(u64);

/// How many more cancelled entries than live ones the queue tolerates before it removes them.
const COMPACTION_SLACK: usize = 32;

#[derive(Debug)]
struct Entry<E> {
    at: Cycles,
    id: EventId,
    event: E,
}

impl<E> Entry<E> {
    fn key(&self) -> (Cycles, EventId) {
        (self.at, self.id)
    }
}

impl<E> PartialEq for Entry<E> {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl<E> Eq for Entry<E> {}

impl<E> PartialOrd for Entry<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<E> Ord for Entry<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

/// A deterministic queue of future events.
///
/// Events fire in cycle order; events due at the same cycle fire in the order they were scheduled.
/// Cancellation is lazy: a cancelled entry stays in the heap until it reaches the head, or until
/// a cancel finds the cancelled entries outnumber the live ones (scheduled, not yet fired, not
/// cancelled) by more than `COMPACTION_SLACK`, when the heap is rebuilt without them. So the
/// cancelled entries never number more than the live ones plus the slack, the heap never holds
/// more than twice the live entries plus the slack, and a cancel costs amortised O(log n): a
/// rebuild is linear and follows at least `live + slack` cancels. A cancelled entry whose
/// instant is far ahead does not stay until then.
#[derive(Debug)]
pub struct Scheduler<E> {
    heap: BinaryHeap<Reverse<Entry<E>>>,
    /// Ids scheduled and neither fired nor cancelled yet.
    live: BTreeSet<EventId>,
    /// Ids of live-then-cancelled entries still physically present in `heap`.
    cancelled: BTreeSet<EventId>,
    next_id: u64,
}

impl<E> Default for Scheduler<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E> Scheduler<E> {
    #[must_use]
    pub fn new() -> Self {
        Scheduler {
            heap: BinaryHeap::new(),
            live: BTreeSet::new(),
            cancelled: BTreeSet::new(),
            next_id: 0,
        }
    }

    /// Schedules `event` to fire at cycle `at` and returns its id for [`Scheduler::cancel`].
    pub fn schedule(&mut self, at: Cycles, event: E) -> EventId {
        let id = EventId(self.next_id);
        self.next_id += 1;
        self.live.insert(id);
        self.heap.push(Reverse(Entry { at, id, event }));
        id
    }

    /// Cancels a pending event so it never fires.
    ///
    /// Cancelling an id that has already fired, was already cancelled, or was never returned by
    /// [`Scheduler::schedule`] is a no-op: nothing is recorded, so no memory is retained.
    pub fn cancel(&mut self, id: EventId) {
        if self.live.remove(&id) {
            self.cancelled.insert(id);
            if self.cancelled.len() > self.live.len() + COMPACTION_SLACK {
                self.compact();
            }
        }
    }

    /// Rebuilds the heap without the cancelled entries. The order of the others is a total order
    /// on (cycle, id), which compaction does not touch.
    fn compact(&mut self) {
        let cancelled = &self.cancelled;
        self.heap.retain(|Reverse(e)| !cancelled.contains(&e.id));
        self.cancelled.clear();
    }

    /// Removes cancelled entries sitting at the head of the heap.
    fn drop_cancelled_head(&mut self) {
        while let Some(Reverse(head)) = self.heap.peek() {
            if !self.cancelled.remove(&head.id) {
                break;
            }
            self.heap.pop();
        }
    }

    /// Cycle of the earliest pending (not cancelled) event, or `None` when nothing is pending.
    #[must_use]
    pub fn next_due(&self) -> Option<Cycles> {
        self.heap
            .iter()
            .filter(|Reverse(e)| !self.cancelled.contains(&e.id))
            .map(|Reverse(e)| e.at)
            .min()
    }

    /// Pops the earliest pending event whose cycle is `<= now`, with its cycle.
    ///
    /// Returns `None` when the earliest pending event is still in the future or nothing is pending.
    pub fn pop_due(&mut self, now: Cycles) -> Option<(Cycles, E)> {
        self.pop_due_where(now, |_| true)
    }

    /// The id the next scheduled event will get: every event scheduled from now on has an id at
    /// or past it, every event scheduled before it, an id below. Meant for
    /// [`Scheduler::pop_due_before`].
    #[must_use]
    pub fn watermark(&self) -> EventId {
        EventId(self.next_id)
    }

    /// Pops the earliest pending event whose cycle is `<= now` and whose id is below `watermark`
    /// (see [`Scheduler::watermark`]): an event scheduled after the watermark is not returned,
    /// even if it is due and the earliest.
    ///
    /// The heap is ordered by (cycle, id), so this stops at the first due event at or past the
    /// watermark: it returns `None` while older due events may still sit behind it. That loses
    /// nothing when no event is scheduled for a cycle before the earliest due one, as long as
    /// the caller schedules nothing earlier than `now` (an event scheduled for `now` or later
    /// has a larger cycle, or the same cycle and a larger id, than every event due at `now`,
    /// and sorts after them). The number of events returned for one watermark is bounded by the
    /// number of events pending when it was taken.
    pub fn pop_due_before(&mut self, now: Cycles, watermark: EventId) -> Option<(Cycles, E)> {
        self.pop_due_where(now, |id| id < watermark)
    }

    fn pop_due_where(
        &mut self,
        now: Cycles,
        accepts: impl Fn(EventId) -> bool,
    ) -> Option<(Cycles, E)> {
        self.drop_cancelled_head();
        match self.heap.peek() {
            Some(Reverse(head)) if head.at <= now && accepts(head.id) => {
                self.heap.pop().map(|Reverse(e)| {
                    self.live.remove(&e.id);
                    (e.at, e.event)
                })
            }
            _ => None,
        }
    }

    /// Number of entries the queue physically holds: the pending ones and the cancelled ones not
    /// removed yet. A measure of memory, for the callers that must show it stays bounded.
    #[must_use]
    pub fn stored_entries(&self) -> usize {
        self.heap.len()
    }

    /// Number of cancelled entries still awaiting removal from the heap.
    #[cfg(test)]
    fn tombstones(&self) -> usize {
        self.cancelled.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pops_in_cycle_order() {
        let mut s = Scheduler::new();
        s.schedule(Cycles(30), 'c');
        s.schedule(Cycles(10), 'a');
        s.schedule(Cycles(20), 'b');
        assert_eq!(s.next_due(), Some(Cycles(10)));
        let order: Vec<char> =
            std::iter::from_fn(|| s.pop_due(Cycles(100)).map(|(_, e)| e)).collect();
        assert_eq!(order, vec!['a', 'b', 'c']);
    }
    #[test]
    fn same_cycle_events_fifo() {
        let mut s = Scheduler::new();
        for e in 0..100 {
            s.schedule(Cycles(5), e);
        }
        let order: Vec<i32> = std::iter::from_fn(|| s.pop_due(Cycles(5)).map(|(_, e)| e)).collect();
        assert_eq!(order, (0..100).collect::<Vec<_>>());
    }
    #[test]
    fn not_due_yet_returns_none() {
        let mut s = Scheduler::new();
        s.schedule(Cycles(50), ());
        assert!(s.pop_due(Cycles(49)).is_none());
        assert!(s.pop_due(Cycles(50)).is_some());
    }
    #[test]
    fn cancelled_event_never_fires() {
        let mut s = Scheduler::new();
        let id = s.schedule(Cycles(1), "x");
        s.schedule(Cycles(2), "y");
        s.cancel(id);
        assert_eq!(s.next_due(), Some(Cycles(2)));
        assert_eq!(s.pop_due(Cycles(10)).map(|(_, e)| e), Some("y"));
        assert!(s.pop_due(Cycles(10)).is_none());
    }
    #[test]
    fn cancelling_non_head_event_skips_it() {
        let mut s = Scheduler::new();
        s.schedule(Cycles(10), 'a');
        let b = s.schedule(Cycles(20), 'b');
        s.schedule(Cycles(30), 'c');
        s.cancel(b);
        assert_eq!(s.next_due(), Some(Cycles(10)));
        let order: Vec<char> =
            std::iter::from_fn(|| s.pop_due(Cycles(100)).map(|(_, e)| e)).collect();
        assert_eq!(order, vec!['a', 'c']);
        assert_eq!(s.tombstones(), 0);
    }
    #[test]
    fn next_due_after_cancelling_head_is_next_live_event() {
        let mut s = Scheduler::new();
        let x = s.schedule(Cycles(1), 'x');
        s.schedule(Cycles(2), 'y');
        s.schedule(Cycles(3), 'z');
        s.cancel(x);
        assert_eq!(s.next_due(), Some(Cycles(2)));
    }
    #[test]
    fn cancel_after_fire_and_twice_are_noops() {
        let mut s = Scheduler::new();
        let fired = s.schedule(Cycles(1), 'a');
        let pending = s.schedule(Cycles(2), 'b');
        assert_eq!(s.pop_due(Cycles(1)).map(|(_, e)| e), Some('a'));
        // Fired id: nothing is recorded.
        s.cancel(fired);
        assert_eq!(s.tombstones(), 0);
        // Live id cancelled twice: only the first call leaves a tombstone.
        s.cancel(pending);
        s.cancel(pending);
        assert_eq!(s.tombstones(), 1);
        // Draining the heap removes the tombstone and fires nothing.
        assert!(s.pop_due(Cycles(100)).is_none());
        assert_eq!(s.tombstones(), 0);
        assert_eq!(s.next_due(), None);
    }
    /// What the struct promises at every step: the heap holds at most twice the live entries plus
    /// the slack, and so do the tombstones.
    fn assert_bounded<E>(s: &Scheduler<E>) {
        assert!(s.tombstones() <= s.live.len() + COMPACTION_SLACK);
        assert!(
            s.stored_entries() <= 2 * s.live.len() + COMPACTION_SLACK,
            "{} entries for {} live",
            s.stored_entries(),
            s.live.len()
        );
    }

    #[test]
    fn scheduling_far_ahead_and_cancelling_forever_keeps_the_heap_bounded() {
        let mut s: Scheduler<u32> = Scheduler::new();
        for live in 0..3 {
            s.schedule(Cycles(100 + u64::from(live)), live);
        }
        for round in 0..10_000u64 {
            let id = s.schedule(Cycles(1_000_000_000 + round), 99);
            s.cancel(id);
            assert_bounded(&s);
        }
        assert_eq!(s.next_due(), Some(Cycles(100)));
        let order: Vec<u32> =
            std::iter::from_fn(|| s.pop_due(Cycles(u64::MAX)).map(|(_, e)| e)).collect();
        assert_eq!(order, vec![0, 1, 2], "the cancelled ones never fire");
    }

    #[test]
    fn compaction_keeps_the_live_events_in_cycle_then_scheduling_order() {
        let mut s: Scheduler<u32> = Scheduler::new();
        let mut expected: Vec<(u64, u32)> = Vec::new();
        for round in 0..10_000u32 {
            // A live event every 7th round, at an instant that repeats (so some share a cycle).
            if round % 7 == 0 {
                let at = 50 + u64::from(round % 5) * 10;
                s.schedule(Cycles(at), round);
                expected.push((at, round));
            }
            // A cancelled one, far ahead or early, at mixed instants.
            let at = if round % 2 == 0 {
                1_000_000 + u64::from(round)
            } else {
                55
            };
            let id = s.schedule(Cycles(at), u32::MAX);
            s.cancel(id);
            assert_bounded(&s);
        }
        // Stable on the cycle: equal cycles stay in the order they were scheduled.
        expected.sort_by_key(|&(at, _)| at);
        let order: Vec<(u64, u32)> =
            std::iter::from_fn(|| s.pop_due(Cycles(u64::MAX)).map(|(at, e)| (at.get(), e)))
                .collect();
        assert_eq!(order, expected);
        assert_eq!(s.tombstones(), 0);
    }

    #[test]
    fn cancelling_stays_a_noop_for_an_id_fired_or_already_cancelled_across_a_compaction() {
        let mut s: Scheduler<u32> = Scheduler::new();
        let fired = s.schedule(Cycles(1), 1);
        let early = s.schedule(Cycles(2_000_000), 2);
        s.cancel(early);
        assert_eq!(s.pop_due(Cycles(1)).map(|(_, e)| e), Some(1));
        let keep = s.schedule(Cycles(500), 3);
        // Enough cancels to compact, the heap rebuilt without `early`.
        for round in 0..(COMPACTION_SLACK as u64 * 3) {
            let id = s.schedule(Cycles(1_000_000 + round), 0);
            s.cancel(id);
        }
        assert!(s.tombstones() <= 1 + COMPACTION_SLACK);
        let tombstones = s.tombstones();
        // Cancelled before the compaction, cancelled again after it: nothing is recorded.
        s.cancel(early);
        s.cancel(fired);
        assert_eq!(
            s.tombstones(),
            tombstones,
            "no id enters the set twice or after firing"
        );
        assert_eq!(s.next_due(), Some(Cycles(500)));
        assert_eq!(s.pop_due(Cycles(u64::MAX)).map(|(_, e)| e), Some(3));
        assert!(s.pop_due(Cycles(u64::MAX)).is_none());
        s.cancel(keep);
        assert_eq!(s.tombstones(), 0);
    }

    #[test]
    fn cancelled_ids_never_accumulate_across_rearms() {
        let mut s: Scheduler<()> = Scheduler::new();
        for round in 0..1_000u64 {
            let id = s.schedule(Cycles(round + 1), ());
            s.cancel(id);
            assert!(s.pop_due(Cycles(round + 1)).is_none());
        }
        assert_eq!(s.tombstones(), 0);
    }
    #[test]
    fn the_watermark_is_the_id_the_next_event_gets() {
        let mut s = Scheduler::new();
        let before = s.watermark();
        let first = s.schedule(Cycles(1), 'a');
        let after = s.watermark();
        assert!(before <= first && first < after);
        assert_eq!(after, s.watermark(), "reading it schedules nothing");
        let second = s.schedule(Cycles(1), 'b');
        assert!(after <= second);
    }
    #[test]
    fn pop_due_before_returns_older_due_events_by_cycle_then_id() {
        let mut s = Scheduler::new();
        s.schedule(Cycles(20), 'c');
        s.schedule(Cycles(10), 'a');
        s.schedule(Cycles(10), 'b');
        s.schedule(Cycles(50), 'x');
        let watermark = s.watermark();
        let order: Vec<char> =
            std::iter::from_fn(|| s.pop_due_before(Cycles(30), watermark).map(|(_, e)| e))
                .collect();
        assert_eq!(order, vec!['a', 'b', 'c'], "'x' is not due");
    }
    #[test]
    fn pop_due_before_does_not_return_an_event_scheduled_after_the_watermark() {
        // Due and the earliest, but newer than the watermark: it is not returned, and it stops
        // the pass (the older event behind it is left for a caller that schedules nothing in the
        // past, which never has this case).
        let mut s = Scheduler::new();
        s.schedule(Cycles(10), 'o');
        let watermark = s.watermark();
        s.schedule(Cycles(5), 'n');
        assert_eq!(s.pop_due_before(Cycles(10), watermark), None);
        assert_eq!(
            s.pop_due(Cycles(10)).map(|(_, e)| e),
            Some('n'),
            "it is due"
        );
        // Scheduled for the same cycle as an older event, it sorts after it.
        let mut s = Scheduler::new();
        s.schedule(Cycles(10), 'o');
        let watermark = s.watermark();
        s.schedule(Cycles(10), 'n');
        assert_eq!(
            s.pop_due_before(Cycles(10), watermark).map(|(_, e)| e),
            Some('o')
        );
        assert_eq!(s.pop_due_before(Cycles(10), watermark), None);
        assert_eq!(
            s.pop_due(Cycles(10)).map(|(_, e)| e),
            Some('n'),
            "left for the next pass"
        );
    }
    #[test]
    fn pop_due_before_skips_a_cancelled_head() {
        let mut s = Scheduler::new();
        let gone = s.schedule(Cycles(1), 'g');
        s.schedule(Cycles(2), 'k');
        let watermark = s.watermark();
        s.cancel(gone);
        assert_eq!(
            s.pop_due_before(Cycles(9), watermark).map(|(_, e)| e),
            Some('k')
        );
        assert_eq!(s.pop_due_before(Cycles(9), watermark), None);
    }
}
