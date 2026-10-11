// SPDX-License-Identifier: GPL-3.0-or-later
//! Verified direct Maxwell vertex attribute interface lowering.

use crate::spirv::SpirvCompileError;
use crate::{ShaderModule, ShaderOperation as S, ShaderStage, VertexFormat};
use std::collections::BTreeMap;

/// Exact word-to-F32 conversion using integer arithmetic only. Host casts would
/// bake in the host rounding mode instead of the captured Maxwell mode.
fn constant_i2f(bits: u32, control: crate::shader::ConversionControl) -> Option<u32> {
    use crate::shader::{RoundingMode as R, ScalarType as T};
    if !control.is_plain_word_to_f32() {
        return None;
    }
    let signed = control.src == T::I32;
    let negative = signed && (bits as i32) < 0;
    let magnitude = if negative {
        (-(i64::from(bits as i32))) as u32
    } else {
        bits
    };
    if magnitude == 0 {
        return Some(0);
    }
    let exponent = 31 - magnitude.leading_zeros();
    let significand = if exponent <= 23 {
        magnitude << (23 - exponent)
    } else {
        let shift = exponent - 23;
        let truncated = magnitude >> shift;
        let remainder = magnitude & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let increment = match control.rounding {
            R::NearestEven => remainder > halfway || (remainder == halfway && truncated & 1 != 0),
            R::TowardPositive => !negative && remainder != 0,
            R::TowardNegative => negative && remainder != 0,
            R::TowardZero => false,
            R::None => unreachable!(),
        };
        truncated + u32::from(increment)
    };
    // Addition also carries a rounded significand overflow into the exponent.
    Some((u32::from(negative) << 31) | (((exponent + 127) << 23) + significand - (1 << 23)))
}

/// Explicit pairing of compact hardware attribute slots and host shader locations.
/// Format is required: integer inputs cannot acquire a floating-point interface.
#[derive(Debug, Clone, Copy)]
pub struct VertexAttributeInterface {
    pub slot: u8,
    pub location: u32,
    pub format: VertexFormat,
}

/// Specialize the direct integer InstanceID read for one captured instance.
/// Native fetch rebasing does not preserve Vulkan's InstanceIndex, so use the
/// actual ordered guest counter. Shader content identities include this value.
/// VertexID, vector/indirect system reads and raw S2R numbers remain unsupported.
pub fn specialize_instance_attribute(
    shader: &ShaderModule,
    state: &crate::maxwell_state::DrawStateSnapshot,
) -> Result<ShaderModule, SpirvCompileError> {
    if shader.stage != ShaderStage::Vertex {
        return Err(SpirvCompileError::UnsupportedStage(shader.stage));
    }
    let mut specialized = shader.clone();
    for operation in &mut specialized.operations {
        if let S::LoadAttributeRich {
            dst,
            address,
            components,
            output,
        } = operation
            && address.byte_offset == 0x2f8
        {
            if *output
                || *components != 1
                || address.address_register.is_some()
                || address.vertex_register.is_some()
                || address.per_patch
            {
                return Err(SpirvCompileError::UnsupportedOperation);
            }
            let bits = state
                .draw_instance_id()
                .ok_or(SpirvCompileError::UnsupportedOperation)?;
            *operation = S::ConstantBits32 {
                dst: *dst,
                value: bits,
            };
        }
    }
    Ok(specialized)
}

/// Resolve direct VertexID reads from this draw's exact generation/base policy.
/// Indexed fetch bias and shader ID bias are independent hardware registers.
pub fn specialize_vertex_id_attribute(
    shader: &ShaderModule,
    event: &crate::maxwell_state::MaxwellEvent,
) -> Result<ShaderModule, SpirvCompileError> {
    use crate::maxwell_state::{MaxwellEvent, method};
    let unsupported = SpirvCompileError::UnsupportedOperation;
    if shader.stage != ShaderStage::Vertex {
        return Err(SpirvCompileError::UnsupportedStage(shader.stage));
    }
    let state = match event {
        MaxwellEvent::DrawVertexArray { state, .. }
        | MaxwellEvent::DrawIndexBuffer { state, .. } => state,
        _ => return Err(unsupported),
    };
    let captured = |register| {
        state
            .raw_registers
            .get(&register)
            .copied()
            .ok_or(unsupported.clone())
    };
    let mut specialized = shader.clone();
    for operation in &mut specialized.operations {
        if let S::LoadAttributeRich {
            dst,
            address,
            components,
            output,
        } = operation
            && address.byte_offset == 0x2fc
        {
            if *output
                || *components != 1
                || address.address_register.is_some()
                || address.vertex_register.is_some()
                || address.per_patch
                || captured(method::SET_VERTEX_ID_REPLACE)? != 0
            {
                return Err(unsupported);
            }
            let mode = captured(method::SET_VERTEX_ID_GEN_MODE)?;
            if mode & !0x1000 != 0 {
                return Err(unsupported);
            }
            let base = captured(method::SET_VERTEX_ID_BASE)?;
            let bias = match event {
                MaxwellEvent::DrawIndexBuffer { .. } => {
                    base.wrapping_sub(captured(method::SET_VERTEX_ELEMENT_BASE)?)
                }
                // Nonzero array ID bases need independently verified semantics.
                MaxwellEvent::DrawVertexArray { first, .. } if base == 0 => {
                    if mode == 0 {
                        first.wrapping_neg()
                    } else {
                        0
                    }
                }
                _ => return Err(unsupported),
            };
            *operation = S::LoadVertexIndex { dst: *dst, bias };
        }
    }
    Ok(specialized)
}

/// Build the direct attribute interface from actual ALD accesses and captured
/// fetch formats. Canonical locations deliberately equal hardware slots; both
/// shader lowering and vertex fetch must consume this same interface.
pub fn discover_direct_vertex_interface(
    shader: &ShaderModule,
    state: &crate::maxwell_state::DrawStateSnapshot,
) -> Result<Vec<VertexAttributeInterface>, SpirvCompileError> {
    let unsupported = SpirvCompileError::UnsupportedOperation;
    let mut slots = std::collections::BTreeSet::new();
    for operation in &shader.operations {
        if let S::LoadAttributeRich {
            address,
            components,
            output,
            ..
        } = operation
        {
            if *output
                || address.address_register.is_some()
                || address.vertex_register.is_some()
                || address.per_patch
                || !(1..=4).contains(components)
            {
                return Err(unsupported);
            }
            for component in 0..u16::from(*components) {
                let byte = address
                    .byte_offset
                    .checked_add(component * 4)
                    .ok_or(unsupported.clone())?;
                if !(0x80..0x280).contains(&byte) || byte & 3 != 0 {
                    return Err(unsupported);
                }
                slots.insert(((byte - 0x80) / 16) as u8);
            }
        }
    }
    let mut interface = Vec::new();
    for slot in slots {
        let format = state
            .vertex_attribute(usize::from(slot))
            .and_then(|attribute| attribute.canonical_format())
            .ok_or(unsupported.clone())?;
        interface.push(VertexAttributeInterface {
            slot,
            location: u32::from(slot),
            format,
        });
    }
    // Reuse exact lowering checks for unsupported formats and shader
    // operations; discovery must not make a nominally valid interface for them.
    lower_vertex_interface(shader, &interface)?;
    Ok(interface)
}

pub fn lower_vertex_interface(
    shader: &ShaderModule,
    interface: &[VertexAttributeInterface],
) -> Result<ShaderModule, SpirvCompileError> {
    let unsupported = SpirvCompileError::UnsupportedOperation;
    if shader.stage != ShaderStage::Vertex {
        return Err(SpirvCompileError::UnsupportedStage(shader.stage));
    }
    let mut inputs = BTreeMap::new();
    let mut locations = std::collections::BTreeSet::new();
    for input in interface {
        if input.slot >= 32
            || input.location >= 32
            || inputs.insert(input.slot, input).is_some()
            || !locations.insert(input.location)
        {
            return Err(unsupported);
        }
    }
    let mut operations = Vec::new();
    let mut constants = BTreeMap::new();
    for operation in &shader.operations {
        match operation {
            S::ConstantBits32 { dst, value } => {
                constants.insert(*dst, *value);
            }
            S::Move { dst, src } => {
                if let Some(value) = constants.get(src).copied() {
                    constants.insert(*dst, value);
                } else {
                    constants.remove(dst);
                }
            }
            S::ConvertRich { dst, src, control } => {
                if !control.is_plain_word_to_f32() {
                    return Err(unsupported);
                }
                let value = constants
                    .get(src)
                    .copied()
                    .and_then(|value| constant_i2f(value, *control));
                if let Some(value) = value {
                    constants.insert(*dst, value);
                    operations.push(S::ConstantBits32 { dst: *dst, value });
                } else {
                    constants.remove(dst);
                    operations.push(operation.clone());
                }
                continue;
            }
            S::Nop => {}
            // Unknown register effects invalidate knowledge conservatively;
            // an overwritten or conditionally written source cannot be folded.
            _ => constants.clear(),
        }
        match operation {
            S::LoadAttributeRich {
                dst,
                address,
                components,
                output: false,
            } if address.address_register.is_none()
                && address.vertex_register.is_none()
                && !address.per_patch
                && (1..=4).contains(components) =>
            {
                for component in 0..u16::from(*components) {
                    let byte = address
                        .byte_offset
                        .checked_add(component * 4)
                        .ok_or(unsupported.clone())?;
                    if !(0x80..0x280).contains(&byte) || byte & 3 != 0 {
                        return Err(unsupported);
                    }
                    let input = inputs
                        .get(&(((byte - 0x80) / 16) as u8))
                        .ok_or(unsupported.clone())?;
                    use VertexFormat::*;
                    let integer = input.format.integer_signedness();
                    if integer.is_none()
                        && !matches!(
                            input.format,
                            R8Unorm
                                | R8Snorm
                                | Rg8Unorm
                                | Rg8Snorm
                                | Rgba8Unorm
                                | Rgba8Snorm
                                | R16Float
                                | Rg16Float
                                | Rgba16Float
                                | R32Float
                                | Rg32Float
                                | Rgb32Float
                                | Rgba32Float
                        )
                    {
                        return Err(unsupported);
                    }
                    let dst = dst
                        .checked_add(u32::from(component))
                        .ok_or(unsupported.clone())?;
                    let component = ((byte & 15) / 4) as u8;
                    operations.push(if let Some(signed) = integer {
                        S::LoadInputInteger {
                            dst,
                            location: input.location,
                            component,
                            signed,
                        }
                    } else {
                        S::LoadInput {
                            dst,
                            location: input.location,
                            component,
                        }
                    });
                }
            }
            S::StoreAttributeRich {
                src,
                address,
                components,
            } if address.address_register.is_none()
                && address.vertex_register.is_none()
                && !address.per_patch
                && (1..=4).contains(components) =>
            {
                if address.byte_offset < 0x80
                    && u32::from(address.byte_offset) + u32::from(*components) * 4 > 0x80
                {
                    return Err(unsupported);
                }
                for component in 0..u16::from(*components) {
                    let byte = address
                        .byte_offset
                        .checked_add(component * 4)
                        .ok_or(unsupported.clone())?;
                    if !(0x70..0x280).contains(&byte) || byte & 3 != 0 {
                        return Err(unsupported);
                    }
                    let src = src
                        .checked_add(u32::from(component))
                        .ok_or(unsupported.clone())?;
                    operations.push(if byte < 0x80 {
                        S::StoreBuiltinComponent {
                            builtin: 0,
                            component: ((byte - 0x70) / 4) as u8,
                            src,
                        }
                    } else {
                        S::StoreVarying {
                            location: u32::from((byte - 0x80) / 16),
                            component: ((byte & 15) / 4) as u8,
                            src,
                        }
                    });
                }
            }
            S::LoadAttributeRich { .. } | S::StoreAttributeRich { .. } => return Err(unsupported),
            S::Nop
            | S::Return
            | S::ConstantF32 { .. }
            | S::ConstantBits32 { .. }
            | S::LoadUniformWord { .. }
            | S::ModifiedBinary { .. }
            | S::ExtractWord32 { .. }
            | S::CountBits32 { .. }
            | S::FindLeadingWord32 { .. }
            | S::LoadInput { .. }
            | S::LoadInputInteger { .. }
            | S::LoadInputVec4 { .. }
            | S::LoadVertexIndex { .. }
            | S::StoreVarying { .. }
            | S::Move { .. }
            | S::ComposeVec4 { .. }
            | S::StoreBuiltin { builtin: 0, .. }
            | S::StoreBuiltinComponent { builtin: 0, .. } => operations.push(operation.clone()),
            _ => return Err(unsupported),
        }
    }
    let mut lowered = shader.clone();
    lowered.operations = operations;
    Ok(lowered)
}

#[cfg(test)]
mod tests {
    #[test]
    fn constant_integer_conversion_preserves_rounding_boundaries() {
        use crate::shader::{ConversionControl, RoundingMode as R, ScalarType as T};
        let mut control = ConversionControl {
            src: T::U32,
            dst: T::F32,
            rounding: R::NearestEven,
            saturate: false,
            flush_to_zero: false,
            absolute: false,
            negate: false,
            raw_flags: 0,
        };
        for (input, nearest, zero, positive) in [
            (0, 0, 0, 0),
            (1, 0x3f800000, 0x3f800000, 0x3f800000),
            (16777217, 0x4b800000, 0x4b800000, 0x4b800001),
            (16777219, 0x4b800002, 0x4b800001, 0x4b800002),
            (u32::MAX, 0x4f800000, 0x4f7fffff, 0x4f800000),
        ] {
            for (mode, expected) in [
                (R::NearestEven, nearest),
                (R::TowardZero, zero),
                (R::TowardPositive, positive),
                (R::TowardNegative, zero),
            ] {
                control.rounding = mode;
                assert_eq!(super::constant_i2f(input, control), Some(expected));
            }
        }
        control.src = T::I32;
        for (mode, expected) in [
            (R::NearestEven, 0xcb800000),
            (R::TowardZero, 0xcb800000),
            (R::TowardPositive, 0xcb800000),
            (R::TowardNegative, 0xcb800001),
        ] {
            control.rounding = mode;
            assert_eq!(
                super::constant_i2f((-16777217i32) as u32, control),
                Some(expected)
            );
            assert_eq!(
                super::constant_i2f(i32::MIN as u32, control),
                Some(0xcf000000)
            );
        }
        control.absolute = true;
        assert_eq!(super::constant_i2f(1, control), None);
    }

    #[test]
    fn constant_conversion_is_ordered_and_emits_float_bits() {
        use super::*;
        use crate::shader::{ConversionControl, RoundingMode, ScalarType};
        let conversion = S::ConvertRich {
            dst: 9,
            src: 7,
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
        };
        let mut shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![
                S::ConstantBits32 {
                    dst: 7,
                    value: 16777217,
                },
                conversion.clone(),
            ],
        };
        let lowered = lower_vertex_interface(&shader, &[]).unwrap();
        assert!(matches!(
            lowered.operations[1],
            S::ConstantBits32 {
                dst: 9,
                value: 0x4b800000
            }
        ));
        shader
            .operations
            .insert(1, S::ConstantF32 { dst: 7, value: 1.0 });
        let lowered = lower_vertex_interface(&shader, &[]).unwrap();
        assert!(matches!(lowered.operations[2], S::ConvertRich { .. }));
        shader.operations = vec![conversion, S::ConstantBits32 { dst: 7, value: 1 }];
        let mut lowered = lower_vertex_interface(&shader, &[]).unwrap();
        lowered.operations.push(S::StoreBuiltinComponent {
            builtin: 0,
            component: 0,
            src: 9,
        });
        assert!(crate::spirv::compile(&lowered).is_err());
    }

    #[test]
    fn vertex_id_policy_distinguishes_fetch_base_and_array_first() {
        use super::*;
        use crate::maxwell_state::{DrawStateSnapshot, MaxwellEvent, method};
        let shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![S::LoadAttributeRich {
                dst: 9,
                components: 1,
                output: false,
                address: crate::shader::ShaderAttributeAddress {
                    byte_offset: 0x2fc,
                    address_register: None,
                    vertex_register: None,
                    per_patch: false,
                },
            }],
        };
        let mut state = DrawStateSnapshot::default();
        for (register, value) in [
            (method::SET_VERTEX_ID_REPLACE, 0),
            (method::SET_VERTEX_ID_GEN_MODE, 0),
            (method::SET_VERTEX_ID_BASE, 7),
            (method::SET_VERTEX_ELEMENT_BASE, u32::MAX),
        ] {
            state.raw_registers.insert(register, value);
        }
        let indexed = MaxwellEvent::DrawIndexBuffer {
            raw: 3,
            state: Box::new(state.clone()),
        };
        assert!(matches!(
            specialize_vertex_id_attribute(&shader, &indexed)
                .unwrap()
                .operations[0],
            S::LoadVertexIndex { dst: 9, bias: 8 }
        ));
        state.raw_registers.insert(method::SET_VERTEX_ID_BASE, 0);
        for (mode, bias) in [(0, 9u32.wrapping_neg()), (0x1000, 0)] {
            state
                .raw_registers
                .insert(method::SET_VERTEX_ID_GEN_MODE, mode);
            let event = MaxwellEvent::DrawVertexArray {
                first: 9,
                count: 3,
                state: Box::new(state.clone()),
            };
            assert!(
                matches!(specialize_vertex_id_attribute(&shader, &event).unwrap().operations[0],
                S::LoadVertexIndex { dst: 9, bias: actual } if actual == bias)
            );
        }
        state.raw_registers.insert(method::SET_VERTEX_ID_REPLACE, 1);
        let event = MaxwellEvent::DrawIndexBuffer {
            raw: 3,
            state: Box::new(state.clone()),
        };
        assert!(specialize_vertex_id_attribute(&shader, &event).is_err());
        state.raw_registers.remove(&method::SET_VERTEX_ID_REPLACE);
        let event = MaxwellEvent::DrawIndexBuffer {
            raw: 3,
            state: Box::new(state),
        };
        assert!(specialize_vertex_id_attribute(&shader, &event).is_err());
    }
    #[test]
    fn instance_attribute_specialization_preserves_integer_bits_and_rejects_missing_history() {
        use super::*;
        let mut shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![S::LoadAttributeRich {
                dst: 9,
                components: 1,
                output: false,
                address: crate::shader::ShaderAttributeAddress {
                    byte_offset: 0x2f8,
                    address_register: None,
                    vertex_register: None,
                    per_patch: false,
                },
            }],
        };
        let mut state = crate::maxwell_state::DrawStateSnapshot {
            primitive_raw: Some(4 | crate::maxwell_state::method::BEGIN_INSTANCE_CONTINUE),
            instance_id: Some(0x40000000),
            ..Default::default()
        };
        let lowered = specialize_instance_attribute(&shader, &state).unwrap();
        assert!(matches!(
            lowered.operations[0],
            S::ConstantBits32 {
                dst: 9,
                value: 0x40000000
            }
        ));
        state.instance_id = None;
        assert!(specialize_instance_attribute(&shader, &state).is_err());
        state.instance_id = Some(0);
        if let S::LoadAttributeRich { components, .. } = &mut shader.operations[0] {
            *components = 2;
        }
        assert!(specialize_instance_attribute(&shader, &state).is_err());
    }
    use super::*;
    use crate::maxwell_shader::{MaxwellBundle, MaxwellInstruction, MaxwellProgram, lift_semantic};

    #[test]
    fn direct_discovery_tracks_cross_slot_reads_and_requires_captured_formats() {
        let mut shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![
                S::LoadAttributeRich {
                    dst: 0,
                    components: 2,
                    output: false,
                    address: crate::shader::ShaderAttributeAddress {
                        byte_offset: 0x8c,
                        address_register: None,
                        vertex_register: None,
                        per_patch: false,
                    },
                },
                S::Return,
            ],
        };
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert!(discover_direct_vertex_interface(&shader, &state).is_err());
        for slot in [0, 1] {
            state.raw_registers.insert(
                crate::maxwell_state::method::VERTEX_ATTRIBUTE_BASE + slot,
                (1 << 21) | (7 << 27),
            );
        }
        let interface = discover_direct_vertex_interface(&shader, &state).unwrap();
        assert_eq!(
            interface
                .iter()
                .map(|input| (input.slot, input.location))
                .collect::<Vec<_>>(),
            vec![(0, 0), (1, 1)]
        );
        assert!(
            interface
                .iter()
                .all(|input| input.format == VertexFormat::Rgba32Float)
        );
        state.raw_registers.insert(
            crate::maxwell_state::method::VERTEX_ATTRIBUTE_BASE,
            (1 << 21) | (4 << 27),
        );
        let integer = discover_direct_vertex_interface(&shader, &state).unwrap();
        assert_eq!(integer[0].format, VertexFormat::Rgba32Uint);
        state.raw_registers.insert(
            crate::maxwell_state::method::VERTEX_ATTRIBUTE_BASE,
            (1 << 21) | (7 << 27),
        );
        let S::LoadAttributeRich { address, .. } = &mut shader.operations[0] else {
            unreachable!()
        };
        address.address_register = Some(0);
        assert!(discover_direct_vertex_interface(&shader, &state).is_err());
    }

    #[test]
    fn actual_attribute_encodings_lower_to_executable_vertex_interface() {
        let operands = (3 << 47) | (255 << 39) | (7 << 16) | (255 << 8) | 7;
        let program = MaxwellProgram {
            stage: ShaderStage::Vertex,
            header: None,
            bundles: vec![MaxwellBundle {
                scheduler_offset: 0,
                scheduler: 0,
                instructions: [
                    0xefd8000000000000 | operands | (0x80 << 20),
                    0xeff0000000000000 | operands | (0x70 << 20),
                    0xe30000000007000f,
                ]
                .into_iter()
                .enumerate()
                .map(|(index, raw)| MaxwellInstruction {
                    offset: 8 + index as u32 * 8,
                    raw,
                })
                .collect(),
            }],
        };
        let (shader, stats) = lift_semantic(103, &program);
        assert_eq!(stats.semantic, 3);
        let mut interface = [VertexAttributeInterface {
            slot: 0,
            location: 5,
            format: VertexFormat::Rgba32Float,
        }];
        let lowered = lower_vertex_interface(&shader, &interface).unwrap();
        assert!(matches!(
            lowered.operations[0],
            S::LoadInput {
                dst: 7,
                location: 5,
                component: 0
            }
        ));
        assert!(crate::spirv::compile(&lowered).is_ok());
        let mut partial = shader.clone();
        let S::StoreAttributeRich { src, address, .. } = shader.operations[1] else {
            unreachable!()
        };
        partial.operations.splice(
            1..2,
            [
                S::StoreAttributeRich {
                    src,
                    address,
                    components: 2,
                },
                S::StoreAttributeRich {
                    src: src + 2,
                    address: crate::shader::ShaderAttributeAddress {
                        byte_offset: 0x78,
                        ..address
                    },
                    components: 2,
                },
            ],
        );
        let split = lower_vertex_interface(&partial, &interface).unwrap();
        assert!(crate::spirv::compile(&split).is_ok());
        let mut missing = split.clone();
        missing.operations.remove(6); // Position Z was never initialized.
        assert_eq!(
            crate::spirv::compile(&missing),
            Err(SpirvCompileError::UnsupportedOperation)
        );
        if let S::StoreAttributeRich { address, .. } = &mut partial.operations[2] {
            address.byte_offset = 0x7c; // Two components would cross Position's boundary.
        }
        assert!(lower_vertex_interface(&partial, &interface).is_err());
        assert!(lower_vertex_interface(&shader, &[]).is_err());
        interface[0].format = VertexFormat::Rgba32Uint;
        let integer = lower_vertex_interface(&shader, &interface).unwrap();
        assert!(matches!(
            integer.operations[0],
            S::LoadInputInteger { signed: false, .. }
        ));
        assert!(crate::spirv::compile(&integer).is_ok());
        interface[0].format = VertexFormat::Rgba32Float;
        assert!(lower_vertex_interface(&shader, &[interface[0], interface[0]]).is_err());
        for mode in 0..4 {
            let mut invalid = shader.clone();
            if let S::LoadAttributeRich {
                address, output, ..
            } = &mut invalid.operations[0]
            {
                match mode {
                    0 => address.address_register = Some(3),
                    1 => address.vertex_register = Some(17),
                    2 => address.per_patch = true,
                    _ => *output = true,
                }
            }
            assert!(lower_vertex_interface(&invalid, &interface).is_err());
        }
    }
}
