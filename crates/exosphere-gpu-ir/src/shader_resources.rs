// SPDX-License-Identifier: GPL-3.0-or-later
//! Descriptor reflection for resolved canonical shader operations.

use std::collections::BTreeMap;

use crate::{
    DescriptorBindingDesc, DescriptorType, PipelineLayoutDesc, ShaderModule, ShaderOperation as S,
    ShaderStage, ShaderStageMask, spirv::SpirvCompileError as Error,
};

fn flatten(operations: &[S], out: &mut Vec<S>) {
    for operation in operations {
        match operation {
            S::Predicated { operations, .. } => flatten(operations, out),
            _ => out.push(operation.clone()),
        }
    }
}

/// Reflect uniforms and separate float sampled images/samplers. Descriptor
/// namespace collisions fail even when the SPIR-V compiler has not run yet.
/// Predicated accesses still require descriptors; this is not execution support
/// for the predicate itself. Unresolved handles cannot acquire guessed bindings.
pub fn descriptor_layout(shader: &ShaderModule) -> Result<PipelineLayoutDesc, Error> {
    let mut flat = shader.clone();
    flat.operations.clear();
    flatten(&shader.operations, &mut flat.operations);
    let uniforms = crate::maxwell_constant::uniform_descriptor_layout(&flat)?;
    let mut bindings: BTreeMap<_, _> = uniforms
        .descriptors
        .into_iter()
        .map(|binding| ([binding.set, binding.binding], binding))
        .collect();
    for operation in &flat.operations {
        match operation {
            S::SampleTexture2dHandle { .. } => return Err(Error::UnsupportedOperation),
            S::SampleTexture2d { image, sampler, .. } => {
                if shader.stage != ShaderStage::Fragment {
                    return Err(Error::UnsupportedOperation);
                }
                for (key, descriptor_type) in [
                    (*image, DescriptorType::SampledImage),
                    (*sampler, DescriptorType::Sampler),
                ] {
                    let declaration = DescriptorBindingDesc {
                        set: key[0],
                        binding: key[1],
                        descriptor_type,
                        count: 1,
                        stages: ShaderStageMask(ShaderStageMask::FRAGMENT),
                    };
                    if bindings
                        .insert(key, declaration)
                        .is_some_and(|previous| previous != declaration)
                    {
                        return Err(Error::UnsupportedOperation);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(PipelineLayoutDesc {
        descriptors: bindings.into_values().collect(),
        push_constants: Vec::new(),
    })
}

/// Merge stage visibility only when descriptor types/counts and uniform sizes
/// agree. A sampler in one stage cannot overwrite another stage's uniform.
pub fn pipeline_layout(shaders: &[&ShaderModule]) -> Result<PipelineLayoutDesc, Error> {
    let mut flat = Vec::new();
    let mut bindings: BTreeMap<[u32; 2], DescriptorBindingDesc> = BTreeMap::new();
    for shader in shaders {
        let mut uniform_shader = (*shader).clone();
        uniform_shader.operations.clear();
        flatten(&shader.operations, &mut uniform_shader.operations);
        flat.push(uniform_shader);
        for declaration in descriptor_layout(shader)?.descriptors {
            let key = [declaration.set, declaration.binding];
            if let Some(previous) = bindings.get_mut(&key) {
                if previous.descriptor_type != declaration.descriptor_type
                    || previous.count != declaration.count
                {
                    return Err(Error::UnsupportedOperation);
                }
                previous.stages.0 |= declaration.stages.0;
            } else {
                bindings.insert(key, declaration);
            }
        }
    }
    // Uniform byte extents are part of the ABI, even though descriptor layout
    // declarations do not carry their sizes.
    crate::maxwell_constant::uniform_pipeline_layout(&flat.iter().collect::<Vec<_>>())?;
    Ok(PipelineLayoutDesc {
        descriptors: bindings.into_values().collect(),
        push_constants: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shader(stage: ShaderStage, operations: Vec<S>) -> ShaderModule {
        ShaderModule {
            id: 1,
            stage,
            variant_key: Vec::new(),
            operations,
        }
    }
    fn sample(image: [u32; 2], sampler: [u32; 2]) -> S {
        S::SampleTexture2d {
            dst: vec![0],
            image,
            sampler,
            coords: [1, 2],
            components: vec![crate::shader::TextureSampleChannel::Component(0)],
            level_zero: false,
        }
    }
    fn uniform(byte_size: u32) -> S {
        S::LoadUniformWord {
            dst: 0,
            set: 0,
            binding: 0,
            byte_offset: 0,
            byte_size,
        }
    }

    #[test]
    fn reflects_nested_accesses_and_deduplicates_shared_texture_bindings() {
        let shader = shader(
            ShaderStage::Fragment,
            vec![
                uniform(16),
                S::Predicated {
                    predicate: 0,
                    negated: false,
                    operations: vec![sample([1, 4], [1, 5]), sample([1, 4], [1, 6])],
                },
            ],
        );
        let layout = descriptor_layout(&shader).unwrap();
        assert_eq!(
            layout
                .descriptors
                .iter()
                .map(|d| ([d.set, d.binding], d.descriptor_type))
                .collect::<Vec<_>>(),
            vec![
                ([0, 0], DescriptorType::UniformBuffer),
                ([1, 4], DescriptorType::SampledImage),
                ([1, 5], DescriptorType::Sampler),
                ([1, 6], DescriptorType::Sampler)
            ]
        );
        assert!(
            layout
                .descriptors
                .iter()
                .all(|d| d.count == 1 && d.stages.0 == ShaderStageMask::FRAGMENT)
        );
    }

    #[test]
    fn rejects_collisions_unresolved_handles_and_wrong_sampling_stage() {
        for operations in [
            vec![sample([1, 4], [1, 4])],
            vec![uniform(16), sample([0, 0], [1, 5])],
            vec![sample([1, 4], [1, 5]), sample([1, 5], [1, 6])],
            vec![S::SampleTexture2dHandle {
                dst: vec![0],
                handle_selector: 0,
                coords: [1, 2],
                components: vec![0],
                level_zero: false,
            }],
        ] {
            assert!(descriptor_layout(&shader(ShaderStage::Fragment, operations)).is_err());
        }
        assert!(
            descriptor_layout(&shader(ShaderStage::Vertex, vec![sample([1, 4], [1, 5])])).is_err()
        );
    }

    #[test]
    fn pipeline_visibility_merges_without_losing_uniform_abi_checks() {
        let vertex = shader(ShaderStage::Vertex, vec![uniform(16)]);
        let mut fragment = shader(
            ShaderStage::Fragment,
            vec![uniform(16), sample([1, 4], [1, 5])],
        );
        let layout = pipeline_layout(&[&vertex, &fragment]).unwrap();
        assert_eq!(
            layout.descriptors[0].stages.0,
            ShaderStageMask::VERTEX | ShaderStageMask::FRAGMENT
        );
        assert_eq!(layout.descriptors[1].stages.0, ShaderStageMask::FRAGMENT);
        fragment.operations[0] = uniform(32);
        assert!(pipeline_layout(&[&vertex, &fragment]).is_err());
        fragment.operations = vec![sample([0, 0], [1, 5])];
        assert!(pipeline_layout(&[&vertex, &fragment]).is_err());
    }
}
