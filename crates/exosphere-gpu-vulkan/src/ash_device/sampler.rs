// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated canonical sampler state -> Vulkan creation parameters.

use crate::{VulkanError, vk_convert};
use ash::vk;
use exosphere_gpu_ir::{
    FilterMode, SamplerAddressMode, SamplerBorderColor, SamplerMipFilter, SamplerState,
};

fn address(value: SamplerAddressMode) -> Result<vk::SamplerAddressMode, VulkanError> {
    Ok(match value {
        SamplerAddressMode::Repeat => vk::SamplerAddressMode::REPEAT,
        SamplerAddressMode::MirroredRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
        SamplerAddressMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        SamplerAddressMode::ClampToBorder => vk::SamplerAddressMode::CLAMP_TO_BORDER,
        // Legacy clamp modes require shader emulation; mirror-clamp-to-edge
        // requires an optional Vulkan feature not currently enabled.
        _ => return Err(VulkanError::UnsupportedFeature("sampler address mode")),
    })
}

fn filter(value: FilterMode) -> vk::Filter {
    match value {
        FilterMode::Nearest => vk::Filter::NEAREST,
        FilterMode::Linear => vk::Filter::LINEAR,
    }
}

pub(super) fn create_info(
    state: &SamplerState,
    limits: &vk::PhysicalDeviceLimits,
    anisotropy_enabled: bool,
) -> Result<vk::SamplerCreateInfo<'static>, VulkanError> {
    if state.min_lod_8_8 > state.max_lod_8_8
        || (f32::from(state.lod_bias_8_8) / 256.0).abs() > limits.max_sampler_lod_bias
        || state.max_anisotropy == 0
        || (state.max_anisotropy > 1
            && (!anisotropy_enabled
                || f32::from(state.max_anisotropy) > limits.max_sampler_anisotropy))
    {
        return Err(VulkanError::UnsupportedFeature(
            "sampler LOD/anisotropy limits",
        ));
    }
    let mut min_lod = f32::from(state.min_lod_8_8) / 256.0;
    let mut max_lod = f32::from(state.max_lod_8_8) / 256.0;
    let mip_mode = match state.mip_filter {
        SamplerMipFilter::Nearest => vk::SamplerMipmapMode::NEAREST,
        SamplerMipFilter::Linear => vk::SamplerMipmapMode::LINEAR,
        SamplerMipFilter::None => {
            // Vulkan's documented non-mipmap emulation keeps minification
            // distinct from magnification while rounding to the base level.
            // maxLod=0 would incorrectly force magFilter when filters differ.
            // Nonzero guest clamps require separate Maxwell policy validation.
            if state.min_lod_8_8 != 0 || state.max_lod_8_8 != 0 {
                return Err(VulkanError::UnsupportedFeature(
                    "non-mipmap sampler LOD clamps",
                ));
            }
            min_lod = 0.0;
            max_lod = if state.min_filter == state.mag_filter {
                0.0
            } else {
                0.25
            };
            vk::SamplerMipmapMode::NEAREST
        }
    };
    if state.unnormalized_coordinates
        && (state.min_filter != state.mag_filter
            || state.mip_filter == SamplerMipFilter::Linear
            || min_lod != 0.0
            || max_lod != 0.0
            || state.max_anisotropy != 1
            || state.compare.is_some()
            || state.address[..2].iter().any(|mode| {
                !matches!(
                    mode,
                    SamplerAddressMode::ClampToEdge | SamplerAddressMode::ClampToBorder
                )
            }))
    {
        return Err(VulkanError::UnsupportedFeature(
            "unnormalized sampler restrictions",
        ));
    }
    let compare = vk_convert::compare(state.compare.unwrap_or(exosphere_gpu_ir::CompareOp::Always))
        .ok_or(VulkanError::UnsupportedFeature("sampler compare operation"))?;
    let border = match state.border {
        SamplerBorderColor::FloatTransparentBlack => vk::BorderColor::FLOAT_TRANSPARENT_BLACK,
        SamplerBorderColor::FloatOpaqueBlack => vk::BorderColor::FLOAT_OPAQUE_BLACK,
        SamplerBorderColor::FloatOpaqueWhite => vk::BorderColor::FLOAT_OPAQUE_WHITE,
        SamplerBorderColor::IntTransparentBlack => vk::BorderColor::INT_TRANSPARENT_BLACK,
        SamplerBorderColor::IntOpaqueBlack => vk::BorderColor::INT_OPAQUE_BLACK,
        SamplerBorderColor::IntOpaqueWhite => vk::BorderColor::INT_OPAQUE_WHITE,
    };
    Ok(vk::SamplerCreateInfo::default()
        .min_filter(filter(state.min_filter))
        .mag_filter(filter(state.mag_filter))
        .mipmap_mode(mip_mode)
        .address_mode_u(address(state.address[0])?)
        .address_mode_v(address(state.address[1])?)
        .address_mode_w(address(state.address[2])?)
        .mip_lod_bias(f32::from(state.lod_bias_8_8) / 256.0)
        .min_lod(min_lod)
        .max_lod(max_lod)
        .anisotropy_enable(state.max_anisotropy > 1)
        .max_anisotropy(f32::from(state.max_anisotropy))
        .compare_enable(state.compare.is_some())
        .compare_op(compare)
        .border_color(border)
        .unnormalized_coordinates(state.unnormalized_coordinates))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> SamplerState {
        SamplerState {
            min_filter: FilterMode::Nearest,
            mag_filter: FilterMode::Linear,
            mip_filter: SamplerMipFilter::Linear,
            address: [
                SamplerAddressMode::Repeat,
                SamplerAddressMode::MirroredRepeat,
                SamplerAddressMode::ClampToBorder,
            ],
            min_lod_8_8: 128,
            max_lod_8_8: 1024,
            lod_bias_8_8: -64,
            max_anisotropy: 4,
            compare: Some(exosphere_gpu_ir::CompareOp::LessOrEqual),
            border: SamplerBorderColor::FloatOpaqueWhite,
            unnormalized_coordinates: false,
        }
    }
    fn limits() -> vk::PhysicalDeviceLimits {
        vk::PhysicalDeviceLimits {
            max_sampler_lod_bias: 2.0,
            max_sampler_anisotropy: 8.0,
            ..Default::default()
        }
    }
    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_2d_sampling_observes_filters_channels_and_texture_refresh() {
        verify_2d_sampling(false, MipSampling::SingleLevel);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_bc1_sampling_observes_channels_and_texture_refresh() {
        verify_2d_sampling(true, MipSampling::SingleLevel);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_mip_sampling_reads_live_tiled_levels() {
        for mode in [
            MipSampling::NearestLevelOne,
            MipSampling::BlendHalf,
            MipSampling::ImplicitGradient,
        ] {
            verify_2d_sampling(false, mode);
            verify_2d_sampling(true, mode);
        }
    }

    #[derive(Clone, Copy)]
    enum MipSampling {
        SingleLevel,
        NearestLevelOne,
        BlendHalf,
        ImplicitGradient,
    }

    fn verify_2d_sampling(compressed: bool, mip_mode: MipSampling) {
        let mipped = !matches!(mip_mode, MipSampling::SingleLevel);
        let automatic = matches!(mip_mode, MipSampling::ImplicitGradient);
        let (mip_filter, min_lod, max_lod) = match mip_mode {
            MipSampling::SingleLevel => (1, 0, 0),
            MipSampling::NearestLevelOne => (2, 256, 256),
            MipSampling::BlendHalf => (3, 128, 128),
            MipSampling::ImplicitGradient => (2, 0, 256),
        };
        use super::super::AshVulkanDevice;
        use crate::device::VulkanDevice;
        use exosphere_gpu_ir::{
            Command, GpuModule, InMemoryGpuResources, ResourceId, ShaderOperation as S,
        };
        let mut device = AshVulkanDevice::create().unwrap();
        if compressed && device.core_features.texture_compression_bc != vk::TRUE {
            assert!(
                super::super::image_usage::validate_compression(
                    vk::Format::BC1_RGBA_UNORM_BLOCK,
                    &device.core_features,
                )
                .is_err()
            );
            eprintln!("BC1 pixel verification unavailable: device has no BC compression feature");
            return;
        }
        let backend = crate::VulkanBackend::new();
        let baseline = GpuModule::triangle_vulkan_smoke_test(32, 32);
        let target = baseline.textures[0].id;
        let mut resources = InMemoryGpuResources::new();
        device
            .execute_plan_with_resources(&backend.plan(&baseline).unwrap(), &mut resources)
            .unwrap();
        let reference = device.readback_rgba8(target).unwrap();
        let coverage: Vec<_> = reference
            .chunks_exact(4)
            .map(|pixel| pixel[..3] != [0, 0, 0])
            .collect();
        assert!(coverage.iter().any(|covered| *covered));
        let source = ResourceId(900);
        let mut header_bytes = vec![0; exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE];
        header_bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        header_bytes.extend(
            [0u64, 0xe30000000007000f]
                .into_iter()
                .flat_map(u64::to_le_bytes),
        );
        let header = exosphere_gpu_ir::maxwell_shader::parse_graphics(
            exosphere_gpu_ir::ShaderStage::Fragment,
            &header_bytes,
        )
        .unwrap()
        .header;
        let allocation = ResourceId(903);
        let mut space = exosphere_gpu_ir::GpuAddressSpace::new();
        space
            .map(exosphere_gpu_ir::GpuMapping {
                guest_base: 0x1000,
                size: if mipped { 1024 } else { 512 },
                resource: allocation,
                resource_offset: 64,
                permissions: exosphere_gpu_ir::GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        let tic = |header: u32| {
            [
                (if compressed { 0x24 } else { 0x08 })
                    | 2 << 7
                    | 2 << 10
                    | 2 << 13
                    | 2 << 16
                    | 4 << 19
                    | 2 << 22
                    | 3 << 25
                    | 5 << 28,
                0x1000,
                header << 21,
                if header == 2 {
                    1
                } else if mipped {
                    1 << 28
                } else {
                    0
                },
                1 | 1 << 23 | 7 << 29,
                1 | 1 << 31,
                0,
                if mipped { 1 << 4 } else { 0 },
            ]
        };
        let mut ordinal = 0;
        for linear in [false, true] {
            for level_zero in [false, true] {
                if automatic && level_zero {
                    continue;
                }
                for coordinate in [0.25, 0.5] {
                    let mut module = baseline.clone();
                    if automatic {
                        use exosphere_gpu_ir::{
                            BufferDesc, BufferUsage, VertexAttributeDesc, VertexBindingDesc,
                            VertexFormat, VertexInputRate, VertexInputState,
                        };
                        let vertices = ResourceId(904);
                        // Same triangle as the reference. UV spans yield rho=2
                        // or 4 for the 2x2 source: unclamped LOD 1 or 2. Only
                        // maxLOD=1 constrains the latter; minLOD remains zero.
                        let span = if coordinate == 0.25 { 32.0f32 } else { 64.0 };
                        let data = [
                            [-1.0f32, -1.0, 0.0, 1.0, 0.0, 0.0],
                            [1.0, -1.0, 0.0, 1.0, span, 0.0],
                            [-1.0, 1.0, 0.0, 1.0, 0.0, span],
                        ]
                        .into_iter()
                        .flatten()
                        .flat_map(f32::to_le_bytes)
                        .collect::<Vec<_>>();
                        module.buffers.push(BufferDesc {
                            id: vertices,
                            size: data.len() as u64,
                            usage: BufferUsage::Vertex,
                        });
                        resources.insert(vertices, data, false);
                        let vertex = &mut module.shaders[0];
                        vertex.id = 5000 + ordinal;
                        vertex.operations = vec![
                            S::LoadInputVec4 {
                                dst: 10,
                                location: 0,
                            },
                            S::StoreBuiltin {
                                builtin: 0,
                                src: 10,
                            },
                            S::LoadInput {
                                dst: 7,
                                location: 1,
                                component: 0,
                            },
                            S::LoadInput {
                                dst: 8,
                                location: 1,
                                component: 1,
                            },
                            S::StoreVarying {
                                location: 0,
                                component: 0,
                                src: 7,
                            },
                            S::StoreVarying {
                                location: 0,
                                component: 1,
                                src: 8,
                            },
                            S::Return,
                        ];
                        module.graphics_pipelines[0].vertex_shader = vertex.id;
                        module.graphics_pipelines[0].vertex_input = VertexInputState {
                            bindings: vec![VertexBindingDesc {
                                binding: 0,
                                stride: 24,
                                input_rate: VertexInputRate::Vertex,
                            }],
                            attributes: vec![
                                VertexAttributeDesc {
                                    location: 0,
                                    binding: 0,
                                    format: VertexFormat::Rgba32Float,
                                    offset: 0,
                                },
                                VertexAttributeDesc {
                                    location: 1,
                                    binding: 0,
                                    format: VertexFormat::Rg32Float,
                                    offset: 16,
                                },
                            ],
                        };
                        module.commands.insert(
                            0,
                            Command::BindVertexBuffer {
                                slot: 0,
                                buffer: vertices,
                                offset: 0,
                            },
                        );
                    }
                    let sampler = ResourceId(if linear { 902 } else { 901 });
                    let filter = if linear { 2 } else { 1 };
                    let tsc = [
                        2 | 2 << 3 | 2 << 6 | 1 << 13 | 1 << 14 | 1 << 17,
                        filter | filter << 4 | mip_filter << 6 | 1 << 8 | 1 << 9,
                        min_lod | max_lod << 12,
                        0,
                        0,
                        0,
                        0,
                        0,
                    ];
                    let prepared = exosphere_gpu_ir::texture_sampling::prepare_texture_2d(
                        tic(if mipped { 3 } else { 2 }),
                        tsc,
                        source,
                        sampler,
                        &space,
                    )
                    .unwrap();
                    module.samplers.push(prepared.sampler);
                    module.textures.push(prepared.image.clone());
                    ordinal += 1;
                    let shader_id = 1000 + ordinal;
                    let shader = module
                        .shaders
                        .iter_mut()
                        .find(|shader| shader.id == 12)
                        .unwrap();
                    shader.id = shader_id;
                    shader.operations = if automatic {
                        vec![
                            S::Interpolate {
                                dst: 7,
                                attribute: 0x80,
                                component: 0,
                                mode: exosphere_gpu_ir::shader::InterpolationMode::Pass,
                                offset: None,
                            },
                            S::Interpolate {
                                dst: 8,
                                attribute: 0x84,
                                component: 1,
                                mode: exosphere_gpu_ir::shader::InterpolationMode::Pass,
                                offset: None,
                            },
                        ]
                    } else {
                        vec![S::ConstantF32 {
                            dst: 7,
                            value: coordinate,
                        }]
                    };
                    shader.operations.push(S::SampleTexture2dHandle {
                        dst: vec![0, 1, 2, 3],
                        handle_selector: 7,
                        coords: if automatic { [7, 8] } else { [7, 7] },
                        components: vec![0, 1, 2, 3],
                        level_zero,
                    });
                    shader.operations.push(S::Return);
                    let compiled = exosphere_gpu_ir::CompiledShader {
                        address: 0x2000,
                        graphics_source: None,
                        graphics_header: header.clone(),
                        stats: Default::default(),
                        module: shader.clone(),
                    };
                    let fragment = compiled
                        .prepare_fragment_with_textures(
                            &[],
                            &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot::default(),
                            4,
                            &space,
                            &[exosphere_gpu_ir::texture_sampling::TextureShaderInterface {
                                selector: 7,
                                image: [1, 4],
                                sampler: [1, 5],
                                view: &prepared,
                            }],
                        )
                        .unwrap();
                    *shader = fragment.shader.module.clone();
                    let pipeline_id = 2000 + ordinal;
                    let pipeline = &mut module.graphics_pipelines[0];
                    pipeline.id = pipeline_id;
                    pipeline.fragment_shader = shader_id;
                    pipeline.layout = fragment.layout.clone();
                    for command in &mut module.commands {
                        if let Command::BindGraphicsPipeline { pipeline } = command {
                            *pipeline = pipeline_id;
                        }
                    }
                    // Bind/transition the sampled image outside dynamic rendering.
                    module.commands.splice(0..0, fragment.bindings);
                    let plan = backend.plan(&module).unwrap();
                    // One immutable shader/plan samples two live image payloads.
                    for &header in if mipped { &[3][..] } else { &[2, 3][..] } {
                        for refresh in [false, true] {
                            let last = if refresh {
                                [0, 0, 0, 255]
                            } else {
                                [255, 255, 255, 255]
                            };
                            let packed = if compressed {
                                // The 2x2 view occupies BC1 selector positions
                                // 0, 1, 4, 5 (the encoded block is 4x4). Change
                                // the bottom-right texel from red to green to
                                // independently exercise filtering and refresh.
                                let mut block = vec![0; 8];
                                block[..2].copy_from_slice(&0xf800u16.to_le_bytes());
                                block[2..4].copy_from_slice(&0x07e0u16.to_le_bytes());
                                let selectors: u32 =
                                    (1 << 2) | (1 << 8) | (u32::from(refresh) << 10);
                                block[4..].copy_from_slice(&selectors.to_le_bytes());
                                block
                            } else {
                                [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], last]
                                    .concat()
                            };
                            // Exercise a TIC-backed subview, not a prepacked image
                            // allocation. Poison padding to expose wrong offsets.
                            let mut bytes = vec![0xcc; if mipped { 1088 } else { 1024 }];
                            if header == 2 {
                                bytes[64..72].copy_from_slice(&packed[..8]);
                                if !compressed {
                                    bytes[96..104].copy_from_slice(&packed[8..]);
                                }
                            } else {
                                exosphere_gpu_ir::block_linear::swizzle_2d_into(
                                    &packed,
                                    &mut bytes[64..],
                                    exosphere_gpu_ir::BlockLinearLayout::new(
                                        8,
                                        if compressed { 1 } else { 2 },
                                        1,
                                        0,
                                        0,
                                    ),
                                )
                                .unwrap();
                            }
                            if mipped {
                                let last_level = if compressed {
                                    // A 1x1 BC1 mip still stores a full block.
                                    let endpoint = if refresh { 0x07e0u16 } else { 0xf800u16 };
                                    let mut block = vec![0; 8];
                                    block[..2].copy_from_slice(&endpoint.to_le_bytes());
                                    block
                                } else if refresh {
                                    vec![71, 89, 113, 255]
                                } else {
                                    vec![17, 43, 91, 255]
                                };
                                exosphere_gpu_ir::block_linear::swizzle_2d_into(
                                    &last_level,
                                    &mut bytes[64 + 512..],
                                    exosphere_gpu_ir::BlockLinearLayout::new(
                                        if compressed { 8 } else { 4 },
                                        1,
                                        1,
                                        0,
                                        0,
                                    ),
                                )
                                .unwrap();
                            }
                            resources.insert(allocation, bytes, false);
                            let words = tic(header);
                            let current = exosphere_gpu_ir::texture_sampling::prepare_texture_2d(
                                words, tsc, source, sampler, &space,
                            )
                            .unwrap();
                            assert_eq!(current.image, prepared.image);
                            assert_eq!(current.sampler, prepared.sampler);
                            assert_eq!(current.channels, prepared.channels);
                            let mut provider =
                                exosphere_gpu_ir::texture_data::TextureResourceData::new(
                                    &mut resources,
                                );
                            provider.add_texture(source, words, &space).unwrap();
                            device
                                .execute_plan_with_resources(&plan, &mut provider)
                                .unwrap();
                            let pixels = device.readback_rgba8(target).unwrap();
                            let mip_color = if mipped {
                                if compressed {
                                    if refresh {
                                        [0, 0, 255, 255]
                                    } else {
                                        [0, 255, 0, 255]
                                    }
                                } else if refresh {
                                    [113, 71, 89, 255]
                                } else {
                                    [91, 17, 43, 255]
                                }
                            } else {
                                [0; 4]
                            };
                            let base_color = if compressed {
                                if coordinate == 0.25 {
                                    [0, 255, 0, 255]
                                } else if linear {
                                    if refresh {
                                        [0, 64, 191, 255]
                                    } else {
                                        [0, 128, 128, 255]
                                    }
                                } else if refresh {
                                    [0, 0, 255, 255]
                                } else {
                                    [0, 255, 0, 255]
                                }
                            } else if coordinate == 0.25 {
                                [0, 255, 0, 255]
                            } else if linear {
                                let value = if refresh { 64 } else { 128 };
                                [value, value, value, 255]
                            } else {
                                last
                            };
                            // Independent color oracle: filtering within each
                            // level precedes the half-LOD blend between levels.
                            let expected = match mip_mode {
                                MipSampling::SingleLevel => base_color,
                                MipSampling::NearestLevelOne | MipSampling::ImplicitGradient => {
                                    mip_color
                                }
                                MipSampling::BlendHalf => std::array::from_fn(|i| {
                                    (u16::from(base_color[i]) + u16::from(mip_color[i])).div_ceil(2)
                                        as u8
                                }),
                            };
                            for (index, pixel) in pixels.chunks_exact(4).enumerate() {
                                if coverage[index] {
                                    assert!(
                                        pixel.iter().zip(expected).all(|(actual, expected)| actual
                                            .abs_diff(expected)
                                            <= 1),
                                        "linear={linear} zero={level_zero} coord={coordinate} refresh={refresh}: {pixel:?} vs {expected:?}"
                                    );
                                } else {
                                    assert_eq!(pixel, &reference[index * 4..index * 4 + 4]);
                                }
                            }
                        }
                    }
                }
            }
        }
        if compressed {
            if mipped {
                eprintln!(
                    "BC1 mip pixels verified: selected/blended levels, implicit/zero LOD, block-linear and live refresh"
                );
            } else {
                eprintln!(
                    "BC1 pixels verified: nearest/linear, implicit/zero LOD, pitch/block-linear and live refresh"
                );
            }
        }
    }
    #[test]
    fn explicit_sampler_settings_reach_native_parameters() {
        let info = create_info(&state(), &limits(), true).unwrap();
        assert_eq!(
            (info.min_filter, info.mag_filter),
            (vk::Filter::NEAREST, vk::Filter::LINEAR)
        );
        assert_eq!(
            (info.min_lod, info.max_lod, info.mip_lod_bias),
            (0.5, 4.0, -0.25)
        );
        assert_eq!(info.max_anisotropy, 4.0);
        assert_eq!(info.anisotropy_enable, vk::TRUE);
        assert_eq!(info.compare_op, vk::CompareOp::LESS_OR_EQUAL);
        assert_eq!(info.compare_enable, vk::TRUE);
        assert_eq!(info.address_mode_v, vk::SamplerAddressMode::MIRRORED_REPEAT);
        assert_eq!(info.address_mode_w, vk::SamplerAddressMode::CLAMP_TO_BORDER);
        assert_eq!(info.border_color, vk::BorderColor::FLOAT_OPAQUE_WHITE);
    }
    #[test]
    fn unsupported_settings_are_rejected_not_substituted() {
        let original = state();
        assert!(create_info(&original, &limits(), false).is_err());
        for mode in [
            SamplerAddressMode::Clamp,
            SamplerAddressMode::MirrorClamp,
            SamplerAddressMode::MirrorClampToBorder,
            SamplerAddressMode::MirrorClampToEdge,
        ] {
            let mut value = original;
            value.address[0] = mode;
            assert!(create_info(&value, &limits(), true).is_err());
        }
        for ratio in [0, 9] {
            let mut value = original;
            value.max_anisotropy = ratio;
            assert!(create_info(&value, &limits(), true).is_err());
        }
        let mut value = original;
        value.min_lod_8_8 = 1025;
        assert!(create_info(&value, &limits(), true).is_err());
        value = original;
        value.lod_bias_8_8 = 513;
        assert!(create_info(&value, &limits(), true).is_err());
        value = original;
        value.compare = Some(exosphere_gpu_ir::CompareOp::Unknown(8));
        assert!(create_info(&value, &limits(), true).is_err());
    }
    #[test]
    fn non_mipmap_and_unnormalized_modes_obey_vulkan_constraints() {
        let mut value = state();
        value.max_anisotropy = 1;
        value.compare = None;
        value.mip_filter = SamplerMipFilter::None;
        value.min_lod_8_8 = 0;
        value.max_lod_8_8 = 0;
        assert_eq!(create_info(&value, &limits(), false).unwrap().max_lod, 0.25);
        value.mag_filter = value.min_filter;
        assert_eq!(create_info(&value, &limits(), false).unwrap().max_lod, 0.0);
        value.unnormalized_coordinates = true;
        assert!(create_info(&value, &limits(), false).is_err());
        value.address = [SamplerAddressMode::ClampToEdge; 3];
        assert_eq!(
            create_info(&value, &limits(), false)
                .unwrap()
                .unnormalized_coordinates,
            vk::TRUE
        );
        value.compare = Some(exosphere_gpu_ir::CompareOp::Always);
        assert!(create_info(&value, &limits(), false).is_err());
    }
    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_sampler_creation_preserves_identity_and_rejects_mutation() {
        use super::super::AshVulkanDevice;
        use exosphere_gpu_ir::{ResourceId, SamplerDesc};
        let mut device = AshVulkanDevice::create().unwrap();
        let mut desc = SamplerDesc {
            id: ResourceId(97),
            state: state(),
        };
        desc.state.max_anisotropy = 1;
        device.ensure_sampler(&desc).unwrap();
        device.ensure_sampler(&desc).unwrap();
        let handle = device.samplers[&desc.id].sampler;
        desc.state.min_filter = FilterMode::Linear;
        assert!(device.ensure_sampler(&desc).is_err());
        assert_eq!(device.samplers[&desc.id].sampler, handle);
        desc.id = ResourceId(98);
        device.ensure_sampler(&desc).unwrap();
        assert_eq!(device.samplers.len(), 2);
    }
}
