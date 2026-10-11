// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Canonical GPU IR -> Vulkan execution-plan lowering.

use crate::{
    VulkanError,
    plan::{VulkanExecutionPlan, VulkanPlanCommand, VulkanShaderModule},
};
use exosphere_gpu_ir::{Command, GpuModule, ShaderStage};

pub fn lower(module: &GpuModule) -> Result<VulkanExecutionPlan, VulkanError> {
    let mut shaders = Vec::with_capacity(module.shaders.len());
    let mut identities = std::collections::BTreeSet::new();
    for shader in &module.shaders {
        if !identities.insert(shader.id) {
            return Err(VulkanError::UnsupportedFeature("duplicate shader identity"));
        }
        let spirv = exosphere_gpu_ir::spirv::compile(shader)
            .map_err(|_| VulkanError::ShaderNeedsTranslation)?;
        shaders.push(VulkanShaderModule {
            id: shader.id,
            stage: shader.stage,
            spirv,
        });
    }
    for p in &module.graphics_pipelines {
        for (id, expected) in [
            (Some(p.vertex_shader), ShaderStage::Vertex),
            (p.tess_control_shader, ShaderStage::TessControl),
            (p.tess_eval_shader, ShaderStage::TessEvaluation),
            (p.geometry_shader, ShaderStage::Geometry),
            (Some(p.fragment_shader), ShaderStage::Fragment),
        ]
        .into_iter()
        .filter_map(|(id, stage)| id.map(|id| (id, stage)))
        {
            require_shader_stage(&shaders, id, expected)?;
        }
    }
    for p in &module.compute_pipelines {
        require_shader_stage(&shaders, p.compute_shader, ShaderStage::Compute)?;
    }
    let mut out = VulkanExecutionPlan {
        graphics_pipelines: module.graphics_pipelines.clone(),
        compute_pipelines: module.compute_pipelines.clone(),
        buffers: module.buffers.clone(),
        textures: module.textures.clone(),
        samplers: module.samplers.clone(),
        commands: Vec::new(),
        shaders,
    };
    for c in &module.commands {
        use VulkanPlanCommand as V;
        let v = match c {
            Command::BeginRenderPass { width, height } => V::BeginRendering {
                width: *width,
                height: *height,
            },
            Command::BindGraphicsPipeline { pipeline } => {
                if !out.graphics_pipelines.iter().any(|p| p.id == *pipeline) {
                    return Err(VulkanError::MissingPipeline(*pipeline));
                }
                V::BindGraphicsPipeline {
                    pipeline: *pipeline,
                }
            }
            Command::BindComputePipeline { pipeline } => V::BindComputePipeline {
                pipeline: *pipeline,
            },
            Command::BindVertexBuffer {
                slot,
                buffer,
                offset,
            } => V::BindVertexBuffer {
                slot: *slot,
                buffer: *buffer,
                offset: *offset,
            },
            Command::BindIndexBuffer {
                buffer,
                offset,
                index_type,
            } => V::BindIndexBuffer {
                buffer: *buffer,
                offset: *offset,
                index_type: *index_type,
            },
            Command::SetViewport { viewport } => V::SetViewport {
                viewport: *viewport,
            },
            Command::SetScissor { scissor } => V::SetScissor { scissor: *scissor },
            Command::SetRenderTarget { slot, texture } => V::SetRenderTarget {
                slot: *slot,
                texture: *texture,
            },
            Command::SetDepthStencilTarget { texture } => {
                V::SetDepthStencilTarget { texture: *texture }
            }
            Command::BindTexture { slot, texture } => V::BindTexture {
                slot: *slot,
                texture: *texture,
            },
            Command::BindSampler { slot, sampler } => V::BindSampler {
                slot: *slot,
                sampler: *sampler,
            },
            Command::BindBufferDescriptor {
                set,
                binding,
                array_element,
                buffer,
                offset,
                range,
            } => V::BindBufferDescriptor {
                set: *set,
                binding: *binding,
                array_element: *array_element,
                buffer: *buffer,
                offset: *offset,
                range: *range,
            },
            Command::BindImageDescriptor {
                set,
                binding,
                array_element,
                texture,
            } => V::BindImageDescriptor {
                set: *set,
                binding: *binding,
                array_element: *array_element,
                texture: *texture,
            },
            Command::BindSamplerDescriptor {
                set,
                binding,
                array_element,
                sampler,
            } => V::BindSamplerDescriptor {
                set: *set,
                binding: *binding,
                array_element: *array_element,
                sampler: *sampler,
            },
            Command::PushConstants {
                stages,
                offset,
                data,
            } => V::PushConstants {
                stages: *stages,
                offset: *offset,
                data: data.clone(),
            },
            Command::Draw {
                vertices,
                instances,
                first_vertex,
            } => V::Draw {
                vertices: *vertices,
                instances: *instances,
                first_vertex: *first_vertex,
            },
            Command::DrawIndexed {
                indices,
                instances,
                first_index,
                vertex_offset,
            } => V::DrawIndexed {
                indices: *indices,
                instances: *instances,
                first_index: *first_index,
                vertex_offset: *vertex_offset,
            },
            Command::DrawIndirect {
                buffer,
                offset,
                draw_count,
                stride,
                indexed,
            } => V::DrawIndirect {
                buffer: *buffer,
                offset: *offset,
                draw_count: *draw_count,
                stride: *stride,
                indexed: *indexed,
            },
            Command::EndRenderPass => V::EndRendering,
            Command::Dispatch { x, y, z } => V::Dispatch {
                x: *x,
                y: *y,
                z: *z,
            },
            Command::DispatchIndirect { buffer, offset } => V::DispatchIndirect {
                buffer: *buffer,
                offset: *offset,
            },
            Command::Copy {
                source,
                destination,
                source_offset,
                destination_offset,
                bytes,
            } => V::CopyBuffer {
                source: *source,
                destination: *destination,
                source_offset: *source_offset,
                destination_offset: *destination_offset,
                bytes: *bytes,
            },
            Command::CopyImage {
                source,
                destination,
            } => V::CopyImage {
                source: *source,
                destination: *destination,
            },
            Command::Blit {
                source,
                destination,
                filter,
            } => V::Blit {
                source: *source,
                destination: *destination,
                filter: *filter,
            },
            Command::Resolve {
                source,
                destination,
            } => V::Resolve {
                source: *source,
                destination: *destination,
            },
            Command::Fill {
                destination,
                value,
                bytes,
            } => V::FillBuffer {
                destination: *destination,
                value: *value,
                bytes: *bytes,
            },
            Command::ClearColor { target, value } => V::ClearColor {
                target: *target,
                value: *value,
            },
            Command::ClearDepthStencil {
                target,
                depth_bits,
                stencil,
                aspect,
            } => V::ClearDepthStencil {
                target: *target,
                depth_bits: *depth_bits,
                stencil: *stencil,
                aspect: *aspect,
            },
            Command::Barrier { scope } => V::PipelineBarrier { scope: *scope },
            Command::SignalSemaphore { address, payload } => V::SignalSemaphore {
                address: *address,
                payload: *payload,
            },
            Command::WaitSemaphore { address, payload } => V::WaitSemaphore {
                address: *address,
                payload: *payload,
            },
            Command::IncrementSyncpoint { id, amount } => V::IncrementSyncpoint {
                id: *id,
                amount: *amount,
            },
            Command::WaitSyncpoint { id, threshold } => V::WaitSyncpoint {
                id: *id,
                threshold: *threshold,
            },
            Command::ResetQuery { pool, first, count } => V::ResetQuery {
                pool: *pool,
                first: *first,
                count: *count,
            },
            Command::BeginQuery { pool, index, kind } => V::BeginQuery {
                pool: *pool,
                index: *index,
                kind: *kind,
            },
            Command::EndQuery { pool, index, kind } => V::EndQuery {
                pool: *pool,
                index: *index,
                kind: *kind,
            },
            Command::WriteTimestamp { pool, index } => V::WriteTimestamp {
                pool: *pool,
                index: *index,
            },
            Command::CopyQueryResults {
                pool,
                destination,
                first,
                count,
                offset,
            } => V::CopyQueryResults {
                pool: *pool,
                destination: *destination,
                first: *first,
                count: *count,
                offset: *offset,
            },
            Command::BeginConditional {
                buffer,
                offset,
                mode,
            } => V::BeginConditional {
                buffer: *buffer,
                offset: *offset,
                mode: *mode,
            },
            Command::EndConditional => V::EndConditional,
            Command::BeginTransformFeedback { buffers } => V::BeginTransformFeedback {
                buffers: buffers.clone(),
            },
            Command::EndTransformFeedback => V::EndTransformFeedback,
            Command::CacheFlush { scope } => V::CacheFlush { scope: *scope },
            Command::InvalidateCaches { scope } => V::InvalidateCaches { scope: *scope },
            Command::Opaque { .. } => return Err(VulkanError::OpaqueCommand),
        };
        out.commands.push(v);
    }
    Ok(out)
}

fn require_shader_stage(
    shaders: &[VulkanShaderModule],
    id: u64,
    expected: ShaderStage,
) -> Result<(), VulkanError> {
    let shader = shaders
        .iter()
        .find(|shader| shader.id == id)
        .ok_or(VulkanError::MissingShader(id))?;
    if shader.stage != expected {
        return Err(VulkanError::ShaderStageMismatch {
            shader: id,
            expected,
            actual: shader.stage,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shader_references_require_unique_identities_and_matching_stages() {
        let mut module = GpuModule::triangle_vulkan_smoke_test(64, 64);
        module.graphics_pipelines[0].vertex_shader = module.shaders[1].id;
        assert!(matches!(
            lower(&module),
            Err(VulkanError::ShaderStageMismatch {
                expected: ShaderStage::Vertex,
                actual: ShaderStage::Fragment,
                ..
            })
        ));
        module.graphics_pipelines[0].vertex_shader = module.shaders[0].id;
        module.graphics_pipelines[0].fragment_shader = module.shaders[0].id;
        assert!(matches!(
            lower(&module),
            Err(VulkanError::ShaderStageMismatch {
                expected: ShaderStage::Fragment,
                actual: ShaderStage::Vertex,
                ..
            })
        ));
        module.graphics_pipelines[0].fragment_shader = module.shaders[1].id;
        module.shaders.push(module.shaders[0].clone());
        assert_eq!(
            lower(&module).unwrap_err(),
            VulkanError::UnsupportedFeature("duplicate shader identity")
        );
    }
    #[test]
    fn triangle_lowers() {
        let p = lower(&GpuModule::triangle_smoke_test(64, 64)).unwrap();
        assert_eq!(
            p.graphics_pipelines
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(
            p.commands
                .iter()
                .any(|c| matches!(c, VulkanPlanCommand::Draw { vertices: 3, .. }))
        );
    }
}
