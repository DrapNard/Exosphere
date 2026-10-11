// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit Maxwell constant-buffer to host uniform interface mapping.

use crate::spirv::SpirvCompileError;
use crate::{ShaderModule, ShaderOperation};
use std::collections::{BTreeMap, BTreeSet};

/// Derive descriptor declarations from explicit canonical uniform accesses.
/// This does not infer Maxwell slot mappings or resolve memory payloads.
pub fn uniform_descriptor_layout(
    shader: &ShaderModule,
) -> Result<crate::resource::PipelineLayoutDesc, SpirvCompileError> {
    use crate::resource::{
        DescriptorBindingDesc, DescriptorType, PipelineLayoutDesc, ShaderStageMask,
    };
    let stage = match shader.stage {
        crate::ShaderStage::Vertex => ShaderStageMask::VERTEX,
        crate::ShaderStage::TessControl => ShaderStageMask::TESS_CONTROL,
        crate::ShaderStage::TessEvaluation => ShaderStageMask::TESS_EVAL,
        crate::ShaderStage::Geometry => ShaderStageMask::GEOMETRY,
        crate::ShaderStage::Fragment => ShaderStageMask::FRAGMENT,
        crate::ShaderStage::Compute => ShaderStageMask::COMPUTE,
    };
    let mut bindings = BTreeMap::new();
    for operation in &shader.operations {
        if matches!(operation, ShaderOperation::LoadConstantBuffer { .. }) {
            return Err(SpirvCompileError::UnsupportedOperation);
        }
        if let ShaderOperation::LoadUniformWord {
            set,
            binding,
            byte_offset,
            byte_size,
            ..
        } = operation
        {
            if *byte_size == 0
                || *byte_size > 0x1ffff
                || byte_size & 15 != 0
                || byte_offset & 3 != 0
                || byte_offset
                    .checked_add(4)
                    .is_none_or(|end| end > *byte_size)
                || bindings
                    .insert((*set, *binding), *byte_size)
                    .is_some_and(|previous| previous != *byte_size)
            {
                return Err(SpirvCompileError::UnsupportedOperation);
            }
        }
    }
    Ok(PipelineLayoutDesc {
        descriptors: bindings
            .keys()
            .map(|(set, binding)| DescriptorBindingDesc {
                set: *set,
                binding: *binding,
                descriptor_type: DescriptorType::UniformBuffer,
                count: 1,
                stages: ShaderStageMask(stage),
            })
            .collect(),
        push_constants: Vec::new(),
    })
}

#[derive(Debug, Clone, Copy)]
pub struct ConstantBufferInterface {
    pub slot: u8,
    pub set: u32,
    pub binding: u32,
    pub byte_size: u32,
}

/// Derive the supported direct constant interface from real shader accesses and
/// captured bindings. Host bindings equal hardware slots within the caller's
/// explicit descriptor set; declared sizes come from captured selectors, not
/// access offsets or presumed allocation sizes. Live memory remains unresolved.
pub fn discover_direct_constant_interface(
    shader: &ShaderModule,
    state: &crate::maxwell_state::DrawStateSnapshot,
    group: u8,
    set: u32,
) -> Result<Vec<ConstantBufferInterface>, SpirvCompileError> {
    let unsupported = SpirvCompileError::UnsupportedOperation;
    let mut slots = BTreeSet::new();
    for operation in &shader.operations {
        if let ShaderOperation::LoadConstantBuffer { slot, address, .. } = operation {
            if address.is_some() {
                return Err(unsupported);
            }
            slots.insert(*slot);
        }
    }
    let mut interface = Vec::new();
    for slot in slots {
        let captured = state
            .constant_buffers
            .get(&(group, slot))
            .ok_or(unsupported.clone())?;
        let (_, size) = captured.byte_range().ok_or(unsupported.clone())?;
        let byte_size = u32::try_from(size).map_err(|_| unsupported.clone())?;
        interface.push(ConstantBufferInterface {
            slot,
            set,
            binding: u32::from(slot),
            byte_size,
        });
    }
    lower_constant_interface(shader, &interface)?;
    Ok(interface)
}

/// Merge uniform declarations across stages without letting the last shader
/// silently replace an incompatible buffer layout from an earlier stage.
pub fn uniform_pipeline_layout(
    shaders: &[&ShaderModule],
) -> Result<crate::resource::PipelineLayoutDesc, SpirvCompileError> {
    let mut descriptors = BTreeMap::new();
    let mut sizes = BTreeMap::new();
    for shader in shaders {
        let layout = uniform_descriptor_layout(shader)?;
        for operation in &shader.operations {
            if let ShaderOperation::LoadUniformWord {
                set,
                binding,
                byte_size,
                ..
            } = operation
            {
                if sizes
                    .insert((*set, *binding), *byte_size)
                    .is_some_and(|previous| previous != *byte_size)
                {
                    return Err(SpirvCompileError::UnsupportedOperation);
                }
            }
        }
        for descriptor in layout.descriptors {
            let previous = descriptors
                .entry((descriptor.set, descriptor.binding))
                .or_insert(descriptor);
            previous.stages.0 |= descriptor.stages.0;
        }
    }
    Ok(crate::resource::PipelineLayoutDesc {
        descriptors: descriptors.into_values().collect(),
        push_constants: Vec::new(),
    })
}

/// Prepare the shader's actual uniform bindings from one captured graphics group.
/// Reject undersized buffers before submission; no implicit padding or slot mapping.
pub fn bind_uniform_descriptors(
    shader: &ShaderModule,
    state: &crate::maxwell_state::DrawStateSnapshot,
    group: u8,
    interface: &[ConstantBufferInterface],
    address_space: &crate::GpuAddressSpace,
) -> Result<Vec<crate::Command>, crate::maxwell_state::ConstantBufferReadError> {
    use crate::maxwell_state::ConstantBufferReadError as Error;
    let layout = uniform_descriptor_layout(shader).map_err(|_| Error::InvalidDescriptor)?;
    let mut mappings = BTreeMap::new();
    let mut slots = BTreeSet::new();
    for entry in interface {
        if entry.slot >= 32
            || !slots.insert(entry.slot)
            || mappings.insert((entry.set, entry.binding), entry).is_some()
        {
            return Err(Error::InvalidDescriptor);
        }
    }
    let mut commands = Vec::new();
    for descriptor in layout.descriptors {
        let entry = mappings
            .get(&(descriptor.set, descriptor.binding))
            .ok_or(Error::InvalidDescriptor)?;
        let (_, size) = state
            .constant_buffers
            .get(&(group, entry.slot))
            .and_then(|binding| binding.byte_range())
            .ok_or(Error::InvalidBinding)?;
        for operation in &shader.operations {
            if let ShaderOperation::LoadUniformWord {
                set,
                binding,
                byte_size,
                ..
            } = operation
            {
                if (*set, *binding) == (descriptor.set, descriptor.binding)
                    && (*byte_size != entry.byte_size || u64::from(*byte_size) > size)
                {
                    return Err(Error::OutOfBounds);
                }
            }
        }
        commands.push(state.prepare_constant_buffer_descriptor(
            group,
            entry.slot,
            descriptor,
            0,
            address_space,
        )?);
    }
    Ok(commands)
}

/// Preserve runtime loads; never specialize the shader using guest payload bytes.
/// Register-relative addressing requires further lowering and is rejected here.
pub fn lower_constant_interface(
    shader: &ShaderModule,
    interface: &[ConstantBufferInterface],
) -> Result<ShaderModule, SpirvCompileError> {
    let mut slots = BTreeMap::new();
    let mut descriptors = BTreeSet::new();
    for entry in interface {
        if entry.slot >= 32
            || entry.byte_size == 0
            || entry.byte_size > 0x1ffff
            || slots.insert(entry.slot, entry).is_some()
            || !descriptors.insert((entry.set, entry.binding))
        {
            return Err(SpirvCompileError::UnsupportedOperation);
        }
    }
    let mut lowered = shader.clone();
    for operation in &mut lowered.operations {
        if let ShaderOperation::LoadConstantBuffer {
            dst,
            slot,
            offset,
            address,
        } = operation
        {
            let entry = slots
                .get(slot)
                .ok_or(SpirvCompileError::UnsupportedOperation)?;
            let offset =
                u32::try_from(*offset).map_err(|_| SpirvCompileError::UnsupportedOperation)?;
            if address.is_some()
                || offset & 3 != 0
                || offset
                    .checked_add(4)
                    .is_none_or(|end| end > entry.byte_size)
            {
                return Err(SpirvCompileError::UnsupportedOperation);
            }
            *operation = ShaderOperation::LoadUniformWord {
                dst: *dst,
                set: entry.set,
                binding: entry.binding,
                byte_offset: offset,
                byte_size: entry.byte_size,
            };
        }
    }
    Ok(lowered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_discovery_uses_captured_sizes_and_rejects_invalid_accesses() {
        let mut shader = ShaderModule {
            id: 1,
            stage: crate::ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![ShaderOperation::LoadConstantBuffer {
                dst: 0,
                slot: 3,
                offset: 28,
                address: None,
            }],
        };
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert!(discover_direct_constant_interface(&shader, &state, 0, 2).is_err());
        let binding = crate::maxwell_state::ConstantBufferBinding {
            size_raw: 32,
            address: crate::maxwell_state::RawGpuAddress { a: 0, b: 0x1000 },
            bind_raw: 1 | (3 << 4),
        };
        state.constant_buffers.insert((0, 3), binding);
        shader.operations.push(shader.operations[0].clone());
        let interface = discover_direct_constant_interface(&shader, &state, 0, 2).unwrap();
        assert_eq!(interface.len(), 1);
        assert_eq!(
            (
                interface[0].slot,
                interface[0].set,
                interface[0].binding,
                interface[0].byte_size
            ),
            (3, 2, 3, 32)
        );
        assert!(discover_direct_constant_interface(&shader, &state, 1, 2).is_err());
        for (offset, address) in [(32, None), (-4, None), (1, None), (0, Some(1))] {
            shader.operations[0] = ShaderOperation::LoadConstantBuffer {
                dst: 0,
                slot: 3,
                offset,
                address,
            };
            assert!(discover_direct_constant_interface(&shader, &state, 0, 2).is_err());
        }
        shader.operations[0] = shader.operations[1].clone();
        state.constant_buffers.insert(
            (0, 3),
            crate::maxwell_state::ConstantBufferBinding {
                bind_raw: 3 << 4,
                ..binding
            },
        );
        assert!(discover_direct_constant_interface(&shader, &state, 0, 2).is_err());
    }

    #[test]
    fn shared_uniform_pipeline_bindings_merge_visibility_not_conflicting_sizes() {
        use crate::resource::ShaderStageMask;
        let vertex = ShaderModule {
            id: 1,
            stage: crate::ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![ShaderOperation::LoadUniformWord {
                dst: 0,
                set: 2,
                binding: 7,
                byte_offset: 0,
                byte_size: 32,
            }],
        };
        let mut fragment = vertex.clone();
        fragment.id = 2;
        fragment.stage = crate::ShaderStage::Fragment;
        let layout = uniform_pipeline_layout(&[&vertex, &fragment]).unwrap();
        assert_eq!(layout.descriptors.len(), 1);
        assert_eq!(
            layout.descriptors[0].stages.0,
            ShaderStageMask::VERTEX | ShaderStageMask::FRAGMENT
        );
        assert_eq!(
            uniform_pipeline_layout(&[&fragment, &vertex]).unwrap(),
            layout
        );
        fragment.operations[0] = ShaderOperation::LoadUniformWord {
            dst: 0,
            set: 2,
            binding: 7,
            byte_offset: 0,
            byte_size: 16,
        };
        assert!(uniform_pipeline_layout(&[&vertex, &fragment]).is_err());
    }

    #[test]
    fn explicit_uniform_mapping_preserves_loads_and_rejects_unsupported_addressing() {
        let shader = ShaderModule {
            id: 9,
            stage: crate::ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![
                ShaderOperation::LoadConstantBuffer {
                    dst: 4,
                    slot: 3,
                    offset: 28,
                    address: None,
                },
                ShaderOperation::Return,
            ],
        };
        let entry = ConstantBufferInterface {
            slot: 3,
            set: 2,
            binding: 7,
            byte_size: 32,
        };
        let lowered = lower_constant_interface(&shader, &[entry]).unwrap();
        let layout = uniform_descriptor_layout(&lowered).unwrap();
        assert_eq!(layout.descriptors.len(), 1);
        assert_eq!(
            (layout.descriptors[0].set, layout.descriptors[0].binding),
            (2, 7)
        );
        assert_eq!(
            layout.descriptors[0].stages.0,
            crate::resource::ShaderStageMask::VERTEX
        );
        let mut duplicated = lowered.clone();
        duplicated.operations.push(lowered.operations[0].clone());
        assert_eq!(uniform_descriptor_layout(&duplicated).unwrap(), layout);
        duplicated
            .operations
            .push(ShaderOperation::LoadUniformWord {
                dst: 5,
                set: 2,
                binding: 7,
                byte_offset: 0,
                byte_size: 64,
            });
        assert!(uniform_descriptor_layout(&duplicated).is_err());
        assert!(matches!(
            lowered.operations[0],
            ShaderOperation::LoadUniformWord {
                dst: 4,
                set: 2,
                binding: 7,
                byte_offset: 28,
                byte_size: 32,
            }
        ));
        assert!(matches!(
            shader.operations[0],
            ShaderOperation::LoadConstantBuffer { .. }
        ));
        for (offset, address) in [(-4, None), (1, None), (32, None), (0, Some(1))] {
            let mut invalid = shader.clone();
            invalid.operations[0] = ShaderOperation::LoadConstantBuffer {
                dst: 4,
                slot: 3,
                offset,
                address,
            };
            assert!(lower_constant_interface(&invalid, &[entry]).is_err());
        }
        assert!(lower_constant_interface(&shader, &[]).is_err());
        assert!(lower_constant_interface(&shader, &[entry, entry]).is_err());
    }
}
