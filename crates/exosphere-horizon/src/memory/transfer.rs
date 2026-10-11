// SPDX-License-Identifier: GPL-3.0-or-later
//! Transfer reservations restrict access without copying or splitting heap backing.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct TransferReservation {
    pub base: u64,
    pub size: u64,
    pub permission: MemoryPermission,
}

impl AddressSpace {
    pub(crate) fn release_unreferenced_transfers(&mut self, retained: &[(u64, u64)]) {
        let old_count = self.transfers.len();
        self.transfers
            .retain(|reservation| retained.contains(&(reservation.base, reservation.size)));
        if self.transfers.len() != old_count {
            self.write_epoch = self.write_epoch.wrapping_add(1);
        }
    }
    pub fn reserve_transfer(
        &mut self,
        base: u64,
        size: u64,
        permission: MemoryPermission,
    ) -> HorizonResult<()> {
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        if base % PAGE_SIZE != 0 || size == 0 || size % PAGE_SIZE != 0 || end > 1 << 39 {
            return Err(HorizonError::InvalidAddress(base));
        }
        if !matches!(
            permission,
            MemoryPermission::NONE | MemoryPermission::READ | MemoryPermission::READ_WRITE
        ) {
            return Err(HorizonError::PermissionDenied);
        }
        let size_usize = usize::try_from(size).map_err(|_| HorizonError::InvalidAddress(base))?;
        let region = self
            .regions
            .iter()
            .find(|region| region.contains_range(base, size_usize))
            .ok_or(HorizonError::InvalidAddress(base))?;
        // Heap backing is currently the only implemented transferable state.
        if region.kind != MemoryKind::Heap || region.permission != MemoryPermission::READ_WRITE {
            return Err(HorizonError::PermissionDenied);
        }
        self.reject_transfer_overlap(base, size)?;
        self.reject_stack_alias_overlap(base, size)?;
        self.transfers.try_reserve(1).map_err(|_| {
            HorizonError::InvalidState("transfer metadata allocation failed".into())
        })?;
        self.transfers.push(TransferReservation {
            base,
            size,
            permission,
        });
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    pub fn release_transfer(&mut self, base: u64, size: u64) -> HorizonResult<()> {
        let index = self
            .transfers
            .iter()
            .position(|reservation| reservation.base == base && reservation.size == size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        self.transfers.remove(index);
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    pub(super) fn reject_transfer_overlap(&self, base: u64, size: u64) -> HorizonResult<()> {
        self.reject_device_overlap(base, size)?;
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        if self
            .transfers
            .iter()
            .any(|reservation| base < reservation.base + reservation.size && end > reservation.base)
        {
            return Err(HorizonError::InvalidState(
                "memory range is locked for transfer".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn check_transfer_access(
        &self,
        base: u64,
        size: usize,
        permission: MemoryPermission,
    ) -> HorizonResult<()> {
        let end = base
            .checked_add(size as u64)
            .ok_or(HorizonError::InvalidAddress(base))?;
        for reservation in &self.transfers {
            if base < reservation.base + reservation.size
                && end > reservation.base
                && !reservation.permission.contains(permission)
            {
                return Err(HorizonError::PermissionDenied);
            }
        }
        Ok(())
    }

    /// Effective owner permission and Locked attribute for QueryMemory.
    pub fn transfer_metadata(&self, address: u64) -> Option<(MemoryPermission, u32)> {
        self.transfers
            .iter()
            .find(|reservation| {
                address >= reservation.base && address < reservation.base + reservation.size
            })
            .map(|reservation| (reservation.permission, 1))
    }

    pub(super) fn transfer_query_bounds(&self, address: u64, region: &MemoryRegion) -> (u64, u64) {
        let mut start = region.base;
        let mut end = region.end();
        for reservation in &self.transfers {
            for boundary in [reservation.base, reservation.base + reservation.size] {
                if boundary <= address {
                    start = start.max(boundary);
                } else {
                    end = end.min(boundary);
                }
            }
        }
        (start, end - start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservations_restrict_access_and_preserve_backing_until_release() {
        for permission in [
            MemoryPermission::NONE,
            MemoryPermission::READ,
            MemoryPermission::READ_WRITE,
        ] {
            let mut memory = AddressSpace::default();
            memory
                .map(
                    0x1000,
                    4 * PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
            memory.write(0x2000, &[42]).unwrap();
            let charged = memory.used_memory_size().unwrap();
            memory
                .reserve_transfer(0x2000, PAGE_SIZE, permission)
                .unwrap();
            assert_eq!(memory.used_memory_size().unwrap(), charged);
            assert_eq!(memory.transfer_metadata(0x2000), Some((permission, 1)));
            assert_eq!(
                (
                    memory.query(0x1000, 1 << 39).unwrap().0,
                    memory.query(0x1000, 1 << 39).unwrap().1
                ),
                (0x1000, PAGE_SIZE)
            );
            assert_eq!(
                (
                    memory.query(0x2000, 1 << 39).unwrap().0,
                    memory.query(0x2000, 1 << 39).unwrap().1
                ),
                (0x2000, PAGE_SIZE)
            );
            assert_eq!(
                (
                    memory.query(0x3000, 1 << 39).unwrap().0,
                    memory.query(0x3000, 1 << 39).unwrap().1
                ),
                (0x3000, 2 * PAGE_SIZE)
            );
            let mut byte = [0];
            assert_eq!(
                memory.read(0x2000, &mut byte).is_ok(),
                permission.contains(MemoryPermission::READ)
            );
            assert_eq!(
                memory.write(0x2000, &[42]).is_ok(),
                permission.contains(MemoryPermission::WRITE)
            );
            assert_eq!(
                memory
                    .validate_access(0x1fff, 2, MemoryPermission::WRITE)
                    .is_ok(),
                permission.contains(MemoryPermission::WRITE)
            );
            memory.write(0x1000, &[99]).unwrap();
            assert!(
                memory
                    .reserve_transfer(0x2000, PAGE_SIZE, permission)
                    .is_err()
            );
            assert!(memory.unmap(0x1000, 4 * PAGE_SIZE).is_err());
            assert!(
                memory
                    .protect(0x1000, 4 * PAGE_SIZE, MemoryPermission::READ)
                    .is_err()
            );
            assert!(memory.resize_heap(0x1000, PAGE_SIZE).is_err());
            assert!(memory.load_bytes(0x2000, &[5]).is_err());
            memory.release_transfer(0x2000, PAGE_SIZE).unwrap();
            memory.read(0x2000, &mut byte).unwrap();
            assert_eq!(byte, [42]);
            memory.write(0x2000, &[43]).unwrap();
            assert_eq!(memory.transfer_metadata(0x2000), None);
            assert!(memory.release_transfer(0x2000, PAGE_SIZE).is_err());
        }
    }
    #[test]
    fn invalid_reservations_do_not_change_permissions_or_epochs() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x1000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let epoch = memory.write_epoch();
        for (base, size, permission) in [
            (0x1001, PAGE_SIZE, MemoryPermission::NONE),
            (0x1000, 0, MemoryPermission::NONE),
            (0x1000, 1, MemoryPermission::NONE),
            (0x2000, 2 * PAGE_SIZE, MemoryPermission::NONE),
            (0x1000, PAGE_SIZE, MemoryPermission::WRITE),
            (u64::MAX - 0xfff, PAGE_SIZE, MemoryPermission::NONE),
        ] {
            assert!(memory.reserve_transfer(base, size, permission).is_err());
            assert_eq!(memory.write_epoch(), epoch);
            memory
                .validate_access(0x1000, 1, MemoryPermission::READ_WRITE)
                .unwrap();
        }
    }
}
