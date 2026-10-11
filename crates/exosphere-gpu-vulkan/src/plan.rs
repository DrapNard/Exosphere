// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Vulkan-specific execution plan built from canonical GPU IR.

use exosphere_gpu_ir::{
    BufferDesc, ComputePipelineDesc, ConditionalMode, FilterMode, GraphicsPipelineDesc,
    ImageAspect, IndexType, QueryKind, ResourceId, SamplerDesc, ScissorDesc, ShaderStage,
    ShaderStageMask, SyncScope, TextureDesc, ViewportDesc, spirv::SpirvModule,
};

#[derive(Debug, Clone, PartialEq)]
pub enum VulkanPlanCommand {
    BeginRendering {
        width: u32,
        height: u32,
    },
    BindGraphicsPipeline {
        pipeline: u64,
    },
    BindComputePipeline {
        pipeline: u64,
    },
    BindVertexBuffer {
        slot: u32,
        buffer: ResourceId,
        offset: u64,
    },
    BindIndexBuffer {
        buffer: ResourceId,
        offset: u64,
        index_type: IndexType,
    },
    SetViewport {
        viewport: ViewportDesc,
    },
    SetScissor {
        scissor: ScissorDesc,
    },
    SetRenderTarget {
        slot: u32,
        texture: ResourceId,
    },
    SetDepthStencilTarget {
        texture: ResourceId,
    },
    BindTexture {
        slot: u32,
        texture: ResourceId,
    },
    BindSampler {
        slot: u32,
        sampler: ResourceId,
    },
    BindBufferDescriptor {
        set: u32,
        binding: u32,
        array_element: u32,
        buffer: ResourceId,
        offset: u64,
        range: u64,
    },
    BindImageDescriptor {
        set: u32,
        binding: u32,
        array_element: u32,
        texture: ResourceId,
    },
    BindSamplerDescriptor {
        set: u32,
        binding: u32,
        array_element: u32,
        sampler: ResourceId,
    },
    PushConstants {
        stages: ShaderStageMask,
        offset: u32,
        data: Vec<u8>,
    },
    Draw {
        vertices: u32,
        instances: u32,
        first_vertex: u32,
    },
    DrawIndexed {
        indices: u32,
        instances: u32,
        first_index: u32,
        vertex_offset: i32,
    },
    DrawIndirect {
        buffer: ResourceId,
        offset: u64,
        draw_count: u32,
        stride: u32,
        indexed: bool,
    },
    EndRendering,
    Dispatch {
        x: u32,
        y: u32,
        z: u32,
    },
    DispatchIndirect {
        buffer: ResourceId,
        offset: u64,
    },
    CopyBuffer {
        source: ResourceId,
        destination: ResourceId,
        source_offset: u64,
        destination_offset: u64,
        bytes: u64,
    },
    CopyImage {
        source: ResourceId,
        destination: ResourceId,
    },
    Blit {
        source: ResourceId,
        destination: ResourceId,
        filter: FilterMode,
    },
    Resolve {
        source: ResourceId,
        destination: ResourceId,
    },
    FillBuffer {
        destination: ResourceId,
        value: u32,
        bytes: u64,
    },
    ClearColor {
        target: ResourceId,
        value: [u32; 4],
    },
    ClearDepthStencil {
        target: ResourceId,
        depth_bits: u32,
        stencil: u32,
        aspect: ImageAspect,
    },
    PipelineBarrier {
        scope: SyncScope,
    },
    SignalSemaphore {
        address: u64,
        payload: u32,
    },
    WaitSemaphore {
        address: u64,
        payload: u32,
    },
    IncrementSyncpoint {
        id: u32,
        amount: u32,
    },
    WaitSyncpoint {
        id: u32,
        threshold: u32,
    },
    ResetQuery {
        pool: ResourceId,
        first: u32,
        count: u32,
    },
    BeginQuery {
        pool: ResourceId,
        index: u32,
        kind: QueryKind,
    },
    EndQuery {
        pool: ResourceId,
        index: u32,
        kind: QueryKind,
    },
    WriteTimestamp {
        pool: ResourceId,
        index: u32,
    },
    CopyQueryResults {
        pool: ResourceId,
        destination: ResourceId,
        first: u32,
        count: u32,
        offset: u64,
    },
    BeginConditional {
        buffer: ResourceId,
        offset: u64,
        mode: ConditionalMode,
    },
    EndConditional,
    BeginTransformFeedback {
        buffers: Vec<(ResourceId, u64, u64)>,
    },
    EndTransformFeedback,
    CacheFlush {
        scope: SyncScope,
    },
    InvalidateCaches {
        scope: SyncScope,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanShaderModule {
    pub id: u64,
    pub stage: ShaderStage,
    pub spirv: SpirvModule,
}
#[derive(Debug, Clone, Default)]
pub struct VulkanExecutionPlan {
    pub commands: Vec<VulkanPlanCommand>,
    pub graphics_pipelines: Vec<GraphicsPipelineDesc>,
    pub compute_pipelines: Vec<ComputePipelineDesc>,
    pub buffers: Vec<BufferDesc>,
    pub textures: Vec<TextureDesc>,
    pub samplers: Vec<SamplerDesc>,
    pub shaders: Vec<VulkanShaderModule>,
}
