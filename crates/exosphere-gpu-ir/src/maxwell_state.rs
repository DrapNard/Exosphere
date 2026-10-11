// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Stateful MAXWELL_B graphics engine front-end.
//!
//! This layer records Switch-visible Maxwell state before backend lowering. Register indices follow
//! the public Maxwell B method table. Unknown writes remain lossless `OpaqueRegister` events.

use crate::engine::{EngineClass, EngineMethodWrite};
use std::collections::BTreeMap;

pub mod method {
    pub const SET_LINKED_TSC: u16 = 0x48d;
    pub const SET_BINDLESS_TEXTURE: u16 = 0x982;
    pub const SET_TEX_SAMPLER_POOL_A: u16 = 0x557;
    pub const SET_TEX_SAMPLER_POOL_B: u16 = 0x558;
    pub const SET_TEX_SAMPLER_POOL_MAXIMUM_INDEX: u16 = 0x559;
    pub const SET_TEX_HEADER_POOL_A: u16 = 0x55d;
    pub const SET_TEX_HEADER_POOL_B: u16 = 0x55e;
    pub const SET_TEX_HEADER_POOL_MAXIMUM_INDEX: u16 = 0x55f;
    pub const WAIT_FOR_IDLE: u16 = 0x44;
    pub const LOAD_MME_INSTRUCTION_RAM_POINTER: u16 = 0x45;
    pub const LOAD_MME_INSTRUCTION_RAM: u16 = 0x46;
    pub const LOAD_MME_START_ADDRESS_RAM_POINTER: u16 = 0x47;
    pub const LOAD_MME_START_ADDRESS_RAM: u16 = 0x48;
    pub const SET_MME_SHADOW_RAM_CONTROL: u16 = 0x49;
    pub const INCREMENT_SYNC_POINT: u16 = 0xB2;

    pub const COLOR_TARGET_BASE: u16 = 0x200;
    pub const COLOR_TARGET_STRIDE: u16 = 16;
    pub const COLOR_TARGET_COUNT: usize = 8;

    pub const VIEWPORT_BASE: u16 = 0x280;
    pub const VIEWPORT_STRIDE: u16 = 8;
    pub const VIEWPORT_COUNT: usize = 16;
    pub const SET_WINDOW_OFFSET_X: u16 = 0x37e;
    pub const SET_WINDOW_OFFSET_Y: u16 = 0x37f;
    pub const SET_WINDOW_ORIGIN: u16 = 0x4eb;
    pub const SET_VIEWPORT_SCALE_OFFSET: u16 = 0x64b;
    pub const SET_VIEWPORT_CLIP_CONTROL: u16 = 0x64f;
    pub const OGL_SET_CULL: u16 = 0x646;
    pub const OGL_SET_FRONT_FACE: u16 = 0x647;
    pub const OGL_SET_CULL_FACE: u16 = 0x648;
    pub const SET_FRONT_POLYGON_MODE: u16 = 0x36b;
    pub const SET_BACK_POLYGON_MODE: u16 = 0x36c;
    pub const SET_RASTER_ENABLE: u16 = 0xdf;
    pub const SET_POLY_OFFSET_POINT: u16 = 0x370;
    pub const SET_POLY_OFFSET_LINE: u16 = 0x371;
    pub const SET_POLY_OFFSET_FILL: u16 = 0x372;
    pub const SET_DEPTH_BIAS_CONTROL: u16 = 0x444;
    pub const SET_SLOPE_SCALE_DEPTH_BIAS: u16 = 0x55b;
    pub const SET_DEPTH_BIAS: u16 = 0x56f;
    pub const SET_DEPTH_BIAS_CLAMP: u16 = 0x61f;
    pub const SET_DEPTH_TEST: u16 = 0x4b3;
    pub const SET_DEPTH_WRITE: u16 = 0x4ba;
    pub const SET_DEPTH_FUNC: u16 = 0x4c3;
    pub const SET_STENCIL_TEST: u16 = 0x4e0;
    pub const SET_SINGLE_ROP_CONTROL: u16 = 0x4d7;
    pub const SET_BLEND_BASE: u16 = 0x4d8;
    pub const SET_LOGIC_OP: u16 = 0x671;
    pub const SET_CT_WRITE_BASE: u16 = 0x680;
    pub const SET_CT_SELECT: u16 = 0x487;
    pub const SET_CT_MRT_ENABLE: u16 = 0x3eb;
    pub const SET_PS_OUTPUT_SAMPLE_MASK_USAGE: u16 = 0xc0;
    pub const SET_SAMPLE_MASK: u16 = 0x3e9;
    pub const SET_COLOR_TARGET_SAMPLE_MASK: u16 = 0x3ea;
    pub const SET_ANTI_ALIAS_ENABLE: u16 = 0x54d;
    pub const SET_ANTI_ALIAS_ALPHA_CONTROL: u16 = 0x54f;
    pub const SET_ANTI_ALIAS: u16 = 0x574;
    pub const SET_ALPHA_TO_COVERAGE_OVERRIDE: u16 = 0x5ad;
    pub const SET_BLEND_STATE_PER_TARGET: u16 = 0x4b9;
    pub const SET_BLEND_COLOR_OP: u16 = 0x4d0;
    pub const SET_BLEND_COLOR_SOURCE_COEFF: u16 = 0x4d1;
    pub const SET_BLEND_COLOR_DEST_COEFF: u16 = 0x4d2;
    pub const SET_BLEND_ALPHA_OP: u16 = 0x4d3;
    pub const SET_BLEND_ALPHA_SOURCE_COEFF: u16 = 0x4d4;
    pub const SET_BLEND_ALPHA_DEST_COEFF: u16 = 0x4d6;

    pub const SCISSOR_BASE: u16 = 0x380;
    pub const SCISSOR_STRIDE: u16 = 4;

    pub const SET_VERTEX_ARRAY_START: u16 = 0x35D;
    pub const DRAW_VERTEX_ARRAY: u16 = 0x35E;
    pub const PIXEL_SHADER_BARRIER: u16 = 0x378;

    // Attribute banks A/B occupy adjacent 16-word ranges (byte offsets 0x1160/0x11a0).
    pub const VERTEX_ATTRIBUTE_BASE: u16 = 0x458;
    pub const VERTEX_ATTRIBUTE_COUNT: usize = 32;
    pub const VERTEX_STREAM_BASE: u16 = 0x700;
    pub const VERTEX_STREAM_STRIDE: u16 = 4;
    pub const VERTEX_STREAM_COUNT: usize = 32;
    pub const VERTEX_STREAM_LIMIT_BASE: u16 = 0x7c0;
    pub const VERTEX_STREAM_LIMIT_STRIDE: u16 = 2;

    pub const SET_PROGRAM_REGION_A: u16 = 0x582;
    pub const SET_PROGRAM_REGION_B: u16 = 0x583;
    pub const END: u16 = 0x585;
    pub const BEGIN: u16 = 0x586;
    pub const SET_INDEX_BUFFER_A: u16 = 0x5F2;
    pub const SET_INDEX_BUFFER_B: u16 = 0x5F3;
    pub const SET_INDEX_BUFFER_C: u16 = 0x5F4;
    pub const SET_INDEX_BUFFER_D: u16 = 0x5F5;
    pub const SET_INDEX_BUFFER_E: u16 = 0x5F6;
    pub const SET_INDEX_BUFFER_F: u16 = 0x5F7;
    pub const DRAW_INDEX_BUFFER: u16 = 0x5F8;
    pub const SET_PRIMITIVE_TOPOLOGY: u16 = 0x65C;
    pub const SET_PRIMITIVE_TOPOLOGY_CONTROL: u16 = 0x652;
    // Nouveau gf100_3d.xml byte offsets 0x1434, 0x1644, 0x1648 / 4.
    pub const SET_VERTEX_ELEMENT_BASE: u16 = 0x50d;
    pub const SET_VERTEX_ID_BASE: u16 = 0x446;
    pub const SET_VERTEX_ID_REPLACE: u16 = 0x587;
    pub const SET_VERTEX_ID_GEN_MODE: u16 = 0x593;
    pub const SET_PRIMITIVE_RESTART: u16 = 0x591;
    pub const SET_PRIMITIVE_RESTART_INDEX: u16 = 0x592;
    // VERTEX_ARRAY_PER_INSTANCE byte address 0x1880, one boolean per stream.
    pub const VERTEX_STREAM_PER_INSTANCE_BASE: u16 = 0x620;
    pub const BEGIN_INSTANCE_NEXT: u32 = 1 << 26;
    pub const BEGIN_INSTANCE_CONTINUE: u32 = 1 << 27;

    pub const PIPELINE_BASE: u16 = 0x800;
    pub const PIPELINE_STRIDE: u16 = 16;
    pub const PIPELINE_COUNT: usize = 6;

    pub const CONSTANT_BUFFER_SELECTOR_SIZE: u16 = 0x8e0;
    pub const CONSTANT_BUFFER_SELECTOR_HIGH: u16 = 0x8e1;
    pub const CONSTANT_BUFFER_SELECTOR_LOW: u16 = 0x8e2;
    pub const CONSTANT_BUFFER_UPLOAD_OFFSET: u16 = 0x8e3;
    pub const CONSTANT_BUFFER_UPLOAD_BASE: u16 = 0x8e4;
    pub const CONSTANT_BUFFER_BIND_BASE: u16 = 0x904;
    pub const CONSTANT_BUFFER_BIND_STRIDE: u16 = 8;
    pub const CONSTANT_BUFFER_GROUP_COUNT: usize = 5;

    pub const MACRO_BASE: u16 = 0xE00;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawGpuAddress {
    pub a: u32,
    pub b: u32,
}
impl RawGpuAddress {
    pub fn high_low(self) -> u64 {
        (u64::from(self.a) << 32) | u64::from(self.b)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MmeState {
    pub instruction_pointer: u32,
    pub instruction_ram: Vec<u32>,
    pub start_address_pointer: u32,
    pub start_addresses: Vec<u32>,
    pub shadow_ram_control: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroInvocation {
    pub index: u16,
    pub parameters: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ColorTargetState {
    pub address: RawGpuAddress,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub memory_layout: u32,
    pub layers: u32,
    pub array_pitch: u32,
    pub layer: u32,
}

impl ColorTargetState {
    /// Guest byte range of the selected supported block-linear array layer.
    /// Nouveau writes ARRAY_PITCH as layer_stride >> 2, so its unit is four
    /// bytes, not pixels or GOBs. This does not admit host memory writes.
    pub fn block_linear_layer_range(self) -> Option<(u64, u64)> {
        let length = self.block_linear_layer_layout()?.storage_size();
        if self.address.a & !0xff != 0 {
            return None;
        }
        let stride = u64::from(self.array_pitch) * 4;
        if self.layers > 1 && stride < length {
            return None;
        }
        let address = self
            .address
            .high_low()
            .checked_add(u64::from(self.layer).checked_mul(stride)?)?;
        if address.checked_add(length)? > (1u64 << 40) {
            return None;
        }
        Some((address, length))
    }

    /// Footprint of one 2D array layer in the supported block-linear layout.
    /// Layer placement/array pitch and live memory permissions are not resolved
    /// here. Pitch-linear and depth surfaces require separate interpretation.
    pub fn block_linear_layer_layout(self) -> Option<crate::block_linear::BlockLinearLayout> {
        // clb197.h MEMORY: block width 3:0, height 7:4, depth 11:8,
        // layout 12, third-dimension control 16. Only height varies for 2D.
        let block_height = (self.memory_layout >> 4) & 15;
        if self.memory_layout & !0xf0 != 0
            || block_height > 5
            || self.width == 0
            || self.width & !0x0fff_ffff != 0
            || self.height == 0
            || self.height & !0x1ffff != 0
            || self.layers == 0
            || self.layers & !0x0fff_ffff != 0
            || self.layer & !0xffff != 0
            || self.layer >= self.layers
        {
            return None;
        }
        let format = crate::texture_layout::format_info(self.canonical_format()?)?;
        let width_bytes = self.width.checked_mul(u32::from(format.bytes_per_block))?;
        Some(crate::block_linear::BlockLinearLayout::new(
            width_bytes,
            self.height,
            1,
            block_height as u8,
            0,
        ))
    }

    /// Exact supported NVB197 SET_COLOR_TARGET_FORMAT values. Disabled,
    /// reserved and unrepresented formats do not acquire a fallback texture.
    pub fn canonical_format(self) -> Option<crate::resource::TextureFormat> {
        use crate::resource::TextureFormat::*;
        Some(match self.format {
            0xc0 => Rgba32Float,
            0xca => Rgba16Float,
            0xcb => Rg32Float,
            0xd5 => Rgba8Unorm,
            0xde => Rg16Float,
            0xe3 => R32Sint,
            0xe4 => R32Uint,
            0xe5 => R32Float,
            0xf2 => R16Float,
            0xf3 => R8Unorm,
            0xf5 => R8Sint,
            0xf6 => R8Uint,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ViewportState {
    pub scale: [f32; 3],
    pub offset: [f32; 3],
    pub swizzle: u32,
    pub precision: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScissorState {
    pub enable: bool,
    pub horizontal: u32,
    pub vertical: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PipelineShaderState {
    pub select: u32,
    pub program: u32,
    pub register_count: u32,
    pub binding: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexBufferState {
    pub address: RawGpuAddress,
    pub limit: RawGpuAddress,
    /// NVB197 index-size word: 0/1/2 mean one/two/four bytes. Preserve
    /// unsupported and reserved bits for validation rather than masking them.
    pub index_size_raw: u32,
    pub first: u32,
}

impl IndexBufferState {
    /// Decode only documented index-size words, rejecting reserved bits.
    pub fn index_bytes(self) -> Option<u64> {
        match self.index_size_raw {
            0 => Some(1),
            1 => Some(2),
            2 => Some(4),
            _ => None,
        }
    }

    /// Address and byte count for a nonempty draw, before mapping validation.
    /// This does not admit the programmed limit or prove readable GPU memory;
    /// those checks belong to the resource-binding layer. Zero-count draws need
    /// no index fetch and deliberately return no range.
    pub fn index_read_range(self, count: u32) -> Option<(u64, u64)> {
        if count == 0 || self.address.a & !0xff != 0 {
            return None;
        }
        let width = self.index_bytes()?;
        let address = self
            .address
            .high_low()
            .checked_add(u64::from(self.first) * width)?;
        let bytes = u64::from(count) * width;
        if address.checked_add(bytes)? > 1 << 40 {
            return None;
        }
        Some((address, bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroFailure {
    MissingProgram,
    ExecutionStopped(crate::mme_exec::MmeStopReason),
    EmissionLimit,
    RecursiveEmission { method: u16 },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrawStateSnapshot {
    /// Exact register words, including viewport float bit patterns and registers
    /// whose typed interpretation is not yet implemented.
    pub raw_registers: BTreeMap<u16, u32>,
    pub primitive_raw: Option<u32>,
    /// Derived from ordered BEGIN commands, not from the latest raw word alone.
    pub instance_id: Option<u32>,
    pub index_buffer: IndexBufferState,
    pub constant_buffers: BTreeMap<(u8, u8), ConstantBufferBinding>,
}

/// Selector words captured at bind time, not read back from the current selector.
/// Preserve reserved bits so downstream validation cannot silently admit malformed commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantBufferBinding {
    pub size_raw: u32,
    pub address: RawGpuAddress,
    pub bind_raw: u32,
}

impl ConstantBufferBinding {
    /// Bound byte range only; this does not prove that GPU memory is readable.
    pub fn byte_range(self) -> Option<(u64, u64)> {
        if self.bind_raw & !0x1f1 != 0
            || self.bind_raw & 1 == 0
            || self.size_raw & !0x1ffff != 0
            || self.address.a & !0xff != 0
        {
            return None;
        }
        let size = u64::from(self.size_raw);
        let address = self.address.high_low();
        if size == 0 || address.checked_add(size)? > 1 << 40 {
            return None;
        }
        Some((address, size))
    }
}

/// Decoded fetch descriptor, not a Vulkan shader location or a validated format.
/// Unknown encodings remain available for explicit rejection during lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexAttributeState {
    pub raw: u32,
    pub stream: u8,
    pub inactive: bool,
    pub offset: u16,
    pub component_bit_widths: u8,
    pub numerical_type: u8,
    pub swap_r_and_b: bool,
    pub reserved_bits: u32,
}

impl VertexAttributeState {
    /// Size of an exactly supported fetch, before missing shader components are filled.
    pub fn canonical_byte_width(self) -> Option<u64> {
        self.canonical_format()?;
        Some(match self.component_bit_widths {
            0x1d => 1,
            0x18 | 0x1b => 2,
            0x0a | 0x0f | 0x12 => 4,
            0x03 | 0x04 => 8,
            0x02 => 12,
            0x01 => 16,
            _ => return None,
        })
    }

    /// Exact canonical equivalents only. Scaled, packed and channel-swapped
    /// fetches need additional backend support; inactive inputs need constants.
    pub fn canonical_format(self) -> Option<crate::resource::VertexFormat> {
        use crate::resource::VertexFormat::*;
        if self.inactive || self.reserved_bits != 0 || self.swap_r_and_b {
            return None;
        }
        Some(match (self.component_bit_widths, self.numerical_type) {
            (0x1d, 1) => R8Snorm,
            (0x1d, 2) => R8Unorm,
            (0x1d, 3) => R8Sint,
            (0x1d, 4) => R8Uint,
            (0x18, 1) => Rg8Snorm,
            (0x18, 2) => Rg8Unorm,
            (0x18, 3) => Rg8Sint,
            (0x18, 4) => Rg8Uint,
            (0x0a, 1) => Rgba8Snorm,
            (0x0a, 2) => Rgba8Unorm,
            (0x0a, 3) => Rgba8Sint,
            (0x0a, 4) => Rgba8Uint,
            (0x1b, 3) => R16Sint,
            (0x1b, 4) => R16Uint,
            (0x1b, 7) => R16Float,
            (0x0f, 3) => Rg16Sint,
            (0x0f, 4) => Rg16Uint,
            (0x0f, 7) => Rg16Float,
            (0x03, 3) => Rgba16Sint,
            (0x03, 4) => Rgba16Uint,
            (0x03, 7) => Rgba16Float,
            (0x12, 3) => R32Sint,
            (0x12, 4) => R32Uint,
            (0x12, 7) => R32Float,
            (0x04, 3) => Rg32Sint,
            (0x04, 4) => Rg32Uint,
            (0x04, 7) => Rg32Float,
            (0x02, 3) => Rgb32Sint,
            (0x02, 4) => Rgb32Uint,
            (0x02, 7) => Rgb32Float,
            (0x01, 3) => Rgba32Sint,
            (0x01, 4) => Rgba32Uint,
            (0x01, 7) => Rgba32Float,
            _ => return None,
        })
    }

    pub fn from_raw(raw: u32) -> Self {
        Self {
            raw,
            stream: (raw & 0x1f) as u8,
            inactive: raw & (1 << 6) != 0,
            offset: ((raw >> 7) & 0x3fff) as u16,
            component_bit_widths: ((raw >> 21) & 0x3f) as u8,
            numerical_type: ((raw >> 27) & 7) as u8,
            swap_r_and_b: raw & (1 << 31) != 0,
            reserved_bits: raw & ((1 << 5) | (1 << 30)),
        }
    }
}

impl DrawStateSnapshot {
    /// Raw pool configuration from this draw, not from subsequent engine writes.
    /// The maximum is an inclusive descriptor index, not a byte length. Address
    /// legality, descriptor layout and memory access are checked by consumers.
    /// Missing words remain unknown rather than acquiring guessed reset values.
    pub fn texture_descriptor_pool(&self, sampler: bool) -> Option<(RawGpuAddress, u32)> {
        let base = if sampler {
            method::SET_TEX_SAMPLER_POOL_A
        } else {
            method::SET_TEX_HEADER_POOL_A
        };
        Some((
            RawGpuAddress {
                a: *self.raw_registers.get(&base)?,
                b: *self.raw_registers.get(&(base + 1))?,
            },
            *self.raw_registers.get(&(base + 2))?,
        ))
    }
    pub fn draw_instance_id(&self) -> Option<u32> {
        let raw = self.primitive_raw?;
        let flags = raw & (method::BEGIN_INSTANCE_NEXT | method::BEGIN_INSTANCE_CONTINUE);
        if raw & !(0xffff | method::BEGIN_INSTANCE_NEXT | method::BEGIN_INSTANCE_CONTINUE) != 0
            || flags == (method::BEGIN_INSTANCE_NEXT | method::BEGIN_INSTANCE_CONTINUE)
        {
            return None;
        }
        if flags == 0 {
            Some(0)
        } else {
            self.instance_id
        }
    }
    /// Prepare one selected, single-sample color layer. Image identity is a
    /// caller-owned allocation, not the backing buffer's resource identity.
    pub fn prepare_color_target_layer(
        &self,
        index: usize,
        image: crate::ResourceId,
        samples: u8,
        address_space: &crate::GpuAddressSpace,
    ) -> Result<PreparedColorTarget, ColorTargetResolveError> {
        let invalid = ColorTargetResolveError::InvalidDescriptor;
        if samples != 1 {
            return Err(invalid);
        }
        let target = self.color_target(index).ok_or(invalid.clone())?;
        let layout = target.block_linear_layer_layout().ok_or(invalid.clone())?;
        let backing = self.resolve_color_target_layer(index, address_space)?;
        if image == backing.resource {
            return Err(invalid);
        }
        Ok(PreparedColorTarget {
            texture: crate::resource::TextureDesc {
                id: image,
                width: target.width,
                height: target.height,
                depth: 1,
                layers: 1,
                mip_levels: 1,
                samples,
                format: target.canonical_format().ok_or(invalid)?,
                layout: crate::resource::TextureLayout::BlockLinear {
                    block_height_log2: layout.block_height_log2,
                    block_depth_log2: 0,
                    raw: target.memory_layout,
                },
            },
            backing,
            layout,
        })
    }

    /// Resolve the selected supported color layer against live mappings. Captured
    /// registers do not freeze resource identities or grant write permissions.
    pub fn resolve_color_target_layer(
        &self,
        index: usize,
        address_space: &crate::GpuAddressSpace,
    ) -> Result<crate::address_space::ResolvedGpuRange, ColorTargetResolveError> {
        let (address, length) = self
            .color_target(index)
            .and_then(ColorTargetState::block_linear_layer_range)
            .ok_or(ColorTargetResolveError::InvalidDescriptor)?;
        let range = address_space
            .resolve(address, length)
            .map_err(ColorTargetResolveError::Address)?;
        if !range.permissions.write {
            return Err(ColorTargetResolveError::NotWritable);
        }
        Ok(range)
    }

    /// Read the complete raw color descriptor captured for this draw. Missing
    /// words are not hardware defaults. Layout/format validation remains a
    /// separate step before allocating or writing a render target.
    pub fn color_target(&self, index: usize) -> Option<ColorTargetState> {
        if index >= method::COLOR_TARGET_COUNT {
            return None;
        }
        // NVIDIA clb197.h: byte methods 0x0800..0x0820, 64-byte target stride.
        let base = method::COLOR_TARGET_BASE + index as u16 * method::COLOR_TARGET_STRIDE;
        let word = |field| self.raw_registers.get(&(base + field)).copied();
        Some(ColorTargetState {
            address: RawGpuAddress {
                a: word(0)?,
                b: word(1)?,
            },
            width: word(2)?,
            height: word(3)?,
            format: word(4)?,
            memory_layout: word(5)?,
            layers: word(6)?,
            array_pitch: word(7)?,
            layer: word(8)?,
        })
    }

    /// Prepare an existing canonical uniform descriptor. The caller supplies the
    /// shader's descriptor mapping; hardware groups/slots are not Vulkan bindings.
    pub fn prepare_constant_buffer_descriptor(
        &self,
        group: u8,
        slot: u8,
        descriptor: crate::resource::DescriptorBindingDesc,
        array_element: u32,
        address_space: &crate::address_space::GpuAddressSpace,
    ) -> Result<crate::command::Command, ConstantBufferReadError> {
        if descriptor.descriptor_type != crate::resource::DescriptorType::UniformBuffer
            || array_element >= descriptor.count
        {
            return Err(ConstantBufferReadError::InvalidDescriptor);
        }
        let (_, size) = self
            .constant_buffers
            .get(&(group, slot))
            .and_then(|binding| binding.byte_range())
            .ok_or(ConstantBufferReadError::InvalidBinding)?;
        // Bind the declared buffer, not merely the bytes read by one instruction.
        // Device alignment and range limits are validated by the backend.
        let range = self.resolve_constant_buffer_read(group, slot, 0, size, address_space)?;
        Ok(crate::command::Command::BindBufferDescriptor {
            set: descriptor.set,
            binding: descriptor.binding,
            array_element,
            buffer: range.resource,
            offset: range.offset,
            range: range.length,
        })
    }
    /// Resolve only the requested bytes against current mappings. The descriptor
    /// is frozen by the draw, but memory identity and permissions remain live.
    pub fn resolve_constant_buffer_read(
        &self,
        group: u8,
        slot: u8,
        offset: u64,
        length: u64,
        address_space: &crate::address_space::GpuAddressSpace,
    ) -> Result<crate::address_space::ResolvedGpuRange, ConstantBufferReadError> {
        if usize::from(group) >= method::CONSTANT_BUFFER_GROUP_COUNT || slot >= 32 {
            return Err(ConstantBufferReadError::InvalidBinding);
        }
        let (address, size) = self
            .constant_buffers
            .get(&(group, slot))
            .and_then(|binding| binding.byte_range())
            .ok_or(ConstantBufferReadError::InvalidBinding)?;
        if length == 0 || offset.checked_add(length).is_none_or(|end| end > size) {
            return Err(ConstantBufferReadError::OutOfBounds);
        }
        let range = address_space
            .resolve(address + offset, length)
            .map_err(ConstantBufferReadError::Address)?;
        if !range.permissions.read {
            return Err(ConstantBufferReadError::NotReadable);
        }
        Ok(range)
    }
    /// Prepare one canonical input with a caller-verified shader location and
    /// input rate. Neither application location nor instancing is inferred from
    /// the Maxwell attribute slot/frequency word.
    pub fn prepare_vertex_input(
        &self,
        attribute_index: usize,
        location: u32,
        input_rate: crate::resource::VertexInputRate,
        address_space: &crate::address_space::GpuAddressSpace,
    ) -> Result<PreparedVertexInput, VertexStreamResolveError> {
        let attribute = self.vertex_attribute(attribute_index).ok_or(
            VertexStreamResolveError::InvalidAttribute {
                index: attribute_index,
            },
        )?;
        let format =
            attribute
                .canonical_format()
                .ok_or(VertexStreamResolveError::InvalidAttribute {
                    index: attribute_index,
                })?;
        let stream_index = usize::from(attribute.stream);
        let stream = self.vertex_stream(stream_index).ok_or(
            VertexStreamResolveError::InvalidDescriptor {
                index: stream_index,
            },
        )?;
        let range = self.resolve_vertex_stream(stream_index, address_space)?;
        let binding = u32::from(attribute.stream);
        Ok(PreparedVertexInput {
            binding: crate::resource::VertexBindingDesc {
                binding,
                stride: u32::from(stream.stride()),
                input_rate,
            },
            attribute: crate::resource::VertexAttributeDesc {
                location,
                binding,
                format,
                offset: u32::from(attribute.offset),
            },
            range,
        })
    }

    /// Pair a shader's explicit input type with the live stream descriptor.
    /// The hardware slot is not assumed to be the shader location.
    pub fn prepare_shader_vertex_input(
        &self,
        interface: crate::maxwell_vertex::VertexAttributeInterface,
        input_rate: crate::resource::VertexInputRate,
        address_space: &crate::address_space::GpuAddressSpace,
    ) -> Result<PreparedVertexInput, VertexStreamResolveError> {
        let index = usize::from(interface.slot);
        if interface.slot >= 32 || interface.location >= 32 {
            return Err(VertexStreamResolveError::InvalidAttribute { index });
        }
        let prepared =
            self.prepare_vertex_input(index, interface.location, input_rate, address_space)?;
        if prepared.attribute.format != interface.format {
            return Err(VertexStreamResolveError::InvalidAttribute { index });
        }
        Ok(prepared)
    }

    /// Read the explicit stream rate, separately from its divisor word.
    /// A missing control is not evidence of a per-vertex hardware reset state.
    pub fn captured_vertex_input_rate(
        &self,
        interface: crate::maxwell_vertex::VertexAttributeInterface,
    ) -> Result<crate::resource::VertexInputRate, VertexStreamResolveError> {
        let index = usize::from(interface.slot);
        let attribute = self
            .vertex_attribute(index)
            .ok_or(VertexStreamResolveError::InvalidAttribute { index })?;
        let stream = usize::from(attribute.stream);
        match self
            .raw_registers
            .get(&(method::VERTEX_STREAM_PER_INSTANCE_BASE + stream as u16))
        {
            Some(0) => Ok(crate::resource::VertexInputRate::Vertex),
            Some(1)
                if self
                    .vertex_stream(stream)
                    .is_some_and(|state| state.frequency() != 0) =>
            {
                Ok(crate::resource::VertexInputRate::Instance)
            }
            _ => Err(VertexStreamResolveError::InvalidDescriptor { index: stream }),
        }
    }

    /// Validate a contiguous range of already selected stream elements before
    /// submission. Indexed draws and instance divisors must resolve their element
    /// selection separately; this does not infer it from a vertex count.
    pub fn prepare_shader_vertex_range(
        &self,
        interface: crate::maxwell_vertex::VertexAttributeInterface,
        input_rate: crate::resource::VertexInputRate,
        first_element: u64,
        element_count: u64,
        address_space: &crate::address_space::GpuAddressSpace,
    ) -> Result<PreparedVertexInput, VertexStreamResolveError> {
        let prepared = self.prepare_shader_vertex_input(interface, input_rate, address_space)?;
        if element_count != 0 {
            let index = usize::from(interface.slot);
            let last = first_element.checked_add(element_count - 1).ok_or(
                VertexStreamResolveError::ElementOutOfBounds {
                    attribute_index: index,
                    element_index: first_element,
                },
            )?;
            // Unsigned stride makes offsets monotonic, including zero stride.
            // Checking endpoints therefore bounds every intermediate fetch.
            self.resolve_vertex_element(index, first_element, address_space)?;
            self.resolve_vertex_element(index, last, address_space)?;
        }
        Ok(prepared)
    }

    /// Select one draw's instance element and rebase its native binding. This
    /// models fetches, not shader InstanceID builtins. A one-instance draw can
    /// express any nonzero guest divisor without a native divisor extension.
    pub fn prepare_draw_vertex_range(
        &self,
        interface: crate::maxwell_vertex::VertexAttributeInterface,
        first_vertex: u64,
        vertex_count: u64,
        address_space: &crate::address_space::GpuAddressSpace,
    ) -> Result<PreparedVertexInput, VertexStreamResolveError> {
        let rate = self.captured_vertex_input_rate(interface)?;
        if rate == crate::resource::VertexInputRate::Vertex {
            return self.prepare_shader_vertex_range(
                interface,
                rate,
                first_vertex,
                vertex_count,
                address_space,
            );
        }
        let index = usize::from(interface.slot);
        let attribute = self
            .vertex_attribute(index)
            .ok_or(VertexStreamResolveError::InvalidAttribute { index })?;
        let stream_index = usize::from(attribute.stream);
        let invalid = || VertexStreamResolveError::InvalidDescriptor {
            index: stream_index,
        };
        let stream = self.vertex_stream(stream_index).ok_or_else(invalid)?;
        let instance = self.draw_instance_id().ok_or_else(invalid)?;
        let element = u64::from(instance / stream.frequency());
        let mut prepared = self.prepare_shader_vertex_range(
            interface,
            rate,
            element,
            u64::from(vertex_count != 0),
            address_space,
        )?;
        if vertex_count != 0 {
            let shift = element
                .checked_mul(u64::from(stream.stride()))
                .ok_or_else(invalid)?;
            prepared.range.offset = prepared
                .range
                .offset
                .checked_add(shift)
                .ok_or_else(invalid)?;
            prepared.range.length = prepared
                .range
                .length
                .checked_sub(shift)
                .ok_or_else(invalid)?;
        }
        Ok(prepared)
    }

    /// Resolve one attribute fetch. The caller supplies the already selected
    /// stream element (vertex/instance selection is deliberately not inferred).
    /// Zero stride repeats the same element; offsets may legally exceed stride.
    pub fn resolve_vertex_element(
        &self,
        attribute_index: usize,
        element_index: u64,
        address_space: &crate::address_space::GpuAddressSpace,
    ) -> Result<crate::address_space::ResolvedGpuRange, VertexStreamResolveError> {
        let attribute = self.vertex_attribute(attribute_index).ok_or(
            VertexStreamResolveError::InvalidAttribute {
                index: attribute_index,
            },
        )?;
        let width =
            attribute
                .canonical_byte_width()
                .ok_or(VertexStreamResolveError::InvalidAttribute {
                    index: attribute_index,
                })?;
        let stream_index = usize::from(attribute.stream);
        let stream = self.vertex_stream(stream_index).ok_or(
            VertexStreamResolveError::InvalidDescriptor {
                index: stream_index,
            },
        )?;
        let mut range = self.resolve_vertex_stream(stream_index, address_space)?;
        let offset = element_index
            .checked_mul(u64::from(stream.stride()))
            .and_then(|value| value.checked_add(u64::from(attribute.offset)))
            .ok_or(VertexStreamResolveError::ElementOutOfBounds {
                attribute_index,
                element_index,
            })?;
        if offset
            .checked_add(width)
            .is_none_or(|end| end > range.length)
        {
            return Err(VertexStreamResolveError::ElementOutOfBounds {
                attribute_index,
                element_index,
            });
        }
        // Full stream mapping validation guarantees this subrange offset cannot overflow.
        range.offset += offset;
        range.length = width;
        Ok(range)
    }

    /// Resolve against current mappings: a snapshot freezes registers, not the
    /// address space. Remaps must select their new resource, not a cached identity.
    pub fn resolve_vertex_stream(
        &self,
        index: usize,
        address_space: &crate::address_space::GpuAddressSpace,
    ) -> Result<crate::address_space::ResolvedGpuRange, VertexStreamResolveError> {
        let (address, length) = self
            .vertex_stream_range(index)
            .ok_or(VertexStreamResolveError::InvalidDescriptor { index })?;
        let range = address_space
            .resolve(address, length)
            .map_err(VertexStreamResolveError::Address)?;
        if !range.permissions.read {
            return Err(VertexStreamResolveError::NotReadable { index });
        }
        Ok(range)
    }

    /// Last permitted byte address, as programmed independently of stream base.
    pub fn vertex_stream_limit(&self, index: usize) -> Option<RawGpuAddress> {
        if index >= method::VERTEX_STREAM_COUNT {
            return None;
        }
        let base =
            method::VERTEX_STREAM_LIMIT_BASE + index as u16 * method::VERTEX_STREAM_LIMIT_STRIDE;
        Some(RawGpuAddress {
            a: *self.raw_registers.get(&base)?,
            b: *self.raw_registers.get(&(base + 1))?,
        })
    }

    /// Known enabled stream's bounded address/length, before mapping resolution.
    /// This does not assert that memory is mapped, readable or valid for a draw.
    pub fn vertex_stream_range(&self, index: usize) -> Option<(u64, u64)> {
        let stream = self.vertex_stream(index)?;
        if !stream.enabled() || stream.reserved_format_bits() != 0 {
            return None;
        }
        let start = stream.address()?;
        let limit = self.vertex_stream_limit(index)?;
        if limit.a > 0xff {
            return None;
        }
        let bytes = limit.high_low().checked_sub(start)?.checked_add(1)?;
        Some((start, bytes))
    }

    /// Both stream banks have the same four-word layout. Require explicit words
    /// rather than substituting undocumented hardware reset values.
    pub fn vertex_stream(&self, index: usize) -> Option<VertexStreamState> {
        if index >= method::VERTEX_STREAM_COUNT {
            return None;
        }
        let base = method::VERTEX_STREAM_BASE + index as u16 * method::VERTEX_STREAM_STRIDE;
        let mut raw = [0; 4];
        for (offset, word) in raw.iter_mut().enumerate() {
            *word = *self.raw_registers.get(&(base + offset as u16))?;
        }
        Some(VertexStreamState { raw })
    }

    /// Read from this draw's immutable mirror, never from later engine state.
    /// An unwritten register is unknown: it must not acquire an invented reset value.
    pub fn vertex_attribute(&self, index: usize) -> Option<VertexAttributeState> {
        if index >= method::VERTEX_ATTRIBUTE_COUNT {
            return None;
        }
        self.raw_registers
            .get(&(method::VERTEX_ATTRIBUTE_BASE + index as u16))
            .copied()
            .map(VertexAttributeState::from_raw)
    }
}

/// Descriptor preparation only; actual element bounds and shader type matching
/// must be validated for the draw before submitting any backend commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedVertexInput {
    pub binding: crate::resource::VertexBindingDesc,
    pub attribute: crate::resource::VertexAttributeDesc,
    pub range: crate::address_space::ResolvedGpuRange,
}

/// Assemble checked attribute streams into deterministic pipeline inputs and
/// buffer commands. Shared stream bindings must agree on their full descriptor.
pub fn merge_vertex_inputs(
    inputs: &[PreparedVertexInput],
) -> Result<(crate::resource::VertexInputState, Vec<crate::Command>), VertexStreamResolveError> {
    let mut streams = BTreeMap::new();
    let mut attributes = BTreeMap::new();
    for input in inputs {
        let index = input.attribute.location as usize;
        let width = input
            .attribute
            .format
            .byte_width()
            .ok_or(VertexStreamResolveError::InvalidAttribute { index })?;
        if input.attribute.location >= 32
            || input.binding.binding >= 32
            || input.attribute.binding != input.binding.binding
            || input.range.length == 0
            || !input.range.permissions.read
            || input.range.offset.checked_add(input.range.length).is_none()
            || u64::from(input.attribute.offset)
                .checked_add(width)
                .is_none_or(|end| end > input.range.length)
            || attributes
                .insert(input.attribute.location, input.attribute)
                .is_some()
        {
            return Err(VertexStreamResolveError::InvalidAttribute { index });
        }
        let descriptor = (input.binding, input.range);
        if streams
            .insert(input.binding.binding, descriptor)
            .is_some_and(|previous| previous != descriptor)
        {
            return Err(VertexStreamResolveError::InvalidDescriptor {
                index: input.binding.binding as usize,
            });
        }
    }
    let mut bindings = Vec::new();
    let mut commands = Vec::new();
    for (slot, (binding, range)) in streams {
        bindings.push(binding);
        commands.push(crate::Command::BindVertexBuffer {
            slot,
            buffer: range.resource,
            offset: range.offset,
        });
    }
    Ok((
        crate::resource::VertexInputState {
            bindings,
            attributes: attributes.into_values().collect(),
        },
        commands,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstantBufferReadError {
    InvalidDescriptor,
    InvalidBinding,
    OutOfBounds,
    Address(crate::address_space::GpuAddressError),
    NotReadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorTargetResolveError {
    InvalidDescriptor,
    Address(crate::address_space::GpuAddressError),
    NotWritable,
}

/// Image allocation metadata paired with the exact selected guest layer range.
/// Creating this value does not upload, render, write back or signal completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedColorTarget {
    pub texture: crate::resource::TextureDesc,
    pub backing: crate::address_space::ResolvedGpuRange,
    pub layout: crate::block_linear::BlockLinearLayout,
}

impl std::fmt::Display for ColorTargetResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDescriptor => {
                f.write_str("missing or unsupported color-target descriptor")
            }
            Self::Address(error) => write!(f, "color-target address: {error}"),
            Self::NotWritable => f.write_str("color-target memory is not writable"),
        }
    }
}

impl std::error::Error for ColorTargetResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Address(error) => Some(error),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConstantBufferReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDescriptor => f.write_str("invalid uniform-buffer descriptor mapping"),
            Self::InvalidBinding => f.write_str("missing or invalid constant-buffer binding"),
            Self::OutOfBounds => f.write_str("constant-buffer read exceeds bound size"),
            Self::Address(error) => write!(f, "constant-buffer address: {error}"),
            Self::NotReadable => f.write_str("constant-buffer memory is not readable"),
        }
    }
}

impl std::error::Error for ConstantBufferReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Address(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VertexStreamResolveError {
    InvalidAttribute {
        index: usize,
    },
    ElementOutOfBounds {
        attribute_index: usize,
        element_index: u64,
    },
    InvalidDescriptor {
        index: usize,
    },
    Address(crate::address_space::GpuAddressError),
    NotReadable {
        index: usize,
    },
}

impl std::fmt::Display for VertexStreamResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAttribute { index } => write!(f, "unsupported vertex attribute {index}"),
            Self::ElementOutOfBounds {
                attribute_index,
                element_index,
            } => write!(
                f,
                "vertex attribute {attribute_index} element {element_index} exceeds its stream"
            ),
            Self::InvalidDescriptor { index } => write!(f, "invalid vertex stream {index}"),
            Self::Address(error) => write!(f, "vertex stream address: {error}"),
            Self::NotReadable { index } => write!(f, "vertex stream {index} is not readable"),
        }
    }
}

impl std::error::Error for VertexStreamResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Address(error) => Some(error),
            _ => None,
        }
    }
}

/// Fetch stream state; frequency is retained without assuming an instance divisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexStreamState {
    pub raw: [u32; 4],
}

impl VertexStreamState {
    pub fn stride(self) -> u16 {
        (self.raw[0] & 0xfff) as u16
    }

    pub fn enabled(self) -> bool {
        self.raw[0] & (1 << 12) != 0
    }

    pub fn frequency(self) -> u32 {
        self.raw[3]
    }

    pub fn reserved_format_bits(self) -> u32 {
        self.raw[0] & !0x1fff
    }

    /// Address registers expose 40 bits. Reject reserved upper bits instead of
    /// masking them into a different, apparently valid memory access.
    pub fn address(self) -> Option<u64> {
        (self.raw[1] <= 0xff).then_some((u64::from(self.raw[1]) << 32) | u64::from(self.raw[2]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaxwellEvent {
    ConstantBufferStateChanged,
    ConstantBufferUpload {
        address: Option<u64>,
        word: u32,
    },
    ProgramRegionChanged(RawGpuAddress),
    Begin {
        primitive_raw: u32,
    },
    End,
    DrawVertexArray {
        first: u32,
        count: u32,
        state: Box<DrawStateSnapshot>,
    },
    DrawIndexBuffer {
        raw: u32,
        state: Box<DrawStateSnapshot>,
    },
    ColorTargetChanged {
        index: u8,
    },
    PipelineShaderChanged {
        slot: u8,
    },
    Barrier,
    WaitForIdle,
    SyncPointIncrement {
        payload: u32,
    },
    MmeInstructionWritten {
        index: u32,
        value: u32,
    },
    MmeStartAddressWritten {
        index: u32,
        value: u32,
    },
    MacroStarted {
        index: u16,
        first_parameter: u32,
    },
    MacroParameter {
        index: u16,
        parameter: u32,
    },
    MacroFailed {
        index: u16,
        reason: MacroFailure,
    },
    OpaqueRegister {
        method: u16,
        argument: u32,
    },
}

#[derive(Debug, Clone)]
pub struct Maxwell3dState {
    /// Lossless mirror of every method write seen by MAXWELL_B. Typed fields below are promoted views.
    pub raw_registers: BTreeMap<u16, u32>,
    pub program_region: RawGpuAddress,
    pub mme: MmeState,
    pub active_macro: Option<MacroInvocation>,
    pub primitive_raw: Option<u32>,
    pub instance_id: Option<u32>,
    pub primitive_topology_raw: u32,
    pub vertex_array_start: u32,
    pub color_targets: [ColorTargetState; method::COLOR_TARGET_COUNT],
    pub viewports: [ViewportState; method::VIEWPORT_COUNT],
    pub scissors: [ScissorState; method::VIEWPORT_COUNT],
    pub pipeline_shaders: [PipelineShaderState; method::PIPELINE_COUNT],
    pub index_buffer: IndexBufferState,
    pub constant_buffers: BTreeMap<(u8, u8), ConstantBufferBinding>,
}

impl Default for Maxwell3dState {
    fn default() -> Self {
        Self {
            raw_registers: BTreeMap::new(),
            program_region: RawGpuAddress::default(),
            mme: MmeState::default(),
            active_macro: None,
            primitive_raw: None,
            instance_id: None,
            primitive_topology_raw: 0,
            vertex_array_start: 0,
            color_targets: [ColorTargetState::default(); method::COLOR_TARGET_COUNT],
            viewports: [ViewportState::default(); method::VIEWPORT_COUNT],
            scissors: [ScissorState::default(); method::VIEWPORT_COUNT],
            pipeline_shaders: [PipelineShaderState::default(); method::PIPELINE_COUNT],
            index_buffer: IndexBufferState::default(),
            constant_buffers: BTreeMap::new(),
        }
    }
}

impl Maxwell3dState {
    /// Address for the next upload word, before memory permissions are checked.
    /// Uploads use the current selector, not a previously bound shader slot.
    /// This accessor does not perform the write or advance the upload cursor.
    pub fn constant_buffer_upload_address(&self) -> Option<u64> {
        let word = |method| self.raw_registers.get(&method).copied();
        let selector = ConstantBufferBinding {
            size_raw: word(method::CONSTANT_BUFFER_SELECTOR_SIZE)?,
            address: RawGpuAddress {
                a: word(method::CONSTANT_BUFFER_SELECTOR_HIGH)?,
                b: word(method::CONSTANT_BUFFER_SELECTOR_LOW)?,
            },
            bind_raw: 1,
        };
        let (address, size) = selector.byte_range()?;
        let offset = word(method::CONSTANT_BUFFER_UPLOAD_OFFSET)?;
        if offset & !0xffff != 0 || u64::from(offset).checked_add(4)? > size {
            return None;
        }
        address.checked_add(u64::from(offset))
    }
    pub fn draw_state(&self) -> DrawStateSnapshot {
        DrawStateSnapshot {
            raw_registers: self.raw_registers.clone(),
            primitive_raw: self.primitive_raw,
            instance_id: self.instance_id,
            index_buffer: self.index_buffer,
            constant_buffers: self.constant_buffers.clone(),
        }
    }
    /// Maxwell macro methods occupy paired method slots starting at MACRO_BASE.
    pub fn is_macro_method(method: u16) -> bool {
        method >= method::MACRO_BASE
    }

    /// Execute the currently accumulated macro invocation and replay its emitted PGRAPH methods.
    /// The caller chooses the invocation boundary because Maxwell macro parameter streams do not
    /// carry an intrinsic end marker. Execution and emission limits make malformed guest macros safe.
    pub fn execute_active_macro(
        &mut self,
        max_words: usize,
        step_limit: usize,
        emission_limit: usize,
    ) -> Vec<MaxwellEvent> {
        let Some(invocation) = self.active_macro.take() else {
            return Vec::new();
        };
        let Some(program) = crate::mme::extract_program(&self.mme, invocation.index, max_words)
        else {
            return vec![MaxwellEvent::MacroFailed {
                index: invocation.index,
                reason: MacroFailure::MissingProgram,
            }];
        };
        let regs = self.raw_registers.clone();
        let result = crate::mme_exec::execute_with_method_reader(
            &program,
            &invocation.parameters,
            step_limit,
            |method| regs.get(&method).copied().unwrap_or(0),
        );
        let mut events = Vec::new();
        let emission_count = result.emissions.len();
        for emission in result.emissions.into_iter().take(emission_limit) {
            if emission.method >= method::MACRO_BASE {
                events.push(MaxwellEvent::MacroFailed {
                    index: invocation.index,
                    reason: MacroFailure::RecursiveEmission {
                        method: emission.method,
                    },
                });
                return events;
            }
            if let Some(event) = self.apply(EngineMethodWrite {
                subchannel: 0,
                class: EngineClass::Graphics,
                method: emission.method,
                argument: emission.argument,
            }) {
                events.push(event);
            }
        }
        // Keep the valid prefix for diagnostics, but never disguise a truncated
        // or unsupported macro as a completed invocation.
        let failure = if emission_count > emission_limit {
            Some(MacroFailure::EmissionLimit)
        } else if result.stop_reason != crate::mme_exec::MmeStopReason::Exit {
            Some(MacroFailure::ExecutionStopped(result.stop_reason))
        } else {
            None
        };
        if let Some(reason) = failure {
            events.push(MaxwellEvent::MacroFailed {
                index: invocation.index,
                reason,
            });
        }
        events
    }

    pub fn apply(&mut self, write: EngineMethodWrite) -> Option<MaxwellEvent> {
        if write.class != EngineClass::Graphics {
            return None;
        }
        use method::*;
        self.raw_registers.insert(write.method, write.argument);
        // These methods only update captured fixed-function state. Their values
        // are validated by draw preparation; they are not standalone GPU work.
        // Do not include barriers, memory operations or unknown/reserved methods.
        if matches!(
            write.method,
            SET_WINDOW_OFFSET_X
                | SET_LINKED_TSC
                | SET_BINDLESS_TEXTURE
                | SET_TEX_SAMPLER_POOL_A
                | SET_TEX_SAMPLER_POOL_B
                | SET_TEX_SAMPLER_POOL_MAXIMUM_INDEX
                | SET_TEX_HEADER_POOL_A
                | SET_TEX_HEADER_POOL_B
                | SET_TEX_HEADER_POOL_MAXIMUM_INDEX
                | SET_WINDOW_OFFSET_Y
                | SET_WINDOW_ORIGIN
                | SET_VIEWPORT_SCALE_OFFSET
                | SET_VIEWPORT_CLIP_CONTROL
                | OGL_SET_CULL
                | OGL_SET_FRONT_FACE
                | OGL_SET_CULL_FACE
                | SET_FRONT_POLYGON_MODE
                | SET_BACK_POLYGON_MODE
                | SET_RASTER_ENABLE
                | SET_POLY_OFFSET_POINT
                | SET_POLY_OFFSET_LINE
                | SET_POLY_OFFSET_FILL
                | SET_DEPTH_BIAS_CONTROL
                | SET_SLOPE_SCALE_DEPTH_BIAS
                | SET_DEPTH_BIAS
                | SET_DEPTH_BIAS_CLAMP
                | SET_DEPTH_TEST
                | SET_DEPTH_WRITE
                | SET_DEPTH_FUNC
                | SET_STENCIL_TEST
                | SET_PRIMITIVE_TOPOLOGY_CONTROL
                | SET_VERTEX_ELEMENT_BASE
                | SET_VERTEX_ID_BASE
                | SET_VERTEX_ID_REPLACE
                | SET_VERTEX_ID_GEN_MODE
                | SET_PRIMITIVE_RESTART
                | SET_PRIMITIVE_RESTART_INDEX
        ) || (VERTEX_ATTRIBUTE_BASE..VERTEX_ATTRIBUTE_BASE + VERTEX_ATTRIBUTE_COUNT as u16)
            .contains(&write.method)
            || (VERTEX_STREAM_BASE
                ..VERTEX_STREAM_BASE + VERTEX_STREAM_STRIDE * VERTEX_STREAM_COUNT as u16)
                .contains(&write.method)
            || (VERTEX_STREAM_LIMIT_BASE
                ..VERTEX_STREAM_LIMIT_BASE
                    + VERTEX_STREAM_LIMIT_STRIDE * VERTEX_STREAM_COUNT as u16)
                .contains(&write.method)
            || (VERTEX_STREAM_PER_INSTANCE_BASE
                ..VERTEX_STREAM_PER_INSTANCE_BASE + VERTEX_STREAM_COUNT as u16)
                .contains(&write.method)
        {
            return None;
        }
        if matches!(
            write.method,
            SET_PS_OUTPUT_SAMPLE_MASK_USAGE
                | SET_SAMPLE_MASK
                | SET_COLOR_TARGET_SAMPLE_MASK
                | SET_ANTI_ALIAS_ENABLE
                | SET_ANTI_ALIAS_ALPHA_CONTROL
                | SET_ANTI_ALIAS
                | SET_ALPHA_TO_COVERAGE_OVERRIDE
        ) {
            return None;
        }
        if matches!(
            write.method,
            SET_BLEND_STATE_PER_TARGET
                | SET_BLEND_COLOR_OP
                | SET_BLEND_COLOR_SOURCE_COEFF
                | SET_BLEND_COLOR_DEST_COEFF
                | SET_BLEND_ALPHA_OP
                | SET_BLEND_ALPHA_SOURCE_COEFF
                | SET_BLEND_ALPHA_DEST_COEFF
        ) {
            return None;
        }
        if (SET_BLEND_BASE..SET_BLEND_BASE + 8).contains(&write.method)
            || (SET_CT_WRITE_BASE..SET_CT_WRITE_BASE + 8).contains(&write.method)
            || matches!(
                write.method,
                SET_SINGLE_ROP_CONTROL | SET_LOGIC_OP | SET_CT_SELECT | SET_CT_MRT_ENABLE
            )
        {
            return None;
        }
        if (CONSTANT_BUFFER_SELECTOR_SIZE..=CONSTANT_BUFFER_UPLOAD_OFFSET).contains(&write.method) {
            return Some(MaxwellEvent::ConstantBufferStateChanged);
        }
        if write.method == CONSTANT_BUFFER_UPLOAD_BASE {
            let address = self
                .constant_buffer_upload_address()
                .filter(|address| address & 3 == 0);
            if address.is_some() {
                // Capture each destination in the trace: later selector writes
                // must not retarget already decoded upload words.
                let offset = self.raw_registers[&CONSTANT_BUFFER_UPLOAD_OFFSET];
                self.raw_registers
                    .insert(CONSTANT_BUFFER_UPLOAD_OFFSET, offset + 4);
            }
            return Some(MaxwellEvent::ConstantBufferUpload {
                address,
                word: write.argument,
            });
        }

        if let Some((group, 0)) = ranged_method(
            write.method,
            CONSTANT_BUFFER_BIND_BASE,
            CONSTANT_BUFFER_BIND_STRIDE,
            CONSTANT_BUFFER_GROUP_COUNT,
        ) {
            let slot = ((write.argument >> 4) & 0x1f) as u8;
            // Every bind replaces this slot, including invalid/disabled binds. Keeping
            // the exact word prevents an invalid command from exposing a stale buffer.
            let register = |method| self.raw_registers.get(&method).copied().unwrap_or(0);
            self.constant_buffers.insert(
                (group as u8, slot),
                ConstantBufferBinding {
                    size_raw: register(CONSTANT_BUFFER_SELECTOR_SIZE),
                    address: RawGpuAddress {
                        a: register(CONSTANT_BUFFER_SELECTOR_HIGH),
                        b: register(CONSTANT_BUFFER_SELECTOR_LOW),
                    },
                    bind_raw: write.argument,
                },
            );
            // State capture alone does not execute uploads or admit a draw.
            return Some(MaxwellEvent::ConstantBufferStateChanged);
        }

        if let Some((index, field)) = ranged_method(
            write.method,
            COLOR_TARGET_BASE,
            COLOR_TARGET_STRIDE,
            COLOR_TARGET_COUNT,
        ) {
            let target = &mut self.color_targets[index];
            match field {
                0 => target.address.a = write.argument,
                1 => target.address.b = write.argument,
                2 => target.width = write.argument,
                3 => target.height = write.argument,
                4 => target.format = write.argument,
                5 => target.memory_layout = write.argument,
                6 => target.layers = write.argument,
                7 => target.array_pitch = write.argument,
                8 => target.layer = write.argument,
                _ => {
                    return Some(MaxwellEvent::OpaqueRegister {
                        method: write.method,
                        argument: write.argument,
                    });
                }
            }
            return Some(MaxwellEvent::ColorTargetChanged { index: index as u8 });
        }
        if let Some((index, field)) =
            ranged_method(write.method, VIEWPORT_BASE, VIEWPORT_STRIDE, VIEWPORT_COUNT)
        {
            let vp = &mut self.viewports[index];
            match field {
                0..=2 => vp.scale[field] = f32::from_bits(write.argument),
                3..=5 => vp.offset[field - 3] = f32::from_bits(write.argument),
                6 => vp.swizzle = write.argument,
                7 => vp.precision = write.argument,
                _ => unreachable!(),
            }
            return None;
        }
        if let Some((index, field)) =
            ranged_method(write.method, SCISSOR_BASE, SCISSOR_STRIDE, VIEWPORT_COUNT)
        {
            let sc = &mut self.scissors[index];
            match field {
                0 => sc.enable = write.argument != 0,
                1 => sc.horizontal = write.argument,
                2 => sc.vertical = write.argument,
                _ => {
                    return Some(MaxwellEvent::OpaqueRegister {
                        method: write.method,
                        argument: write.argument,
                    });
                }
            }
            return None;
        }
        if let Some((index, field)) =
            ranged_method(write.method, PIPELINE_BASE, PIPELINE_STRIDE, PIPELINE_COUNT)
        {
            let p = &mut self.pipeline_shaders[index];
            match field {
                0 => p.select = write.argument,
                1 => p.program = write.argument,
                3 => p.register_count = write.argument,
                4 => p.binding = write.argument,
                _ => {}
            }
            return Some(MaxwellEvent::PipelineShaderChanged { slot: index as u8 });
        }

        let event = match write.method {
            WAIT_FOR_IDLE => MaxwellEvent::WaitForIdle,
            INCREMENT_SYNC_POINT => MaxwellEvent::SyncPointIncrement {
                payload: write.argument,
            },
            LOAD_MME_INSTRUCTION_RAM_POINTER => {
                self.mme.instruction_pointer = write.argument;
                return None;
            }
            LOAD_MME_INSTRUCTION_RAM => {
                let index = self.mme.instruction_pointer;
                write_indexed(&mut self.mme.instruction_ram, index, write.argument);
                self.mme.instruction_pointer = index.wrapping_add(1);
                MaxwellEvent::MmeInstructionWritten {
                    index,
                    value: write.argument,
                }
            }
            LOAD_MME_START_ADDRESS_RAM_POINTER => {
                self.mme.start_address_pointer = write.argument;
                return None;
            }
            LOAD_MME_START_ADDRESS_RAM => {
                let index = self.mme.start_address_pointer;
                write_indexed(&mut self.mme.start_addresses, index, write.argument);
                self.mme.start_address_pointer = index.wrapping_add(1);
                MaxwellEvent::MmeStartAddressWritten {
                    index,
                    value: write.argument,
                }
            }
            SET_MME_SHADOW_RAM_CONTROL => {
                self.mme.shadow_ram_control = write.argument;
                return None;
            }
            SET_VERTEX_ARRAY_START => {
                self.vertex_array_start = write.argument;
                return None;
            }
            DRAW_VERTEX_ARRAY => MaxwellEvent::DrawVertexArray {
                first: self.vertex_array_start,
                count: write.argument,
                state: Box::new(self.draw_state()),
            },
            PIXEL_SHADER_BARRIER => MaxwellEvent::Barrier,
            SET_PROGRAM_REGION_A => {
                self.program_region.a = write.argument;
                MaxwellEvent::ProgramRegionChanged(self.program_region)
            }
            SET_PROGRAM_REGION_B => {
                self.program_region.b = write.argument;
                MaxwellEvent::ProgramRegionChanged(self.program_region)
            }
            BEGIN => {
                self.primitive_raw = Some(write.argument);
                let flags = write.argument & (BEGIN_INSTANCE_NEXT | BEGIN_INSTANCE_CONTINUE);
                self.instance_id = if write.argument
                    & !(0xffff | BEGIN_INSTANCE_NEXT | BEGIN_INSTANCE_CONTINUE)
                    != 0
                {
                    None
                } else {
                    match flags {
                        0 => Some(0),
                        BEGIN_INSTANCE_NEXT => self.instance_id.and_then(|id| id.checked_add(1)),
                        BEGIN_INSTANCE_CONTINUE => self.instance_id,
                        _ => None,
                    }
                };
                MaxwellEvent::Begin {
                    primitive_raw: write.argument,
                }
            }
            END => {
                self.primitive_raw = None;
                MaxwellEvent::End
            }
            SET_DEPTH_TEST
            | SET_DEPTH_WRITE
            | SET_DEPTH_FUNC
            | SET_STENCIL_TEST
            | SET_DEPTH_BIAS_CONTROL
            | SET_SLOPE_SCALE_DEPTH_BIAS
            | SET_DEPTH_BIAS
            | SET_DEPTH_BIAS_CLAMP
            | SET_RASTER_ENABLE
            | SET_POLY_OFFSET_POINT
            | SET_POLY_OFFSET_LINE
            | SET_POLY_OFFSET_FILL
            | OGL_SET_CULL
            | OGL_SET_FRONT_FACE
            | OGL_SET_CULL_FACE
            | SET_FRONT_POLYGON_MODE
            | SET_BACK_POLYGON_MODE
            | SET_PRIMITIVE_TOPOLOGY_CONTROL
            | SET_WINDOW_OFFSET_X
            | SET_WINDOW_OFFSET_Y
            | SET_WINDOW_ORIGIN
            | SET_VIEWPORT_SCALE_OFFSET
            | SET_VIEWPORT_CLIP_CONTROL => return None,
            SET_PRIMITIVE_TOPOLOGY => {
                self.primitive_topology_raw = write.argument;
                return None;
            }
            SET_INDEX_BUFFER_A => {
                self.index_buffer.address.a = write.argument;
                return None;
            }
            SET_INDEX_BUFFER_B => {
                self.index_buffer.address.b = write.argument;
                return None;
            }
            SET_INDEX_BUFFER_C => {
                self.index_buffer.limit.a = write.argument;
                return None;
            }
            SET_INDEX_BUFFER_D => {
                self.index_buffer.limit.b = write.argument;
                return None;
            }
            SET_INDEX_BUFFER_E => {
                self.index_buffer.index_size_raw = write.argument;
                return None;
            }
            SET_INDEX_BUFFER_F => {
                self.index_buffer.first = write.argument;
                return None;
            }
            DRAW_INDEX_BUFFER => MaxwellEvent::DrawIndexBuffer {
                raw: write.argument,
                state: Box::new(self.draw_state()),
            },
            method if method >= MACRO_BASE => {
                let relative = method - MACRO_BASE;
                let index = relative / 2;
                if relative % 2 == 0 {
                    self.active_macro = Some(MacroInvocation {
                        index,
                        parameters: vec![write.argument],
                    });
                    MaxwellEvent::MacroStarted {
                        index,
                        first_parameter: write.argument,
                    }
                } else {
                    if let Some(active) = self.active_macro.as_mut().filter(|m| m.index == index) {
                        active.parameters.push(write.argument);
                    }
                    MaxwellEvent::MacroParameter {
                        index,
                        parameter: write.argument,
                    }
                }
            }
            _ => MaxwellEvent::OpaqueRegister {
                method: write.method,
                argument: write.argument,
            },
        };
        Some(event)
    }
}

fn ranged_method(method: u16, base: u16, stride: u16, count: usize) -> Option<(usize, usize)> {
    if method < base {
        return None;
    }
    let rel = method - base;
    let index = usize::from(rel / stride);
    let field = usize::from(rel % stride);
    (index < count).then_some((index, field))
}

fn write_indexed(ram: &mut Vec<u32>, index: u32, value: u32) {
    let index = index as usize;
    if ram.len() <= index {
        ram.resize(index + 1, 0);
    }
    ram[index] = value;
}

#[cfg(test)]
mod tests {
    #[test]
    fn begin_instance_sequence_retains_order_and_rejects_invalid_history() {
        use super::*;
        let mut state = Maxwell3dState::default();
        state.apply(graphics(method::BEGIN, 4 | method::BEGIN_INSTANCE_CONTINUE));
        assert_eq!(state.draw_state().draw_instance_id(), None);
        for (flags, expected) in [
            (0, Some(0)),
            (method::BEGIN_INSTANCE_NEXT, Some(1)),
            (method::BEGIN_INSTANCE_CONTINUE, Some(1)),
            (method::BEGIN_INSTANCE_NEXT, Some(2)),
            (0, Some(0)),
            (
                method::BEGIN_INSTANCE_NEXT | method::BEGIN_INSTANCE_CONTINUE,
                None,
            ),
            (method::BEGIN_INSTANCE_NEXT, None),
            (0, Some(0)),
            (1 << 24, None),
        ] {
            state.apply(graphics(method::BEGIN, 4 | flags));
            let snapshot = state.draw_state();
            assert_eq!(snapshot.draw_instance_id(), expected);
            state.apply(graphics(method::END, 0));
            assert_eq!(state.draw_state().draw_instance_id(), None);
            assert_eq!(snapshot.draw_instance_id(), expected);
        }
        state.instance_id = Some(u32::MAX);
        state.apply(graphics(method::BEGIN, 4 | method::BEGIN_INSTANCE_NEXT));
        assert_eq!(state.draw_state().draw_instance_id(), None);
    }
    #[test]
    fn captured_draw_state_writes_are_not_work_but_unknown_methods_stay_opaque() {
        use super::*;
        let mut state = Maxwell3dState::default();
        for register in [
            method::SET_RASTER_ENABLE,
            method::SET_DEPTH_TEST,
            method::SET_WINDOW_ORIGIN,
            method::SET_VERTEX_ELEMENT_BASE,
            method::SET_PRIMITIVE_RESTART,
            method::SET_PRIMITIVE_RESTART_INDEX,
            method::VERTEX_STREAM_PER_INSTANCE_BASE,
            method::VIEWPORT_BASE + 7,
            method::SCISSOR_BASE + 2,
            method::VERTEX_ATTRIBUTE_BASE,
            method::VERTEX_STREAM_BASE + 3,
            method::VERTEX_STREAM_LIMIT_BASE + 1,
        ] {
            assert!(state.apply(graphics(register, u32::MAX)).is_none());
            assert_eq!(state.draw_state().raw_registers[&register], u32::MAX);
        }
        for register in [0x123, method::SCISSOR_BASE + 3] {
            assert!(matches!(state.apply(graphics(register, 17)),
                Some(MaxwellEvent::OpaqueRegister { method, argument: 17 }) if method == register));
        }
        assert!(matches!(
            state.apply(graphics(method::PIXEL_SHADER_BARRIER, 0)),
            Some(MaxwellEvent::Barrier)
        ));
        assert!(matches!(
            state.apply(graphics(method::WAIT_FOR_IDLE, 0)),
            Some(MaxwellEvent::WaitForIdle)
        ));
    }

    #[test]
    fn texture_pools_require_complete_independent_snapshot_state() {
        use super::*;
        let mut state = Maxwell3dState::default();
        for (sampler, base) in [
            (true, method::SET_TEX_SAMPLER_POOL_A),
            (false, method::SET_TEX_HEADER_POOL_A),
        ] {
            assert_eq!(state.draw_state().texture_descriptor_pool(sampler), None);
            for (offset, value) in [(0, 0x12), (1, 0x3456_7800)] {
                assert!(state.apply(graphics(base + offset, value)).is_none());
                assert_eq!(state.draw_state().texture_descriptor_pool(sampler), None);
            }
            assert!(state.apply(graphics(base + 2, 0)).is_none());
            let snapshot = state.draw_state();
            let (address, maximum) = snapshot.texture_descriptor_pool(sampler).unwrap();
            assert_eq!(address.high_low(), 0x12_3456_7800);
            assert_eq!(maximum, 0); // Index zero is a real one-entry pool.
            assert!(state.apply(graphics(base + 2, u32::MAX)).is_none());
            assert_eq!(snapshot.texture_descriptor_pool(sampler).unwrap().1, 0);
            assert_eq!(
                state
                    .draw_state()
                    .texture_descriptor_pool(sampler)
                    .unwrap()
                    .1,
                u32::MAX
            );
        }
        // Cache invalidations are commands, never harmless pool-state writes.
        for register in [0x3dd, 0x4a2, 0x509, 0x50a] {
            assert!(matches!(state.apply(graphics(register, 0)),
                Some(MaxwellEvent::OpaqueRegister { method, .. }) if method == register));
        }
    }
    use super::*;
    #[test]
    fn constant_upload_addresses_use_current_complete_bounded_selector() {
        let mut state = Maxwell3dState::default();
        assert_eq!(state.constant_buffer_upload_address(), None);
        for (method, value) in [
            (method::CONSTANT_BUFFER_SELECTOR_SIZE, 32),
            (method::CONSTANT_BUFFER_SELECTOR_HIGH, 0),
            (method::CONSTANT_BUFFER_SELECTOR_LOW, 0x1000),
            (method::CONSTANT_BUFFER_UPLOAD_OFFSET, 28),
        ] {
            state.apply(graphics(method, value));
        }
        assert_eq!(state.constant_buffer_upload_address(), Some(0x101c));
        state.apply(graphics(method::CONSTANT_BUFFER_BIND_BASE, 1));
        state.apply(graphics(method::CONSTANT_BUFFER_SELECTOR_LOW, 0x2000));
        assert_eq!(state.constant_buffer_upload_address(), Some(0x201c));
        let event = state.apply(graphics(method::CONSTANT_BUFFER_UPLOAD_BASE, 0x12345678));
        assert_eq!(
            event,
            Some(MaxwellEvent::ConstantBufferUpload {
                address: Some(0x201c),
                word: 0x12345678
            })
        );
        assert_eq!(
            state.raw_registers[&method::CONSTANT_BUFFER_UPLOAD_OFFSET],
            32
        );
        assert_eq!(
            state.draw_state().constant_buffers[&(0, 0)].byte_range(),
            Some((0x1000, 32))
        );
        for offset in [29, 32, 0x10000, u32::MAX] {
            state.apply(graphics(method::CONSTANT_BUFFER_UPLOAD_OFFSET, offset));
            assert_eq!(state.constant_buffer_upload_address(), None);
        }
        state.apply(graphics(method::CONSTANT_BUFFER_UPLOAD_OFFSET, 0));
        state.apply(graphics(method::CONSTANT_BUFFER_SELECTOR_HIGH, 0x100));
        assert_eq!(state.constant_buffer_upload_address(), None);
    }

    #[test]
    fn constant_buffer_reads_follow_live_mappings_and_check_bounds() {
        use crate::address_space::{GpuAddressSpace, GpuMapPermissions, GpuMapping};
        use crate::resource::ResourceId;
        let mut state = Maxwell3dState::default();
        state.apply(graphics(method::CONSTANT_BUFFER_SELECTOR_SIZE, 32));
        state.apply(graphics(method::CONSTANT_BUFFER_SELECTOR_LOW, 0x1000));
        state.apply(graphics(method::CONSTANT_BUFFER_BIND_BASE, 1));
        let snapshot = state.draw_state();
        let mut space = GpuAddressSpace::new();
        let mapping = GpuMapping {
            guest_base: 0x1000,
            size: 32,
            resource: ResourceId(8),
            resource_offset: 128,
            permissions: GpuMapPermissions::READ_ONLY,
        };
        space.map(mapping).unwrap();
        let descriptor = crate::resource::DescriptorBindingDesc {
            set: 2,
            binding: 7,
            descriptor_type: crate::resource::DescriptorType::UniformBuffer,
            count: 3,
            stages: crate::resource::ShaderStageMask(crate::resource::ShaderStageMask::VERTEX),
        };
        assert!(matches!(
            snapshot
                .prepare_constant_buffer_descriptor(0, 0, descriptor, 2, &space)
                .unwrap(),
            crate::command::Command::BindBufferDescriptor {
                set: 2,
                binding: 7,
                array_element: 2,
                buffer: ResourceId(8),
                offset: 128,
                range: 32,
            }
        ));
        assert_eq!(
            snapshot
                .prepare_constant_buffer_descriptor(0, 0, descriptor, 3, &space)
                .unwrap_err(),
            ConstantBufferReadError::InvalidDescriptor
        );
        assert_eq!(
            snapshot
                .prepare_constant_buffer_descriptor(
                    0,
                    0,
                    crate::resource::DescriptorBindingDesc {
                        descriptor_type: crate::resource::DescriptorType::StorageBuffer,
                        ..descriptor
                    },
                    0,
                    &space
                )
                .unwrap_err(),
            ConstantBufferReadError::InvalidDescriptor
        );
        let read = |group, slot, offset, length, space: &GpuAddressSpace| {
            snapshot.resolve_constant_buffer_read(group, slot, offset, length, space)
        };
        let range = read(0, 0, 28, 4, &space).unwrap();
        assert_eq!(
            (range.resource, range.offset, range.length),
            (ResourceId(8), 156, 4)
        );
        for (offset, length) in [(32, 1), (29, 4), (0, 0), (u64::MAX, 2)] {
            assert_eq!(
                read(0, 0, offset, length, &space),
                Err(ConstantBufferReadError::OutOfBounds)
            );
        }
        for (group, slot) in [(5, 0), (0, 32), (1, 0), (0, 1)] {
            assert_eq!(
                read(group, slot, 0, 4, &space),
                Err(ConstantBufferReadError::InvalidBinding)
            );
        }
        let mut remapped = GpuAddressSpace::new();
        remapped
            .map(GpuMapping {
                resource: ResourceId(9),
                resource_offset: 256,
                ..mapping
            })
            .unwrap();
        assert_eq!(read(0, 0, 0, 4, &remapped).unwrap().resource, ResourceId(9));
        assert!(matches!(
            snapshot
                .prepare_constant_buffer_descriptor(0, 0, descriptor, 0, &remapped)
                .unwrap(),
            crate::command::Command::BindBufferDescriptor {
                buffer: ResourceId(9),
                offset: 256,
                ..
            }
        ));
        let mut denied = GpuAddressSpace::new();
        denied
            .map(GpuMapping {
                permissions: GpuMapPermissions {
                    read: false,
                    write: true,
                    execute: false,
                },
                ..mapping
            })
            .unwrap();
        assert_eq!(
            read(0, 0, 0, 4, &denied),
            Err(ConstantBufferReadError::NotReadable)
        );
        assert!(matches!(
            read(0, 0, 0, 4, &GpuAddressSpace::new()),
            Err(ConstantBufferReadError::Address(_))
        ));
    }

    #[test]
    fn constant_buffer_bindings_capture_selectors_and_replace_stale_slots() {
        let mut state = Maxwell3dState::default();
        let bind = method::CONSTANT_BUFFER_BIND_BASE;
        state.apply(graphics(method::CONSTANT_BUFFER_SELECTOR_SIZE, 256));
        state.apply(graphics(method::CONSTANT_BUFFER_SELECTOR_HIGH, 1));
        state.apply(graphics(method::CONSTANT_BUFFER_SELECTOR_LOW, 0x1000));
        state.apply(graphics(bind, (31 << 4) | 1));
        let old = state.draw_state();
        assert_eq!(
            old.constant_buffers[&(0, 31)].byte_range(),
            Some((0x100001000, 256))
        );
        state.apply(graphics(method::CONSTANT_BUFFER_SELECTOR_LOW, 0x2000));
        state.apply(graphics(bind + 4 * method::CONSTANT_BUFFER_BIND_STRIDE, 1));
        assert_eq!(
            state.draw_state().constant_buffers[&(4, 0)].byte_range(),
            Some((0x100002000, 256))
        );
        assert_eq!(
            state.draw_state().constant_buffers[&(0, 31)],
            old.constant_buffers[&(0, 31)]
        );
        state.apply(graphics(bind, 31 << 4));
        assert_eq!(
            state.draw_state().constant_buffers[&(0, 31)].byte_range(),
            None
        );
        state.apply(graphics(bind, (31 << 4) | 3));
        assert_eq!(
            state.draw_state().constant_buffers[&(0, 31)].byte_range(),
            None
        );
        assert!(old.constant_buffers[&(0, 31)].byte_range().is_some());

        let valid = old.constant_buffers[&(0, 31)];
        for size_raw in [0, 0x20000, u32::MAX] {
            assert_eq!(
                ConstantBufferBinding { size_raw, ..valid }.byte_range(),
                None
            );
        }
        for address in [
            RawGpuAddress { a: 0x100, b: 0 },
            RawGpuAddress {
                a: 0xff,
                b: u32::MAX,
            },
        ] {
            assert_eq!(
                ConstantBufferBinding { address, ..valid }.byte_range(),
                None
            );
        }
    }

    #[test]
    fn vertex_elements_respect_stride_offsets_limits_and_zero_stride() {
        use crate::address_space::{GpuAddressSpace, GpuMapPermissions, GpuMapping};
        use crate::resource::ResourceId;
        let mut state = Maxwell3dState::default();
        for (offset, word) in [0x1010, 0, 0x1000, 0].into_iter().enumerate() {
            state.apply(graphics(method::VERTEX_STREAM_BASE + offset as u16, word));
        }
        state.apply(graphics(method::VERTEX_STREAM_LIMIT_BASE, 0));
        state.apply(graphics(method::VERTEX_STREAM_LIMIT_BASE + 1, 0x103f));
        let attribute_word = (0x04 << 21) | (7 << 27) | (8 << 7);
        state.apply(graphics(method::VERTEX_ATTRIBUTE_BASE, attribute_word));
        let mut space = GpuAddressSpace::new();
        space
            .map(GpuMapping {
                guest_base: 0x1000,
                size: 64,
                resource: ResourceId(7),
                resource_offset: 128,
                permissions: GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        let snapshot = state.draw_state();
        for element_index in 0..4 {
            let range = snapshot
                .resolve_vertex_element(0, element_index, &space)
                .unwrap();
            assert_eq!(range.resource, ResourceId(7));
            assert_eq!(range.offset, 136 + element_index * 16);
            assert_eq!(range.length, 8);
        }
        let input = snapshot
            .prepare_vertex_input(0, 19, crate::resource::VertexInputRate::Instance, &space)
            .unwrap();
        assert_eq!(input.attribute.location, 19);
        assert_eq!(input.attribute.binding, 0);
        assert_eq!(input.attribute.offset, 8);
        assert_eq!(
            input.attribute.format,
            crate::resource::VertexFormat::Rg32Float
        );
        assert_eq!(input.binding.stride, 16);
        assert_eq!(
            input.binding.input_rate,
            crate::resource::VertexInputRate::Instance
        );
        assert_eq!(input.range.offset, 128);
        assert_eq!(input.range.length, 64);
        let mut second = input;
        second.attribute.location = 20;
        let (layout, commands) = merge_vertex_inputs(&[second, input]).unwrap();
        assert_eq!(layout.bindings.len(), 1);
        assert_eq!(layout.attributes.len(), 2);
        assert_eq!(layout.attributes[0].location, 19);
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            commands[0],
            crate::Command::BindVertexBuffer {
                slot: 0,
                buffer: ResourceId(7),
                offset: 128,
            }
        ));
        assert!(merge_vertex_inputs(&[input, input]).is_err());
        let mut invalid = input;
        invalid.attribute.offset = 57;
        assert!(merge_vertex_inputs(&[invalid]).is_err());
        invalid.attribute.offset = 56;
        assert!(merge_vertex_inputs(&[invalid]).is_ok());
        invalid.attribute.format = crate::VertexFormat::Unknown(0);
        assert!(merge_vertex_inputs(&[invalid]).is_err());
        second.binding.stride += 4;
        assert!(merge_vertex_inputs(&[input, second]).is_err());
        let interface = crate::maxwell_vertex::VertexAttributeInterface {
            slot: 0,
            location: 19,
            format: crate::resource::VertexFormat::Rg32Float,
        };
        for (first, count) in [(0, 4), (1, 3), (u64::MAX, 0)] {
            assert_eq!(
                snapshot
                    .prepare_shader_vertex_range(
                        interface,
                        crate::resource::VertexInputRate::Instance,
                        first,
                        count,
                        &space,
                    )
                    .unwrap(),
                input
            );
        }
        for (first, count) in [(0, 5), (4, 1), (u64::MAX, 2)] {
            assert!(
                snapshot
                    .prepare_shader_vertex_range(
                        interface,
                        crate::resource::VertexInputRate::Instance,
                        first,
                        count,
                        &space,
                    )
                    .is_err()
            );
        }
        assert_eq!(
            snapshot
                .prepare_shader_vertex_input(
                    interface,
                    crate::resource::VertexInputRate::Instance,
                    &space
                )
                .unwrap(),
            input
        );
        for invalid in [
            crate::maxwell_vertex::VertexAttributeInterface {
                format: crate::resource::VertexFormat::R32Float,
                ..interface
            },
            crate::maxwell_vertex::VertexAttributeInterface {
                location: 32,
                ..interface
            },
            crate::maxwell_vertex::VertexAttributeInterface {
                slot: 32,
                ..interface
            },
        ] {
            assert!(
                snapshot
                    .prepare_shader_vertex_input(
                        invalid,
                        crate::resource::VertexInputRate::Instance,
                        &space
                    )
                    .is_err()
            );
        }
        for element_index in [4, u64::MAX] {
            assert_eq!(
                snapshot.resolve_vertex_element(0, element_index, &space),
                Err(VertexStreamResolveError::ElementOutOfBounds {
                    attribute_index: 0,
                    element_index
                })
            );
        }
        state.apply(graphics(method::VERTEX_STREAM_BASE, 0x1000));
        assert_eq!(
            state
                .draw_state()
                .resolve_vertex_element(0, u64::MAX, &space)
                .unwrap()
                .offset,
            136
        );
        // A large attribute offset is valid with zero stride if the stream covers it.
        state.apply(graphics(
            method::VERTEX_ATTRIBUTE_BASE,
            (attribute_word & !(0x3fff << 7)) | (56 << 7),
        ));
        assert_eq!(
            state
                .draw_state()
                .resolve_vertex_element(0, 9, &space)
                .unwrap()
                .offset,
            184
        );
        state.apply(graphics(
            method::VERTEX_ATTRIBUTE_BASE,
            attribute_word | (1 << 31),
        ));
        assert_eq!(
            state.draw_state().resolve_vertex_element(0, 0, &space),
            Err(VertexStreamResolveError::InvalidAttribute { index: 0 })
        );
    }

    #[test]
    fn vertex_formats_promote_only_exact_supported_encodings() {
        use crate::resource::VertexFormat::*;
        let rows = [
            (
                0x1d,
                [(1, R8Snorm), (2, R8Unorm), (3, R8Sint), (4, R8Uint)].to_vec(),
            ),
            (
                0x18,
                [(1, Rg8Snorm), (2, Rg8Unorm), (3, Rg8Sint), (4, Rg8Uint)].to_vec(),
            ),
            (
                0x0a,
                [
                    (1, Rgba8Snorm),
                    (2, Rgba8Unorm),
                    (3, Rgba8Sint),
                    (4, Rgba8Uint),
                ]
                .to_vec(),
            ),
            (0x1b, [(3, R16Sint), (4, R16Uint), (7, R16Float)].to_vec()),
            (
                0x0f,
                [(3, Rg16Sint), (4, Rg16Uint), (7, Rg16Float)].to_vec(),
            ),
            (
                0x03,
                [(3, Rgba16Sint), (4, Rgba16Uint), (7, Rgba16Float)].to_vec(),
            ),
            (0x12, [(3, R32Sint), (4, R32Uint), (7, R32Float)].to_vec()),
            (
                0x04,
                [(3, Rg32Sint), (4, Rg32Uint), (7, Rg32Float)].to_vec(),
            ),
            (
                0x02,
                [(3, Rgb32Sint), (4, Rgb32Uint), (7, Rgb32Float)].to_vec(),
            ),
            (
                0x01,
                [(3, Rgba32Sint), (4, Rgba32Uint), (7, Rgba32Float)].to_vec(),
            ),
        ];
        for widths in 0..64 {
            for numerical_type in 0..8 {
                let raw = (widths << 21) | (numerical_type << 27) | 31 | (0x3fff << 7);
                let expected = rows
                    .iter()
                    .find(|(code, _)| *code == widths)
                    .and_then(|(_, types)| types.iter().find(|(code, _)| *code == numerical_type))
                    .map(|(_, format)| *format);
                assert_eq!(
                    VertexAttributeState::from_raw(raw).canonical_format(),
                    expected
                );
                for unsupported_bit in [5, 6, 30, 31] {
                    assert_eq!(
                        VertexAttributeState::from_raw(raw | (1 << unsupported_bit))
                            .canonical_format(),
                        None
                    );
                }
            }
        }
    }

    #[test]
    fn vertex_stream_resolution_uses_live_resource_offsets_and_permissions() {
        use crate::address_space::{GpuAddressSpace, GpuMapPermissions, GpuMapping};
        use crate::resource::ResourceId;

        let mut state = Maxwell3dState::default();
        for (offset, word) in [0x1010, 0, 0x1080, 0].into_iter().enumerate() {
            state.apply(graphics(method::VERTEX_STREAM_BASE + offset as u16, word));
        }
        state.apply(graphics(method::VERTEX_STREAM_LIMIT_BASE, 0));
        state.apply(graphics(method::VERTEX_STREAM_LIMIT_BASE + 1, 0x10ff));
        let snapshot = state.draw_state();
        let mut space = GpuAddressSpace::new();
        assert!(matches!(
            snapshot.resolve_vertex_stream(0, &space),
            Err(VertexStreamResolveError::Address(_))
        ));
        let mapping = GpuMapping {
            guest_base: 0x1000,
            size: 0x100,
            resource: ResourceId(7),
            resource_offset: 0x200,
            permissions: GpuMapPermissions::READ_ONLY,
        };
        space.map(mapping).unwrap();
        let range = snapshot.resolve_vertex_stream(0, &space).unwrap();
        assert_eq!(range.resource, ResourceId(7));
        assert_eq!(range.offset, 0x280);
        assert_eq!(range.length, 0x80);
        space.unmap(0x1000).unwrap();
        space
            .map(GpuMapping {
                resource: ResourceId(9),
                resource_offset: 0x400,
                ..mapping
            })
            .unwrap();
        let range = snapshot.resolve_vertex_stream(0, &space).unwrap();
        assert_eq!(range.resource, ResourceId(9));
        assert_eq!(range.offset, 0x480);
        space.unmap(0x1000).unwrap();
        space
            .map(GpuMapping {
                permissions: GpuMapPermissions {
                    read: false,
                    write: true,
                    execute: false,
                },
                ..mapping
            })
            .unwrap();
        assert_eq!(
            snapshot.resolve_vertex_stream(0, &space),
            Err(VertexStreamResolveError::NotReadable { index: 0 })
        );
        space.unmap(0x1000).unwrap();
        space
            .map(GpuMapping {
                size: 0xff,
                ..mapping
            })
            .unwrap();
        assert!(matches!(
            snapshot.resolve_vertex_stream(0, &space),
            Err(VertexStreamResolveError::Address(_))
        ));
        assert_eq!(
            snapshot.resolve_vertex_stream(32, &space),
            Err(VertexStreamResolveError::InvalidDescriptor { index: 32 })
        );
    }

    #[test]
    fn vertex_stream_limits_bound_ranges_without_invented_memory() {
        let mut state = Maxwell3dState::default();
        for index in [0, 15, 16, 31] {
            let stream_base = method::VERTEX_STREAM_BASE + index * method::VERTEX_STREAM_STRIDE;
            for (offset, word) in [0x1010, 0xff, 0xfffffff0, 0].into_iter().enumerate() {
                state.apply(graphics(stream_base + offset as u16, word));
            }
            assert_eq!(state.draw_state().vertex_stream_range(index as usize), None);
            let limit_base =
                method::VERTEX_STREAM_LIMIT_BASE + index * method::VERTEX_STREAM_LIMIT_STRIDE;
            state.apply(graphics(limit_base, 0xff));
            assert_eq!(state.draw_state().vertex_stream_limit(index as usize), None);
            state.apply(graphics(limit_base + 1, u32::MAX));
            assert_eq!(
                state.draw_state().vertex_stream_range(index as usize),
                Some((0xfffffffff0, 16))
            );
        }
        let snapshot = state.draw_state();
        let base = method::VERTEX_STREAM_LIMIT_BASE;
        state.apply(graphics(base + 1, 0xfffffff0));
        assert_eq!(
            state.draw_state().vertex_stream_range(0),
            Some((0xfffffffff0, 1))
        );
        state.apply(graphics(base + 1, 0xffffffef));
        assert_eq!(state.draw_state().vertex_stream_range(0), None);
        state.apply(graphics(base, 0x100));
        assert_eq!(state.draw_state().vertex_stream_range(0), None);
        assert_eq!(state.draw_state().vertex_stream_limit(0).unwrap().a, 0x100);
        assert_eq!(snapshot.vertex_stream_range(0), Some((0xfffffffff0, 16)));
        for format in [0x10, 0x80001010] {
            state.apply(graphics(base, 0xff));
            state.apply(graphics(base + 1, u32::MAX));
            state.apply(graphics(method::VERTEX_STREAM_BASE, format));
            assert_eq!(state.draw_state().vertex_stream_range(0), None);
        }
        for index in [1, 32, usize::MAX] {
            assert_eq!(snapshot.vertex_stream_limit(index), None);
            assert_eq!(snapshot.vertex_stream_range(index), None);
        }
    }

    #[test]
    fn vertex_streams_require_complete_snapshot_and_preserve_bank_boundaries() {
        let mut state = Maxwell3dState::default();
        let words = [0x80001fff, 0xff, 0xfffffff0, u32::MAX];
        for index in [0, 15, 16, 31] {
            let base = method::VERTEX_STREAM_BASE + index * method::VERTEX_STREAM_STRIDE;
            for (offset, word) in words.into_iter().enumerate() {
                assert_eq!(state.draw_state().vertex_stream(index as usize), None);
                state.apply(graphics(base + offset as u16, word));
            }
        }
        let first = state.apply(graphics(method::DRAW_INDEX_BUFFER, 3)).unwrap();
        state.apply(graphics(method::VERTEX_STREAM_BASE + 1, 0x100));
        let MaxwellEvent::DrawIndexBuffer { state: first, .. } = first else {
            unreachable!()
        };
        for index in [0, 15, 16, 31] {
            let stream = first.vertex_stream(index).unwrap();
            assert_eq!(stream.raw, words);
            assert_eq!(stream.stride(), 0xfff);
            assert!(stream.enabled());
            assert_eq!(stream.address(), Some(0xfffffffff0));
            assert_eq!(stream.frequency(), u32::MAX);
            assert_eq!(stream.reserved_format_bits(), 0x80000000);
        }
        assert_eq!(state.draw_state().vertex_stream(0).unwrap().address(), None);
        for index in [1, 32, usize::MAX] {
            assert_eq!(first.vertex_stream(index), None);
        }
        let zero = VertexStreamState { raw: [0; 4] };
        assert_eq!(zero.stride(), 0);
        assert!(!zero.enabled());
        assert_eq!(zero.address(), Some(0));
        assert_eq!(zero.frequency(), 0);
        assert_eq!(zero.reserved_format_bits(), 0);
    }

    #[test]
    fn vertex_attributes_preserve_fields_and_draw_boundaries() {
        let mut state = Maxwell3dState::default();
        assert_eq!(state.draw_state().vertex_attribute(0), None);
        let raw = 31
            | (1 << 6)
            | (0x3fff << 7)
            | (0x30 << 21)
            | (7 << 27)
            | (1 << 31)
            | (1 << 5)
            | (1 << 30);
        for index in [0, 15, 16, 31] {
            state.apply(graphics(method::VERTEX_ATTRIBUTE_BASE + index, raw));
        }
        let first = state.apply(graphics(method::DRAW_VERTEX_ARRAY, 3)).unwrap();
        state.apply(graphics(method::VERTEX_ATTRIBUTE_BASE + 16, 0));
        let MaxwellEvent::DrawVertexArray { state: first, .. } = first else {
            unreachable!()
        };
        for index in [0, 15, 16, 31] {
            let attribute = first.vertex_attribute(index).unwrap();
            assert_eq!(attribute.raw, raw);
            assert_eq!(attribute.stream, 31);
            assert!(attribute.inactive);
            assert_eq!(attribute.offset, 0x3fff);
            assert_eq!(attribute.component_bit_widths, 0x30);
            assert_eq!(attribute.numerical_type, 7);
            assert!(attribute.swap_r_and_b);
            assert_eq!(attribute.reserved_bits, (1 << 5) | (1 << 30));
        }
        assert_eq!(state.draw_state().vertex_attribute(16).unwrap().raw, 0);
        assert_eq!(first.vertex_attribute(1), None);
        assert_eq!(first.vertex_attribute(32), None);
        assert_eq!(first.vertex_attribute(usize::MAX), None);
        // Every bit survives decoding, including undocumented/reserved bits.
        for bit in 0..32 {
            assert_eq!(VertexAttributeState::from_raw(1 << bit).raw, 1 << bit);
        }
    }

    #[test]
    fn draw_events_keep_register_state_at_their_own_emission_boundary() {
        let mut state = Maxwell3dState::default();
        state.apply(graphics(method::BEGIN, 4));
        state.apply(graphics(method::SET_PROGRAM_REGION_B, 0x1000));
        state.apply(graphics(method::PIPELINE_BASE + 1, 0x80));
        state.apply(graphics(method::COLOR_TARGET_BASE + 1, 0x2000));
        let nan_bits = 0x7fc01234;
        state.apply(graphics(method::VIEWPORT_BASE, nan_bits));
        let first = state.apply(graphics(method::DRAW_VERTEX_ARRAY, 3)).unwrap();
        state.apply(graphics(method::BEGIN, 5));
        state.apply(graphics(method::SET_PROGRAM_REGION_B, 0x3000));
        state.apply(graphics(method::PIPELINE_BASE + 1, 0x100));
        state.apply(graphics(method::COLOR_TARGET_BASE + 1, 0x4000));
        state.apply(graphics(method::VIEWPORT_BASE, 1f32.to_bits()));
        let second = state.apply(graphics(method::DRAW_INDEX_BUFFER, 9)).unwrap();
        let MaxwellEvent::DrawVertexArray { state: first, .. } = first else {
            unreachable!()
        };
        let MaxwellEvent::DrawIndexBuffer { state: second, .. } = second else {
            unreachable!()
        };
        assert_eq!(first.primitive_raw, Some(4));
        assert_eq!(second.primitive_raw, Some(5));
        for (register, old, new) in [
            (method::SET_PROGRAM_REGION_B, 0x1000, 0x3000),
            (method::PIPELINE_BASE + 1, 0x80, 0x100),
            (method::COLOR_TARGET_BASE + 1, 0x2000, 0x4000),
            (method::VIEWPORT_BASE, nan_bits, 1f32.to_bits()),
        ] {
            assert_eq!(first.raw_registers[&register], old);
            assert_eq!(second.raw_registers[&register], new);
        }
        // Snapshot equality is bit-exact even for non-reflexive float values.
        assert_eq!(first, first.clone());
    }

    #[test]
    fn macro_reads_nonzero_method_index_from_frontend_register_mirror() {
        let mut state = Maxwell3dState::default();
        state.apply(graphics(method::SET_VERTEX_ARRAY_START, 4));
        state.mme.start_addresses = vec![0];
        // ADD immediate + MoveAndSetMethod selects BEGIN. READ loads the
        // vertex-start method into R1; ADD R1,R0 + Send emits that value.
        state.mme.instruction_ram = vec![
            1 | (2 << 4) | (u32::from(method::BEGIN) << 14),
            5 | (1 << 8) | (u32::from(method::SET_VERTEX_ARRAY_START) << 14),
            (4 << 4) | (1 << 11),
            0x91,
            0x11,
        ];
        state.apply(graphics(method::MACRO_BASE, 0));
        assert_eq!(
            state.execute_active_macro(16, 16, 16),
            vec![MaxwellEvent::Begin { primitive_raw: 4 }]
        );
        assert_eq!(state.primitive_raw, Some(4));
    }

    #[test]
    fn reports_missing_unsupported_and_incomplete_macro_programs() {
        let mut state = Maxwell3dState::default();
        state.apply(graphics(method::MACRO_BASE, 7));
        assert_eq!(
            state.execute_active_macro(16, 16, 16),
            vec![MaxwellEvent::MacroFailed {
                index: 0,
                reason: MacroFailure::MissingProgram,
            }]
        );
        state.mme.start_addresses = vec![0];
        for (words, steps, expected) in [
            (
                vec![6],
                16,
                crate::mme_exec::MmeStopReason::OpaqueInstruction { pc: 0, raw: 6 },
            ),
            (vec![0x11], 16, crate::mme_exec::MmeStopReason::EndOfProgram),
            (vec![0x91], 0, crate::mme_exec::MmeStopReason::StepLimit),
        ] {
            state.mme.instruction_ram = words;
            state.apply(graphics(method::MACRO_BASE, 7));
            assert_eq!(
                state.execute_active_macro(16, steps, 16),
                vec![MaxwellEvent::MacroFailed {
                    index: 0,
                    reason: MacroFailure::ExecutionStopped(expected),
                }]
            );
            assert!(state.active_macro.is_none());
        }
        // EXIT still executes its delay slot; a lone exit word is truncated.
        state.mme.instruction_ram = vec![0x91, 0x11];
        state.apply(graphics(method::MACRO_BASE, 7));
        assert!(state.execute_active_macro(16, 16, 16).is_empty());
        state.mme.instruction_ram = vec![0x30, 0x30];
        state.apply(graphics(method::MACRO_BASE, 7));
        let events = state.execute_active_macro(16, 16, 16);
        assert!(matches!(
            events.last(),
            Some(MaxwellEvent::MacroFailed {
                reason: MacroFailure::ExecutionStopped(
                    crate::mme_exec::MmeStopReason::ParameterUnderflow {
                        pc: 1,
                        parameter: 1,
                    }
                ),
                ..
            })
        ));
    }
    fn graphics(method: u16, argument: u32) -> EngineMethodWrite {
        EngineMethodWrite {
            subchannel: 0,
            class: EngineClass::Graphics,
            method,
            argument,
        }
    }

    #[test]
    fn topology_override_is_captured_without_changing_begin_encoding() {
        use crate::{pipeline::draw_topology, resource::PrimitiveTopology};

        let mut state = Maxwell3dState::default();
        state.apply(graphics(method::BEGIN, 1));
        assert_eq!(draw_topology(&state.draw_state()), None);
        assert!(
            state
                .apply(graphics(method::SET_PRIMITIVE_TOPOLOGY_CONTROL, 0))
                .is_none()
        );
        let captured = state.draw_state();
        assert_eq!(draw_topology(&captured), Some(PrimitiveTopology::LineList));

        state.apply(graphics(method::SET_PRIMITIVE_TOPOLOGY, 1));
        state.apply(graphics(method::SET_PRIMITIVE_TOPOLOGY_CONTROL, 1));
        assert_eq!(
            draw_topology(&state.draw_state()),
            Some(PrimitiveTopology::PointList)
        );
        // Later register writes must not alter the primitive captured for an earlier draw.
        assert_eq!(draw_topology(&captured), Some(PrimitiveTopology::LineList));
    }

    #[test]
    fn indexed_draw_retains_correct_size_first_and_address_limits() {
        let mut state = Maxwell3dState::default();
        for (method, argument) in [
            (method::SET_INDEX_BUFFER_A, 1),
            (method::SET_INDEX_BUFFER_B, 0x1000),
            (method::SET_INDEX_BUFFER_C, 1),
            (method::SET_INDEX_BUFFER_D, 0x1fff),
            (method::SET_INDEX_BUFFER_E, 0),
            (method::SET_INDEX_BUFFER_F, 7),
        ] {
            state.apply(graphics(method, argument));
        }
        let MaxwellEvent::DrawIndexBuffer { raw, state: draw } =
            state.apply(graphics(method::DRAW_INDEX_BUFFER, 9)).unwrap()
        else {
            panic!("expected indexed draw");
        };
        assert_eq!(raw, 9);
        assert_eq!(draw.index_buffer.address.high_low(), 0x1_00001000);
        assert_eq!(draw.index_buffer.limit.high_low(), 0x1_00001fff);
        assert_eq!(draw.index_buffer.index_size_raw, 0);
        assert_eq!(draw.index_buffer.first, 7);
        state.apply(graphics(method::SET_INDEX_BUFFER_E, 0xffff));
        state.apply(graphics(method::SET_INDEX_BUFFER_F, 3));
        assert_eq!(draw.index_buffer.first, 7);
        assert_eq!(draw.index_buffer.index_size_raw, 0);
        assert_eq!(state.index_buffer.index_size_raw, 0xffff);
        assert_eq!(state.index_buffer.first, 3);
    }

    #[test]
    fn index_read_ranges_decode_width_and_reject_out_of_address_space() {
        let mut state = IndexBufferState {
            address: RawGpuAddress { a: 1, b: 0x1000 },
            first: 7,
            ..IndexBufferState::default()
        };
        for (raw, width) in [(0, 1), (1, 2), (2, 4)] {
            state.index_size_raw = raw;
            assert_eq!(state.index_bytes(), Some(width));
            assert_eq!(
                state.index_read_range(9),
                Some((0x1_00001000 + 7 * width, 9 * width))
            );
            assert_eq!(state.index_read_range(0), None);
        }
        for raw in [3, 4, 0x100, u32::MAX] {
            state.index_size_raw = raw;
            assert_eq!(state.index_read_range(1), None);
        }
        state.index_size_raw = 2;
        state.first = 0;
        state.address = RawGpuAddress {
            a: 0xff,
            b: 0xfffffffc,
        };
        assert_eq!(state.index_read_range(1), Some((0xff_fffffffc, 4)));
        assert_eq!(state.index_read_range(2), None);
        state.first = 1;
        assert_eq!(state.index_read_range(1), None);
        state.address.a = 0x100;
        assert_eq!(state.index_read_range(1), None);
    }
    #[test]
    fn tracks_render_target_and_draw() {
        use crate::address_space::{GpuMapPermissions, GpuMapping};
        let mut snapshot = DrawStateSnapshot::default();
        for (field, value) in [0, 0x2000, 65, 9, 0xe5, 0x20, 2, 2560, 1]
            .into_iter()
            .enumerate()
        {
            snapshot
                .raw_registers
                .insert(method::COLOR_TARGET_BASE + field as u16, value);
        }
        let mut space = crate::GpuAddressSpace::new();
        assert!(matches!(
            snapshot.resolve_color_target_layer(0, &space),
            Err(ColorTargetResolveError::Address(_))
        ));
        let mapping = GpuMapping {
            guest_base: 0x2000,
            size: 20480,
            resource: crate::ResourceId(9),
            resource_offset: 128,
            permissions: GpuMapPermissions::READ_ONLY,
        };
        space.map(mapping).unwrap();
        assert_eq!(
            snapshot.resolve_color_target_layer(0, &space),
            Err(ColorTargetResolveError::NotWritable)
        );
        space.unmap(0x2000).unwrap();
        space
            .map(GpuMapping {
                resource: crate::ResourceId(10),
                permissions: GpuMapPermissions::READ_WRITE,
                ..mapping
            })
            .unwrap();
        let range = snapshot.resolve_color_target_layer(0, &space).unwrap();
        assert_eq!(range.resource, crate::ResourceId(10));
        assert_eq!(range.offset, 10368);
        assert_eq!(range.length, 10240);
        let prepared = snapshot
            .prepare_color_target_layer(0, crate::ResourceId(11), 1, &space)
            .unwrap();
        assert_eq!(prepared.backing, range);
        assert_eq!(prepared.texture.id, crate::ResourceId(11));
        assert_eq!(prepared.texture.format, crate::TextureFormat::R32Float);
        assert_eq!(
            (
                prepared.texture.width,
                prepared.texture.height,
                prepared.texture.layers
            ),
            (65, 9, 1)
        );
        assert_eq!(prepared.layout.storage_size(), prepared.backing.length);
        for samples in [0, 2, 4, 8] {
            assert!(
                snapshot
                    .prepare_color_target_layer(0, crate::ResourceId(11), samples, &space)
                    .is_err()
            );
        }
        assert!(
            snapshot
                .prepare_color_target_layer(0, range.resource, 1, &space)
                .is_err()
        );
        assert_eq!(
            snapshot.resolve_color_target_layer(1, &space),
            Err(ColorTargetResolveError::InvalidDescriptor)
        );
        let target = ColorTargetState {
            width: 65,
            height: 9,
            format: 0xe5,
            memory_layout: 0x20,
            layers: 2,
            layer: 1,
            ..Default::default()
        };
        let layout = target.block_linear_layer_layout().unwrap();
        assert_eq!(layout.width_bytes, 260);
        assert_eq!(layout.height_rows, 9);
        assert_eq!(layout.storage_size(), 5 * 4 * 512);
        let layered = ColorTargetState {
            address: RawGpuAddress { a: 0, b: 0x2000 },
            array_pitch: 2560,
            ..target
        };
        assert_eq!(layered.block_linear_layer_range(), Some((0x4800, 10240)));
        assert!(
            ColorTargetState {
                array_pitch: 2559,
                ..layered
            }
            .block_linear_layer_range()
            .is_none()
        );
        assert!(
            ColorTargetState {
                address: RawGpuAddress {
                    a: 0xff,
                    b: 0xfffff000
                },
                ..layered
            }
            .block_linear_layer_range()
            .is_none()
        );
        assert!(
            ColorTargetState {
                address: RawGpuAddress { a: 0x100, b: 0 },
                ..layered
            }
            .block_linear_layer_range()
            .is_none()
        );
        assert_eq!(
            ColorTargetState {
                layer: 0,
                layers: 1,
                array_pitch: 0,
                ..layered
            }
            .block_linear_layer_range(),
            Some((0x2000, 10240))
        );
        for memory_layout in [1, 0x60, 0x100, 0x1000, 0x10000, u32::MAX] {
            assert!(
                ColorTargetState {
                    memory_layout,
                    ..target
                }
                .block_linear_layer_layout()
                .is_none()
            );
        }
        for invalid in [
            ColorTargetState { width: 0, ..target },
            ColorTargetState {
                width: 0x10000000,
                ..target
            },
            ColorTargetState {
                height: 0x20000,
                ..target
            },
            ColorTargetState {
                layers: 0,
                ..target
            },
            ColorTargetState { layer: 2, ..target },
            ColorTargetState {
                format: 0,
                ..target
            },
        ] {
            assert!(invalid.block_linear_layer_layout().is_none());
        }
        use crate::resource::TextureFormat;
        for (raw, format, bytes) in [
            (0xc0, TextureFormat::Rgba32Float, 16),
            (0xca, TextureFormat::Rgba16Float, 8),
            (0xcb, TextureFormat::Rg32Float, 8),
            (0xd5, TextureFormat::Rgba8Unorm, 4),
            (0xde, TextureFormat::Rg16Float, 4),
            (0xe3, TextureFormat::R32Sint, 4),
            (0xe4, TextureFormat::R32Uint, 4),
            (0xe5, TextureFormat::R32Float, 4),
            (0xf2, TextureFormat::R16Float, 2),
            (0xf3, TextureFormat::R8Unorm, 1),
            (0xf5, TextureFormat::R8Sint, 1),
            (0xf6, TextureFormat::R8Uint, 1),
        ] {
            let target = ColorTargetState {
                format: raw,
                ..Default::default()
            };
            assert_eq!(target.canonical_format(), Some(format));
            assert_eq!(
                crate::texture_layout::format_info(format)
                    .unwrap()
                    .bytes_per_block,
                bytes
            );
            assert!(
                ColorTargetState {
                    format: raw | 0x100,
                    ..target
                }
                .canonical_format()
                .is_none()
            );
        }
        for raw in [0, 0xc1, 0xff, u32::MAX] {
            assert!(
                ColorTargetState {
                    format: raw,
                    ..Default::default()
                }
                .canonical_format()
                .is_none()
            );
        }
        let mut captured = Maxwell3dState::default();
        assert!(captured.draw_state().color_target(0).is_none());
        let words = [0, 0x2000, 64, 32, 0xd5, 0x1000, 1, 2048, 0];
        for (field, value) in words.into_iter().enumerate() {
            captured.apply(graphics(method::COLOR_TARGET_BASE + field as u16, value));
        }
        let first = captured.draw_state();
        assert_eq!(first.color_target(0), Some(captured.color_targets[0]));
        assert!(first.color_target(1).is_none());
        assert!(first.color_target(method::COLOR_TARGET_COUNT).is_none());
        captured.apply(graphics(method::COLOR_TARGET_BASE + 1, 0x4000));
        assert_eq!(first.color_target(0).unwrap().address.b, 0x2000);
        assert_eq!(
            captured.draw_state().color_target(0).unwrap().address.b,
            0x4000
        );
        for field in 0..9 {
            let mut incomplete = first.clone();
            incomplete
                .raw_registers
                .remove(&(method::COLOR_TARGET_BASE + field));
            assert!(incomplete.color_target(0).is_none());
        }
        let mut state = Maxwell3dState::default();
        state.apply(graphics(method::COLOR_TARGET_BASE, 1));
        state.apply(graphics(method::COLOR_TARGET_BASE + 1, 2));
        state.apply(graphics(method::COLOR_TARGET_BASE + 2, 1280));
        state.apply(graphics(method::COLOR_TARGET_BASE + 3, 720));
        assert_eq!(state.color_targets[0].address, RawGpuAddress { a: 1, b: 2 });
        assert_eq!(state.color_targets[0].width, 1280);
        state.apply(graphics(method::SET_VERTEX_ARRAY_START, 7));
        assert!(matches!(
            state.apply(graphics(method::DRAW_VERTEX_ARRAY, 3)),
            Some(MaxwellEvent::DrawVertexArray {
                first: 7,
                count: 3,
                ..
            })
        ));
    }
    #[test]
    fn tracks_pipeline_slots() {
        let mut state = Maxwell3dState::default();
        state.apply(graphics(method::PIPELINE_BASE + 1, 0x1234));
        assert_eq!(state.pipeline_shaders[0].program, 0x1234);
    }
}
