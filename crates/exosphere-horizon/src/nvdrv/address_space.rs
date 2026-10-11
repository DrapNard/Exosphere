// SPDX-License-Identifier: GPL-3.0-or-later
//! Guest GPU mappings retain nvmap backing rather than treating VA as a resource ID.

use super::NvMapBacking;
use crate::{HorizonError, HorizonResult, memory::AddressSpace};
use std::collections::BTreeMap;

const GPU_VA_LIMIT: u64 = 1 << 40;

#[derive(Debug, Clone)]
struct Mapping {
    size: u64,
    offset: usize,
    backing: NvMapBacking,
}

/// Borrowed canonical mapping metadata for host resource translation. GPU VA is
/// not a resource identity; aliases may refer to the same backing allocation.
/// Reads/writes still require live lease validation through NvGpuAddressSpace.
#[derive(Debug, Clone, Copy)]
pub struct NvGpuMapping<'a> {
    pub gpu_address: u64,
    pub size: u64,
    pub buffer_offset: u64,
    pub backing: &'a NvMapBacking,
}

/// Device-side guest VA ownership. Backend resource translation is a separate layer.
#[derive(Debug, Clone, Default)]
pub struct NvGpuAddressSpace {
    mappings: BTreeMap<u64, Mapping>,
    configuration: Option<NvGpuAddressConfiguration>,
    reservations: BTreeMap<u64, Reservation>,
    regions: Option<[NvGpuVaRegion; 2]>,
}

#[derive(Debug, Clone, Copy)]
struct Reservation {
    size: u64,
    page_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvGpuAddressConfiguration {
    pub flags: u32,
    pub big_page_size: u32,
}

/// Page-specific allocation bounds; end is exclusive, never a byte count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvGpuVaRegion {
    pub start: u64,
    pub end: u64,
    pub page_size: u32,
}

impl NvGpuAddressSpace {
    /// Enumerate current mappings without cloning ownership or manufacturing
    /// host resource IDs. The returned backing size is the actual lease size,
    /// not the size of a mapped subrange.
    pub fn mappings(&self) -> impl Iterator<Item = NvGpuMapping<'_>> {
        self.mappings
            .iter()
            .map(|(&gpu_address, mapping)| NvGpuMapping {
                gpu_address,
                size: mapping.size,
                buffer_offset: mapping.offset as u64,
                backing: &mapping.backing,
            })
    }
    pub(super) fn map_reserved(
        &mut self,
        address: u64,
        size: u64,
        buffer_offset: u64,
        requested_page_size: u32,
        backing: &NvMapBacking,
    ) -> HorizonResult<u32> {
        let (&base, reservation) = self
            .reservations
            .range(..=address)
            .next_back()
            .ok_or(HorizonError::InvalidAddress(address))?;
        let end = address
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(address))?;
        let page_size = reservation.page_size;
        if size == 0
            || end > base + reservation.size
            || address % u64::from(page_size) != 0
            || size % u64::from(page_size) != 0
            || buffer_offset % u64::from(page_size) != 0
            || requested_page_size != 0 && requested_page_size != page_size
        {
            return Err(HorizonError::InvalidAddress(address));
        }
        self.map_fixed(address, size, buffer_offset, backing)?;
        Ok(page_size)
    }

    pub fn reserve_fixed(&mut self, address: u64, pages: u32, page_size: u32) -> HorizonResult<()> {
        let configuration = self.configuration.ok_or_else(|| {
            HorizonError::InvalidState("GPU address space is uninitialized".into())
        })?;
        let size = u64::from(pages) * u64::from(page_size);
        let end = address
            .checked_add(size)
            .filter(|&end| end <= GPU_VA_LIMIT)
            .ok_or(HorizonError::InvalidAddress(address))?;
        if pages == 0
            || !matches!(page_size, 4096) && page_size != configuration.big_page_size
            || address == 0
            || address % u64::from(page_size.max(1)) != 0
        {
            return Err(HorizonError::InvalidAddress(address));
        }
        if let Some(regions) = self.regions
            && !regions.iter().any(|region| {
                region.page_size == page_size && address >= region.start && end <= region.end
            })
        {
            return Err(HorizonError::InvalidAddress(address));
        }
        if self
            .reservations
            .iter()
            .any(|(&base, reservation)| address < base + reservation.size && end > base)
            || self
                .mappings
                .iter()
                .any(|(&base, mapping)| address < base + mapping.size && end > base)
        {
            return Err(HorizonError::InvalidState(
                "overlapping GPU reservation".into(),
            ));
        }
        self.reservations
            .insert(address, Reservation { size, page_size });
        Ok(())
    }

    /// First-fit allocation within the configured page-class region. No default
    /// firmware bounds are invented; validation and placement precede mutation.
    pub(super) fn reserve_auto(
        &mut self,
        pages: u32,
        page_size: u32,
        alignment: u64,
    ) -> HorizonResult<u64> {
        let configuration = self.configuration.ok_or_else(|| {
            HorizonError::InvalidState("GPU address space is uninitialized".into())
        })?;
        if pages == 0
            || page_size != 4096 && page_size != configuration.big_page_size
            || alignment != 0 && !alignment.is_power_of_two()
        {
            return Err(HorizonError::IpcProtocol(
                "invalid automatic GPU reservation".into(),
            ));
        }
        let region = self
            .regions
            .and_then(|regions| {
                regions
                    .into_iter()
                    .find(|region| region.page_size == page_size)
            })
            .ok_or_else(|| {
                HorizonError::InvalidState(
                    "automatic GPU reservation requires established VA bounds".into(),
                )
            })?;
        let alignment = alignment.max(u64::from(page_size));
        let align = |address: u64| {
            address
                .checked_add(alignment - 1)
                .map(|value| value & !(alignment - 1))
        };
        let size = u64::from(pages) * u64::from(page_size);
        let mut address = align(region.start).ok_or(HorizonError::InvalidAddress(region.start))?;
        // Reservations and standalone fixed mappings both own VA. Sorting their
        // union also handles a mapping nested inside an existing reservation.
        let mut occupied: Vec<_> = self
            .reservations
            .iter()
            .map(|(&base, reservation)| (base, reservation.size))
            .chain(
                self.mappings
                    .iter()
                    .map(|(&base, mapping)| (base, mapping.size)),
            )
            .collect();
        occupied.sort_unstable();
        for (base, length) in occupied {
            if base >= region.end {
                break;
            }
            let end = base
                .checked_add(length)
                .ok_or(HorizonError::InvalidAddress(base))?;
            if end <= address {
                continue;
            }
            if address.checked_add(size).is_some_and(|end| end <= base) {
                break;
            }
            address = align(end).ok_or(HorizonError::InvalidAddress(end))?;
        }
        if address.checked_add(size).is_none_or(|end| end > region.end) {
            return Err(HorizonError::InvalidAddress(address));
        }
        self.reserve_fixed(address, pages, page_size)?;
        Ok(address)
    }

    pub fn free_space(&mut self, address: u64, pages: u32, page_size: u32) -> HorizonResult<()> {
        let size = u64::from(pages) * u64::from(page_size);
        let reservation = self
            .reservations
            .get(&address)
            .ok_or(HorizonError::InvalidAddress(address))?;
        if reservation.size != size || reservation.page_size != page_size {
            return Err(HorizonError::InvalidAddress(address));
        }
        if self
            .mappings
            .iter()
            .any(|(&base, mapping)| address < base + mapping.size && address + size > base)
        {
            return Err(HorizonError::InvalidState(
                "GPU reservation still has mappings".into(),
            ));
        }
        self.reservations.remove(&address);
        Ok(())
    }

    pub fn configuration(&self) -> Option<NvGpuAddressConfiguration> {
        self.configuration
    }

    pub(super) fn initialize(&mut self, flags: u32, big_page_size: u32) -> HorizonResult<()> {
        self.initialize_with_ranges(flags, big_page_size, None)
    }

    pub fn regions(&self) -> Option<[NvGpuVaRegion; 2]> {
        self.regions
    }

    pub(super) fn initialize_with_ranges(
        &mut self,
        flags: u32,
        big_page_size: u32,
        bounds: Option<(u64, u64, u64)>,
    ) -> HorizonResult<()> {
        if self.configuration.is_some() {
            return Err(HorizonError::InvalidState(
                "GPU address space already initialized".into(),
            ));
        }
        if flags & !1 != 0 || !matches!(big_page_size, 0x10000 | 0x20000) {
            return Err(HorizonError::IpcProtocol(
                "unsupported GPU address-space configuration".into(),
            ));
        }
        let regions = if let Some((start, end, split)) = bounds {
            // Small/big page tables cannot share a PDE. A PDE covers 1024
            // big pages on GM20B, so the page-class split must align to it.
            if start == 0
                || start % 4096 != 0
                || start >= split
                || split >= end
                || end > GPU_VA_LIMIT
                || end % u64::from(big_page_size) != 0
                || split % (u64::from(big_page_size) << 10) != 0
            {
                return Err(HorizonError::IpcProtocol(
                    "invalid custom GPU VA bounds".into(),
                ));
            }
            Some([
                NvGpuVaRegion {
                    start,
                    end: split,
                    page_size: 4096,
                },
                NvGpuVaRegion {
                    start: split,
                    end,
                    page_size: big_page_size,
                },
            ])
        } else {
            None
        };
        if let Some(regions) = regions
            && self.mappings.iter().any(|(&address, mapping)| {
                !regions.iter().any(|region| {
                    address >= region.start
                        && address
                            .checked_add(mapping.size)
                            .is_some_and(|end| end <= region.end)
                })
            })
        {
            // Public fixed mappings may predate initialization (e.g. a backend
            // fixture). Never install a policy that contradicts retained state.
            return Err(HorizonError::InvalidState(
                "GPU VA bounds exclude existing mappings".into(),
            ));
        }
        self.configuration = Some(NvGpuAddressConfiguration {
            flags,
            big_page_size,
        });
        self.regions = regions;
        Ok(())
    }
    pub fn map_fixed(
        &mut self,
        gpu_address: u64,
        size: u64,
        buffer_offset: u64,
        backing: &NvMapBacking,
    ) -> HorizonResult<()> {
        let end = gpu_address
            .checked_add(size)
            .filter(|&end| end <= GPU_VA_LIMIT)
            .ok_or(HorizonError::InvalidAddress(gpu_address))?;
        let buffer_end = buffer_offset
            .checked_add(size)
            .ok_or(HorizonError::InvalidAddress(buffer_offset))?;
        if size == 0
            || gpu_address == 0
            || gpu_address % 4096 != 0
            || size % 4096 != 0
            || buffer_offset % 4096 != 0
            || buffer_end > backing.lease.size() as u64
        {
            return Err(HorizonError::InvalidAddress(gpu_address));
        }
        if let Some(regions) = self.regions
            && !regions
                .iter()
                .any(|region| gpu_address >= region.start && end <= region.end)
        {
            return Err(HorizonError::InvalidAddress(gpu_address));
        }
        if self
            .mappings
            .iter()
            .any(|(&base, mapping)| gpu_address < base + mapping.size && end > base)
        {
            return Err(HorizonError::InvalidState("overlapping GPU mapping".into()));
        }
        let offset = usize::try_from(buffer_offset)
            .map_err(|_| HorizonError::InvalidAddress(buffer_offset))?;
        self.mappings.insert(
            gpu_address,
            Mapping {
                size,
                offset,
                backing: backing.clone(),
            },
        );
        Ok(())
    }

    pub fn unmap(&mut self, gpu_address: u64) -> HorizonResult<()> {
        self.mappings
            .remove(&gpu_address)
            .map(|_| ())
            .ok_or(HorizonError::InvalidAddress(gpu_address))
    }

    fn resolve(&self, address: u64, size: usize) -> HorizonResult<(&Mapping, usize)> {
        let (&base, mapping) = self
            .mappings
            .range(..=address)
            .next_back()
            .ok_or(HorizonError::InvalidAddress(address))?;
        let end = address
            .checked_add(size as u64)
            .ok_or(HorizonError::InvalidAddress(address))?;
        if end > base + mapping.size || address >= base + mapping.size {
            return Err(HorizonError::InvalidAddress(address));
        }
        let delta =
            usize::try_from(address - base).map_err(|_| HorizonError::InvalidAddress(address))?;
        let offset = mapping
            .offset
            .checked_add(delta)
            .ok_or(HorizonError::InvalidAddress(address))?;
        Ok((mapping, offset))
    }

    fn segments(&self, address: u64, size: usize) -> HorizonResult<Vec<(&Mapping, usize, usize)>> {
        address
            .checked_add(size as u64)
            .filter(|&end| end <= GPU_VA_LIMIT)
            .ok_or(HorizonError::InvalidAddress(address))?;
        if size == 0 {
            self.resolve(address, 0)?;
            return Ok(Vec::new());
        }
        let mut segments = Vec::new();
        let mut done = 0;
        while done < size {
            let current = address + done as u64;
            let (mapping, offset) = self.resolve(current, 1)?;
            let available = mapping.size - (offset - mapping.offset) as u64;
            let length = usize::try_from(available.min((size - done) as u64))
                .map_err(|_| HorizonError::InvalidAddress(current))?;
            segments.push((mapping, offset, length));
            done += length;
        }
        Ok(segments)
    }

    pub fn validate_access(
        &self,
        memory: &AddressSpace,
        address: u64,
        size: usize,
        permission: crate::memory::MemoryPermission,
    ) -> HorizonResult<()> {
        for (mapping, offset, length) in self.segments(address, size)? {
            memory.validate_device_access(&mapping.backing.lease, offset, length, permission)?;
        }
        Ok(())
    }

    pub fn read(
        &self,
        memory: &AddressSpace,
        address: u64,
        output: &mut [u8],
    ) -> HorizonResult<()> {
        let segments = self.segments(address, output.len())?;
        for &(mapping, offset, length) in &segments {
            memory.validate_device_access(
                &mapping.backing.lease,
                offset,
                length,
                crate::memory::MemoryPermission::READ,
            )?;
        }
        let mut done = 0;
        for (mapping, offset, length) in segments {
            memory.read_device_memory(
                &mapping.backing.lease,
                offset,
                &mut output[done..done + length],
            )?;
            done += length;
        }
        Ok(())
    }

    /// Synchronously copy disjoint guest ranges. Validate all rows and backing
    /// aliases before mutation; callers must separately implement queue completion.
    pub fn copy_ranges(
        &self,
        memory: &mut AddressSpace,
        ranges: &[(u64, u64, usize)],
    ) -> HorizonResult<()> {
        let mut sources = Vec::new();
        let mut destinations = Vec::new();
        let mut total = 0usize;
        for &(source, destination, size) in ranges {
            total = total
                .checked_add(size)
                .ok_or(HorizonError::InvalidState("GPU copy size overflow".into()))?;
            for (address, permission, intervals) in [
                (source, crate::memory::MemoryPermission::READ, &mut sources),
                (
                    destination,
                    crate::memory::MemoryPermission::WRITE,
                    &mut destinations,
                ),
            ] {
                for (mapping, offset, length) in self.segments(address, size)? {
                    memory.validate_device_access(
                        &mapping.backing.lease,
                        offset,
                        length,
                        permission,
                    )?;
                    let start = mapping.backing.address.checked_add(offset as u64).ok_or(
                        HorizonError::InvalidState("GPU backing address overflow".into()),
                    )?;
                    let end =
                        start
                            .checked_add(length as u64)
                            .ok_or(HorizonError::InvalidState(
                                "GPU backing address overflow".into(),
                            ))?;
                    if length != 0 {
                        intervals.push((start, end));
                    }
                }
            }
        }
        // Compare actual CPU backing ranges, not GPU VAs: aliases can have
        // unrelated device addresses while referring to the same bytes.
        sources.sort_unstable();
        destinations.sort_unstable();
        let (mut source, mut destination) = (0, 0);
        while source < sources.len() && destination < destinations.len() {
            let a = sources[source];
            let b = destinations[destination];
            if a.0 < b.1 && b.0 < a.1 {
                return Err(HorizonError::InvalidState(
                    "overlapping GPU copy backing".into(),
                ));
            }
            if a.1 <= b.0 {
                source += 1;
            } else {
                destination += 1;
            }
        }
        // Bound scratch use explicitly; never truncate a guest transfer.
        if total > 64 * 1024 * 1024 {
            return Err(HorizonError::InvalidState("GPU copy staging limit".into()));
        }
        let mut staged = Vec::new();
        staged
            .try_reserve_exact(total)
            .map_err(|_| HorizonError::InvalidState("GPU copy staging allocation".into()))?;
        staged.resize(total, 0);
        let mut offset = 0;
        for &(source, _, size) in ranges {
            self.read(memory, source, &mut staged[offset..offset + size])?;
            offset += size;
        }
        offset = 0;
        for &(_, destination, size) in ranges {
            self.write(memory, destination, &staged[offset..offset + size])?;
            offset += size;
        }
        Ok(())
    }

    pub fn write(
        &self,
        memory: &mut AddressSpace,
        address: u64,
        input: &[u8],
    ) -> HorizonResult<()> {
        let segments = self.segments(address, input.len())?;
        // Preflight the entire range: a hole or read-only later mapping must
        // not leave bytes written to an earlier mapping.
        for &(mapping, offset, length) in &segments {
            memory.validate_device_access(
                &mapping.backing.lease,
                offset,
                length,
                crate::memory::MemoryPermission::WRITE,
            )?;
        }
        let mut done = 0;
        for (mapping, offset, length) in segments {
            memory.write_device_memory(
                &mapping.backing.lease,
                offset,
                &input[done..done + length],
            )?;
            done += length;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_first_fit_matches_small_bitmap_reference() {
        let split = 0x4000000u64;
        let start = split - 16 * 4096;
        for occupied in 0u16..256 {
            let mut baseline = NvGpuAddressSpace::default();
            baseline
                .initialize_with_ranges(1, 0x10000, Some((start, split + 0x10000, split)))
                .unwrap();
            for page in 0..8 {
                if occupied & (1 << page) != 0 {
                    baseline
                        .reserve_fixed(start + page * 4096, 1, 4096)
                        .unwrap();
                }
            }
            for pages in 1..=3u32 {
                for alignment_pages in [1u64, 2, 4] {
                    let expected = (0..=16 - u64::from(pages))
                        .find(|&first| {
                            first % alignment_pages == 0
                                && (first..first + u64::from(pages))
                                    .all(|page| occupied & (1 << page) == 0)
                        })
                        .map(|first| start + first * 4096);
                    let mut gpu = baseline.clone();
                    assert_eq!(
                        gpu.reserve_auto(pages, 4096, alignment_pages * 4096).ok(),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn automatic_reservations_find_aligned_holes_and_reuse_freed_space() {
        let mut gpu = NvGpuAddressSpace::default();
        gpu.initialize_with_ranges(1, 0x10000, Some((0x100000, 0x8040000, 0x8000000)))
            .unwrap();
        gpu.reserve_fixed(0x100000, 1, 4096).unwrap();
        gpu.reserve_fixed(0x103000, 1, 4096).unwrap();
        assert_eq!(gpu.reserve_auto(2, 4096, 4096).unwrap(), 0x101000);
        assert_eq!(gpu.reserve_auto(1, 4096, 0).unwrap(), 0x104000);
        gpu.free_space(0x101000, 2, 4096).unwrap();
        assert_eq!(gpu.reserve_auto(2, 4096, 1).unwrap(), 0x101000);
        assert_eq!(gpu.reserve_auto(1, 4096, 0x200000).unwrap(), 0x200000);
        assert_eq!(gpu.reserve_auto(2, 0x10000, 0).unwrap(), 0x8000000);
        assert_eq!(gpu.reserve_auto(2, 0x10000, 0).unwrap(), 0x8020000);
        let count = gpu.reservations.len();
        assert!(gpu.reserve_auto(1, 0x10000, 0).is_err());
        assert_eq!(gpu.reservations.len(), count);
    }

    #[test]
    fn invalid_automatic_reservations_leave_state_unchanged() {
        let mut gpu = NvGpuAddressSpace::default();
        assert!(gpu.reserve_auto(1, 4096, 4096).is_err());
        gpu.initialize(1, 0x20000).unwrap();
        assert!(gpu.reserve_auto(1, 4096, 4096).is_err());
        assert!(gpu.reservations.is_empty());
        let mut gpu = NvGpuAddressSpace::default();
        gpu.initialize_with_ranges(1, 0x20000, Some((0x100000, 0x8040000, 0x8000000)))
            .unwrap();
        for (pages, page, alignment) in [
            (0, 4096, 0),
            (1, 0, 0),
            (1, 0x10000, 0),
            (1, 4096, 3),
            (1, 4096, 1 << 63),
            (u32::MAX, 4096, 0),
        ] {
            assert!(gpu.reserve_auto(pages, page, alignment).is_err());
            assert!(gpu.reservations.is_empty());
        }
        assert_eq!(gpu.reserve_auto(1, 4096, 4096).unwrap(), 0x100000);
    }

    #[test]
    fn automatic_reservations_exclude_standalone_and_nested_mappings() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                3 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, 3 * PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.initialize_with_ranges(1, 0x10000, Some((0x100000, 0x8040000, 0x8000000)))
            .unwrap();
        gpu.map_fixed(0x100000, 2 * PAGE_SIZE, 0, &backing).unwrap();
        gpu.reserve_fixed(0x102000, 1, 4096).unwrap();
        gpu.map_reserved(0x102000, PAGE_SIZE, 2 * PAGE_SIZE, 4096, &backing)
            .unwrap();
        assert_eq!(gpu.reserve_auto(1, 4096, 0).unwrap(), 0x103000);
        assert_eq!(gpu.mappings.len(), 2);
    }

    #[test]
    fn installing_custom_bounds_preserves_existing_mapping_invariants() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, 2 * PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        for (address, size) in [
            (0xff000, PAGE_SIZE),
            (0x40000000, PAGE_SIZE),
            (0x1ffff000, 2 * PAGE_SIZE),
        ] {
            let mut gpu = NvGpuAddressSpace::default();
            gpu.map_fixed(address, size, 0, &backing).unwrap();
            assert!(
                gpu.initialize_with_ranges(1, 0x20000, Some((0x100000, 0x40000000, 0x20000000)))
                    .is_err()
            );
            assert_eq!(gpu.configuration(), None);
            assert_eq!(gpu.regions(), None);
            assert_eq!(gpu.mappings().count(), 1);
            gpu.read(&memory, address, &mut [0]).unwrap();
        }
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
        gpu.initialize_with_ranges(1, 0x20000, Some((0x100000, 0x40000000, 0x20000000)))
            .unwrap();
        for (address, size) in [
            (0xff000, PAGE_SIZE),
            (0x40000000, PAGE_SIZE),
            (0x1ffff000, 2 * PAGE_SIZE),
        ] {
            assert!(gpu.map_fixed(address, size, 0, &backing).is_err());
            assert_eq!(gpu.mappings().count(), 1);
        }
    }

    #[test]
    fn custom_ranges_are_atomic_page_specific_and_end_exclusive() {
        for bounds in [
            (0, 0x40000000, 0x20000000),
            (0x100001, 0x40000000, 0x20000000),
            (0x20000000, 0x40000000, 0x20000000),
            (0x100000, 0x20000000, 0x20000000),
            (0x100000, GPU_VA_LIMIT + 0x20000, 0x20000000),
            (0x100000, 0x40000001, 0x20000000),
            (0x100000, 0x40000000, 0x20020000),
        ] {
            let mut gpu = NvGpuAddressSpace::default();
            assert!(
                gpu.initialize_with_ranges(1, 0x20000, Some(bounds))
                    .is_err()
            );
            assert_eq!(gpu.configuration(), None);
            assert_eq!(gpu.regions(), None);
        }
        let mut gpu = NvGpuAddressSpace::default();
        gpu.initialize_with_ranges(1, 0x20000, Some((0x100000, 0x40000000, 0x20000000)))
            .unwrap();
        let original = gpu.regions();
        assert!(gpu.initialize_with_ranges(1, 0x10000, None).is_err());
        assert_eq!(gpu.regions(), original);
        gpu.reserve_fixed(0x1ffff000, 1, 4096).unwrap();
        assert!(gpu.reserve_fixed(0x1ffff000, 2, 4096).is_err());
        gpu.reserve_fixed(0x3ffe0000, 1, 0x20000).unwrap();
        assert!(gpu.reserve_fixed(0x3ffe0000, 2, 0x20000).is_err());
        assert!(gpu.reserve_fixed(0xff000, 1, 4096).is_err());
        assert!(gpu.reserve_fixed(0x30000000, 1, 4096).is_err());
        assert!(gpu.reserve_fixed(0x200000, 1, 0x20000).is_err());
    }
    use crate::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};

    #[test]
    fn accesses_span_adjacent_mappings_and_preflight_later_faults() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let first = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let second = NvMapBacking {
            address: 0x5000,
            alignment: 4096,
            flags: 0,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x5000, PAGE_SIZE, MemoryPermission::READ)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &first).unwrap();
        gpu.map_fixed(0x101000, PAGE_SIZE, 0, &second).unwrap();
        memory.write(0x4fff, &[1, 2]).unwrap();
        let mut bytes = [0; 2];
        gpu.read(&memory, 0x100fff, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2]);
        assert!(gpu.write(&mut memory, 0x100fff, &[9, 8]).is_err());
        memory.read(0x4fff, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2]);
        gpu.unmap(0x101000).unwrap();
        let mut sentinel = [0x55; 2];
        assert!(gpu.read(&memory, 0x100fff, &mut sentinel).is_err());
        assert_eq!(sentinel, [0x55; 2]);
        assert!(gpu.write(&mut memory, 0x100fff, &[9, 8]).is_err());
        memory.read(0x4fff, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2]);
        let writable = NvMapBacking {
            lease: memory
                .pin_device_memory(0x5000, PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
            flags: 1,
            ..second.clone()
        };
        gpu.map_fixed(0x101000, PAGE_SIZE, 0, &writable).unwrap();
        gpu.write(&mut memory, 0x100fff, &[7, 8]).unwrap();
        memory.read(0x4fff, &mut bytes).unwrap();
        assert_eq!(bytes, [7, 8]);
        assert!(gpu.read(&memory.clone(), 0x100fff, &mut sentinel).is_err());
        assert_eq!(sentinel, [0x55; 2]);
    }

    #[test]
    fn fixed_reservations_enforce_policy_bounds_and_exact_release() {
        let mut gpu = NvGpuAddressSpace::default();
        assert!(gpu.reserve_fixed(0x100000, 1, 4096).is_err());
        gpu.initialize(1, 0x20000).unwrap();
        gpu.reserve_fixed(0x100000, 2, 4096).unwrap();
        assert!(gpu.reserve_fixed(0x101000, 1, 4096).is_err());
        assert!(gpu.reserve_fixed(0x200000, 1, 0x10000).is_err());
        assert!(gpu.reserve_fixed(GPU_VA_LIMIT - 4096, 2, 4096).is_err());
        assert!(gpu.reserve_fixed(u64::MAX - 4095, 2, 4096).is_err());
        assert!(gpu.reserve_fixed(0x200000, 0, 4096).is_err());
        assert!(gpu.free_space(0x100000, 1, 4096).is_err());
        assert_eq!(gpu.reservations.len(), 1);
        gpu.free_space(0x100000, 2, 4096).unwrap();
        assert!(gpu.free_space(0x100000, 2, 4096).is_err());
        gpu.reserve_fixed(0x100000, 1, 0x20000).unwrap();
        gpu.free_space(0x100000, 1, 0x20000).unwrap();
    }

    #[test]
    fn gpu_aliases_share_live_backing_and_hold_it_until_last_unmap() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, 2 * PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.initialize(1, 0x20000).unwrap();
        gpu.reserve_fixed(0x100000, 1, 4096).unwrap();
        gpu.map_fixed(0x100000, PAGE_SIZE, PAGE_SIZE, &backing)
            .unwrap();
        gpu.map_fixed(0x200000, PAGE_SIZE, PAGE_SIZE, &backing)
            .unwrap();
        let mappings: Vec<_> = gpu.mappings().collect();
        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[0].gpu_address, 0x100000);
        assert_eq!(mappings[1].gpu_address, 0x200000);
        for mapping in mappings {
            assert_eq!(mapping.size, PAGE_SIZE);
            assert_eq!(mapping.buffer_offset, PAGE_SIZE);
            assert_eq!(mapping.backing.lease.size() as u64, 2 * PAGE_SIZE);
            assert_eq!(mapping.backing.address, 0x4000);
        }
        assert!(gpu.free_space(0x100000, 1, 4096).is_err());
        gpu.write(&mut memory, 0x100008, &[37]).unwrap();
        let mut byte = [0];
        gpu.read(&memory, 0x200008, &mut byte).unwrap();
        assert_eq!(byte, [37]);
        memory.read(0x5008, &mut byte).unwrap();
        assert_eq!(byte, [37]);
        assert!(gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).is_err());
        assert!(gpu.map_fixed(GPU_VA_LIMIT, PAGE_SIZE, 0, &backing).is_err());
        assert!(
            gpu.map_fixed(0x300000, 2 * PAGE_SIZE, PAGE_SIZE, &backing)
                .is_err()
        );
        assert!(gpu.read(&memory, 0x100fff, &mut [0; 2]).is_err());
        assert!(gpu.read(&memory.clone(), 0x100000, &mut byte).is_err());
        drop(backing);
        gpu.unmap(0x100000).unwrap();
        assert_eq!(
            gpu.mappings()
                .map(|mapping| mapping.gpu_address)
                .collect::<Vec<_>>(),
            vec![0x200000]
        );
        gpu.free_space(0x100000, 1, 4096).unwrap();
        assert!(memory.unmap(0x4000, 2 * PAGE_SIZE).is_err());
        assert!(gpu.unmap(0x100000).is_err());
        gpu.unmap(0x200000).unwrap();
        assert_eq!(gpu.mappings().count(), 0);
        memory.unmap(0x4000, 2 * PAGE_SIZE).unwrap();
    }
}
