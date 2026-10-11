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

#[derive(Debug)]
pub struct Scheduler<E> {
    heap: BinaryHeap<Reverse<Entry<E>>>,
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
            cancelled: BTreeSet::new(),
            next_id: 0,
        }
    }

    pub fn schedule(&mut self, at: Cycles, event: E) -> EventId {
        let id = EventId(self.next_id);
        self.next_id += 1;
        self.heap.push(Reverse(Entry { at, id, event }));
        id
    }

    pub fn cancel(&mut self, id: EventId) {
        self.cancelled.insert(id);
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

    #[must_use]
    pub fn next_due(&self) -> Option<Cycles> {
        self.heap
            .iter()
            .filter(|Reverse(e)| !self.cancelled.contains(&e.id))
            .map(|Reverse(e)| e.at)
            .min()
    }

    pub fn pop_due(&mut self, now: Cycles) -> Option<(Cycles, E)> {
        self.drop_cancelled_head();
        match self.heap.peek() {
            Some(Reverse(head)) if head.at <= now => {
                self.heap.pop().map(|Reverse(e)| (e.at, e.event))
            }
            _ => None,
        }
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
}
