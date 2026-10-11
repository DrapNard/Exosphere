// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Guest virtual-memory model used by Horizon processes.
//!
//! This is not a host allocator. It describes guest mappings and provides a conservative
//! byte-backed implementation for early runtime/testing. A future VM/hypervisor backend can
//! mirror these mappings without changing the public Horizon model.

use crate::error::{HorizonError, HorizonResult};

/// Guest page size used by Horizon user processes.
pub const PAGE_SIZE: u64 = 0x1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPermission(u8);

impl MemoryPermission {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const EXECUTE: Self = Self(1 << 2);
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);
    pub const READ_EXECUTE: Self = Self(Self::READ.0 | Self::EXECUTE.0);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    Code,
    Data,
    Heap,
    Stack,
    ThreadLocal,
    Shared,
    Alias,
    Device,
    Other,
}

#[derive(Debug, Clone)]
pub struct MemoryRegion {
    pub base: u64,
    pub size: u64,
    pub permission: MemoryPermission,
    pub kind: MemoryKind,
    bytes: backing::Backing,
    /// Stack aliases resolve into the owner's existing backing. Their empty
    /// storage is never accessed directly and is not charged a second time.
    alias_source: Option<u64>,
}

mod backing;
mod device;
mod shared;
mod stack_alias;
mod transfer;
pub use device::{DeviceMemoryLease, DeviceMemoryLeaseIdentity};

// TLS and shared mappings require separate accounting. Stack aliases are excluded
// at the region level because their source already owns the charged backing.
fn charged(kind: MemoryKind) -> bool {
    matches!(
        kind,
        MemoryKind::Code | MemoryKind::Data | MemoryKind::Stack | MemoryKind::Heap
    )
}

impl MemoryRegion {
    fn end(&self) -> u64 {
        self.base.saturating_add(self.size)
    }

    fn contains_range(&self, address: u64, size: usize) -> bool {
        let Ok(size64) = u64::try_from(size) else {
            return false;
        };
        let Some(end) = address.checked_add(size64) else {
            return false;
        };
        address >= self.base && end <= self.end()
    }
}

#[derive(Debug, Default)]
pub struct AddressSpace {
    device_identity: std::sync::Arc<()>,
    device_leases: Vec<device::DeviceReservation>,
    transfers: Vec<transfer::TransferReservation>,
    regions: Vec<MemoryRegion>,
    write_epoch: u64,
    memory_limit: Option<u64>,
}

impl Clone for AddressSpace {
    fn clone(&self) -> Self {
        // Guest snapshots own their copied backing; a device lease cannot be
        // replayed against that copy or retain the original process's mappings.
        Self {
            device_identity: std::sync::Arc::new(()),
            device_leases: Vec::new(),
            transfers: self.transfers.clone(),
            regions: self.regions.clone(),
            write_epoch: self.write_epoch,
            memory_limit: self.memory_limit,
        }
    }
}

impl AddressSpace {
    /// Configure charged guest memory, independently of virtual-address reservations.
    /// Synthetic address spaces may leave this unset; loaders must choose a policy.
    pub fn set_memory_limit(&mut self, limit: u64) -> HorizonResult<()> {
        if limit == 0 || limit % PAGE_SIZE != 0 || self.used_memory_size()? > limit {
            return Err(HorizonError::InvalidState(
                "invalid or already exceeded memory quota".into(),
            ));
        }
        self.memory_limit = Some(limit);
        Ok(())
    }

    pub fn memory_limit(&self) -> Option<u64> {
        self.memory_limit
    }

    pub fn used_memory_size(&self) -> HorizonResult<u64> {
        self.regions
            .iter()
            .filter(|region| charged(region.kind) && region.alias_source.is_none())
            .try_fold(0u64, |total, region| {
                total
                    .checked_add(region.size)
                    .ok_or_else(|| HorizonError::InvalidState("memory accounting overflow".into()))
            })
    }
    /// Validate a complete access before invoking a service with external side effects.
    pub fn validate_access(
        &self,
        address: u64,
        size: usize,
        permission: MemoryPermission,
    ) -> HorizonResult<()> {
        self.check_transfer_access(address, size, permission)?;
        self.check_stack_source_access(address, size)?;
        let region = self
            .regions
            .iter()
            .find(|region| region.contains_range(address, size))
            .ok_or(HorizonError::InvalidAddress(address))?;
        if !region.permission.contains(permission) {
            return Err(HorizonError::PermissionDenied);
        }
        Ok(())
    }
    /// Conservative exclusive-monitor version. Any guest write invalidates reservations,
    /// including writes of the same bytes (which a value comparison would miss).
    pub fn write_epoch(&self) -> u64 {
        self.regions.iter().fold(self.write_epoch, |epoch, region| {
            if let backing::Backing::Shared(memory) = &region.bytes {
                epoch.wrapping_add(memory.revision())
            } else {
                epoch
            }
        })
    }
    /// Returns the containing mapping or unmapped gap in a bounded guest address space.
    pub fn query(
        &self,
        address: u64,
        limit: u64,
    ) -> HorizonResult<(u64, u64, Option<&MemoryRegion>)> {
        if address >= limit {
            // Horizon describes the inaccessible tail through the end of the u64 space.
            return Ok((limit, limit.wrapping_neg(), None));
        }
        let mut previous_end = 0;
        for region in &self.regions {
            if address < region.base {
                return Ok((previous_end, region.base.min(limit) - previous_end, None));
            }
            if address < region.end() {
                let (base, size) = self.transfer_query_bounds(address, region);
                let (base, size) = self.stack_source_query_bounds(address, base, size);
                return Ok((base, size, Some(region)));
            }
            previous_end = region.end();
        }
        Ok((previous_end, limit - previous_end, None))
    }

    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions
    }

    /// Creates an aligned guest mapping backed by zero-initialized bytes.
    pub fn map(
        &mut self,
        base: u64,
        size: u64,
        permission: MemoryPermission,
        kind: MemoryKind,
    ) -> HorizonResult<()> {
        if size == 0 || base % PAGE_SIZE != 0 || size % PAGE_SIZE != 0 {
            return Err(HorizonError::InvalidAddress(base));
        }
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        if self.regions.iter().any(|r| base < r.end() && end > r.base) {
            return Err(HorizonError::MemoryOverlap);
        }
        if charged(kind) {
            let used = self
                .used_memory_size()?
                .checked_add(size)
                .ok_or_else(|| HorizonError::InvalidState("memory accounting overflow".into()))?;
            if self.memory_limit.is_some_and(|limit| used > limit) {
                return Err(HorizonError::InvalidState(
                    "guest memory quota exceeded".into(),
                ));
            }
        }
        let len = usize::try_from(size).map_err(|_| HorizonError::InvalidAddress(base))?;
        let mut bytes = Vec::new();
        let bytes = if kind == MemoryKind::Heap {
            backing::Backing::Sparse(Default::default())
        } else {
            bytes
                .try_reserve_exact(len)
                .map_err(|_| HorizonError::InvalidState("host backing allocation failed".into()))?;
            bytes.resize(len, 0);
            backing::Backing::Dense(bytes)
        };
        self.regions.push(MemoryRegion {
            base,
            size,
            permission,
            kind,
            bytes,
            alias_source: None,
        });
        self.regions.sort_by_key(|r| r.base);
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    pub fn unmap(&mut self, base: u64, size: u64) -> HorizonResult<()> {
        self.reject_shared_overlap(base, size)?;
        self.reject_transfer_overlap(base, size)?;
        self.reject_stack_alias_overlap(base, size)?;
        let index = self
            .regions
            .iter()
            .position(|r| r.base == base && r.size == size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        self.regions.remove(index);
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    /// Resize the sole heap mapping at a fixed base. Admission checks happen
    /// before mutation; growth preserves pages and shrink discards removed pages.
    pub fn resize_heap(&mut self, base: u64, size: u64) -> HorizonResult<()> {
        if base % PAGE_SIZE != 0 || size % PAGE_SIZE != 0 {
            return Err(HorizonError::InvalidAddress(base));
        }
        let end = base
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        let index = self
            .regions
            .iter()
            .position(|region| region.base == base && region.kind == MemoryKind::Heap);
        if self
            .regions
            .iter()
            .enumerate()
            .any(|(i, region)| Some(i) != index && base < region.end() && end > region.base)
        {
            return Err(HorizonError::MemoryOverlap);
        }
        let old_size = index.map_or(0, |index| self.regions[index].size);
        if size < old_size {
            self.reject_transfer_overlap(end, old_size - size)?;
            self.reject_stack_alias_overlap(end, old_size - size)?;
        }
        let used = self
            .used_memory_size()?
            .checked_sub(old_size)
            .and_then(|used| used.checked_add(size))
            .ok_or_else(|| HorizonError::InvalidState("heap accounting overflow".into()))?;
        if self.memory_limit.is_some_and(|limit| used > limit) {
            return Err(HorizonError::InvalidState(
                "guest memory quota exceeded".into(),
            ));
        }
        if let Some(index) = index {
            if size == 0 {
                self.regions.remove(index);
            } else {
                let region = &mut self.regions[index];
                if size < old_size {
                    region.bytes.shrink_heap(size as usize);
                }
                region.size = size;
            }
            self.write_epoch = self.write_epoch.wrapping_add(1);
        } else if size != 0 {
            self.map(base, size, MemoryPermission::READ_WRITE, MemoryKind::Heap)?;
        }
        Ok(())
    }

    pub fn protect(
        &mut self,
        base: u64,
        size: u64,
        permission: MemoryPermission,
    ) -> HorizonResult<()> {
        self.reject_shared_overlap(base, size)?;
        self.reject_transfer_overlap(base, size)?;
        self.reject_stack_alias_overlap(base, size)?;
        let region = self
            .regions
            .iter_mut()
            .find(|r| r.base == base && r.size == size)
            .ok_or(HorizonError::InvalidAddress(base))?;
        region.permission = permission;
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    pub fn read(&self, address: u64, output: &mut [u8]) -> HorizonResult<()> {
        self.validate_access(address, output.len(), MemoryPermission::READ)?;
        let (index, offset) = self.resolve_backing(address, output.len())?;
        self.regions[index].bytes.read(offset, output)?;
        Ok(())
    }

    /// Instruction fetch checks execute permission independently of data-read permission.
    pub fn fetch_instruction(&self, address: u64) -> HorizonResult<u32> {
        if address % 4 != 0 {
            return Err(HorizonError::InvalidAddress(address));
        }
        let region = self
            .regions
            .iter()
            .find(|region| region.contains_range(address, 4))
            .ok_or(HorizonError::InvalidAddress(address))?;
        if !region.permission.contains(MemoryPermission::EXECUTE) {
            return Err(HorizonError::PermissionDenied);
        }
        let offset = usize::try_from(address - region.base)
            .map_err(|_| HorizonError::InvalidAddress(address))?;
        let mut bytes = [0; 4];
        region.bytes.read(offset, &mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn write(&mut self, address: u64, input: &[u8]) -> HorizonResult<()> {
        self.validate_access(address, input.len(), MemoryPermission::WRITE)?;
        let (index, offset) = self.resolve_backing(address, input.len())?;
        self.regions[index].bytes.write(offset, input)?;
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }

    pub fn load_bytes(&mut self, address: u64, input: &[u8]) -> HorizonResult<()> {
        self.reject_shared_overlap(address, input.len() as u64)?;
        self.reject_transfer_overlap(address, input.len() as u64)?;
        self.reject_stack_alias_overlap(address, input.len() as u64)?;
        // Loader-only path: allows initializing RX code without temporarily lying about its
        // guest-visible permission. Keeping this separate avoids accidentally giving runtime
        // callers a permission bypass.
        let region = self
            .regions
            .iter_mut()
            .find(|r| r.contains_range(address, input.len()))
            .ok_or(HorizonError::InvalidAddress(address))?;
        let offset = usize::try_from(address - region.base)
            .map_err(|_| HorizonError::InvalidAddress(address))?;
        region.bytes.write(offset, input)?;
        self.write_epoch = self.write_epoch.wrapping_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_gib_heap_is_demand_zero_and_resize_preserves_only_retained_pages() {
        let mut memory = AddressSpace::default();
        let base = 0x2000000000;
        memory.set_memory_limit(3 << 30).unwrap();
        memory.resize_heap(base, 3 << 30).unwrap();
        let backing::Backing::Sparse(pages) = &memory.regions[0].bytes else {
            panic!("heap must be sparse");
        };
        assert!(pages.is_empty());
        let mut bytes = [0xff; 4];
        memory.read(base + (3 << 30) - 4, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 4]);
        memory.write(base + PAGE_SIZE - 2, &[1, 2, 3, 4]).unwrap();
        memory.read(base + PAGE_SIZE - 2, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);
        let backing::Backing::Sparse(pages) = &memory.regions[0].bytes else {
            unreachable!()
        };
        assert_eq!(pages.len(), 2);
        memory.resize_heap(base, PAGE_SIZE).unwrap();
        memory.resize_heap(base, 3 << 30).unwrap();
        memory.read(base + PAGE_SIZE - 2, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 0, 0]);
        let epoch = memory.write_epoch();
        assert!(memory.resize_heap(base, (3 << 30) + PAGE_SIZE).is_err());
        assert_eq!(memory.write_epoch(), epoch);
        assert_eq!(memory.regions[0].size, 3 << 30);
        memory.resize_heap(base, 0).unwrap();
        assert!(memory.read(base, &mut bytes).is_err());
        assert_eq!(memory.used_memory_size().unwrap(), 0);
    }

    #[test]
    fn quota_rejects_overcommit_without_mutation_and_unmap_refunds_usage() {
        let mut memory = AddressSpace::default();
        memory.set_memory_limit(2 * PAGE_SIZE).unwrap();
        memory
            .map(
                0x1000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let epoch = memory.write_epoch();
        assert!(
            memory
                .map(
                    0x4000,
                    PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap
                )
                .is_err()
        );
        assert_eq!(memory.write_epoch(), epoch);
        assert_eq!(memory.regions().len(), 1);
        assert!(memory.set_memory_limit(PAGE_SIZE).is_err());
        assert_eq!(memory.memory_limit(), Some(2 * PAGE_SIZE));
        memory.unmap(0x1000, 2 * PAGE_SIZE).unwrap();
        memory
            .map(
                0x4000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        assert_eq!(memory.used_memory_size().unwrap(), PAGE_SIZE);
        assert!(memory.set_memory_limit(PAGE_SIZE + 1).is_err());
    }

    #[test]
    fn query_reports_mapping_gaps_and_inaccessible_tail() {
        let mut memory = AddressSpace::default();
        memory
            .map(0x2000, PAGE_SIZE, MemoryPermission::READ, MemoryKind::Data)
            .unwrap();
        memory
            .map(0x5000, PAGE_SIZE, MemoryPermission::READ, MemoryKind::Data)
            .unwrap();
        for (address, expected) in [
            (0, (0, 0x2000)),
            (0x2001, (0x2000, PAGE_SIZE)),
            (0x4000, (0x3000, 0x2000)),
            (0x7000, (0x6000, 0x2000)),
            (0x8000, (0x8000, 0u64.wrapping_sub(0x8000))),
        ] {
            let (base, size, region) = memory.query(address, 0x8000).unwrap();
            assert_eq!((base, size), expected);
            assert_eq!(region.is_some(), address == 0x2001);
        }
    }

    #[test]
    fn mapping_enforces_permissions_and_overlap() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Data,
            )
            .unwrap();
        assert_eq!(
            memory.map(0x1000, PAGE_SIZE, MemoryPermission::READ, MemoryKind::Data),
            Err(HorizonError::MemoryOverlap)
        );
        memory.write(0x1010, &[1, 2, 3]).unwrap();
        let mut out = [0; 3];
        memory.read(0x1010, &mut out).unwrap();
        assert_eq!(out, [1, 2, 3]);
        memory
            .protect(0x1000, PAGE_SIZE, MemoryPermission::READ)
            .unwrap();
        assert_eq!(
            memory.write(0x1010, &[4]),
            Err(HorizonError::PermissionDenied)
        );
    }
}
