// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Deterministic cooperative scheduler state used before host-thread integration.
use std::collections::VecDeque;
#[derive(Debug, Default)]
pub struct Scheduler {
    ready: VecDeque<(u64, i32)>,
}
impl Scheduler {
    pub fn enqueue(&mut self, id: u64) {
        self.enqueue_with_priority(id, 44);
    }
    pub fn enqueue_with_priority(&mut self, id: u64, priority: i32) {
        if let Some(index) = self.ready.iter().position(|entry| entry.0 == id) {
            if self.ready[index].1 == priority {
                return;
            }
            self.ready.remove(index);
        }
        self.ready.push_back((id, priority));
    }
    /// Reorder only queued threads; a policy change must not wake a waiting thread.
    pub fn reprioritize(&mut self, id: u64, priority: i32) {
        if self.ready.iter().any(|entry| entry.0 == id) {
            self.enqueue_with_priority(id, priority);
        }
    }
    /// Select within a runner's scope without consuming another process's queue.
    pub fn next_matching(&mut self, mut eligible: impl FnMut(u64) -> bool) -> Option<u64> {
        // Lower numeric priorities win; equal priorities retain FIFO order.
        let index = self
            .ready
            .iter()
            .enumerate()
            .filter(|(_, entry)| eligible(entry.0))
            .min_by_key(|(_, entry)| entry.1)?
            .0;
        self.ready.remove(index).map(|entry| entry.0)
    }
    pub fn len(&self) -> usize {
        self.ready.len()
    }
    pub fn remove(&mut self, id: u64) {
        self.ready.retain(|entry| entry.0 != id);
    }
    pub fn is_empty(&self) -> bool {
        self.ready.is_empty()
    }
}

impl Iterator for Scheduler {
    type Item = u64;

    /// Consume the highest-priority ready thread. Enqueueing can make an empty queue ready again.
    fn next(&mut self) -> Option<Self::Item> {
        self.next_matching(|_| true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn priority_selection_retains_fifo_ties_and_updates_only_queued_threads() {
        let mut scheduler = Scheduler::default();
        scheduler.enqueue_with_priority(1, 44);
        scheduler.enqueue_with_priority(2, 24);
        scheduler.enqueue_with_priority(3, 24);
        scheduler.enqueue_with_priority(2, 24);
        scheduler.reprioritize(99, 4);
        assert_eq!(scheduler.len(), 3);
        assert_eq!(scheduler.next(), Some(2));
        scheduler.reprioritize(1, 20);
        assert_eq!(scheduler.next(), Some(1));
        assert_eq!(scheduler.next(), Some(3));
        assert_eq!(scheduler.next(), None);
        assert!(scheduler.is_empty());
    }
}
