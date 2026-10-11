// SPDX-License-Identifier: GPL-3.0-or-later
//! Npad common LIFO publication into service-owned shared storage.

use super::NpadState;
use crate::{HorizonError, HorizonResult, shared_memory::SharedMemory};

const CAPACITY: usize = 17;
const HEADER_SIZE: usize = 0x20;
const SLOT_SIZE: usize = 0x30;
pub const NPAD_RING_SIZE: usize = HEADER_SIZE + CAPACITY * SLOT_SIZE;

/// Owns one controller/style history. A caller must select the firmware layout
/// offset and apply resource activation/style/ID policy before publishing.
#[derive(Debug)]
pub struct NpadRing {
    bytes: [u8; NPAD_RING_SIZE],
    count: usize,
    tail: usize,
    sampling_number: u64,
}

impl Default for NpadRing {
    fn default() -> Self {
        Self {
            bytes: [0; NPAD_RING_SIZE],
            count: 0,
            tail: 0,
            sampling_number: 0,
        }
    }
}

impl NpadRing {
    /// Publish one tick, including unchanged input. Success commits both the
    /// ring and counters; failure leaves history untouched. SharedMemory writes
    /// hold a single lock, so guest readers never observe a partially written slot.
    pub fn publish(
        &mut self,
        memory: &SharedMemory,
        offset: usize,
        state: NpadState,
    ) -> HorizonResult<()> {
        self.publish_fields(memory, offset, state, None)
    }

    /// Publish the FullKey style word and sample as one shared-memory revision.
    pub(crate) fn publish_full_key(
        &mut self,
        memory: &SharedMemory,
        npad_base: usize,
        state: NpadState,
    ) -> HorizonResult<()> {
        let ring_offset = npad_base
            .checked_add(0x28)
            .ok_or(HorizonError::InvalidAddress(npad_base as u64))?;
        self.publish_fields(memory, ring_offset, state, Some(npad_base))
    }

    fn publish_fields(
        &mut self,
        memory: &SharedMemory,
        offset: usize,
        state: NpadState,
        style_offset: Option<usize>,
    ) -> HorizonResult<()> {
        if offset % 8 != 0 {
            return Err(HorizonError::InvalidState(
                "HID ring offset must be 8-byte aligned".into(),
            ));
        }
        let next_sample = self.sampling_number.wrapping_add(1);
        let tail = if self.count == 0 {
            0
        } else {
            (self.tail + 1) % CAPACITY
        };
        let count = (self.count + 1).min(CAPACITY);
        let mut bytes = self.bytes;
        bytes[8..16].copy_from_slice(&(CAPACITY as u64).to_le_bytes());
        bytes[16..24].copy_from_slice(&(tail as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&(count as u64).to_le_bytes());
        let slot = HEADER_SIZE + tail * SLOT_SIZE;
        bytes[slot..slot + 8].copy_from_slice(&next_sample.to_le_bytes());
        bytes[slot + 8..slot + SLOT_SIZE].copy_from_slice(&state.encode_common_sample(next_sample));
        if let Some(style_offset) = style_offset {
            let style = u32::from(state.connected).to_le_bytes();
            memory.write_batch(&[(style_offset, &style), (offset, &bytes)])?;
        } else {
            memory.write(offset, &bytes)?;
        }
        self.bytes = bytes;
        self.tail = tail;
        self.count = count;
        self.sampling_number = next_sample;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_key_style_and_ring_commit_together_or_not_at_all() {
        let memory = SharedMemory::new(4096).unwrap();
        let mut ring = NpadRing::default();
        let connected = NpadState {
            connected: true,
            ..NpadState::default()
        };
        // The style fits but the ring does not: no style byte may be published.
        assert!(ring.publish_full_key(&memory, 4088, connected).is_err());
        let mut style = [0; 4];
        memory.read(4088, &mut style).unwrap();
        assert_eq!(style, [0; 4]);
        assert_eq!(memory.revision(), 0);
        ring.publish_full_key(&memory, 0, connected).unwrap();
        assert_eq!(memory.revision(), 1);
        memory.read(0, &mut style).unwrap();
        assert_eq!(u32::from_le_bytes(style), 1);
        let mut bytes = [0; NPAD_RING_SIZE];
        memory.read(0x28, &mut bytes).unwrap();
        assert_eq!(word(&bytes, HEADER_SIZE), 1);
        assert!(memory.write_batch(&[(0, &[1, 2]), (1, &[3])]).is_err());
        assert_eq!(memory.revision(), 1);
    }

    fn word(bytes: &[u8], offset: usize) -> u64 {
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    }

    #[test]
    fn wrapped_ring_contains_consecutive_atomic_and_state_samples() {
        let memory = SharedMemory::new(4096).unwrap();
        let alias = memory.clone();
        let mut ring = NpadRing::default();
        for tick in 1..=40 {
            ring.publish(
                &memory,
                64,
                NpadState {
                    connected: true,
                    buttons: tick,
                    ..NpadState::default()
                },
            )
            .unwrap();
        }
        let mut bytes = [0; NPAD_RING_SIZE];
        alias.read(64, &mut bytes).unwrap();
        assert_eq!(word(&bytes, 0), 0);
        assert_eq!(word(&bytes, 8), 17);
        assert_eq!(word(&bytes, 24), 17);
        let tail = word(&bytes, 16) as usize;
        for age in 0..CAPACITY {
            let index = (tail + CAPACITY - age) % CAPACITY;
            let slot = HEADER_SIZE + index * SLOT_SIZE;
            let expected = 40 - age as u64;
            assert_eq!(word(&bytes, slot), expected);
            assert_eq!(word(&bytes, slot + 8), expected);
            assert_eq!(word(&bytes, slot + 16), expected);
        }
    }

    #[test]
    fn failed_publication_does_not_advance_history() {
        let memory = SharedMemory::new(4096).unwrap();
        let mut ring = NpadRing::default();
        assert!(ring.publish(&memory, 1, NpadState::default()).is_err());
        assert!(ring.publish(&memory, 4096, NpadState::default()).is_err());
        assert_eq!(memory.revision(), 0);
        ring.publish(&memory, 0, NpadState::default()).unwrap();
        let mut bytes = [0; NPAD_RING_SIZE];
        memory.read(0, &mut bytes).unwrap();
        assert_eq!(word(&bytes, 24), 1);
        assert_eq!(word(&bytes, HEADER_SIZE), 1);
        assert_eq!(word(&bytes, HEADER_SIZE + 16), 0);
    }
}
