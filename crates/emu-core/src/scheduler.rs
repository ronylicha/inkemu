//! Deterministic event queue: ordered by cycle, then by insertion order.
use crate::time::Cycles;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeSet, BinaryHeap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(u64);

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
/// Cancellation is lazy: a cancelled entry stays in the heap until it reaches the head, and only
/// live (scheduled, not yet fired, not cancelled) ids are tracked, so memory stays bounded by the
/// number of pending events.
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
        }
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
        self.drop_cancelled_head();
        match self.heap.peek() {
            Some(Reverse(head)) if head.at <= now => self.heap.pop().map(|Reverse(e)| {
                self.live.remove(&e.id);
                (e.at, e.event)
            }),
            _ => None,
        }
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
}
