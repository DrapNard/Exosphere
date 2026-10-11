// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Canonical Exosphere GPU enums -> Vulkan 1.3 enums.
//! Keeping conversions here prevents Vulkan constants from leaking into canonical GPU IR.

use ash::vk;
use exosphere_gpu_ir::{
    BlendFactor, BlendOp, CompareOp, CullMode, FrontFace, IndexType, PolygonMode,
    PrimitiveTopology, StencilOp, TextureFormat, VertexFormat, VertexInputRate,
};

pub fn texture_format(value: TextureFormat) -> Option<vk::Format> {
    Some(match value {
        TextureFormat::R8Unorm => vk::Format::R8_UNORM,
        TextureFormat::R8Uint => vk::Format::R8_UINT,
        TextureFormat::R8Sint => vk::Format::R8_SINT,
        TextureFormat::Rg8Unorm => vk::Format::R8G8_UNORM,
        TextureFormat::Rg8Uint => vk::Format::R8G8_UINT,
        TextureFormat::Rg8Sint => vk::Format::R8G8_SINT,
        TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        TextureFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        TextureFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        TextureFormat::Bgra8Srgb => vk::Format::B8G8R8A8_SRGB,
        TextureFormat::R16Float => vk::Format::R16_SFLOAT,
        TextureFormat::Rg16Float => vk::Format::R16G16_SFLOAT,
        TextureFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        TextureFormat::R32Float => vk::Format::R32_SFLOAT,
        TextureFormat::Rg32Float => vk::Format::R32G32_SFLOAT,
        TextureFormat::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
        TextureFormat::R32Uint => vk::Format::R32_UINT,
        TextureFormat::R32Sint => vk::Format::R32_SINT,
        TextureFormat::D16Unorm => vk::Format::D16_UNORM,
        TextureFormat::D24UnormS8 => vk::Format::D24_UNORM_S8_UINT,
        TextureFormat::D32Float => vk::Format::D32_SFLOAT,
        TextureFormat::D32FloatS8 => vk::Format::D32_SFLOAT_S8_UINT,
        TextureFormat::Bc1RgbaUnorm => vk::Format::BC1_RGBA_UNORM_BLOCK,
        TextureFormat::Bc1RgbaSrgb => vk::Format::BC1_RGBA_SRGB_BLOCK,
        TextureFormat::Bc2RgbaUnorm => vk::Format::BC2_UNORM_BLOCK,
        TextureFormat::Bc2RgbaSrgb => vk::Format::BC2_SRGB_BLOCK,
        TextureFormat::Bc3RgbaUnorm => vk::Format::BC3_UNORM_BLOCK,
        TextureFormat::Bc3RgbaSrgb => vk::Format::BC3_SRGB_BLOCK,
        TextureFormat::Bc4Unorm => vk::Format::BC4_UNORM_BLOCK,
        TextureFormat::Bc4Snorm => vk::Format::BC4_SNORM_BLOCK,
        TextureFormat::Bc5Unorm => vk::Format::BC5_UNORM_BLOCK,
        TextureFormat::Bc5Snorm => vk::Format::BC5_SNORM_BLOCK,
        TextureFormat::Bc6hUfloat => vk::Format::BC6H_UFLOAT_BLOCK,
        TextureFormat::Bc6hSfloat => vk::Format::BC6H_SFLOAT_BLOCK,
        TextureFormat::Bc7RgbaUnorm => vk::Format::BC7_UNORM_BLOCK,
        TextureFormat::Bc7RgbaSrgb => vk::Format::BC7_SRGB_BLOCK,
        TextureFormat::Astc4x4Unorm => vk::Format::ASTC_4X4_UNORM_BLOCK,
        TextureFormat::Astc5x4Unorm => vk::Format::ASTC_5X4_UNORM_BLOCK,
        TextureFormat::Astc5x5Unorm => vk::Format::ASTC_5X5_UNORM_BLOCK,
        TextureFormat::Astc6x5Unorm => vk::Format::ASTC_6X5_UNORM_BLOCK,
        TextureFormat::Astc6x6Unorm => vk::Format::ASTC_6X6_UNORM_BLOCK,
        TextureFormat::Astc8x5Unorm => vk::Format::ASTC_8X5_UNORM_BLOCK,
        TextureFormat::Astc8x6Unorm => vk::Format::ASTC_8X6_UNORM_BLOCK,
        TextureFormat::Astc8x8Unorm => vk::Format::ASTC_8X8_UNORM_BLOCK,
        TextureFormat::Astc10x5Unorm => vk::Format::ASTC_10X5_UNORM_BLOCK,
        TextureFormat::Astc10x6Unorm => vk::Format::ASTC_10X6_UNORM_BLOCK,
        TextureFormat::Astc10x8Unorm => vk::Format::ASTC_10X8_UNORM_BLOCK,
        TextureFormat::Astc10x10Unorm => vk::Format::ASTC_10X10_UNORM_BLOCK,
        TextureFormat::Astc12x10Unorm => vk::Format::ASTC_12X10_UNORM_BLOCK,
        TextureFormat::Astc12x12Unorm => vk::Format::ASTC_12X12_UNORM_BLOCK,
        TextureFormat::Unknown(_) => return None,
    })
}

pub fn vertex_format(value: VertexFormat) -> Option<vk::Format> {
    Some(match value {
        VertexFormat::R8Unorm => vk::Format::R8_UNORM,
        VertexFormat::R8Snorm => vk::Format::R8_SNORM,
        VertexFormat::R8Uint => vk::Format::R8_UINT,
        VertexFormat::R8Sint => vk::Format::R8_SINT,
        VertexFormat::Rg8Unorm => vk::Format::R8G8_UNORM,
        VertexFormat::Rg8Snorm => vk::Format::R8G8_SNORM,
        VertexFormat::Rg8Uint => vk::Format::R8G8_UINT,
        VertexFormat::Rg8Sint => vk::Format::R8G8_SINT,
        VertexFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        VertexFormat::Rgba8Snorm => vk::Format::R8G8B8A8_SNORM,
        VertexFormat::Rgba8Uint => vk::Format::R8G8B8A8_UINT,
        VertexFormat::Rgba8Sint => vk::Format::R8G8B8A8_SINT,
        VertexFormat::R16Uint => vk::Format::R16_UINT,
        VertexFormat::R16Sint => vk::Format::R16_SINT,
        VertexFormat::R16Float => vk::Format::R16_SFLOAT,
        VertexFormat::Rg16Uint => vk::Format::R16G16_UINT,
        VertexFormat::Rg16Sint => vk::Format::R16G16_SINT,
        VertexFormat::Rg16Float => vk::Format::R16G16_SFLOAT,
        VertexFormat::Rgba16Uint => vk::Format::R16G16B16A16_UINT,
        VertexFormat::Rgba16Sint => vk::Format::R16G16B16A16_SINT,
        VertexFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        VertexFormat::R32Uint => vk::Format::R32_UINT,
        VertexFormat::R32Sint => vk::Format::R32_SINT,
        VertexFormat::R32Float => vk::Format::R32_SFLOAT,
        VertexFormat::Rg32Uint => vk::Format::R32G32_UINT,
        VertexFormat::Rg32Sint => vk::Format::R32G32_SINT,
        VertexFormat::Rg32Float => vk::Format::R32G32_SFLOAT,
        VertexFormat::Rgb32Uint => vk::Format::R32G32B32_UINT,
        VertexFormat::Rgb32Sint => vk::Format::R32G32B32_SINT,
        VertexFormat::Rgb32Float => vk::Format::R32G32B32_SFLOAT,
        VertexFormat::Rgba32Uint => vk::Format::R32G32B32A32_UINT,
        VertexFormat::Rgba32Sint => vk::Format::R32G32B32A32_SINT,
        VertexFormat::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
        VertexFormat::Unknown(_) => return None,
    })
}

pub fn input_rate(value: VertexInputRate) -> vk::VertexInputRate {
    match value {
        VertexInputRate::Vertex => vk::VertexInputRate::VERTEX,
        VertexInputRate::Instance => vk::VertexInputRate::INSTANCE,
    }
}
pub fn index_type(value: IndexType) -> Option<vk::IndexType> {
    Some(match value {
        IndexType::U8 => vk::IndexType::UINT8_KHR,
        IndexType::U16 => vk::IndexType::UINT16,
        IndexType::U32 => vk::IndexType::UINT32,
        IndexType::Unknown(_) => return None,
    })
}
pub fn topology(value: PrimitiveTopology) -> Option<vk::PrimitiveTopology> {
    Some(match value {
        PrimitiveTopology::TriangleList => vk::PrimitiveTopology::TRIANGLE_LIST,
        PrimitiveTopology::TriangleStrip => vk::PrimitiveTopology::TRIANGLE_STRIP,
        PrimitiveTopology::TriangleFan => vk::PrimitiveTopology::TRIANGLE_FAN,
        PrimitiveTopology::LineList => vk::PrimitiveTopology::LINE_LIST,
        PrimitiveTopology::LineStrip => vk::PrimitiveTopology::LINE_STRIP,
        PrimitiveTopology::PointList => vk::PrimitiveTopology::POINT_LIST,
        PrimitiveTopology::PatchList(_) => vk::PrimitiveTopology::PATCH_LIST,
        PrimitiveTopology::LineLoop | PrimitiveTopology::Unknown(_) => return None,
    })
}
pub fn compare(value: CompareOp) -> Option<vk::CompareOp> {
    Some(match value {
        CompareOp::Never => vk::CompareOp::NEVER,
        CompareOp::Less => vk::CompareOp::LESS,
        CompareOp::Equal => vk::CompareOp::EQUAL,
        CompareOp::LessOrEqual => vk::CompareOp::LESS_OR_EQUAL,
        CompareOp::Greater => vk::CompareOp::GREATER,
        CompareOp::NotEqual => vk::CompareOp::NOT_EQUAL,
        CompareOp::GreaterOrEqual => vk::CompareOp::GREATER_OR_EQUAL,
        CompareOp::Always => vk::CompareOp::ALWAYS,
        CompareOp::Unknown(_) => return None,
    })
}
pub fn stencil(value: StencilOp) -> Option<vk::StencilOp> {
    Some(match value {
        StencilOp::Keep => vk::StencilOp::KEEP,
        StencilOp::Zero => vk::StencilOp::ZERO,
        StencilOp::Replace => vk::StencilOp::REPLACE,
        StencilOp::IncrementClamp => vk::StencilOp::INCREMENT_AND_CLAMP,
        StencilOp::DecrementClamp => vk::StencilOp::DECREMENT_AND_CLAMP,
        StencilOp::Invert => vk::StencilOp::INVERT,
        StencilOp::IncrementWrap => vk::StencilOp::INCREMENT_AND_WRAP,
        StencilOp::DecrementWrap => vk::StencilOp::DECREMENT_AND_WRAP,
        StencilOp::Unknown(_) => return None,
    })
}
pub fn cull(value: CullMode) -> Option<vk::CullModeFlags> {
    Some(match value {
        CullMode::None => vk::CullModeFlags::NONE,
        CullMode::Front => vk::CullModeFlags::FRONT,
        CullMode::Back => vk::CullModeFlags::BACK,
        CullMode::FrontAndBack => vk::CullModeFlags::FRONT_AND_BACK,
        CullMode::Unknown(_) => return None,
    })
}
pub fn front_face(value: FrontFace) -> Option<vk::FrontFace> {
    Some(match value {
        FrontFace::Clockwise => vk::FrontFace::CLOCKWISE,
        FrontFace::CounterClockwise => vk::FrontFace::COUNTER_CLOCKWISE,
        FrontFace::Unknown(_) => return None,
    })
}
pub fn polygon(value: PolygonMode) -> Option<vk::PolygonMode> {
    Some(match value {
        PolygonMode::Fill => vk::PolygonMode::FILL,
        PolygonMode::Line => vk::PolygonMode::LINE,
        PolygonMode::Point => vk::PolygonMode::POINT,
        PolygonMode::Unknown(_) => return None,
    })
}
pub fn blend_factor(value: BlendFactor) -> Option<vk::BlendFactor> {
    Some(match value {
        BlendFactor::Zero => vk::BlendFactor::ZERO,
        BlendFactor::One => vk::BlendFactor::ONE,
        BlendFactor::SrcColor => vk::BlendFactor::SRC_COLOR,
        BlendFactor::OneMinusSrcColor => vk::BlendFactor::ONE_MINUS_SRC_COLOR,
        BlendFactor::DstColor => vk::BlendFactor::DST_COLOR,
        BlendFactor::OneMinusDstColor => vk::BlendFactor::ONE_MINUS_DST_COLOR,
        BlendFactor::SrcAlpha => vk::BlendFactor::SRC_ALPHA,
        BlendFactor::OneMinusSrcAlpha => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        BlendFactor::DstAlpha => vk::BlendFactor::DST_ALPHA,
        BlendFactor::OneMinusDstAlpha => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
        BlendFactor::ConstantColor => vk::BlendFactor::CONSTANT_COLOR,
        BlendFactor::OneMinusConstantColor => vk::BlendFactor::ONE_MINUS_CONSTANT_COLOR,
        BlendFactor::ConstantAlpha => vk::BlendFactor::CONSTANT_ALPHA,
        BlendFactor::OneMinusConstantAlpha => vk::BlendFactor::ONE_MINUS_CONSTANT_ALPHA,
        BlendFactor::SrcAlphaSaturate => vk::BlendFactor::SRC_ALPHA_SATURATE,
        BlendFactor::Unknown(_) => return None,
    })
}
pub fn blend_op(value: BlendOp) -> Option<vk::BlendOp> {
    Some(match value {
        BlendOp::Add => vk::BlendOp::ADD,
        BlendOp::Subtract => vk::BlendOp::SUBTRACT,
        BlendOp::ReverseSubtract => vk::BlendOp::REVERSE_SUBTRACT,
        BlendOp::Min => vk::BlendOp::MIN,
        BlendOp::Max => vk::BlendOp::MAX,
        BlendOp::Unknown(_) => return None,
    })
}

pub fn shader_stage(value: exosphere_gpu_ir::ShaderStage) -> vk::ShaderStageFlags {
    use exosphere_gpu_ir::ShaderStage::*;
    match value {
        Vertex => vk::ShaderStageFlags::VERTEX,
        TessControl => vk::ShaderStageFlags::TESSELLATION_CONTROL,
        TessEvaluation => vk::ShaderStageFlags::TESSELLATION_EVALUATION,
        Geometry => vk::ShaderStageFlags::GEOMETRY,
        Fragment => vk::ShaderStageFlags::FRAGMENT,
        Compute => vk::ShaderStageFlags::COMPUTE,
    }
}

pub fn stage_mask(value: exosphere_gpu_ir::ShaderStageMask) -> vk::ShaderStageFlags {
    let mut out = vk::ShaderStageFlags::empty();
    if value.0 & exosphere_gpu_ir::ShaderStageMask::VERTEX != 0 {
        out |= vk::ShaderStageFlags::VERTEX;
    }
    if value.0 & exosphere_gpu_ir::ShaderStageMask::TESS_CONTROL != 0 {
        out |= vk::ShaderStageFlags::TESSELLATION_CONTROL;
    }
    if value.0 & exosphere_gpu_ir::ShaderStageMask::TESS_EVAL != 0 {
        out |= vk::ShaderStageFlags::TESSELLATION_EVALUATION;
    }
    if value.0 & exosphere_gpu_ir::ShaderStageMask::GEOMETRY != 0 {
        out |= vk::ShaderStageFlags::GEOMETRY;
    }
    if value.0 & exosphere_gpu_ir::ShaderStageMask::FRAGMENT != 0 {
        out |= vk::ShaderStageFlags::FRAGMENT;
    }
    if value.0 & exosphere_gpu_ir::ShaderStageMask::COMPUTE != 0 {
        out |= vk::ShaderStageFlags::COMPUTE;
    }
    out
}

pub fn descriptor_type(value: exosphere_gpu_ir::DescriptorType) -> vk::DescriptorType {
    use exosphere_gpu_ir::DescriptorType::*;
    match value {
        UniformBuffer => vk::DescriptorType::UNIFORM_BUFFER,
        StorageBuffer => vk::DescriptorType::STORAGE_BUFFER,
        SampledImage => vk::DescriptorType::SAMPLED_IMAGE,
        StorageImage => vk::DescriptorType::STORAGE_IMAGE,
        Sampler => vk::DescriptorType::SAMPLER,
        CombinedImageSampler => vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
    }
}
