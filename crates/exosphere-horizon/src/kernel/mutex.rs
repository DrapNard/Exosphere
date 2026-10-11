// SPDX-License-Identifier: GPL-3.0-or-later
//! Process-local guest mutex arbitration for the serial runtime.

use super::*;
use crate::handle::Handle;

pub(super) const WAIT_MASK: u32 = 1 << 30;

#[derive(Debug)]
pub(super) struct MutexWait {
    pub process_id: u64,
    pub thread_id: u64,
    pub owner: u64,
    pub address: u64,
    pub tag: u32,
    /// Condition waits retain their deadline while reacquiring a mutex.
    pub deadline_ns: Option<u64>,
}

pub(super) fn address_error(address: u64) -> Option<u32> {
    if address >= 1 << 39 {
        Some(0xe601) // InvalidPointer
    } else if address % 4 != 0 {
        Some(0xcc01) // InvalidAddress
    } else {
        None
    }
}

impl HorizonKernel {
    pub fn arbitrate_lock(
        &mut self,
        process_id: u64,
        thread_id: u64,
        owner_handle: u32,
        address: u64,
        tag: u32,
    ) -> HorizonResult<u32> {
        if let Some(error) = address_error(address) {
            return Ok(error);
        }
        let caller = self.thread(thread_id)?;
        if caller.process_id != process_id
            || matches!(caller.state, ThreadState::Waiting | ThreadState::Exited)
        {
            return Err(HorizonError::InvalidState("invalid mutex caller".into()));
        }
        let mut bytes = [0; 4];
        if self
            .process(process_id)?
            .memory
            .read(address, &mut bytes)
            .is_err()
        {
            return Ok(0xd401);
        }
        // Userland may have released the mutex before exception entry. In that
        // case the caller retries its atomic acquisition, without a kernel wait.
        if u32::from_le_bytes(bytes) != (owner_handle | WAIT_MASK) {
            return Ok(0);
        }
        let owner = match self.process(process_id)?.handles.get(Handle(owner_handle)) {
            Ok(KernelObject::Thread(id)) => *id,
            _ => return Ok(0xe401),
        };
        let owner_thread = self.thread(owner)?;
        if owner_thread.process_id != process_id || owner_thread.state == ThreadState::Exited {
            return Ok(0xe401);
        }
        self.mutex_waits
            .try_reserve(1)
            .map_err(|_| HorizonError::InvalidState("mutex wait allocation failed".into()))?;
        self.mutex_waits.push(MutexWait {
            process_id,
            thread_id,
            owner,
            address,
            tag,
            deadline_ns: None,
        });
        self.thread_mut(thread_id)?.state = ThreadState::Waiting;
        self.scheduler.remove(thread_id);
        self.recompute_mutex_priorities();
        Ok(0)
    }

    pub fn arbitrate_unlock(
        &mut self,
        process_id: u64,
        thread_id: u64,
        address: u64,
    ) -> HorizonResult<u32> {
        if let Some(error) = address_error(address) {
            return Ok(error);
        }
        if self.thread(thread_id)?.process_id != process_id {
            return Err(HorizonError::InvalidState("invalid mutex caller".into()));
        }
        let matching = |wait: &MutexWait| {
            wait.process_id == process_id && wait.owner == thread_id && wait.address == address
        };
        // Vec order is arrival order, so equal-priority waiters retain FIFO.
        let next = self
            .mutex_waits
            .iter()
            .enumerate()
            .filter(|(_, wait)| matching(wait))
            .min_by_key(|(_, wait)| self.threads[&wait.thread_id].priority)
            .map(|(index, _)| index);
        let count = self
            .mutex_waits
            .iter()
            .filter(|wait| matching(wait))
            .count();
        let value = next.map_or(0, |index| {
            self.mutex_waits[index].tag | if count > 1 { WAIT_MASK } else { 0 }
        });
        // Commit the guest word before changing ownership or waking anyone.
        // A memory fault leaves the wait graph and priorities untouched.
        if self
            .process_mut(process_id)?
            .memory
            .write(address, &value.to_le_bytes())
            .is_err()
        {
            return Ok(0xd401);
        }
        if let Some(index) = next {
            let next = self.mutex_waits.remove(index);
            for wait in &mut self.mutex_waits {
                if matching(wait) {
                    wait.owner = next.thread_id;
                }
            }
            let thread = self.thread_mut(next.thread_id)?;
            thread.context.x[0] = 0;
            thread.state = ThreadState::Ready;
            let priority = thread.priority;
            self.scheduler
                .enqueue_with_priority(next.thread_id, priority);
        }
        self.recompute_mutex_priorities();
        Ok(0)
    }

    pub fn set_thread_base_priority(&mut self, id: u64, priority: i32) -> HorizonResult<()> {
        self.thread_mut(id)?.base_priority = priority;
        self.recompute_mutex_priorities();
        Ok(())
    }

    pub(super) fn recompute_mutex_priorities(&mut self) {
        for thread in self.threads.values_mut() {
            thread.priority = thread.base_priority;
        }
        // Donation propagates across chains. Starting from base priorities also
        // removes obsolete donations after handoff, cancellation or base changes.
        for _ in 0..self.threads.len() {
            let mut changed = false;
            for wait in &self.mutex_waits {
                let priority = self.threads[&wait.thread_id].priority;
                let owner = self
                    .threads
                    .get_mut(&wait.owner)
                    .expect("validated mutex owner");
                if priority < owner.priority {
                    owner.priority = priority;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for thread in self.threads.values() {
            self.scheduler.reprioritize(thread.id, thread.priority);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};

    const WORD: u64 = 0x2000;

    fn fixture() -> (HorizonKernel, u64) {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "mutex");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                WORD,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        (kernel, pid)
    }

    fn word(kernel: &HorizonKernel, pid: u64, address: u64) -> u32 {
        let mut bytes = [0; 4];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(address, &mut bytes)
            .unwrap();
        u32::from_le_bytes(bytes)
    }

    fn lock(kernel: &mut HorizonKernel, pid: u64, address: u64, handle: Handle) {
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(address, &(handle.0 | WAIT_MASK).to_le_bytes())
            .unwrap();
    }

    #[test]
    fn handoff_selects_priority_then_fifo_and_reparents_waiters() {
        let (mut kernel, pid) = fixture();
        let (owner, owner_handle) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let (first, first_handle) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        let (second, second_handle) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        lock(&mut kernel, pid, WORD, owner_handle);
        // Arrival order deliberately differs from thread IDs.
        kernel
            .arbitrate_lock(pid, second, owner_handle.0, WORD, second_handle.0)
            .unwrap();
        kernel
            .arbitrate_lock(pid, first, owner_handle.0, WORD, first_handle.0)
            .unwrap();
        assert_eq!(kernel.thread(owner).unwrap().priority, 24);
        assert_eq!(kernel.thread(first).unwrap().state, ThreadState::Waiting);
        assert_eq!(kernel.arbitrate_unlock(pid, owner, WORD).unwrap(), 0);
        assert_eq!(word(&kernel, pid, WORD), second_handle.0 | WAIT_MASK);
        assert_eq!(kernel.thread(second).unwrap().state, ThreadState::Ready);
        assert_eq!(kernel.thread(first).unwrap().state, ThreadState::Waiting);
        assert_eq!(kernel.thread(owner).unwrap().priority, 44);
        assert_eq!(kernel.scheduler.next(), Some(second));
        kernel.arbitrate_unlock(pid, second, WORD).unwrap();
        assert_eq!(word(&kernel, pid, WORD), first_handle.0);
        assert_eq!(kernel.scheduler.next(), Some(first));
        kernel.arbitrate_unlock(pid, first, WORD).unwrap();
        assert_eq!(word(&kernel, pid, WORD), 0);
        assert!(kernel.mutex_waits.is_empty());
    }

    #[test]
    fn priority_donation_propagates_and_base_changes_do_not_wake_waiters() {
        let (mut kernel, pid) = fixture();
        let (owner, oh) = kernel.create_thread(pid, 0x1000, 0x9000, 48).unwrap();
        let (middle, mh) = kernel.create_thread(pid, 0x1000, 0x9000, 40).unwrap();
        let (high, hh) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        lock(&mut kernel, pid, WORD, oh);
        lock(&mut kernel, pid, WORD + 4, mh);
        kernel
            .arbitrate_lock(pid, middle, oh.0, WORD, mh.0)
            .unwrap();
        kernel
            .arbitrate_lock(pid, high, mh.0, WORD + 4, hh.0)
            .unwrap();
        assert_eq!(kernel.thread(owner).unwrap().priority, 24);
        assert_eq!(kernel.thread(middle).unwrap().priority, 24);
        kernel.set_thread_base_priority(high, 32).unwrap();
        assert_eq!(kernel.thread(owner).unwrap().priority, 32);
        assert_eq!(kernel.thread(middle).unwrap().priority, 32);
        assert_eq!(kernel.thread(high).unwrap().state, ThreadState::Waiting);
        assert!(kernel.scheduler.is_empty());
        kernel.arbitrate_unlock(pid, owner, WORD).unwrap();
        assert_eq!(kernel.thread(owner).unwrap().priority, 48);
        assert_eq!(kernel.thread(middle).unwrap().priority, 32);
        kernel.arbitrate_unlock(pid, middle, WORD + 4).unwrap();
        assert_eq!(kernel.thread(middle).unwrap().priority, 40);
        assert_eq!(kernel.thread(high).unwrap().state, ThreadState::Ready);
    }

    #[test]
    fn changed_word_and_invalid_inputs_do_not_suspend_or_mutate() {
        let (mut kernel, pid) = fixture();
        let (caller, _) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        assert_eq!(
            kernel
                .arbitrate_lock(pid, caller, u32::MAX, WORD, 7)
                .unwrap(),
            0
        );
        for (address, error) in [(WORD + 1, 0xcc01), (1 << 39, 0xe601), (0x8000, 0xd401)] {
            assert_eq!(
                kernel.arbitrate_lock(pid, caller, 7, address, 9).unwrap(),
                error
            );
        }
        lock(&mut kernel, pid, WORD, Handle(7));
        assert_eq!(
            kernel.arbitrate_lock(pid, caller, 7, WORD, 9).unwrap(),
            0xe401
        );
        assert!(kernel.mutex_waits.is_empty());
        assert_eq!(kernel.thread(caller).unwrap().state, ThreadState::Created);
    }

    #[test]
    fn failed_unlock_preserves_wait_graph_word_and_priority() {
        let (mut kernel, pid) = fixture();
        let (owner, oh) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let (waiter, wh) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        lock(&mut kernel, pid, WORD, oh);
        kernel
            .arbitrate_lock(pid, waiter, oh.0, WORD, wh.0)
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .protect(WORD, PAGE_SIZE, MemoryPermission::READ)
            .unwrap();
        let epoch = kernel.process(pid).unwrap().memory.write_epoch();
        assert_eq!(kernel.arbitrate_unlock(pid, owner, WORD).unwrap(), 0xd401);
        assert_eq!(kernel.process(pid).unwrap().memory.write_epoch(), epoch);
        assert_eq!(word(&kernel, pid, WORD), oh.0 | WAIT_MASK);
        assert_eq!(kernel.thread(waiter).unwrap().state, ThreadState::Waiting);
        assert_eq!(kernel.thread(owner).unwrap().priority, 24);
        assert_eq!(kernel.mutex_waits.len(), 1);
        assert!(kernel.scheduler.is_empty());
    }

    #[test]
    fn condition_signal_does_not_confuse_or_wake_mutex_waiters() {
        let (mut kernel, pid) = fixture();
        let (owner, oh) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let (waiter, wh) = kernel.create_thread(pid, 0x1000, 0x9000, 24).unwrap();
        lock(&mut kernel, pid, WORD, oh);
        kernel
            .arbitrate_lock(pid, waiter, oh.0, WORD, wh.0)
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(WORD + 4, &1u32.to_le_bytes())
            .unwrap();
        kernel.signal_process_wide_key(pid, WORD + 4, 1).unwrap();
        assert_eq!(word(&kernel, pid, WORD + 4), 0);
        assert_eq!(word(&kernel, pid, WORD), oh.0 | WAIT_MASK);
        assert_eq!(kernel.thread(waiter).unwrap().state, ThreadState::Waiting);
        assert_eq!(kernel.thread(owner).unwrap().priority, 24);
        assert_eq!(kernel.mutex_waits.len(), 1);
        assert!(kernel.scheduler.is_empty());
    }
}
