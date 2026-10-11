// SPDX-License-Identifier: GPL-3.0-or-later
//! Device-local transfer from a rendered color resource to a native target.

use super::*;

impl AshVulkanDevice {
    /// Record a nearest-filter blit and restore the source resource's layout.
    ///
    /// # Safety
    /// `command` must be recording outside a render pass on this device's queue
    /// family. `target` must be a distinct, acquired single-sample 2D image from
    /// this device, with the stated format/extent, TRANSFER_DST usage and layout.
    /// The caller must submit on this backend's queue, synchronize target access,
    /// and retain both images and their allocations until execution completes.
    pub unsafe fn record_present_blit(
        &mut self,
        command: vk::CommandBuffer,
        source: ResourceId,
        target: vk::Image,
        target_format: vk::Format,
        target_extent: vk::Extent2D,
    ) -> Result<(), VulkanError> {
        let image = self
            .images
            .get(&source)
            .ok_or(VulkanError::MissingResource(source.0))?;
        if self.submission_failed
            || image.image == target
            || image.format != vk::Format::R8G8B8A8_UNORM
            || image.extent.depth != 1
            || image.layers != 1
            || image.mip_levels != 1
            || image.samples != vk::SampleCountFlags::TYPE_1
            || image.layout == vk::ImageLayout::UNDEFINED
            || image.extent.width == 0
            || image.extent.height == 0
            || !matches!(
                target_format,
                vk::Format::R8G8B8A8_UNORM | vk::Format::B8G8R8A8_UNORM
            )
            || image.extent.width > i32::MAX as u32
            || image.extent.height > i32::MAX as u32
            || target_extent.width == 0
            || target_extent.height == 0
            || target_extent.width > i32::MAX as u32
            || target_extent.height > i32::MAX as u32
        {
            return Err(VulkanError::UnsupportedFeature(
                "presentation blit image shape/state",
            ));
        }
        let src_properties = unsafe {
            self.instance
                .get_physical_device_format_properties(self.physical, image.format)
        };
        let dst_properties = unsafe {
            self.instance
                .get_physical_device_format_properties(self.physical, target_format)
        };
        if !src_properties
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::BLIT_SRC)
            || !dst_properties
                .optimal_tiling_features
                .contains(vk::FormatFeatureFlags::BLIT_DST)
        {
            return Err(VulkanError::UnsupportedFeature(
                "presentation blit format support",
            ));
        }
        let handle = image.image;
        let old_layout = image.layout;
        let extent = image.extent;
        let layers = vk::ImageSubresourceLayers::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .layer_count(1);
        let region = [vk::ImageBlit::default()
            .src_subresource(layers)
            .src_offsets([
                vk::Offset3D::default(),
                vk::Offset3D {
                    x: extent.width as i32,
                    y: extent.height as i32,
                    z: 1,
                },
            ])
            .dst_subresource(layers)
            .dst_offsets([
                vk::Offset3D::default(),
                vk::Offset3D {
                    x: target_extent.width as i32,
                    y: target_extent.height as i32,
                    z: 1,
                },
            ])];
        self.transition_image(command, source, vk::ImageLayout::TRANSFER_SRC_OPTIMAL)?;
        unsafe {
            self.device.cmd_blit_image(
                command,
                handle,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                target,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &region,
                vk::Filter::NEAREST,
            )
        };
        self.transition_image(command, source, old_layout)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_gpu_ir::{GpuModule, InMemoryGpuResources};

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_shader_identity_changes_reject_before_cached_pipeline_execution() {
        let mut device = AshVulkanDevice::create().unwrap();
        let module = GpuModule::triangle_vulkan_smoke_test(32, 32);
        let backend = crate::VulkanBackend::new();
        device
            .execute_plan_with_resources(
                &backend.plan(&module).unwrap(),
                &mut InMemoryGpuResources::new(),
            )
            .unwrap();
        let image = module.textures[0].id;
        let expected = device.readback_rgba8(image).unwrap();
        let mut changed = module.clone();
        let exosphere_gpu_ir::ShaderOperation::ConstantF32 { value, .. } =
            &mut changed.shaders[1].operations[0]
        else {
            panic!("expected explicit fragment constant")
        };
        *value = 0.7;
        let plan = backend.plan(&changed).unwrap();
        assert!(matches!(
            device.execute_plan_with_resources(&plan, &mut InMemoryGpuResources::new()),
            Err(crate::VulkanError::UnsupportedFeature(
                "shader identity changed"
            ))
        ));
        assert_eq!(device.readback_rgba8(image).unwrap(), expected);
        let mut duplicate = backend.plan(&module).unwrap();
        duplicate.shaders.push(plan.shaders[1].clone());
        assert!(matches!(
            device.execute_plan_with_resources(&duplicate, &mut InMemoryGpuResources::new()),
            Err(crate::VulkanError::UnsupportedFeature(
                "shader identity changed"
            ))
        ));
        assert_eq!(device.readback_rgba8(image).unwrap(), expected);
        // A genuinely new shader/pipeline identity admits the changed code;
        // rejecting ID reuse must not disable later valid recompilation.
        changed.shaders[1].id = 13;
        changed.graphics_pipelines[0].fragment_shader = 13;
        changed.graphics_pipelines[0].id = 12;
        for command in &mut changed.commands {
            if let exosphere_gpu_ir::Command::BindGraphicsPipeline { pipeline } = command {
                *pipeline = 12;
            }
        }
        device
            .execute_plan_with_resources(
                &backend.plan(&changed).unwrap(),
                &mut InMemoryGpuResources::new(),
            )
            .unwrap();
        assert_ne!(device.readback_rgba8(image).unwrap(), expected);
        device
            .execute_plan_with_resources(
                &backend.plan(&module).unwrap(),
                &mut InMemoryGpuResources::new(),
            )
            .unwrap();
        assert_eq!(device.readback_rgba8(image).unwrap(), expected);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_indirect_draw_matches_reference_pixels() {
        use exosphere_gpu_ir::{BufferDesc, BufferUsage, Command, IndexType};
        let reference = GpuModule::triangle_vulkan_smoke_test(64, 64);
        let target = reference.textures[0].id;
        let mut backend = AshVulkanDevice::create().unwrap();
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&reference).unwrap(),
                &mut InMemoryGpuResources::new(),
            )
            .unwrap();
        let expected = backend.readback_rgba8(target).unwrap();
        let mut identity = 0x10000;
        for indexed in [false, true] {
            for count in [1, 2] {
                for stride in [0, 28] {
                    identity += 2;
                    let indirect = ResourceId(identity);
                    let indices = ResourceId(identity + 1);
                    let mut module = reference.clone();
                    let mut resources = InMemoryGpuResources::new();
                    let words = if indexed {
                        vec![3u32, 1, 1, u32::MAX, 0]
                    } else {
                        vec![3u32, 1, 0, 0]
                    };
                    let command_bytes: Vec<u8> =
                        words.into_iter().flat_map(u32::to_le_bytes).collect();
                    let mut data = vec![0xff; 4];
                    if count == 2 {
                        // First command draws nothing; only the correctly-strided
                        // second command can produce the reference triangle.
                        data.extend(vec![0; command_bytes.len()]);
                        let step = if stride == 0 {
                            command_bytes.len()
                        } else {
                            stride as usize
                        };
                        data.resize(4 + step, 0xff);
                    }
                    data.extend(&command_bytes);
                    module.buffers.push(BufferDesc {
                        id: indirect,
                        size: data.len() as u64,
                        usage: BufferUsage::Indirect,
                    });
                    resources.insert(indirect, data.clone(), false);
                    let mut replacement = Vec::new();
                    if indexed {
                        let bytes = [99u32, 99, 1, 2, 3]
                            .into_iter()
                            .flat_map(u32::to_le_bytes)
                            .collect::<Vec<_>>();
                        module.buffers.push(BufferDesc {
                            id: indices,
                            size: bytes.len() as u64,
                            usage: BufferUsage::Index,
                        });
                        resources.insert(indices, bytes, false);
                        replacement.push(Command::BindIndexBuffer {
                            buffer: indices,
                            offset: 4,
                            index_type: IndexType::U32,
                        });
                    }
                    replacement.push(Command::DrawIndirect {
                        buffer: indirect,
                        offset: 4,
                        draw_count: count,
                        stride,
                        indexed,
                    });
                    let position = module
                        .commands
                        .iter()
                        .position(|command| matches!(command, Command::Draw { .. }))
                        .unwrap();
                    module.commands.splice(position..=position, replacement);
                    let plan = crate::VulkanBackend::new().plan(&module).unwrap();
                    if count > 1 && backend.core_features.multi_draw_indirect != vk::TRUE {
                        assert!(
                            backend
                                .execute_plan_with_resources(&plan, &mut resources)
                                .is_err()
                        );
                        continue;
                    }
                    backend
                        .execute_plan_with_resources(&plan, &mut resources)
                        .unwrap();
                    assert!(
                        backend.readback_rgba8(target).unwrap() == expected,
                        "indirect mismatch: indexed={indexed}, count={count}, stride={stride}"
                    );
                    // Reuse the native allocation while the CPU changes the
                    // argument payload. Both suppression and restoration must
                    // be observed without an explicit backend refresh call.
                    resources.insert(indirect, vec![0; data.len()], false);
                    backend
                        .execute_plan_with_resources(&plan, &mut resources)
                        .unwrap();
                    assert!(
                        backend
                            .readback_rgba8(target)
                            .unwrap()
                            .chunks_exact(4)
                            .all(|pixel| pixel == [0, 0, 0, 255])
                    );
                    resources.insert(indirect, data, false);
                    backend
                        .execute_plan_with_resources(&plan, &mut resources)
                        .unwrap();
                    assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
                }
            }
        }
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_draw_input_upload_tracks_provider_changes() {
        use exosphere_gpu_ir::{BufferDesc, BufferUsage};
        let mut backend = AshVulkanDevice::create().unwrap();
        let mut resources = InMemoryGpuResources::new();
        for (index, usage) in [
            BufferUsage::Vertex,
            BufferUsage::Index,
            BufferUsage::Indirect,
        ]
        .into_iter()
        .enumerate()
        {
            let id = ResourceId(0x20000 + index as u64);
            let desc = BufferDesc {
                id,
                size: 16,
                usage,
            };
            for marker in [1, 2, 1] {
                resources.insert(id, vec![marker; 16], false);
                backend.ensure_buffer(&desc, &mut resources).unwrap();
                let buffer = backend.buffers.get(&id).unwrap();
                let mapped = unsafe {
                    backend.device.map_memory(
                        buffer.memory,
                        0,
                        vk::WHOLE_SIZE,
                        vk::MemoryMapFlags::empty(),
                    )
                }
                .unwrap();
                let range = vk::MappedMemoryRange::default()
                    .memory(buffer.memory)
                    .offset(0)
                    .size(vk::WHOLE_SIZE);
                // Host-visible memory is not necessarily coherent. Invalidate
                // before inspection and always unmap, including on error.
                let invalidated =
                    unsafe { backend.device.invalidate_mapped_memory_ranges(&[range]) };
                let bytes = if invalidated.is_ok() {
                    unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), 16) }.to_vec()
                } else {
                    Vec::new()
                };
                unsafe { backend.device.unmap_memory(buffer.memory) };
                invalidated.unwrap();
                assert_eq!(bytes, vec![marker; 16]);
                assert_eq!(
                    buffer.content_fingerprint,
                    Some(*blake3::hash(&bytes).as_bytes())
                );
            }
            let certified = backend.buffers.get(&id).unwrap().content_fingerprint;
            resources.insert(id, vec![9; 8], false);
            assert!(backend.ensure_buffer(&desc, &mut resources).is_err());
            assert_eq!(
                backend.buffers.get(&id).unwrap().content_fingerprint,
                certified
            );
        }
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_vertex_attributes_and_refresh_match_reference_pixels() {
        run_native_vertex_fixture(false, false);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device; checks rejection when exact float modes are unavailable"]
    fn native_vertex_arithmetic_checks_device_modes_or_reference_pixels() {
        run_native_vertex_fixture(true, false);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_integer_word_add_mul_preserve_wraparound_and_aliasing() {
        use exosphere_gpu_ir::shader::{BinaryOp, IntegerControl, ValueModifier};
        use exosphere_gpu_ir::{GpuModule, InMemoryGpuResources, ShaderOperation as S};
        let mut device = AshVulkanDevice::create().unwrap();
        let backend = crate::VulkanBackend::new();
        let baseline = GpuModule::triangle_vulkan_smoke_test(32, 32);
        let target = baseline.textures[0].id;
        let mut resources = InMemoryGpuResources::new();
        device
            .execute_plan_with_resources(&backend.plan(&baseline).unwrap(), &mut resources)
            .unwrap();
        let reference = device.readback_rgba8(target).unwrap();
        let plain = ValueModifier {
            negate: false,
            absolute: false,
            invert: false,
        };
        for signed in [false, true] {
            for (ordinal, op, left, right, expected) in [
                (0, BinaryOp::Add, 0x3f000001, 0xffffffff, 128),
                (1, BinaryOp::Mul, 0x9f400000, 2, 64),
            ] {
                for alias_right in [false, true] {
                    let mut module = baseline.clone();
                    let id = 5000 + u64::from(signed) * 4 + ordinal * 2 + u64::from(alias_right);
                    let shader = module.shaders.iter_mut().find(|s| s.id == 12).unwrap();
                    shader.id = id;
                    let dst = u32::from(alias_right);
                    shader.operations = vec![
                        S::ConstantBits32 {
                            dst: 0,
                            value: left,
                        },
                        S::ConstantBits32 {
                            dst: 1,
                            value: right,
                        },
                        S::IntegerBinary {
                            dst,
                            op,
                            left: 0,
                            right: 1,
                            left_modifier: plain,
                            right_modifier: plain,
                            control: IntegerControl {
                                saturate: false,
                                carry_in: false,
                                carry_out: false,
                                high: false,
                                signed,
                            },
                        },
                    ];
                    shader
                        .operations
                        .push(S::ConstantF32 { dst: 2, value: 1.0 });
                    for component in 0..4 {
                        shader.operations.push(S::StoreOutput {
                            location: 0,
                            component,
                            src: if component == 3 { 2 } else { dst },
                        });
                    }
                    shader.operations.push(S::Return);
                    module.graphics_pipelines[0].id = id;
                    module.graphics_pipelines[0].fragment_shader = id;
                    for command in &mut module.commands {
                        if let exosphere_gpu_ir::Command::BindGraphicsPipeline { pipeline } =
                            command
                        {
                            *pipeline = id;
                        }
                    }
                    device
                        .execute_plan_with_resources(
                            &backend.plan(&module).unwrap(),
                            &mut resources,
                        )
                        .unwrap();
                    let pixels = device.readback_rgba8(target).unwrap();
                    for (pixel, reference) in pixels.chunks_exact(4).zip(reference.chunks_exact(4))
                    {
                        if reference[..3] != [0, 0, 0] {
                            assert_eq!(pixel, [expected, expected, expected, 255]);
                        } else {
                            assert_eq!(pixel, reference);
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_word_shifts_observe_wrap_clamp_and_sign() {
        use exosphere_gpu_ir::{ShaderOperation as S, shader::WordShiftKind as K};
        let mut cases = Vec::new();
        for kind in [K::Left, K::RightLogical, K::RightArithmetic] {
            for wrap in [false, true] {
                for amount in [0, 1, 31, 32, 33, u32::MAX] {
                    for value in [0x80000001u32, 0x7fffffff] {
                        let count = if wrap { amount & 31 } else { amount.min(32) };
                        let expected = match kind {
                            K::Left => value.checked_shl(count).unwrap_or(0),
                            K::RightLogical => value.checked_shr(count).unwrap_or(0),
                            K::RightArithmetic => ((value as i32) >> count.min(31)) as u32,
                        };
                        cases.push((
                            value,
                            amount,
                            S::ShiftWord32 {
                                dst: 0,
                                value: 0,
                                amount: 1,
                                kind,
                                wrap,
                            },
                            expected,
                        ));
                    }
                }
            }
        }
        verify_native_word_operations(6000, &cases, &[1, 0x80000000]);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_word_extraction_matches_bitwise_reference() {
        use exosphere_gpu_ir::maxwell_semantics::{decode_semantic, lower_semantic};
        use exosphere_gpu_ir::maxwell_shader::MaxwellInstruction;
        let mut cases = Vec::new();
        for signed in [false, true] {
            // Execute the actual register BFE decoder/lowering, not a manually
            // constructed canonical operation. R0 aliases the source/destination.
            let lowered = lower_semantic(&decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: 0x5c00000000170000 | (u64::from(signed) << 48),
            }));
            assert!(lowered.lossless);
            assert_eq!(lowered.operations.len(), 1);
            for offset in [0u32, 1, 7, 31, 32, 33, 255] {
                for width in [0u32, 1, 8, 31, 32, 33, 255] {
                    for value in [0x80000001u32, 0x7fffffff] {
                        // Independent PTX-style bit oracle, not the SPIR-V shift algorithm.
                        let sign = signed
                            && width != 0
                            && (value >> (offset + width - 1).min(31)) & 1 != 0;
                        let mut expected = 0;
                        for bit in 0..32 {
                            let set = if bit < width && offset + bit < 32 {
                                value & (1 << (offset + bit)) != 0
                            } else {
                                sign
                            };
                            expected |= u32::from(set) << bit;
                        }
                        // Upper control bits are ignored, including all-one padding.
                        let control = 0xffff0000 | (width << 8) | offset;
                        cases.push((value, control, lowered.operations[0].clone(), expected));
                    }
                }
            }
        }
        let masks: Vec<u32> = (0..32).map(|bit| 1 << bit).collect();
        verify_native_word_operations(7000, &cases, &masks);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_population_count_preserves_word_inversion_and_aliasing() {
        use exosphere_gpu_ir::maxwell_semantics::{decode_semantic, lower_semantic};
        use exosphere_gpu_ir::maxwell_shader::MaxwellInstruction;
        let mut cases = Vec::new();
        for invert in [false, true] {
            let lowered = lower_semantic(&decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: 0x5c08000000070000 | (u64::from(invert) << 40),
            }));
            assert!(lowered.lossless);
            assert_eq!(lowered.operations.len(), 1);
            // All possible population counts, plus noncontiguous word patterns.
            let words = (0..=32)
                .map(|count| {
                    if count == 0 {
                        0
                    } else {
                        u32::MAX >> (32 - count)
                    }
                })
                .chain([0xaaaaaaaa, 0x55555555, 0x80000000, 0x80000001, 0xf0f00f0f]);
            for value in words {
                let source = if invert { !value } else { value };
                let expected = (0..32).filter(|bit| source & (1 << bit) != 0).count() as u32;
                cases.push((value, 0, lowered.operations[0].clone(), expected));
            }
        }
        verify_native_word_operations(14000, &cases, &[1, 2, 4, 8, 16, 32]);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_leading_bit_search_preserves_sign_shift_and_inversion() {
        use exosphere_gpu_ir::maxwell_semantics::{decode_semantic, lower_semantic};
        use exosphere_gpu_ir::maxwell_shader::MaxwellInstruction;
        let mut cases = Vec::new();
        for signed in [false, true] {
            for invert in [false, true] {
                for shift_amount in [false, true] {
                    let lowered = lower_semantic(&decode_semantic(MaxwellInstruction {
                        offset: 8,
                        raw: 0x5c30000000070000
                            | (u64::from(signed) << 48)
                            | (u64::from(invert) << 40)
                            | (u64::from(shift_amount) << 41),
                    }));
                    assert!(lowered.lossless);
                    assert_eq!(lowered.operations.len(), 1);
                    let words = (0..32)
                        .flat_map(|bit| [1u32 << bit, !(1u32 << bit)])
                        .chain([
                            0,
                            u32::MAX,
                            0x80000001,
                            0x7ffffffe,
                            0xaaaaaaaa,
                            0x55555555,
                            0x80000,
                        ]);
                    for value in words {
                        let source = if invert { !value } else { value };
                        let negative = signed && source & 0x80000000 != 0;
                        // Bit-by-bit search is independent of the emitted prefix-fill/count algorithm.
                        let found = (0..32)
                            .rev()
                            .find(|bit| (source & (1 << bit) != 0) != negative);
                        let expected =
                            found.map_or(u32::MAX, |bit| if shift_amount { 31 - bit } else { bit });
                        cases.push((value, 0, lowered.operations[0].clone(), expected));
                    }
                }
            }
        }
        verify_native_word_operations(15000, &cases, &[1, 2, 4, 8, 16, 32, 0x80000000]);
    }

    fn verify_native_word_operations(
        id_base: u64,
        cases: &[(u32, u32, exosphere_gpu_ir::ShaderOperation, u32)],
        masks: &[u32],
    ) {
        use exosphere_gpu_ir::shader::{
            BinaryOp, ConversionControl, IntegerControl, RoundingMode, ScalarType, ValueModifier,
        };
        use exosphere_gpu_ir::{GpuModule, InMemoryGpuResources, ShaderOperation as S};
        let mut device = AshVulkanDevice::create().unwrap();
        let backend = crate::VulkanBackend::new();
        let baseline = GpuModule::triangle_vulkan_smoke_test(32, 32);
        let target = baseline.textures[0].id;
        let mut resources = InMemoryGpuResources::new();
        device
            .execute_plan_with_resources(&backend.plan(&baseline).unwrap(), &mut resources)
            .unwrap();
        let reference = device.readback_rgba8(target).unwrap();
        let mut ordinal = 0;
        for (value, amount, operation, expected) in cases {
            for &mask in masks {
                ordinal += 1;
                let id = id_base + ordinal;
                let mut module = baseline.clone();
                let shader = module.shaders.iter_mut().find(|s| s.id == 12).unwrap();
                shader.id = id;
                let plain = ValueModifier {
                    negate: false,
                    absolute: false,
                    invert: false,
                };
                shader.operations = vec![
                    S::ConstantBits32 {
                        dst: 0,
                        value: *value,
                    },
                    S::ConstantBits32 {
                        dst: 1,
                        value: *amount,
                    },
                    operation.clone(),
                    S::ConstantBits32 {
                        dst: 1,
                        value: mask,
                    },
                    S::IntegerBinary {
                        dst: 0,
                        op: BinaryOp::And,
                        left: 0,
                        right: 1,
                        left_modifier: plain,
                        right_modifier: plain,
                        control: IntegerControl {
                            saturate: false,
                            carry_in: false,
                            carry_out: false,
                            high: false,
                            signed: false,
                        },
                    },
                    // Project a selected result bit to black/white. This
                    // checks negative/sign words without exporting NaNs.
                    S::ConvertRich {
                        dst: 0,
                        src: 0,
                        control: ConversionControl {
                            src: ScalarType::U32,
                            dst: ScalarType::F32,
                            rounding: RoundingMode::NearestEven,
                            saturate: false,
                            flush_to_zero: false,
                            absolute: false,
                            negate: false,
                            raw_flags: 0,
                        },
                    },
                    S::ConstantF32 { dst: 2, value: 1.0 },
                ];
                for component in 0..4 {
                    shader.operations.push(S::StoreOutput {
                        location: 0,
                        component,
                        src: if component == 3 { 2 } else { 0 },
                    });
                }
                shader.operations.push(S::Return);
                module.graphics_pipelines[0].id = id;
                module.graphics_pipelines[0].fragment_shader = id;
                for command in &mut module.commands {
                    if let exosphere_gpu_ir::Command::BindGraphicsPipeline { pipeline } = command {
                        *pipeline = id;
                    }
                }
                device
                    .execute_plan_with_resources(&backend.plan(&module).unwrap(), &mut resources)
                    .unwrap();
                let pixels = device.readback_rgba8(target).unwrap();
                let level = if expected & mask == 0 { 0 } else { 255 };
                for (pixel, reference) in pixels.chunks_exact(4).zip(reference.chunks_exact(4)) {
                    if reference[..3] != [0, 0, 0] {
                        assert_eq!(
                            pixel,
                            [level, level, level, 255],
                            "{operation:?} control={amount:x} value={value:x} mask={mask:x}"
                        );
                    } else {
                        assert_eq!(pixel, reference);
                    }
                }
            }
        }
        assert_eq!(ordinal as usize, cases.len() * masks.len());
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_vertex_uniform_words_and_refresh_match_reference_pixels() {
        run_native_vertex_fixture(false, true);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_fragment_constant_mov_and_refresh_match_pixels() {
        run_fragment_constant_fixture(FragmentConstantLoad::Mov);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_fragment_ldc_and_refresh_match_pixels() {
        run_fragment_constant_fixture(FragmentConstantLoad::Ldc32);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_fragment_ldc64_and_refresh_match_pixels() {
        run_fragment_constant_fixture(FragmentConstantLoad::Ldc64);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_fragment_ldc128_and_refresh_match_pixels() {
        run_fragment_constant_fixture(FragmentConstantLoad::Ldc128);
    }

    #[derive(Clone, Copy)]
    enum FragmentConstantLoad {
        Mov,
        Ldc32,
        Ldc64,
        Ldc128,
    }

    fn run_fragment_constant_fixture(load: FragmentConstantLoad) {
        use exosphere_gpu_ir::{BufferDesc, BufferUsage, Command, ShaderStage};
        let mut backend = AshVulkanDevice::create().unwrap();
        let mut module = GpuModule::triangle_vulkan_smoke_test(64, 64);
        let target = module.textures[0].id;
        let mut resources = InMemoryGpuResources::new();
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&module).unwrap(),
                &mut resources,
            )
            .unwrap();
        let expected = backend.readback_rgba8(target).unwrap();
        let mut bytes = vec![0; exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE];
        bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        let instruction = |dst: u64| {
            if let FragmentConstantLoad::Mov = load {
                0x4c98000000000000 | (3 << 34) | (15 << 39) | (7 << 16) | (dst << 20) | dst
            } else {
                let size = match load {
                    FragmentConstantLoad::Ldc32 => 4,
                    FragmentConstantLoad::Ldc64 => 5,
                    FragmentConstantLoad::Ldc128 => 6,
                    FragmentConstantLoad::Mov => unreachable!(),
                };
                // LDC byte offsets differ from MOV's four-byte word offsets.
                0xef90000000000000
                    | (size << 48)
                    | (3 << 36)
                    | (255 << 8)
                    | (7 << 16)
                    | ((dst * 4) << 20)
                    | dst
            }
        };
        // Each Maxwell bundle starts with a scheduler word followed by up to
        // three instructions. Wide loads still populate all four color registers.
        let exit = 0xe30000000007000f;
        let instructions = match load {
            FragmentConstantLoad::Mov | FragmentConstantLoad::Ldc32 => vec![
                0,
                instruction(0),
                instruction(1),
                instruction(2),
                0,
                instruction(3),
                exit,
            ],
            FragmentConstantLoad::Ldc64 => vec![0, instruction(0), instruction(2), exit],
            FragmentConstantLoad::Ldc128 => vec![0, instruction(0), exit],
        };
        bytes.extend(instructions.into_iter().flat_map(u64::to_le_bytes));
        let program =
            exosphere_gpu_ir::maxwell_shader::parse_graphics(ShaderStage::Fragment, &bytes)
                .unwrap();
        let (shader, stats) = exosphere_gpu_ir::maxwell_shader::lift_semantic(205, &program);
        assert_eq!(stats.opaque, 0);
        assert_eq!(
            shader
                .operations
                .iter()
                .filter(|operation| matches!(
                    operation,
                    exosphere_gpu_ir::ShaderOperation::LoadConstantBuffer { .. }
                ))
                .count(),
            4
        );
        let interface = [
            exosphere_gpu_ir::maxwell_constant::ConstantBufferInterface {
                slot: 3,
                set: 0,
                binding: 8,
                byte_size: 16,
            },
        ];
        let source = exosphere_gpu_ir::compiler::CompiledShader {
            address: 0x100080,
            graphics_source: None,
            graphics_header: program.header.clone(),
            module: shader.clone(),
            stats,
        };
        let shader =
            exosphere_gpu_ir::maxwell_constant::lower_constant_interface(&shader, &interface)
                .unwrap();
        module.shaders[1] = exosphere_gpu_ir::maxwell_fragment::lower_fragment_outputs(
            &shader,
            program.header.as_ref().unwrap(),
        )
        .unwrap();
        module.graphics_pipelines[0].fragment_shader = 205;
        module.graphics_pipelines[0].id = 205;
        module.graphics_pipelines[0].layout =
            exosphere_gpu_ir::maxwell_constant::uniform_pipeline_layout(&[
                &module.shaders[0],
                &module.shaders[1],
            ])
            .unwrap();
        for command in &mut module.commands {
            if let Command::BindGraphicsPipeline { pipeline } = command {
                *pipeline = 205;
            }
        }
        let id = ResourceId(0x22000);
        module.buffers.push(BufferDesc {
            id,
            size: 16,
            usage: BufferUsage::Uniform,
        });
        let mut space = exosphere_gpu_ir::GpuAddressSpace::new();
        space
            .map(exosphere_gpu_ir::GpuMapping {
                guest_base: 0x200000,
                size: 16,
                resource: id,
                resource_offset: 0,
                permissions: exosphere_gpu_ir::GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        let mut snapshot = exosphere_gpu_ir::maxwell_state::DrawStateSnapshot::default();
        let backing = ResourceId(0x22001);
        space
            .map(exosphere_gpu_ir::GpuMapping {
                guest_base: 0x300000,
                size: 64 * 64 * 4,
                resource: backing,
                resource_offset: 0,
                permissions: exosphere_gpu_ir::GpuMapPermissions::READ_WRITE,
            })
            .unwrap();
        for (field, word) in [0, 0x300000, 64, 64, 0xd5, 0, 1, 0, 0]
            .into_iter()
            .enumerate()
        {
            snapshot.raw_registers.insert(
                exosphere_gpu_ir::maxwell_state::method::COLOR_TARGET_BASE + field as u16,
                word,
            );
        }
        let color = snapshot
            .prepare_color_target_layer(0, target, 1, &space)
            .unwrap();
        assert_eq!(color.backing.resource, backing);
        assert_eq!(color.backing.length, 64 * 64 * 4);
        module.textures[0] = color.texture;
        // Use a fresh device resource cache so the descriptor must allocate a
        // target, rather than reuse the earlier host-only reference image.
        backend = AshVulkanDevice::create().unwrap();
        snapshot.constant_buffers.insert(
            (4, 3),
            exosphere_gpu_ir::maxwell_state::ConstantBufferBinding {
                size_raw: 16,
                address: exosphere_gpu_ir::RawGpuAddress { a: 0, b: 0x200000 },
                bind_raw: (3 << 4) | 1,
            },
        );
        let prepared = source
            .prepare_fragment(&interface, &snapshot, 4, &space)
            .unwrap();
        assert_eq!(prepared.shader.address, source.address);
        assert_eq!(prepared.layout, module.graphics_pipelines[0].layout);
        // The host-only reference vertex shader has no descriptors; combine its
        // actual interface with the encoded fragment's prepared live resources.
        let vertex_module = module.shaders[0].clone();
        let vertex = exosphere_gpu_ir::compiler::PreparedShader {
            textures: Vec::new(),
            layout: exosphere_gpu_ir::maxwell_constant::uniform_descriptor_layout(&vertex_module)
                .unwrap(),
            spirv: exosphere_gpu_ir::spirv::compile(&vertex_module).unwrap(),
            shader: exosphere_gpu_ir::compiler::CompiledShader {
                address: 0,
                graphics_source: None,
                graphics_header: None,
                module: vertex_module,
                stats: Default::default(),
            },
            bindings: vec![],
        };
        let (layout, bindings) =
            exosphere_gpu_ir::compiler::merge_graphics_resources(&vertex, &prepared).unwrap();
        assert_eq!(layout, module.graphics_pipelines[0].layout);
        module.graphics_pipelines[0].layout = layout;
        module.shaders[1] = prepared.shader.module;
        let draw = module
            .commands
            .iter()
            .position(|command| matches!(command, Command::Draw { .. }))
            .unwrap();
        module.commands.splice(draw..draw, bindings);
        let plan = crate::VulkanBackend::new().plan(&module).unwrap();
        let color = [0.2f32, 0.8, 1., 1.]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        resources.insert(id, color.clone(), false);
        backend
            .execute_plan_with_resources(&plan, &mut resources)
            .unwrap();
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        resources.insert(
            id,
            [0f32, 0., 0., 1.]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
            false,
        );
        backend
            .execute_plan_with_resources(&plan, &mut resources)
            .unwrap();
        assert!(
            backend
                .readback_rgba8(target)
                .unwrap()
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 255])
        );
        resources.insert(id, color, false);
        backend
            .execute_plan_with_resources(&plan, &mut resources)
            .unwrap();
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
    }

    fn run_native_vertex_fixture(arithmetic: bool, uniform: bool) {
        use exosphere_gpu_ir::maxwell_state::{DrawStateSnapshot, method};
        use exosphere_gpu_ir::{
            BufferDesc, BufferUsage, Command, ShaderOperation, ShaderStage, VertexInputRate,
        };
        use exosphere_gpu_ir::{GpuAddressSpace, GpuMapPermissions, GpuMapping};
        let reference = GpuModule::triangle_vulkan_smoke_test(64, 64);
        let target = reference.textures[0].id;
        let mut backend = AshVulkanDevice::create().unwrap();
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&reference).unwrap(),
                &mut InMemoryGpuResources::new(),
            )
            .unwrap();
        let expected = backend.readback_rgba8(target).unwrap();
        let mut module = reference.clone();
        // Encoded fragment constants populate the physical color registers.
        // The header, rather than the last constant definition, selects exports.
        let mut fragment_bytes = vec![0; exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE];
        fragment_bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        let mov =
            |dst: u64, value: f32| 0x010000000007f000 | (u64::from(value.to_bits()) << 20) | dst;
        fragment_bytes.extend(
            [
                0,
                mov(0, 0.2),
                mov(1, 0.8),
                mov(2, 1.0),
                0,
                0x5c98000000000000 | (15 << 39) | (2 << 20) | (7 << 16) | 3,
                // Unrelated -1 bits must not become the fragment output color.
                0x3898000000000000 | (1 << 56) | (15 << 39) | (0x7ffff << 20) | (7 << 16) | 7,
                0xe30000000007000f,
            ]
            .into_iter()
            .flat_map(u64::to_le_bytes),
        );
        let fragment = exosphere_gpu_ir::maxwell_shader::parse_graphics(
            ShaderStage::Fragment,
            &fragment_bytes,
        )
        .unwrap();
        let (lifted_fragment, fragment_stats) =
            exosphere_gpu_ir::maxwell_shader::lift_semantic(104, &fragment);
        assert_eq!(fragment_stats.opaque, 0);
        module.shaders[1] = exosphere_gpu_ir::maxwell_fragment::lower_fragment_outputs(
            &lifted_fragment,
            fragment.header.as_ref().unwrap(),
        )
        .unwrap();
        module.graphics_pipelines[0].fragment_shader = 104;
        module.shaders[0].id = 103;
        module.shaders[0].variant_key = b"attribute-position-vs".to_vec();
        let pipeline = &mut module.graphics_pipelines[0];
        pipeline.id = 103;
        pipeline.vertex_shader = 103;
        let id = ResourceId(0x21000);
        let mut bytes = vec![0xff; 8];
        for mut position in [[-1f32, -1., 0., 1.], [1., -1., 0., 1.], [-1., 1., 0., 1.]] {
            if arithmetic {
                // The shader must undo these changes to match reference geometry.
                position[0] -= 1.0;
                position[1] *= 2.0;
            }
            bytes.extend([0xff; 4]);
            bytes.extend(position.into_iter().flat_map(f32::to_le_bytes));
        }
        // Synthetic Maxwell descriptors feed the real Vulkan input path. The
        // shader/location pairing remains explicit, not inferred from guest slots.
        let mut snapshot = DrawStateSnapshot::default();
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_PER_INSTANCE_BASE + 1, 0);
        snapshot.raw_registers.insert(
            method::VERTEX_ATTRIBUTE_BASE + 2,
            1 | (4 << 7) | (1 << 21) | (7 << 27),
        );
        for (offset, word) in [0x1014, 0, 0x100008, 0].into_iter().enumerate() {
            snapshot
                .raw_registers
                .insert(method::VERTEX_STREAM_BASE + 4 + offset as u16, word);
        }
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_LIMIT_BASE + 2, 0);
        snapshot.raw_registers.insert(
            method::VERTEX_STREAM_LIMIT_BASE + 3,
            0x100000 + bytes.len() as u32 - 1,
        );
        let mut address_space = GpuAddressSpace::new();
        address_space
            .map(GpuMapping {
                guest_base: 0x100000,
                size: bytes.len() as u64,
                resource: id,
                resource_offset: 0,
                permissions: GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        let input = snapshot
            .prepare_shader_vertex_range(
                exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
                    slot: 2,
                    location: 2,
                    format: exosphere_gpu_ir::VertexFormat::Rgba32Float,
                },
                VertexInputRate::Vertex,
                0,
                3,
                &address_space,
            )
            .unwrap();
        // Real ALD/AST/EXIT encodings, not prewritten canonical shader operations.
        let operands = (3u64 << 47) | (255 << 39) | (7 << 16) | (255 << 8) | 7;
        let mut shader_bytes = vec![0; exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE];
        let mut instructions = vec![
            0u64,
            0xefd8000000000000 | operands | (0xa0 << 20),
            0x010000000007f009, // MOV32I R9, 0.0 bits, full lane mask.
            0x0100000000000000 | (0x3f800000u64 << 20) | (7 << 16) | (15 << 12) | 10,
            0, // Next Maxwell scheduler bundle.
            0xeff0000000000000 | (operands & !(3 << 47)) | (1 << 47) | (0x70 << 20),
            0xeff0000000000000 | (operands & !((3 << 47) | 0xff)) | (1 << 47) | (0x78 << 20) | 9,
            0xe30000000007000f,
        ];
        if arithmetic {
            instructions.splice(
                5..5,
                [
                    0x0800000000000000
                        | (0x3f800000u64 << 20)
                        | (1 << 55)
                        | (7 << 16)
                        | (7 << 8)
                        | 7, // FADD32I R7, R7, 1, FTZ.
                    0x1e00000000000000
                        | (1 << 53)
                        | (0x3f000000u64 << 20)
                        | (7 << 16)
                        | (8 << 8)
                        | 8, // FMUL32I R8, R8, 0.5, FTZ.
                ],
            );
            instructions.insert(8, 0); // Scheduler for the final AST/EXIT bundle.
        }
        if uniform {
            let mov = 0x4c98000000000000 | (3 << 34) | (15 << 39) | (7 << 16);
            instructions[2] = mov | 9; // MOV R9, c[3][0]
            instructions[3] = mov | (1 << 20) | 10; // MOV R10, c[3][4]
        }
        shader_bytes.extend(instructions.into_iter().flat_map(u64::to_le_bytes));
        let program =
            exosphere_gpu_ir::maxwell_shader::parse_graphics(ShaderStage::Vertex, &shader_bytes)
                .unwrap();
        let (lifted, stats) = exosphere_gpu_ir::maxwell_shader::lift_semantic(103, &program);
        assert_eq!(stats.opaque, 0);
        let source = exosphere_gpu_ir::compiler::CompiledShader {
            address: 0x100080,
            graphics_source: None,
            graphics_header: program.header.clone(),
            module: lifted.clone(),
            stats,
        };
        let lifted = if uniform {
            exosphere_gpu_ir::maxwell_constant::lower_constant_interface(
                &lifted,
                &[
                    exosphere_gpu_ir::maxwell_constant::ConstantBufferInterface {
                        slot: 3,
                        set: 0,
                        binding: 7,
                        byte_size: 16,
                    },
                ],
            )
            .unwrap()
        } else {
            lifted
        };
        module.shaders[0] = exosphere_gpu_ir::maxwell_vertex::lower_vertex_interface(
            &lifted,
            &[exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
                slot: 2,
                location: input.attribute.location,
                format: input.attribute.format,
            }],
        )
        .unwrap();
        for element in 0..3 {
            let fetch = snapshot
                .resolve_vertex_element(2, element, &address_space)
                .unwrap();
            assert_eq!(fetch.resource, id);
            assert_eq!(fetch.offset, 12 + element * 20);
            assert_eq!(fetch.length, 16);
        }
        snapshot.primitive_raw = Some(4);
        snapshot
            .raw_registers
            .insert(method::SET_PRIMITIVE_TOPOLOGY_CONTROL, 0);
        let draw_event = exosphere_gpu_ir::maxwell_state::MaxwellEvent::DrawVertexArray {
            first: 0,
            count: 3,
            state: Box::new(snapshot.clone()),
        };
        let prepared_draw = exosphere_gpu_ir::pipeline::prepare_vertex_array_draw(
            &draw_event,
            &[exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
                slot: 2,
                location: input.attribute.location,
                format: input.attribute.format,
            }],
            &address_space,
        )
        .unwrap();
        let allocation = BufferDesc {
            id,
            size: bytes.len() as u64,
            usage: BufferUsage::Vertex,
        };
        let draw_buffers = exosphere_gpu_ir::pipeline::vertex_draw_buffers(
            &prepared_draw,
            std::slice::from_ref(&allocation),
        )
        .unwrap();
        assert_eq!(draw_buffers.as_slice(), std::slice::from_ref(&allocation));
        assert!(exosphere_gpu_ir::pipeline::vertex_draw_buffers(&prepared_draw, &[]).is_none());
        for invalid in [
            BufferDesc {
                size: allocation.size - 1,
                ..allocation.clone()
            },
            BufferDesc {
                usage: BufferUsage::Uniform,
                ..allocation.clone()
            },
        ] {
            assert!(
                exosphere_gpu_ir::pipeline::vertex_draw_buffers(&prepared_draw, &[invalid])
                    .is_none()
            );
        }
        pipeline.vertex_input = prepared_draw.vertex_input;
        pipeline.topology = prepared_draw.topology;
        for register in [
            method::SET_DEPTH_TEST,
            method::SET_DEPTH_WRITE,
            method::SET_STENCIL_TEST,
        ] {
            snapshot.raw_registers.insert(register, 0);
        }
        pipeline.fixed_function.depth_stencil =
            exosphere_gpu_ir::pipeline::captured_depth_stencil(&snapshot).unwrap();
        let mut excessive_draw = draw_event.clone();
        let exosphere_gpu_ir::maxwell_state::MaxwellEvent::DrawVertexArray { count, .. } =
            &mut excessive_draw
        else {
            unreachable!()
        };
        *count = 4;
        assert!(matches!(
            exosphere_gpu_ir::pipeline::prepare_vertex_array_draw(
                &excessive_draw,
                &[exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
                    slot: 2,
                    location: input.attribute.location,
                    format: input.attribute.format,
                }],
                &address_space,
            ),
            Err(exosphere_gpu_ir::pipeline::VertexArrayDrawError::Vertex(_))
        ));
        let uniform_id = ResourceId(0x21002);
        if uniform {
            let layout = exosphere_gpu_ir::maxwell_constant::uniform_pipeline_layout(&[
                &module.shaders[0],
                &module.shaders[1],
            ])
            .unwrap();
            assert_eq!(layout.descriptors.len(), 1);
            pipeline.layout = layout;
            let uniform_allocation = BufferDesc {
                id: uniform_id,
                size: 16,
                usage: BufferUsage::Uniform,
            };
            address_space
                .map(GpuMapping {
                    guest_base: 0x200000,
                    size: 16,
                    resource: uniform_id,
                    resource_offset: 0,
                    permissions: GpuMapPermissions::READ_ONLY,
                })
                .unwrap();
            snapshot.constant_buffers.insert(
                (0, 3),
                exosphere_gpu_ir::maxwell_state::ConstantBufferBinding {
                    size_raw: 16,
                    address: exosphere_gpu_ir::maxwell_state::RawGpuAddress { a: 0, b: 0x200000 },
                    bind_raw: (3 << 4) | 1,
                },
            );
            let interface = [
                exosphere_gpu_ir::maxwell_constant::ConstantBufferInterface {
                    slot: 3,
                    set: 0,
                    binding: 7,
                    byte_size: 16,
                },
            ];
            let attributes = [exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
                slot: 2,
                location: input.attribute.location,
                format: input.attribute.format,
            }];
            snapshot.constant_buffers.get_mut(&(0, 3)).unwrap().size_raw = 8;
            assert!(matches!(
                source.prepare_vertex(&attributes, &interface, &snapshot, 0, &address_space),
                Err(exosphere_gpu_ir::compiler::GpuCompileError::Binding(
                    exosphere_gpu_ir::maxwell_state::ConstantBufferReadError::OutOfBounds
                ))
            ));
            snapshot.constant_buffers.get_mut(&(0, 3)).unwrap().size_raw = 16;
            let prepared = source
                .prepare_vertex(&attributes, &interface, &snapshot, 0, &address_space)
                .unwrap();
            assert_eq!(prepared.shader.address, source.address);
            assert_eq!(prepared.layout, pipeline.layout);
            let allocations = exosphere_gpu_ir::compiler::prepared_uniform_buffers(
                &prepared,
                std::slice::from_ref(&uniform_allocation),
            )
            .unwrap();
            assert!(exosphere_gpu_ir::compiler::prepared_uniform_buffers(&prepared, &[]).is_err());
            for invalid in [
                BufferDesc {
                    size: 15,
                    ..uniform_allocation.clone()
                },
                BufferDesc {
                    usage: BufferUsage::Vertex,
                    ..uniform_allocation.clone()
                },
            ] {
                assert!(
                    exosphere_gpu_ir::compiler::prepared_uniform_buffers(&prepared, &[invalid])
                        .is_err()
                );
            }
            module.buffers.extend(allocations);
            module.shaders[0] = prepared.shader.module;
            let mut bindings = prepared.bindings;
            assert_eq!(bindings.len(), 1);
            let bind = bindings.remove(0);
            let draw = module
                .commands
                .iter()
                .position(|command| matches!(command, Command::Draw { .. }))
                .unwrap();
            module.commands.insert(draw, bind);
        }
        module.buffers.extend(draw_buffers);
        for command in &mut module.commands {
            if let Command::BindGraphicsPipeline { pipeline } = command {
                *pipeline = 103;
            }
        }
        let draw = module
            .commands
            .iter()
            .position(|command| matches!(command, Command::Draw { .. }))
            .unwrap();
        module.commands[draw] = prepared_draw.draw;
        module.commands.splice(draw..draw, prepared_draw.bindings);
        let mut incomplete_fragment = module.clone();
        incomplete_fragment.shaders[1]
            .operations
            .retain(|operation| {
                !matches!(operation, ShaderOperation::StoreOutput { component: 3, .. })
            });
        assert!(matches!(
            crate::VulkanBackend::new().plan(&incomplete_fragment),
            Err(crate::VulkanError::ShaderNeedsTranslation)
        ));
        let plan = crate::VulkanBackend::new().plan(&module).unwrap();
        let mut resources = InMemoryGpuResources::new();
        resources.insert(id, bytes.clone(), false);
        let uniform_bytes = [0f32, 1., 0., 0.]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        if uniform {
            resources.insert(uniform_id, uniform_bytes.clone(), false);
        }
        if arithmetic {
            let mut float_properties = vk::PhysicalDeviceFloatControlsProperties::default();
            let mut properties =
                vk::PhysicalDeviceProperties2::default().push_next(&mut float_properties);
            unsafe {
                backend
                    .instance
                    .get_physical_device_properties2(backend.physical, &mut properties)
            };
            if float_properties.shader_signed_zero_inf_nan_preserve_float32 != vk::TRUE
                || float_properties.shader_rounding_mode_rte_float32 != vk::TRUE
                || float_properties.shader_denorm_flush_to_zero_float32 != vk::TRUE
            {
                assert!(matches!(
                    backend.execute_plan_with_resources(&plan, &mut resources),
                    Err(crate::VulkanError::UnsupportedFeature(_))
                ));
                assert!(!backend.shader_modules.contains_key(&103));
                eprintln!(
                    "Arithmetic execution NOT verified: device lacks required Float32 modes; rejection verified."
                );
                return;
            }
        }
        backend
            .execute_plan_with_resources(&plan, &mut resources)
            .unwrap();
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        if uniform {
            // Reject malformed bindings without submitting their recorded clear
            // or disturbing the previous image. Then reuse the same device.
            let mut invalid_ranges = vec![(16, 0), (0, 17), (u64::MAX, 4)];
            let limits = unsafe {
                backend
                    .instance
                    .get_physical_device_properties(backend.physical)
            }
            .limits;
            if limits.min_uniform_buffer_offset_alignment > 1 {
                invalid_ranges.push((1, 4));
            }
            for (bad_offset, bad_range) in invalid_ranges {
                let mut invalid = module.clone();
                let mut changed = false;
                for command in &mut invalid.commands {
                    if let Command::BindBufferDescriptor {
                        buffer,
                        offset,
                        range,
                        ..
                    } = command
                    {
                        if *buffer == uniform_id {
                            *offset = bad_offset;
                            *range = bad_range;
                            changed = true;
                        }
                    }
                }
                assert!(changed);
                let rejected = match crate::VulkanBackend::new().plan(&invalid) {
                    Ok(invalid_plan) => {
                        backend.execute_plan_with_resources(&invalid_plan, &mut resources)
                    }
                    Err(error) => Err(error),
                };
                assert!(rejected.is_err(), "invalid uniform descriptor admitted");
                assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
                backend
                    .execute_plan_with_resources(&plan, &mut resources)
                    .unwrap();
                assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
            }
            // Negative W clips all vertices. Same pipeline/descriptor, new CPU
            // bytes: a cached or compile-time constant would retain the triangle.
            let clipped = [0f32, -1., 0., 0.]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>();
            resources.insert(uniform_id, clipped, false);
            backend
                .execute_plan_with_resources(&plan, &mut resources)
                .unwrap();
            assert!(
                backend
                    .readback_rgba8(target)
                    .unwrap()
                    .chunks_exact(4)
                    .all(|pixel| pixel == [0, 0, 0, 255])
            );
            resources.insert(uniform_id, uniform_bytes, false);
            backend
                .execute_plan_with_resources(&plan, &mut resources)
                .unwrap();
            assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        }
        snapshot.raw_registers.insert(method::SCISSOR_BASE, 1);
        snapshot
            .raw_registers
            .insert(method::SCISSOR_BASE + 1, 8 | (40 << 16));
        snapshot
            .raw_registers
            .insert(method::SCISSOR_BASE + 2, 8 | (48 << 16));
        let exosphere_gpu_ir::pipeline::CapturedScissor::Enabled(scissor) =
            exosphere_gpu_ir::pipeline::captured_scissor(&snapshot, 0).unwrap()
        else {
            panic!("fixture scissor must be enabled")
        };
        let mut scissored = module.clone();
        let draw = scissored
            .commands
            .iter()
            .position(|command| matches!(command, Command::Draw { .. }))
            .unwrap();
        scissored
            .commands
            .insert(draw, Command::SetScissor { scissor });
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&scissored).unwrap(),
                &mut resources,
            )
            .unwrap();
        let clipped_pixels = backend.readback_rgba8(target).unwrap();
        let mut retained_colored_pixels = 0;
        for (index, pixel) in clipped_pixels.chunks_exact(4).enumerate() {
            let x = index % 64;
            let y = index / 64;
            if (8..40).contains(&x) && (8..48).contains(&y) {
                assert_eq!(pixel, &expected[index * 4..index * 4 + 4]);
                retained_colored_pixels += usize::from(pixel != [0, 0, 0, 255]);
            } else {
                assert_eq!(pixel, [0, 0, 0, 255]);
            }
        }
        assert!(retained_colored_pixels > 0);
        let viewport = exosphere_gpu_ir::ViewportDesc {
            x: 0.5,
            y: 48.25,
            width: 32.5,
            height: -32.0,
            min_depth: 0.75,
            max_depth: 0.25,
        };
        for (register, value) in [
            (method::SET_WINDOW_OFFSET_X, 0),
            (method::SET_WINDOW_OFFSET_Y, 0),
            (method::SET_WINDOW_ORIGIN, 0),
            (method::SET_VIEWPORT_SCALE_OFFSET, 1),
            (method::SET_VIEWPORT_CLIP_CONTROL, 1 | (3 << 11)),
        ] {
            snapshot.raw_registers.insert(register, value);
        }
        for (field, value) in [16.25f32, -16.0, -0.5, 16.75, 32.25, 0.75]
            .into_iter()
            .enumerate()
        {
            snapshot
                .raw_registers
                .insert(method::VIEWPORT_BASE + field as u16, value.to_bits());
        }
        snapshot
            .raw_registers
            .insert(method::VIEWPORT_BASE + 6, 0x6420);
        snapshot.raw_registers.insert(method::VIEWPORT_BASE + 7, 0);
        let captured_viewport =
            exosphere_gpu_ir::pipeline::captured_viewport(&snapshot, 0).unwrap();
        assert_eq!(captured_viewport, viewport);
        let with_viewport = |source: &GpuModule, viewport| {
            let mut changed = source.clone();
            for command in &mut changed.commands {
                if let Command::SetViewport { viewport: current } = command {
                    *current = viewport;
                }
            }
            changed
        };
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new()
                    .plan(&with_viewport(&reference, viewport))
                    .unwrap(),
                &mut InMemoryGpuResources::new(),
            )
            .unwrap();
        let viewport_expected = backend.readback_rgba8(target).unwrap();
        assert_ne!(viewport_expected, expected);
        assert!(
            viewport_expected
                .chunks_exact(4)
                .any(|pixel| pixel != [0, 0, 0, 255])
        );
        // Disabled guest scissoring must override any prior small rectangle.
        snapshot.raw_registers.insert(method::SCISSOR_BASE, 0);
        let mut viewport_module = module.clone();
        viewport_module.commands.retain(|command| {
            !matches!(
                command,
                Command::SetViewport { .. } | Command::SetScissor { .. }
            )
        });
        let draw = viewport_module
            .commands
            .iter()
            .position(|command| matches!(command, Command::Draw { .. }))
            .unwrap();
        viewport_module.commands.splice(
            draw..draw,
            exosphere_gpu_ir::pipeline::prepare_raster_area(&snapshot, 0, 64, 64).unwrap(),
        );
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&viewport_module).unwrap(),
                &mut resources,
            )
            .unwrap();
        assert_eq!(backend.readback_rgba8(target).unwrap(), viewport_expected);
        for invalid in [
            exosphere_gpu_ir::ViewportDesc {
                x: f32::NAN,
                ..viewport
            },
            exosphere_gpu_ir::ViewportDesc {
                width: -1.0,
                ..viewport
            },
            exosphere_gpu_ir::ViewportDesc {
                min_depth: 1.1,
                ..viewport
            },
        ] {
            let invalid_plan = crate::VulkanBackend::new()
                .plan(&with_viewport(&module, invalid))
                .unwrap();
            assert!(
                backend
                    .execute_plan_with_resources(&invalid_plan, &mut resources)
                    .is_err()
            );
            assert_eq!(backend.readback_rgba8(target).unwrap(), viewport_expected);
        }
        for (register, value) in [
            (method::OGL_SET_CULL, 1),
            (method::OGL_SET_CULL_FACE, 0x408),
            (method::OGL_SET_FRONT_FACE, 0x901),
            (method::SET_FRONT_POLYGON_MODE, 0x1b02),
            (method::SET_BACK_POLYGON_MODE, 0x1b02),
            (method::SET_RASTER_ENABLE, 1),
            (method::SET_POLY_OFFSET_FILL, 0),
        ] {
            snapshot.raw_registers.insert(register, value);
        }
        let raster = exosphere_gpu_ir::pipeline::captured_raster(&snapshot).unwrap();
        let mut culled = module.clone();
        let pipeline = &mut culled.graphics_pipelines[0];
        // Distinct pipeline identity because these are different immutable states.
        pipeline.id = 105;
        pipeline.fixed_function.raster = raster;
        for command in &mut culled.commands {
            if let Command::BindGraphicsPipeline { pipeline } = command {
                *pipeline = 105;
            }
        }
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&culled).unwrap(),
                &mut resources,
            )
            .unwrap();
        assert!(
            backend
                .readback_rgba8(target)
                .unwrap()
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 255])
        );
        snapshot.raw_registers.insert(method::OGL_SET_CULL, 0);
        snapshot.raw_registers.insert(method::SET_RASTER_ENABLE, 0);
        let mut discarded = module.clone();
        discarded.graphics_pipelines[0].id = 106;
        discarded.graphics_pipelines[0].fixed_function.raster =
            exosphere_gpu_ir::pipeline::captured_raster(&snapshot).unwrap();
        for command in &mut discarded.commands {
            if let Command::BindGraphicsPipeline { pipeline } = command {
                *pipeline = 106;
            }
        }
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&discarded).unwrap(),
                &mut resources,
            )
            .unwrap();
        assert!(
            backend
                .readback_rgba8(target)
                .unwrap()
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 255])
        );
        snapshot.raw_registers.insert(method::OGL_SET_CULL, 1);
        snapshot.raw_registers.insert(method::SET_RASTER_ENABLE, 1);
        let mut coverage = [[[false; 2]; 2]; 2];
        for (viewport_index, (source, reference_pixels)) in
            [(&module, &expected), (&viewport_module, &viewport_expected)]
                .into_iter()
                .enumerate()
        {
            for (winding_index, winding) in [0x900, 0x901].into_iter().enumerate() {
                for (face_index, face) in [0x404, 0x405].into_iter().enumerate() {
                    snapshot
                        .raw_registers
                        .insert(method::OGL_SET_FRONT_FACE, winding);
                    snapshot
                        .raw_registers
                        .insert(method::OGL_SET_CULL_FACE, face);
                    let faces = exosphere_gpu_ir::pipeline::captured_faces(&snapshot).unwrap();
                    let mut candidate = source.clone();
                    let pipeline_id =
                        1000 + (viewport_index * 4 + winding_index * 2 + face_index) as u64;
                    let pipeline = &mut candidate.graphics_pipelines[0];
                    pipeline.id = pipeline_id;
                    pipeline.fixed_function.raster.cull = faces.cull;
                    pipeline.fixed_function.raster.front_face = faces.front_face;
                    pipeline.fixed_function.raster.polygon_mode = faces.polygon_mode;
                    for command in &mut candidate.commands {
                        if let Command::BindGraphicsPipeline { pipeline } = command {
                            *pipeline = pipeline_id;
                        }
                    }
                    backend
                        .execute_plan_with_resources(
                            &crate::VulkanBackend::new().plan(&candidate).unwrap(),
                            &mut resources,
                        )
                        .unwrap();
                    let pixels = backend.readback_rgba8(target).unwrap();
                    let visible = pixels.chunks_exact(4).any(|pixel| pixel != [0, 0, 0, 255]);
                    coverage[viewport_index][winding_index][face_index] = visible;
                    if visible {
                        assert_eq!(&pixels, reference_pixels);
                    } else {
                        assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
                    }
                }
                // Front/back selection must preserve exactly one orientation.
                assert_ne!(
                    coverage[viewport_index][winding_index][0],
                    coverage[viewport_index][winding_index][1]
                );
            }
            assert_ne!(coverage[viewport_index][0], coverage[viewport_index][1]);
        }
        // Negative viewport height reverses framebuffer winding, not the shader.
        assert_eq!(coverage[0][0], coverage[1][1]);
        assert_eq!(coverage[0][1], coverage[1][0]);
        let depth_target = ResourceId(0x21003);
        snapshot.raw_registers.insert(method::SET_DEPTH_TEST, 1);
        snapshot.raw_registers.insert(method::SET_DEPTH_WRITE, 0);
        let mut depth_reference = None;
        for (case, (compare, write)) in [(0x201, 0), (0x204, 0), (0x201, 1), (0x201, 0)]
            .into_iter()
            .enumerate()
        {
            snapshot
                .raw_registers
                .insert(method::SET_DEPTH_WRITE, write);
            snapshot
                .raw_registers
                .insert(method::SET_DEPTH_FUNC, compare);
            let mut depth_module = module.clone();
            let mut depth_texture = depth_module.textures[0].clone();
            depth_texture.id = depth_target;
            depth_texture.format = exosphere_gpu_ir::TextureFormat::D32Float;
            depth_module.textures.push(depth_texture);
            let pipeline_id = 1200 + case as u64;
            depth_module.graphics_pipelines[0].id = pipeline_id;
            depth_module.graphics_pipelines[0]
                .fixed_function
                .depth_stencil =
                exosphere_gpu_ir::pipeline::captured_depth_stencil(&snapshot).unwrap();
            for command in &mut depth_module.commands {
                if let Command::BindGraphicsPipeline { pipeline } = command {
                    *pipeline = pipeline_id;
                }
            }
            depth_module.commands.insert(
                0,
                Command::ClearDepthStencil {
                    target: depth_target,
                    depth_bits: 0.5f32.to_bits(),
                    stencil: 0,
                    aspect: exosphere_gpu_ir::ImageAspect::Depth,
                },
            );
            let begin = depth_module
                .commands
                .iter()
                .position(|command| matches!(command, Command::BeginRenderPass { .. }))
                .unwrap();
            depth_module.commands.insert(
                begin,
                Command::SetDepthStencilTarget {
                    texture: depth_target,
                },
            );
            if case == 0 {
                depth_reference = Some(depth_module.clone());
            }
            backend
                .execute_plan_with_resources(
                    &crate::VulkanBackend::new().plan(&depth_module).unwrap(),
                    &mut resources,
                )
                .unwrap();
            let pixels = backend.readback_rgba8(target).unwrap();
            if compare == 0x201 {
                assert_eq!(pixels, expected);
            } else {
                assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
            }
            if case >= 2 {
                // Probe the depth left by the first draw, without clearing it.
                // Color is cleared independently, so stale color cannot pass.
                let mut probe = depth_module.clone();
                probe
                    .commands
                    .retain(|command| !matches!(command, Command::ClearDepthStencil { .. }));
                snapshot.raw_registers.insert(method::SET_DEPTH_FUNC, 0x202);
                snapshot.raw_registers.insert(method::SET_DEPTH_WRITE, 0);
                let probe_id = 1300 + case as u64;
                probe.graphics_pipelines[0].id = probe_id;
                probe.graphics_pipelines[0].fixed_function.depth_stencil =
                    exosphere_gpu_ir::pipeline::captured_depth_stencil(&snapshot).unwrap();
                for command in &mut probe.commands {
                    if let Command::BindGraphicsPipeline { pipeline } = command {
                        *pipeline = probe_id;
                    }
                }
                backend
                    .execute_plan_with_resources(
                        &crate::VulkanBackend::new().plan(&probe).unwrap(),
                        &mut resources,
                    )
                    .unwrap();
                let pixels = backend.readback_rgba8(target).unwrap();
                if write == 1 {
                    assert_eq!(pixels, expected);
                } else {
                    assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
                }
            }
        }
        let mut biased = module.clone();
        for (register, value) in [
            (method::SET_DEPTH_BIAS_CONTROL, 1),
            (method::SET_DEPTH_BIAS, 2f32.to_bits()),
            (method::SET_SLOPE_SCALE_DEPTH_BIAS, 2f32.to_bits()),
            (method::SET_DEPTH_BIAS_CLAMP, 0),
            (method::SET_POLY_OFFSET_FILL, 1),
            (method::OGL_SET_CULL, 0),
        ] {
            snapshot.raw_registers.insert(register, value);
        }
        let captured_bias = exosphere_gpu_ir::pipeline::captured_raster(&snapshot).unwrap();
        for (case, hardware_units) in [0f32, 33_554_432.0].into_iter().enumerate() {
            snapshot
                .raw_registers
                .insert(method::SET_DEPTH_BIAS, hardware_units.to_bits());
            snapshot
                .raw_registers
                .insert(method::SET_SLOPE_SCALE_DEPTH_BIAS, 0);
            let mut displaced = depth_reference.as_ref().unwrap().clone();
            let pipeline_id = 1400 + case as u64;
            displaced.graphics_pipelines[0].id = pipeline_id;
            displaced.graphics_pipelines[0].fixed_function.raster =
                exosphere_gpu_ir::pipeline::captured_raster(&snapshot).unwrap();
            for command in &mut displaced.commands {
                match command {
                    Command::BindGraphicsPipeline { pipeline } => *pipeline = pipeline_id,
                    // Z=0 maps to 0.25, away from the floating-point exponent
                    // boundary at zero. Positive bias must cross depth 0.5.
                    Command::SetViewport { viewport } => {
                        viewport.min_depth = 0.25;
                        viewport.max_depth = 0.75;
                    }
                    _ => {}
                }
            }
            backend
                .execute_plan_with_resources(
                    &crate::VulkanBackend::new().plan(&displaced).unwrap(),
                    &mut resources,
                )
                .unwrap();
            let pixels = backend.readback_rgba8(target).unwrap();
            if case == 0 {
                assert_eq!(pixels, expected);
            } else {
                assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
            }
        }
        biased.graphics_pipelines[0].id = 1100;
        biased.graphics_pipelines[0]
            .fixed_function
            .raster
            .depth_bias = true;
        biased.graphics_pipelines[0]
            .fixed_function
            .raster
            .depth_bias_factors = exosphere_gpu_ir::resource::DepthBiasFactors {
            constant_bits: 1f32.to_bits(),
            slope_bits: 2f32.to_bits(),
            clamp_bits: 0,
        };
        assert_eq!(
            captured_bias.depth_bias_factors,
            biased.graphics_pipelines[0]
                .fixed_function
                .raster
                .depth_bias_factors
        );
        biased.graphics_pipelines[0].fixed_function.raster = captured_bias;
        for command in &mut biased.commands {
            if let Command::BindGraphicsPipeline { pipeline } = command {
                *pipeline = 1100;
            }
        }
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&biased).unwrap(),
                &mut resources,
            )
            .unwrap();
        // No depth attachment here: this verifies creation/submission, not a
        // measured depth displacement. The color coverage must remain intact.
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        biased.graphics_pipelines[0].id = 1101;
        biased.graphics_pipelines[0]
            .fixed_function
            .raster
            .depth_bias_factors
            .slope_bits = f32::NAN.to_bits();
        // A cached identity cannot hide changed state, including invalid bias.
        let mut reused_identity = biased.clone();
        reused_identity.graphics_pipelines[0].id = 1100;
        assert!(matches!(
            backend.execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&reused_identity).unwrap(),
                &mut resources,
            ),
            Err(crate::VulkanError::UnsupportedFeature(
                "graphics pipeline identity changed"
            ))
        ));
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        for command in &mut biased.commands {
            if let Command::BindGraphicsPipeline { pipeline } = command {
                *pipeline = 1101;
            }
        }
        assert!(matches!(
            backend.execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&biased).unwrap(),
                &mut resources,
            ),
            Err(crate::VulkanError::UnsupportedFeature(
                "non-finite depth bias"
            ))
        ));
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        for register in [
            method::SET_SINGLE_ROP_CONTROL,
            method::SET_LOGIC_OP,
            method::SET_BLEND_BASE,
        ] {
            snapshot.raw_registers.insert(register, 0);
        }
        for (case, mask) in [0u32, 1, 5, 15].into_iter().enumerate() {
            let raw = (mask & 1) | ((mask & 2) << 3) | ((mask & 4) << 6) | ((mask & 8) << 9);
            snapshot
                .raw_registers
                .insert(method::SET_CT_WRITE_BASE, raw);
            let mut masked = module.clone();
            let pipeline_id = 1500 + case as u64;
            masked.graphics_pipelines[0].id = pipeline_id;
            masked.graphics_pipelines[0].fixed_function.blend.targets[0] =
                exosphere_gpu_ir::pipeline::captured_unblended_target(&snapshot, 0).unwrap();
            for command in &mut masked.commands {
                match command {
                    Command::BindGraphicsPipeline { pipeline } => *pipeline = pipeline_id,
                    Command::ClearColor { value, .. } => *value = [0; 4],
                    _ => {}
                }
            }
            backend
                .execute_plan_with_resources(
                    &crate::VulkanBackend::new().plan(&masked).unwrap(),
                    &mut resources,
                )
                .unwrap();
            for (pixel, reference_pixel) in backend
                .readback_rgba8(target)
                .unwrap()
                .chunks_exact(4)
                .zip(expected.chunks_exact(4))
            {
                let covered = reference_pixel != [0, 0, 0, 255];
                for component in 0..4 {
                    let expected_component = if covered && mask & (1 << component) != 0 {
                        reference_pixel[component]
                    } else {
                        0
                    };
                    assert_eq!(pixel[component], expected_component);
                }
            }
        }
        for (register, value) in [
            (method::SET_BLEND_BASE, 1),
            (method::SET_CT_WRITE_BASE, 0x1111),
            (method::SET_BLEND_STATE_PER_TARGET, 0),
            (method::SET_BLEND_COLOR_OP, 0x800b),
            (method::SET_BLEND_ALPHA_OP, 0x800b),
            (method::SET_BLEND_COLOR_SOURCE_COEFF, 0x4001),
            (method::SET_BLEND_COLOR_DEST_COEFF, 0x4001),
            (method::SET_BLEND_ALPHA_SOURCE_COEFF, 0x4001),
            (method::SET_BLEND_ALPHA_DEST_COEFF, 0x4001),
        ] {
            snapshot.raw_registers.insert(register, value);
        }
        let mut blended = module.clone();
        blended.graphics_pipelines[0].id = 1600;
        blended.graphics_pipelines[0].fixed_function.blend.targets[0] =
            exosphere_gpu_ir::pipeline::captured_blend_target(&snapshot, 0).unwrap();
        for command in &mut blended.commands {
            match command {
                Command::BindGraphicsPipeline { pipeline } => *pipeline = 1600,
                Command::ClearColor { value, .. } => *value = [1f32.to_bits(); 4],
                _ => {}
            }
        }
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&blended).unwrap(),
                &mut resources,
            )
            .unwrap();
        for (pixel, reference_pixel) in backend
            .readback_rgba8(target)
            .unwrap()
            .chunks_exact(4)
            .zip(expected.chunks_exact(4))
        {
            if reference_pixel == [0, 0, 0, 255] {
                assert_eq!(pixel, [255; 4]);
            } else {
                for component in 0..4 {
                    assert_eq!(pixel[component], 255 - reference_pixel[component]);
                }
            }
        }
        for (register, value) in [
            (method::SET_BLEND_COLOR_OP, 0x8006),
            (method::SET_BLEND_ALPHA_OP, 0x8006),
            (method::SET_BLEND_COLOR_SOURCE_COEFF, 0x4304),
            (method::SET_BLEND_COLOR_DEST_COEFF, 0x4000),
            (method::SET_BLEND_ALPHA_DEST_COEFF, 0x4000),
        ] {
            snapshot.raw_registers.insert(register, value);
        }
        let mut alpha_blended = blended.clone();
        alpha_blended.graphics_pipelines[0].id = 1601;
        alpha_blended.graphics_pipelines[0]
            .fixed_function
            .blend
            .targets[0] = exosphere_gpu_ir::pipeline::captured_blend_target(&snapshot, 0).unwrap();
        for command in &mut alpha_blended.commands {
            match command {
                Command::BindGraphicsPipeline { pipeline } => *pipeline = 1601,
                Command::ClearColor { value, .. } => *value = [0, 0, 0, 0.25f32.to_bits()],
                _ => {}
            }
        }
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&alpha_blended).unwrap(),
                &mut resources,
            )
            .unwrap();
        for (pixel, reference_pixel) in backend
            .readback_rgba8(target)
            .unwrap()
            .chunks_exact(4)
            .zip(expected.chunks_exact(4))
        {
            if reference_pixel == [0, 0, 0, 255] {
                assert_eq!(pixel, [0, 0, 0, 64]);
            } else {
                for component in 0..3 {
                    assert_eq!(
                        pixel[component],
                        (f32::from(reference_pixel[component]) * 64.0 / 255.0).round() as u8
                    );
                }
                assert_eq!(pixel[3], 255);
            }
        }
        for register in [
            method::SET_PS_OUTPUT_SAMPLE_MASK_USAGE,
            method::SET_ANTI_ALIAS_ENABLE,
            method::SET_ANTI_ALIAS_ALPHA_CONTROL,
            method::SET_ANTI_ALIAS,
            method::SET_ALPHA_TO_COVERAGE_OVERRIDE,
        ] {
            snapshot.raw_registers.insert(register, 0);
        }
        snapshot.raw_registers.insert(method::SET_SAMPLE_MASK, 0x10);
        snapshot.raw_registers.insert(method::SET_DEPTH_TEST, 0);
        snapshot.raw_registers.insert(method::SET_DEPTH_WRITE, 0);
        snapshot
            .raw_registers
            .insert(method::SET_POLY_OFFSET_FILL, 0);
        for mask in [0, 1] {
            snapshot
                .raw_registers
                .insert(method::SET_COLOR_TARGET_SAMPLE_MASK, mask);
            let mut sampled = module.clone();
            let pipeline_id = 1700 + u64::from(mask);
            sampled.graphics_pipelines[0].id = pipeline_id;
            sampled.graphics_pipelines[0].fixed_function =
                exosphere_gpu_ir::pipeline::captured_fixed_function(&snapshot, &[0]).unwrap();
            assert!(
                exosphere_gpu_ir::pipeline::captured_fixed_function(&snapshot, &[0, 0]).is_none()
            );
            assert!(
                exosphere_gpu_ir::pipeline::captured_fixed_function(&snapshot, &[0, 1]).is_none()
            );
            assert!(exosphere_gpu_ir::pipeline::captured_fixed_function(&snapshot, &[]).is_none());
            assert!(exosphere_gpu_ir::pipeline::captured_fixed_function(&snapshot, &[8]).is_none());
            for register in [
                method::SET_RASTER_ENABLE,
                method::SET_DEPTH_TEST,
                method::SET_LOGIC_OP,
                method::SET_ANTI_ALIAS_ENABLE,
                method::SET_SAMPLE_MASK,
            ] {
                let mut incomplete = snapshot.clone();
                incomplete.raw_registers.remove(&register);
                assert!(
                    exosphere_gpu_ir::pipeline::captured_fixed_function(&incomplete, &[0])
                        .is_none()
                );
            }
            for command in &mut sampled.commands {
                if let Command::BindGraphicsPipeline { pipeline } = command {
                    *pipeline = pipeline_id;
                }
            }
            backend
                .execute_plan_with_resources(
                    &crate::VulkanBackend::new().plan(&sampled).unwrap(),
                    &mut resources,
                )
                .unwrap();
            let pixels = backend.readback_rgba8(target).unwrap();
            if mask == 1 {
                assert_eq!(pixels, expected);
            } else {
                assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
            }
        }
        snapshot.raw_registers.insert(method::SET_CT_SELECT, 1);
        snapshot.raw_registers.insert(method::SET_CT_MRT_ENABLE, 0);
        for (field, word) in [0, 0x300000, 64, 64, 0xd5, 0, 1, 0, 0]
            .into_iter()
            .enumerate()
        {
            snapshot
                .raw_registers
                .insert(method::COLOR_TARGET_BASE + field as u16, word);
        }
        address_space
            .map(GpuMapping {
                guest_base: 0x300000,
                size: 16384,
                resource: ResourceId(0x21004),
                resource_offset: 0,
                permissions: GpuMapPermissions::READ_WRITE,
            })
            .unwrap();
        let attributes = [exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
            slot: 2,
            location: input.attribute.location,
            format: input.attribute.format,
        }];
        let constants = if uniform {
            vec![
                exosphere_gpu_ir::maxwell_constant::ConstantBufferInterface {
                    slot: 3,
                    set: 0,
                    binding: 7,
                    byte_size: 16,
                },
            ]
        } else {
            vec![]
        };
        let prepared_vertex = source
            .prepare_vertex(&attributes, &constants, &snapshot, 0, &address_space)
            .unwrap();
        let fragment_source = exosphere_gpu_ir::compiler::CompiledShader {
            address: 0x100100,
            graphics_source: None,
            graphics_header: fragment.header.clone(),
            module: lifted_fragment.clone(),
            stats: fragment_stats,
        };
        let prepared_fragment = fragment_source
            .prepare_fragment(&[], &snapshot, 4, &address_space)
            .unwrap();
        let captured_draw = exosphere_gpu_ir::maxwell_state::MaxwellEvent::DrawVertexArray {
            first: 0,
            count: 3,
            state: Box::new(snapshot.clone()),
        };
        let assembly_inputs = exosphere_gpu_ir::compiler::ColorVertexArrayInputs {
            pipeline_id: 1800,
            image: target,
            vertex: &prepared_vertex,
            fragment: &prepared_fragment,
            attributes: &attributes,
            allocations: &module.buffers,
        };
        let wrong_attributes = [exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
            location: input.attribute.location + 1,
            ..attributes[0]
        }];
        for attributes in [&wrong_attributes[..], &[][..]] {
            let mismatched = exosphere_gpu_ir::compiler::ColorVertexArrayInputs {
                attributes,
                ..assembly_inputs
            };
            assert!(matches!(
                exosphere_gpu_ir::compiler::prepare_color_vertex_array_module(
                    &captured_draw,
                    &mismatched,
                    &address_space,
                ),
                Err(exosphere_gpu_ir::compiler::GpuCompileError::InvalidDrawState)
            ));
        }
        for (register, value) in [
            (method::SET_CT_SELECT, 2),
            (method::SET_DEPTH_TEST, 1),
            (method::SET_CT_SELECT, 1 | (3 << 4)),
            (method::SET_ANTI_ALIAS_ENABLE, 1),
        ] {
            let mut invalid_state = snapshot.clone();
            invalid_state.raw_registers.insert(register, value);
            let invalid_draw = exosphere_gpu_ir::maxwell_state::MaxwellEvent::DrawVertexArray {
                first: 0,
                count: 3,
                state: Box::new(invalid_state),
            };
            assert!(
                exosphere_gpu_ir::compiler::prepare_color_vertex_array_module(
                    &invalid_draw,
                    &assembly_inputs,
                    &address_space,
                )
                .is_err()
            );
        }
        let mut assembled = exosphere_gpu_ir::compiler::prepare_color_vertex_array_module(
            &captured_draw,
            &assembly_inputs,
            &address_space,
        )
        .unwrap();
        assert!(
            !assembled
                .commands
                .iter()
                .any(|command| matches!(command, Command::ClearColor { .. }))
        );
        // The host fixture explicitly initializes its attachment; assembly itself
        // must preserve whatever guest pixels its caller supplied.
        assembled.commands.insert(
            0,
            Command::ClearColor {
                target,
                value: [0, 0, 0, 1f32.to_bits()],
            },
        );
        backend
            .execute_plan_with_resources(
                &crate::VulkanBackend::new().plan(&assembled).unwrap(),
                &mut resources,
            )
            .unwrap();
        assert_eq!(backend.readback_rgba8(target).unwrap(), viewport_expected);
        let mut degenerate = bytes.clone();
        for vertex in 0..3 {
            degenerate[12 + vertex * 20..28 + vertex * 20].copy_from_slice(
                &[0f32, 0., 0., 1.]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>(),
            );
        }
        resources.insert(id, degenerate, false);
        backend
            .execute_plan_with_resources(&plan, &mut resources)
            .unwrap();
        assert!(
            backend
                .readback_rgba8(target)
                .unwrap()
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 255])
        );
        resources.insert(id, bytes, false);
        backend
            .execute_plan_with_resources(&plan, &mut resources)
            .unwrap();
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        let indices = ResourceId(0x21001);
        module.buffers.push(BufferDesc {
            id: indices,
            size: 8,
            usage: BufferUsage::IndexVertexUniform,
        });
        let draw = module
            .commands
            .iter()
            .position(|command| matches!(command, Command::Draw { .. }))
            .unwrap();
        module.commands.splice(
            draw..=draw,
            [
                Command::BindIndexBuffer {
                    buffer: indices,
                    offset: 2,
                    index_type: exosphere_gpu_ir::IndexType::U16,
                },
                Command::DrawIndexed {
                    indices: 3,
                    instances: 1,
                    first_index: 0,
                    vertex_offset: -1,
                },
            ],
        );
        let indexed_plan = crate::VulkanBackend::new().plan(&module).unwrap();
        resources.insert(
            indices,
            [99u16, 1, 2, 3]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
            false,
        );
        backend
            .execute_plan_with_resources(&indexed_plan, &mut resources)
            .unwrap();
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        resources.insert(
            indices,
            [99u16, 1, 2, 4]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
            false,
        );
        assert!(
            backend
                .execute_plan_with_resources(&indexed_plan, &mut resources)
                .is_err()
        );
        assert_eq!(backend.readback_rgba8(target).unwrap(), expected);
        // Use one full allocation for both vertex fetch and index fetch. A fresh
        // device avoids changing an existing native allocation's descriptor.
        let mut shared = resources.bytes(id).unwrap().to_vec();
        while shared.len() % 4 != 0 {
            shared.push(0);
        }
        let index_offset = shared.len();
        shared.extend([99u16, 1, 2, 3].into_iter().flat_map(u16::to_le_bytes));
        module.buffers.retain(|buffer| buffer.id != indices);
        let allocation = module
            .buffers
            .iter_mut()
            .find(|buffer| buffer.id == id)
            .unwrap();
        allocation.size = shared.len() as u64;
        allocation.usage = BufferUsage::IndexVertexUniform;
        for command in &mut module.commands {
            if let Command::BindIndexBuffer { buffer, offset, .. } = command {
                *buffer = id;
                *offset = index_offset as u64 + 2;
            }
        }
        let shared_plan = crate::VulkanBackend::new().plan(&module).unwrap();
        let mut shared_backend = AshVulkanDevice::create().unwrap();
        for degenerate in [false, true, false] {
            let mut live = shared.clone();
            if degenerate {
                live[index_offset + 2..index_offset + 8].copy_from_slice(
                    &[1u16, 1, 1]
                        .into_iter()
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<_>>(),
                );
            }
            resources.insert(id, live, false);
            shared_backend
                .execute_plan_with_resources(&shared_plan, &mut resources)
                .unwrap();
            let actual = shared_backend.readback_rgba8(target).unwrap();
            if degenerate {
                assert!(actual.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
            } else {
                assert_eq!(actual, expected);
            }
        }
        if !arithmetic && !uniform {
            // Native VertexIndex includes the indexed draw's signed base vertex.
            // Interpret its register bits as depth to certify integer preservation
            // without relying on an unimplemented conversion operation.
            for (ordinal, bias) in [0, 0x40000000, 0].into_iter().enumerate() {
                let shader = &mut module.shaders[0];
                let depth = shader
                    .operations
                    .iter_mut()
                    .find(|operation| {
                        matches!(
                            operation,
                            exosphere_gpu_ir::ShaderOperation::ConstantBits32 { dst: 9, .. }
                                | exosphere_gpu_ir::ShaderOperation::LoadVertexIndex { dst: 9, .. }
                        )
                    })
                    .unwrap();
                *depth = exosphere_gpu_ir::ShaderOperation::LoadVertexIndex { dst: 9, bias };
                shader.id = 500 + ordinal as u64;
                module.graphics_pipelines[0].id = shader.id;
                module.graphics_pipelines[0].vertex_shader = shader.id;
                for command in &mut module.commands {
                    if let Command::BindGraphicsPipeline { pipeline } = command {
                        *pipeline = shader.id;
                    }
                }
                let plan = crate::VulkanBackend::new().plan(&module).unwrap();
                shared_backend
                    .execute_plan_with_resources(&plan, &mut resources)
                    .unwrap();
                let pixels = shared_backend.readback_rgba8(target).unwrap();
                if bias == 0 {
                    assert_eq!(pixels, expected);
                } else {
                    assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
                }
            }
        }
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_indexed_draw_matches_triangle_pixels() {
        use exosphere_gpu_ir::{BufferDesc, BufferUsage, Command, IndexType};
        let reference = GpuModule::triangle_vulkan_smoke_test(64, 64);
        let target = reference.textures[0].id;
        let mut backend = AshVulkanDevice::create().unwrap();
        let missing = BufferDesc {
            id: ResourceId(0xffff0),
            size: 8,
            usage: BufferUsage::Index,
        };
        let mut incomplete = InMemoryGpuResources::new();
        assert!(backend.ensure_buffer(&missing, &mut incomplete).is_err());
        assert!(!backend.buffers.contains_key(&missing.id));
        incomplete.insert(missing.id, vec![0; 4], false);
        assert!(matches!(
            backend.ensure_buffer(&missing, &mut incomplete),
            Err(crate::VulkanError::UnsupportedFeature(
                "draw input upload provider too small"
            ))
        ));
        assert!(!backend.buffers.contains_key(&missing.id));
        let reference_plan = crate::VulkanBackend::new().plan(&reference).unwrap();
        backend
            .execute_plan_with_resources(&reference_plan, &mut InMemoryGpuResources::new())
            .unwrap();
        let expected = backend.readback_rgba8(target).unwrap();

        for (index_type, width) in [(IndexType::U8, 1), (IndexType::U16, 2), (IndexType::U32, 4)] {
            let mut module = reference.clone();
            let buffer = ResourceId(0xfffe + width as u64);
            let bytes: Vec<u8> = [99u32, 99, 1, 2, 3]
                .into_iter()
                .flat_map(|value| value.to_le_bytes().into_iter().take(width))
                .collect();
            module.buffers.push(BufferDesc {
                id: buffer,
                size: bytes.len() as u64,
                usage: BufferUsage::Index,
            });
            let position = module
                .commands
                .iter()
                .position(|command| matches!(command, Command::Draw { .. }))
                .unwrap();
            // Both binding offset and first_index skip one sentinel each.
            // Base vertex -1 turns the remaining 1/2/3 into reference vertices 0/1/2.
            module.commands.splice(
                position..=position,
                [
                    Command::BindIndexBuffer {
                        buffer,
                        offset: width as u64,
                        index_type,
                    },
                    Command::DrawIndexed {
                        indices: 3,
                        instances: 1,
                        first_index: 1,
                        vertex_offset: -1,
                    },
                ],
            );
            let plan = crate::VulkanBackend::new().plan(&module).unwrap();
            let mut resources = InMemoryGpuResources::new();
            resources.insert(buffer, bytes, false);
            if index_type == IndexType::U8 && !backend.index_type_uint8 {
                assert!(matches!(
                    backend.execute_plan_with_resources(&plan, &mut resources),
                    Err(crate::VulkanError::UnsupportedFeature(
                        "uint8 index feature not enabled"
                    ))
                ));
                continue;
            }
            for _ in 0..2 {
                backend
                    .execute_plan_with_resources(&plan, &mut resources)
                    .unwrap();
                let actual = backend.readback_rgba8(target).unwrap();
                assert!(
                    actual == expected,
                    "indexed pixel mismatch for {index_type:?}"
                );
            }
            let original = module.buffers[0].clone();
            for changed in [
                BufferDesc {
                    size: original.size + 4,
                    ..original.clone()
                },
                BufferDesc {
                    usage: BufferUsage::Uniform,
                    ..original.clone()
                },
            ] {
                assert!(matches!(
                    backend.ensure_buffer(&changed, &mut resources),
                    Err(crate::VulkanError::UnsupportedFeature(
                        "cached buffer descriptor changed"
                    ))
                ));
                module.buffers[0] = changed;
                let incompatible = crate::VulkanBackend::new().plan(&module).unwrap();
                assert!(
                    backend
                        .execute_plan_with_resources(&incompatible, &mut resources)
                        .is_err()
                );
            }
            // Rejection must not replace the old allocation or poison the device.
            backend
                .execute_plan_with_resources(&plan, &mut resources)
                .unwrap();
            assert!(backend.readback_rgba8(target).unwrap() == expected);
            let original_bytes = resources.bytes(buffer).unwrap().to_vec();
            let degenerate: Vec<u8> = [99u32, 99, 1, 1, 1]
                .into_iter()
                .flat_map(|value| value.to_le_bytes().into_iter().take(width))
                .collect();
            resources.insert(buffer, degenerate, false);
            backend
                .execute_plan_with_resources(&plan, &mut resources)
                .unwrap();
            let cleared = backend.readback_rgba8(target).unwrap();
            assert!(
                cleared != expected,
                "refreshed indices must change the rendered image"
            );
            assert!(cleared.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
            resources.insert(buffer, original_bytes, false);
            backend
                .execute_plan_with_resources(&plan, &mut resources)
                .unwrap();
            assert!(backend.readback_rgba8(target).unwrap() == expected);
        }
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_present_blit_preserves_pixels_and_source_layout() {
        for target_size in [256, 128] {
            let mut module = GpuModule::triangle_vulkan_smoke_test(256, 256);
            let source = module.textures[0].id;
            let mut destination = module.textures[0].clone();
            destination.id = ResourceId(0xffff);
            destination.width = target_size;
            destination.height = target_size;
            let target = destination.id;
            module.textures.push(destination);
            let plan = crate::VulkanBackend::new().plan(&module).unwrap();
            let mut backend = AshVulkanDevice::create().unwrap();
            backend
                .execute_plan_with_resources(&plan, &mut InMemoryGpuResources::new())
                .unwrap();
            let expected = backend.readback_rgba8(source).unwrap();
            let source_layout = backend.images[&source].layout;
            let handle = backend.images[&target].image;
            let extent = vk::Extent2D {
                width: target_size,
                height: target_size,
            };
            // Repeated use also verifies restoration after the previous transfer.
            for _ in 0..2 {
                let command = backend.begin_commands().unwrap();
                backend
                    .transition_image(command, target, vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .unwrap();
                unsafe {
                    backend.record_present_blit(
                        command,
                        source,
                        handle,
                        vk::Format::R8G8B8A8_UNORM,
                        extent,
                    )
                }
                .unwrap();
                backend
                    .transition_image(command, target, vk::ImageLayout::GENERAL)
                    .unwrap();
                backend.submit_and_wait(command).unwrap();
                assert_eq!(backend.images[&source].layout, source_layout);
                let actual = backend.readback_rgba8(target).unwrap();
                let scale = 256 / target_size;
                for y in 0..target_size {
                    for x in 0..target_size {
                        // Nearest filtering samples the source at destination
                        // pixel centers, including the 2:1 downscaled case.
                        let sx = x * scale + scale / 2;
                        let sy = y * scale + scale / 2;
                        let src = ((sy * 256 + sx) * 4) as usize;
                        let dst = ((y * target_size + x) * 4) as usize;
                        assert_eq!(
                            &actual[dst..dst + 4],
                            &expected[src..src + 4],
                            "target {target_size}, pixel ({x}, {y})"
                        );
                    }
                }
                assert_eq!(backend.readback_rgba8(source).unwrap(), expected);
            }
        }
    }
}
