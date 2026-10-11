// SPDX-License-Identifier: GPL-3.0-or-later
//! Device access to live guest backing, protected until the last lease drops.

use super::*;
use std::sync::{Arc, Weak};

/// Opaque identity of one device pin, shared by cloned leases. Keeping this
/// token does not keep guest memory pinned. A later pin of the same CPU range
/// has a different identity and must not inherit cached native resources.
#[derive(Debug, Clone)]
pub struct DeviceMemoryLeaseIdentity(Weak<()>);

impl PartialEq for DeviceMemoryLeaseIdentity {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for DeviceMemoryLeaseIdentity {}

impl DeviceMemoryLeaseIdentity {
    pub fn is_live(&self) -> bool {
        self.0.strong_count() != 0
    }
}

#[derive(Debug, Clone)]
pub struct DeviceMemoryLease {
    identity: Arc<()>,
    _lifetime: Arc<()>,
    base: u64,
    size: usize,
    permission: MemoryPermission,
}

impl DeviceMemoryLease {
    pub fn permission(&self) -> MemoryPermission {
        self.permission
    }
    pub fn allocation_identity(&self) -> DeviceMemoryLeaseIdentity {
        DeviceMemoryLeaseIdentity(Arc::downgrade(&self._lifetime))
    }
    pub fn size(&self) -> usize {
        self.size
    }
    pub fn is_exclusively_owned(&self) -> bool {
        Arc::strong_count(&self._lifetime) == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_identity_tracks_lease_clones_without_retaining_guest_memory() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let lease = memory
            .pin_device_memory(0x1000, PAGE_SIZE, MemoryPermission::READ_WRITE)
            .unwrap();
        let identity = lease.allocation_identity();
        let alias = lease.clone();
        assert_eq!(identity, alias.allocation_identity());
        drop(lease);
        drop(alias);
        memory.unmap(0x1000, PAGE_SIZE).unwrap();
        memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let replacement = memory
            .pin_device_memory(0x1000, PAGE_SIZE, MemoryPermission::READ_WRITE)
            .unwrap();
        assert_ne!(identity, replacement.allocation_identity());
        drop(replacement);
        memory.unmap(0x1000, PAGE_SIZE).unwrap();
    }

    #[test]
    fn device_lease_reads_live_backing_and_retains_until_last_reference() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x1000,
                3 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let lease = memory
            .pin_device_memory(0x2000, PAGE_SIZE, MemoryPermission::READ_WRITE)
            .unwrap();
        let retained = lease.clone();
        memory.write(0x2000, &[41]).unwrap();
        let mut bytes = [0];
        memory.read_device_memory(&lease, 0, &mut bytes).unwrap();
        assert_eq!(bytes, [41]);
        let epoch = memory.write_epoch();
        memory.write_device_memory(&lease, 0, &[42]).unwrap();
        assert!(memory.write_epoch() > epoch);
        memory.read(0x2000, &mut bytes).unwrap();
        assert_eq!(bytes, [42]);
        assert!(memory.resize_heap(0x1000, PAGE_SIZE).is_err());
        assert!(
            memory
                .protect(0x1000, 3 * PAGE_SIZE, MemoryPermission::READ)
                .is_err()
        );
        assert!(memory.unmap(0x1000, 3 * PAGE_SIZE).is_err());
        assert!(memory.read_device_memory(&lease, 4096, &mut bytes).is_err());
        let mut snapshot = memory.clone();
        assert!(snapshot.read_device_memory(&lease, 0, &mut bytes).is_err());
        snapshot.unmap(0x1000, 3 * PAGE_SIZE).unwrap();
        drop(lease);
        assert!(memory.unmap(0x1000, 3 * PAGE_SIZE).is_err());
        drop(retained);
        memory.resize_heap(0x1000, PAGE_SIZE).unwrap();
    }

    #[test]
    fn device_lease_rejects_invalid_ranges_permissions_and_readonly_writes() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        assert!(
            memory
                .pin_device_memory(0x1001, PAGE_SIZE, MemoryPermission::READ)
                .is_err()
        );
        assert!(
            memory
                .pin_device_memory(0x1000, 0, MemoryPermission::READ)
                .is_err()
        );
        assert!(
            memory
                .pin_device_memory(0x1000, PAGE_SIZE, MemoryPermission::EXECUTE)
                .is_err()
        );
        assert!(
            memory
                .pin_device_memory(0x1000, 2 * PAGE_SIZE, MemoryPermission::READ)
                .is_err()
        );
        let lease = memory
            .pin_device_memory(0x1000, PAGE_SIZE, MemoryPermission::READ)
            .unwrap();
        assert!(memory.write_device_memory(&lease, 0, &[1]).is_err());
        assert!(
            memory
                .reserve_transfer(0x1000, PAGE_SIZE, MemoryPermission::NONE)
                .is_err()
        );
        assert!(memory.load_bytes(0x1000, &[1]).is_err());
        memory.write(0x1000, &[2]).unwrap();
        drop(lease);
        memory
            .reserve_transfer(0x1000, PAGE_SIZE, MemoryPermission::NONE)
            .unwrap();
    }
}

#[derive(Debug, Clone)]
pub(super) struct DeviceReservation {
    lifetime: Weak<()>,
    base: u64,
    size: u64,
}

impl AddressSpace {
    pub fn pin_device_memory(
        &mut self,
        base: u64,
        size: u64,
        permission: MemoryPermission,
    ) -> HorizonResult<DeviceMemoryLease> {
        if size == 0
            || base % PAGE_SIZE != 0
            || size % PAGE_SIZE != 0
            || !matches!(
                permission,
                MemoryPermission::READ | MemoryPermission::READ_WRITE
            )
        {
            return Err(HorizonError::InvalidAddress(base));
        }
        let host_size = usize::try_from(size).map_err(|_| HorizonError::InvalidAddress(base))?;
        self.validate_access(base, host_size, permission)?;
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        if self
            .transfers
            .iter()
            .any(|reservation| base < reservation.base + reservation.size && end > reservation.base)
        {
            return Err(HorizonError::InvalidState(
                "device range overlaps transfer memory".into(),
            ));
        }
        self.reject_stack_alias_overlap(base, size)?;
        let region = self
            .regions
            .iter()
            .find(|region| region.contains_range(base, host_size))
            .unwrap();
        if !matches!(region.kind, MemoryKind::Heap | MemoryKind::Data)
            || region.alias_source.is_some()
        {
            return Err(HorizonError::PermissionDenied);
        }
        let lifetime = Arc::new(());
        self.device_leases
            .retain(|reservation| reservation.lifetime.strong_count() != 0);
        self.device_leases.push(DeviceReservation {
            lifetime: Arc::downgrade(&lifetime),
            base,
            size,
        });
        Ok(DeviceMemoryLease {
            identity: self.device_identity.clone(),
            _lifetime: lifetime,
            base,
            size: host_size,
            permission,
        })
    }

    fn device_address(
        &self,
        lease: &DeviceMemoryLease,
        offset: usize,
        length: usize,
    ) -> HorizonResult<u64> {
        if !Arc::ptr_eq(&lease.identity, &self.device_identity)
            || offset
                .checked_add(length)
                .is_none_or(|end| end > lease.size)
        {
            return Err(HorizonError::InvalidAddress(lease.base));
        }
        lease
            .base
            .checked_add(offset as u64)
            .ok_or(HorizonError::InvalidAddress(lease.base))
    }

    pub(crate) fn validate_device_access(
        &self,
        lease: &DeviceMemoryLease,
        offset: usize,
        size: usize,
        permission: MemoryPermission,
    ) -> HorizonResult<()> {
        if !lease.permission.contains(permission) {
            return Err(HorizonError::PermissionDenied);
        }
        let address = self.device_address(lease, offset, size)?;
        self.validate_access(address, size, permission)
    }

    pub fn read_device_memory(
        &self,
        lease: &DeviceMemoryLease,
        offset: usize,
        output: &mut [u8],
    ) -> HorizonResult<()> {
        let address = self.device_address(lease, offset, output.len())?;
        self.read(address, output)
    }

    pub fn write_device_memory(
        &mut self,
        lease: &DeviceMemoryLease,
        offset: usize,
        input: &[u8],
    ) -> HorizonResult<()> {
        if !lease.permission.contains(MemoryPermission::WRITE) {
            return Err(HorizonError::PermissionDenied);
        }
        let address = self.device_address(lease, offset, input.len())?;
        self.write(address, input)
    }

    pub(super) fn reject_device_overlap(&self, base: u64, size: u64) -> HorizonResult<()> {
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        if self.device_leases.iter().any(|reservation| {
            reservation.lifetime.strong_count() != 0
                && base < reservation.base + reservation.size
                && end > reservation.base
        }) {
            return Err(HorizonError::InvalidState(
                "guest range is retained by a device".into(),
            ));
        }
        Ok(())
    }
}
