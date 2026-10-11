// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Pre-driver validation of Vulkan plan resource references.
//!
//! Vulkan validation layers are useful, but Exosphere should reject guest mistakes before any raw
//! driver handle is touched. This module checks stable resource IDs, ranges and required payloads.

use std::collections::{BTreeMap, BTreeSet};

use exosphere_gpu_ir::{
    BlockLinearLayout, BufferDesc, GpuResourceData, ResourceDataError, ResourceId, TextureDesc,
    TextureLayout, linear_footprint, texture_layout::mip_extent,
};

use crate::{
    VulkanError,
    plan::{VulkanExecutionPlan, VulkanPlanCommand},
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanResourceUsage {
    pub read_buffers: BTreeSet<ResourceId>,
    pub written_buffers: BTreeSet<ResourceId>,
    pub read_textures: BTreeSet<ResourceId>,
    pub written_textures: BTreeSet<ResourceId>,
    pub samplers: BTreeSet<ResourceId>,
}

pub fn analyze(plan: &VulkanExecutionPlan) -> Result<PlanResourceUsage, VulkanError> {
    for pipeline in &plan.graphics_pipelines {
        if pipeline.primitive_restart && !pipeline.topology.supports_standard_restart() {
            return Err(VulkanError::UnsupportedFeature(
                "primitive restart topology",
            ));
        }
    }
    let buffers = plan
        .buffers
        .iter()
        .map(|buffer| (buffer.id, buffer))
        .collect::<BTreeMap<_, _>>();
    let textures = plan
        .textures
        .iter()
        .map(|texture| (texture.id, texture))
        .collect::<BTreeMap<_, _>>();
    let samplers = plan
        .samplers
        .iter()
        .map(|sampler| sampler.id)
        .collect::<BTreeSet<_>>();
    let mut usage = PlanResourceUsage::default();
    let mut index_binding = None;
    let mut vertex_bindings = BTreeMap::new();
    let mut graphics_pipeline = None;

    for command in &plan.commands {
        match command {
            VulkanPlanCommand::BindGraphicsPipeline { pipeline } => {
                graphics_pipeline = Some(
                    plan.graphics_pipelines
                        .iter()
                        .find(|desc| desc.id == *pipeline)
                        .ok_or(VulkanError::MissingResource(*pipeline))?,
                );
            }
            VulkanPlanCommand::BindVertexBuffer {
                slot,
                buffer,
                offset,
            } => {
                let desc = require_buffer(&buffers, *buffer)?;
                if !desc.usage.supports_vertex() {
                    return Err(VulkanError::UnsupportedFeature("vertex buffer usage"));
                }
                require_range(desc, *offset, 0)?;
                vertex_bindings.insert(*slot, (desc, *offset));
                usage.read_buffers.insert(*buffer);
            }
            VulkanPlanCommand::Draw {
                vertices,
                instances,
                first_vertex,
            } => {
                if *vertices == 0 || *instances == 0 {
                    continue;
                }
                let pipeline = graphics_pipeline.ok_or(VulkanError::UnsupportedFeature(
                    "draw without graphics pipeline",
                ))?;
                for attribute in &pipeline.vertex_input.attributes {
                    let binding = pipeline
                        .vertex_input
                        .bindings
                        .iter()
                        .find(|binding| binding.binding == attribute.binding)
                        .ok_or(VulkanError::UnsupportedFeature(
                            "vertex attribute without binding",
                        ))?;
                    let (desc, offset) = vertex_bindings.get(&attribute.binding).ok_or(
                        VulkanError::UnsupportedFeature("draw without vertex buffer"),
                    )?;
                    let last = match binding.input_rate {
                        exosphere_gpu_ir::VertexInputRate::Vertex => {
                            u64::from(*first_vertex) + u64::from(*vertices) - 1
                        }
                        exosphere_gpu_ir::VertexInputRate::Instance => u64::from(*instances) - 1,
                    };
                    // Stride zero repeatedly fetches the same attribute. Check
                    // the final fetch, including both binding and attribute offsets.
                    let address = last
                        .checked_mul(u64::from(binding.stride))
                        .and_then(|bytes| offset.checked_add(bytes))
                        .and_then(|bytes| bytes.checked_add(u64::from(attribute.offset)))
                        .ok_or(VulkanError::ResourceRangeOverflow(desc.id.0))?;
                    require_range(desc, address, vertex_format_bytes(attribute.format)?)?;
                }
            }
            VulkanPlanCommand::BindIndexBuffer {
                buffer,
                offset,
                index_type,
            } => {
                let desc = require_buffer(&buffers, *buffer)?;
                let width = match index_type {
                    exosphere_gpu_ir::IndexType::U8 => 1u64,
                    exosphere_gpu_ir::IndexType::U16 => 2,
                    exosphere_gpu_ir::IndexType::U32 => 4,
                    _ => return Err(VulkanError::UnsupportedFeature("index type")),
                };
                if !desc.usage.supports_index() || offset % width != 0 {
                    return Err(VulkanError::UnsupportedFeature(
                        "index buffer usage or alignment",
                    ));
                }
                require_range(desc, *offset, width)?;
                index_binding = Some((desc, *offset, width));
                usage.read_buffers.insert(*buffer);
            }
            VulkanPlanCommand::DrawIndexed {
                indices,
                instances,
                first_index,
                ..
            } => {
                let (desc, offset, width) = index_binding.ok_or(
                    VulkanError::UnsupportedFeature("indexed draw without index buffer"),
                )?;
                if *indices != 0 && *instances != 0 {
                    let start = offset
                        .checked_add(u64::from(*first_index) * width)
                        .ok_or(VulkanError::UnsupportedFeature("index address overflow"))?;
                    require_range(desc, start, u64::from(*indices) * width)?;
                }
            }
            VulkanPlanCommand::DrawIndirect {
                buffer,
                offset,
                draw_count,
                stride,
                indexed,
            } => {
                let desc = require_buffer(&buffers, *buffer)?;
                let command_size = if *indexed { 20_u64 } else { 16_u64 };
                if desc.usage != exosphere_gpu_ir::BufferUsage::Indirect || offset % 4 != 0 {
                    return Err(VulkanError::UnsupportedFeature(
                        "indirect buffer usage or alignment",
                    ));
                }
                if *indexed && index_binding.is_none() {
                    return Err(VulkanError::UnsupportedFeature(
                        "indexed indirect draw without index buffer",
                    ));
                }
                let stride = if *stride == 0 {
                    command_size
                } else {
                    u64::from(*stride)
                };
                let count = u64::from(*draw_count);
                if count > 1 && (stride < command_size || stride % 4 != 0) {
                    return Err(VulkanError::UnsupportedFeature("indirect draw stride"));
                }
                let bytes = if count == 0 {
                    0
                } else {
                    stride
                        .saturating_mul(count - 1)
                        .saturating_add(command_size)
                };
                require_range(desc, *offset, bytes)?;
                usage.read_buffers.insert(*buffer);
            }
            VulkanPlanCommand::DispatchIndirect { buffer, offset } => {
                let desc = require_buffer(&buffers, *buffer)?;
                require_range(desc, *offset, 12)?;
                usage.read_buffers.insert(*buffer);
            }
            VulkanPlanCommand::BindBufferDescriptor {
                buffer,
                offset,
                range,
                ..
            } => {
                let desc = require_buffer(&buffers, *buffer)?;
                let length = if *range == 0 {
                    desc.size.saturating_sub(*offset)
                } else {
                    *range
                };
                require_range(desc, *offset, length)?;
                usage.read_buffers.insert(*buffer);
            }
            VulkanPlanCommand::SetRenderTarget { texture, .. }
            | VulkanPlanCommand::SetDepthStencilTarget { texture } => {
                require_texture(&textures, *texture)?;
                usage.written_textures.insert(*texture);
            }
            VulkanPlanCommand::BindTexture { texture, .. }
            | VulkanPlanCommand::BindImageDescriptor { texture, .. } => {
                require_texture(&textures, *texture)?;
                usage.read_textures.insert(*texture);
            }
            VulkanPlanCommand::BindSampler { sampler, .. }
            | VulkanPlanCommand::BindSamplerDescriptor { sampler, .. } => {
                if !samplers.contains(sampler) {
                    return Err(VulkanError::MissingResource(sampler.0));
                }
                usage.samplers.insert(*sampler);
            }
            VulkanPlanCommand::CopyBuffer {
                source,
                destination,
                source_offset,
                destination_offset,
                bytes,
            } => {
                let source_desc = require_buffer(&buffers, *source)?;
                let destination_desc = require_buffer(&buffers, *destination)?;
                require_range(source_desc, *source_offset, *bytes)?;
                require_range(destination_desc, *destination_offset, *bytes)?;
                // Native buffer copies cannot implement overlapping regions. Exact
                // self-copies are treated as no-ops by every execution path.
                if source == destination
                    && source_offset != destination_offset
                    && *source_offset < destination_offset + bytes
                    && *destination_offset < source_offset + bytes
                {
                    return Err(VulkanError::UnsupportedFeature("overlapping buffer copy"));
                }
                usage.read_buffers.insert(*source);
                usage.written_buffers.insert(*destination);
            }
            VulkanPlanCommand::CopyImage {
                source,
                destination,
            }
            | VulkanPlanCommand::Blit {
                source,
                destination,
                ..
            }
            | VulkanPlanCommand::Resolve {
                source,
                destination,
            } => {
                require_texture(&textures, *source)?;
                require_texture(&textures, *destination)?;
                usage.read_textures.insert(*source);
                usage.written_textures.insert(*destination);
            }
            VulkanPlanCommand::FillBuffer {
                destination, bytes, ..
            } => {
                let desc = require_buffer(&buffers, *destination)?;
                require_range(desc, 0, *bytes)?;
                usage.written_buffers.insert(*destination);
            }
            VulkanPlanCommand::ClearColor { target, .. }
            | VulkanPlanCommand::ClearDepthStencil { target, .. } => {
                require_texture(&textures, *target)?;
                usage.written_textures.insert(*target);
            }
            VulkanPlanCommand::CopyQueryResults {
                destination,
                offset,
                count,
                ..
            } => {
                let desc = require_buffer(&buffers, *destination)?;
                let bytes = u64::from(*count).saturating_mul(8);
                require_range(desc, *offset, bytes)?;
                usage.written_buffers.insert(*destination);
            }
            VulkanPlanCommand::BeginConditional { buffer, offset, .. } => {
                let desc = require_buffer(&buffers, *buffer)?;
                require_range(desc, *offset, 4)?;
                usage.read_buffers.insert(*buffer);
            }
            VulkanPlanCommand::BeginTransformFeedback { buffers: bindings } => {
                for (buffer, offset, size) in bindings {
                    let desc = require_buffer(&buffers, *buffer)?;
                    require_range(desc, *offset, *size)?;
                    usage.written_buffers.insert(*buffer);
                }
            }
            _ => {}
        }
    }
    Ok(usage)
}

/// Verifies that resources read by a plan have at least the descriptor-sized payload available.
/// Render targets and other write-only allocations do not require initial bytes.
pub fn validate_payloads(
    plan: &VulkanExecutionPlan,
    provider: &dyn GpuResourceData,
) -> Result<PlanResourceUsage, VulkanError> {
    let usage = analyze(plan)?;
    let buffers = plan
        .buffers
        .iter()
        .map(|buffer| (buffer.id, buffer))
        .collect::<BTreeMap<_, _>>();
    let textures = plan
        .textures
        .iter()
        .map(|texture| (texture.id, texture))
        .collect::<BTreeMap<_, _>>();

    for resource in &usage.read_buffers {
        let desc = buffers
            .get(resource)
            .expect("analyze already validated buffer existence");
        let available = provider.len(*resource).map_err(map_resource_error)?;
        if available < desc.size {
            return Err(VulkanError::ResourcePayloadTooSmall {
                resource: resource.0,
                required: desc.size,
                available,
            });
        }
    }
    for resource in &usage.read_textures {
        let texture = textures
            .get(resource)
            .expect("analyze already validated texture existence");
        let required = minimum_texture_bytes(texture);
        let available = provider.len(*resource).map_err(map_resource_error)?;
        if available < required {
            return Err(VulkanError::ResourcePayloadTooSmall {
                resource: resource.0,
                required,
                available,
            });
        }
    }
    validate_indexed_vertex_payloads(plan, provider, &buffers, &usage.written_buffers)?;
    validate_indirect_draw_payloads(plan, provider, &usage.written_buffers)?;
    Ok(usage)
}

fn validate_indirect_draw_payloads(
    plan: &VulkanExecutionPlan,
    provider: &dyn GpuResourceData,
    written: &BTreeSet<ResourceId>,
) -> Result<(), VulkanError> {
    if !plan
        .commands
        .iter()
        .any(|command| matches!(command, VulkanPlanCommand::DrawIndirect { .. }))
    {
        return Ok(());
    }
    // This validation-only expansion preserves command ordering and reuses the
    // direct draw checks. It is never submitted in place of the original plan.
    let mut expanded = plan.clone();
    expanded.commands.clear();
    let mut inspected = 0usize;
    let mut inspected_bytes = 0u64;
    for command in &plan.commands {
        let VulkanPlanCommand::DrawIndirect {
            buffer,
            offset,
            draw_count,
            stride,
            indexed,
        } = command
        else {
            expanded.commands.push(command.clone());
            continue;
        };
        inspected =
            inspected
                .checked_add(*draw_count as usize)
                .ok_or(VulkanError::UnsupportedFeature(
                    "indirect inspection budget",
                ))?;
        if inspected > 65_536 {
            return Err(VulkanError::UnsupportedFeature(
                "indirect inspection budget",
            ));
        }
        if *draw_count == 0 {
            continue;
        }
        if written.contains(buffer) {
            return Err(VulkanError::UnsupportedFeature(
                "GPU-written indirect validation",
            ));
        }
        let size = if *indexed { 20u64 } else { 16 };
        let step = if *stride == 0 {
            size
        } else {
            u64::from(*stride)
        };
        let length = step
            .checked_mul(u64::from(*draw_count) - 1)
            .and_then(|value| value.checked_add(size))
            .ok_or(VulkanError::ResourceRangeOverflow(buffer.0))?;
        charge_inspection_bytes(&mut inspected_bytes, length)?;
        let mut bytes = vec![0; length as usize];
        provider
            .read(*buffer, *offset, &mut bytes)
            .map_err(map_resource_error)?;
        for index in 0..u64::from(*draw_count) {
            let start = (index * step) as usize;
            let word = |offset: usize| {
                u32::from_le_bytes(
                    bytes[start + offset..start + offset + 4]
                        .try_into()
                        .unwrap(),
                )
            };
            let first_instance = word(if *indexed { 16 } else { 12 });
            // The logical device does not enable drawIndirectFirstInstance.
            if first_instance != 0 {
                return Err(VulkanError::UnsupportedFeature("indirect first instance"));
            }
            expanded.commands.push(if *indexed {
                VulkanPlanCommand::DrawIndexed {
                    indices: word(0),
                    instances: word(4),
                    first_index: word(8),
                    vertex_offset: word(12) as i32,
                }
            } else {
                VulkanPlanCommand::Draw {
                    vertices: word(0),
                    instances: word(4),
                    first_vertex: word(8),
                }
            });
        }
    }
    // Every indirect draw was replaced, so recursive validation stops here.
    validate_payloads(&expanded, provider)?;
    Ok(())
}

fn validate_indexed_vertex_payloads(
    plan: &VulkanExecutionPlan,
    provider: &dyn GpuResourceData,
    buffers: &BTreeMap<ResourceId, &BufferDesc>,
    written: &BTreeSet<ResourceId>,
) -> Result<(), VulkanError> {
    let mut pipeline = None;
    let mut vertices = BTreeMap::new();
    let mut index = None;
    let mut inspected_bytes = 0u64;
    for command in &plan.commands {
        match command {
            VulkanPlanCommand::BindGraphicsPipeline { pipeline: id } => {
                pipeline = plan.graphics_pipelines.iter().find(|desc| desc.id == *id);
            }
            VulkanPlanCommand::BindVertexBuffer {
                slot,
                buffer,
                offset,
            } => {
                vertices.insert(*slot, (*buffer, *offset));
            }
            VulkanPlanCommand::BindIndexBuffer {
                buffer,
                offset,
                index_type,
            } => {
                let width = match index_type {
                    exosphere_gpu_ir::IndexType::U8 => 1usize,
                    exosphere_gpu_ir::IndexType::U16 => 2,
                    exosphere_gpu_ir::IndexType::U32 => 4,
                    _ => return Err(VulkanError::UnsupportedFeature("index type")),
                };
                index = Some((*buffer, *offset, width));
            }
            VulkanPlanCommand::DrawIndexed {
                indices,
                instances,
                first_index,
                vertex_offset,
            } => {
                if *indices == 0 || *instances == 0 {
                    continue;
                }
                let pipeline = pipeline.ok_or(VulkanError::UnsupportedFeature(
                    "indexed draw without graphics pipeline",
                ))?;
                if pipeline.vertex_input.attributes.is_empty() {
                    continue;
                }
                let (buffer, offset, width) = index.ok_or(VulkanError::UnsupportedFeature(
                    "indexed draw without index buffer",
                ))?;
                // Pre-submit CPU bytes cannot certify indices produced by this
                // GPU plan. Such dependencies need execution-time inspection.
                if written.contains(&buffer) {
                    return Err(VulkanError::UnsupportedFeature(
                        "GPU-written index validation",
                    ));
                }
                let size = u64::from(*indices) * width as u64;
                charge_inspection_bytes(&mut inspected_bytes, size)?;
                let start = offset
                    .checked_add(u64::from(*first_index) * width as u64)
                    .ok_or(VulkanError::ResourceRangeOverflow(buffer.0))?;
                let mut bytes = vec![0; size as usize];
                provider
                    .read(buffer, start, &mut bytes)
                    .map_err(map_resource_error)?;
                let mut maximum = None;
                for value in bytes.chunks_exact(width) {
                    let raw = match width {
                        1 => u32::from(value[0]),
                        2 => u32::from(u16::from_le_bytes(value.try_into().unwrap())),
                        4 => u32::from_le_bytes(value.try_into().unwrap()),
                        _ => unreachable!("validated index width"),
                    };
                    // Restart comparison precedes base-vertex addition and
                    // never causes a vertex fetch, including all-marker draws.
                    let marker = match width {
                        1 => 0xff,
                        2 => 0xffff,
                        _ => u32::MAX,
                    };
                    if pipeline.primitive_restart && raw == marker {
                        continue;
                    }
                    let effective = i64::from(raw) + i64::from(*vertex_offset);
                    if effective < 0 {
                        return Err(VulkanError::UnsupportedFeature(
                            "negative indexed vertex fetch",
                        ));
                    }
                    maximum = Some(maximum.map_or(effective as u64, |previous: u64| {
                        previous.max(effective as u64)
                    }));
                }
                let Some(maximum) = maximum else { continue };
                for attribute in &pipeline.vertex_input.attributes {
                    let binding = pipeline
                        .vertex_input
                        .bindings
                        .iter()
                        .find(|binding| binding.binding == attribute.binding)
                        .ok_or(VulkanError::UnsupportedFeature(
                            "vertex attribute without binding",
                        ))?;
                    let (resource, offset) =
                        vertices
                            .get(&attribute.binding)
                            .ok_or(VulkanError::UnsupportedFeature(
                                "draw without vertex buffer",
                            ))?;
                    let desc = require_buffer(buffers, *resource)?;
                    let last = match binding.input_rate {
                        exosphere_gpu_ir::VertexInputRate::Vertex => maximum,
                        exosphere_gpu_ir::VertexInputRate::Instance => u64::from(*instances) - 1,
                    };
                    let address = last
                        .checked_mul(u64::from(binding.stride))
                        .and_then(|bytes| offset.checked_add(bytes))
                        .and_then(|bytes| bytes.checked_add(u64::from(attribute.offset)))
                        .ok_or(VulkanError::ResourceRangeOverflow(desc.id.0))?;
                    require_range(desc, address, vertex_format_bytes(attribute.format)?)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn charge_inspection_bytes(used: &mut u64, bytes: u64) -> Result<(), VulkanError> {
    // Bound the whole inspection phase, not merely each allocation. Otherwise
    // many padded indirect commands can cause arbitrarily large cumulative reads.
    let total = used
        .checked_add(bytes)
        .filter(|total| *total <= 64 * 1024 * 1024)
        .ok_or(VulkanError::UnsupportedFeature(
            "draw payload inspection byte budget",
        ))?;
    *used = total;
    Ok(())
}

fn vertex_format_bytes(format: exosphere_gpu_ir::VertexFormat) -> Result<u64, VulkanError> {
    format
        .byte_width()
        .ok_or(VulkanError::UnsupportedFeature("vertex attribute format"))
}

fn require_buffer<'a>(
    buffers: &'a BTreeMap<ResourceId, &'a BufferDesc>,
    id: ResourceId,
) -> Result<&'a BufferDesc, VulkanError> {
    buffers
        .get(&id)
        .copied()
        .ok_or(VulkanError::MissingResource(id.0))
}

fn require_texture<'a>(
    textures: &'a BTreeMap<ResourceId, &'a TextureDesc>,
    id: ResourceId,
) -> Result<&'a TextureDesc, VulkanError> {
    textures
        .get(&id)
        .copied()
        .ok_or(VulkanError::MissingResource(id.0))
}

fn require_range(desc: &BufferDesc, offset: u64, length: u64) -> Result<(), VulkanError> {
    let end = offset
        .checked_add(length)
        .ok_or(VulkanError::ResourceRangeOverflow(desc.id.0))?;
    if offset > desc.size || end > desc.size {
        return Err(VulkanError::ResourceRangeOutOfBounds {
            resource: desc.id.0,
            offset,
            length,
            size: desc.size,
        });
    }
    Ok(())
}

fn minimum_texture_bytes(texture: &TextureDesc) -> u64 {
    let mut total = 0u64;
    for mip in 0..texture.mip_levels.max(1) {
        let w = mip_extent(texture.width, mip);
        let h = mip_extent(texture.height, mip);
        let d = mip_extent(texture.depth, mip);
        match texture.layout {
            TextureLayout::Linear => {
                if let Some(fp) = linear_footprint(texture.format, w, h, d, texture.layers) {
                    total = total.saturating_add(fp.total_bytes)
                }
            }
            TextureLayout::BlockLinear {
                block_height_log2,
                block_depth_log2,
                ..
            } => {
                if let Some(fp) = linear_footprint(texture.format, w, h, d, texture.layers) {
                    let width_bytes = fp.row_bytes.min(u64::from(u32::MAX)) as u32;
                    let layout = BlockLinearLayout::new(
                        width_bytes,
                        fp.blocks_y,
                        d,
                        block_height_log2,
                        block_depth_log2,
                    );
                    total = total.saturating_add(
                        layout
                            .storage_size()
                            .saturating_mul(u64::from(texture.layers.max(1))),
                    );
                }
            }
            TextureLayout::Unknown(_) => return 0,
        }
    }
    total
}

fn map_resource_error(error: ResourceDataError) -> VulkanError {
    VulkanError::ResourceData(error.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn restart_topology_is_checked_before_any_resource_payload() {
        use exosphere_gpu_ir::{GpuModule, PrimitiveTopology};
        let mut plan = crate::VulkanBackend::new()
            .plan(&GpuModule::triangle_vulkan_smoke_test(32, 32))
            .unwrap();
        plan.graphics_pipelines[0].primitive_restart = true;
        for (topology, valid) in [
            (PrimitiveTopology::TriangleStrip, true),
            (PrimitiveTopology::TriangleFan, true),
            (PrimitiveTopology::LineStrip, true),
            (PrimitiveTopology::TriangleList, false),
            (PrimitiveTopology::LineList, false),
            (PrimitiveTopology::PointList, false),
            (PrimitiveTopology::LineLoop, false),
            (PrimitiveTopology::PatchList(3), false),
            (PrimitiveTopology::Unknown(9), false),
        ] {
            plan.graphics_pipelines[0].topology = topology;
            assert_eq!(super::analyze(&plan).is_ok(), valid);
        }
    }
    use super::*;
    use exosphere_gpu_ir::{BufferUsage, InMemoryGpuResources};

    #[test]
    fn payload_inspection_budget_is_cumulative_and_checked() {
        let mut used = 0;
        charge_inspection_bytes(&mut used, 32 * 1024 * 1024).unwrap();
        charge_inspection_bytes(&mut used, 32 * 1024 * 1024).unwrap();
        assert!(charge_inspection_bytes(&mut used, 1).is_err());
        assert_eq!(used, 64 * 1024 * 1024);
        assert!(charge_inspection_bytes(&mut used, u64::MAX).is_err());
        assert_eq!(used, 64 * 1024 * 1024);
    }

    #[test]
    fn direct_vertex_fetch_checks_offsets_stride_and_input_rate() {
        use exosphere_gpu_ir::{
            VertexAttributeDesc, VertexBindingDesc, VertexFormat, VertexInputRate,
        };
        let module = exosphere_gpu_ir::GpuModule::triangle_vulkan_smoke_test(64, 64);
        let mut plan = crate::VulkanBackend::new().plan(&module).unwrap();
        let id = ResourceId(92);
        plan.buffers.push(BufferDesc {
            id,
            size: 108,
            usage: BufferUsage::Vertex,
        });
        plan.graphics_pipelines[0].vertex_input.bindings = vec![VertexBindingDesc {
            binding: 1,
            stride: 20,
            input_rate: VertexInputRate::Vertex,
        }];
        plan.graphics_pipelines[0].vertex_input.attributes = vec![VertexAttributeDesc {
            location: 2,
            binding: 1,
            format: VertexFormat::Rgba32Float,
            offset: 4,
        }];
        plan.commands = vec![
            VulkanPlanCommand::BindGraphicsPipeline { pipeline: 11 },
            VulkanPlanCommand::BindVertexBuffer {
                slot: 1,
                buffer: id,
                offset: 8,
            },
            VulkanPlanCommand::Draw {
                vertices: 3,
                instances: 3,
                first_vertex: 2,
            },
        ];
        assert!(analyze(&plan).is_ok());
        plan.buffers[0].size = 107;
        assert!(matches!(
            analyze(&plan),
            Err(VulkanError::ResourceRangeOutOfBounds { .. })
        ));
        plan.buffers[0].size = 68;
        plan.graphics_pipelines[0].vertex_input.bindings[0].input_rate = VertexInputRate::Instance;
        assert!(analyze(&plan).is_ok());
        plan.buffers[0].size = 67;
        assert!(analyze(&plan).is_err());
        plan.graphics_pipelines[0].vertex_input.bindings[0].stride = 0;
        plan.buffers[0].size = 28;
        assert!(analyze(&plan).is_ok());
        plan.commands.remove(1);
        assert!(analyze(&plan).is_err());
        plan.commands[1] = VulkanPlanCommand::Draw {
            vertices: 0,
            instances: 3,
            first_vertex: u32::MAX,
        };
        assert!(analyze(&plan).is_ok());
        // Indexed draws must validate decoded values, including base vertex,
        // rather than merely the size of the index allocation.
        plan.buffers[0].size = 68;
        plan.graphics_pipelines[0].vertex_input.bindings[0].stride = 20;
        plan.graphics_pipelines[0].vertex_input.bindings[0].input_rate = VertexInputRate::Vertex;
        let index = ResourceId(93);
        plan.buffers.push(BufferDesc {
            id: index,
            size: 10,
            usage: BufferUsage::Index,
        });
        plan.commands = vec![
            VulkanPlanCommand::BindGraphicsPipeline { pipeline: 11 },
            VulkanPlanCommand::BindVertexBuffer {
                slot: 1,
                buffer: id,
                offset: 8,
            },
            VulkanPlanCommand::BindIndexBuffer {
                buffer: index,
                offset: 2,
                index_type: exosphere_gpu_ir::IndexType::U16,
            },
            VulkanPlanCommand::DrawIndexed {
                indices: 3,
                instances: 1,
                first_index: 1,
                vertex_offset: -1,
            },
        ];
        let mut provider = InMemoryGpuResources::new();
        provider.insert(id, vec![0; 68], false);
        for (values, valid) in [
            ([99u16, 99, 1, 2, 3], true),
            ([99, 99, 1, 2, 4], false),
            ([99, 99, 0, 2, 3], false),
        ] {
            provider.insert(
                index,
                values.into_iter().flat_map(u16::to_le_bytes).collect(),
                false,
            );
            assert_eq!(validate_payloads(&plan, &provider).is_ok(), valid);
        }
        plan.graphics_pipelines[0].topology = exosphere_gpu_ir::PrimitiveTopology::TriangleStrip;
        for (restart, values, valid) in [
            (false, [99u16, 99, 1, 2, u16::MAX], false),
            (true, [99, 99, 1, 2, u16::MAX], true),
            (true, [99, 99, u16::MAX, u16::MAX, u16::MAX], true),
            (true, [99, 99, 0, u16::MAX, 2], false),
        ] {
            plan.graphics_pipelines[0].primitive_restart = restart;
            provider.insert(
                index,
                values.into_iter().flat_map(u16::to_le_bytes).collect(),
                false,
            );
            assert_eq!(validate_payloads(&plan, &provider).is_ok(), valid);
        }
        plan.graphics_pipelines[0].primitive_restart = false;
        provider.insert(
            index,
            [99u16, 99, 1, 2, 3]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
            false,
        );
        let indirect = ResourceId(94);
        plan.buffers.push(BufferDesc {
            id: indirect,
            size: 24,
            usage: BufferUsage::Indirect,
        });
        *plan.commands.last_mut().unwrap() = VulkanPlanCommand::DrawIndirect {
            buffer: indirect,
            offset: 4,
            draw_count: 1,
            stride: 0,
            indexed: true,
        };
        for (arguments, valid) in [
            ([3u32, 1, 1, u32::MAX, 0], true),
            ([3, 1, 2, u32::MAX, 0], false),
            ([3, 1, 1, u32::MAX, 1], false),
        ] {
            let mut payload = vec![0xff; 4];
            payload.extend(arguments.into_iter().flat_map(u32::to_le_bytes));
            provider.insert(indirect, payload, false);
            assert_eq!(validate_payloads(&plan, &provider).is_ok(), valid);
        }
    }

    #[test]
    fn indexed_draw_range_accounts_for_binding_offset_first_and_width() {
        use exosphere_gpu_ir::IndexType;
        let id = ResourceId(88);
        for (index_type, width) in [(IndexType::U8, 1), (IndexType::U16, 2), (IndexType::U32, 4)] {
            let mut plan = VulkanExecutionPlan {
                buffers: vec![BufferDesc {
                    id,
                    size: 5 * width,
                    usage: BufferUsage::Index,
                }],
                commands: vec![
                    VulkanPlanCommand::BindIndexBuffer {
                        buffer: id,
                        offset: width,
                        index_type,
                    },
                    VulkanPlanCommand::DrawIndexed {
                        indices: 3,
                        instances: 1,
                        first_index: 1,
                        vertex_offset: -1,
                    },
                ],
                ..Default::default()
            };
            assert!(analyze(&plan).is_ok());
            plan.buffers[0].size -= 1;
            assert!(analyze(&plan).is_err());
            plan.buffers[0].size += 1;
            plan.commands.remove(0);
            assert!(analyze(&plan).is_err());
        }
    }

    #[test]
    fn indirect_multi_draw_checks_stride_and_offset_alignment() {
        let mut plan = VulkanExecutionPlan {
            buffers: vec![BufferDesc {
                id: ResourceId(9),
                size: 64,
                usage: BufferUsage::Indirect,
            }],
            ..Default::default()
        };
        for (offset, stride, valid) in [
            (0, 0, true),
            (0, 16, true),
            (0, 20, true),
            (0, 12, false),
            (0, 17, false),
            (1, 16, false),
        ] {
            plan.commands = vec![VulkanPlanCommand::DrawIndirect {
                buffer: ResourceId(9),
                offset,
                stride,
                draw_count: 2,
                indexed: false,
            }];
            assert_eq!(analyze(&plan).is_ok(), valid);
        }
    }

    #[test]
    fn detects_out_of_range_indirect_buffer() {
        let id = ResourceId(1);
        let plan = VulkanExecutionPlan {
            buffers: vec![BufferDesc {
                id,
                size: 16,
                usage: BufferUsage::Indirect,
            }],
            commands: vec![VulkanPlanCommand::DrawIndirect {
                buffer: id,
                offset: 8,
                draw_count: 1,
                stride: 0,
                indexed: false,
            }],
            ..Default::default()
        };
        assert!(matches!(
            analyze(&plan),
            Err(VulkanError::ResourceRangeOutOfBounds { .. })
        ));
    }

    #[test]
    fn read_buffer_requires_payload() {
        let id = ResourceId(2);
        let plan = VulkanExecutionPlan {
            buffers: vec![BufferDesc {
                id,
                size: 32,
                usage: BufferUsage::Vertex,
            }],
            commands: vec![VulkanPlanCommand::BindVertexBuffer {
                slot: 0,
                buffer: id,
                offset: 0,
            }],
            ..Default::default()
        };
        let mut resources = InMemoryGpuResources::new();
        resources.insert(id, vec![0; 32], false);
        assert!(validate_payloads(&plan, &resources).is_ok());
    }
}
