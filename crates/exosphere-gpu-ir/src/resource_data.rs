// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Runtime payload access for canonical GPU resources.
//!
//! Resource descriptors intentionally do not own potentially huge guest allocations. Backends use
//! this trait to obtain the current bytes behind a stable [`ResourceId`]. This keeps GPU IR
//! serializable while still allowing Vulkan/Metal/OpenGL to upload, download and synchronize real
//! guest data.

use std::collections::BTreeMap;

use crate::resource::ResourceId;

/// Errors returned when a backend accesses resource payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceDataError {
    Missing(ResourceId),
    ReadOnly(ResourceId),
    OutOfBounds {
        resource: ResourceId,
        offset: u64,
        length: u64,
        size: u64,
    },
    SizeOverflow,
    Memory {
        resource: ResourceId,
        source: crate::shader_memory::GpuMemoryError,
    },
    UnexpectedReadSize {
        resource: ResourceId,
        expected: u64,
        actual: u64,
    },
}

impl std::fmt::Display for ResourceDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(resource) => write!(f, "GPU resource {:?} has no payload", resource),
            Self::ReadOnly(resource) => write!(f, "GPU resource {:?} is read-only", resource),
            Self::OutOfBounds {
                resource,
                offset,
                length,
                size,
            } => write!(
                f,
                "GPU resource {:?} access 0x{offset:x}..+0x{length:x} exceeds payload size 0x{size:x}",
                resource,
            ),
            Self::SizeOverflow => write!(f, "GPU resource range overflow"),
            Self::Memory { resource, source } => {
                write!(
                    f,
                    "GPU resource {resource:?} memory access failed: {source}"
                )
            }
            Self::UnexpectedReadSize {
                resource,
                expected,
                actual,
            } => write!(
                f,
                "GPU resource {resource:?} read returned {actual} bytes, expected {expected}"
            ),
        }
    }
}

impl std::error::Error for ResourceDataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Memory { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Provides current resource bytes to a host GPU backend.
///
/// Implementations may expose guest RAM directly, resolve CAS objects, use copy-on-write shadow
/// storage, or proxy another process. Callers must not retain pointers across calls; the trait is
/// deliberately copy-based so remaps and guest memory invalidation remain safe.
pub trait GpuResourceData {
    /// Optional BLAKE3 content identity for an exact range. Equal fingerprints mean
    /// equal bytes, including across provider replacement; allocation-local
    /// counters are not sufficient. None means the provider cannot certify it.
    fn content_fingerprint(
        &self,
        _resource: ResourceId,
        _offset: u64,
        _length: u64,
    ) -> Result<Option<[u8; 32]>, ResourceDataError> {
        Ok(None)
    }
    /// Returns the current byte size of `resource`.
    fn len(&self, resource: ResourceId) -> Result<u64, ResourceDataError>;

    /// Reads exactly `out.len()` bytes at `offset`.
    fn read(
        &self,
        resource: ResourceId,
        offset: u64,
        out: &mut [u8],
    ) -> Result<(), ResourceDataError>;

    /// Writes exactly `data.len()` bytes at `offset`.
    fn write(
        &mut self,
        resource: ResourceId,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ResourceDataError>;

    /// Flushes host-side writes so guest-visible memory observes them.
    fn flush(&mut self, _resource: ResourceId) -> Result<(), ResourceDataError> {
        Ok(())
    }

    /// Marks a range as changed by the guest/CPU. Backends may use this to invalidate cached host
    /// copies without immediately uploading data.
    fn invalidate(
        &mut self,
        _resource: ResourceId,
        _offset: u64,
        _length: u64,
    ) -> Result<(), ResourceDataError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct MemoryResource {
    bytes: Vec<u8>,
    writable: bool,
}

/// Small deterministic provider used by tests, tooling and standalone compiled packages.
#[derive(Debug, Clone, Default)]
pub struct InMemoryGpuResources {
    resources: BTreeMap<ResourceId, MemoryResource>,
}

impl InMemoryGpuResources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: ResourceId, bytes: Vec<u8>, writable: bool) -> Option<Vec<u8>> {
        self.resources
            .insert(id, MemoryResource { bytes, writable })
            .map(|old| old.bytes)
    }

    pub fn remove(&mut self, id: ResourceId) -> Option<Vec<u8>> {
        self.resources.remove(&id).map(|entry| entry.bytes)
    }

    pub fn bytes(&self, id: ResourceId) -> Option<&[u8]> {
        self.resources.get(&id).map(|entry| entry.bytes.as_slice())
    }
}

impl GpuResourceData for InMemoryGpuResources {
    fn content_fingerprint(
        &self,
        resource: ResourceId,
        offset: u64,
        length: u64,
    ) -> Result<Option<[u8; 32]>, ResourceDataError> {
        let entry = self
            .resources
            .get(&resource)
            .ok_or(ResourceDataError::Missing(resource))?;
        let end = offset
            .checked_add(length)
            .ok_or(ResourceDataError::SizeOverflow)?;
        let size = entry.bytes.len() as u64;
        if end > size {
            return Err(ResourceDataError::OutOfBounds {
                resource,
                offset,
                length,
                size,
            });
        }
        Ok(Some(
            *blake3::hash(&entry.bytes[offset as usize..end as usize]).as_bytes(),
        ))
    }

    fn len(&self, resource: ResourceId) -> Result<u64, ResourceDataError> {
        let entry = self
            .resources
            .get(&resource)
            .ok_or(ResourceDataError::Missing(resource))?;
        u64::try_from(entry.bytes.len()).map_err(|_| ResourceDataError::SizeOverflow)
    }

    fn read(
        &self,
        resource: ResourceId,
        offset: u64,
        out: &mut [u8],
    ) -> Result<(), ResourceDataError> {
        let entry = self
            .resources
            .get(&resource)
            .ok_or(ResourceDataError::Missing(resource))?;
        let length = u64::try_from(out.len()).map_err(|_| ResourceDataError::SizeOverflow)?;
        let end = offset
            .checked_add(length)
            .ok_or(ResourceDataError::SizeOverflow)?;
        let size = u64::try_from(entry.bytes.len()).map_err(|_| ResourceDataError::SizeOverflow)?;
        if end > size {
            return Err(ResourceDataError::OutOfBounds {
                resource,
                offset,
                length,
                size,
            });
        }
        let start = usize::try_from(offset).map_err(|_| ResourceDataError::SizeOverflow)?;
        let end = usize::try_from(end).map_err(|_| ResourceDataError::SizeOverflow)?;
        out.copy_from_slice(&entry.bytes[start..end]);
        Ok(())
    }

    fn write(
        &mut self,
        resource: ResourceId,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ResourceDataError> {
        let entry = self
            .resources
            .get_mut(&resource)
            .ok_or(ResourceDataError::Missing(resource))?;
        if !entry.writable {
            return Err(ResourceDataError::ReadOnly(resource));
        }
        let length = u64::try_from(data.len()).map_err(|_| ResourceDataError::SizeOverflow)?;
        let end = offset
            .checked_add(length)
            .ok_or(ResourceDataError::SizeOverflow)?;
        let size = u64::try_from(entry.bytes.len()).map_err(|_| ResourceDataError::SizeOverflow)?;
        if end > size {
            return Err(ResourceDataError::OutOfBounds {
                resource,
                offset,
                length,
                size,
            });
        }
        let start = usize::try_from(offset).map_err(|_| ResourceDataError::SizeOverflow)?;
        let end = usize::try_from(end).map_err(|_| ResourceDataError::SizeOverflow)?;
        entry.bytes[start..end].copy_from_slice(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fingerprints_track_range_contents_not_provider_identity() {
        use super::*;
        let id = ResourceId(4);
        let mut first = InMemoryGpuResources::new();
        first.insert(id, vec![1, 2, 3, 4], true);
        let original = first.content_fingerprint(id, 1, 2).unwrap();
        let mut second = InMemoryGpuResources::new();
        second.insert(id, vec![9, 2, 3, 8], true);
        assert_eq!(second.content_fingerprint(id, 1, 2).unwrap(), original);
        first.write(id, 0, &[7]).unwrap();
        assert_eq!(first.content_fingerprint(id, 1, 2).unwrap(), original);
        first.write(id, 1, &[8]).unwrap();
        assert_ne!(first.content_fingerprint(id, 1, 2).unwrap(), original);
        assert!(first.content_fingerprint(id, 3, 2).is_err());
        assert!(first.content_fingerprint(id, u64::MAX, 1).is_err());
        assert!(first.content_fingerprint(ResourceId(99), 0, 1).is_err());
    }

    use super::*;

    #[test]
    fn bounded_round_trip() {
        let id = ResourceId(9);
        let mut resources = InMemoryGpuResources::new();
        resources.insert(id, vec![0; 16], true);
        resources.write(id, 4, &[1, 2, 3, 4]).unwrap();
        let mut out = [0; 4];
        resources.read(id, 4, &mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 4]);
        assert!(matches!(
            resources.read(id, 15, &mut [0; 2]),
            Err(ResourceDataError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn read_only_payload_rejects_write() {
        let id = ResourceId(10);
        let mut resources = InMemoryGpuResources::new();
        resources.insert(id, vec![7; 4], false);
        assert!(matches!(
            resources.write(id, 0, &[1]),
            Err(ResourceDataError::ReadOnly(_))
        ));
    }
}

/// Resource provider backed by Exosphere's GPU virtual address mappings.
///
/// Multiple guest virtual addresses may alias one resource. Access is resolved by resource-relative
/// offset, so backend code never needs to know which guest alias supplied the bytes.
pub struct MappedGpuResourceData<'a, M: crate::shader_memory::GpuMemoryAccess> {
    address_space: &'a crate::address_space::GpuAddressSpace,
    memory: &'a mut M,
}

impl<'a, M: crate::shader_memory::GpuMemoryAccess> MappedGpuResourceData<'a, M> {
    pub fn new(
        address_space: &'a crate::address_space::GpuAddressSpace,
        memory: &'a mut M,
    ) -> Self {
        Self {
            address_space,
            memory,
        }
    }

    fn resolve_resource_range(
        &self,
        resource: ResourceId,
        offset: u64,
        length: u64,
        write: bool,
    ) -> Result<u64, ResourceDataError> {
        let end = offset
            .checked_add(length)
            .ok_or(ResourceDataError::SizeOverflow)?;
        let mut denied = false;
        for mapping in self.address_space.mappings() {
            if mapping.resource != resource {
                continue;
            }
            let mapping_start = mapping.resource_offset;
            let mapping_end = mapping_start
                .checked_add(mapping.size)
                .ok_or(ResourceDataError::SizeOverflow)?;
            if offset >= mapping_start && end <= mapping_end {
                if write && !mapping.permissions.write {
                    denied = true;
                    continue;
                }
                if !write && !mapping.permissions.read {
                    denied = true;
                    continue;
                }
                return mapping
                    .guest_base
                    .checked_add(offset - mapping_start)
                    .ok_or(ResourceDataError::SizeOverflow);
            }
        }
        // Permissions belong to each GPU alias, not the resource identity.
        // Reject only after all aliases covering the whole range were checked.
        if denied {
            return Err(if write {
                ResourceDataError::ReadOnly(resource)
            } else {
                ResourceDataError::Missing(resource)
            });
        }
        let size = self.len(resource).unwrap_or(0);
        Err(ResourceDataError::OutOfBounds {
            resource,
            offset,
            length,
            size,
        })
    }
}

impl<M: crate::shader_memory::GpuMemoryAccess> GpuResourceData for MappedGpuResourceData<'_, M> {
    fn content_fingerprint(
        &self,
        resource: ResourceId,
        offset: u64,
        length: u64,
    ) -> Result<Option<[u8; 32]>, ResourceDataError> {
        let address = self.resolve_resource_range(resource, offset, length, false)?;
        // A software hashing budget, not a guest buffer-size capability. None
        // retains explicit refresh policy for ranges we cannot certify cheaply.
        if length > 64 * 1024 * 1024 {
            return Ok(None);
        }
        let bytes = self
            .memory
            .read(address, length as usize)
            .map_err(|source| ResourceDataError::Memory { resource, source })?;
        if bytes.len() as u64 != length {
            return Err(ResourceDataError::UnexpectedReadSize {
                resource,
                expected: length,
                actual: bytes.len() as u64,
            });
        }
        Ok(Some(*blake3::hash(&bytes).as_bytes()))
    }

    fn len(&self, resource: ResourceId) -> Result<u64, ResourceDataError> {
        let mut found = false;
        let mut end = 0_u64;
        for mapping in self.address_space.mappings() {
            if mapping.resource != resource {
                continue;
            }
            found = true;
            end = end.max(
                mapping
                    .resource_offset
                    .checked_add(mapping.size)
                    .ok_or(ResourceDataError::SizeOverflow)?,
            );
        }
        if found {
            Ok(end)
        } else {
            Err(ResourceDataError::Missing(resource))
        }
    }

    fn read(
        &self,
        resource: ResourceId,
        offset: u64,
        out: &mut [u8],
    ) -> Result<(), ResourceDataError> {
        let length = u64::try_from(out.len()).map_err(|_| ResourceDataError::SizeOverflow)?;
        let address = self.resolve_resource_range(resource, offset, length, false)?;
        let bytes = self
            .memory
            .read(address, out.len())
            .map_err(|source| ResourceDataError::Memory { resource, source })?;
        if bytes.len() != out.len() {
            return Err(ResourceDataError::UnexpectedReadSize {
                resource,
                expected: length,
                actual: bytes.len() as u64,
            });
        }
        out.copy_from_slice(&bytes);
        Ok(())
    }

    fn write(
        &mut self,
        resource: ResourceId,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ResourceDataError> {
        let length = u64::try_from(data.len()).map_err(|_| ResourceDataError::SizeOverflow)?;
        let address = self.resolve_resource_range(resource, offset, length, true)?;
        self.memory
            .write(address, data)
            .map_err(|source| ResourceDataError::Memory { resource, source })
    }
}

#[cfg(test)]
mod mapped_tests {
    use super::*;
    use crate::{GpuAddressSpace, GpuMapPermissions, GpuMapping, shader_memory::LinearGpuMemory};

    #[test]
    fn searches_permitted_aliases_before_rejecting_access() {
        let id = ResourceId(79);
        let mut address_space = GpuAddressSpace::new();
        for (guest_base, permissions) in [
            (
                0x1000,
                GpuMapPermissions {
                    read: false,
                    write: false,
                    execute: false,
                },
            ),
            (0x1008, GpuMapPermissions::READ_WRITE),
        ] {
            address_space
                .map(GpuMapping {
                    guest_base,
                    size: 8,
                    resource: id,
                    resource_offset: 4,
                    permissions,
                })
                .unwrap();
        }
        let mut memory = LinearGpuMemory::new(0x1000, vec![0; 16]);
        {
            let mut provider = MappedGpuResourceData::new(&address_space, &mut memory);
            provider.write(id, 6, &[1, 2]).unwrap();
            let mut out = [0; 2];
            provider.read(id, 6, &mut out).unwrap();
            assert_eq!(out, [1, 2]);
            assert_eq!(
                provider.content_fingerprint(id, 6, 2).unwrap(),
                Some(*blake3::hash(&out).as_bytes())
            );
            assert!(matches!(
                provider.write(id, 3, &[9]),
                Err(ResourceDataError::OutOfBounds { .. })
            ));
        }
        use crate::shader_memory::GpuMemory;
        assert_eq!(memory.read(0x1000, 8).unwrap(), [0; 8]);
        address_space.unmap(0x1008).unwrap();
        let mut provider = MappedGpuResourceData::new(&address_space, &mut memory);
        assert_eq!(
            provider.write(id, 6, &[9]),
            Err(ResourceDataError::ReadOnly(id))
        );
        assert_eq!(
            provider.read(id, 6, &mut [0]),
            Err(ResourceDataError::Missing(id))
        );
    }

    #[test]
    fn resolves_resource_offsets_through_gpu_va() {
        let id = ResourceId(77);
        let mut address_space = GpuAddressSpace::new();
        address_space
            .map(GpuMapping {
                guest_base: 0x1000,
                size: 8,
                resource: id,
                resource_offset: 4,
                permissions: GpuMapPermissions::READ_WRITE,
            })
            .unwrap();
        let mut memory = LinearGpuMemory::new(0x1000, vec![10, 11, 12, 13, 14, 15, 16, 17]);
        let mut provider = MappedGpuResourceData::new(&address_space, &mut memory);
        let mut out = [0; 4];
        provider.read(id, 6, &mut out).unwrap();
        assert_eq!(out, [12, 13, 14, 15]);
        let fingerprint = provider.content_fingerprint(id, 6, 4).unwrap();
        assert_eq!(fingerprint, Some(*blake3::hash(&out).as_bytes()));
        provider.write(id, 8, &[1, 2]).unwrap();
        assert_ne!(provider.content_fingerprint(id, 6, 4).unwrap(), fingerprint);
        assert!(provider.content_fingerprint(id, 0, 4).is_err());
        assert!(provider.content_fingerprint(id, 10, 4).is_err());
        let mut check = [0; 2];
        provider.read(id, 8, &mut check).unwrap();
        assert_eq!(check, [1, 2]);
    }
}
