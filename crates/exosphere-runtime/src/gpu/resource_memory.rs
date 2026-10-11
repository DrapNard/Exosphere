// SPDX-License-Identifier: GPL-3.0-or-later
//! Translate live canonical nvmap ownership into portable resource metadata.

use exosphere_gpu_ir::{
    GpuAddressSpace, ResourceId,
    address_space::{GpuMapPermissions, GpuMapping},
};
use exosphere_horizon::{
    memory::{AddressSpace, DeviceMemoryLeaseIdentity, MemoryPermission},
    nvdrv::address_space::NvGpuAddressSpace,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestGpuAllocation {
    pub id: ResourceId,
    pub size: u64,
}

pub struct GuestGpuResourceSnapshot {
    pub address_space: GpuAddressSpace,
    pub allocations: Vec<GuestGpuAllocation>,
}

impl GuestGpuResourceSnapshot {
    /// Select real full-sized allocations for the supported vertex-array draw.
    /// Usage is the union of actual fetch and uniform roles, not a fabricated
    /// second allocation when both bindings refer to the same nvmap backing.
    pub fn prepare_draw_allocations(
        &self,
        event: &exosphere_gpu_ir::MaxwellEvent,
        shaders: &exosphere_gpu_ir::compiler::PreparedGraphicsShaders,
        attributes: &[exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface],
    ) -> Result<Vec<exosphere_gpu_ir::BufferDesc>, exosphere_gpu_ir::compiler::GpuCompileError>
    {
        use exosphere_gpu_ir::{compiler::GpuCompileError, pipeline};
        let draw = pipeline::prepare_vertex_array_draw(event, attributes, &self.address_space)
            .map_err(GpuCompileError::Draw)?;
        self.select_draw_allocations(shaders, &draw.vertex_ranges, None)
    }

    /// Select actual backing allocations for already prepared indexed fetches.
    /// Recheck the index range/binding against this snapshot; old mappings must
    /// not silently carry a previously prepared resource identity into a remap.
    pub fn prepare_indexed_draw_allocations(
        &self,
        draw: &super::index_memory::CapturedIndexedVertexInputs,
        shaders: &exosphere_gpu_ir::compiler::PreparedGraphicsShaders,
    ) -> Result<Vec<exosphere_gpu_ir::BufferDesc>, exosphere_gpu_ir::compiler::GpuCompileError>
    {
        use exosphere_gpu_ir::{Command, compiler::GpuCompileError, resource::IndexType};
        let invalid = || GpuCompileError::InvalidDrawState;
        let expected = match draw
            .selection
            .indices
            .index_read_range(draw.selection.count)
        {
            None if draw.selection.count == 0 => None,
            None => return Err(invalid()),
            Some((address, bytes)) => Some(
                self.address_space
                    .resolve(address, bytes)
                    .map_err(|_| invalid())?,
            ),
        };
        if expected != draw.index_range {
            return Err(invalid());
        }
        match (&draw.index_binding, expected) {
            (None, None) => {}
            (
                Some(Command::BindIndexBuffer {
                    buffer,
                    offset,
                    index_type,
                }),
                Some(range),
            ) => {
                let (kind, width) = match draw.selection.indices.index_size_raw {
                    0 => (IndexType::U8, 1),
                    1 => (IndexType::U16, 2),
                    2 => (IndexType::U32, 4),
                    _ => return Err(invalid()),
                };
                if *buffer != range.resource
                    || *offset != range.offset
                    || *index_type != kind
                    || range.offset % width != 0
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        let vertex_ranges: Vec<_> = draw.inputs.iter().map(|input| input.range).collect();
        self.select_draw_allocations(shaders, &vertex_ranges, expected)
    }

    fn select_draw_allocations(
        &self,
        shaders: &exosphere_gpu_ir::compiler::PreparedGraphicsShaders,
        vertex_ranges: &[exosphere_gpu_ir::address_space::ResolvedGpuRange],
        index_range: Option<exosphere_gpu_ir::address_space::ResolvedGpuRange>,
    ) -> Result<Vec<exosphere_gpu_ir::BufferDesc>, exosphere_gpu_ir::compiler::GpuCompileError>
    {
        use exosphere_gpu_ir::{
            BufferDesc, BufferUsage, Command,
            compiler::{self, GpuCompileError},
            pipeline,
        };
        let (_, bindings) = compiler::merge_graphics_resources(&shaders.vertex, &shaders.fragment)?;
        let mut roles = std::collections::BTreeMap::new();
        for range in vertex_ranges {
            roles
                .entry(range.resource)
                .or_insert((false, false, false))
                .0 = true;
        }
        for binding in bindings {
            if let Command::BindBufferDescriptor { buffer, .. } = binding {
                roles.entry(buffer).or_insert((false, false, false)).1 = true;
            }
        }
        if let Some(range) = index_range {
            roles
                .entry(range.resource)
                .or_insert((false, false, false))
                .2 = true;
        }
        let mut allocations = Vec::new();
        for (id, (vertex, uniform, index)) in roles {
            let mut matches = self
                .allocations
                .iter()
                .filter(|allocation| allocation.id == id);
            let allocation = matches
                .next()
                .ok_or(GpuCompileError::InvalidBufferAllocation(id))?;
            if matches.next().is_some() {
                return Err(GpuCompileError::InvalidBufferAllocation(id));
            }
            let usage = match (vertex, uniform, index) {
                (false, false, true) => BufferUsage::Index,
                (_, _, true) => BufferUsage::IndexVertexUniform,
                (true, true, false) => BufferUsage::VertexUniform,
                (true, false, false) => BufferUsage::Vertex,
                (false, true, false) => BufferUsage::Uniform,
                (false, false, false) => {
                    unreachable!("every selected allocation has a binding role")
                }
            };
            allocations.push(BufferDesc {
                id,
                size: allocation.size,
                usage,
            });
        }
        pipeline::vertex_range_buffers(vertex_ranges, &allocations)
            .ok_or(GpuCompileError::InvalidDrawState)?;
        if let Some(range) = index_range {
            let allocation = allocations
                .iter()
                .find(|allocation| allocation.id == range.resource)
                .ok_or(GpuCompileError::InvalidBufferAllocation(range.resource))?;
            if !range.permissions.read
                || range.length == 0
                || range
                    .offset
                    .checked_add(range.length)
                    .is_none_or(|end| end > allocation.size)
            {
                return Err(GpuCompileError::InvalidBufferAllocation(range.resource));
            }
        }
        compiler::graphics_uniform_buffers(&shaders.vertex, &shaders.fragment, &allocations)?;
        Ok(allocations)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GuestGpuResourceError {
    #[error("GPU resource identities exhausted")]
    IdentityExhausted,
    #[error(transparent)]
    Memory(#[from] exosphere_horizon::HorizonError),
    #[error(transparent)]
    Mapping(#[from] exosphere_gpu_ir::address_space::GpuAddressError),
}

/// One renderer namespace's allocation identities. Tokens are weak: this
/// catalog must not prevent nvmap teardown. Do not share IDs with another native
/// resource namespace unless its allocator reserves the same sequence.
#[derive(Debug, Clone)]
pub struct GuestGpuResourceCatalog {
    next_id: u64,
    identities: Vec<(DeviceMemoryLeaseIdentity, ResourceId)>,
}

impl GuestGpuResourceCatalog {
    pub fn new(first_id: u64) -> Self {
        Self {
            next_id: first_id,
            identities: Vec::new(),
        }
    }

    /// Refresh metadata atomically. All actual memory operations still use live
    /// canonical access validation; a snapshot grants no access after unmap.
    pub fn refresh(
        &mut self,
        memory: &AddressSpace,
        gpu: &NvGpuAddressSpace,
    ) -> Result<GuestGpuResourceSnapshot, GuestGpuResourceError> {
        let mut updated = self.clone();
        updated
            .identities
            .retain(|(identity, _)| identity.is_live());
        let mut address_space = GpuAddressSpace::new();
        let mut allocations = Vec::new();
        for mapping in gpu.mappings() {
            let size = usize::try_from(mapping.size).map_err(|_| {
                exosphere_horizon::HorizonError::InvalidAddress(mapping.gpu_address)
            })?;
            gpu.validate_access(
                memory,
                mapping.gpu_address,
                size,
                mapping.backing.lease.permission(),
            )?;
            let identity = mapping.backing.lease.allocation_identity();
            let id = match updated
                .identities
                .iter()
                .find(|(known, _)| *known == identity)
            {
                Some((_, id)) => *id,
                None => {
                    let id = ResourceId(updated.next_id);
                    updated.next_id = updated
                        .next_id
                        .checked_add(1)
                        .ok_or(GuestGpuResourceError::IdentityExhausted)?;
                    updated.identities.push((identity, id));
                    id
                }
            };
            address_space.map(GpuMapping {
                guest_base: mapping.gpu_address,
                size: mapping.size,
                resource: id,
                resource_offset: mapping.buffer_offset,
                permissions: if mapping.backing.lease.permission() == MemoryPermission::READ_WRITE {
                    GpuMapPermissions::READ_WRITE
                } else {
                    GpuMapPermissions::READ_ONLY
                },
            })?;
            if !allocations
                .iter()
                .any(|allocation: &GuestGpuAllocation| allocation.id == id)
            {
                allocations.push(GuestGpuAllocation {
                    id,
                    size: mapping.backing.lease.size() as u64,
                });
            }
        }
        *self = updated;
        Ok(GuestGpuResourceSnapshot {
            address_space,
            allocations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_horizon::{
        memory::{MemoryKind, PAGE_SIZE},
        nvdrv::NvMapBacking,
    };

    #[test]
    fn live_aliases_preserve_allocation_identity_offsets_and_permissions() {
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
        gpu.map_fixed(0x100000, PAGE_SIZE, PAGE_SIZE, &backing)
            .unwrap();
        gpu.map_fixed(0x200000, PAGE_SIZE, 0, &backing).unwrap();
        let mut catalog = GuestGpuResourceCatalog::new(7);
        let snapshot = catalog.refresh(&memory, &gpu).unwrap();
        assert_eq!(
            snapshot.allocations,
            vec![GuestGpuAllocation {
                id: ResourceId(7),
                size: 2 * PAGE_SIZE
            }]
        );
        let first = snapshot.address_space.resolve(0x100008, 4).unwrap();
        let second = snapshot.address_space.resolve(0x200008, 4).unwrap();
        assert_eq!(first.resource, second.resource);
        assert_eq!(first.offset, PAGE_SIZE + 8);
        assert_eq!(second.offset, 8);
        assert_eq!(first.permissions, GpuMapPermissions::READ_WRITE);
        assert!(catalog.refresh(&memory.clone(), &gpu).is_err());
        assert_eq!(
            catalog.refresh(&memory, &gpu).unwrap().allocations,
            snapshot.allocations
        );
        gpu.unmap(0x100000).unwrap();
        assert!(
            catalog
                .refresh(&memory, &gpu)
                .unwrap()
                .address_space
                .resolve(0x100008, 4)
                .is_err()
        );
        gpu.unmap(0x200000).unwrap();
        drop(backing);
        memory.unmap(0x4000, 2 * PAGE_SIZE).unwrap();
        memory
            .map(
                0x4000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let replacement = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, 2 * PAGE_SIZE, MemoryPermission::READ)
                .unwrap(),
        };
        gpu.map_fixed(0x100000, PAGE_SIZE, PAGE_SIZE, &replacement)
            .unwrap();
        let replaced = catalog.refresh(&memory, &gpu).unwrap();
        assert_eq!(replaced.allocations[0].id, ResourceId(8));
        assert_eq!(
            replaced
                .address_space
                .resolve(0x100008, 4)
                .unwrap()
                .permissions,
            GpuMapPermissions::READ_ONLY
        );
    }
}
