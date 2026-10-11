// SPDX-License-Identifier: GPL-3.0-or-later
//! Service-owned storage retained independently by handles and guest mappings.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::{HorizonError, HorizonResult};

#[derive(Debug)]
struct Storage {
    bytes: Vec<u8>,
}

/// Cloning retains the same storage; it never snapshots the published bytes.
#[derive(Debug, Clone)]
pub struct SharedMemory {
    storage: Arc<RwLock<Storage>>,
    size: usize,
    revision: Arc<AtomicU64>,
    guest_permission: crate::memory::MemoryPermission,
}

impl PartialEq for SharedMemory {
    fn eq(&self, other: &Self) -> bool {
        self.same_object(other)
    }
}

impl Eq for SharedMemory {}

impl SharedMemory {
    pub fn new(size: usize) -> HorizonResult<Self> {
        Self::with_guest_permission(size, crate::memory::MemoryPermission::READ_WRITE)
    }

    pub fn with_guest_permission(
        size: usize,
        guest_permission: crate::memory::MemoryPermission,
    ) -> HorizonResult<Self> {
        if guest_permission != crate::memory::MemoryPermission::READ
            && guest_permission != crate::memory::MemoryPermission::READ_WRITE
        {
            return Err(HorizonError::PermissionDenied);
        }
        if size == 0 || size % 4096 != 0 {
            return Err(HorizonError::InvalidState(
                "shared memory size must be nonzero and page aligned".into(),
            ));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| HorizonError::InvalidState("shared memory allocation failed".into()))?;
        bytes.resize(size, 0);
        Ok(Self {
            storage: Arc::new(RwLock::new(Storage { bytes })),
            size,
            revision: Arc::new(AtomicU64::new(0)),
            guest_permission,
        })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn guest_permission(&self) -> crate::memory::MemoryPermission {
        self.guest_permission
    }

    pub fn same_object(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.storage, &other.storage)
    }

    fn checked_range(&self, offset: usize, length: usize) -> HorizonResult<std::ops::Range<usize>> {
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= self.size)
            .ok_or(HorizonError::InvalidAddress(offset as u64))?;
        Ok(offset..end)
    }

    pub fn read(&self, offset: usize, output: &mut [u8]) -> HorizonResult<()> {
        let range = self.checked_range(offset, output.len())?;
        let storage = self.storage.read().map_err(|_| {
            HorizonError::InvalidState("shared memory storage lock poisoned".into())
        })?;
        output.copy_from_slice(&storage.bytes[range]);
        Ok(())
    }

    /// Host/service publication. Guest permissions must be checked by the mapping layer.
    pub fn write(&self, offset: usize, input: &[u8]) -> HorizonResult<()> {
        let range = self.checked_range(offset, input.len())?;
        let mut storage = self.storage.write().map_err(|_| {
            HorizonError::InvalidState("shared memory storage lock poisoned".into())
        })?;
        storage.bytes[range].copy_from_slice(input);
        // Keep the revision and bytes under one lock so observers cannot see a torn update.
        if !input.is_empty() {
            self.revision.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Atomically publish disjoint fields under one storage lock. Validate the
    /// entire batch before changing bytes, including later out-of-range fields.
    pub fn write_batch(&self, writes: &[(usize, &[u8])]) -> HorizonResult<()> {
        for (index, (offset, input)) in writes.iter().enumerate() {
            let range = self.checked_range(*offset, input.len())?;
            for (other_offset, other_input) in &writes[..index] {
                let other = self.checked_range(*other_offset, other_input.len())?;
                if !range.is_empty()
                    && !other.is_empty()
                    && range.start < other.end
                    && other.start < range.end
                {
                    return Err(HorizonError::InvalidState(
                        "shared publication fields overlap".into(),
                    ));
                }
            }
        }
        let mut storage = self.storage.write().map_err(|_| {
            HorizonError::InvalidState("shared memory storage lock poisoned".into())
        })?;
        for (offset, input) in writes {
            storage.bytes[*offset..*offset + input.len()].copy_from_slice(input);
        }
        if writes.iter().any(|(_, input)| !input.is_empty()) {
            self.revision.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_references_share_publication_and_survive_owner_drop() {
        let owner = SharedMemory::new(0x40000).unwrap();
        let handle = owner.clone();
        let mapping = handle.clone();
        assert!(owner.same_object(&mapping));
        assert!(!owner.same_object(&SharedMemory::new(0x40000).unwrap()));
        let mut bytes = [0xff; 4];
        mapping.read(4094, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 4]);
        owner.write(4094, &[1, 2, 3, 4]).unwrap();
        drop(owner);
        drop(handle);
        mapping.read(4094, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);
        assert_eq!(mapping.revision(), 1);
    }

    #[test]
    fn rejected_access_preserves_storage_output_and_revision() {
        for size in [0, 1, 4095, 4097] {
            assert!(SharedMemory::new(size).is_err());
        }
        let memory = SharedMemory::new(4096).unwrap();
        memory.write(4095, &[7]).unwrap();
        assert!(memory.write(4095, &[8, 9]).is_err());
        assert!(memory.write(usize::MAX, &[8]).is_err());
        let mut output = [3; 2];
        assert!(memory.read(4095, &mut output).is_err());
        assert_eq!(output, [3; 2]);
        memory.read(4095, &mut output[..1]).unwrap();
        assert_eq!(output[0], 7);
        memory.write(4096, &[]).unwrap();
        assert_eq!(memory.revision(), 1);
    }
}
