// SPDX-License-Identifier: GPL-3.0-or-later
//! Ownership transitions for display buffers, independent of Binder wire framing.
//!
//! The payload owns the real backing resource supplied by the graphics allocator.
//! Queueing retains that resource; it never manufactures a rendered image.

use std::collections::VecDeque;

use crate::{HorizonError, HorizonResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState {
    Free,
    Dequeued,
    Queued,
    Acquired,
}

#[derive(Debug)]
struct Slot<B> {
    buffer: Option<B>,
    state: BufferState,
    frame: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedFrame {
    pub slot: usize,
    pub number: u64,
}

/// Serial producer/consumer ownership model. Fence signaling and Binder commands
/// must be integrated separately before this can present guest GPU work.
#[derive(Debug)]
pub struct BufferQueue<B> {
    slots: Vec<Slot<B>>,
    pending: VecDeque<QueuedFrame>,
    connected: bool,
    next_frame: u64,
    availability: crate::event::SharedEvent,
}

impl<B> Drop for BufferQueue<B> {
    fn drop(&mut self) {
        // Copied event handles may outlive the queue; a destroyed producer must
        // not continue advertising usable buffers through those handles.
        self.availability.set_signaled(false);
    }
}

fn invalid(message: &str) -> HorizonError {
    HorizonError::InvalidState(message.into())
}

impl<B> BufferQueue<B> {
    pub fn new(slot_count: usize) -> HorizonResult<Self> {
        if slot_count == 0 {
            return Err(invalid("buffer queue requires slots"));
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(slot_count)
            .map_err(|_| invalid("buffer queue allocation failed"))?;
        slots.resize_with(slot_count, || Slot {
            buffer: None,
            state: BufferState::Free,
            frame: None,
        });
        Ok(Self {
            slots,
            pending: VecDeque::new(),
            connected: false,
            next_frame: 1,
            availability: Default::default(),
        })
    }

    pub fn connect(&mut self) -> HorizonResult<()> {
        if self.connected {
            return Err(invalid("buffer producer already connected"));
        }
        self.connected = true;
        self.update_availability();
        Ok(())
    }

    pub fn availability_event(&self) -> crate::event::SharedEvent {
        self.availability.clone()
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn update_availability(&self) {
        self.availability.set_signaled(
            self.connected
                && self
                    .slots
                    .iter()
                    .any(|slot| slot.state == BufferState::Free && slot.buffer.is_some()),
        );
    }

    pub fn state(&self, slot: usize) -> HorizonResult<BufferState> {
        self.slots
            .get(slot)
            .map(|slot| slot.state)
            .ok_or_else(|| invalid("invalid buffer slot"))
    }

    pub fn attach(&mut self, index: usize, buffer: B) -> HorizonResult<Option<B>> {
        let slot = self
            .slots
            .get_mut(index)
            .ok_or_else(|| invalid("invalid buffer slot"))?;
        if slot.state != BufferState::Free {
            return Err(invalid("cannot replace an in-use buffer"));
        }
        let previous = slot.buffer.replace(buffer);
        self.update_availability();
        Ok(previous)
    }

    /// No free attached buffer is temporary exhaustion, not a fabricated slot.
    pub fn dequeue(&mut self) -> HorizonResult<Option<usize>> {
        if !self.connected {
            return Err(invalid("buffer producer is not connected"));
        }
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.state == BufferState::Free && slot.buffer.is_some())
        else {
            return Ok(None);
        };
        self.slots[index].state = BufferState::Dequeued;
        self.update_availability();
        Ok(Some(index))
    }

    pub fn producer_buffer(&self, index: usize) -> HorizonResult<&B> {
        let slot = self
            .slots
            .get(index)
            .ok_or_else(|| invalid("invalid buffer slot"))?;
        if slot.state != BufferState::Dequeued {
            return Err(invalid("producer does not own buffer"));
        }
        slot.buffer
            .as_ref()
            .ok_or_else(|| invalid("buffer is not attached"))
    }

    pub fn cancel(&mut self, index: usize) -> HorizonResult<()> {
        let slot = self
            .slots
            .get_mut(index)
            .ok_or_else(|| invalid("invalid buffer slot"))?;
        if slot.state != BufferState::Dequeued {
            return Err(invalid("only dequeued buffers can be cancelled"));
        }
        slot.state = BufferState::Free;
        self.update_availability();
        Ok(())
    }

    pub fn queue(&mut self, index: usize) -> HorizonResult<QueuedFrame> {
        if !self.connected {
            return Err(invalid("buffer producer is not connected"));
        }
        if self.state(index)? != BufferState::Dequeued {
            return Err(invalid("only dequeued buffers can be queued"));
        }
        let next = self
            .next_frame
            .checked_add(1)
            .ok_or_else(|| invalid("buffer frame sequence exhausted"))?;
        self.pending
            .try_reserve(1)
            .map_err(|_| invalid("pending frame allocation failed"))?;
        let frame = QueuedFrame {
            slot: index,
            number: self.next_frame,
        };
        self.pending.push_back(frame);
        self.slots[index].state = BufferState::Queued;
        self.slots[index].frame = Some(frame.number);
        self.next_frame = next;
        Ok(frame)
    }

    pub fn acquire(&mut self) -> Option<QueuedFrame> {
        let frame = self.pending.pop_front()?;
        self.slots[frame.slot].state = BufferState::Acquired;
        Some(frame)
    }

    pub fn consumer_buffer(&self, frame: QueuedFrame) -> HorizonResult<&B> {
        let slot = self
            .slots
            .get(frame.slot)
            .ok_or_else(|| invalid("invalid buffer slot"))?;
        if slot.state != BufferState::Acquired || slot.frame != Some(frame.number) {
            return Err(invalid("consumer does not own this frame"));
        }
        slot.buffer
            .as_ref()
            .ok_or_else(|| invalid("buffer is not attached"))
    }

    pub fn release(&mut self, frame: QueuedFrame) -> HorizonResult<()> {
        self.consumer_buffer(frame)?;
        let slot = &mut self.slots[frame.slot];
        slot.state = BufferState::Free;
        slot.frame = None;
        self.update_availability();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_memory::SharedMemory;

    #[test]
    fn copied_availability_tracks_real_free_buffers_and_outlives_queue() {
        let mut queue = BufferQueue::new(2).unwrap();
        let event = queue.availability_event();
        assert_eq!(event, queue.availability_event());
        assert_ne!(event, crate::event::SharedEvent::default());
        assert!(!event.is_signaled());
        queue.attach(0, 10).unwrap();
        assert!(!event.is_signaled());
        queue.connect().unwrap();
        assert!(event.is_signaled());
        assert!(queue.attach(3, 30).is_err());
        assert!(event.is_signaled());
        assert_eq!(queue.dequeue().unwrap(), Some(0));
        assert!(!event.is_signaled());
        queue.cancel(0).unwrap();
        assert!(event.is_signaled());
        queue.dequeue().unwrap();
        let frame = queue.queue(0).unwrap();
        queue.acquire().unwrap();
        assert!(!event.is_signaled());
        queue.release(frame).unwrap();
        assert!(event.is_signaled());
        queue.attach(1, 20).unwrap();
        queue.dequeue().unwrap();
        assert!(event.is_signaled());
        drop(queue);
        assert!(!event.is_signaled());
    }

    #[test]
    fn frame_lifecycle_retains_actual_backing_and_prevents_early_reuse() {
        let memory = SharedMemory::new(4096).unwrap();
        let mut queue = BufferQueue::new(1).unwrap();
        queue.attach(0, memory.clone()).unwrap();
        queue.connect().unwrap();
        assert_eq!(queue.dequeue().unwrap(), Some(0));
        queue.producer_buffer(0).unwrap().write(0, &[42]).unwrap();
        drop(memory);
        let frame = queue.queue(0).unwrap();
        assert_eq!(queue.dequeue().unwrap(), None);
        assert!(queue.attach(0, SharedMemory::new(4096).unwrap()).is_err());
        assert_eq!(queue.acquire(), Some(frame));
        let mut byte = [0];
        queue
            .consumer_buffer(frame)
            .unwrap()
            .read(0, &mut byte)
            .unwrap();
        assert_eq!(byte, [42]);
        assert_eq!(queue.dequeue().unwrap(), None);
        queue.release(frame).unwrap();
        assert_eq!(queue.dequeue().unwrap(), Some(0));
        let next = queue.queue(0).unwrap();
        queue.acquire().unwrap();
        assert_ne!(next.number, frame.number);
        assert!(queue.release(frame).is_err());
        assert_eq!(queue.state(0).unwrap(), BufferState::Acquired);
        queue.release(next).unwrap();
    }

    #[test]
    fn fifo_submission_cancel_and_rejected_operations_preserve_ownership() {
        let mut queue = BufferQueue::new(2).unwrap();
        queue.attach(0, 10).unwrap();
        queue.attach(1, 20).unwrap();
        assert!(queue.dequeue().is_err());
        queue.connect().unwrap();
        assert!(queue.connect().is_err());
        assert!(queue.queue(0).is_err());
        assert_eq!(queue.dequeue().unwrap(), Some(0));
        queue.cancel(0).unwrap();
        assert!(queue.cancel(0).is_err());
        assert_eq!(queue.dequeue().unwrap(), Some(0));
        assert_eq!(queue.dequeue().unwrap(), Some(1));
        let first = queue.queue(1).unwrap();
        let second = queue.queue(0).unwrap();
        assert!(queue.queue(1).is_err());
        assert!(queue.cancel(1).is_err());
        assert_eq!(queue.acquire(), Some(first));
        assert_eq!(queue.acquire(), Some(second));
        assert_eq!(queue.acquire(), None);
        assert_eq!(*queue.consumer_buffer(first).unwrap(), 20);
        assert!(queue.state(2).is_err());
    }
}
