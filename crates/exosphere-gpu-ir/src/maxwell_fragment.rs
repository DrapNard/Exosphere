// SPDX-License-Identifier: GPL-3.0-or-later
//! Restricted Maxwell fragment export resolution.

use crate::maxwell_shader::MaxwellShaderHeader;
use crate::spirv::SpirvCompileError;
use crate::{ShaderModule, ShaderOperation};

/// Resolve a single complete color export from its physical R0..R3 registers.
/// Sparse/multiple targets and depth/sample-mask exports require other layouts.
pub fn lower_fragment_outputs(
    shader: &ShaderModule,
    header: &MaxwellShaderHeader,
) -> Result<ShaderModule, SpirvCompileError> {
    let outputs = header
        .fragment_outputs(shader.stage)
        .ok_or(SpirvCompileError::UnsupportedStage(shader.stage))?;
    if outputs.color_components != 15 || outputs.control != 0 {
        return Err(SpirvCompileError::UnsupportedOperation);
    }
    let mut lowered = shader.clone();
    lowered.operations.clear();
    let mut returned = false;
    for operation in &shader.operations {
        if returned && !matches!(operation, ShaderOperation::Nop) {
            return Err(SpirvCompileError::UnsupportedOperation);
        }
        match operation {
            ShaderOperation::Return => {
                // The hardware consumes the final color registers at termination,
                // not at each earlier MOV that happened to write one of them.
                for component in 0..4 {
                    lowered.operations.push(ShaderOperation::StoreOutput {
                        location: 0,
                        component,
                        src: u32::from(component),
                    });
                }
                lowered.operations.push(ShaderOperation::Return);
                returned = true;
            }
            ShaderOperation::Nop
            | ShaderOperation::ConstantBits32 { .. }
            | ShaderOperation::ConstantF32 { .. }
            | ShaderOperation::LoadUniformWord { .. }
            | ShaderOperation::SampleTexture2d { .. }
            | ShaderOperation::Move { .. } => lowered.operations.push(operation.clone()),
            ShaderOperation::ShiftWord32 { .. }
            | ShaderOperation::ExtractWord32 { .. }
            | ShaderOperation::CountBits32 { .. }
            | ShaderOperation::FindLeadingWord32 { .. } => {
                lowered.operations.push(operation.clone());
            }
            ShaderOperation::ConvertRich { control, .. } if control.is_plain_word_to_f32() => {
                lowered.operations.push(operation.clone());
            }
            ShaderOperation::IntegerBinary { .. }
                if crate::spirv::supports_integer_binary(operation) =>
            {
                lowered.operations.push(operation.clone());
            }
            ShaderOperation::Interpolate {
                attribute,
                component,
                mode: crate::shader::InterpolationMode::Pass,
                offset: None,
                ..
            } if (0x80..0x280).contains(attribute)
                && attribute & 3 == 0
                && u32::from(*component) == (attribute & 15) / 4 =>
            {
                lowered.operations.push(operation.clone());
            }
            _ => return Err(SpirvCompileError::UnsupportedOperation),
        }
    }
    if !returned {
        return Err(SpirvCompileError::UnsupportedOperation);
    }
    Ok(lowered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShaderStage;

    #[test]
    fn fragment_word_conversion_preserves_final_exports_and_rejects_unknown_controls() {
        use crate::shader::{ConversionControl, RoundingMode, ScalarType};
        let mut bytes = vec![0; crate::maxwell_shader::GRAPHICS_HEADER_SIZE];
        bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        bytes.extend(
            [0u64, 0xe30000000007000f]
                .into_iter()
                .flat_map(u64::to_le_bytes),
        );
        let header = crate::maxwell_shader::parse_graphics(ShaderStage::Fragment, &bytes)
            .unwrap()
            .header
            .unwrap();
        let mut control = ConversionControl {
            src: ScalarType::U32,
            dst: ScalarType::F32,
            rounding: RoundingMode::NearestEven,
            saturate: false,
            flush_to_zero: false,
            absolute: false,
            negate: false,
            raw_flags: 0,
        };
        let mut shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Fragment,
            variant_key: vec![],
            operations: vec![
                ShaderOperation::ConstantBits32 { dst: 7, value: 1 },
                ShaderOperation::ConvertRich {
                    dst: 0,
                    src: 7,
                    control,
                },
                ShaderOperation::ConstantF32 { dst: 1, value: 1.0 },
                ShaderOperation::ConstantF32 { dst: 2, value: 1.0 },
                ShaderOperation::ConstantF32 { dst: 3, value: 1.0 },
                ShaderOperation::Return,
            ],
        };
        let lowered = lower_fragment_outputs(&shader, &header).unwrap();
        assert!(crate::spirv::compile(&lowered).is_ok());
        assert_eq!(
            lowered
                .operations
                .iter()
                .filter(|op| matches!(op, ShaderOperation::StoreOutput { .. }))
                .count(),
            4
        );
        control.negate = true;
        shader.operations[1] = ShaderOperation::ConvertRich {
            dst: 0,
            src: 7,
            control,
        };
        assert!(lower_fragment_outputs(&shader, &header).is_err());
        control.negate = false;
        shader.operations[1] = ShaderOperation::ConvertRich {
            dst: 0,
            src: 8,
            control,
        };
        assert!(crate::spirv::compile(&lower_fragment_outputs(&shader, &header).unwrap()).is_err());
    }

    #[test]
    fn encoded_constants_export_final_color_registers() {
        let mut bytes = vec![0; crate::maxwell_shader::GRAPHICS_HEADER_SIZE];
        bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        let mov = |dst: u64, value: u32| 0x010000000007f000 | (u64::from(value) << 20) | dst;
        bytes.extend(
            [
                0,
                mov(0, 0x3e4ccccd),
                mov(1, 0x3f4ccccd),
                mov(2, 0x3f800000),
                0,
                0x5c98000000000000 | (15 << 39) | (2 << 20) | (7 << 16) | 3,
                0x3898000000000000 | (1 << 56) | (15 << 39) | (0x7ffff << 20) | (7 << 16) | 7,
                0xe30000000007000f,
            ]
            .into_iter()
            .flat_map(u64::to_le_bytes),
        );
        let program = crate::maxwell_shader::parse_graphics(ShaderStage::Fragment, &bytes).unwrap();
        let (shader, stats) = crate::maxwell_shader::lift_semantic(1, &program);
        assert_eq!(stats.opaque, 0);
        let header = program.header.unwrap();
        let lowered = lower_fragment_outputs(&shader, &header).unwrap();
        assert!(crate::spirv::compile(&lowered).is_ok());
        for mask in [0u32, 7, 0xf0, 0xff, 0xffffffff] {
            let mut invalid = header.clone();
            invalid.raw[72..76].copy_from_slice(&mask.to_le_bytes());
            assert!(lower_fragment_outputs(&shader, &invalid).is_err());
        }
        for control in [1u32, 2, 3, 0x80000000] {
            let mut invalid = header.clone();
            invalid.raw[76..80].copy_from_slice(&control.to_le_bytes());
            assert!(lower_fragment_outputs(&shader, &invalid).is_err());
        }
        let mut missing = shader.clone();
        missing.operations.remove(0);
        let missing = lower_fragment_outputs(&missing, &header).unwrap();
        assert!(crate::spirv::compile(&missing).is_err());
        let mut wrong_stage = shader;
        wrong_stage.stage = ShaderStage::Vertex;
        assert!(lower_fragment_outputs(&wrong_stage, &header).is_err());
    }
}
