// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Backend-neutral GPU command stream after semantic lowering.

use crate::resource::{IndexType, ResourceId, ScissorDesc, ShaderStageMask, ViewportDesc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueClass {
    Graphics,
    Compute,
    Transfer,
    TwoD,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncScope {
    Queue,
    Device,
    HostVisible,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageAspect {
    Color,
    Depth,
    Stencil,
    DepthStencil,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    Occlusion,
    Timestamp,
    PipelineStatistics,
    TransformFeedback,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalMode {
    Wait,
    NoWait,
    ByRegion,
    Inverted,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Nearest,
    Linear,
}

#[derive(Debug, Clone)]
pub enum Command {
    BeginRenderPass {
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
    /// Explicit descriptor-buffer binding used once shader reflection/layout information is known.
    BindBufferDescriptor {
        set: u32,
        binding: u32,
        array_element: u32,
        buffer: ResourceId,
        offset: u64,
        range: u64,
    },
    /// Explicit sampled/storage image binding. The pipeline layout determines the descriptor type.
    BindImageDescriptor {
        set: u32,
        binding: u32,
        array_element: u32,
        texture: ResourceId,
    },
    /// Explicit sampler binding. Combined image samplers are assembled by the backend from matching image/sampler bindings.
    BindSamplerDescriptor {
        set: u32,
        binding: u32,
        array_element: u32,
        sampler: ResourceId,
    },
    /// Raw push-constant bytes. The range is validated against the bound pipeline layout.
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
    EndRenderPass,
    Dispatch {
        x: u32,
        y: u32,
        z: u32,
    },
    DispatchIndirect {
        buffer: ResourceId,
        offset: u64,
    },
    Copy {
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
    Fill {
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
    Barrier {
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
    Opaque {
        class_id: u32,
        method: u32,
        argument: u32,
    },
}
