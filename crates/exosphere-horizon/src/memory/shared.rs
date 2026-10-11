// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared mappings retain storage rather than copying service publications.

use super::*;
use crate::shared_memory::SharedMemory;

impl AddressSpace {
    pub fn map_shared(
        &mut self,
        base: u64,
        memory: &SharedMemory,
        permission: MemoryPermission,
    ) -> HorizonResult<()> {
        let size = memory.size() as u64;
        if permission != memory.guest_permission() {
            return Err(HorizonError::PermissionDenied);
        }
        if base % PAGE_SIZE != 0 {
            return Err(HorizonError::InvalidAddress(base));
        }
        if permission != MemoryPermission::READ && permission != MemoryPermission::READ_WRITE {
            return Err(HorizonError::PermissionDenied);
        }
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        if self.regions.iter().any(|r| base < r.end() && end > r.base) {
            return Err(HorizonError::MemoryOverlap);
        }
        self.regions
            .try_reserve(1)
            .map_err(|_| HorizonError::InvalidState("shared mapping allocation failed".into()))?;
        self.regions.push(MemoryRegion {
            base,
            size,
            permission,
            kind: MemoryKind::Shared,
            bytes: backing::Backing::Shared(memory.clone()),
            alias_source: None,
        });
        self.regions.sort_by_key(|r| r.base);
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    /// An equally sized but unrelated shared object cannot remove this mapping.
    pub fn unmap_shared(&mut self, base: u64, memory: &SharedMemory) -> HorizonResult<()> {
        let index = self.regions.iter().position(|region| {
            region.base == base && region.size == memory.size() as u64
                && matches!(&region.bytes, backing::Backing::Shared(mapped) if mapped.same_object(memory))
        }).ok_or(HorizonError::InvalidAddress(base))?;
        self.regions.remove(index);
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    pub(super) fn reject_shared_overlap(&self, base: u64, size: u64) -> HorizonResult<()> {
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        if self.regions.iter().any(|region| {
            matches!(region.bytes, backing::Backing::Shared(_))
                && base < region.end()
                && end > region.base
        }) {
            return Err(HorizonError::PermissionDenied);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publications_reach_readonly_guest_and_cloned_address_space() {
        let storage = SharedMemory::with_guest_permission(4096, MemoryPermission::READ).unwrap();
        let mut guest = AddressSpace::default();
        guest
            .map_shared(0x1000, &storage, MemoryPermission::READ)
            .unwrap();
        let clone = guest.clone();
        let epoch = guest.write_epoch();
        storage.write(0, &[7]).unwrap();
        assert_ne!(guest.write_epoch(), epoch);
        for space in [&guest, &clone] {
            let mut byte = [0];
            space.read(0x1000, &mut byte).unwrap();
            assert_eq!(byte, [7]);
            assert!(space.fetch_instruction(0x1000).is_err());
        }
        assert!(guest.write(0x1000, &[8]).is_err());
        assert!(guest.load_bytes(0x1000, &[8]).is_err());
        assert!(
            guest
                .protect(0x1000, 4096, MemoryPermission::READ_WRITE)
                .is_err()
        );
        assert!(guest.unmap(0x1000, 4096).is_err());
        assert_eq!(guest.used_memory_size().unwrap(), 0);
        let wrong = SharedMemory::new(4096).unwrap();
        assert!(guest.unmap_shared(0x1000, &wrong).is_err());
        guest.unmap_shared(0x1000, &storage).unwrap();
        let mut byte = [0];
        clone.read(0x1000, &mut byte).unwrap();
        assert_eq!(byte, [7]);
    }

    #[test]
    fn writable_mappings_share_bytes_and_invalid_maps_leave_state_unchanged() {
        let storage = SharedMemory::new(4096).unwrap();
        let mut guest = AddressSpace::default();
        for (base, permission) in [
            (1, MemoryPermission::READ),
            (0x1000, MemoryPermission::EXECUTE),
            (u64::MAX - 4095, MemoryPermission::READ),
        ] {
            assert!(guest.map_shared(base, &storage, permission).is_err());
            assert!(guest.regions().is_empty());
        }
        guest
            .map_shared(0x1000, &storage, MemoryPermission::READ_WRITE)
            .unwrap();
        let epoch = guest.write_epoch();
        assert!(
            guest
                .map_shared(0x1000, &storage, MemoryPermission::READ)
                .is_err()
        );
        assert_eq!(guest.write_epoch(), epoch);
        guest.write(0x1000, &[9]).unwrap();
        let mut byte = [0];
        storage.read(0, &mut byte).unwrap();
        assert_eq!(byte, [9]);
    }
}
