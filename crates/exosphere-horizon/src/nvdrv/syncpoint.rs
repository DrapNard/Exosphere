// SPDX-License-Identifier: GPL-3.0-or-later
//! Channel-owned software syncpoints. Guest transport and wait thresholds are
//! deliberately separate: reserving a fence never implies GPU completion.

use super::{NvGpuChannelIdentity, submission::SubmissionFence};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyncpointError {
    #[error("invalid software syncpoint capacity")]
    Capacity,
    #[error("syncpoint identities exhausted")]
    Exhausted,
    #[error("channel already owns a syncpoint")]
    AlreadyAllocated,
    #[error("syncpoint is missing or owned by another channel")]
    Owner,
    #[error("syncpoint reservation budget exhausted")]
    Full,
    #[error("syncpoint completion is not the oldest pending reservation")]
    CompletionOrder,
    #[error("syncpoint cancellation is not the newest pending reservation")]
    CancellationOrder,
}

/// Unforgeable reservation identity within a manager, separate from the public
/// wrapping fence counter. Consumers must retain it until actual work retires.
#[derive(Debug, Clone)]
pub struct SyncpointReservation {
    fence: SubmissionFence,
    serial: u64,
    manager: Arc<()>,
}

impl PartialEq for SyncpointReservation {
    fn eq(&self, other: &Self) -> bool {
        self.fence == other.fence
            && self.serial == other.serial
            && Arc::ptr_eq(&self.manager, &other.manager)
    }
}
impl Eq for SyncpointReservation {}

impl SyncpointReservation {
    pub fn fence(&self) -> SubmissionFence {
        self.fence
    }
}

#[derive(Debug)]
struct Syncpoint {
    owner: NvGpuChannelIdentity,
    reserved: u32,
    completed: u32,
    next_serial: u64,
    pending: VecDeque<SyncpointReservation>,
}

#[derive(Debug)]
pub struct NvGpuSyncpoints {
    identity: Arc<()>,
    points: BTreeMap<u32, Syncpoint>,
    next_id: u32,
    capacity: usize,
}

impl NvGpuSyncpoints {
    /// Capacity is an emulator resource policy, not a reported hardware limit.
    pub fn new(capacity: usize) -> Result<Self, SyncpointError> {
        if capacity == 0 || capacity > 65_536 {
            return Err(SyncpointError::Capacity);
        }
        Ok(Self {
            identity: Arc::new(()),
            points: BTreeMap::new(),
            next_id: 1,
            capacity,
        })
    }

    pub fn allocate(
        &mut self,
        owner: NvGpuChannelIdentity,
    ) -> Result<SubmissionFence, SyncpointError> {
        if self.points.values().any(|point| point.owner == owner) {
            return Err(SyncpointError::AlreadyAllocated);
        }
        if self.points.len() >= self.capacity {
            return Err(SyncpointError::Full);
        }
        let id = self.next_id;
        let next_id = id.checked_add(1).ok_or(SyncpointError::Exhausted)?;
        self.points.insert(
            id,
            Syncpoint {
                owner,
                reserved: 0,
                completed: 0,
                next_serial: 1,
                pending: VecDeque::new(),
            },
        );
        self.next_id = next_id;
        Ok(SubmissionFence { id, value: 0 })
    }

    fn owned(
        &mut self,
        owner: &NvGpuChannelIdentity,
        id: u32,
    ) -> Result<&mut Syncpoint, SyncpointError> {
        self.points
            .get_mut(&id)
            .filter(|point| point.owner == *owner)
            .ok_or(SyncpointError::Owner)
    }

    pub fn completed(
        &self,
        owner: &NvGpuChannelIdentity,
        id: u32,
    ) -> Result<SubmissionFence, SyncpointError> {
        let point = self
            .points
            .get(&id)
            .filter(|point| point.owner == *owner)
            .ok_or(SyncpointError::Owner)?;
        Ok(SubmissionFence {
            id,
            value: point.completed,
        })
    }

    pub fn reserve(
        &mut self,
        owner: &NvGpuChannelIdentity,
        id: u32,
        increments: u32,
    ) -> Result<SyncpointReservation, SyncpointError> {
        let manager = self.identity.clone();
        let point = self.owned(owner, id)?;
        if point.pending.len() >= 4096 {
            return Err(SyncpointError::Full);
        }
        let next_serial = point
            .next_serial
            .checked_add(1)
            .ok_or(SyncpointError::Exhausted)?;
        point
            .pending
            .try_reserve(1)
            .map_err(|_| SyncpointError::Full)?;
        let reservation = SyncpointReservation {
            fence: SubmissionFence {
                id,
                value: point.reserved.wrapping_add(increments),
            },
            serial: point.next_serial,
            manager,
        };
        point.pending.push_back(reservation.clone());
        point.reserved = reservation.fence.value;
        point.next_serial = next_serial;
        Ok(reservation)
    }

    /// Caller must confirm that this reservation's actual GPU work retired.
    /// No wait-threshold comparison or synthetic success is performed here.
    pub fn complete(
        &mut self,
        owner: &NvGpuChannelIdentity,
        reservation: &SyncpointReservation,
    ) -> Result<SubmissionFence, SyncpointError> {
        let point = self.owned(owner, reservation.fence.id)?;
        if point.pending.front() != Some(reservation) {
            return Err(SyncpointError::CompletionOrder);
        }
        point.pending.pop_front();
        point.completed = reservation.fence.value;
        Ok(reservation.fence)
    }

    /// Roll back admission before the corresponding work enters the GPU queue.
    /// Only the newest reservation can be removed without invalidating later
    /// fence values. Serial identities are never reused, even after rollback.
    pub fn cancel_latest(
        &mut self,
        owner: &NvGpuChannelIdentity,
        reservation: &SyncpointReservation,
    ) -> Result<(), SyncpointError> {
        let point = self.owned(owner, reservation.fence.id)?;
        if point.pending.back() != Some(reservation) {
            return Err(SyncpointError::CancellationOrder);
        }
        point.pending.pop_back();
        point.reserved = point
            .pending
            .back()
            .map_or(point.completed, |pending| pending.fence.value);
        Ok(())
    }

    pub fn release(&mut self, owner: &NvGpuChannelIdentity, id: u32) -> Result<(), SyncpointError> {
        self.owned(owner, id)?;
        // IDs are never recycled: a stale fence cannot attach to a new channel.
        self.points.remove(&id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn cancellation_restores_the_tail_without_reusing_a_stale_ticket() {
        let owner = NvGpuChannelIdentity {
            client: Arc::new(()),
            descriptor: 1,
        };
        let mut points = NvGpuSyncpoints::new(1).unwrap();
        let initial = points.allocate(owner.clone()).unwrap();
        let first = points.reserve(&owner, initial.id, 3).unwrap();
        let second = points.reserve(&owner, initial.id, 5).unwrap();
        assert_eq!(
            points.cancel_latest(&owner, &first),
            Err(SyncpointError::CancellationOrder)
        );
        points.cancel_latest(&owner, &second).unwrap();
        assert_eq!(points.completed(&owner, initial.id).unwrap(), initial);
        let replacement = points.reserve(&owner, initial.id, 5).unwrap();
        assert_eq!(replacement.fence(), second.fence());
        assert_ne!(replacement, second);
        assert_eq!(
            points.cancel_latest(&owner, &second),
            Err(SyncpointError::CancellationOrder)
        );
        points.complete(&owner, &first).unwrap();
        assert_eq!(
            points.complete(&owner, &second),
            Err(SyncpointError::CompletionOrder)
        );
        points.cancel_latest(&owner, &replacement).unwrap();
        let next = points.reserve(&owner, initial.id, 2).unwrap();
        assert_eq!(next.fence().value, 5);
        points.complete(&owner, &next).unwrap();
        assert_eq!(
            points.cancel_latest(&owner, &next),
            Err(SyncpointError::CancellationOrder)
        );
    }

    #[test]
    fn cancellation_preserves_ownership_and_wrapped_counter_state() {
        let owner = NvGpuChannelIdentity {
            client: Arc::new(()),
            descriptor: 1,
        };
        let foreign = NvGpuChannelIdentity {
            client: Arc::new(()),
            descriptor: 1,
        };
        let mut points = NvGpuSyncpoints::new(1).unwrap();
        let initial = points.allocate(owner.clone()).unwrap();
        let first = points.reserve(&owner, initial.id, u32::MAX).unwrap();
        points.complete(&owner, &first).unwrap();
        let wrapped = points.reserve(&owner, initial.id, 2).unwrap();
        assert_eq!(wrapped.fence().value, 1);
        assert_eq!(
            points.cancel_latest(&foreign, &wrapped),
            Err(SyncpointError::Owner)
        );
        // A rejected cancellation must retain the pending ticket unchanged.
        points.cancel_latest(&owner, &wrapped).unwrap();
        assert_eq!(points.completed(&owner, initial.id).unwrap(), first.fence());
        let replacement = points.reserve(&owner, initial.id, 1).unwrap();
        assert_eq!(replacement.fence().value, 0);
        points.complete(&owner, &replacement).unwrap();
        assert_eq!(
            points.completed(&owner, initial.id).unwrap(),
            replacement.fence()
        );
    }

    #[test]
    fn reservations_do_not_complete_or_cross_owners_and_retire_in_order() {
        let owner = NvGpuChannelIdentity {
            client: Arc::new(()),
            descriptor: 1,
        };
        let foreign = NvGpuChannelIdentity {
            client: Arc::new(()),
            descriptor: 1,
        };
        let mut points = NvGpuSyncpoints::new(1).unwrap();
        let initial = points.allocate(owner.clone()).unwrap();
        assert_eq!(
            points.allocate(owner.clone()),
            Err(SyncpointError::AlreadyAllocated)
        );
        let first = points.reserve(&owner, initial.id, u32::MAX).unwrap();
        let second = points.reserve(&owner, initial.id, 2).unwrap();
        assert_eq!(second.fence().value, 1);
        assert_eq!(points.completed(&owner, initial.id).unwrap(), initial);
        assert_eq!(
            points.complete(&foreign, &first),
            Err(SyncpointError::Owner)
        );
        assert_eq!(
            points.complete(&owner, &second),
            Err(SyncpointError::CompletionOrder)
        );
        assert_eq!(points.completed(&owner, initial.id).unwrap(), initial);
        let mut other_manager = NvGpuSyncpoints::new(1).unwrap();
        let other_point = other_manager.allocate(owner.clone()).unwrap();
        let foreign_ticket = other_manager
            .reserve(&owner, other_point.id, u32::MAX)
            .unwrap();
        assert_eq!(
            points.complete(&owner, &foreign_ticket),
            Err(SyncpointError::CompletionOrder)
        );
        points.complete(&owner, &first).unwrap();
        assert_eq!(
            points.complete(&owner, &first),
            Err(SyncpointError::CompletionOrder)
        );
        points.complete(&owner, &second).unwrap();
        assert_eq!(
            points.completed(&owner, initial.id).unwrap(),
            second.fence()
        );
        assert_eq!(
            points.release(&foreign, initial.id),
            Err(SyncpointError::Owner)
        );
        points.release(&owner, initial.id).unwrap();
        assert_eq!(
            points.completed(&owner, initial.id),
            Err(SyncpointError::Owner)
        );
        let replacement = points.allocate(owner.clone()).unwrap();
        assert_ne!(replacement.id, initial.id);
        assert_eq!(points.complete(&owner, &second), Err(SyncpointError::Owner));
    }
}
