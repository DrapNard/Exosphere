// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Generation-checked Horizon handle table.
//!
//! Reusing a table slot increments its generation. This catches stale handles early instead of
//! accidentally referring to a newly-created kernel object that happens to occupy the same slot.

use crate::error::{HorizonError, HorizonResult};

const INDEX_BITS: u32 = 20;
const INDEX_MASK: u32 = (1 << INDEX_BITS) - 1;
const GENERATION_MASK: u32 = (1 << (32 - INDEX_BITS)) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelObject {
    Process(u64),
    Thread(u64),
    Session(u64),
    Event(u64),
    ReadableEvent(crate::event::SharedEvent),
    SharedMemory(crate::shared_memory::SharedMemory),
    TransferMemory { owner: u64, base: u64, size: u64 },
    Custom { kind: String, id: u64 },
}

#[derive(Debug, Clone)]
struct Slot {
    generation: u16,
    object: Option<KernelObject>,
}

#[derive(Debug, Clone)]
pub struct HandleTable {
    slots: Vec<Slot>,
    max_slots: usize,
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new(0x4000)
    }
}

impl HandleTable {
    pub fn objects(&self) -> impl Iterator<Item = &KernelObject> {
        self.slots.iter().filter_map(|slot| slot.object.as_ref())
    }
    pub fn contains_object(&self, object: &KernelObject) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.object.as_ref() == Some(object))
    }
    pub fn new(max_slots: usize) -> Self {
        Self {
            slots: Vec::new(),
            max_slots,
        }
    }

    pub fn insert(&mut self, object: KernelObject) -> HorizonResult<Handle> {
        if let Some((index, slot)) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.object.is_none())
        {
            slot.object = Some(object);
            return Ok(encode(index, slot.generation));
        }
        if self.slots.len() >= self.max_slots || self.slots.len() > INDEX_MASK as usize {
            return Err(HorizonError::HandleTableFull);
        }
        let index = self.slots.len();
        self.slots.push(Slot {
            generation: 1,
            object: Some(object),
        });
        Ok(encode(index, 1))
    }

    pub fn get(&self, handle: Handle) -> HorizonResult<&KernelObject> {
        let (index, generation) = decode(handle);
        let slot = self
            .slots
            .get(index)
            .ok_or(HorizonError::InvalidHandle(handle.0))?;
        if slot.generation != generation || slot.object.is_none() {
            return Err(HorizonError::InvalidHandle(handle.0));
        }
        Ok(slot.object.as_ref().expect("checked above"))
    }

    pub fn close(&mut self, handle: Handle) -> HorizonResult<KernelObject> {
        let (index, generation) = decode(handle);
        let slot = self
            .slots
            .get_mut(index)
            .ok_or(HorizonError::InvalidHandle(handle.0))?;
        if slot.generation != generation {
            return Err(HorizonError::InvalidHandle(handle.0));
        }
        let object = slot
            .object
            .take()
            .ok_or(HorizonError::InvalidHandle(handle.0))?;
        slot.generation = ((u32::from(slot.generation) + 1) & GENERATION_MASK) as u16;
        if slot.generation == 0 {
            slot.generation = 1;
        }
        Ok(object)
    }
}

fn encode(index: usize, generation: u16) -> Handle {
    Handle((u32::from(generation) << INDEX_BITS) | (index as u32 & INDEX_MASK))
}

fn decode(handle: Handle) -> (usize, u16) {
    (
        (handle.0 & INDEX_MASK) as usize,
        (handle.0 >> INDEX_BITS) as u16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_handle_is_rejected() {
        let mut table = HandleTable::default();
        let old = table.insert(KernelObject::Event(1)).unwrap();
        table.close(old).unwrap();
        let new = table.insert(KernelObject::Event(2)).unwrap();
        assert_ne!(old, new);
        assert!(table.get(old).is_err());
        assert_eq!(table.get(new).unwrap(), &KernelObject::Event(2));
    }
}
