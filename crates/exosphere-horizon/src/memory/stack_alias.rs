// SPDX-License-Identifier: GPL-3.0-or-later
//! Same-process stack mappings share the original sparse heap backing.

use super::*;

impl AddressSpace {
    pub fn map_stack_alias(
        &mut self,
        destination: u64,
        source: u64,
        size: u64,
    ) -> HorizonResult<()> {
        Self::validate_alias_range(destination, source, size)?;
        self.reject_transfer_overlap(source, size)
            .map_err(|_| HorizonError::PermissionDenied)?;
        self.reject_stack_alias_overlap(source, size)
            .map_err(|_| HorizonError::PermissionDenied)?;
        if self
            .regions
            .iter()
            .any(|region| destination < region.end() && destination + size > region.base)
        {
            return Err(HorizonError::MemoryOverlap);
        }
        let region = self
            .regions
            .iter()
            .find(|region| region.contains_range(source, size as usize))
            .ok_or(HorizonError::InvalidAddress(source))?;
        if region.kind != MemoryKind::Heap || region.alias_source.is_some() {
            return Err(HorizonError::InvalidState(
                "stack mapping source state is not implemented beyond heap".into(),
            ));
        }
        if region.permission != MemoryPermission::READ_WRITE {
            return Err(HorizonError::PermissionDenied);
        }
        self.regions.try_reserve(1).map_err(|_| {
            HorizonError::InvalidState("stack alias metadata allocation failed".into())
        })?;
        // No bytes are copied or allocated: every destination access resolves
        // into the source backing, bypassing only the owner's alias lock.
        self.regions.push(MemoryRegion {
            base: destination,
            size,
            permission: MemoryPermission::READ_WRITE,
            kind: MemoryKind::Stack,
            bytes: backing::Backing::Dense(Vec::new()),
            alias_source: Some(source),
        });
        self.regions.sort_by_key(|region| region.base);
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    pub fn unmap_stack_alias(
        &mut self,
        destination: u64,
        source: u64,
        size: u64,
    ) -> HorizonResult<()> {
        Self::validate_alias_range(destination, source, size)?;
        let index = self
            .regions
            .iter()
            .position(|region| {
                region.base == destination
                    && region.size == size
                    && region.alias_source == Some(source)
            })
            .ok_or(HorizonError::InvalidAddress(destination))?;
        // Exact pair matching is intentional until partial unmap/splitting is
        // implemented. Never restore access to still-mapped source pages.
        self.regions.remove(index);
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    fn validate_alias_range(destination: u64, source: u64, size: u64) -> HorizonResult<()> {
        for base in [destination, source] {
            let end = base
                .checked_add(size)
                .ok_or(HorizonError::InvalidAddress(base))?;
            if base % PAGE_SIZE != 0 || size == 0 || size % PAGE_SIZE != 0 || end > 1 << 39 {
                return Err(HorizonError::InvalidAddress(base));
            }
        }
        Ok(())
    }

    pub(super) fn resolve_backing(
        &self,
        address: u64,
        size: usize,
    ) -> HorizonResult<(usize, usize)> {
        let index = self
            .regions
            .iter()
            .position(|region| region.contains_range(address, size))
            .ok_or(HorizonError::InvalidAddress(address))?;
        let region = &self.regions[index];
        let physical_address = region
            .alias_source
            .map_or(address, |source| source + address - region.base);
        let physical_index = if region.alias_source.is_some() {
            self.regions
                .iter()
                .position(|region| {
                    region.alias_source.is_none() && region.contains_range(physical_address, size)
                })
                .ok_or(HorizonError::InvalidAddress(physical_address))?
        } else {
            index
        };
        let offset = usize::try_from(physical_address - self.regions[physical_index].base)
            .map_err(|_| HorizonError::InvalidAddress(physical_address))?;
        Ok((physical_index, offset))
    }

    pub(super) fn reject_stack_alias_overlap(&self, base: u64, size: u64) -> HorizonResult<()> {
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        for region in &self.regions {
            if let Some(source) = region.alias_source {
                if (base < source + region.size && end > source)
                    || (base < region.end() && end > region.base)
                {
                    return Err(HorizonError::InvalidState(
                        "memory range has a live stack alias".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn check_stack_source_access(&self, base: u64, size: usize) -> HorizonResult<()> {
        let end = base
            .checked_add(size as u64)
            .ok_or(HorizonError::InvalidAddress(base))?;
        for region in &self.regions {
            if let Some(source) = region.alias_source {
                if base < source + region.size && end > source {
                    return Err(HorizonError::PermissionDenied);
                }
            }
        }
        Ok(())
    }

    pub fn stack_source_metadata(&self, address: u64) -> Option<(MemoryPermission, u32)> {
        self.regions
            .iter()
            .find_map(|region| {
                region
                    .alias_source
                    .filter(|source| address >= *source && address < *source + region.size)
            })
            .map(|_| (MemoryPermission::NONE, 1))
    }

    pub(super) fn stack_source_query_bounds(
        &self,
        address: u64,
        mut base: u64,
        size: u64,
    ) -> (u64, u64) {
        let mut end = base + size;
        for region in &self.regions {
            if let Some(source) = region.alias_source {
                for boundary in [source, source + region.size] {
                    if boundary <= address {
                        base = base.max(boundary);
                    } else {
                        end = end.min(boundary);
                    }
                }
            }
        }
        (base, end - base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAP: u64 = 0x200000;
    const STACK: u64 = 0x700000;

    fn memory() -> AddressSpace {
        let mut memory = AddressSpace::default();
        memory
            .map(
                HEAP,
                4 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        memory
    }

    #[test]
    fn alias_shares_backing_without_charging_and_restores_source() {
        let mut memory = memory();
        let source = HEAP + PAGE_SIZE;
        memory.write(source + 7, &[1, 2, 3]).unwrap();
        let charged = memory.used_memory_size().unwrap();
        memory.map_stack_alias(STACK, source, PAGE_SIZE).unwrap();
        assert_eq!(memory.used_memory_size().unwrap(), charged);
        let mut bytes = [0; 3];
        memory.read(STACK + 7, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3]);
        assert!(memory.read(source, &mut bytes).is_err());
        assert!(memory.write(source, &[9]).is_err());
        assert!(memory.fetch_instruction(STACK).is_err());
        memory.write(STACK + 7, &[4, 5, 6]).unwrap();
        memory.unmap_stack_alias(STACK, source, PAGE_SIZE).unwrap();
        memory.read(source + 7, &mut bytes).unwrap();
        assert_eq!(bytes, [4, 5, 6]);
        assert!(memory.read(STACK, &mut bytes).is_err());
        assert_eq!(memory.stack_source_metadata(source), None);
    }

    #[test]
    fn locks_clip_queries_and_prevent_backing_changes() {
        let mut memory = memory();
        let source = HEAP + PAGE_SIZE;
        memory.map_stack_alias(STACK, source, PAGE_SIZE).unwrap();
        for (address, base, size) in [
            (HEAP, HEAP, PAGE_SIZE),
            (source, source, PAGE_SIZE),
            (source + PAGE_SIZE, source + PAGE_SIZE, 2 * PAGE_SIZE),
        ] {
            let query = memory.query(address, 1 << 39).unwrap();
            assert_eq!((query.0, query.1), (base, size));
        }
        assert_eq!(
            memory.stack_source_metadata(source),
            Some((MemoryPermission::NONE, 1))
        );
        let epoch = memory.write_epoch();
        assert!(memory.unmap(HEAP, 4 * PAGE_SIZE).is_err());
        assert!(
            memory
                .protect(HEAP, 4 * PAGE_SIZE, MemoryPermission::READ)
                .is_err()
        );
        assert!(
            memory
                .protect(STACK, PAGE_SIZE, MemoryPermission::READ_EXECUTE)
                .is_err()
        );
        assert!(memory.load_bytes(source, &[8]).is_err());
        assert!(
            memory
                .reserve_transfer(source, PAGE_SIZE, MemoryPermission::NONE)
                .is_err()
        );
        assert!(memory.resize_heap(HEAP, PAGE_SIZE).is_err());
        assert_eq!(memory.write_epoch(), epoch);
        // Growing the owner mapping must not invalidate address-based aliases.
        memory.resize_heap(HEAP, 5 * PAGE_SIZE).unwrap();
        memory.write(STACK, &[42]).unwrap();
        memory.unmap_stack_alias(STACK, source, PAGE_SIZE).unwrap();
        let mut byte = [0];
        memory.read(source, &mut byte).unwrap();
        assert_eq!(byte, [42]);
    }

    #[test]
    fn invalid_mapping_and_unmapping_are_transactional() {
        let mut memory = memory();
        let source = HEAP + PAGE_SIZE;
        for (destination, source, size) in [
            (STACK + 1, source, PAGE_SIZE),
            (STACK, source + 1, PAGE_SIZE),
            (STACK, source, 0),
            (STACK, source, PAGE_SIZE + 1),
            ((1 << 39) - PAGE_SIZE, source, 2 * PAGE_SIZE),
            (STACK, u64::MAX - PAGE_SIZE + 1, PAGE_SIZE),
            (HEAP, source, PAGE_SIZE),
            (STACK, HEAP + 4 * PAGE_SIZE, PAGE_SIZE),
        ] {
            let epoch = memory.write_epoch();
            assert!(memory.map_stack_alias(destination, source, size).is_err());
            assert_eq!(memory.write_epoch(), epoch);
            assert_eq!(memory.regions().len(), 1);
        }
        memory.map_stack_alias(STACK, source, PAGE_SIZE).unwrap();
        let epoch = memory.write_epoch();
        assert!(
            memory
                .map_stack_alias(STACK + PAGE_SIZE, source, PAGE_SIZE)
                .is_err()
        );
        assert!(memory.unmap_stack_alias(STACK, HEAP, PAGE_SIZE).is_err());
        assert!(
            memory
                .unmap_stack_alias(STACK, source, 2 * PAGE_SIZE)
                .is_err()
        );
        assert_eq!(memory.write_epoch(), epoch);
        assert!(memory.write(source, &[1]).is_err());
    }

    #[test]
    fn independent_aliases_and_clones_keep_their_own_lifetimes() {
        let mut memory = memory();
        memory.map_stack_alias(STACK, HEAP, PAGE_SIZE).unwrap();
        memory
            .map_stack_alias(STACK + PAGE_SIZE, HEAP + PAGE_SIZE, PAGE_SIZE)
            .unwrap();
        let mut cloned = memory.clone();
        cloned.write(STACK, &[7]).unwrap();
        cloned.unmap_stack_alias(STACK, HEAP, PAGE_SIZE).unwrap();
        let mut byte = [0];
        cloned.read(HEAP, &mut byte).unwrap();
        assert_eq!(byte, [7]);
        memory.read(STACK, &mut byte).unwrap();
        assert_eq!(byte, [0]);
        assert!(memory.read(HEAP, &mut byte).is_err());
        assert!(cloned.read(HEAP + PAGE_SIZE, &mut byte).is_err());
        memory.unmap_stack_alias(STACK, HEAP, PAGE_SIZE).unwrap();
        assert!(memory.read(HEAP + PAGE_SIZE, &mut byte).is_err());
    }
}
