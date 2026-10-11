// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Multi-stage canonical GPU IR shared by Vulkan, Metal and OpenGL compatibility backends.

pub mod address_space;
pub mod block_linear;
pub mod cache;
pub mod command;
pub mod compiler;
pub mod compute;
pub mod coverage;
pub mod descriptor_memory;
pub mod descriptors;
pub mod dma;
pub mod engine;
pub mod envydis_oracle;
pub mod frontend;
pub mod gpfifo;
pub mod graph;
pub mod hazard;
pub mod host_semaphore;
pub mod inline_to_memory;
pub mod maxwell;
pub mod maxwell_constant;
pub mod maxwell_fragment;
pub mod maxwell_register_ir;
pub mod maxwell_semantics;
pub mod maxwell_shader;
pub mod maxwell_state;
pub mod maxwell_vertex;
pub mod mme;
pub mod mme_exec;
pub mod module;
pub mod pfifo;
pub mod pipeline;
pub mod qmd;
pub mod resource;
pub mod resource_data;
pub mod resource_table;
pub mod semaphore;
pub mod shader;
pub mod shader_discovery;
pub mod shader_extent;
pub mod shader_memory;
pub mod shader_resources;
pub mod spirv;
pub mod sync;
pub mod texture_backing;
pub mod texture_data;
pub mod texture_layout;
pub mod texture_sampling;
pub mod tic_tsc;
pub mod translator;
pub mod twod;

pub use command::{
    Command, ConditionalMode, FilterMode, ImageAspect, QueryKind, QueueClass, SyncScope,
};
pub use module::GpuModule;
pub use resource::{
    BlendFactor, BlendOp, BlendState, BlendTargetState, BufferDesc, BufferUsage, CompareOp,
    ComputePipelineDesc, CullMode, DepthStencilState, DescriptorBindingDesc, DescriptorType,
    FixedFunctionState, FrontFace, GraphicsPipelineDesc, IndexType, LogicOp, MultisampleState,
    PipelineLayoutDesc, PolygonMode, PrimitiveTopology, PushConstantRangeDesc, RasterState,
    ResourceId, SamplerAddressMode, SamplerBorderColor, SamplerDesc, SamplerMipFilter,
    SamplerState, ScissorDesc, ShaderStageMask, StencilFaceState, StencilOp, TextureDesc,
    TextureFormat, TextureLayout, VertexAttributeDesc, VertexBindingDesc, VertexFormat,
    VertexInputRate, VertexInputState, ViewportDesc,
};
pub use shader::{ShaderModule, ShaderOperation, ShaderStage, TextureSampleChannel};

pub use engine::{EngineClass, EngineMethodWrite, SubchannelTracker};
pub use pfifo::{MethodWrite, PfifoError, SecondaryOpcode, decode_pushbuffer};

pub use maxwell_state::{Maxwell3dState, MaxwellEvent, RawGpuAddress};
pub use shader_memory::{
    GpuMemory, GpuMemoryAccess, GpuMemoryError, LinearGpuMemory, ShaderLocation,
};

pub use frontend::{FrontEndTrace, MaxwellFrontEnd, OpaqueEngineWrite};

pub use dma::{DmaCopyState, DmaEvent, DmaLaunch};

pub use compute::{ComputeEvent, ComputeState};
pub use inline_to_memory::{InlineToMemoryEvent, InlineToMemoryState};
pub use translator::{TranslationStats, lower_trace, lower_trace_with_address_space};
pub use twod::{TwoDEvent, TwoDState};

pub use coverage::{CoverageCounter, GpuCoverage};
pub use graph::{GpuGraph, GraphNode, NodeId};
pub use shader_discovery::{DiscoveredShader, discover_draw_shaders, discover_graphics_shaders};

pub use compiler::{CompiledShader, GpuCompileError, ShaderWindow};

pub use address_space::{
    GpuAddressError, GpuAddressSpace, GpuMapPermissions, GpuMapping, ResolvedGpuRange,
};
pub use descriptors::{DescriptorHeap, RawDescriptor, SamplerDescriptor, TextureDescriptor};
pub use hazard::{Hazard, HazardKind, HazardReport, analyze_hazards};
pub use texture_layout::{FormatInfo, TextureFootprint, format_info, linear_footprint};

pub use resource_table::ResourceTable;

pub use block_linear::{
    BlockLinearLayout, GOB_HEIGHT, GOB_SIZE, GOB_WIDTH_BYTES, deswizzle_layers,
};
pub use mme_exec::{
    MmeAluOp, MmeExecutionResult, MmeInstruction, MmeMethodEmission, MmeResultAction,
    MmeStopReason, decode_program as decode_mme_program, decode_word as decode_mme_word,
    execute as execute_mme, execute_with_method_reader as execute_mme_with_method_reader,
};
pub use semaphore::{SemaphoreCompare, SemaphoreOperation, SyncTimeline};
pub use tic_tsc::{
    AddressMode, DecodedSamplerState, FilterMode as SamplerFilterMode, decode_tic, decode_tsc,
    split_texture_handle,
};

pub use descriptor_memory::{
    DESCRIPTOR_SIZE, read_tic, read_tsc, read_words as read_descriptor_words,
};

pub use resource_data::{
    GpuResourceData, InMemoryGpuResources, MappedGpuResourceData, ResourceDataError,
};

pub use maxwell_shader::{
    MaxwellCommonOperands, MaxwellDecodedInstruction, MaxwellOpcode, MaxwellOpcodeFamily,
    MaxwellPredicateGuard, MaxwellScheduleEntry, OpcodeCoverage,
    classify_instruction as classify_maxwell_instruction,
    decode_branch_target as decode_maxwell_branch_target,
    decode_common_operands as decode_maxwell_common_operands,
    decode_opcode as decode_maxwell_opcode,
    decode_predicate_guard as decode_maxwell_predicate_guard,
    decode_scheduler_word as decode_maxwell_scheduler_word,
    opcode_coverage as maxwell_opcode_coverage,
};

pub use maxwell_semantics::{
    MaxwellOperand, MaxwellOperandForm, MaxwellSemanticInstruction, MaxwellSemanticOpcode,
    SemanticCoverageReport, SemanticDisposition,
    canonical_semantic_coverage as maxwell_canonical_semantic_coverage,
    canonicalize_disposition as canonicalize_maxwell_semantic,
    decode_semantic as decode_maxwell_semantic, lower_semantic as lower_maxwell_semantic,
    lowering_coverage as maxwell_lowering_coverage, semantic_coverage as maxwell_semantic_coverage,
};
