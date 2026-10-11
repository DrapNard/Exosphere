// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded shader/compiler reads through canonical guest GPU mappings.

use exosphere_gpu_ir::shader_memory::{GpuMemory, GpuMemoryAccess, GpuMemoryError};
use exosphere_horizon::{
    memory::{AddressSpace, MemoryPermission},
    nvdrv::address_space::NvGpuAddressSpace,
};

/// Borrowed live memory, not a shader-byte or mapping snapshot. Callers retain
/// responsibility for supplying a verified shader window and executing shaders.
pub struct GuestShaderMemory<'a> {
    pub memory: &'a AddressSpace,
    pub gpu: &'a NvGpuAddressSpace,
}

pub struct DirectDrawShaderBindings<'a> {
    pub vertex_constants: &'a [exosphere_gpu_ir::maxwell_constant::ConstantBufferInterface],
    pub fragment_constants: &'a [exosphere_gpu_ir::maxwell_constant::ConstantBufferInterface],
    pub vertex_group: u8,
    pub fragment_group: u8,
}

pub struct PreparedDirectDrawShaders {
    pub shaders: exosphere_gpu_ir::compiler::PreparedGraphicsShaders,
    pub attributes: Vec<exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface>,
}

/// Explicit stage-group ownership and host descriptor namespace policy.
#[derive(Debug, Clone, Copy)]
pub struct DirectDrawShaderGroups {
    pub vertex_group: u8,
    pub fragment_group: u8,
    pub vertex_set: u32,
    pub fragment_set: u32,
}

/// Discover direct constant/vertex interfaces from the compiled programs and
/// captured bindings. Program windows still require verified lengths.
pub fn prepare_discovered_direct_draw_shaders(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    state: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot,
    windows: &[exosphere_gpu_ir::compiler::ShaderWindow],
    groups: &DirectDrawShaderGroups,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<PreparedDirectDrawShaders, exosphere_gpu_ir::compiler::GpuCompileError> {
    let shaders = exosphere_gpu_ir::compiler::compile_draw_shaders(
        &GuestShaderMemory { memory, gpu },
        state,
        windows,
    )?;
    prepare_discovered_compiled_draw_shaders(&shaders, state, groups, resources)
}

/// Event-aware preparation preserves FIRST/index bias for shader VertexID.
pub fn prepare_discovered_event_draw_shaders(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    event: &exosphere_gpu_ir::MaxwellEvent,
    windows: &[exosphere_gpu_ir::compiler::ShaderWindow],
    groups: &DirectDrawShaderGroups,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<PreparedDirectDrawShaders, exosphere_gpu_ir::compiler::GpuCompileError> {
    use exosphere_gpu_ir::{MaxwellEvent, ShaderStage, compiler::GpuCompileError};
    let state = match event {
        MaxwellEvent::DrawVertexArray { state, .. }
        | MaxwellEvent::DrawIndexBuffer { state, .. } => state,
        _ => return Err(GpuCompileError::InvalidDrawState),
    };
    let mut shaders = exosphere_gpu_ir::compiler::compile_draw_shaders(
        &GuestShaderMemory { memory, gpu },
        state,
        windows,
    )?;
    for shader in &mut shaders {
        if shader.module.stage == ShaderStage::Vertex {
            shader.module = exosphere_gpu_ir::maxwell_vertex::specialize_vertex_id_attribute(
                &shader.module,
                event,
            )
            .map_err(GpuCompileError::Interface)?;
        }
    }
    prepare_discovered_compiled_draw_shaders(&shaders, state, groups, resources)
}

fn prepare_discovered_compiled_draw_shaders(
    shaders: &[exosphere_gpu_ir::compiler::CompiledShader],
    state: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot,
    groups: &DirectDrawShaderGroups,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<PreparedDirectDrawShaders, exosphere_gpu_ir::compiler::GpuCompileError> {
    use exosphere_gpu_ir::{ShaderStage, compiler::GpuCompileError};
    let select = |stage| {
        shaders
            .iter()
            .find(|shader| shader.module.stage == stage)
            .ok_or(GpuCompileError::InvalidDrawState)
    };
    let vertex = exosphere_gpu_ir::maxwell_constant::discover_direct_constant_interface(
        &select(ShaderStage::Vertex)?.module,
        state,
        groups.vertex_group,
        groups.vertex_set,
    )
    .map_err(GpuCompileError::Interface)?;
    let fragment = exosphere_gpu_ir::maxwell_constant::discover_direct_constant_interface(
        &select(ShaderStage::Fragment)?.module,
        state,
        groups.fragment_group,
        groups.fragment_set,
    )
    .map_err(GpuCompileError::Interface)?;
    prepare_direct_compiled_shaders(
        shaders,
        state,
        &DirectDrawShaderBindings {
            vertex_constants: &vertex,
            fragment_constants: &fragment,
            vertex_group: groups.vertex_group,
            fragment_group: groups.fragment_group,
        },
        resources,
    )
}

/// Capture every direct fragment TEX handle from live channel memory. The
/// renderer owns resource IDs; bindings use the fragment set above hardware
/// constant-buffer slots, never a TIC pool index masquerading as an image ID.
pub fn prepare_textured_event_draw_shaders(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    event: &exosphere_gpu_ir::MaxwellEvent,
    windows: &[exosphere_gpu_ir::compiler::ShaderWindow],
    groups: &DirectDrawShaderGroups,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
    mut reserve: impl FnMut(
        &exosphere_gpu_ir::tic_tsc::TextureHandleRecords,
    ) -> Result<
        exosphere_gpu_ir::texture_sampling::PreparedTexture2d,
        exosphere_gpu_ir::compiler::GpuCompileError,
    >,
) -> Result<PreparedDirectDrawShaders, exosphere_gpu_ir::compiler::GpuCompileError> {
    use exosphere_gpu_ir::{
        MaxwellEvent, ShaderOperation as S, ShaderStage, compiler::GpuCompileError,
    };
    let state = match event {
        MaxwellEvent::DrawVertexArray { state, .. }
        | MaxwellEvent::DrawIndexBuffer { state, .. } => state,
        _ => return Err(GpuCompileError::InvalidDrawState),
    };
    let live = GuestShaderMemory { memory, gpu };
    let mut shaders = exosphere_gpu_ir::compiler::compile_draw_shaders(&live, state, windows)?;
    if shaders.len() != 2 {
        return Err(GpuCompileError::InvalidDrawState);
    }
    for shader in &mut shaders {
        if shader.module.stage == ShaderStage::Vertex {
            shader.module = exosphere_gpu_ir::maxwell_vertex::specialize_vertex_id_attribute(
                &shader.module,
                event,
            )
            .map_err(GpuCompileError::Interface)?;
        }
    }
    let vertex = shaders
        .iter()
        .find(|s| s.module.stage == ShaderStage::Vertex)
        .ok_or(GpuCompileError::InvalidDrawState)?;
    let fragment = shaders
        .iter()
        .find(|s| s.module.stage == ShaderStage::Fragment)
        .ok_or(GpuCompileError::InvalidDrawState)?;
    let vertex_constants = exosphere_gpu_ir::maxwell_constant::discover_direct_constant_interface(
        &vertex.module,
        state,
        groups.vertex_group,
        groups.vertex_set,
    )
    .map_err(GpuCompileError::Interface)?;
    let fragment_constants =
        exosphere_gpu_ir::maxwell_constant::discover_direct_constant_interface(
            &fragment.module,
            state,
            groups.fragment_group,
            groups.fragment_set,
        )
        .map_err(GpuCompileError::Interface)?;
    let lowered = vertex
        .specialize_instance_attribute(state)?
        .lower_constant_interface(&vertex_constants)?;
    let attributes =
        exosphere_gpu_ir::maxwell_vertex::discover_direct_vertex_interface(&lowered.module, state)
            .map_err(GpuCompileError::Interface)?;
    fn collect(operations: &[S], selectors: &mut std::collections::BTreeSet<u16>) {
        for operation in operations {
            match operation {
                S::SampleTexture2dHandle {
                    handle_selector, ..
                } => {
                    selectors.insert(*handle_selector);
                }
                S::Predicated { operations, .. } => collect(operations, selectors),
                _ => {}
            }
        }
    }
    let mut selectors = std::collections::BTreeSet::new();
    collect(&fragment.module.operations, &mut selectors);
    let mut views = Vec::new();
    for selector in selectors {
        let records = exosphere_gpu_ir::tic_tsc::read_texture_handle_records(
            state,
            groups.fragment_group,
            selector,
            resources,
            &live,
        )
        .map_err(GpuCompileError::TextureDescriptor)?;
        views.push((selector, reserve(&records)?));
    }
    let interface: Vec<_> = views
        .iter()
        .enumerate()
        .map(|(index, (selector, view))| {
            exosphere_gpu_ir::texture_sampling::TextureShaderInterface {
                selector: *selector,
                image: [groups.fragment_set, 32 + index as u32 * 2],
                sampler: [groups.fragment_set, 33 + index as u32 * 2],
                view,
            }
        })
        .collect();
    let vertex = vertex.prepare_vertex(
        &attributes,
        &vertex_constants,
        state,
        groups.vertex_group,
        resources,
    )?;
    let fragment = fragment.prepare_fragment_with_textures(
        &fragment_constants,
        state,
        groups.fragment_group,
        resources,
        &interface,
    )?;
    let (layout, bindings) =
        exosphere_gpu_ir::compiler::merge_graphics_resources(&vertex, &fragment)?;
    Ok(PreparedDirectDrawShaders {
        attributes,
        shaders: exosphere_gpu_ir::compiler::PreparedGraphicsShaders {
            vertex,
            fragment,
            layout,
            bindings,
        },
    })
}

/// Derive direct vertex inputs after resolving constant accesses, then use the
/// same interface for shader preparation and subsequent draw/fetch assembly.
/// Program lengths and constant descriptor interfaces still require evidence
/// from the caller; this does not infer those from mapped allocation sizes.
pub fn prepare_direct_draw_shaders(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    state: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot,
    windows: &[exosphere_gpu_ir::compiler::ShaderWindow],
    bindings: &DirectDrawShaderBindings<'_>,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<PreparedDirectDrawShaders, exosphere_gpu_ir::compiler::GpuCompileError> {
    let shaders = exosphere_gpu_ir::compiler::compile_draw_shaders(
        &GuestShaderMemory { memory, gpu },
        state,
        windows,
    )?;
    prepare_direct_compiled_shaders(&shaders, state, bindings, resources)
}

fn prepare_direct_compiled_shaders(
    shaders: &[exosphere_gpu_ir::compiler::CompiledShader],
    state: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot,
    bindings: &DirectDrawShaderBindings<'_>,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<PreparedDirectDrawShaders, exosphere_gpu_ir::compiler::GpuCompileError> {
    use exosphere_gpu_ir::compiler::{GpuCompileError, GraphicsShaderInterfaces};
    let vertex = shaders
        .iter()
        .find(|shader| shader.module.stage == exosphere_gpu_ir::ShaderStage::Vertex)
        .ok_or(GpuCompileError::InvalidDrawState)?;
    let lowered = vertex
        .specialize_instance_attribute(state)?
        .lower_constant_interface(bindings.vertex_constants)?;
    let attributes =
        exosphere_gpu_ir::maxwell_vertex::discover_direct_vertex_interface(&lowered.module, state)
            .map_err(GpuCompileError::Interface)?;
    let shaders = exosphere_gpu_ir::compiler::prepare_graphics_shaders(
        shaders,
        &GraphicsShaderInterfaces {
            attributes: &attributes,
            vertex_constants: bindings.vertex_constants,
            fragment_constants: bindings.fragment_constants,
            vertex_group: bindings.vertex_group,
            fragment_group: bindings.fragment_group,
        },
        state,
        resources,
    )?;
    Ok(PreparedDirectDrawShaders {
        shaders,
        attributes,
    })
}

/// Discover programs from this draw's immutable slot capture, compile through
/// live canonical memory, then resolve explicitly supplied shader interfaces.
/// Windows must have verified lengths: mappings alone do not identify a shader
/// program's end, and unsupported extra stages must not be silently discarded.
pub fn prepare_draw_shaders(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    state: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot,
    windows: &[exosphere_gpu_ir::compiler::ShaderWindow],
    interfaces: &exosphere_gpu_ir::compiler::GraphicsShaderInterfaces<'_>,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<
    exosphere_gpu_ir::compiler::PreparedGraphicsShaders,
    exosphere_gpu_ir::compiler::GpuCompileError,
> {
    let shaders = exosphere_gpu_ir::compiler::compile_draw_shaders(
        &GuestShaderMemory { memory, gpu },
        state,
        windows,
    )?;
    exosphere_gpu_ir::compiler::prepare_graphics_shaders(&shaders, interfaces, state, resources)
}

impl GpuMemory for GuestShaderMemory<'_> {
    fn read(&self, address: u64, size: usize) -> Result<Vec<u8>, GpuMemoryError> {
        address
            .checked_add(size as u64)
            .ok_or(GpuMemoryError::AddressOverflow)?;
        let failed = |reason| GpuMemoryError::BackendRead {
            address,
            size,
            reason,
        };
        // A software read budget, not a presumed hardware shader-size limit.
        if size > exosphere_gpu_ir::compiler::MAX_SHADER_WINDOW_BYTES {
            return Err(failed("shader read budget exceeded".into()));
        }
        self.gpu
            .validate_access(self.memory, address, size, MemoryPermission::READ)
            .map_err(|error| failed(error.to_string()))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| failed("shader read allocation failed".into()))?;
        bytes.resize(size, 0);
        self.gpu
            .read(self.memory, address, &mut bytes)
            .map_err(|error| failed(error.to_string()))?;
        Ok(bytes)
    }
}

/// Read/write bridge for mapped resource providers. Device leases and range
/// permissions remain owned and validated by the canonical nvmap address space.
pub struct GuestGpuResourceMemory<'a> {
    pub memory: &'a mut AddressSpace,
    pub gpu: &'a NvGpuAddressSpace,
}

#[derive(Debug)]
pub enum ConstantUploadError {
    InvalidSelector,
    Memory(GpuMemoryError),
}

impl std::fmt::Display for ConstantUploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSelector => f.write_str("invalid constant-buffer upload selector"),
            Self::Memory(error) => write!(f, "constant-buffer upload: {error}"),
        }
    }
}

impl std::error::Error for ConstantUploadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Memory(error) => Some(error),
            Self::InvalidSelector => None,
        }
    }
}

/// Execute one upload word. The live memory bridge preflights mapped segments
/// before writing; cursor advancement occurs only after the write succeeds.
pub fn upload_constant_word(
    state: &mut exosphere_gpu_ir::Maxwell3dState,
    word: u32,
    memory: &mut dyn GpuMemoryAccess,
) -> Result<(), ConstantUploadError> {
    use exosphere_gpu_ir::maxwell_state::method;
    let address = state
        .constant_buffer_upload_address()
        .ok_or(ConstantUploadError::InvalidSelector)?;
    let offset = state.raw_registers[&method::CONSTANT_BUFFER_UPLOAD_OFFSET];
    if offset & 3 != 0 {
        return Err(ConstantUploadError::InvalidSelector);
    }
    memory
        .write(address, &word.to_le_bytes())
        .map_err(ConstantUploadError::Memory)?;
    // Retain the end cursor even if the next word exceeds the descriptor. Never
    // wrap a completed upload back into the beginning of the constant buffer.
    state
        .raw_registers
        .insert(method::CONSTANT_BUFFER_UPLOAD_OFFSET, offset + 4);
    Ok(())
}

#[derive(Debug)]
pub enum VertexReadError {
    Descriptor(exosphere_gpu_ir::maxwell_state::VertexStreamResolveError),
    Payload(exosphere_gpu_ir::ResourceDataError),
}

impl std::fmt::Display for VertexReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Descriptor(error) => write!(f, "vertex fetch descriptor: {error}"),
            Self::Payload(error) => write!(f, "vertex fetch payload: {error}"),
        }
    }
}

impl std::error::Error for VertexReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Descriptor(error) => error,
            Self::Payload(error) => error,
        })
    }
}

/// Read exact raw fetch bytes, without guessing shader component conversion or
/// vertex/instance selection. The live provider validates nvmap leases again.
pub fn read_vertex_element(
    state: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot,
    attribute_index: usize,
    element_index: u64,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
    provider: &dyn exosphere_gpu_ir::GpuResourceData,
) -> Result<Vec<u8>, VertexReadError> {
    let range = state
        .resolve_vertex_element(attribute_index, element_index, resources)
        .map_err(VertexReadError::Descriptor)?;
    // Supported attribute widths are at most 16 bytes; resolution rejects all
    // unsupported formats before allocation or payload access.
    let mut bytes = vec![0; range.length as usize];
    provider
        .read(range.resource, range.offset, &mut bytes)
        .map_err(VertexReadError::Payload)?;
    Ok(bytes)
}

#[derive(Debug)]
pub enum ConstantReadError {
    Descriptor(exosphere_gpu_ir::maxwell_state::ConstantBufferReadError),
    Payload(exosphere_gpu_ir::ResourceDataError),
}

impl std::fmt::Display for ConstantReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Descriptor(error) => write!(f, "constant fetch descriptor: {error}"),
            Self::Payload(error) => write!(f, "constant fetch payload: {error}"),
        }
    }
}

impl std::error::Error for ConstantReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Descriptor(error) => error,
            Self::Payload(error) => error,
        })
    }
}

/// Read raw constant-buffer bytes through live resource leases. No float or
/// integer conversion is applied: the shader instruction determines their type.
pub fn read_constant_buffer(
    state: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot,
    group: u8,
    slot: u8,
    offset: u64,
    length: u64,
    resources: &exosphere_gpu_ir::GpuAddressSpace,
    provider: &dyn exosphere_gpu_ir::GpuResourceData,
) -> Result<Vec<u8>, ConstantReadError> {
    let range = state
        .resolve_constant_buffer_read(group, slot, offset, length, resources)
        .map_err(ConstantReadError::Descriptor)?;
    // Descriptor validation bounds allocation to the 17-bit hardware size field.
    let mut bytes = vec![0; range.length as usize];
    provider
        .read(range.resource, range.offset, &mut bytes)
        .map_err(ConstantReadError::Payload)?;
    Ok(bytes)
}

/// Access backend resources through the authenticated channel's live mappings.
/// The provider cannot outlive this borrow or retain a snapshot of nvmap state.
pub fn with_channel_resources<T>(
    kernel: &mut exosphere_horizon::kernel::HorizonKernel,
    process_id: u64,
    session_handle: exosphere_horizon::handle::Handle,
    descriptor: u32,
    resources: &exosphere_gpu_ir::address_space::GpuAddressSpace,
    action: impl FnOnce(&mut dyn exosphere_gpu_ir::GpuResourceData) -> T,
) -> exosphere_horizon::HorizonResult<T> {
    kernel.with_gpu_channel(process_id, session_handle, descriptor, |_, memory, gpu| {
        let mut live = GuestGpuResourceMemory { memory, gpu };
        let mut provider = exosphere_gpu_ir::MappedGpuResourceData::new(resources, &mut live);
        action(&mut provider)
    })
}

impl GpuMemory for GuestGpuResourceMemory<'_> {
    fn read(&self, address: u64, size: usize) -> Result<Vec<u8>, GpuMemoryError> {
        GuestShaderMemory {
            memory: self.memory,
            gpu: self.gpu,
        }
        .read(address, size)
    }
}

impl GpuMemoryAccess for GuestGpuResourceMemory<'_> {
    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), GpuMemoryError> {
        address
            .checked_add(data.len() as u64)
            .ok_or(GpuMemoryError::AddressOverflow)?;
        let failed = |reason| GpuMemoryError::BackendWrite {
            address,
            size: data.len(),
            reason,
        };
        if data.len() > exosphere_gpu_ir::compiler::MAX_SHADER_WINDOW_BYTES {
            return Err(failed("GPU resource write budget exceeded".into()));
        }
        // Nvmap validates all segments before touching any guest bytes.
        self.gpu
            .write(self.memory, address, data)
            .map_err(|error| failed(error.to_string()))
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
    fn encoded_constant_upload_rejects_read_only_backing_without_memory_changes() {
        use exosphere_gpu_ir::maxwell_state::method;
        let mut memory = AddressSpace::default();
        memory
            .map(0x4000, PAGE_SIZE, MemoryPermission::READ, MemoryKind::Heap)
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: PAGE_SIZE as u32,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
        let mut frontend = exosphere_gpu_ir::MaxwellFrontEnd::new();
        let trace = frontend
            .process_pushbuffer(&[
                0x60010000,
                0xb197,
                (1 << 29) | (3 << 16) | u32::from(method::CONSTANT_BUFFER_SELECTOR_SIZE),
                8,
                0,
                0x100000,
                (5 << 29) | (3 << 16) | u32::from(method::CONSTANT_BUFFER_UPLOAD_OFFSET),
                0,
                0x12345678,
                0x87654321,
            ])
            .unwrap();
        let failure = super::super::execute_trace(&mut memory, &gpu, &trace).unwrap_err();
        assert_eq!(failure.completed_transfers, 0);
        assert!(matches!(
            failure.reason,
            super::super::GuestGpuExecutionFailure::Transfer(_)
        ));
        let mut bytes = [0; 8];
        memory.read(0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);
    }

    #[test]
    fn resource_fingerprints_follow_cpu_writes_and_gpu_remaps() {
        use exosphere_gpu_ir::address_space::{GpuAddressSpace, GpuMapPermissions, GpuMapping};
        use exosphere_gpu_ir::{GpuResourceData, MappedGpuResourceData, ResourceId};

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
            alignment: u32::try_from(PAGE_SIZE).unwrap(),
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, 2 * PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
        let id = ResourceId(78);
        let mut resources = GpuAddressSpace::new();
        resources
            .map(GpuMapping {
                guest_base: 0x100000,
                size: 8,
                resource: id,
                resource_offset: 0,
                permissions: GpuMapPermissions::READ_WRITE,
            })
            .unwrap();
        let mut draw = exosphere_gpu_ir::maxwell_state::DrawStateSnapshot::default();
        draw.constant_buffers.insert(
            (0, 0),
            exosphere_gpu_ir::maxwell_state::ConstantBufferBinding {
                size_raw: 8,
                address: exosphere_gpu_ir::maxwell_state::RawGpuAddress { a: 0, b: 0x100000 },
                bind_raw: 1,
            },
        );
        use exosphere_gpu_ir::maxwell_state::method;
        for (offset, word) in [0x1008, 0, 0x100000, 0].into_iter().enumerate() {
            draw.raw_registers
                .insert(method::VERTEX_STREAM_BASE + offset as u16, word);
        }
        draw.raw_registers
            .insert(method::VERTEX_STREAM_LIMIT_BASE, 0);
        draw.raw_registers
            .insert(method::VERTEX_STREAM_LIMIT_BASE + 1, 0x100007);
        draw.raw_registers
            .insert(method::VERTEX_ATTRIBUTE_BASE, (4 << 21) | (7 << 27));
        let vertex = |memory: &mut AddressSpace, gpu: &NvGpuAddressSpace| {
            let mut live = GuestGpuResourceMemory { memory, gpu };
            let provider = MappedGpuResourceData::new(&resources, &mut live);
            read_vertex_element(&draw, 0, 0, &resources, &provider)
        };
        let constants = |memory: &mut AddressSpace, gpu: &NvGpuAddressSpace| {
            let mut live = GuestGpuResourceMemory { memory, gpu };
            let provider = MappedGpuResourceData::new(&resources, &mut live);
            read_constant_buffer(&draw, 0, 0, 4, 4, &resources, &provider)
        };
        assert_eq!(constants(&mut memory, &gpu).unwrap(), vec![0; 4]);
        assert_eq!(vertex(&mut memory, &gpu).unwrap(), vec![0; 8]);
        let fingerprint = |memory: &mut AddressSpace, gpu: &NvGpuAddressSpace| {
            let mut live = GuestGpuResourceMemory { memory, gpu };
            MappedGpuResourceData::new(&resources, &mut live).content_fingerprint(id, 0, 8)
        };
        let initial = fingerprint(&mut memory, &gpu).unwrap();
        assert!(initial.is_some());
        memory.write(0x4000, &[1; 8]).unwrap();
        assert_eq!(vertex(&mut memory, &gpu).unwrap(), vec![1; 8]);
        assert_eq!(constants(&mut memory, &gpu).unwrap(), vec![1; 4]);
        let modified = fingerprint(&mut memory, &gpu).unwrap();
        assert_ne!(modified, initial);
        memory.write(0x4008, &[2; 8]).unwrap();
        assert_eq!(fingerprint(&mut memory, &gpu).unwrap(), modified);

        // Resource identity stays fixed while its GPU address acquires different
        // backing. The provider must consult the live mapping, not cached bytes.
        memory.write(0x5000, &[3; 8]).unwrap();
        gpu.unmap(0x100000).unwrap();
        assert!(matches!(
            constants(&mut memory, &gpu),
            Err(ConstantReadError::Payload(_))
        ));
        assert!(matches!(
            vertex(&mut memory, &gpu),
            Err(VertexReadError::Payload(_))
        ));
        let failure = fingerprint(&mut memory, &gpu).unwrap_err();
        assert!(matches!(
            &failure,
            exosphere_gpu_ir::ResourceDataError::Memory {
                resource,
                source: GpuMemoryError::BackendRead { address: 0x100000, size: 8, .. },
            } if *resource == id
        ));
        assert!(std::error::Error::source(&failure).is_some());
        gpu.map_fixed(0x100000, PAGE_SIZE, PAGE_SIZE, &backing)
            .unwrap();
        assert_eq!(vertex(&mut memory, &gpu).unwrap(), vec![3; 8]);
        assert_eq!(constants(&mut memory, &gpu).unwrap(), vec![3; 4]);
        {
            let mut live = GuestGpuResourceMemory {
                memory: &mut memory,
                gpu: &gpu,
            };
            let provider = MappedGpuResourceData::new(&resources, &mut live);
            assert!(matches!(
                read_constant_buffer(&draw, 0, 0, 7, 2, &resources, &provider),
                Err(ConstantReadError::Descriptor(_))
            ));
            assert!(matches!(
                read_constant_buffer(&draw, 0, 1, 0, 4, &resources, &provider),
                Err(ConstantReadError::Descriptor(_))
            ));
        }
        assert_ne!(fingerprint(&mut memory, &gpu).unwrap(), modified);
        memory.write(0x5000, &[1; 8]).unwrap();
        assert_eq!(fingerprint(&mut memory, &gpu).unwrap(), modified);
        let mut upload = exosphere_gpu_ir::Maxwell3dState::default();
        for (method, value) in [
            (method::CONSTANT_BUFFER_SELECTOR_SIZE, 8),
            (method::CONSTANT_BUFFER_SELECTOR_HIGH, 0),
            (method::CONSTANT_BUFFER_SELECTOR_LOW, 0x100000),
            (method::CONSTANT_BUFFER_UPLOAD_OFFSET, 0),
        ] {
            upload.raw_registers.insert(method, value);
        }
        {
            let mut live = GuestGpuResourceMemory {
                memory: &mut memory,
                gpu: &gpu,
            };
            upload_constant_word(&mut upload, 0x80000000, &mut live).unwrap();
            upload_constant_word(&mut upload, 0x7fa12345, &mut live).unwrap();
            assert!(matches!(
                upload_constant_word(&mut upload, 0, &mut live),
                Err(ConstantUploadError::InvalidSelector)
            ));
        }
        assert_eq!(
            upload.raw_registers[&method::CONSTANT_BUFFER_UPLOAD_OFFSET],
            8
        );
        let mut frontend = exosphere_gpu_ir::MaxwellFrontEnd::new();
        let trace = frontend
            .process_pushbuffer(&[
                (3 << 29) | (1 << 16),
                0xb197, // Bind graphics engine to subchannel zero.
                (1 << 29) | (3 << 16) | u32::from(method::CONSTANT_BUFFER_SELECTOR_SIZE),
                8,
                0,
                0x100000,
                // One-increment packet writes offset once, then repeats the upload method.
                (5 << 29) | (3 << 16) | u32::from(method::CONSTANT_BUFFER_UPLOAD_OFFSET),
                0,
                0x80000000,
                0x7fa12345,
            ])
            .unwrap();
        memory.write(0x5000, &[0; 8]).unwrap();
        super::super::execute_trace(&mut memory, &gpu, &trace).unwrap();
        let waiting = frontend
            .process_pushbuffer(&[
                (5 << 29) | (2 << 16) | u32::from(method::CONSTANT_BUFFER_UPLOAD_OFFSET),
                0,
                0x80000000,
                0x20040004,
                0,
                0x100100,
                42,
                1, // FIFO equality acquire.
                (3 << 29) | (1 << 16) | u32::from(method::CONSTANT_BUFFER_UPLOAD_BASE),
                0x7fa12345,
            ])
            .unwrap();
        memory.write(0x5000, &[0; 8]).unwrap();
        memory.write(0x5100, &0u32.to_le_bytes()).unwrap();
        let mut executor = super::super::GuestGpuExecutor::default();
        let paused = executor.execute(&mut memory, &gpu, &waiting).unwrap_err();
        assert_eq!(paused.completed_transfers, 1);
        assert!(matches!(
            paused.reason,
            super::super::GuestGpuExecutionFailure::SemaphoreWait
        ));
        memory.write(0x5000, &0x12345678u32.to_le_bytes()).unwrap();
        memory.write(0x5100, &42u32.to_le_bytes()).unwrap();
        assert_eq!(
            executor
                .execute_from(&mut memory, &gpu, &waiting, paused.event_index)
                .unwrap(),
            1
        );
        let mut resumed = [0; 8];
        memory.read(0x5000, &mut resumed).unwrap();
        assert_eq!(
            u32::from_le_bytes(resumed[..4].try_into().unwrap()),
            0x12345678
        );
        assert_eq!(
            u32::from_le_bytes(resumed[4..].try_into().unwrap()),
            0x7fa12345
        );
        let first_packet = frontend
            .process_pushbuffer(&[
                (5 << 29) | (2 << 16) | u32::from(method::CONSTANT_BUFFER_UPLOAD_OFFSET),
                0,
                0x80000000,
            ])
            .unwrap();
        executor.execute(&mut memory, &gpu, &first_packet).unwrap();
        let second_packet = frontend
            .process_pushbuffer(&[
                (3 << 29) | (1 << 16) | u32::from(method::CONSTANT_BUFFER_UPLOAD_BASE),
                0x7fa12345,
            ])
            .unwrap();
        assert!(matches!(
            second_packet.graphics_events.as_slice(),
            [exosphere_gpu_ir::MaxwellEvent::ConstantBufferUpload {
                address: Some(0x100004),
                ..
            }]
        ));
        executor.execute(&mut memory, &gpu, &second_packet).unwrap();
        let invalid = frontend
            .process_pushbuffer(&[
                (5 << 29) | (4 << 16) | u32::from(method::CONSTANT_BUFFER_UPLOAD_OFFSET),
                0,
                0x80000000,
                0x7fa12345,
                0,
                (5 << 29) | (2 << 16) | u32::from(method::CONSTANT_BUFFER_UPLOAD_OFFSET),
                0,
                0, // Must not overwrite the committed prefix after the invalid third word.
            ])
            .unwrap();
        let failure = super::super::execute_trace(&mut memory, &gpu, &invalid).unwrap_err();
        assert_eq!(failure.completed_transfers, 2);
        assert!(matches!(
            failure.reason,
            super::super::GuestGpuExecutionFailure::UnsupportedEvent(_)
        ));
        let mut uploaded = vec![0; 8];
        memory.read(0x5000, &mut uploaded).unwrap();
        assert_eq!(
            uploaded,
            [0x80000000u32, 0x7fa12345]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>()
        );
        upload
            .raw_registers
            .insert(method::CONSTANT_BUFFER_UPLOAD_OFFSET, 0);
        gpu.unmap(0x100000).unwrap();
        let mut live = GuestGpuResourceMemory {
            memory: &mut memory,
            gpu: &gpu,
        };
        assert!(matches!(
            upload_constant_word(&mut upload, 0, &mut live),
            Err(ConstantUploadError::Memory(_))
        ));
        assert_eq!(
            upload.raw_registers[&method::CONSTANT_BUFFER_UPLOAD_OFFSET],
            0
        );
    }

    #[test]
    fn shader_reads_follow_live_backing_and_reject_foreign_or_invalid_ranges() {
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
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
        gpu.map_fixed(0x101000, PAGE_SIZE, PAGE_SIZE, &backing)
            .unwrap();
        memory.write(0x4ffc, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(
            GuestShaderMemory {
                memory: &memory,
                gpu: &gpu
            }
            .read(0x100ffc, 8)
            .unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
        memory.write(0x4ffc, &[8, 7, 6, 5, 4, 3, 2, 1]).unwrap();
        let reader = GuestShaderMemory {
            memory: &memory,
            gpu: &gpu,
        };
        assert_eq!(reader.read(0x100ffc, 8).unwrap(), [8, 7, 6, 5, 4, 3, 2, 1]);
        assert!(reader.read(0x101ffc, 8).is_err());
        assert!(reader.read(0x100000, 64 * 1024 * 1024 + 1).is_err());
        assert_eq!(
            reader.read(u64::MAX, 8),
            Err(GpuMemoryError::AddressOverflow)
        );
        // Compile a draw whose shader header straddles two canonical mappings.
        // Later register changes must not redirect the earlier draw's program.
        use exosphere_gpu_ir::{
            compiler::{GpuCompileError, ShaderWindow, compile_draw_shaders},
            engine::{EngineClass, EngineMethodWrite},
            maxwell_state::{Maxwell3dState, MaxwellEvent, method},
        };
        let mut graphics = Maxwell3dState::default();
        let slot = method::PIPELINE_BASE + 5 * method::PIPELINE_STRIDE;
        let mut apply = |method, argument| {
            graphics.apply(EngineMethodWrite {
                subchannel: 0,
                class: EngineClass::Graphics,
                method,
                argument,
            })
        };
        apply(method::SET_PROGRAM_REGION_B, 0x100000);
        apply(slot, 0x51);
        apply(slot + 1, 0xffc);
        apply(slot + 3, 12);
        apply(slot + 4, 3);
        let MaxwellEvent::DrawVertexArray { state: draw, .. } =
            apply(method::DRAW_VERTEX_ARRAY, 3).unwrap()
        else {
            panic!("expected captured draw state");
        };
        apply(method::SET_PROGRAM_REGION_B, 0x200000);
        let window = [ShaderWindow {
            gpu_address: 0x100ffc,
            bytes: exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE,
        }];
        for marker in [0x5a, 0xa5] {
            memory
                .write(0x4ffc, &vec![marker; window[0].bytes])
                .unwrap();
            let compiled = compile_draw_shaders(
                &GuestShaderMemory {
                    memory: &memory,
                    gpu: &gpu,
                },
                &draw,
                &window,
            )
            .unwrap();
            assert_eq!(compiled.len(), 1);
            assert_eq!(compiled[0].address, 0x100ffc);
            assert_eq!(
                compiled[0].module.stage,
                exosphere_gpu_ir::ShaderStage::Fragment
            );
            assert_eq!(compiled[0].graphics_source.unwrap().binding, 3);
            assert!(
                compiled[0]
                    .graphics_header
                    .as_ref()
                    .unwrap()
                    .raw
                    .iter()
                    .all(|byte| *byte == marker)
            );
        }
        // Compile actual ALD/MOV32I/AST encodings across the live nvmap boundary,
        // then resolve the interface while retaining source slot/header metadata.
        let mut vertex_draw = (*draw).clone();
        vertex_draw.raw_registers.insert(slot, 0x50);
        let vertex_slot = method::PIPELINE_BASE + method::PIPELINE_STRIDE;
        for (offset, word) in [(0, 0x11), (1, 0xffc), (3, 12), (4, 3)] {
            vertex_draw.raw_registers.insert(vertex_slot + offset, word);
        }
        let operands = (3u64 << 47) | (255 << 39) | (7 << 16) | (255 << 8) | 7;
        let mut encoded = vec![0; exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE];
        encoded.extend(
            [
                0u64,
                0xefd8000000000000 | operands | (0x80 << 20),
                0x010000000007f009,
                0x0100000000000000 | (0x3f800000u64 << 20) | (7 << 16) | (15 << 12) | 10,
                0u64,
                0xeff0000000000000 | (operands & !(3 << 47)) | (1 << 47) | (0x70 << 20),
                0xeff0000000000000
                    | (operands & !((3 << 47) | 0xff))
                    | (1 << 47)
                    | (0x78 << 20)
                    | 9,
                0xe30000000007000f,
            ]
            .into_iter()
            .flat_map(u64::to_le_bytes),
        );
        memory.write(0x4ffc, &encoded).unwrap();
        let vertex_window = [ShaderWindow {
            gpu_address: 0x100ffc,
            bytes: encoded.len(),
        }];
        let compiled = compile_draw_shaders(
            &GuestShaderMemory {
                memory: &memory,
                gpu: &gpu,
            },
            &vertex_draw,
            &vertex_window,
        )
        .unwrap();
        let interface = [exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
            slot: 0,
            location: 5,
            format: exosphere_gpu_ir::VertexFormat::Rgba32Float,
        }];
        let resolved = compiled[0].lower_vertex_interface(&interface).unwrap();
        assert_eq!(resolved.graphics_source.unwrap().slot, 1);
        assert_eq!(resolved.graphics_source.unwrap().binding, 3);
        assert_eq!(resolved.address, 0x100ffc);
        assert_eq!(
            resolved.graphics_header.as_ref().unwrap().raw,
            compiled[0].graphics_header.as_ref().unwrap().raw
        );
        assert_eq!(resolved.stats.semantic, 6);
        assert!(exosphere_gpu_ir::spirv::compile(&resolved.module).is_ok());
        // Recompilation must observe raw constant changes, including nonnumeric
        // bit patterns, without mutating an already compiled shader snapshot.
        let constant_address =
            0x4ffc + exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE as u64 + 16;
        memory
            .write(
                constant_address,
                &(0x0100000000000000u64 | (0x80000000u64 << 20) | (7 << 16) | (15 << 12) | 9)
                    .to_le_bytes(),
            )
            .unwrap();
        let refreshed = compile_draw_shaders(
            &GuestShaderMemory {
                memory: &memory,
                gpu: &gpu,
            },
            &vertex_draw,
            &vertex_window,
        )
        .unwrap();
        let refreshed = refreshed[0].lower_vertex_interface(&interface).unwrap();
        for (shader, expected) in [(&resolved, 0), (&refreshed, 0x80000000)] {
            assert!(shader.module.operations.iter().any(|operation| matches!(
                operation,
                exosphere_gpu_ir::ShaderOperation::ConstantBits32 { dst: 9, value }
                    if *value == expected
            )));
            assert!(exosphere_gpu_ir::spirv::compile(&shader.module).is_ok());
        }
        // A CPU change to the encoded input address must be observed on recompile.
        let instruction_address =
            0x4ffc + exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE as u64 + 8;
        memory
            .write(
                instruction_address,
                &(0xefd8000000000000 | operands | (0x90 << 20)).to_le_bytes(),
            )
            .unwrap();
        let changed = compile_draw_shaders(
            &GuestShaderMemory {
                memory: &memory,
                gpu: &gpu,
            },
            &vertex_draw,
            &vertex_window,
        )
        .unwrap();
        let error = changed[0].lower_vertex_interface(&interface).unwrap_err();
        assert!(matches!(error, GpuCompileError::Interface(_)));
        assert!(std::error::Error::source(&error).is_some());
        assert!(exosphere_gpu_ir::spirv::compile(&resolved.module).is_ok());

        // The captured fragment slot uses the same live mappings. Its export
        // layout comes from the header read now, not cached vertex metadata.
        let mut fragment_bytes = vec![0; exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE];
        fragment_bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        let mov = |dst: u64| 0x010000000007f000 | (0x3f800000u64 << 20) | dst;
        fragment_bytes.extend(
            [0, mov(0), mov(1), mov(2), 0, mov(3), 0xe30000000007000f]
                .into_iter()
                .flat_map(u64::to_le_bytes),
        );
        memory.write(0x4ffc, &fragment_bytes).unwrap();
        let fragment_window = [ShaderWindow {
            gpu_address: 0x100ffc,
            bytes: fragment_bytes.len(),
        }];
        let compile_fragment = |memory: &AddressSpace| {
            compile_draw_shaders(
                &GuestShaderMemory { memory, gpu: &gpu },
                &draw,
                &fragment_window,
            )
            .unwrap()
        };
        let original_fragment = compile_fragment(&memory)[0]
            .lower_fragment_outputs()
            .unwrap();
        assert_eq!(original_fragment.graphics_source.unwrap().slot, 5);
        assert_eq!(original_fragment.graphics_source.unwrap().binding, 3);
        assert_eq!(original_fragment.address, 0x100ffc);
        assert_eq!(original_fragment.stats.semantic, 5);
        let original_spirv = exosphere_gpu_ir::spirv::compile(&original_fragment.module).unwrap();
        // An invalid export layout in CPU memory must fail the next resolution,
        // while the previously resolved shader remains an immutable snapshot.
        memory.write(0x4ffc + 72, &7u32.to_le_bytes()).unwrap();
        let invalid_fragment = compile_fragment(&memory);
        assert!(matches!(
            invalid_fragment[0].lower_fragment_outputs(),
            Err(GpuCompileError::Interface(_))
        ));
        assert_eq!(
            exosphere_gpu_ir::spirv::compile(&original_fragment.module).unwrap(),
            original_spirv
        );
        memory.write(0x4ffc + 72, &15u32.to_le_bytes()).unwrap();
        memory
            .write(
                0x4ffc + exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE as u64 + 8,
                &0x010000000007f000u64.to_le_bytes(),
            )
            .unwrap();
        let refreshed_fragment = compile_fragment(&memory)[0]
            .lower_fragment_outputs()
            .unwrap();
        assert_ne!(
            exosphere_gpu_ir::spirv::compile(&refreshed_fragment.module).unwrap(),
            original_spirv
        );

        use exosphere_gpu_ir::address_space::{GpuAddressSpace, GpuMapPermissions, GpuMapping};
        use exosphere_gpu_ir::{GpuResourceData, MappedGpuResourceData, ResourceId};
        let id = ResourceId(77);
        let mut resources = GpuAddressSpace::new();
        resources
            .map(GpuMapping {
                guest_base: 0x100ffc,
                size: 80,
                resource: id,
                resource_offset: 0,
                permissions: GpuMapPermissions::READ_WRITE,
            })
            .unwrap();
        {
            let mut live = GuestGpuResourceMemory {
                memory: &mut memory,
                gpu: &gpu,
            };
            let mut provider = MappedGpuResourceData::new(&resources, &mut live);
            let original = provider.content_fingerprint(id, 0, 8).unwrap();
            provider.write(id, 0, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
            assert_ne!(provider.content_fingerprint(id, 0, 8).unwrap(), original);
            let mut bytes = [0; 8];
            provider.read(id, 0, &mut bytes).unwrap();
            assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        }
        let mut written = [0; 8];
        memory.read(0x4ffc, &mut written).unwrap();
        assert_eq!(written, [1, 2, 3, 4, 5, 6, 7, 8]);
        let mut cloned = memory.clone();
        assert!(matches!(
            GuestGpuResourceMemory {
                memory: &mut cloned,
                gpu: &gpu,
            }
            .write(0x100ffc, &[9; 8]),
            Err(GpuMemoryError::BackendWrite { .. })
        ));
        assert!(matches!(
            compile_draw_shaders(
                &GuestShaderMemory {
                    memory: &cloned,
                    gpu: &gpu
                },
                &draw,
                &window
            ),
            Err(GpuCompileError::Memory(_))
        ));
        assert!(
            GuestShaderMemory {
                memory: &cloned,
                gpu: &gpu
            }
            .read(0x100ffc, 8)
            .is_err()
        );
        gpu.unmap(0x101000).unwrap();
        // A missing second segment must reject the whole write, including the
        // still-mapped first segment, rather than partially modifying the game.
        assert!(matches!(
            GuestGpuResourceMemory {
                memory: &mut memory,
                gpu: &gpu,
            }
            .write(0x100ffc, &[9; 8]),
            Err(GpuMemoryError::BackendWrite { .. })
        ));
        memory.read(0x4ffc, &mut written).unwrap();
        assert_eq!(written, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(matches!(
            compile_draw_shaders(
                &GuestShaderMemory {
                    memory: &memory,
                    gpu: &gpu
                },
                &draw,
                &window
            ),
            Err(GpuCompileError::Memory(_))
        ));
        assert!(
            GuestShaderMemory {
                memory: &memory,
                gpu: &gpu
            }
            .read(0x100ffc, 8)
            .is_err()
        );
    }
}
