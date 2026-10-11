// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Complete canonical GPU module.

use crate::{
    command::Command,
    resource::{BufferDesc, ComputePipelineDesc, GraphicsPipelineDesc, SamplerDesc, TextureDesc},
    shader::ShaderModule,
};

#[derive(Debug, Clone, Default)]
pub struct GpuModule {
    pub commands: Vec<Command>,
    pub shaders: Vec<ShaderModule>,
    pub buffers: Vec<BufferDesc>,
    pub textures: Vec<TextureDesc>,
    pub samplers: Vec<SamplerDesc>,
    pub graphics_pipelines: Vec<GraphicsPipelineDesc>,
    pub compute_pipelines: Vec<ComputePipelineDesc>,
}

impl GpuModule {
    fn constant_fragment_operations(color: [f32; 4]) -> Vec<crate::ShaderOperation> {
        use crate::ShaderOperation;
        let mut operations = Vec::with_capacity(9);
        for (component, value) in color.into_iter().enumerate() {
            operations.push(ShaderOperation::ConstantF32 { dst: 0, value });
            operations.push(ShaderOperation::StoreOutput {
                location: 0,
                component: component as u8,
                src: 0,
            });
        }
        operations.push(ShaderOperation::Return);
        operations
    }

    /// Portable triangle used to validate IR/backend plumbing before full Maxwell translation exists.
    pub fn triangle_smoke_test(width: u32, height: u32) -> Self {
        use crate::{
            resource::{GraphicsPipelineDesc, PrimitiveTopology},
            shader::{ShaderModule, ShaderOperation, ShaderStage},
        };
        let vertex_shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: b"triangle-vs".to_vec(),
            operations: vec![
                ShaderOperation::SmokeVertexTriangle,
                ShaderOperation::Return,
            ],
        };
        let fragment_shader = ShaderModule {
            id: 2,
            stage: ShaderStage::Fragment,
            variant_key: b"triangle-fs".to_vec(),
            operations: Self::constant_fragment_operations([1.0; 4]),
        };
        Self {
            shaders: vec![vertex_shader, fragment_shader],
            graphics_pipelines: vec![GraphicsPipelineDesc {
                id: 1,
                vertex_shader: 1,
                tess_control_shader: None,
                tess_eval_shader: None,
                geometry_shader: None,
                fragment_shader: 2,
                topology: PrimitiveTopology::TriangleList,
                primitive_restart: false,
                vertex_input: Default::default(),
                layout: Default::default(),
                fixed_function: Default::default(),
            }],
            commands: vec![
                Command::BeginRenderPass { width, height },
                Command::BindGraphicsPipeline { pipeline: 1 },
                Command::Draw {
                    vertices: 3,
                    instances: 1,
                    first_vertex: 0,
                },
                Command::EndRenderPass,
            ],
            ..Self::default()
        }
    }

    /// Triangle module that exercises the general Vulkan backend rather than the legacy raw smoke path.
    ///
    /// Unlike [`Self::triangle_smoke_test`], this version owns an explicit color target and emits viewport/
    /// scissor state so Vulkan 1.3 dynamic rendering has all required canonical information.
    pub fn triangle_vulkan_smoke_test(width: u32, height: u32) -> Self {
        use crate::{
            resource::{
                GraphicsPipelineDesc, PrimitiveTopology, ResourceId, ScissorDesc, TextureDesc,
                TextureFormat, TextureLayout, ViewportDesc,
            },
            shader::{ShaderModule, ShaderOperation, ShaderStage},
        };
        let vertex_shader = ShaderModule {
            id: 11,
            stage: ShaderStage::Vertex,
            variant_key: b"ash-triangle-vs".to_vec(),
            operations: vec![
                ShaderOperation::SmokeVertexTriangle,
                ShaderOperation::Return,
            ],
        };
        let fragment_shader = ShaderModule {
            id: 12,
            stage: ShaderStage::Fragment,
            variant_key: b"ash-triangle-fs".to_vec(),
            operations: Self::constant_fragment_operations([0.2, 0.8, 1.0, 1.0]),
        };
        let target = ResourceId(0xE000_0001);
        Self {
            shaders: vec![vertex_shader, fragment_shader],
            textures: vec![TextureDesc {
                id: target,
                width,
                height,
                depth: 1,
                layers: 1,
                mip_levels: 1,
                samples: 1,
                format: TextureFormat::Rgba8Unorm,
                layout: TextureLayout::Linear,
            }],
            graphics_pipelines: vec![GraphicsPipelineDesc {
                id: 11,
                vertex_shader: 11,
                tess_control_shader: None,
                tess_eval_shader: None,
                geometry_shader: None,
                fragment_shader: 12,
                topology: PrimitiveTopology::TriangleList,
                primitive_restart: false,
                vertex_input: Default::default(),
                layout: Default::default(),
                fixed_function: Default::default(),
            }],
            commands: vec![
                Command::ClearColor {
                    target,
                    value: [0, 0, 0, 1.0f32.to_bits()],
                },
                Command::SetRenderTarget {
                    slot: 0,
                    texture: target,
                },
                Command::BeginRenderPass { width, height },
                Command::BindGraphicsPipeline { pipeline: 11 },
                Command::SetViewport {
                    viewport: ViewportDesc {
                        x: 0.0,
                        y: 0.0,
                        width: width as f32,
                        height: height as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    },
                },
                Command::SetScissor {
                    scissor: ScissorDesc {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                },
                Command::Draw {
                    vertices: 3,
                    instances: 1,
                    first_vertex: 0,
                },
                Command::EndRenderPass,
            ],
            ..Self::default()
        }
    }
}
