// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Canonical GPU resources and fixed-function pipeline state.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferUsage {
    Vertex,
    /// One real allocation consumed by both vertex fetch and uniform reads.
    VertexUniform,
    /// One real allocation shared by index, vertex and uniform reads.
    IndexVertexUniform,
    Index,
    Uniform,
    Storage,
    TransferSource,
    TransferDestination,
    Indirect,
    Query,
    TransformFeedback,
}

impl BufferUsage {
    pub fn supports_vertex(self) -> bool {
        matches!(
            self,
            Self::Vertex | Self::VertexUniform | Self::IndexVertexUniform
        )
    }

    pub fn supports_uniform(self) -> bool {
        matches!(
            self,
            Self::Uniform | Self::VertexUniform | Self::IndexVertexUniform
        )
    }

    pub fn supports_index(self) -> bool {
        matches!(self, Self::Index | Self::IndexVertexUniform)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferDesc {
    pub id: ResourceId,
    pub size: u64,
    pub usage: BufferUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    R8Unorm,
    R8Uint,
    R8Sint,
    Rg8Unorm,
    Rg8Uint,
    Rg8Sint,
    Rgba8Unorm,
    Rgba8Srgb,
    Bgra8Unorm,
    Bgra8Srgb,
    R16Float,
    Rg16Float,
    Rgba16Float,
    R32Float,
    Rg32Float,
    Rgba32Float,
    R32Uint,
    R32Sint,
    D16Unorm,
    D24UnormS8,
    D32Float,
    D32FloatS8,
    Bc1RgbaUnorm,
    Bc1RgbaSrgb,
    Bc2RgbaUnorm,
    Bc2RgbaSrgb,
    Bc3RgbaUnorm,
    Bc3RgbaSrgb,
    Bc4Unorm,
    Bc4Snorm,
    Bc5Unorm,
    Bc5Snorm,
    Bc6hUfloat,
    Bc6hSfloat,
    Bc7RgbaUnorm,
    Bc7RgbaSrgb,
    Astc4x4Unorm,
    Astc5x4Unorm,
    Astc5x5Unorm,
    Astc6x5Unorm,
    Astc6x6Unorm,
    Astc8x5Unorm,
    Astc8x6Unorm,
    Astc8x8Unorm,
    Astc10x5Unorm,
    Astc10x6Unorm,
    Astc10x8Unorm,
    Astc10x10Unorm,
    Astc12x10Unorm,
    Astc12x12Unorm,
    Unknown(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureLayout {
    Linear,
    BlockLinear {
        block_height_log2: u8,
        block_depth_log2: u8,
        raw: u32,
    },
    Unknown(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureDesc {
    pub id: ResourceId,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub layers: u32,
    pub mip_levels: u32,
    /// Number of samples for renderable images (1 for ordinary textures).
    pub samples: u8,
    pub format: TextureFormat,
    pub layout: TextureLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTopology {
    TriangleList,
    TriangleStrip,
    TriangleFan,
    LineList,
    LineStrip,
    LineLoop,
    PointList,
    PatchList(u8),
    Unknown(u32),
}

impl PrimitiveTopology {
    /// Core Vulkan restart support, without optional list-restart features.
    pub fn supports_standard_restart(self) -> bool {
        matches!(
            self,
            Self::TriangleStrip | Self::TriangleFan | Self::LineStrip
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    U8,
    U16,
    U32,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Never,
    Less,
    Equal,
    LessOrEqual,
    Greater,
    NotEqual,
    GreaterOrEqual,
    Always,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StencilOp {
    Keep,
    Zero,
    Replace,
    IncrementClamp,
    DecrementClamp,
    Invert,
    IncrementWrap,
    DecrementWrap,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullMode {
    None,
    Front,
    Back,
    FrontAndBack,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontFace {
    Clockwise,
    CounterClockwise,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolygonMode {
    Fill,
    Line,
    Point,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    ConstantColor,
    OneMinusConstantColor,
    ConstantAlpha,
    OneMinusConstantAlpha,
    SrcAlphaSaturate,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendOp {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
    Unknown(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicOp {
    Clear,
    And,
    AndReverse,
    Copy,
    AndInverted,
    NoOp,
    Xor,
    Or,
    Nor,
    Equivalent,
    Invert,
    OrReverse,
    CopyInverted,
    OrInverted,
    Nand,
    Set,
    Unknown(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportDesc {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScissorDesc {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StencilFaceState {
    pub fail: StencilOp,
    pub pass: StencilOp,
    pub depth_fail: StencilOp,
    pub compare: CompareOp,
    pub compare_mask: u32,
    pub write_mask: u32,
    pub reference: u32,
}
impl Default for StencilFaceState {
    fn default() -> Self {
        Self {
            fail: StencilOp::Keep,
            pass: StencilOp::Keep,
            depth_fail: StencilOp::Keep,
            compare: CompareOp::Always,
            compare_mask: u32::MAX,
            write_mask: u32::MAX,
            reference: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthStencilState {
    pub depth_test: bool,
    pub depth_write: bool,
    pub depth_compare: CompareOp,
    pub stencil_test: bool,
    pub front: StencilFaceState,
    pub back: StencilFaceState,
}
impl Default for DepthStencilState {
    fn default() -> Self {
        Self {
            depth_test: false,
            depth_write: false,
            depth_compare: CompareOp::Always,
            stencil_test: false,
            front: Default::default(),
            back: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DepthBiasFactors {
    /// IEEE-754 words retain exact pipeline identity, including signed zero.
    pub constant_bits: u32,
    pub slope_bits: u32,
    pub clamp_bits: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterState {
    pub cull: CullMode,
    pub front_face: FrontFace,
    pub polygon_mode: PolygonMode,
    pub depth_clamp: bool,
    pub rasterizer_discard: bool,
    pub depth_bias: bool,
    pub depth_bias_factors: DepthBiasFactors,
}
impl Default for RasterState {
    fn default() -> Self {
        Self {
            cull: CullMode::None,
            front_face: FrontFace::CounterClockwise,
            polygon_mode: PolygonMode::Fill,
            depth_clamp: false,
            rasterizer_discard: false,
            depth_bias: false,
            depth_bias_factors: DepthBiasFactors::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlendTargetState {
    pub enable: bool,
    pub src_color: BlendFactor,
    pub dst_color: BlendFactor,
    pub color_op: BlendOp,
    pub src_alpha: BlendFactor,
    pub dst_alpha: BlendFactor,
    pub alpha_op: BlendOp,
    pub color_write_mask: u8,
}
impl Default for BlendTargetState {
    fn default() -> Self {
        Self {
            enable: false,
            src_color: BlendFactor::One,
            dst_color: BlendFactor::Zero,
            color_op: BlendOp::Add,
            src_alpha: BlendFactor::One,
            dst_alpha: BlendFactor::Zero,
            alpha_op: BlendOp::Add,
            color_write_mask: 0xF,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlendState {
    pub targets: [BlendTargetState; 8],
    pub logic_op_enable: bool,
    pub logic_op: LogicOp,
    pub constants: [u32; 4],
}
impl Default for BlendState {
    fn default() -> Self {
        Self {
            targets: [BlendTargetState::default(); 8],
            logic_op_enable: false,
            logic_op: LogicOp::Copy,
            constants: [0; 4],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultisampleState {
    pub raster_samples: u8,
    pub sample_mask: u32,
    pub alpha_to_coverage: bool,
    pub alpha_to_one: bool,
}
impl Default for MultisampleState {
    fn default() -> Self {
        Self {
            raster_samples: 1,
            sample_mask: u32::MAX,
            alpha_to_coverage: false,
            alpha_to_one: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FixedFunctionState {
    pub raster: RasterState,
    pub depth_stencil: DepthStencilState,
    pub blend: BlendState,
    pub multisample: MultisampleState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexInputRate {
    Vertex,
    Instance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    R8Unorm,
    R8Snorm,
    R8Uint,
    R8Sint,
    Rg8Unorm,
    Rg8Snorm,
    Rg8Uint,
    Rg8Sint,
    Rgba8Unorm,
    Rgba8Snorm,
    Rgba8Uint,
    Rgba8Sint,
    R16Uint,
    R16Sint,
    R16Float,
    Rg16Uint,
    Rg16Sint,
    Rg16Float,
    Rgba16Uint,
    Rgba16Sint,
    Rgba16Float,
    R32Uint,
    R32Sint,
    R32Float,
    Rg32Uint,
    Rg32Sint,
    Rg32Float,
    Rgb32Uint,
    Rgb32Sint,
    Rgb32Float,
    Rgba32Uint,
    Rgba32Sint,
    Rgba32Float,
    Unknown(u32),
}

impl VertexFormat {
    /// Scalar signedness required by a typed integer shader input.
    pub fn integer_signedness(self) -> Option<bool> {
        use VertexFormat::*;
        match self {
            R8Uint | Rg8Uint | Rgba8Uint | R16Uint | Rg16Uint | Rgba16Uint | R32Uint | Rg32Uint
            | Rgb32Uint | Rgba32Uint => Some(false),
            R8Sint | Rg8Sint | Rgba8Sint | R16Sint | Rg16Sint | Rgba16Sint | R32Sint | Rg32Sint
            | Rgb32Sint | Rgba32Sint => Some(true),
            _ => None,
        }
    }
    /// Exact storage width of one attribute fetch; unknown encodings have no
    /// trustworthy width and must not acquire a guessed layout.
    pub fn byte_width(self) -> Option<u64> {
        use VertexFormat::*;
        Some(match self {
            R8Unorm | R8Snorm | R8Uint | R8Sint => 1,
            Rg8Unorm | Rg8Snorm | Rg8Uint | Rg8Sint | R16Uint | R16Sint | R16Float => 2,
            Rgba8Unorm | Rgba8Snorm | Rgba8Uint | Rgba8Sint | Rg16Uint | Rg16Sint | Rg16Float
            | R32Uint | R32Sint | R32Float => 4,
            Rgba16Uint | Rgba16Sint | Rgba16Float | Rg32Uint | Rg32Sint | Rg32Float => 8,
            Rgb32Uint | Rgb32Sint | Rgb32Float => 12,
            Rgba32Uint | Rgba32Sint | Rgba32Float => 16,
            Unknown(_) => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexBindingDesc {
    pub binding: u32,
    pub stride: u32,
    pub input_rate: VertexInputRate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexAttributeDesc {
    pub location: u32,
    pub binding: u32,
    pub format: VertexFormat,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VertexInputState {
    pub bindings: Vec<VertexBindingDesc>,
    pub attributes: Vec<VertexAttributeDesc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorType {
    UniformBuffer,
    StorageBuffer,
    SampledImage,
    StorageImage,
    Sampler,
    CombinedImageSampler,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderStageMask(pub u32);
impl ShaderStageMask {
    pub const VERTEX: u32 = 1 << 0;
    pub const TESS_CONTROL: u32 = 1 << 1;
    pub const TESS_EVAL: u32 = 1 << 2;
    pub const GEOMETRY: u32 = 1 << 3;
    pub const FRAGMENT: u32 = 1 << 4;
    pub const COMPUTE: u32 = 1 << 5;
    pub const ALL_GRAPHICS: u32 =
        Self::VERTEX | Self::TESS_CONTROL | Self::TESS_EVAL | Self::GEOMETRY | Self::FRAGMENT;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescriptorBindingDesc {
    pub set: u32,
    pub binding: u32,
    pub descriptor_type: DescriptorType,
    pub count: u32,
    pub stages: ShaderStageMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushConstantRangeDesc {
    pub offset: u32,
    pub size: u32,
    pub stages: ShaderStageMask,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PipelineLayoutDesc {
    pub descriptors: Vec<DescriptorBindingDesc>,
    pub push_constants: Vec<PushConstantRangeDesc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicsPipelineDesc {
    pub id: u64,
    pub vertex_shader: u64,
    pub tess_control_shader: Option<u64>,
    pub tess_eval_shader: Option<u64>,
    pub geometry_shader: Option<u64>,
    pub fragment_shader: u64,
    pub topology: PrimitiveTopology,
    /// Use the index type's maximum value to restart strip/fan assembly.
    pub primitive_restart: bool,
    pub vertex_input: VertexInputState,
    pub layout: PipelineLayoutDesc,
    pub fixed_function: FixedFunctionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputePipelineDesc {
    pub id: u64,
    pub compute_shader: u64,
    pub layout: PipelineLayoutDesc,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplerAddressMode {
    Repeat,
    MirroredRepeat,
    ClampToEdge,
    ClampToBorder,
    Clamp,
    MirrorClampToEdge,
    MirrorClampToBorder,
    MirrorClamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplerMipFilter {
    None,
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplerBorderColor {
    FloatTransparentBlack,
    FloatOpaqueBlack,
    FloatOpaqueWhite,
    IntTransparentBlack,
    IntOpaqueBlack,
    IntOpaqueWhite,
}

/// Explicit sampling state. Fixed-point LODs remain exact and hash/equality-safe;
/// backends convert them to floats only when creating the native sampler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplerState {
    pub min_filter: crate::command::FilterMode,
    pub mag_filter: crate::command::FilterMode,
    pub mip_filter: SamplerMipFilter,
    pub address: [SamplerAddressMode; 3],
    pub min_lod_8_8: u16,
    pub max_lod_8_8: u16,
    pub lod_bias_8_8: i16,
    pub max_anisotropy: u8,
    pub compare: Option<CompareOp>,
    pub border: SamplerBorderColor,
    pub unnormalized_coordinates: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplerDesc {
    pub id: ResourceId,
    pub state: SamplerState,
}
