// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Maxwell TIC/TSC descriptor boundaries.
//!
//! Switchbrew documents that both TIC and TSC records are 0x20 bytes / eight 32-bit words and that
//! texture handles pack a 20-bit TIC index with a 12-bit TSC index.  Exosphere keeps the full raw
//! descriptor losslessly.  Individual bitfields are promoted only after capture-backed validation;
//! unverified layouts are deliberately left `Unknown` instead of guessed.

use crate::{
    address_space::{GpuAddressError, GpuAddressSpace, ResolvedGpuRange},
    descriptors::{RawDescriptor, SamplerDescriptor, TextureDescriptor},
    maxwell_state::{DrawStateSnapshot, RawGpuAddress},
    resource::{ResourceId, TextureFormat, TextureLayout},
    shader_memory::{GpuMemory, GpuMemoryError},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescriptorPoolError {
    MissingConfiguration,
    IndexOutOfBounds { index: u32, maximum: u32 },
    AddressOverflow,
    Address(GpuAddressError),
    NotReadable,
    Memory(GpuMemoryError),
    UnsupportedHandleConfiguration,
    ConstantBuffer(crate::maxwell_state::ConstantBufferReadError),
}

/// Draw-time raw descriptors selected by a direct TEX instruction. None of
/// these backing identities is the image's identity; that comes from the TIC
/// address only after its layout/footprint has been validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureHandleRecords {
    pub handle: u32,
    pub handle_backing: ResolvedGpuRange,
    pub texture_backing: ResolvedGpuRange,
    pub sampler_backing: ResolvedGpuRange,
    pub texture: RawDescriptor<8>,
    pub sampler: RawDescriptor<8>,
}

impl TextureHandleRecords {
    pub fn prepare_sampling(
        &self,
        image_id: ResourceId,
        sampler_id: ResourceId,
        address_space: &GpuAddressSpace,
    ) -> Result<
        crate::texture_sampling::PreparedTexture2d,
        crate::texture_sampling::TextureSamplingError,
    > {
        crate::texture_sampling::prepare_texture_2d(
            self.texture.words,
            self.sampler.words,
            image_id,
            sampler_id,
            address_space,
        )
    }

    /// Discover the selected image backing separately from the three descriptor
    /// allocations. Sampling-view and TSC validation must still follow before
    /// this storage can be bound as a canonical sampled image.
    pub fn resolve_image_backing(
        &self,
        address_space: &GpuAddressSpace,
    ) -> Result<crate::texture_backing::TextureBacking, crate::texture_backing::TextureBackingError>
    {
        crate::texture_backing::resolve_texture_backing(self.texture.words, address_space)
    }
}

/// Direct TEX selectors are word offsets into the selected stage constant
/// buffer (Mesa NVC0LoweringPass::handleTEX/loadTexHandle). Read the handle and
/// both pool entries afresh, so guest writes and VA remaps remain observable.
/// Linked-TSC selection is deliberately not inferred from an absent register.
pub fn read_texture_handle_records(
    snapshot: &DrawStateSnapshot,
    group: u8,
    selector: u16,
    address_space: &GpuAddressSpace,
    memory: &dyn GpuMemory,
) -> Result<TextureHandleRecords, DescriptorPoolError> {
    use crate::maxwell_state::method;
    let slot = snapshot
        .raw_registers
        .get(&method::SET_BINDLESS_TEXTURE)
        .copied()
        .ok_or(DescriptorPoolError::MissingConfiguration)?;
    if slot & !7 != 0
        || selector > 0x1fff
        || snapshot.raw_registers.get(&method::SET_LINKED_TSC) != Some(&0)
    {
        return Err(DescriptorPoolError::UnsupportedHandleConfiguration);
    }
    let offset = u64::from(selector) * 4;
    let handle_backing = snapshot
        .resolve_constant_buffer_read(group, slot as u8, offset, 4, address_space)
        .map_err(DescriptorPoolError::ConstantBuffer)?;
    let binding = snapshot.constant_buffers[&(group, slot as u8)];
    let (base, _) = binding
        .byte_range()
        .ok_or(DescriptorPoolError::UnsupportedHandleConfiguration)?;
    let address = base
        .checked_add(offset)
        .ok_or(DescriptorPoolError::AddressOverflow)?;
    let bytes = memory
        .read(address, 4)
        .map_err(DescriptorPoolError::Memory)?;
    let bytes: [u8; 4] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        DescriptorPoolError::Memory(GpuMemoryError::BackendRead {
            address,
            size: 4,
            reason: format!("texture handle provider returned {} bytes", bytes.len()),
        })
    })?;
    let handle = u32::from_le_bytes(bytes);
    let (tic, tsc) = split_texture_handle(handle);
    let (texture_backing, texture) = read_pool_record(snapshot, false, tic, address_space, memory)?;
    let (sampler_backing, sampler) = read_pool_record(snapshot, true, tsc, address_space, memory)?;
    Ok(TextureHandleRecords {
        handle,
        handle_backing,
        texture_backing,
        sampler_backing,
        texture,
        sampler,
    })
}

/// Read a raw record without interpreting unverified TIC/TSC fields. The
/// returned resource is the descriptor's backing, not the sampled image.
pub fn read_pool_record(
    snapshot: &DrawStateSnapshot,
    sampler: bool,
    index: u32,
    address_space: &GpuAddressSpace,
    memory: &dyn GpuMemory,
) -> Result<(ResolvedGpuRange, RawDescriptor<8>), DescriptorPoolError> {
    let range = resolve_pool_record(snapshot, sampler, index, address_space)?;
    // Resolution above has already checked this exact record's arithmetic.
    let address = snapshot
        .texture_descriptor_pool(sampler)
        .unwrap()
        .0
        .high_low()
        + u64::from(index) * 32;
    let words = crate::descriptor_memory::read_words(memory, address)
        .map_err(DescriptorPoolError::Memory)?;
    Ok((range, RawDescriptor { words }))
}

/// Resolve one raw TIC/TSC record, not the image referenced by that record.
/// Both records occupy eight words. Resolve on every call: draw snapshots freeze
/// register values, but an address-space remap must change the resource identity.
/// Resolving only the selected record avoids requiring unused pool entries to be
/// mapped and avoids allocating memory proportional to a guest-provided maximum.
pub fn resolve_pool_record(
    snapshot: &DrawStateSnapshot,
    sampler: bool,
    index: u32,
    address_space: &GpuAddressSpace,
) -> Result<ResolvedGpuRange, DescriptorPoolError> {
    let (base, maximum) = snapshot
        .texture_descriptor_pool(sampler)
        .ok_or(DescriptorPoolError::MissingConfiguration)?;
    if index > maximum {
        return Err(DescriptorPoolError::IndexOutOfBounds { index, maximum });
    }
    let address = base
        .high_low()
        .checked_add(u64::from(index) * 32)
        .filter(|address| address.checked_add(32).is_some())
        .ok_or(DescriptorPoolError::AddressOverflow)?;
    let range = address_space
        .resolve(address, 32)
        .map_err(DescriptorPoolError::Address)?;
    if !range.permissions.read {
        return Err(DescriptorPoolError::NotReadable);
    }
    Ok(range)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressMode {
    Repeat,
    MirroredRepeat,
    ClampToEdge,
    ClampToBorder,
    /// Legacy GL_CLAMP: the border can contribute half of a filtered edge.
    Clamp,
    MirrorClampToEdge,
    MirrorClampToBorder,
    MirrorClamp,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// No mipmap selection; valid only for the TSC mip-filter field.
    None,
    Nearest,
    Linear,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedSamplerState {
    pub address_u: AddressMode,
    pub address_v: AddressMode,
    pub address_w: AddressMode,
    pub min_filter: FilterMode,
    pub mag_filter: FilterMode,
    pub mip_filter: FilterMode,
    pub anisotropy_raw: u8,
    /// Unsigned 4.8 clamps and signed 5.8 bias, retained as fixed-point words.
    pub min_lod_8_8: u16,
    pub max_lod_8_8: u16,
    pub lod_bias_8_8: i16,
    pub depth_compare: bool,
    pub depth_compare_op: crate::resource::CompareOp,
    pub srgb_conversion: bool,
    pub force_unnormalized_coords: bool,
    /// 0 weighted average, 1 minimum, 2 maximum; 3 remains reserved.
    pub reduction_mode_raw: u8,
    pub border_color_bits: [u32; 4],
}

/// Decode storage fields of the GM200 TIC layout used by Tegra X1.
/// Swizzles, sRGB conversion and view controls remain in the raw descriptor and
/// must be applied separately; a storage format alone is not a sampling view.
/// Reference: envytools rnndb/graph/gm200_texture.xml and deko3d
/// source/maxwell/{texture_image_control_block,image_formats}.h.
pub fn decode_tic(words: [u32; 8], resource: Option<ResourceId>) -> TextureDescriptor {
    let format_raw = words[0] & 0x7f;
    let header = (words[2] >> 21) & 0x7;
    if header > 4 {
        let mut descriptor = TextureDescriptor::opaque(words);
        descriptor.resource = resource;
        return descriptor;
    }
    let types = std::array::from_fn::<_, 4, _>(|i| (words[0] >> (7 + 3 * i)) & 7);
    let format = match (format_raw, types) {
        (0x1d, [4, 4, 4, 4]) => TextureFormat::R8Uint,
        (0x1d, [3, 3, 3, 3]) => TextureFormat::R8Sint,
        (0x18, [2, 2, 2, 2]) => TextureFormat::Rg8Unorm,
        (0x18, [4, 4, 4, 4]) => TextureFormat::Rg8Uint,
        (0x18, [3, 3, 3, 3]) => TextureFormat::Rg8Sint,
        (0x0f, [4, 4, 4, 4]) => TextureFormat::R32Uint,
        (0x0f, [3, 3, 3, 3]) => TextureFormat::R32Sint,
        (0x0f, [7, 7, 7, 7]) => TextureFormat::R32Float,
        (0x1b, [7, 7, 7, 7]) => TextureFormat::R16Float,
        (0x0c, [7, 7, 7, 7]) => TextureFormat::Rg16Float,
        (0x03, [7, 7, 7, 7]) => TextureFormat::Rgba16Float,
        (0x04, [7, 7, 7, 7]) => TextureFormat::Rg32Float,
        (0x01, [7, 7, 7, 7]) => TextureFormat::Rgba32Float,
        (0x10, [7, 7, 7, 7]) => TextureFormat::Bc6hSfloat,
        (0x11, [7, 7, 7, 7]) => TextureFormat::Bc6hUfloat,
        (0x27, [1, 1, 1, 1]) => TextureFormat::Bc4Snorm,
        (0x28, [1, 1, 1, 1]) => TextureFormat::Bc5Snorm,
        (0x29, [4, 2, 2, 2]) => TextureFormat::D24UnormS8,
        (0x2f, [7, 7, 7, 7]) => TextureFormat::D32Float,
        // Unknown or force-FP16 component conversions must not silently become
        // UNORM. Keep the entire format word for an actionable diagnostic.
        (_, [2, 2, 2, 2]) => match format_raw {
            0x08 => TextureFormat::Rgba8Unorm,
            0x1d => TextureFormat::R8Unorm,
            0x24 => TextureFormat::Bc1RgbaUnorm,
            0x25 => TextureFormat::Bc2RgbaUnorm,
            0x26 => TextureFormat::Bc3RgbaUnorm,
            0x27 => TextureFormat::Bc4Unorm,
            0x28 => TextureFormat::Bc5Unorm,
            0x17 => TextureFormat::Bc7RgbaUnorm,
            0x3a => TextureFormat::D16Unorm,
            0x40 => TextureFormat::Astc4x4Unorm,
            0x41 => TextureFormat::Astc5x5Unorm,
            0x42 => TextureFormat::Astc6x6Unorm,
            0x44 => TextureFormat::Astc8x8Unorm,
            0x45 => TextureFormat::Astc10x10Unorm,
            0x46 => TextureFormat::Astc12x12Unorm,
            0x50 => TextureFormat::Astc5x4Unorm,
            0x51 => TextureFormat::Astc6x5Unorm,
            0x52 => TextureFormat::Astc8x6Unorm,
            0x53 => TextureFormat::Astc10x8Unorm,
            0x54 => TextureFormat::Astc12x10Unorm,
            0x55 => TextureFormat::Astc8x5Unorm,
            0x56 => TextureFormat::Astc10x5Unorm,
            0x57 => TextureFormat::Astc10x6Unorm,
            _ => TextureFormat::Unknown(words[0]),
        },
        _ => TextureFormat::Unknown(words[0]),
    };
    let address = RawGpuAddress {
        a: words[2] & 0xffff,
        // Block-linear low bits include GOB depth offset, not address bits.
        b: words[1]
            & match header {
                0 => u32::MAX,
                1 | 2 => !31,
                _ => !511,
            },
    };
    let width = if header == 0 {
        ((words[3] & 0xffff) << 16 | (words[4] & 0xffff)).checked_add(1)
    } else {
        Some((words[4] & 0xffff) + 1)
    };
    let height = Some(if header == 0 {
        1
    } else {
        (words[5] & 0xffff) + 1
    });
    let depth = Some(if header == 0 {
        1
    } else {
        ((words[5] >> 16) & 0x3fff) + 1
    });
    let mip_levels = Some(if header == 0 {
        1
    } else {
        ((words[3] >> 28) & 0xf) + 1
    });
    let layout = match header {
        0..=2 => TextureLayout::Linear,
        3 | 4 => TextureLayout::BlockLinear {
            block_height_log2: ((words[3] >> 3) & 7) as u8,
            block_depth_log2: ((words[3] >> 6) & 7) as u8,
            raw: words[3],
        },
        _ => TextureLayout::Unknown(words[3]),
    };
    TextureDescriptor {
        raw: RawDescriptor { words },
        resource,
        address,
        width,
        height,
        depth,
        mip_levels,
        format,
        layout,
    }
}

fn address_mode(x: u8) -> AddressMode {
    match x {
        0 => AddressMode::Repeat,
        1 => AddressMode::MirroredRepeat,
        2 => AddressMode::ClampToEdge,
        3 => AddressMode::ClampToBorder,
        4 => AddressMode::Clamp,
        5 => AddressMode::MirrorClampToEdge,
        6 => AddressMode::MirrorClampToBorder,
        7 => AddressMode::MirrorClamp,
        x => AddressMode::Unknown(x),
    }
}
fn filter(x: u8) -> FilterMode {
    match x {
        1 => FilterMode::Nearest,
        2 => FilterMode::Linear,
        x => FilterMode::Unknown(x),
    }
}
fn mip_filter(x: u8) -> FilterMode {
    match x {
        1 => FilterMode::None,
        2 => FilterMode::Nearest,
        3 => FilterMode::Linear,
        x => FilterMode::Unknown(x),
    }
}

/// Decode TSC fields without collapsing legacy wrap modes into Vulkan modes.
/// Reference: envytools rnndb/graph/g80_texture.xml (GM204 TSC variants).
/// Raw words also retain optimization/reserved fields not promoted here.
pub fn decode_tsc(
    words: [u32; 8],
    resource: Option<ResourceId>,
) -> (SamplerDescriptor, DecodedSamplerState) {
    let bias = ((words[1] >> 12) & 0x1fff) as i32;
    let bias = ((bias ^ 0x1000) - 0x1000) as i16;
    (
        SamplerDescriptor {
            raw: RawDescriptor { words },
            resource,
        },
        DecodedSamplerState {
            address_u: address_mode((words[0] & 7) as u8),
            address_v: address_mode(((words[0] >> 3) & 7) as u8),
            address_w: address_mode(((words[0] >> 6) & 7) as u8),
            mag_filter: filter((words[1] & 3) as u8),
            min_filter: filter(((words[1] >> 4) & 3) as u8),
            mip_filter: mip_filter(((words[1] >> 6) & 3) as u8),
            anisotropy_raw: ((words[0] >> 20) & 7) as u8,
            min_lod_8_8: (words[2] & 0xfff) as u16,
            max_lod_8_8: ((words[2] >> 12) & 0xfff) as u16,
            lod_bias_8_8: bias,
            depth_compare: words[0] & (1 << 9) != 0,
            depth_compare_op: [
                crate::resource::CompareOp::Never,
                crate::resource::CompareOp::Less,
                crate::resource::CompareOp::Equal,
                crate::resource::CompareOp::LessOrEqual,
                crate::resource::CompareOp::Greater,
                crate::resource::CompareOp::NotEqual,
                crate::resource::CompareOp::GreaterOrEqual,
                crate::resource::CompareOp::Always,
            ][((words[0] >> 10) & 7) as usize],
            srgb_conversion: words[0] & (1 << 13) != 0,
            force_unnormalized_coords: words[1] & (1 << 25) != 0,
            reduction_mode_raw: ((words[1] >> 10) & 3) as u8,
            border_color_bits: [words[4], words[5], words[6], words[7]],
        },
    )
}

/// Split the 32-bit texture handle written through the texture constant-buffer path.
/// Lower 20 bits select TIC; upper 12 bits select TSC.
pub fn split_texture_handle(handle: u32) -> (u32, u32) {
    (handle & 0x000f_ffff, handle >> 20)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed_format(format: u32, component: u32) -> u32 {
        format | component << 7 | component << 10 | component << 13 | component << 16
    }

    #[test]
    fn direct_texture_handle_reads_stage_binding_and_live_pool_payloads() {
        use crate::{
            address_space::{GpuMapPermissions, GpuMapping},
            maxwell_state::{ConstantBufferBinding, Maxwell3dState, method},
            shader_memory::{GpuMemoryAccess, LinearGpuMemory},
        };
        let mut state = Maxwell3dState::default();
        state.raw_registers.extend([
            (method::SET_BINDLESS_TEXTURE, 3),
            (method::SET_LINKED_TSC, 0),
            (method::SET_TEX_HEADER_POOL_A, 0),
            (method::SET_TEX_HEADER_POOL_B, 0x1100),
            (method::SET_TEX_HEADER_POOL_MAXIMUM_INDEX, 1),
            (method::SET_TEX_SAMPLER_POOL_A, 0),
            (method::SET_TEX_SAMPLER_POOL_B, 0x1200),
            (method::SET_TEX_SAMPLER_POOL_MAXIMUM_INDEX, 2),
        ]);
        state.constant_buffers.insert(
            (4, 3),
            ConstantBufferBinding {
                size_raw: 16,
                address: RawGpuAddress { a: 0, b: 0x1000 },
                bind_raw: 1 | 3 << 4,
            },
        );
        let snapshot = state.draw_state();
        let mut memory = LinearGpuMemory::new(0x1000, vec![0; 0x300]);
        let mut space = GpuAddressSpace::new();
        let mapping = GpuMapping {
            guest_base: 0x1000,
            size: 0x300,
            resource: ResourceId(6),
            resource_offset: 64,
            permissions: GpuMapPermissions::READ_ONLY,
        };
        space.map(mapping).unwrap();
        memory
            .write(0x1008, &(1u32 | 2 << 20).to_le_bytes())
            .unwrap();
        memory.write(0x1120, &0x12345678u32.to_le_bytes()).unwrap();
        memory.write(0x1240, &0xabcdef01u32.to_le_bytes()).unwrap();
        let records = read_texture_handle_records(&snapshot, 4, 2, &space, &memory).unwrap();
        assert_eq!(records.handle_backing.offset, 72);
        assert_eq!(records.texture_backing.offset, 64 + 0x120);
        assert_eq!(records.sampler_backing.offset, 64 + 0x240);
        assert_eq!(records.texture.words[0], 0x12345678);
        assert_eq!(records.sampler.words[0], 0xabcdef01);
        // A guest handle write selects a different pool entry on the same snapshot.
        memory.write(0x1008, &0u32.to_le_bytes()).unwrap();
        memory.write(0x1100, &0x87654321u32.to_le_bytes()).unwrap();
        assert_eq!(
            read_texture_handle_records(&snapshot, 4, 2, &space, &memory)
                .unwrap()
                .texture
                .words[0],
            0x87654321
        );
        space.unmap(mapping.guest_base);
        space
            .map(GpuMapping {
                resource: ResourceId(7),
                ..mapping
            })
            .unwrap();
        assert_eq!(
            read_texture_handle_records(&snapshot, 4, 2, &space, &memory)
                .unwrap()
                .texture_backing
                .resource,
            ResourceId(7)
        );
        assert!(matches!(
            read_texture_handle_records(&snapshot, 0, 2, &space, &memory),
            Err(DescriptorPoolError::ConstantBuffer(_))
        ));
        assert!(matches!(
            read_texture_handle_records(&snapshot, 4, 4, &space, &memory),
            Err(DescriptorPoolError::ConstantBuffer(_))
        ));
        state.raw_registers.insert(method::SET_LINKED_TSC, 1);
        assert!(matches!(
            read_texture_handle_records(&state.draw_state(), 4, 2, &space, &memory),
            Err(DescriptorPoolError::UnsupportedHandleConfiguration)
        ));
    }

    #[test]
    fn tic_storage_format_preserves_numeric_type_instead_of_guessing_unorm() {
        for (format, component, expected) in [
            (0x08, 2, TextureFormat::Rgba8Unorm),
            (0x1d, 2, TextureFormat::R8Unorm),
            (0x1d, 3, TextureFormat::R8Sint),
            (0x1d, 4, TextureFormat::R8Uint),
            (0x0f, 7, TextureFormat::R32Float),
            (0x0f, 3, TextureFormat::R32Sint),
            (0x0f, 4, TextureFormat::R32Uint),
            (0x03, 7, TextureFormat::Rgba16Float),
            (0x24, 2, TextureFormat::Bc1RgbaUnorm),
            (0x27, 1, TextureFormat::Bc4Snorm),
            (0x28, 1, TextureFormat::Bc5Snorm),
            (0x10, 7, TextureFormat::Bc6hSfloat),
            (0x40, 2, TextureFormat::Astc4x4Unorm),
        ] {
            let mut words = [0; 8];
            words[0] = typed_format(format, component);
            words[2] = 3 << 21;
            assert_eq!(decode_tic(words, None).format, expected);
            // Corrupt just one component, retaining the same storage size code.
            words[0] ^= 1 << 10;
            assert_eq!(
                decode_tic(words, None).format,
                TextureFormat::Unknown(words[0])
            );
        }
        for component in [0, 1, 3, 4, 5, 6, 7] {
            let words = [typed_format(0x08, component), 0, 2 << 21, 0, 0, 0, 0, 0];
            assert_eq!(
                decode_tic(words, None).format,
                TextureFormat::Unknown(words[0])
            );
        }
    }

    #[test]
    fn tic_header_controls_address_bits_and_buffer_width() {
        for (header, low) in [
            (0, 0x1234_567f_u32),
            (1, 0x1234_5660),
            (2, 0x1234_5660),
            (3, 0x1234_5600),
            (4, 0x1234_5600),
        ] {
            let words = [
                typed_format(0x1d, 2),
                0x1234_567f,
                header << 21 | 0xabcd,
                1,
                7,
                0,
                0,
                0,
            ];
            let descriptor = decode_tic(words, Some(ResourceId(3)));
            assert_eq!(
                descriptor.address.high_low(),
                0xabcd_00000000 | u64::from(low)
            );
            assert_eq!(
                descriptor.width,
                Some(if header == 0 { 0x10008 } else { 8 })
            );
            assert_eq!(descriptor.raw.words, words);
        }
        let mut words = [0; 8];
        words[3] = 0xffff;
        words[4] = 0xffff;
        assert_eq!(decode_tic(words, None).width, None); // 2^32 cannot fit a u32 extent.
        for header in [5, 6, 7] {
            words[2] = header << 21;
            let descriptor = decode_tic(words, Some(ResourceId(3)));
            assert_eq!(descriptor.width, None);
            assert_eq!(descriptor.height, None);
            assert_eq!(descriptor.resource, Some(ResourceId(3)));
            assert_eq!(descriptor.raw.words, words);
        }
    }

    #[test]
    fn tsc_wrap_modes_are_distinct_and_mipmap_none_is_preserved() {
        let modes = [
            AddressMode::Repeat,
            AddressMode::MirroredRepeat,
            AddressMode::ClampToEdge,
            AddressMode::ClampToBorder,
            AddressMode::Clamp,
            AddressMode::MirrorClampToEdge,
            AddressMode::MirrorClampToBorder,
            AddressMode::MirrorClamp,
        ];
        for (raw, expected) in modes.into_iter().enumerate() {
            let mut words = [0; 8];
            words[0] = raw as u32 * (1 | 1 << 3 | 1 << 6);
            words[1] = 1 | 2 << 4 | 1 << 6;
            let (_, state) = decode_tsc(words, None);
            assert_eq!(
                (state.address_u, state.address_v, state.address_w),
                (expected, expected, expected)
            );
            assert_eq!(
                (state.mag_filter, state.min_filter, state.mip_filter),
                (FilterMode::Nearest, FilterMode::Linear, FilterMode::None)
            );
        }
    }

    #[test]
    fn tsc_fixed_point_controls_and_border_bits_are_lossless() {
        for bias in [-4096_i16, -1, 0, 1, 4095] {
            let words = [
                1 << 9 | 6 << 10 | 1 << 13 | 7 << 20,
                ((bias as u32 & 0x1fff) << 12) | 1 << 25 | 2 << 10,
                0xfff << 12 | 0x123,
                0,
                0x7fc0_1234,
                0x8000_0000,
                0x3f80_0000,
                u32::MAX,
            ];
            let (raw, state) = decode_tsc(words, Some(ResourceId(4)));
            assert_eq!(raw.raw.words, words);
            assert_eq!(state.lod_bias_8_8, bias);
            assert_eq!((state.min_lod_8_8, state.max_lod_8_8), (0x123, 0xfff));
            assert_eq!(state.anisotropy_raw, 7);
            assert!(
                state.depth_compare && state.srgb_conversion && state.force_unnormalized_coords
            );
            assert_eq!(
                state.depth_compare_op,
                crate::resource::CompareOp::GreaterOrEqual
            );
            assert_eq!(state.reduction_mode_raw, 2);
            assert_eq!(
                state.border_color_bits,
                [words[4], words[5], words[6], words[7]]
            );
        }
    }

    #[test]
    fn pool_record_resolution_checks_bounds_permissions_and_live_remaps() {
        use crate::{
            address_space::{GpuMapPermissions, GpuMapping},
            maxwell_state::{Maxwell3dState, method},
        };
        let mut state = Maxwell3dState::default();
        let mut space = GpuAddressSpace::new();
        assert_eq!(
            resolve_pool_record(&state.draw_state(), false, 0, &space),
            Err(DescriptorPoolError::MissingConfiguration)
        );
        for base in [
            method::SET_TEX_HEADER_POOL_A,
            method::SET_TEX_SAMPLER_POOL_A,
        ] {
            state
                .raw_registers
                .extend([(base, 0), (base + 1, 0x1000), (base + 2, 2)]);
        }
        let snapshot = state.draw_state();
        assert!(matches!(
            resolve_pool_record(&snapshot, false, 3, &space),
            Err(DescriptorPoolError::IndexOutOfBounds {
                index: 3,
                maximum: 2
            })
        ));
        // Only the selected last record is mapped, at a nonzero backing offset.
        let mut mapping = GpuMapping {
            guest_base: 0x1040,
            size: 32,
            resource: ResourceId(7),
            resource_offset: 96,
            permissions: GpuMapPermissions::READ_ONLY,
        };
        space.map(mapping).unwrap();
        for sampler in [false, true] {
            let range = resolve_pool_record(&snapshot, sampler, 2, &space).unwrap();
            assert_eq!(
                (range.resource, range.offset, range.length),
                (ResourceId(7), 96, 32)
            );
        }
        assert!(matches!(
            resolve_pool_record(&snapshot, false, 1, &space),
            Err(DescriptorPoolError::Address(_))
        ));
        space.unmap(mapping.guest_base);
        mapping.resource = ResourceId(8);
        mapping.resource_offset = 128;
        space.map(mapping).unwrap();
        let range = resolve_pool_record(&snapshot, false, 2, &space).unwrap();
        assert_eq!((range.resource, range.offset), (ResourceId(8), 128));
        space.unmap(mapping.guest_base);
        mapping.permissions.read = false;
        space.map(mapping).unwrap();
        assert_eq!(
            resolve_pool_record(&snapshot, false, 2, &space),
            Err(DescriptorPoolError::NotReadable)
        );
        state
            .raw_registers
            .insert(method::SET_TEX_HEADER_POOL_A, u32::MAX);
        state
            .raw_registers
            .insert(method::SET_TEX_HEADER_POOL_B, u32::MAX - 15);
        assert_eq!(
            resolve_pool_record(&state.draw_state(), false, 0, &space),
            Err(DescriptorPoolError::AddressOverflow)
        );
    }

    #[test]
    fn texture_handle_splits_tic_and_tsc() {
        let (tic, tsc) = split_texture_handle(0xabc5_4321);
        assert_eq!(tic, 0x54321);
        assert_eq!(tsc, 0xabc);
    }

    #[test]
    fn pool_reads_exact_little_endian_record_and_rejects_partial_provider_data() {
        use crate::{
            address_space::{GpuMapPermissions, GpuMapping},
            maxwell_state::{Maxwell3dState, method},
        };
        struct Memory(usize);
        impl GpuMemory for Memory {
            fn read(&self, address: u64, size: usize) -> Result<Vec<u8>, GpuMemoryError> {
                assert_eq!((address, size), (0x2020, 32));
                Ok((0..self.0).map(|i| i as u8).collect())
            }
        }
        let mut state = Maxwell3dState::default();
        let base = method::SET_TEX_HEADER_POOL_A;
        state
            .raw_registers
            .extend([(base, 0), (base + 1, 0x2000), (base + 2, 1)]);
        let mut space = GpuAddressSpace::new();
        space
            .map(GpuMapping {
                guest_base: 0x2020,
                size: 32,
                resource: ResourceId(9),
                resource_offset: 64,
                permissions: GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        let (range, raw) =
            read_pool_record(&state.draw_state(), false, 1, &space, &Memory(32)).unwrap();
        assert_eq!(range.resource, ResourceId(9));
        assert_eq!(raw.words[0], 0x0302_0100);
        assert_eq!(raw.words[7], 0x1f1e_1d1c);
        for length in [0, 31, 33] {
            assert!(matches!(
                read_pool_record(&state.draw_state(), false, 1, &space, &Memory(length)),
                Err(DescriptorPoolError::Memory(
                    GpuMemoryError::BackendRead { .. }
                ))
            ));
        }
    }

    #[test]
    fn raw_tic_is_preserved() {
        let words = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(decode_tic(words, None).raw.words, words);
    }
}
