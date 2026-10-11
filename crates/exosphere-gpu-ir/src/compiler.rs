// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Offline/stateful GPU compiler entry points.
//!
//! Shader lengths are explicit inputs: Maxwell command state provides entry addresses but not a
//! universal trustworthy byte length for every program. Callers may obtain windows from executable
//! memory allocation metadata, cache metadata, or capture tooling.

use crate::{
    Maxwell3dState, ShaderModule, ShaderStage, discover_graphics_shaders,
    maxwell_shader::{LiftStats, MaxwellShaderError, lift_semantic, parse_compute, parse_graphics},
    shader_memory::{GpuMemory, GpuMemoryError},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderWindow {
    pub gpu_address: u64,
    pub bytes: usize,
}

/// Host-side read budget, not a hardware maximum program length.
pub const MAX_SHADER_WINDOW_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CompiledShader {
    pub address: u64,
    /// Keep per-slot identity and bindings even when two programs have the same
    /// address/stage. In particular, vertex-cull and vertex must not be merged.
    pub graphics_source: Option<crate::shader_discovery::DiscoveredShader>,
    /// Graphics header metadata is needed for later input/output interface
    /// construction; semantic lifting alone does not preserve this header.
    pub graphics_header: Option<crate::maxwell_shader::MaxwellShaderHeader>,
    pub module: ShaderModule,
    pub stats: LiftStats,
}

impl CompiledShader {
    /// Prepare a supported vertex shader and its checked uniform resources as
    /// one result. Callers still own input buffers, pipeline merging and submission.
    pub fn prepare_vertex(
        &self,
        attributes: &[crate::maxwell_vertex::VertexAttributeInterface],
        constants: &[crate::maxwell_constant::ConstantBufferInterface],
        state: &crate::maxwell_state::DrawStateSnapshot,
        group: u8,
        address_space: &crate::GpuAddressSpace,
    ) -> Result<PreparedShader, GpuCompileError> {
        let shader = self
            .specialize_instance_attribute(state)?
            .lower_constant_interface(constants)?
            .lower_vertex_interface(attributes)?;
        shader.prepare_bindings(constants, state, group, address_space)
    }

    /// Prepare explicit fragment exports and live uniform bindings together.
    pub fn prepare_fragment(
        &self,
        constants: &[crate::maxwell_constant::ConstantBufferInterface],
        state: &crate::maxwell_state::DrawStateSnapshot,
        group: u8,
        address_space: &crate::GpuAddressSpace,
    ) -> Result<PreparedShader, GpuCompileError> {
        self.lower_constant_interface(constants)?
            .lower_fragment_outputs()?
            .prepare_bindings(constants, state, group, address_space)
    }

    /// Prepare draw-resolved TEX views together with uniforms and fragment
    /// exports. The caller reserves image/sampler IDs and provides live payload
    /// aliases; neither descriptor allocations nor GPU VAs become image IDs.
    pub fn prepare_fragment_with_textures(
        &self,
        constants: &[crate::maxwell_constant::ConstantBufferInterface],
        state: &crate::maxwell_state::DrawStateSnapshot,
        group: u8,
        address_space: &crate::GpuAddressSpace,
        textures: &[crate::texture_sampling::TextureShaderInterface<'_>],
    ) -> Result<PreparedShader, GpuCompileError> {
        let mut shader = self.lower_constant_interface(constants)?;
        shader.module = crate::texture_sampling::lower_texture_interface(&shader.module, textures)
            .map_err(GpuCompileError::Interface)?;
        let mut prepared = shader.lower_fragment_outputs()?.prepare_bindings(
            constants,
            state,
            group,
            address_space,
        )?;
        for entry in textures {
            // Descriptor reads happened during draw capture. Recheck pixel
            // ownership now rather than trusting a previously prepared remap.
            let current = crate::texture_sampling::prepare_texture_2d(
                entry.view.backing.descriptor.raw.words,
                entry.view.sampler_raw.words,
                entry.view.image.id,
                entry.view.sampler.id,
                address_space,
            )
            .map_err(|_| GpuCompileError::InvalidDrawState)?;
            if current != *entry.view {
                return Err(GpuCompileError::InvalidDrawState);
            }
            prepared.textures.push(entry.view.clone());
            prepared.bindings.extend([
                crate::Command::BindImageDescriptor {
                    set: entry.image[0],
                    binding: entry.image[1],
                    array_element: 0,
                    texture: entry.view.image.id,
                },
                crate::Command::BindSamplerDescriptor {
                    set: entry.sampler[0],
                    binding: entry.sampler[1],
                    array_element: 0,
                    sampler: entry.view.sampler.id,
                },
            ]);
        }
        validate_prepared_resources(&prepared)?;
        Ok(prepared)
    }

    fn prepare_bindings(
        self,
        constants: &[crate::maxwell_constant::ConstantBufferInterface],
        state: &crate::maxwell_state::DrawStateSnapshot,
        group: u8,
        address_space: &crate::GpuAddressSpace,
    ) -> Result<PreparedShader, GpuCompileError> {
        let shader = self;
        let bindings = crate::maxwell_constant::bind_uniform_descriptors(
            &shader.module,
            state,
            group,
            constants,
            address_space,
        )
        .map_err(GpuCompileError::Binding)?;
        let layout = crate::shader_resources::descriptor_layout(&shader.module)
            .map_err(GpuCompileError::Interface)?;
        let spirv = crate::spirv::compile(&shader.module).map_err(GpuCompileError::Interface)?;
        Ok(PreparedShader {
            shader,
            bindings,
            layout,
            spirv,
            textures: Vec::new(),
        })
    }
    /// Resolve constant descriptors without discarding source identity/header.
    pub fn lower_constant_interface(
        &self,
        interface: &[crate::maxwell_constant::ConstantBufferInterface],
    ) -> Result<Self, GpuCompileError> {
        let module = crate::maxwell_constant::lower_constant_interface(&self.module, interface)
            .map_err(GpuCompileError::Interface)?;
        let mut resolved = self.clone();
        resolved.module = module;
        Ok(resolved)
    }
    /// Resolve supported fragment exports while retaining original draw provenance.
    pub fn lower_fragment_outputs(&self) -> Result<Self, GpuCompileError> {
        let header = self
            .graphics_header
            .as_ref()
            .ok_or(GpuCompileError::Interface(
                crate::spirv::SpirvCompileError::UnsupportedOperation,
            ))?;
        let module = crate::maxwell_fragment::lower_fragment_outputs(&self.module, header)
            .map_err(GpuCompileError::Interface)?;
        let mut resolved = self.clone();
        resolved.module = module;
        Ok(resolved)
    }

    /// Resolve a verified vertex interface without losing original program/header
    /// provenance. Lift statistics still describe the source Maxwell instructions.
    pub fn lower_vertex_interface(
        &self,
        interface: &[crate::maxwell_vertex::VertexAttributeInterface],
    ) -> Result<Self, GpuCompileError> {
        let module = crate::maxwell_vertex::lower_vertex_interface(&self.module, interface)
            .map_err(GpuCompileError::Interface)?;
        let mut resolved = self.clone();
        resolved.module = module;
        Ok(resolved)
    }

    pub fn specialize_instance_attribute(
        &self,
        state: &crate::maxwell_state::DrawStateSnapshot,
    ) -> Result<Self, GpuCompileError> {
        let mut resolved = self.clone();
        resolved.module = crate::maxwell_vertex::specialize_instance_attribute(&self.module, state)
            .map_err(GpuCompileError::Interface)?;
        Ok(resolved)
    }
}

#[derive(Debug, Clone)]
pub struct PreparedShader {
    pub shader: CompiledShader,
    pub bindings: Vec<crate::Command>,
    pub layout: crate::resource::PipelineLayoutDesc,
    pub spirv: crate::spirv::SpirvModule,
    pub textures: Vec<crate::texture_sampling::PreparedTexture2d>,
}

/// Select real allocations for a validated shader's uniform descriptors.
/// Mapped ranges and descriptor lengths must not stand in for allocation sizes.
pub fn prepared_uniform_buffers(
    prepared: &PreparedShader,
    allocations: &[crate::resource::BufferDesc],
) -> Result<Vec<crate::resource::BufferDesc>, GpuCompileError> {
    use std::collections::BTreeMap;
    validate_prepared_resources(prepared)?;
    let mut available = BTreeMap::new();
    for allocation in allocations {
        if available.insert(allocation.id, allocation).is_some() {
            return Err(GpuCompileError::InvalidBufferAllocation(allocation.id));
        }
    }
    let mut selected = BTreeMap::new();
    for command in &prepared.bindings {
        let crate::Command::BindBufferDescriptor {
            buffer,
            offset,
            range,
            ..
        } = command
        else {
            continue;
        };
        let invalid = || GpuCompileError::InvalidBufferAllocation(*buffer);
        let allocation = *available.get(buffer).ok_or_else(invalid)?;
        if !allocation.usage.supports_uniform()
            || offset.checked_add(*range).ok_or_else(invalid)? > allocation.size
        {
            return Err(invalid());
        }
        selected.insert(*buffer, allocation.clone());
    }
    Ok(selected.into_values().collect())
}

pub struct GraphicsShaderInterfaces<'a> {
    pub attributes: &'a [crate::maxwell_vertex::VertexAttributeInterface],
    pub vertex_constants: &'a [crate::maxwell_constant::ConstantBufferInterface],
    pub fragment_constants: &'a [crate::maxwell_constant::ConstantBufferInterface],
    pub vertex_group: u8,
    pub fragment_group: u8,
}

#[derive(Debug, Clone)]
pub struct PreparedGraphicsShaders {
    pub vertex: PreparedShader,
    pub fragment: PreparedShader,
    pub layout: crate::resource::PipelineLayoutDesc,
    pub bindings: Vec<crate::Command>,
}

pub struct ColorVertexArrayInputs<'a> {
    pub pipeline_id: u64,
    pub image: crate::ResourceId,
    pub vertex: &'a PreparedShader,
    pub fragment: &'a PreparedShader,
    pub attributes: &'a [crate::maxwell_vertex::VertexAttributeInterface],
    pub allocations: &'a [crate::resource::BufferDesc],
}

/// Assemble the supported single-color non-indexed draw without fixture clears,
/// substitute shaders or implicit fixed state. Guest queue/fence ownership and
/// attachment load/writeback remain the caller's responsibilities.
pub fn prepare_color_vertex_array_module(
    event: &crate::maxwell_state::MaxwellEvent,
    inputs: &ColorVertexArrayInputs<'_>,
    address_space: &crate::GpuAddressSpace,
) -> Result<crate::GpuModule, GpuCompileError> {
    let crate::maxwell_state::MaxwellEvent::DrawVertexArray { state, .. } = event else {
        return Err(GpuCompileError::InvalidDrawState);
    };
    let draw = crate::pipeline::prepare_vertex_array_draw(event, inputs.attributes, address_space)
        .map_err(GpuCompileError::Draw)?;
    assemble_color_draw_module(state, inputs, address_space, draw, None)
}

/// Assemble an indexed color draw after canonical index reads have established
/// the vertex fetch ranges. Index FIRST is already included in the binding.
pub fn prepare_color_indexed_module(
    state: &crate::maxwell_state::DrawStateSnapshot,
    inputs: &ColorVertexArrayInputs<'_>,
    address_space: &crate::GpuAddressSpace,
    draw: crate::pipeline::PreparedVertexArrayDraw,
    index_range: crate::address_space::ResolvedGpuRange,
) -> Result<crate::GpuModule, GpuCompileError> {
    use crate::Command;
    let invalid = || GpuCompileError::InvalidDrawState;
    if state.draw_instance_id().is_none()
        || crate::pipeline::draw_topology(state) != Some(draw.topology)
        || matches!(
            draw.topology,
            crate::resource::PrimitiveTopology::Unknown(_)
        )
    {
        return Err(invalid());
    }
    if !matches!(
        draw.draw,
        Command::DrawIndexed {
            indices: 1..,
            instances: 1,
            first_index: 0,
            ..
        }
    ) {
        return Err(invalid());
    }
    let mut index_bindings = draw.bindings.iter().filter_map(|command| match command {
        Command::BindIndexBuffer {
            buffer,
            offset,
            index_type,
        } => Some((*buffer, *offset, *index_type)),
        _ => None,
    });
    let (buffer, offset, kind) = index_bindings.next().ok_or_else(invalid)?;
    let width = match kind {
        crate::resource::IndexType::U8 => 1,
        crate::resource::IndexType::U16 => 2,
        crate::resource::IndexType::U32 => 4,
        crate::resource::IndexType::Unknown(_) => return Err(invalid()),
    };
    if state.index_buffer.index_bytes() != Some(width) {
        return Err(invalid());
    }
    match state
        .raw_registers
        .get(&crate::maxwell_state::method::SET_PRIMITIVE_RESTART)
    {
        Some(0) => {}
        Some(1)
            if draw.topology.supports_standard_restart()
                && state
                    .raw_registers
                    .get(&crate::maxwell_state::method::SET_PRIMITIVE_RESTART_INDEX)
                    .copied()
                    == Some(match width {
                        1 => 0xff,
                        2 => 0xffff,
                        _ => u32::MAX,
                    }) => {}
        _ => return Err(invalid()),
    }
    let Command::DrawIndexed { vertex_offset, .. } = draw.draw else {
        unreachable!()
    };
    if state
        .raw_registers
        .get(&crate::maxwell_state::method::SET_VERTEX_ELEMENT_BASE)
        .copied()
        .map(|value| value as i32)
        != Some(vertex_offset)
    {
        return Err(invalid());
    }
    let Command::DrawIndexed { indices, .. } = draw.draw else {
        unreachable!()
    };
    let (address, length) = state
        .index_buffer
        .index_read_range(indices)
        .ok_or_else(invalid)?;
    if address_space
        .resolve(address, length)
        .map_err(|_| invalid())?
        != index_range
    {
        return Err(invalid());
    }
    if index_bindings.next().is_some()
        || buffer != index_range.resource
        || offset != index_range.offset
        || offset % width != 0
        || !index_range.permissions.read
        || index_range.length != u64::from(indices) * width
    {
        return Err(invalid());
    }
    let mut allocations = inputs
        .allocations
        .iter()
        .filter(|allocation| allocation.id == buffer);
    let allocation = allocations.next().ok_or_else(invalid)?;
    if allocations.next().is_some()
        || !allocation.usage.supports_index()
        || offset
            .checked_add(index_range.length)
            .is_none_or(|end| end > allocation.size)
    {
        return Err(invalid());
    }
    assemble_color_draw_module(state, inputs, address_space, draw, Some(allocation.clone()))
}

fn assemble_color_draw_module(
    state: &crate::maxwell_state::DrawStateSnapshot,
    inputs: &ColorVertexArrayInputs<'_>,
    address_space: &crate::GpuAddressSpace,
    draw: crate::pipeline::PreparedVertexArrayDraw,
    index_allocation: Option<crate::BufferDesc>,
) -> Result<crate::GpuModule, GpuCompileError> {
    use crate::{Command, pipeline, resource::GraphicsPipelineDesc};
    let invalid = || GpuCompileError::InvalidDrawState;
    if inputs.vertex.shader.module.id == inputs.fragment.shader.module.id {
        return Err(invalid());
    }
    if !varying_interface_matches(&inputs.vertex.shader.module, &inputs.fragment.shader.module) {
        return Err(invalid());
    }
    let selection = pipeline::captured_color_targets(state).ok_or_else(invalid)?;
    // Nonidentity output mapping and multiple targets need their own assembly.
    if selection.targets.as_slice() != [0] {
        return Err(invalid());
    }
    for shader in [inputs.vertex, inputs.fragment] {
        if shader
            .shader
            .module
            .operations
            .iter()
            .any(|operation| matches!(operation, crate::ShaderOperation::SmokeVertexTriangle))
        {
            return Err(invalid());
        }
    }
    for operation in &inputs.vertex.shader.module.operations {
        let (location, integer) = match operation {
            crate::ShaderOperation::LoadInput { location, .. }
            | crate::ShaderOperation::LoadInputVec4 { location, .. } => (*location, None),
            crate::ShaderOperation::LoadInputInteger {
                location, signed, ..
            } => (*location, Some(*signed)),
            _ => continue,
        };
        let attribute = draw
            .vertex_input
            .attributes
            .iter()
            .find(|attribute| attribute.location == location)
            .ok_or_else(invalid)?;
        use crate::resource::VertexFormat::*;
        if let Some(signed) = integer {
            if attribute.format.integer_signedness() != Some(signed) {
                return Err(invalid());
            }
            continue;
        }
        // Float loads cannot acquire integer formats through reinterpretation.
        if !matches!(
            attribute.format,
            R8Unorm
                | R8Snorm
                | Rg8Unorm
                | Rg8Snorm
                | Rgba8Unorm
                | Rgba8Snorm
                | R16Float
                | Rg16Float
                | Rgba16Float
                | R32Float
                | Rg32Float
                | Rgb32Float
                | Rgba32Float
        ) {
            return Err(invalid());
        }
    }
    let fixed_function = pipeline::captured_fixed_function(state, &[0]).ok_or_else(invalid)?;
    if fixed_function.depth_stencil.depth_test || fixed_function.depth_stencil.depth_write {
        return Err(invalid());
    }
    let target = state
        .prepare_color_target_layer(0, inputs.image, 1, address_space)
        .map_err(GpuCompileError::ColorTarget)?;
    let dynamic =
        pipeline::prepare_raster_area(state, 0, target.texture.width, target.texture.height)
            .ok_or_else(invalid)?;
    let (layout, descriptor_bindings) = merge_graphics_resources(inputs.vertex, inputs.fragment)?;
    let mut buffers =
        pipeline::vertex_draw_buffers(&draw, inputs.allocations).ok_or_else(invalid)?;
    buffers.extend(graphics_uniform_buffers(
        inputs.vertex,
        inputs.fragment,
        inputs.allocations,
    )?);
    let primitive_restart = index_allocation.is_some()
        && state
            .raw_registers
            .get(&crate::maxwell_state::method::SET_PRIMITIVE_RESTART)
            == Some(&1);
    buffers.extend(index_allocation);
    // Both selectors use the same validated allocation table. A dual-use
    // allocation retains one full-sized descriptor, not one per binding role.
    buffers.sort_by_key(|buffer| buffer.id.0);
    buffers.dedup();
    if buffers.iter().any(|buffer| buffer.id == inputs.image) {
        return Err(invalid());
    }
    let mut sampled_images = std::collections::BTreeMap::new();
    let mut samplers = std::collections::BTreeMap::new();
    for shader in [inputs.vertex, inputs.fragment] {
        for view in &shader.textures {
            let current = crate::texture_sampling::prepare_texture_2d(
                view.backing.descriptor.raw.words,
                view.sampler_raw.words,
                view.image.id,
                view.sampler.id,
                address_space,
            )
            .map_err(|_| invalid())?;
            if current != *view
                || (current.backing.range.resource == target.backing.resource
                    && current.backing.range.offset < target.backing.offset + target.backing.length
                    && target.backing.offset
                        < current.backing.range.offset + current.backing.range.length)
                || view.image.id == inputs.image
                || view.sampler.id == inputs.image
                || buffers
                    .iter()
                    .any(|buffer| buffer.id == view.image.id || buffer.id == view.sampler.id)
                || sampled_images
                    .insert(view.image.id, view.image.clone())
                    .is_some_and(|old| old != view.image)
                || samplers
                    .insert(view.sampler.id, view.sampler)
                    .is_some_and(|old| old != view.sampler)
            {
                return Err(invalid());
            }
        }
    }
    if sampled_images.keys().any(|id| samplers.contains_key(id)) {
        return Err(invalid());
    }
    let mut textures: Vec<_> = sampled_images.into_values().collect();
    textures.push(target.texture.clone());
    // Image bindings may require a layout transition. Keep them outside the
    // render pass; descriptor state is applied when the pipeline is drawn.
    let mut commands = descriptor_bindings;
    commands.extend([
        Command::SetRenderTarget {
            slot: 0,
            texture: inputs.image,
        },
        Command::BeginRenderPass {
            width: target.texture.width,
            height: target.texture.height,
        },
        Command::BindGraphicsPipeline {
            pipeline: inputs.pipeline_id,
        },
    ]);
    commands.extend(dynamic);
    commands.extend(draw.bindings);
    commands.push(draw.draw);
    commands.push(Command::EndRenderPass);
    Ok(crate::GpuModule {
        shaders: vec![
            inputs.vertex.shader.module.clone(),
            inputs.fragment.shader.module.clone(),
        ],
        buffers,
        textures,
        samplers: samplers.into_values().collect(),
        graphics_pipelines: vec![GraphicsPipelineDesc {
            id: inputs.pipeline_id,
            vertex_shader: inputs.vertex.shader.module.id,
            fragment_shader: inputs.fragment.shader.module.id,
            tess_control_shader: None,
            tess_eval_shader: None,
            geometry_shader: None,
            topology: draw.topology,
            primitive_restart,
            vertex_input: draw.vertex_input,
            layout,
            fixed_function,
        }],
        commands,
        ..Default::default()
    })
}

/// Scalar linkage prevents an unwritten component from becoming an undefined
/// fragment input. Interpolation is intentionally linear/default only here.
fn varying_interface_matches(vertex: &ShaderModule, fragment: &ShaderModule) -> bool {
    use crate::{ShaderOperation as S, shader::InterpolationMode};
    let outputs: std::collections::BTreeSet<_> = vertex
        .operations
        .iter()
        .filter_map(|op| {
            if let S::StoreVarying {
                location,
                component,
                ..
            } = op
            {
                Some((*location, *component))
            } else {
                None
            }
        })
        .collect();
    fragment.operations.iter().all(|op| match op {
        S::Interpolate {
            attribute,
            component,
            mode,
            offset,
            ..
        } => {
            (0x80..0x280).contains(attribute)
                && attribute & 3 == 0
                && u32::from(*component) == (attribute & 15) / 4
                && *mode == InterpolationMode::Pass
                && offset.is_none()
                && outputs.contains(&((attribute - 0x80) / 16, *component))
        }
        _ => true,
    })
}

/// Resolve uniforms for both stages only after checking descriptor agreement.
/// Shared allocations are emitted once, with their actual allocation size.
pub fn graphics_uniform_buffers(
    vertex: &PreparedShader,
    fragment: &PreparedShader,
    allocations: &[crate::resource::BufferDesc],
) -> Result<Vec<crate::resource::BufferDesc>, GpuCompileError> {
    merge_graphics_resources(vertex, fragment)?;
    let mut selected = std::collections::BTreeMap::new();
    for prepared in [vertex, fragment] {
        for allocation in prepared_uniform_buffers(prepared, allocations)? {
            selected.insert(allocation.id, allocation);
        }
    }
    Ok(selected.into_values().collect())
}

/// Prepare the supported two-stage graphics path from discovered programs.
/// Additional or duplicate stages must not be silently dropped (in particular,
/// vertex-cull and vertex programs require composition not implemented here).
pub fn prepare_graphics_shaders(
    shaders: &[CompiledShader],
    interfaces: &GraphicsShaderInterfaces<'_>,
    state: &crate::maxwell_state::DrawStateSnapshot,
    address_space: &crate::GpuAddressSpace,
) -> Result<PreparedGraphicsShaders, GpuCompileError> {
    let invalid =
        || GpuCompileError::Interface(crate::spirv::SpirvCompileError::UnsupportedOperation);
    if shaders.len() != 2 {
        return Err(invalid());
    }
    let vertex = shaders
        .iter()
        .find(|shader| shader.module.stage == ShaderStage::Vertex)
        .ok_or_else(invalid)?;
    let fragment = shaders
        .iter()
        .find(|shader| shader.module.stage == ShaderStage::Fragment)
        .ok_or_else(invalid)?;
    let vertex = vertex.prepare_vertex(
        interfaces.attributes,
        interfaces.vertex_constants,
        state,
        interfaces.vertex_group,
        address_space,
    )?;
    let fragment = fragment.prepare_fragment(
        interfaces.fragment_constants,
        state,
        interfaces.fragment_group,
        address_space,
    )?;
    let (layout, bindings) = merge_graphics_resources(&vertex, &fragment)?;
    Ok(PreparedGraphicsShaders {
        vertex,
        fragment,
        layout,
        bindings,
    })
}

/// Combine prepared graphics stages without allowing one stage's descriptor
/// command to silently overwrite a different resource required by another.
pub fn merge_graphics_resources(
    vertex: &PreparedShader,
    fragment: &PreparedShader,
) -> Result<(crate::resource::PipelineLayoutDesc, Vec<crate::Command>), GpuCompileError> {
    if vertex.shader.module.stage != ShaderStage::Vertex
        || fragment.shader.module.stage != ShaderStage::Fragment
    {
        return Err(GpuCompileError::Interface(
            crate::spirv::SpirvCompileError::UnsupportedOperation,
        ));
    }
    validate_prepared_resources(vertex)?;
    validate_prepared_resources(fragment)?;
    let layout =
        crate::shader_resources::pipeline_layout(&[&vertex.shader.module, &fragment.shader.module])
            .map_err(GpuCompileError::Interface)?;
    let bindings = merge_descriptor_commands(&vertex.bindings, &fragment.bindings)?;
    Ok((layout, bindings))
}

fn validate_prepared_resources(prepared: &PreparedShader) -> Result<(), GpuCompileError> {
    let invalid = || {
        GpuCompileError::Binding(crate::maxwell_state::ConstantBufferReadError::InvalidDescriptor)
    };
    let expected = crate::shader_resources::descriptor_layout(&prepared.shader.module)
        .map_err(GpuCompileError::Interface)?;
    if prepared.layout != expected {
        return Err(invalid());
    }
    let commands = merge_descriptor_commands(&prepared.bindings, &[])?;
    if commands.len() != expected.descriptors.len() {
        return Err(invalid());
    }
    for view in &prepared.textures {
        if view.image.id == view.sampler.id
            || view.image.id == view.backing.range.resource
            || view.sampler.id == view.backing.range.resource
            || view.image.layout != crate::TextureLayout::Linear
            || view.image.width != view.backing.descriptor.width.unwrap_or(0)
            || view.image.height != view.backing.descriptor.height.unwrap_or(0)
            || crate::texture_sampling::sampled_view_format(view.backing.descriptor.raw.words, view.sampler_raw.words)
                .ok() != Some(view.image.format)
            || (view.image.depth, view.image.layers, view.image.samples) != (1, 1, 1)
            || view.image.mip_levels == 0
            || view.image.mip_levels != view.backing.descriptor.mip_levels.unwrap_or(0)
            || !commands.iter().any(|command| matches!(command, crate::Command::BindImageDescriptor { texture, .. } if *texture == view.image.id))
            || !commands.iter().any(|command| matches!(command, crate::Command::BindSamplerDescriptor { sampler, .. } if *sampler == view.sampler.id))
        { return Err(invalid()); }
    }
    for command in commands {
        use crate::{Command, DescriptorType};
        if let Command::BindImageDescriptor {
            set,
            binding,
            array_element,
            texture,
        } = command
        {
            if array_element != 0
                || !expected.descriptors.iter().any(|d| {
                    d.set == set
                        && d.binding == binding
                        && d.descriptor_type == DescriptorType::SampledImage
                })
                || !prepared
                    .textures
                    .iter()
                    .any(|view| view.image.id == texture)
            {
                return Err(invalid());
            }
            continue;
        }
        if let Command::BindSamplerDescriptor {
            set,
            binding,
            array_element,
            sampler,
        } = command
        {
            if array_element != 0
                || !expected.descriptors.iter().any(|d| {
                    d.set == set
                        && d.binding == binding
                        && d.descriptor_type == DescriptorType::Sampler
                })
                || !prepared
                    .textures
                    .iter()
                    .any(|view| view.sampler.id == sampler)
            {
                return Err(invalid());
            }
            continue;
        }
        let crate::Command::BindBufferDescriptor {
            set,
            binding,
            array_element,
            offset,
            range,
            ..
        } = command
        else {
            return Err(invalid());
        };
        if array_element != 0
            || range == 0
            || offset.checked_add(range).is_none()
            || !expected.descriptors.iter().any(|descriptor| {
                descriptor.set == set
                    && descriptor.binding == binding
                    && descriptor.descriptor_type == DescriptorType::UniformBuffer
            })
        {
            return Err(invalid());
        }
        // Preparation produces explicit ranges. A zero range cannot be resolved
        // here without live allocation metadata and must not inherit stale state.
        for operation in &prepared.shader.module.operations {
            if let crate::ShaderOperation::LoadUniformWord {
                set: required_set,
                binding: required_binding,
                byte_size,
                ..
            } = operation
            {
                if *required_set == set
                    && *required_binding == binding
                    && range < u64::from(*byte_size)
                {
                    return Err(GpuCompileError::Binding(
                        crate::maxwell_state::ConstantBufferReadError::OutOfBounds,
                    ));
                }
            }
        }
    }
    Ok(())
}

fn merge_descriptor_commands(
    first: &[crate::Command],
    second: &[crate::Command],
) -> Result<Vec<crate::Command>, GpuCompileError> {
    let mut merged = std::collections::BTreeMap::new();
    for command in first.iter().chain(second) {
        let (set, binding, array_element) = match *command {
            crate::Command::BindBufferDescriptor {
                set,
                binding,
                array_element,
                ..
            }
            | crate::Command::BindImageDescriptor {
                set,
                binding,
                array_element,
                ..
            }
            | crate::Command::BindSamplerDescriptor {
                set,
                binding,
                array_element,
                ..
            } => (set, binding, array_element),
            _ => {
                return Err(GpuCompileError::Interface(
                    crate::spirv::SpirvCompileError::UnsupportedOperation,
                ));
            }
        };
        let key = (set, binding, array_element);
        let resource = match *command {
            crate::Command::BindBufferDescriptor {
                buffer,
                offset,
                range,
                ..
            } => (crate::DescriptorType::UniformBuffer, buffer, offset, range),
            crate::Command::BindImageDescriptor { texture, .. } => {
                (crate::DescriptorType::SampledImage, texture, 0, 0)
            }
            crate::Command::BindSamplerDescriptor { sampler, .. } => {
                (crate::DescriptorType::Sampler, sampler, 0, 0)
            }
            _ => unreachable!(),
        };
        if merged
            .insert(key, (resource, command.clone()))
            .is_some_and(|(previous, _)| previous != resource)
        {
            return Err(GpuCompileError::ConflictingDescriptor {
                set,
                binding,
                array_element,
            });
        }
    }
    Ok(merged.into_values().map(|(_, command)| command).collect())
}

#[derive(Debug)]
pub enum GpuCompileError {
    TextureDescriptor(crate::tic_tsc::DescriptorPoolError),
    TextureSampling(crate::texture_sampling::TextureSamplingError),
    InvalidDrawState,
    Draw(crate::pipeline::VertexArrayDrawError),
    ColorTarget(crate::maxwell_state::ColorTargetResolveError),
    InvalidBufferAllocation(crate::ResourceId),
    ConflictingDescriptor {
        set: u32,
        binding: u32,
        array_element: u32,
    },
    Memory(GpuMemoryError),
    Shader(MaxwellShaderError),
    Interface(crate::spirv::SpirvCompileError),
    Binding(crate::maxwell_state::ConstantBufferReadError),
    MissingWindow(u64),
    InvalidWindow {
        address: u64,
        bytes: usize,
    },
    AmbiguousWindow(u64),
    UnexpectedReadSize {
        address: u64,
        expected: usize,
        actual: usize,
    },
}

impl std::fmt::Display for GpuCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TextureDescriptor(error) => write!(f, "texture descriptor: {error:?}"),
            Self::TextureSampling(error) => write!(f, "texture sampling: {error:?}"),
            Self::InvalidDrawState => f.write_str("unsupported or incomplete draw assembly state"),
            Self::Draw(error) => write!(f, "draw assembly: {error}"),
            Self::ColorTarget(error) => write!(f, "draw target: {error}"),
            Self::InvalidBufferAllocation(id) => {
                write!(f, "invalid shader buffer allocation {}", id.0)
            }
            Self::ConflictingDescriptor {
                set,
                binding,
                array_element,
            } => write!(
                f,
                "conflicting shader resources at set {set}, binding {binding}, element {array_element}"
            ),
            Self::Memory(error) => write!(f, "shader memory: {error}"),
            Self::Shader(error) => write!(f, "shader decoding: {error}"),
            Self::Interface(error) => write!(f, "shader interface: {error}"),
            Self::Binding(error) => write!(f, "shader binding: {error}"),
            Self::MissingWindow(address) => write!(f, "missing shader window at 0x{address:x}"),
            Self::InvalidWindow { address, bytes } => {
                write!(f, "invalid shader window at 0x{address:x} ({bytes} bytes)")
            }
            Self::AmbiguousWindow(address) => {
                write!(f, "conflicting shader window sizes at 0x{address:x}")
            }
            Self::UnexpectedReadSize {
                address,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "shader read at 0x{address:x}: expected {expected} bytes, received {actual}"
                )
            }
        }
    }
}

impl std::error::Error for GpuCompileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Draw(error) => Some(error),
            Self::ColorTarget(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::Shader(error) => Some(error),
            Self::Interface(error) => Some(error),
            Self::Binding(error) => Some(error),
            _ => None,
        }
    }
}

impl From<GpuMemoryError> for GpuCompileError {
    fn from(v: GpuMemoryError) -> Self {
        Self::Memory(v)
    }
}
impl From<MaxwellShaderError> for GpuCompileError {
    fn from(v: MaxwellShaderError) -> Self {
        Self::Shader(v)
    }
}

pub fn compile_graphics_shaders(
    memory: &dyn GpuMemory,
    state: &Maxwell3dState,
    windows: &[ShaderWindow],
) -> Result<Vec<CompiledShader>, GpuCompileError> {
    compile_discovered_shaders(memory, discover_graphics_shaders(state)?, windows)
}

pub fn compile_draw_shaders(
    memory: &dyn GpuMemory,
    state: &crate::maxwell_state::DrawStateSnapshot,
    windows: &[ShaderWindow],
) -> Result<Vec<CompiledShader>, GpuCompileError> {
    compile_discovered_shaders(
        memory,
        crate::shader_discovery::discover_draw_shaders(state)?,
        windows,
    )
}

fn compile_discovered_shaders(
    memory: &dyn GpuMemory,
    discovered: Vec<crate::shader_discovery::DiscoveredShader>,
    windows: &[ShaderWindow],
) -> Result<Vec<CompiledShader>, GpuCompileError> {
    let mut indexed = std::collections::BTreeMap::new();
    for window in windows {
        validate_window(window.gpu_address, window.bytes)?;
        if indexed
            .insert(window.gpu_address, window.bytes)
            .is_some_and(|previous| previous != window.bytes)
        {
            return Err(GpuCompileError::AmbiguousWindow(window.gpu_address));
        }
    }
    // Admit the complete window table before reading any shader bytes.
    for entry in &discovered {
        if !indexed.contains_key(&entry.gpu_address) {
            return Err(GpuCompileError::MissingWindow(entry.gpu_address));
        }
    }
    let mut out = Vec::new();
    for entry in discovered {
        let bytes = read_window(memory, entry.gpu_address, indexed[&entry.gpu_address])?;
        let program = parse_graphics(entry.stage, &bytes)?;
        let (module, stats) = lift_semantic(entry.gpu_address, &program);
        out.push(CompiledShader {
            address: entry.gpu_address,
            graphics_source: Some(entry),
            graphics_header: program.header.clone(),
            module,
            stats,
        });
    }
    Ok(out)
}

pub fn compile_compute_shader(
    memory: &dyn GpuMemory,
    address: u64,
    bytes: usize,
) -> Result<CompiledShader, GpuCompileError> {
    validate_window(address, bytes)?;
    let raw = read_window(memory, address, bytes)?;
    let program = parse_compute(&raw)?;
    let (module, stats) = lift_semantic(address, &program);
    debug_assert_eq!(module.stage, ShaderStage::Compute);
    Ok(CompiledShader {
        address,
        graphics_source: None,
        graphics_header: None,
        module,
        stats,
    })
}

fn read_window(
    memory: &dyn GpuMemory,
    address: u64,
    expected: usize,
) -> Result<Vec<u8>, GpuCompileError> {
    let bytes = memory.read(address, expected)?;
    // Providers must supply exactly the admitted window. A word-aligned short
    // read can otherwise parse successfully while silently dropping instructions.
    if bytes.len() != expected {
        return Err(GpuCompileError::UnexpectedReadSize {
            address,
            expected,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

fn validate_window(address: u64, bytes: usize) -> Result<(), GpuCompileError> {
    if bytes == 0 || bytes > MAX_SHADER_WINDOW_BYTES {
        return Err(GpuCompileError::InvalidWindow { address, bytes });
    }
    address
        .checked_add(bytes as u64)
        .ok_or(GpuMemoryError::AddressOverflow)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn varying_link_requires_every_read_component_and_matching_mode() {
        use super::*;
        use crate::{ShaderOperation as S, shader::InterpolationMode};
        let mut vertex = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![S::StoreVarying {
                location: 3,
                component: 2,
                src: 7,
            }],
        };
        let mut fragment = ShaderModule {
            id: 2,
            stage: ShaderStage::Fragment,
            variant_key: vec![],
            operations: vec![S::Interpolate {
                dst: 0,
                attribute: 0xb8,
                component: 2,
                mode: InterpolationMode::Pass,
                offset: None,
            }],
        };
        assert!(varying_interface_matches(&vertex, &fragment));
        vertex.operations.clear();
        assert!(!varying_interface_matches(&vertex, &fragment));
        vertex.operations.push(S::StoreVarying {
            location: 3,
            component: 1,
            src: 7,
        });
        assert!(!varying_interface_matches(&vertex, &fragment));
        vertex.operations.push(S::StoreVarying {
            location: 3,
            component: 2,
            src: 7,
        });
        assert!(varying_interface_matches(&vertex, &fragment));
        if let S::Interpolate { mode, .. } = &mut fragment.operations[0] {
            *mode = InterpolationMode::Constant;
        }
        assert!(!varying_interface_matches(&vertex, &fragment));
    }

    #[test]
    fn prepared_graphics_pair_merges_visibility_and_rejects_resource_conflicts() {
        use crate::{
            Command, ShaderOperation,
            resource::{ResourceId, ShaderStageMask},
        };
        let prepare = |stage| {
            let mut operations = Vec::new();
            for component in 0..4 {
                operations.push(ShaderOperation::LoadUniformWord {
                    dst: component,
                    set: 2,
                    binding: 7,
                    byte_offset: component * 4,
                    byte_size: 16,
                });
                operations.push(if stage == ShaderStage::Vertex {
                    ShaderOperation::StoreBuiltinComponent {
                        builtin: 0,
                        component: component as u8,
                        src: component,
                    }
                } else {
                    ShaderOperation::StoreOutput {
                        location: 0,
                        component: component as u8,
                        src: component,
                    }
                });
            }
            operations.push(ShaderOperation::Return);
            let module = ShaderModule {
                id: 1,
                stage,
                variant_key: vec![],
                operations,
            };
            PreparedShader {
                textures: Vec::new(),
                layout: crate::maxwell_constant::uniform_descriptor_layout(&module).unwrap(),
                spirv: crate::spirv::compile(&module).unwrap(),
                shader: CompiledShader {
                    address: 0x1000,
                    graphics_source: None,
                    graphics_header: None,
                    module,
                    stats: LiftStats::default(),
                },
                bindings: vec![Command::BindBufferDescriptor {
                    set: 2,
                    binding: 7,
                    array_element: 0,
                    buffer: ResourceId(9),
                    offset: 16,
                    range: 16,
                }],
            }
        };
        let vertex = prepare(ShaderStage::Vertex);
        let mut fragment = prepare(ShaderStage::Fragment);
        let mut missing = fragment.clone();
        missing.bindings.clear();
        assert!(merge_graphics_resources(&vertex, &missing).is_err());
        let mut stale_layout = fragment.clone();
        stale_layout.layout.descriptors.clear();
        assert!(merge_graphics_resources(&vertex, &stale_layout).is_err());
        for (element, offset, range) in [(1, 16, 16), (0, 16, 0), (0, 16, 8), (0, u64::MAX, 16)] {
            let mut invalid = fragment.clone();
            invalid.bindings[0] = Command::BindBufferDescriptor {
                set: 2,
                binding: 7,
                array_element: element,
                buffer: ResourceId(9),
                offset,
                range,
            };
            assert!(merge_graphics_resources(&vertex, &invalid).is_err());
        }
        let (layout, bindings) = merge_graphics_resources(&vertex, &fragment).unwrap();
        assert_eq!(bindings.len(), 1);
        let allocation = crate::resource::BufferDesc {
            id: ResourceId(9),
            size: 32,
            usage: crate::resource::BufferUsage::Uniform,
        };
        let allocations =
            graphics_uniform_buffers(&vertex, &fragment, std::slice::from_ref(&allocation))
                .unwrap();
        assert_eq!(allocations.as_slice(), std::slice::from_ref(&allocation));
        assert!(graphics_uniform_buffers(&vertex, &fragment, &[]).is_err());
        assert!(
            graphics_uniform_buffers(&vertex, &missing, std::slice::from_ref(&allocation)).is_err()
        );
        for invalid in [
            crate::resource::BufferDesc {
                size: 31,
                ..allocation.clone()
            },
            crate::resource::BufferDesc {
                usage: crate::resource::BufferUsage::Vertex,
                ..allocation.clone()
            },
        ] {
            assert!(matches!(
                graphics_uniform_buffers(&vertex, &fragment, &[invalid]),
                Err(GpuCompileError::InvalidBufferAllocation(ResourceId(9)))
            ));
        }
        assert!(
            graphics_uniform_buffers(
                &vertex,
                &fragment,
                &[allocation.clone(), allocation.clone()]
            )
            .is_err()
        );
        assert_eq!(layout.descriptors.len(), 1);
        assert_eq!(
            layout.descriptors[0].stages.0,
            ShaderStageMask::VERTEX | ShaderStageMask::FRAGMENT
        );
        assert!(merge_graphics_resources(&fragment, &vertex).is_err());
        if let Command::BindBufferDescriptor { buffer, .. } = &mut fragment.bindings[0] {
            *buffer = ResourceId(10);
        }
        assert!(matches!(
            merge_graphics_resources(&vertex, &fragment),
            Err(GpuCompileError::ConflictingDescriptor { .. })
        ));
        assert!(matches!(
            graphics_uniform_buffers(&vertex, &fragment, &[allocation]),
            Err(GpuCompileError::ConflictingDescriptor { .. })
        ));
    }

    #[test]
    fn shared_descriptor_resources_must_agree() {
        use crate::{Command, resource::ResourceId};
        let command = |buffer, offset, range| Command::BindBufferDescriptor {
            set: 2,
            binding: 7,
            array_element: 0,
            buffer: ResourceId(buffer),
            offset,
            range,
        };
        let original = command(9, 16, 32);
        let merged = super::merge_descriptor_commands(
            std::slice::from_ref(&original),
            std::slice::from_ref(&original),
        )
        .unwrap();
        assert_eq!(merged.len(), 1);
        assert!(matches!(
            merged[0],
            Command::BindBufferDescriptor {
                set: 2,
                binding: 7,
                array_element: 0,
                buffer: ResourceId(9),
                offset: 16,
                range: 32
            }
        ));
        for conflicting in [command(10, 16, 32), command(9, 32, 32), command(9, 16, 16)] {
            assert!(matches!(
                super::merge_descriptor_commands(std::slice::from_ref(&original), &[conflicting]),
                Err(super::GpuCompileError::ConflictingDescriptor {
                    set: 2,
                    binding: 7,
                    array_element: 0
                })
            ));
        }
        assert!(
            super::merge_descriptor_commands(
                &[Command::BindSampler {
                    slot: 0,
                    sampler: ResourceId(1)
                }],
                &[]
            )
            .is_err()
        );
    }
    use super::*;
    #[test]
    fn prepared_fragment_textures_merge_and_reject_stale_or_missing_ownership() {
        use crate::{GpuMapPermissions, GpuMapping, ResourceId, ShaderOperation as S};
        let mut bytes = vec![0; crate::maxwell_shader::GRAPHICS_HEADER_SIZE];
        bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        bytes.extend(
            [0u64, 0xe30000000007000f]
                .into_iter()
                .flat_map(u64::to_le_bytes),
        );
        let source = CompiledShader {
            address: 0x2000,
            graphics_source: None,
            graphics_header: parse_graphics(ShaderStage::Fragment, &bytes)
                .unwrap()
                .header,
            stats: LiftStats::default(),
            module: ShaderModule {
                id: 2,
                stage: ShaderStage::Fragment,
                variant_key: vec![9],
                operations: vec![
                    S::ConstantF32 { dst: 8, value: 0.5 },
                    S::SampleTexture2dHandle {
                        dst: vec![0, 1, 2, 3],
                        handle_selector: 7,
                        coords: [8, 8],
                        components: vec![0, 1, 2, 3],
                        level_zero: true,
                    },
                    S::Return,
                ],
            },
        };
        let mapping = |resource| GpuMapping {
            guest_base: 0x1000,
            size: 512,
            resource: ResourceId(resource),
            resource_offset: 64,
            permissions: GpuMapPermissions::READ_ONLY,
        };
        let mut space = crate::GpuAddressSpace::new();
        space.map(mapping(3)).unwrap();
        let tic = [
            0x08 | 2 << 7 | 2 << 10 | 2 << 13 | 2 << 16 | 2 << 19 | 3 << 22 | 4 << 25 | 5 << 28,
            0x1000,
            2 << 21,
            1,
            1 | 1 << 23 | 7 << 29,
            1 | 1 << 31,
            0,
            0,
        ];
        let tsc = [2 | 2 << 3 | 2 << 6, 1 | 1 << 4 | 1 << 6, 0, 0, 0, 0, 0, 0];
        let view = crate::texture_sampling::prepare_texture_2d(
            tic,
            tsc,
            ResourceId(4),
            ResourceId(5),
            &space,
        )
        .unwrap();
        let interface = [crate::texture_sampling::TextureShaderInterface {
            selector: 7,
            image: [1, 4],
            sampler: [1, 5],
            view: &view,
        }];
        let state = crate::maxwell_state::DrawStateSnapshot::default();
        let fragment = source
            .prepare_fragment_with_textures(&[], &state, 4, &space, &interface)
            .unwrap();
        assert_eq!(fragment.shader.address, source.address);
        assert_eq!(fragment.shader.module.variant_key, vec![9]);
        assert_eq!(fragment.textures, vec![view.clone()]);
        let module = crate::GpuModule::triangle_vulkan_smoke_test(16, 16)
            .shaders
            .into_iter()
            .find(|s| s.stage == ShaderStage::Vertex)
            .unwrap();
        let vertex = PreparedShader {
            layout: crate::shader_resources::descriptor_layout(&module).unwrap(),
            spirv: crate::spirv::compile(&module).unwrap(),
            textures: Vec::new(),
            bindings: Vec::new(),
            shader: CompiledShader {
                address: 0,
                graphics_source: None,
                graphics_header: None,
                stats: LiftStats::default(),
                module,
            },
        };
        let (layout, commands) = merge_graphics_resources(&vertex, &fragment).unwrap();
        assert_eq!(layout.descriptors.len(), 2);
        assert_eq!(commands.len(), 2);
        assert!(prepared_uniform_buffers(&fragment, &[]).unwrap().is_empty());
        let mut invalid = fragment.clone();
        invalid.textures.clear();
        assert!(merge_graphics_resources(&vertex, &invalid).is_err());
        invalid = fragment.clone();
        invalid.bindings.pop();
        assert!(merge_graphics_resources(&vertex, &invalid).is_err());
        invalid = fragment;
        invalid.textures[0].image.width = 9;
        assert!(merge_graphics_resources(&vertex, &invalid).is_err());
        let mut remapped = crate::GpuAddressSpace::new();
        remapped.map(mapping(6)).unwrap();
        assert!(
            source
                .prepare_fragment_with_textures(&[], &state, 4, &remapped, &interface)
                .is_err()
        );
    }

    #[test]
    fn fragment_preparation_rejects_missing_metadata_before_submission() {
        let mut bytes = vec![0; crate::maxwell_shader::GRAPHICS_HEADER_SIZE];
        bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        bytes.extend(
            [0u64, 0xe30000000007000f]
                .into_iter()
                .flat_map(u64::to_le_bytes),
        );
        let program = parse_graphics(ShaderStage::Fragment, &bytes).unwrap();
        let mut shader = CompiledShader {
            address: 0x100080,
            graphics_source: None,
            graphics_header: program.header,
            stats: LiftStats::default(),
            module: ShaderModule {
                id: 2,
                stage: ShaderStage::Fragment,
                variant_key: vec![],
                operations: (0..4)
                    .map(|dst| crate::ShaderOperation::LoadConstantBuffer {
                        dst,
                        slot: 3,
                        offset: (dst * 4) as i32,
                        address: None,
                    })
                    .chain(std::iter::once(crate::ShaderOperation::Return))
                    .collect(),
            },
        };
        let constants = [crate::maxwell_constant::ConstantBufferInterface {
            slot: 3,
            set: 0,
            binding: 8,
            byte_size: 16,
        }];
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        let mut space = crate::GpuAddressSpace::new();
        assert!(matches!(
            shader.prepare_fragment(&[], &state, 4, &space),
            Err(GpuCompileError::Interface(_))
        ));
        assert!(matches!(
            shader.prepare_fragment(&constants, &state, 4, &space),
            Err(GpuCompileError::Binding(_))
        ));
        let header = shader.graphics_header.take();
        assert!(matches!(
            shader.prepare_fragment(&constants, &state, 4, &space),
            Err(GpuCompileError::Interface(_))
        ));
        shader.graphics_header = header;
        shader.module.stage = ShaderStage::Vertex;
        assert!(matches!(
            shader.prepare_fragment(&constants, &state, 4, &space),
            Err(GpuCompileError::Interface(_))
        ));
        shader.module.stage = ShaderStage::Fragment;
        state.constant_buffers.insert(
            (4, 3),
            crate::maxwell_state::ConstantBufferBinding {
                size_raw: 16,
                address: crate::RawGpuAddress { a: 0, b: 0x1000 },
                bind_raw: 49,
            },
        );
        assert!(matches!(
            shader.prepare_fragment(&constants, &state, 4, &space),
            Err(GpuCompileError::Binding(_))
        ));
        space
            .map(crate::GpuMapping {
                guest_base: 0x1000,
                size: 16,
                resource: crate::ResourceId(4),
                resource_offset: 0,
                permissions: crate::GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        let prepared = shader
            .prepare_fragment(&constants, &state, 4, &space)
            .unwrap();
        assert_eq!(prepared.bindings.len(), 1);
        assert_eq!(prepared.shader.address, shader.address);
        let vertex = CompiledShader {
            address: 0x2000,
            graphics_source: None,
            graphics_header: None,
            stats: LiftStats::default(),
            module: ShaderModule {
                id: 3,
                stage: ShaderStage::Vertex,
                variant_key: vec![],
                operations: (0..4)
                    .flat_map(|component| {
                        [
                            crate::ShaderOperation::ConstantBits32 {
                                dst: component,
                                value: 0x3f800000,
                            },
                            crate::ShaderOperation::StoreBuiltinComponent {
                                builtin: 0,
                                component: component as u8,
                                src: component,
                            },
                        ]
                    })
                    .chain([crate::ShaderOperation::Return])
                    .collect(),
            },
        };
        let interfaces = GraphicsShaderInterfaces {
            attributes: &[],
            vertex_constants: &[],
            fragment_constants: &constants,
            vertex_group: 0,
            fragment_group: 4,
        };
        let pair = [shader.clone(), vertex.clone()];
        let graphics = prepare_graphics_shaders(&pair, &interfaces, &state, &space).unwrap();
        assert_eq!(graphics.bindings.len(), 1);
        assert_eq!(graphics.vertex.shader.address, 0x2000);
        assert_eq!(graphics.fragment.shader.address, shader.address);
        for invalid in [
            vec![],
            vec![vertex.clone()],
            vec![vertex.clone(), vertex.clone()],
            vec![vertex.clone(), shader.clone(), vertex],
        ] {
            assert!(prepare_graphics_shaders(&invalid, &interfaces, &state, &space).is_err());
        }
        assert!(matches!(
            shader.module.operations[0],
            crate::ShaderOperation::LoadConstantBuffer { .. }
        ));
    }
    #[test]
    fn constant_lowering_retains_program_identity_and_reports_missing_bindings() {
        let source = CompiledShader {
            address: 0x100080,
            graphics_source: None,
            graphics_header: None,
            stats: LiftStats {
                total: 1,
                semantic: 1,
                classified: 1,
                opaque: 0,
            },
            module: ShaderModule {
                id: 7,
                stage: ShaderStage::Vertex,
                variant_key: vec![8],
                operations: vec![crate::ShaderOperation::LoadConstantBuffer {
                    dst: 3,
                    slot: 2,
                    offset: 4,
                    address: None,
                }],
            },
        };
        assert!(matches!(
            source.lower_constant_interface(&[]),
            Err(GpuCompileError::Interface(_))
        ));
        let resolved = source
            .lower_constant_interface(&[crate::maxwell_constant::ConstantBufferInterface {
                slot: 2,
                set: 1,
                binding: 5,
                byte_size: 16,
            }])
            .unwrap();
        assert_eq!(resolved.address, source.address);
        assert_eq!(resolved.module.id, source.module.id);
        assert_eq!(resolved.module.variant_key, source.module.variant_key);
        assert_eq!(resolved.stats.total, source.stats.total);
        assert!(matches!(
            source.module.operations[0],
            crate::ShaderOperation::LoadConstantBuffer { .. }
        ));
        assert!(matches!(
            resolved.module.operations[0],
            crate::ShaderOperation::LoadUniformWord {
                set: 1,
                binding: 5,
                ..
            }
        ));
        let mut complete = source.clone();
        for component in 0..4 {
            complete
                .module
                .operations
                .push(crate::ShaderOperation::StoreBuiltinComponent {
                    builtin: 0,
                    component,
                    src: 3,
                });
        }
        complete
            .module
            .operations
            .push(crate::ShaderOperation::Return);
        let constants = [crate::maxwell_constant::ConstantBufferInterface {
            slot: 2,
            set: 1,
            binding: 5,
            byte_size: 16,
        }];
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        let mut space = crate::GpuAddressSpace::new();
        assert!(matches!(
            complete.prepare_vertex(&[], &constants, &state, 0, &space),
            Err(GpuCompileError::Binding(_))
        ));
        state.constant_buffers.insert(
            (0, 2),
            crate::maxwell_state::ConstantBufferBinding {
                size_raw: 16,
                address: crate::RawGpuAddress { a: 0, b: 0x1000 },
                bind_raw: 33,
            },
        );
        space
            .map(crate::GpuMapping {
                guest_base: 0x1000,
                size: 16,
                resource: crate::ResourceId(9),
                resource_offset: 256,
                permissions: crate::GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        let prepared = complete
            .prepare_vertex(&[], &constants, &state, 0, &space)
            .unwrap();
        assert_eq!(prepared.shader.address, source.address);
        assert_eq!(prepared.layout.descriptors.len(), 1);
        assert!(matches!(
            prepared.bindings.as_slice(),
            [crate::Command::BindBufferDescriptor {
                set: 1,
                binding: 5,
                buffer: crate::ResourceId(9),
                offset: 256,
                range: 16,
                ..
            }]
        ));
        assert_eq!(prepared.spirv.words[0], crate::spirv::SpirvModule::MAGIC);
    }
    struct NoReads;
    impl GpuMemory for NoReads {
        fn read(&self, _: u64, _: usize) -> Result<Vec<u8>, GpuMemoryError> {
            panic!("invalid metadata must reject before reading shader bytes")
        }
    }

    #[test]
    fn compile_errors_preserve_diagnostic_causes() {
        use std::error::Error;
        let memory = GpuCompileError::from(GpuMemoryError::AddressOverflow);
        assert!(
            memory
                .source()
                .unwrap()
                .downcast_ref::<GpuMemoryError>()
                .is_some()
        );
        assert_eq!(
            memory.to_string(),
            "shader memory: GPU address arithmetic overflow"
        );
        let shader = GpuCompileError::from(MaxwellShaderError::MisalignedCode);
        assert!(
            shader
                .source()
                .unwrap()
                .downcast_ref::<MaxwellShaderError>()
                .is_some()
        );
        let length = GpuCompileError::UnexpectedReadSize {
            address: 0x1000,
            expected: 88,
            actual: 80,
        };
        assert!(length.source().is_none());
        assert_eq!(
            length.to_string(),
            "shader read at 0x1000: expected 88 bytes, received 80"
        );
    }

    #[test]
    fn mismatched_provider_lengths_are_not_parsed_as_complete_programs() {
        struct WrongLength(usize);
        impl GpuMemory for WrongLength {
            fn read(&self, _: u64, _: usize) -> Result<Vec<u8>, GpuMemoryError> {
                Ok(vec![0; self.0])
            }
        }
        let mut state = Maxwell3dState::default();
        state.pipeline_shaders[0].select = 1;
        state.pipeline_shaders[0].program = 0x1000;
        for actual in [0, 80, 96] {
            assert!(matches!(
                compile_graphics_shaders(&WrongLength(actual), &state, &[ShaderWindow { gpu_address: 0x1000, bytes: 88 }]),
                Err(GpuCompileError::UnexpectedReadSize { address: 0x1000, expected: 88, actual: length }) if length == actual
            ));
        }
        for actual in [0, 8, 24] {
            assert!(matches!(
                compile_compute_shader(&WrongLength(actual), 0x2000, 16),
                Err(GpuCompileError::UnexpectedReadSize { address: 0x2000, expected: 16, actual: length }) if length == actual
            ));
        }
    }

    #[test]
    fn compilation_retains_distinct_graphics_slots_and_raw_headers() {
        let header = vec![0x5a; crate::maxwell_shader::GRAPHICS_HEADER_SIZE];
        let memory = crate::shader_memory::LinearGpuMemory::new(0x1000, header.clone());
        let mut state = Maxwell3dState::default();
        state.program_region.b = 0x1000;
        for (slot, select, binding) in [(0, 1, 2), (1, 0x11, 3)] {
            state.pipeline_shaders[slot] = crate::maxwell_state::PipelineShaderState {
                select,
                program: 0,
                register_count: 12 + slot as u32,
                binding,
            };
        }
        let shaders = compile_graphics_shaders(
            &memory,
            &state,
            &[ShaderWindow {
                gpu_address: 0x1000,
                bytes: header.len(),
            }],
        )
        .unwrap();
        assert_eq!(shaders.len(), 2);
        for (slot, shader) in shaders.iter().enumerate() {
            assert_eq!(shader.address, 0x1000);
            assert_eq!(shader.module.stage, ShaderStage::Vertex);
            let source = shader.graphics_source.unwrap();
            assert_eq!(source.slot, slot as u8);
            assert_eq!(source.binding, 2 + slot as u32);
            assert_eq!(source.register_count, 12 + slot as u32);
            assert_eq!(source.enabled_raw, if slot == 0 { 1 } else { 0x11 });
            assert_eq!(
                shader.graphics_header.as_ref().unwrap().raw.as_slice(),
                header
            );
        }
        let compute_memory = crate::shader_memory::LinearGpuMemory::new(0x2000, vec![0; 8]);
        let compute = compile_compute_shader(&compute_memory, 0x2000, 8).unwrap();
        assert_eq!(compute.module.stage, ShaderStage::Compute);
        assert!(compute.graphics_source.is_none());
        assert!(compute.graphics_header.is_none());
    }

    #[test]
    fn validates_all_windows_before_shader_reads_and_rejects_conflicting_sizes() {
        let mut state = Maxwell3dState::default();
        state.pipeline_shaders[0].select = 1;
        state.pipeline_shaders[0].program = 0x1000;
        state.pipeline_shaders[5].select = 0x51;
        state.pipeline_shaders[5].program = 0x2000;
        let first = ShaderWindow {
            gpu_address: 0x1000,
            bytes: 80,
        };
        assert!(matches!(
            compile_graphics_shaders(&NoReads, &state, &[first]),
            Err(GpuCompileError::MissingWindow(0x2000))
        ));
        assert!(matches!(
            compile_graphics_shaders(
                &NoReads,
                &state,
                &[first, ShaderWindow { bytes: 88, ..first }]
            ),
            Err(GpuCompileError::AmbiguousWindow(0x1000))
        ));
        for bytes in [0, MAX_SHADER_WINDOW_BYTES + 1] {
            assert!(matches!(
                compile_compute_shader(&NoReads, 0x1000, bytes),
                Err(GpuCompileError::InvalidWindow { .. })
            ));
        }
        assert!(matches!(
            compile_compute_shader(&NoReads, u64::MAX - 3, 8),
            Err(GpuCompileError::Memory(GpuMemoryError::AddressOverflow))
        ));
    }
}
