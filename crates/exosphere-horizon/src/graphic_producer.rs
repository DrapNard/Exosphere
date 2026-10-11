// SPDX-License-Identifier: GPL-3.0-or-later
//! Display-owned Binder producer identity and its native-window export.

use crate::{HorizonError, HorizonResult, buffer_queue::BufferQueue, shared_memory::SharedMemory};
use std::collections::BTreeMap;

/// Parcel header, flattened native-window object and its payload-relative offset.
pub const NATIVE_WINDOW_SIZE: usize = 16 + 0x28 + 4;

#[derive(Debug, Clone, Copy)]
pub enum ReferenceKind {
    Weak,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProducerConnection {
    pub api: u32,
    pub controlled_by_application: bool,
}

#[derive(Debug)]
pub struct GraphicProducer {
    pub id: i32,
    pub layer: u64,
    pub owner: u64,
    /// Slots are initially unattached: graphics allocation must supply actual backing.
    pub queue: BufferQueue<SharedMemory>,
    connection: Option<ProducerConnection>,
    layer_attached: bool,
    weak_references: u32,
    strong_references: u32,
}

impl GraphicProducer {
    pub fn connection(&self) -> Option<ProducerConnection> {
        self.connection
    }

    pub(crate) fn connect(
        &mut self,
        api: u32,
        controlled_by_application: bool,
    ) -> HorizonResult<()> {
        self.queue.connect()?;
        self.connection = Some(ProducerConnection {
            api,
            controlled_by_application,
        });
        Ok(())
    }
    pub fn reference_counts(&self) -> (u32, u32) {
        (self.weak_references, self.strong_references)
    }

    fn is_retained(&self) -> bool {
        self.layer_attached || self.weak_references != 0 || self.strong_references != 0
    }
}

#[derive(Debug)]
pub struct ProducerRegistry {
    producers: BTreeMap<i32, GraphicProducer>,
    next_id: i32,
}

impl Default for ProducerRegistry {
    fn default() -> Self {
        Self {
            producers: BTreeMap::new(),
            next_id: 1,
        }
    }
}

impl ProducerRegistry {
    pub(crate) fn create(&mut self, layer: u64, owner: u64) -> HorizonResult<i32> {
        if layer == 0 || owner == 0 {
            return Err(HorizonError::InvalidState(
                "producer requires layer and application identity".into(),
            ));
        }
        if let Some(producer) = self
            .producers
            .values()
            .find(|producer| producer.layer_attached && producer.layer == layer)
        {
            if producer.owner != owner {
                return Err(HorizonError::PermissionDenied);
            }
            return Ok(producer.id);
        }
        let next = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| HorizonError::InvalidState("producer identity exhausted".into()))?;
        let queue = BufferQueue::new(64)?;
        let id = self.next_id;
        self.producers.insert(
            id,
            GraphicProducer {
                id,
                layer,
                owner,
                queue,
                connection: None,
                layer_attached: true,
                weak_references: 0,
                strong_references: 0,
            },
        );
        self.next_id = next;
        Ok(id)
    }

    pub fn get_mut(&mut self, id: i32, owner: u64) -> HorizonResult<&mut GraphicProducer> {
        self.producers
            .get_mut(&id)
            .filter(|producer| producer.owner == owner)
            .ok_or_else(|| HorizonError::InvalidState("unknown or foreign graphic producer".into()))
    }

    pub fn native_window(&self, id: i32, owner: u64) -> HorizonResult<Vec<u8>> {
        self.producers
            .get(&id)
            .filter(|producer| producer.owner == owner)
            .ok_or_else(|| {
                HorizonError::InvalidState("unknown or foreign graphic producer".into())
            })?;
        // Switch VI's 0x28-byte native-window payload. The service marker is not
        // the guest PID; the third word resolves to this registry's actual producer.
        let mut payload = [0; 0x28];
        payload[..4].copy_from_slice(&2u32.to_le_bytes());
        payload[4..8].copy_from_slice(&1u32.to_le_bytes());
        payload[8..12].copy_from_slice(&id.to_le_bytes());
        payload[24..32].copy_from_slice(b"dispdrv\0");
        // The flattened producer starts at payload offset zero. Publish that
        // offset in the object table so the SDK's object reader can find it.
        crate::parcel::encode(&payload, &0u32.to_le_bytes())
    }

    pub fn release_layer(&mut self, layer: u64) {
        // Removing the layer drops its service-owned reference, not references
        // explicitly held by Binder clients. Their queue backing must survive.
        self.producers.retain(|_, producer| {
            if producer.layer == layer {
                producer.layer_attached = false;
            }
            producer.is_retained()
        });
    }

    pub fn adjust_reference(
        &mut self,
        id: i32,
        owner: u64,
        kind: ReferenceKind,
        delta: i32,
    ) -> HorizonResult<()> {
        let producer = self.get_mut(id, owner)?;
        let count = match kind {
            ReferenceKind::Weak => &mut producer.weak_references,
            ReferenceKind::Strong => &mut producer.strong_references,
        };
        // Widen before adding so i32::MIN and either overflow direction reject
        // transactionally without modifying the other count or the registry.
        let next = u32::try_from(i64::from(*count) + i64::from(delta)).map_err(|_| {
            HorizonError::InvalidState("invalid Binder reference adjustment".into())
        })?;
        *count = next;
        if !producer.is_retained() {
            self.producers.remove(&id);
        }
        Ok(())
    }

    pub fn release_owner(&mut self, owner: u64) {
        self.producers.retain(|_, producer| producer.owner != owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_retain_detached_queue_and_reject_invalid_adjustments_atomically() {
        let mut registry = ProducerRegistry::default();
        let id = registry.create(10, 1).unwrap();
        let backing = SharedMemory::new(4096).unwrap();
        backing.write(0, &[42]).unwrap();
        registry
            .get_mut(id, 1)
            .unwrap()
            .queue
            .attach(0, backing)
            .unwrap();
        registry
            .adjust_reference(id, 1, ReferenceKind::Weak, 1)
            .unwrap();
        registry
            .adjust_reference(id, 1, ReferenceKind::Strong, 1)
            .unwrap();
        assert!(
            registry
                .adjust_reference(id, 2, ReferenceKind::Weak, 1)
                .is_err()
        );
        assert!(
            registry
                .adjust_reference(id, 1, ReferenceKind::Weak, -2)
                .is_err()
        );
        assert!(
            registry
                .adjust_reference(id, 1, ReferenceKind::Strong, i32::MIN)
                .is_err()
        );
        assert_eq!(registry.get_mut(id, 1).unwrap().reference_counts(), (1, 1));
        registry.release_layer(10);
        let replacement = registry.create(10, 1).unwrap();
        assert_ne!(id, replacement);
        let queue = &mut registry.get_mut(id, 1).unwrap().queue;
        queue.connect().unwrap();
        assert_eq!(queue.dequeue().unwrap(), Some(0));
        let mut byte = [0];
        queue
            .producer_buffer(0)
            .unwrap()
            .read(0, &mut byte)
            .unwrap();
        assert_eq!(byte, [42]);
        registry
            .adjust_reference(id, 1, ReferenceKind::Strong, -1)
            .unwrap();
        assert_eq!(registry.get_mut(id, 1).unwrap().reference_counts(), (1, 0));
        registry
            .adjust_reference(id, 1, ReferenceKind::Weak, -1)
            .unwrap();
        assert!(registry.get_mut(id, 1).is_err());
        registry
            .adjust_reference(replacement, 1, ReferenceKind::Weak, i32::MAX)
            .unwrap();
        registry
            .adjust_reference(replacement, 1, ReferenceKind::Weak, i32::MAX)
            .unwrap();
        assert!(
            registry
                .adjust_reference(replacement, 1, ReferenceKind::Weak, 2)
                .is_err()
        );
        assert_eq!(
            registry.get_mut(replacement, 1).unwrap().reference_counts(),
            (u32::MAX - 1, 0)
        );
        registry.release_owner(1);
        assert!(registry.get_mut(replacement, 1).is_err());
    }

    #[test]
    fn exported_identity_resolves_to_owned_queue_and_is_never_recycled() {
        let mut registry = ProducerRegistry::default();
        let id = registry.create(10, 1).unwrap();
        assert_eq!(registry.create(10, 1).unwrap(), id);
        assert!(registry.create(10, 2).is_err());
        let bytes = registry.native_window(id, 1).unwrap();
        let parcel = crate::parcel::Parcel::parse(&bytes).unwrap();
        assert_eq!(bytes.len(), NATIVE_WINDOW_SIZE);
        assert_eq!(
            &bytes[..16],
            &[40, 0, 0, 0, 16, 0, 0, 0, 4, 0, 0, 0, 56, 0, 0, 0]
        );
        assert_eq!(parcel.objects, 0u32.to_le_bytes());
        assert_eq!(parcel.payload.len(), 0x28);
        assert_eq!(
            i32::from_le_bytes(parcel.payload[8..12].try_into().unwrap()),
            id
        );
        assert_eq!(&parcel.payload[24..32], b"dispdrv\0");
        assert!(registry.native_window(id, 2).is_err());
        let queue = &mut registry.get_mut(id, 1).unwrap().queue;
        queue.connect().unwrap();
        assert_eq!(queue.dequeue().unwrap(), None);
        queue.attach(0, SharedMemory::new(4096).unwrap()).unwrap();
        assert_eq!(queue.dequeue().unwrap(), Some(0));
        registry.release_owner(1);
        assert!(registry.get_mut(id, 1).is_err());
        assert_ne!(registry.create(10, 1).unwrap(), id);
    }
}
