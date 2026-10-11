// SPDX-License-Identifier: GPL-3.0-or-later
//! Condition waits release a mutex atomically with entering the serial wait queue.

use super::*;
use crate::handle::Handle;
use mutex::{MutexWait, WAIT_MASK, address_error};

#[derive(Debug)]
pub(super) struct ConditionWait {
    pub process_id: u64,
    pub thread_id: u64,
    address: u64,
    key: u64,
    tag: u32,
    deadline_ns: Option<u64>,
}

impl HorizonKernel {
    pub fn wait_condition(
        &mut self,
        process_id: u64,
        thread_id: u64,
        address: u64,
        key: u64,
        tag: u32,
        timeout_ns: i64,
    ) -> HorizonResult<u32> {
        if let Some(error) = address_error(address) {
            return Ok(error);
        }
        let caller = self.thread(thread_id)?;
        if caller.process_id != process_id
            || matches!(caller.state, ThreadState::Waiting | ThreadState::Exited)
        {
            return Err(HorizonError::InvalidState(
                "invalid condition caller".into(),
            ));
        }
        if timeout_ns != 0 {
            self.condition_waits.try_reserve(1).map_err(|_| {
                HorizonError::InvalidState("condition wait allocation failed".into())
            })?;
        }
        // Horizon's waiter-flag write is best effort. Failure of the mutex
        // access, unlike failure of this hint, is returned to the caller.
        let _ = self
            .process_mut(process_id)?
            .memory
            .write(key, &1u32.to_le_bytes());
        let code = self.arbitrate_unlock(process_id, thread_id, address)?;
        if code != 0 {
            return Ok(code);
        }
        if timeout_ns == 0 {
            return Ok(0xea01);
        }
        let deadline_ns = (timeout_ns > 0).then(|| {
            self.time
                .steady_clock()
                .time_point_ns
                .saturating_add(timeout_ns as u64)
        });
        self.condition_waits.push(ConditionWait {
            process_id,
            thread_id,
            address,
            key,
            tag,
            deadline_ns,
        });
        self.thread_mut(thread_id)?.state = ThreadState::Waiting;
        self.scheduler.remove(thread_id);
        Ok(0)
    }

    pub(super) fn signal_condition(
        &mut self,
        process_id: u64,
        key: u64,
        count: i32,
    ) -> HorizonResult<()> {
        self.poll_condition_deadlines(self.time.steady_clock().time_point_ns)?;
        let matches = |wait: &ConditionWait| wait.process_id == process_id && wait.key == key;
        let available = self
            .condition_waits
            .iter()
            .filter(|wait| matches(wait))
            .count();
        let limit = if count <= 0 {
            available
        } else {
            available.min(count as usize)
        };
        // Every selected condition waiter can become a mutex waiter. Reserve
        // that capacity before consuming any condition entries.
        self.mutex_waits.try_reserve(limit).map_err(|_| {
            HorizonError::InvalidState("condition reacquisition allocation failed".into())
        })?;
        for _ in 0..limit {
            let index = self
                .condition_waits
                .iter()
                .enumerate()
                .filter(|(_, wait)| matches(wait))
                .min_by_key(|(_, wait)| self.threads[&wait.thread_id].priority)
                .map(|(index, _)| index)
                .expect("counted condition entries");
            let wait = self.condition_waits.remove(index);
            self.reacquire_condition_mutex(wait)?;
        }
        if !self.condition_waits.iter().any(matches) {
            // Signal is a void SVC; a bad flag pointer cannot produce a result.
            let _ = self
                .process_mut(process_id)?
                .memory
                .write(key, &0u32.to_le_bytes());
        }
        Ok(())
    }

    fn reacquire_condition_mutex(&mut self, wait: ConditionWait) -> HorizonResult<()> {
        let mut bytes = [0; 4];
        let memory = &mut self.process_mut(wait.process_id)?.memory;
        if memory.read(wait.address, &mut bytes).is_err() {
            return self.finish_condition_wait(wait.thread_id, 0xd401);
        }
        let previous = u32::from_le_bytes(bytes);
        let value = if previous == 0 {
            wait.tag
        } else {
            previous | WAIT_MASK
        };
        // No other guest executes inside this serial kernel transition, so the
        // read/update is atomic with respect to all guest CPU slices.
        if memory.write(wait.address, &value.to_le_bytes()).is_err() {
            return self.finish_condition_wait(wait.thread_id, 0xd401);
        }
        if previous == 0 {
            return self.finish_condition_wait(wait.thread_id, 0);
        }
        let owner = match self
            .process(wait.process_id)?
            .handles
            .get(Handle(previous & !WAIT_MASK))
        {
            Ok(KernelObject::Thread(id))
                if self.thread(*id)?.process_id == wait.process_id
                    && self.thread(*id)?.state != ThreadState::Exited =>
            {
                *id
            }
            _ => return self.finish_condition_wait(wait.thread_id, 0xfa01),
        };
        self.mutex_waits.push(MutexWait {
            process_id: wait.process_id,
            thread_id: wait.thread_id,
            owner,
            address: wait.address,
            tag: wait.tag,
            deadline_ns: wait.deadline_ns,
        });
        self.recompute_mutex_priorities();
        Ok(())
    }

    fn finish_condition_wait(&mut self, id: u64, result: u32) -> HorizonResult<()> {
        let thread = self.thread_mut(id)?;
        thread.context.x[0] = u64::from(result);
        thread.state = ThreadState::Ready;
        let priority = thread.priority;
        self.scheduler.enqueue_with_priority(id, priority);
        Ok(())
    }

    pub(super) fn poll_condition_deadlines(&mut self, now: u64) -> HorizonResult<()> {
        let expired: Vec<_> = self
            .condition_waits
            .iter()
            .filter(|wait| wait.deadline_ns.is_some_and(|deadline| now >= deadline))
            .map(|wait| wait.thread_id)
            .chain(
                self.mutex_waits
                    .iter()
                    .filter(|wait| wait.deadline_ns.is_some_and(|deadline| now >= deadline))
                    .map(|wait| wait.thread_id),
            )
            .collect();
        for id in &expired {
            self.condition_waits.retain(|wait| wait.thread_id != *id);
            self.mutex_waits.retain(|wait| wait.thread_id != *id);
            if self.thread(*id)?.state != ThreadState::Exited {
                // Expiration cancels the wait; unlike signal, it does not acquire
                // the mutex. Preserve every output register except the result.
                self.finish_condition_wait(*id, 0xea01)?;
            }
        }
        if !expired.is_empty() {
            self.recompute_mutex_priorities();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};

    const MUTEX: u64 = 0x2000;
    const KEY: u64 = MUTEX + 4;

    fn fixture() -> (HorizonKernel, u64) {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        struct FrozenClock;
        impl crate::time::TimeSource for FrozenClock {
            fn monotonic_ns(&self) -> u64 {
                0
            }
            fn unix_seconds(&self) -> i64 {
                0
            }
        }
        kernel.time = crate::time::TimeManager::new(Box::new(FrozenClock));
        let pid = kernel.create_process(1, "condition");
        map_words(&mut kernel, pid);
        (kernel, pid)
    }

    fn map_words(kernel: &mut HorizonKernel, pid: u64) {
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                MUTEX,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
    }

    fn write(kernel: &mut HorizonKernel, pid: u64, address: u64, value: u32) {
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(address, &value.to_le_bytes())
            .unwrap();
    }

    fn read(kernel: &HorizonKernel, pid: u64, address: u64) -> u32 {
        let mut bytes = [0; 4];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(address, &mut bytes)
            .unwrap();
        u32::from_le_bytes(bytes)
    }

    #[test]
    fn wait_releases_and_signal_reacquires_the_mutex() {
        let (mut kernel, pid) = fixture();
        let (thread, handle) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        write(&mut kernel, pid, MUTEX, handle.0);
        kernel.thread_mut(thread).unwrap().context.x[1] = 0x1234;
        assert_eq!(
            kernel
                .wait_condition(pid, thread, MUTEX, KEY, handle.0, -1)
                .unwrap(),
            0
        );
        assert_eq!(read(&kernel, pid, MUTEX), 0);
        assert_eq!(read(&kernel, pid, KEY), 1);
        assert_eq!(kernel.thread(thread).unwrap().state, ThreadState::Waiting);
        kernel.signal_process_wide_key(pid, KEY, 1).unwrap();
        assert_eq!(read(&kernel, pid, MUTEX), handle.0);
        assert_eq!(read(&kernel, pid, KEY), 0);
        assert_eq!(kernel.thread(thread).unwrap().state, ThreadState::Ready);
        assert_eq!(kernel.thread(thread).unwrap().context.x[1], 0x1234);
        assert_eq!(kernel.scheduler.next(), Some(thread));
        assert!(kernel.condition_waits.is_empty());
    }

    #[test]
    fn signaled_waiter_requeues_on_busy_mutex_and_donates_priority() {
        let (mut kernel, pid) = fixture();
        let (owner, oh) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let (waiter, wh) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        write(&mut kernel, pid, MUTEX, wh.0);
        kernel
            .wait_condition(pid, waiter, MUTEX, KEY, wh.0, -1)
            .unwrap();
        write(&mut kernel, pid, MUTEX, oh.0);
        kernel.signal_process_wide_key(pid, KEY, 1).unwrap();
        assert_eq!(read(&kernel, pid, MUTEX), oh.0 | WAIT_MASK);
        assert_eq!(kernel.thread(waiter).unwrap().state, ThreadState::Waiting);
        assert_eq!(kernel.thread(owner).unwrap().priority, 24);
        assert!(kernel.condition_waits.is_empty());
        assert!(kernel.scheduler.is_empty());
        kernel.arbitrate_unlock(pid, owner, MUTEX).unwrap();
        assert_eq!(read(&kernel, pid, MUTEX), wh.0);
        assert_eq!(kernel.thread(owner).unwrap().priority, 44);
        assert_eq!(kernel.scheduler.next(), Some(waiter));
    }

    #[test]
    fn count_priority_fifo_and_broadcast_preserve_remaining_flag() {
        let (mut kernel, pid) = fixture();
        let (low, lh) = kernel.create_thread(pid, 0x1000, 0x9000, 32).unwrap();
        let (first, fh) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        let (second, sh) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        for (thread, handle) in [(low, lh), (second, sh), (first, fh)] {
            write(&mut kernel, pid, MUTEX, handle.0);
            kernel
                .wait_condition(pid, thread, MUTEX, KEY, handle.0, -1)
                .unwrap();
        }
        kernel.signal_process_wide_key(pid, KEY, 1).unwrap();
        assert_eq!(kernel.scheduler.next(), Some(second));
        assert_eq!(read(&kernel, pid, KEY), 1);
        assert_eq!(kernel.condition_waits.len(), 2);
        kernel.signal_process_wide_key(pid, KEY, 0).unwrap();
        assert!(kernel.condition_waits.is_empty());
        assert_eq!(read(&kernel, pid, KEY), 0);
        assert_eq!(kernel.mutex_waits.len(), 2);
        kernel.arbitrate_unlock(pid, second, MUTEX).unwrap();
        assert_eq!(read(&kernel, pid, MUTEX), fh.0 | WAIT_MASK);
        assert_eq!(kernel.scheduler.next(), Some(first));
        assert_eq!(kernel.thread(low).unwrap().state, ThreadState::Waiting);
    }

    #[test]
    fn timeout_cancels_both_condition_and_reacquisition_without_ownership() {
        for signal_before_expiry in [false, true] {
            let (mut kernel, pid) = fixture();
            let (owner, oh) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let (waiter, wh) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
            write(&mut kernel, pid, MUTEX, wh.0);
            kernel
                .wait_condition(pid, waiter, MUTEX, KEY, wh.0, 100)
                .unwrap();
            let deadline = kernel.condition_waits[0].deadline_ns.unwrap();
            write(&mut kernel, pid, MUTEX, oh.0);
            if signal_before_expiry {
                kernel.signal_process_wide_key(pid, KEY, 1).unwrap();
                assert_eq!(kernel.thread(owner).unwrap().priority, 24);
            }
            kernel.poll_condition_deadlines(deadline - 1).unwrap();
            assert_eq!(kernel.thread(waiter).unwrap().state, ThreadState::Waiting);
            kernel.poll_condition_deadlines(deadline).unwrap();
            assert_eq!(kernel.thread(waiter).unwrap().state, ThreadState::Ready);
            assert_eq!(kernel.thread(waiter).unwrap().context.x[0], 0xea01);
            assert_eq!(read(&kernel, pid, MUTEX) & !WAIT_MASK, oh.0);
            assert_eq!(kernel.thread(owner).unwrap().priority, 44);
            assert!(kernel.condition_waits.is_empty());
            assert!(kernel.mutex_waits.is_empty());
        }
    }

    #[test]
    fn zero_timeout_releases_mutex_and_invalid_address_does_not_wait() {
        let (mut kernel, pid) = fixture();
        let (thread, handle) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        write(&mut kernel, pid, MUTEX, handle.0);
        assert_eq!(
            kernel
                .wait_condition(pid, thread, MUTEX + 1, KEY, handle.0, -1)
                .unwrap(),
            0xcc01
        );
        assert_eq!(read(&kernel, pid, MUTEX), handle.0);
        assert_eq!(read(&kernel, pid, KEY), 0);
        assert_eq!(
            kernel
                .wait_condition(pid, thread, MUTEX, KEY, handle.0, 0)
                .unwrap(),
            0xea01
        );
        assert_eq!(read(&kernel, pid, MUTEX), 0);
        assert_eq!(kernel.thread(thread).unwrap().state, ThreadState::Created);
        assert!(kernel.condition_waits.is_empty());
    }

    #[test]
    fn same_key_in_other_process_is_not_signaled_and_bad_owner_returns_error() {
        let (mut kernel, pid) = fixture();
        let other = kernel.create_process(2, "other");
        map_words(&mut kernel, other);
        let (thread, handle) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        write(&mut kernel, pid, MUTEX, handle.0);
        kernel
            .wait_condition(pid, thread, MUTEX, KEY, handle.0, -1)
            .unwrap();
        kernel.signal_process_wide_key(other, KEY, -1).unwrap();
        assert_eq!(kernel.thread(thread).unwrap().state, ThreadState::Waiting);
        assert_eq!(read(&kernel, pid, KEY), 1);
        write(&mut kernel, pid, MUTEX, 7);
        kernel.signal_process_wide_key(pid, KEY, -1).unwrap();
        assert_eq!(kernel.thread(thread).unwrap().context.x[0], 0xfa01);
        assert_eq!(kernel.thread(thread).unwrap().state, ThreadState::Ready);
    }
}
