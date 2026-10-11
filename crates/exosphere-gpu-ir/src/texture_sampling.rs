// SPDX-License-Identifier: GPL-3.0-or-later
//! Joint preparation of a float 2D TIC/TSC pair for canonical sampling.

use crate::shader::TextureSampleChannel;
use crate::{
    FilterMode, GpuAddressSpace, ResourceId, SamplerAddressMode, SamplerBorderColor, SamplerDesc,
    SamplerMipFilter, SamplerState, TextureDesc, TextureFormat, TextureLayout,
    texture_backing::{TextureBacking, TextureBackingError, resolve_texture_backing},
    tic_tsc::{AddressMode, FilterMode as MaxwellFilter, decode_tsc},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureSamplingError {
    Backing(TextureBackingError),
    IdentityCollision,
    UnsupportedViewControls,
    UnsupportedFormat,
    UnsupportedSwizzle,
    UnsupportedSamplerControls,
    UnsupportedBorderColor,
    InvalidLodRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTexture2d {
    pub backing: TextureBacking,
    /// Preserve the sampler controls that participated in view preparation.
    pub sampler_raw: crate::RawDescriptor<8>,
    /// Payload is supplied by TextureResourceData as packed rows, regardless of
    /// guest storage. The image ID is reserved by the renderer, not its VA.
    pub image: TextureDesc,
    pub sampler: SamplerDesc,
    /// TIC output channel -> storage component or float constant. Apply this mapping to
    /// TEX's selected components before emitting SampleTexture2d.
    pub channels: [TextureSampleChannel; 4],
}

/// sRGB is a sampling-view interpretation, not a different storage footprint.
/// Require the independently enabled TSC conversion for a declared sRGB TIC;
/// the disabled-conversion case still needs a verified Maxwell policy.
pub(crate) fn sampled_view_format(
    tic: [u32; 8],
    tsc: [u32; 8],
) -> Result<TextureFormat, TextureSamplingError> {
    use TextureFormat as F;
    let storage = crate::tic_tsc::decode_tic(tic, None).format;
    if tic[4] & (1 << 22) == 0 {
        return Ok(storage);
    }
    if tsc[0] & (1 << 13) == 0 {
        return Err(TextureSamplingError::UnsupportedViewControls);
    }
    Ok(match storage {
        F::Rgba8Unorm => F::Rgba8Srgb,
        F::Bgra8Unorm => F::Bgra8Srgb,
        F::Bc1RgbaUnorm => F::Bc1RgbaSrgb,
        F::Bc2RgbaUnorm => F::Bc2RgbaSrgb,
        F::Bc3RgbaUnorm => F::Bc3RgbaSrgb,
        F::Bc7RgbaUnorm => F::Bc7RgbaSrgb,
        _ => return Err(TextureSamplingError::UnsupportedFormat),
    })
}

pub struct TextureShaderInterface<'a> {
    pub selector: u16,
    pub image: [u32; 2],
    pub sampler: [u32; 2],
    pub view: &'a PreparedTexture2d,
}

/// Replace direct TEX handles with explicit descriptors and apply TIC channel
/// selection after TEX's output mask. Shader identity and predicate structure
/// are preserved; this pass does not claim backend support for predication.
pub fn lower_texture_interface(
    shader: &crate::ShaderModule,
    interface: &[TextureShaderInterface<'_>],
) -> Result<crate::ShaderModule, crate::spirv::SpirvCompileError> {
    use crate::{ShaderOperation as S, spirv::SpirvCompileError as E};
    use std::collections::{BTreeMap, BTreeSet};
    let mut selectors = BTreeMap::new();
    let mut bindings = BTreeSet::new();
    for entry in interface {
        if entry.selector > 0x1fff
            || selectors.insert(entry.selector, entry).is_some()
            || !bindings.insert(entry.image)
            || !bindings.insert(entry.sampler)
            || entry
                .view
                .channels
                .iter()
                .any(|channel| matches!(channel, TextureSampleChannel::Component(4..)))
        {
            return Err(E::UnsupportedOperation);
        }
    }
    fn lower(
        operations: &[S],
        selectors: &BTreeMap<u16, &TextureShaderInterface<'_>>,
        bindings: &BTreeSet<[u32; 2]>,
        used: &mut BTreeSet<u16>,
    ) -> Result<Vec<S>, E> {
        operations
            .iter()
            .map(|operation| {
                Ok(match operation {
                    S::SampleTexture2dHandle {
                        dst,
                        handle_selector,
                        coords,
                        components,
                        level_zero,
                    } => {
                        let entry = selectors
                            .get(handle_selector)
                            .ok_or(E::UnsupportedOperation)?;
                        if dst.is_empty()
                            || dst.len() > 4
                            || dst.len() != components.len()
                            || components.iter().any(|component| *component >= 4)
                            || dst.iter().collect::<BTreeSet<_>>().len() != dst.len()
                        {
                            return Err(E::UnsupportedOperation);
                        }
                        used.insert(*handle_selector);
                        S::SampleTexture2d {
                            dst: dst.clone(),
                            image: entry.image,
                            sampler: entry.sampler,
                            coords: *coords,
                            components: components
                                .iter()
                                .map(|component| entry.view.channels[usize::from(*component)])
                                .collect(),
                            level_zero: *level_zero,
                        }
                    }
                    S::Predicated {
                        predicate,
                        negated,
                        operations,
                    } => S::Predicated {
                        predicate: *predicate,
                        negated: *negated,
                        operations: lower(operations, selectors, bindings, used)?,
                    },
                    S::LoadUniformWord { set, binding, .. }
                        if bindings.contains(&[*set, *binding]) =>
                    {
                        return Err(E::UnsupportedOperation);
                    }
                    // Already resolved descriptors cannot be validated against a new
                    // handle interface. Do not accidentally reuse stale view bindings.
                    S::SampleTexture2d { .. } => return Err(E::UnsupportedOperation),
                    _ => operation.clone(),
                })
            })
            .collect()
    }
    let mut used = BTreeSet::new();
    let operations = lower(&shader.operations, &selectors, &bindings, &mut used)?;
    if used.len() != interface.len()
        || (!used.is_empty() && shader.stage != crate::ShaderStage::Fragment)
    {
        return Err(E::UnsupportedOperation);
    }
    let mut resolved = shader.clone();
    resolved.operations = operations;
    Ok(resolved)
}

/// Prepare only semantics represented by the current float SampleTexture2d IR.
/// No depth/integer reinterpretation or implicit swizzle/sRGB defaults are used.
/// Field references: deko3d Maxwell texture_{image,sampler}_control_block.h and
/// tsc_generate.cpp. Device limits remain the native backend's responsibility.
pub fn prepare_texture_2d(
    tic: [u32; 8],
    tsc: [u32; 8],
    image_id: ResourceId,
    sampler_id: ResourceId,
    space: &GpuAddressSpace,
) -> Result<PreparedTexture2d, TextureSamplingError> {
    use TextureSamplingError as E;
    let backing = resolve_texture_backing(tic, space).map_err(E::Backing)?;
    if image_id == sampler_id
        || image_id == backing.range.resource
        || sampler_id == backing.range.resource
    {
        return Err(E::IdentityCollision);
    }
    // Header-controlled filtering, depth, packed components, unnormalized
    // coordinates and TIC LOD controls need additional lowering. Word 6 cannot
    // be dropped simply because the TSC also contains a LOD bias.
    if tic[0] >> 31 != 0
        || tic[3] & ((1 << 26) | (1 << 27)) != 0
        || tic[5] & (1 << 31) == 0
        || tic[6] != 0
        || tic[7] & !0xfff != 0
    {
        return Err(E::UnsupportedViewControls);
    }
    if matches!(
        backing.descriptor.format,
        TextureFormat::R8Uint
            | TextureFormat::R8Sint
            | TextureFormat::Rg8Uint
            | TextureFormat::Rg8Sint
            | TextureFormat::R32Uint
            | TextureFormat::R32Sint
            | TextureFormat::D16Unorm
            | TextureFormat::D24UnormS8
            | TextureFormat::D32Float
            | TextureFormat::D32FloatS8
            | TextureFormat::Unknown(_)
    ) {
        return Err(E::UnsupportedFormat);
    }
    let mut channels = [TextureSampleChannel::Zero; 4];
    for (i, channel) in channels.iter_mut().enumerate() {
        let raw = (tic[0] >> (19 + i * 3)) & 7;
        *channel = match raw {
            0 => TextureSampleChannel::Zero,
            2..=5 => TextureSampleChannel::Component((raw - 2) as u8),
            7 => TextureSampleChannel::One,
            // Encoding 1 is reserved; OneInt is not a floating-point one.
            _ => return Err(E::UnsupportedSwizzle),
        };
    }
    let (_, state) = decode_tsc(tsc, None);
    let format = sampled_view_format(tic, tsc)?;
    let font_width = (tsc[0] >> 14) & 7;
    let font_height = (tsc[0] >> 17) & 7;
    // Ordinary TEX is not a font instruction. Accept reset/unit font sizes and
    // cubemap-only controls (irrelevant to this verified non-cube view), but no
    // reduction, depth comparison, coordinate override or trilinear snapping.
    if tsc[0] & 0xff80_0000 != 0
        || font_width > 1
        || font_height > 1
        || tsc[1] & 0xfe00_000c != 0
        || state.depth_compare
        || state.reduction_mode_raw != 0
        || tsc[3] & 0xf000_0fff != 0
    {
        return Err(E::UnsupportedSamplerControls);
    }
    let filter = |value| match value {
        MaxwellFilter::Nearest => Ok(FilterMode::Nearest),
        MaxwellFilter::Linear => Ok(FilterMode::Linear),
        _ => Err(E::UnsupportedSamplerControls),
    };
    let address = |value| {
        Ok(match value {
            AddressMode::Repeat => SamplerAddressMode::Repeat,
            AddressMode::MirroredRepeat => SamplerAddressMode::MirroredRepeat,
            AddressMode::ClampToEdge => SamplerAddressMode::ClampToEdge,
            AddressMode::ClampToBorder => SamplerAddressMode::ClampToBorder,
            AddressMode::Clamp => SamplerAddressMode::Clamp,
            AddressMode::MirrorClampToEdge => SamplerAddressMode::MirrorClampToEdge,
            AddressMode::MirrorClampToBorder => SamplerAddressMode::MirrorClampToBorder,
            AddressMode::MirrorClamp => SamplerAddressMode::MirrorClamp,
            AddressMode::Unknown(_) => return Err(E::UnsupportedSamplerControls),
        })
    };
    let mip_filter = match state.mip_filter {
        MaxwellFilter::None => SamplerMipFilter::None,
        MaxwellFilter::Nearest => SamplerMipFilter::Nearest,
        MaxwellFilter::Linear => SamplerMipFilter::Linear,
        MaxwellFilter::Unknown(_) => return Err(E::UnsupportedSamplerControls),
    };
    if state.min_lod_8_8 > state.max_lod_8_8
        || (mip_filter == SamplerMipFilter::None
            && (state.min_lod_8_8 != 0 || state.max_lod_8_8 != 0))
    {
        return Err(E::InvalidLodRange);
    }
    let one = 1.0_f32.to_bits();
    let srgb_border = tic[4] & (1 << 22) != 0
        && [state.address_u, state.address_v].contains(&AddressMode::ClampToBorder);
    let border_bits = if srgb_border {
        // The TSC contains separate encoded RGB border bytes for sRGB TICs.
        // Alpha remains the ordinary float word; do not gamma-convert it.
        let encoded = [tsc[2] >> 24, (tsc[3] >> 12) & 255, (tsc[3] >> 20) & 255];
        let rgb = match encoded {
            [0, 0, 0] => [0; 3],
            [255, 255, 255] => [one; 3],
            _ => return Err(E::UnsupportedBorderColor),
        };
        [rgb[0], rgb[1], rgb[2], state.border_color_bits[3]]
    } else {
        state.border_color_bits
    };
    let border = match border_bits {
        [0, 0, 0, 0] => SamplerBorderColor::FloatTransparentBlack,
        [0, 0, 0, a] if a == one => SamplerBorderColor::FloatOpaqueBlack,
        [r, g, b, a] if [r, g, b, a] == [one; 4] => SamplerBorderColor::FloatOpaqueWhite,
        _ => return Err(E::UnsupportedBorderColor),
    };
    let image = TextureDesc {
        id: image_id,
        width: backing.descriptor.width.unwrap(),
        height: backing.descriptor.height.unwrap(),
        depth: 1,
        layers: 1,
        mip_levels: backing.descriptor.mip_levels.unwrap(),
        samples: 1,
        format,
        layout: TextureLayout::Linear,
    };
    let sampler = SamplerDesc {
        id: sampler_id,
        state: SamplerState {
            min_filter: filter(state.min_filter)?,
            mag_filter: filter(state.mag_filter)?,
            mip_filter,
            address: [
                address(state.address_u)?,
                address(state.address_v)?,
                address(state.address_w)?,
            ],
            min_lod_8_8: state.min_lod_8_8,
            max_lod_8_8: state.max_lod_8_8,
            lod_bias_8_8: state.lod_bias_8_8,
            max_anisotropy: [1, 2, 4, 6, 8, 10, 12, 16][usize::from(state.anisotropy_raw)],
            compare: None,
            border,
            unnormalized_coordinates: false,
        },
    };
    Ok(PreparedTexture2d {
        backing,
        sampler_raw: crate::RawDescriptor { words: tsc },
        image,
        sampler,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GpuMapPermissions, GpuMapping};

    fn fixture() -> ([u32; 8], [u32; 8], GpuAddressSpace) {
        let tic = [
            0x08 | 2 << 7 | 2 << 10 | 2 << 13 | 2 << 16 | 4 << 19 | 2 << 22 | 3 << 25 | 5 << 28,
            0x1000,
            2 << 21,
            1,
            1 | 1 << 23 | 7 << 29,
            1 | 1 << 31,
            0,
            0,
        ];
        let tsc = [
            2 | 2 << 3 | 2 << 6 | 1 << 13 | 1 << 14 | 1 << 17,
            1 | 2 << 4 | 1 << 6 | 1 << 8 | 1 << 9,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let mut space = GpuAddressSpace::new();
        space
            .map(GpuMapping {
                guest_base: 0x1000,
                size: 512,
                resource: ResourceId(3),
                resource_offset: 64,
                permissions: GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        (tic, tsc, space)
    }

    #[test]
    fn srgb_view_interpretation_preserves_storage_and_requires_sampler_conversion() {
        let (mut tic, mut tsc, space) = fixture();
        tic[4] |= 1 << 22;
        for (raw, storage, sampled) in [
            (0x08, TextureFormat::Rgba8Unorm, TextureFormat::Rgba8Srgb),
            (
                0x24,
                TextureFormat::Bc1RgbaUnorm,
                TextureFormat::Bc1RgbaSrgb,
            ),
            (
                0x25,
                TextureFormat::Bc2RgbaUnorm,
                TextureFormat::Bc2RgbaSrgb,
            ),
            (
                0x26,
                TextureFormat::Bc3RgbaUnorm,
                TextureFormat::Bc3RgbaSrgb,
            ),
            (
                0x17,
                TextureFormat::Bc7RgbaUnorm,
                TextureFormat::Bc7RgbaSrgb,
            ),
        ] {
            tic[0] = (tic[0] & !0x7f) | raw;
            let view = prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space).unwrap();
            assert_eq!(view.backing.descriptor.format, storage);
            assert_eq!(view.image.format, sampled);
            assert_eq!(view.sampler_raw.words, tsc);
        }
        tsc[0] &= !(1 << 13);
        assert_eq!(
            prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space),
            Err(TextureSamplingError::UnsupportedViewControls)
        );
        tic[4] &= !(1 << 22);
        assert!(prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space).is_ok());
        tic[4] |= 1 << 22;
        tsc[0] |= 1 << 13;
        tic[0] = (tic[0] & !0x7f) | 0x1d;
        assert_eq!(
            prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space),
            Err(TextureSamplingError::UnsupportedFormat)
        );
    }

    #[test]
    fn srgb_border_uses_encoded_rgb_and_unconverted_float_alpha() {
        let (mut tic, mut tsc, space) = fixture();
        tic[4] |= 1 << 22;
        tsc[0] = (tsc[0] & !7) | 3;
        tsc[2] |= 255 << 24;
        tsc[3] = 255 << 12 | 255 << 20;
        tsc[4..7].fill(0.5_f32.to_bits()); // Not the active border RGB source.
        tsc[7] = 1.0_f32.to_bits();
        let view = prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space).unwrap();
        assert_eq!(
            view.sampler.state.border,
            SamplerBorderColor::FloatOpaqueWhite
        );
        tsc[2] = (tsc[2] & 0xffffff) | 128 << 24;
        assert_eq!(
            prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space),
            Err(TextureSamplingError::UnsupportedBorderColor)
        );
        tsc[2] &= 0xffffff;
        tsc[3] = 0;
        tsc[7] = 0;
        let view = prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space).unwrap();
        assert_eq!(
            view.sampler.state.border,
            SamplerBorderColor::FloatTransparentBlack
        );
    }

    #[test]
    fn float_constant_swizzles_survive_sparse_tex_channel_selection() {
        use crate::{ShaderModule, ShaderOperation as S, ShaderStage};
        let (mut tic, tsc, space) = fixture();
        // [R, zero, one-float, A], selecting only zero and one from the TEX.
        tic[0] = (tic[0] & 0x7ffff) | 2 << 19 | 7 << 25 | 5 << 28;
        let view = prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space).unwrap();
        assert_eq!(
            view.channels,
            [
                TextureSampleChannel::Component(0),
                TextureSampleChannel::Zero,
                TextureSampleChannel::One,
                TextureSampleChannel::Component(3)
            ]
        );
        let shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Fragment,
            variant_key: vec![],
            operations: vec![S::SampleTexture2dHandle {
                dst: vec![8, 9],
                handle_selector: 7,
                coords: [8, 9],
                components: vec![1, 2],
                level_zero: false,
            }],
        };
        let lowered = lower_texture_interface(
            &shader,
            &[TextureShaderInterface {
                selector: 7,
                image: [1, 4],
                sampler: [1, 5],
                view: &view,
            }],
        )
        .unwrap();
        assert_eq!(
            lowered.operations,
            vec![S::SampleTexture2d {
                dst: vec![8, 9],
                image: [1, 4],
                sampler: [1, 5],
                coords: [8, 9],
                components: vec![TextureSampleChannel::Zero, TextureSampleChannel::One],
                level_zero: false
            }]
        );
    }

    #[test]
    fn handle_lowering_applies_view_swizzles_after_channel_selection() {
        use crate::{ShaderModule, ShaderOperation as S, ShaderStage};
        let (tic, tsc, space) = fixture();
        let view = prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space).unwrap();
        let interface = [TextureShaderInterface {
            selector: 7,
            image: [1, 4],
            sampler: [1, 5],
            view: &view,
        }];
        let operation = S::SampleTexture2dHandle {
            dst: vec![8, 9],
            handle_selector: 7,
            coords: [8, 9],
            components: vec![1, 3],
            level_zero: true,
        };
        let shader = ShaderModule {
            id: 89,
            stage: ShaderStage::Fragment,
            variant_key: vec![1, 2],
            operations: vec![S::Predicated {
                predicate: 2,
                negated: true,
                operations: vec![operation.clone()],
            }],
        };
        let lowered = lower_texture_interface(&shader, &interface).unwrap();
        assert_eq!(lowered.id, shader.id);
        assert_eq!(lowered.variant_key, shader.variant_key);
        assert_eq!(
            lowered.operations,
            vec![S::Predicated {
                predicate: 2,
                negated: true,
                operations: vec![S::SampleTexture2d {
                    dst: vec![8, 9],
                    image: [1, 4],
                    sampler: [1, 5],
                    coords: [8, 9],
                    components: vec![
                        TextureSampleChannel::Component(0),
                        TextureSampleChannel::Component(3)
                    ],
                    level_zero: true
                }]
            }]
        );
        assert!(lower_texture_interface(&shader, &[]).is_err());
        let mut invalid = shader.clone();
        invalid.stage = ShaderStage::Vertex;
        assert!(lower_texture_interface(&invalid, &interface).is_err());
        invalid = shader.clone();
        invalid.operations.push(S::LoadUniformWord {
            dst: 0,
            set: 1,
            binding: 4,
            byte_offset: 0,
            byte_size: 16,
        });
        assert!(lower_texture_interface(&invalid, &interface).is_err());
        invalid.operations = vec![operation];
        let collision = [TextureShaderInterface {
            selector: 7,
            image: [1, 4],
            sampler: [1, 4],
            view: &view,
        }];
        assert!(lower_texture_interface(&invalid, &collision).is_err());
        invalid.operations.clear();
        assert!(lower_texture_interface(&invalid, &interface).is_err());
    }

    #[test]
    fn joint_preparation_retains_channels_filters_and_fixed_point_state() {
        let (tic, mut tsc, space) = fixture();
        let prepared = prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space).unwrap();
        assert_eq!(
            prepared.channels,
            [2, 0, 1, 3].map(TextureSampleChannel::Component)
        );
        assert_eq!(prepared.image.layout, TextureLayout::Linear);
        assert_eq!(prepared.backing.range.offset, 64);
        assert_eq!(prepared.sampler.state.min_filter, FilterMode::Linear);
        assert_eq!(prepared.sampler.state.mag_filter, FilterMode::Nearest);
        assert_eq!(prepared.sampler.state.mip_filter, SamplerMipFilter::None);
        tsc[1] = (tsc[1] & !(3 << 6)) | 3 << 6 | 0x1fc0 << 12;
        tsc[2] = 128 | 1024 << 12;
        for (raw, anisotropy) in [1, 2, 4, 6, 8, 10, 12, 16].into_iter().enumerate() {
            tsc[0] = (tsc[0] & !(7 << 20)) | (raw as u32) << 20;
            let state = prepare_texture_2d(tic, tsc, ResourceId(1), ResourceId(2), &space)
                .unwrap()
                .sampler
                .state;
            assert_eq!(state.max_anisotropy, anisotropy);
            assert_eq!(
                (state.min_lod_8_8, state.max_lod_8_8, state.lod_bias_8_8),
                (128, 1024, -64)
            );
        }
    }

    #[test]
    fn unsupported_joint_controls_and_identity_collisions_fail_explicitly() {
        let (tic, tsc, space) = fixture();
        for (word, mask) in [
            (0, 1 << 31),
            (3, 1 << 26),
            (3, 1 << 27),
            (6, 1),
            (7, 1 << 12),
        ] {
            let mut invalid = tic;
            invalid[word] |= mask;
            assert!(
                prepare_texture_2d(invalid, tsc, ResourceId(1), ResourceId(2), &space).is_err()
            );
        }
        for (word, mask) in [
            (0, 1 << 9),
            (0, 1 << 23),
            (0, 2 << 14),
            (1, 1 << 2),
            (1, 1 << 10),
            (1, 1 << 25),
            (1, 1 << 26),
            (3, 1),
            (3, 1 << 28),
        ] {
            let mut invalid = tsc;
            invalid[word] |= mask;
            assert!(
                prepare_texture_2d(tic, invalid, ResourceId(1), ResourceId(2), &space).is_err()
            );
        }
        for (image, sampler) in [(1, 1), (3, 2), (1, 3)] {
            assert_eq!(
                prepare_texture_2d(tic, tsc, ResourceId(image), ResourceId(sampler), &space),
                Err(TextureSamplingError::IdentityCollision)
            );
        }
        for swizzle in [1, 6] {
            let mut invalid = tic;
            invalid[0] = (invalid[0] & !(7 << 19)) | swizzle << 19;
            assert_eq!(
                prepare_texture_2d(invalid, tsc, ResourceId(1), ResourceId(2), &space),
                Err(TextureSamplingError::UnsupportedSwizzle)
            );
        }
        let mut invalid = tsc;
        invalid[4] = 0.5_f32.to_bits();
        assert_eq!(
            prepare_texture_2d(tic, invalid, ResourceId(1), ResourceId(2), &space),
            Err(TextureSamplingError::UnsupportedBorderColor)
        );
        invalid = tsc;
        invalid[2] = 1;
        assert_eq!(
            prepare_texture_2d(tic, invalid, ResourceId(1), ResourceId(2), &space),
            Err(TextureSamplingError::InvalidLodRange)
        );
    }
}
