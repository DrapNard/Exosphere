// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! GPU semaphore/syncpoint model shared by engine frontends and host backends.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemaphoreCompare {
    Equal,
    NotEqual,
    GreaterOrEqual,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemaphoreOperation {
    Signal {
        address: u64,
        payload: u32,
    },
    Wait {
        address: u64,
        payload: u32,
        compare: SemaphoreCompare,
    },
    SyncpointIncrement {
        id: u32,
        amount: u32,
    },
    SyncpointWait {
        id: u32,
        threshold: u32,
    },
}

#[derive(Debug, Clone, Default)]
pub struct SyncTimeline {
    syncpoints: BTreeMap<u32, u32>,
    semaphores: BTreeMap<u64, u32>,
}

impl SyncTimeline {
    pub fn apply(&mut self, op: SemaphoreOperation) -> bool {
        match op {
            SemaphoreOperation::Signal { address, payload } => {
                self.semaphores.insert(address, payload);
                true
            }
            SemaphoreOperation::Wait {
                address,
                payload,
                compare,
            } => {
                let Some(current) = self.semaphores.get(&address).copied() else {
                    // An unknown address is not an observed zero-valued
                    // semaphore. In particular, waiting for zero must not pass.
                    return false;
                };
                match compare {
                    SemaphoreCompare::Equal => current == payload,
                    SemaphoreCompare::NotEqual => current != payload,
                    SemaphoreCompare::GreaterOrEqual => current >= payload,
                    SemaphoreCompare::Unknown(_) => false,
                }
            }
            SemaphoreOperation::SyncpointIncrement { id, amount } => {
                let value = self.syncpoints.entry(id).or_default();
                *value = value.wrapping_add(amount);
                true
            }
            SemaphoreOperation::SyncpointWait { id, threshold } => self
                .syncpoints
                .get(&id)
                .is_some_and(|current| *current >= threshold),
        }
    }

    pub fn syncpoint(&self, id: u32) -> Option<u32> {
        self.syncpoints.get(&id).copied()
    }
    pub fn semaphore(&self, address: u64) -> Option<u32> {
        self.semaphores.get(&address).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_synchronization_state_never_completes_a_wait() {
        let mut timeline = SyncTimeline::default();
        for compare in [
            SemaphoreCompare::Equal,
            SemaphoreCompare::NotEqual,
            SemaphoreCompare::GreaterOrEqual,
            SemaphoreCompare::Unknown(3),
        ] {
            assert!(!timeline.apply(SemaphoreOperation::Wait {
                address: 0x1000,
                payload: 0,
                compare
            }));
        }
        assert!(!timeline.apply(SemaphoreOperation::SyncpointWait {
            id: 1,
            threshold: 0
        }));
        assert_eq!(timeline.syncpoint(1), None);
        assert_eq!(timeline.semaphore(0x1000), None);
    }

    #[test]
    fn explicit_zero_signal_is_distinct_from_missing_state() {
        let mut timeline = SyncTimeline::default();
        assert!(timeline.apply(SemaphoreOperation::Signal {
            address: 0x1000,
            payload: 0
        }));
        assert!(timeline.apply(SemaphoreOperation::Wait {
            address: 0x1000,
            payload: 0,
            compare: SemaphoreCompare::Equal
        }));
        assert!(!timeline.apply(SemaphoreOperation::Wait {
            address: 0x1000,
            payload: 1,
            compare: SemaphoreCompare::GreaterOrEqual
        }));
        assert!(timeline.apply(SemaphoreOperation::SyncpointIncrement { id: 1, amount: 1 }));
        assert_eq!(timeline.syncpoint(1), Some(1));
        assert!(timeline.apply(SemaphoreOperation::SyncpointWait {
            id: 1,
            threshold: 1
        }));
        assert!(!timeline.apply(SemaphoreOperation::SyncpointWait {
            id: 2,
            threshold: 0
        }));
    }
}
