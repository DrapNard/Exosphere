// SPDX-License-Identifier: GPL-3.0-or-later
//! Straight-line typed vertex inputs and explicit position/color outputs.

use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
enum Value {
    Scalar(u32),
    Bits(u32),
    Vector(u32),
}

fn scalar_word(value: Value, b: &mut Builder, uint: u32) -> Result<u32, SpirvCompileError> {
    match value {
        Value::Bits(value) => Ok(value),
        Value::Scalar(value) => {
            let bits = b.id();
            b.inst(124, &[uint, bits, value]); // OpBitcast preserves the register word.
            Ok(bits)
        }
        Value::Vector(_) => Err(SpirvCompileError::UnsupportedOperation),
    }
}

pub(super) fn integer_opcode(
    op: crate::shader::BinaryOp,
    modifiers: [crate::shader::ValueModifier; 2],
    control: crate::shader::IntegerControl,
) -> Result<u16, SpirvCompileError> {
    let logical = matches!(
        op,
        crate::shader::BinaryOp::And | crate::shader::BinaryOp::Or | crate::shader::BinaryOp::Xor
    );
    if control.saturate
        || control.carry_in
        || control.carry_out
        || control.high
        || modifiers
            .iter()
            .any(|m| m.absolute || (m.invert && !logical) || (m.negate && logical))
    {
        return Err(SpirvCompileError::UnsupportedOperation);
    }
    // Low-word add/multiply wrap modulo 2^32 for both signedness modes.
    // No wrap decorations are emitted: overflow is defined, not undefined.
    match op {
        crate::shader::BinaryOp::Add => Ok(128), // OpIAdd
        crate::shader::BinaryOp::Mul => Ok(132), // OpIMul
        crate::shader::BinaryOp::And => Ok(199),
        crate::shader::BinaryOp::Or => Ok(197),
        crate::shader::BinaryOp::Xor => Ok(198),
        _ => Err(SpirvCompileError::UnsupportedOperation),
    }
}

fn scalar_float(value: Value, b: &mut Builder, float: u32) -> Result<u32, SpirvCompileError> {
    match value {
        Value::Scalar(value) => Ok(value),
        Value::Bits(value) => {
            let converted = b.id();
            b.inst(124, &[float, converted, value]); // OpBitcast, never integer-to-float conversion.
            Ok(converted)
        }
        Value::Vector(_) => Err(SpirvCompileError::UnsupportedOperation),
    }
}

fn modified_scalar_float(
    value: Value,
    modifier: crate::shader::ValueModifier,
    b: &mut Builder,
    float: u32,
    uint: u32,
    masks: [u32; 2],
) -> Result<u32, SpirvCompileError> {
    if modifier.invert {
        return Err(SpirvCompileError::UnsupportedOperation);
    }
    if !modifier.absolute && !modifier.negate {
        return scalar_float(value, b, float);
    }
    // ABS precedes NEG. Bit operations preserve NaN payloads and signed zero;
    // no arithmetic conversion or host floating-point evaluation is involved.
    let mut bits = match value {
        Value::Bits(bits) => bits,
        Value::Scalar(value) => {
            let bits = b.id();
            b.inst(124, &[uint, bits, value]);
            bits
        }
        Value::Vector(_) => return Err(SpirvCompileError::UnsupportedOperation),
    };
    for (enabled, opcode, mask) in [
        (modifier.absolute, 199, masks[0]),
        (modifier.negate, 198, masks[1]),
    ] {
        if enabled {
            let modified = b.id();
            b.inst(opcode, &[uint, modified, bits, mask]);
            bits = modified;
        }
    }
    scalar_float(Value::Bits(bits), b, float)
}

pub(super) fn compile(shader: &ShaderModule) -> Result<SpirvModule, SpirvCompileError> {
    let fragment = shader.stage == ShaderStage::Fragment;
    if !matches!(shader.stage, ShaderStage::Vertex | ShaderStage::Fragment) {
        return Err(SpirvCompileError::UnsupportedStage(shader.stage));
    }
    for operation in &shader.operations {
        if (fragment
            && matches!(
                operation,
                ShaderOperation::StoreBuiltin { .. }
                    | ShaderOperation::StoreBuiltinComponent { .. }
                    | ShaderOperation::LoadInput { .. }
                    | ShaderOperation::LoadInputInteger { .. }
                    | ShaderOperation::LoadInputVec4 { .. }
                    | ShaderOperation::LoadVertexIndex { .. }
                    | ShaderOperation::StoreVarying { .. }
            ))
            || (!fragment && matches!(operation, ShaderOperation::StoreOutput { .. }))
            || (!fragment && matches!(operation, ShaderOperation::Interpolate { .. }))
        {
            return Err(SpirvCompileError::UnsupportedOperation);
        }
    }
    let mut b = Builder::new();
    let void = b.id();
    let float = b.id();
    let vector = b.id();
    let input_pointer = b.id();
    let output_pointer = b.id();
    let output_scalar_pointer = b.id();
    let uint = b.id();
    let indices = [b.id(), b.id(), b.id(), b.id()];
    let shift_types = shader
        .operations
        .iter()
        .any(|op| {
            matches!(
                op,
                ShaderOperation::ShiftWord32 { .. }
                    | ShaderOperation::ExtractWord32 { .. }
                    | ShaderOperation::FindLeadingWord32 { .. }
            )
        })
        .then(|| [b.id(), b.id(), b.id()]);
    let word_find = if shader
        .operations
        .iter()
        .any(|op| matches!(op, ShaderOperation::FindLeadingWord32 { .. }))
    {
        Some(super::word_find::WordFind::new(
            &mut b,
            uint,
            shift_types.ok_or(SpirvCompileError::UnsupportedOperation)?,
            [indices[0], indices[1], indices[2]],
        ))
    } else {
        None
    };
    let extract_constants = shader
        .operations
        .iter()
        .any(|op| matches!(op, ShaderOperation::ExtractWord32 { .. }))
        .then(|| [b.id(), b.id()]);
    let modifier_masks = [b.id(), b.id()];
    let function_type = b.id();
    let position = b.id();
    let main = b.id();
    let label = b.id();
    let vertex_index = shader
        .operations
        .iter()
        .any(|operation| matches!(operation, ShaderOperation::LoadVertexIndex { .. }))
        .then(|| [b.id(), b.id(), b.id()]); // Signed integer type, input pointer, variable.
    let mut inputs = BTreeMap::new();
    // std140 uses 16-byte array strides. A uvec4 array preserves tightly packed
    // register words without requiring optional scalar-block-layout features.
    let uniform_vector = b.id();
    let uniform_word_pointer = b.id();
    let mut uniforms = BTreeMap::new();
    let mut uniform_offsets = BTreeMap::new();
    for (ordinal, operation) in shader.operations.iter().enumerate() {
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
            {
                return Err(SpirvCompileError::UnsupportedOperation);
            }
            let entry = uniforms
                .entry((*set, *binding))
                .or_insert_with(|| (*byte_size, [b.id(), b.id(), b.id(), b.id(), b.id()]));
            if entry.0 != *byte_size {
                return Err(SpirvCompileError::UnsupportedOperation);
            }
            uniform_offsets.insert(ordinal, (b.id(), b.id()));
        }
    }
    let mut arithmetic = BTreeMap::new();
    // Separate image/sampler descriptors cannot alias each other or a uniform
    // binding. A shared descriptor of the same type gets one SPIR-V variable.
    let mut images = BTreeMap::new();
    let mut samplers = BTreeMap::new();
    for operation in &shader.operations {
        if let ShaderOperation::SampleTexture2d {
            dst,
            image,
            sampler,
            components,
            ..
        } = operation
        {
            if !fragment
                || dst.is_empty()
                || dst.len() > 4
                || dst.len() != components.len()
                || components.iter().any(|component| {
                    matches!(
                        component,
                        crate::shader::TextureSampleChannel::Component(4..)
                    )
                })
                || dst.iter().collect::<std::collections::BTreeSet<_>>().len() != dst.len()
            {
                return Err(SpirvCompileError::UnsupportedOperation);
            }
            images.entry(*image).or_insert_with(|| b.id());
            samplers.entry(*sampler).or_insert_with(|| b.id());
        }
    }
    if images.keys().any(|key| samplers.contains_key(key))
        || images
            .keys()
            .chain(samplers.keys())
            .any(|key| uniforms.contains_key(&(key[0], key[1])))
    {
        return Err(SpirvCompileError::UnsupportedOperation);
    }
    // Image, sampler, combined sampled-image, their pointers, vec2, zero/one.
    let texture_types = (!images.is_empty()).then(|| {
        [
            b.id(),
            b.id(),
            b.id(),
            b.id(),
            b.id(),
            b.id(),
            b.id(),
            b.id(),
        ]
    });
    let mut denorm_mode = None;
    for (ordinal, operation) in shader.operations.iter().enumerate() {
        if let ShaderOperation::ModifiedBinary {
            op,
            left_modifier,
            right_modifier,
            float,
            ..
        } = operation
        {
            if !matches!(
                op,
                crate::shader::BinaryOp::Add | crate::shader::BinaryOp::Mul
            ) || float.saturate
                || [left_modifier, right_modifier]
                    .iter()
                    .any(|modifier| modifier.invert)
                || denorm_mode.is_some_and(|mode| mode != float.flush_to_zero)
            {
                return Err(SpirvCompileError::UnsupportedOperation);
            }
            denorm_mode = Some(float.flush_to_zero);
            arithmetic.insert(ordinal, b.id());
        }
    }
    // None is floating-point; Some(signed) is an integer interface. One
    // location cannot be consumed with conflicting scalar types.
    let mut input_types = BTreeMap::new();
    for operation in &shader.operations {
        let input = match operation {
            ShaderOperation::LoadInputVec4 { location, .. }
            | ShaderOperation::LoadInput { location, .. } => Some((*location, None)),
            ShaderOperation::LoadInputInteger {
                location, signed, ..
            } => Some((*location, Some(*signed))),
            _ => None,
        };
        if let Some((location, ty)) = input {
            // Software interface bound, not a claimed guest GPU capability.
            if location >= 32
                || input_types
                    .insert(location, ty)
                    .is_some_and(|previous| previous != ty)
            {
                return Err(SpirvCompileError::UnsupportedOperation);
            }
            inputs.entry(location).or_insert_with(|| b.id());
        }
    }
    let integer_inputs = input_types.values().any(Option::is_some).then(|| {
        [
            vertex_index.map_or_else(|| b.id(), |types| types[0]),
            b.id(),
            b.id(),
            b.id(),
        ]
    }); // Signed scalar/vector/pointer, unsigned vector pointer.
    let mut varying_inputs = BTreeMap::new();
    let mut varying_outputs = BTreeMap::new();
    for operation in &shader.operations {
        match operation {
            ShaderOperation::StoreVarying {
                location,
                component,
                ..
            } => {
                if *location >= 32 || *component >= 4 {
                    return Err(SpirvCompileError::UnsupportedOperation);
                }
                varying_outputs
                    .entry((*location, *component))
                    .or_insert_with(|| b.id());
            }
            ShaderOperation::Interpolate {
                attribute,
                component,
                mode,
                offset,
                ..
            } => {
                if *mode != crate::shader::InterpolationMode::Pass
                    || offset.is_some()
                    || !(0x80..0x280).contains(attribute)
                    || attribute & 3 != 0
                    || u32::from(*component) != (attribute & 15) / 4
                {
                    return Err(SpirvCompileError::UnsupportedOperation);
                }
                varying_inputs
                    .entry(((attribute - 0x80) / 16, *component))
                    .or_insert_with(|| b.id());
            }
            _ => {}
        }
    }
    let varying_pointer = (!varying_inputs.is_empty()).then(|| b.id());
    b.inst(OP_CAPABILITY, &[1]);
    if let Some(flush) = denorm_mode {
        for capability in [4466, 4467, if flush { 4465 } else { 4464 }] {
            b.inst(OP_CAPABILITY, &[capability]);
        }
        b.inst_string(10, &[], "SPV_KHR_float_controls", &[]);
    }
    b.inst(OP_MEMORY_MODEL, &[0, 1]);
    let mut interface: Vec<_> = inputs.values().copied().collect();
    interface.push(position);
    interface.extend(
        varying_inputs
            .values()
            .chain(varying_outputs.values())
            .copied(),
    );
    if let Some([_, _, variable]) = vertex_index {
        interface.push(variable);
    }
    b.inst_string(
        OP_ENTRY_POINT,
        &[if fragment { 4 } else { 0 }, main],
        "main",
        &interface,
    );
    if fragment {
        b.inst(16, &[main, 7]);
    } // OriginUpperLeft required by Vulkan.
    if let Some(flush) = denorm_mode {
        for mode in [4461, 4462, if flush { 4460 } else { 4459 }] {
            b.inst(16, &[main, mode, 32]);
        }
    }
    for result in arithmetic.values() {
        b.inst(OP_DECORATE, &[*result, 42]); // NoContraction: preserve separate operations.
    }
    for (location, input) in &inputs {
        b.inst(OP_DECORATE, &[*input, 30, *location]); // Location
    }
    for ((location, component), variable) in varying_inputs.iter().chain(&varying_outputs) {
        b.inst(OP_DECORATE, &[*variable, 30, *location]);
        b.inst(OP_DECORATE, &[*variable, 31, u32::from(*component)]);
    }
    for variable in varying_inputs.values() {
        b.inst(OP_DECORATE, &[*variable, 13]); // NoPerspective: IPA linear.
    }
    if let Some([_, _, variable]) = vertex_index {
        b.inst(OP_DECORATE, &[variable, 11, 42]); // BuiltIn VertexIndex, not Maxwell S2R ID.
    }
    if fragment {
        b.inst(OP_DECORATE, &[position, 30, 0]); // Fragment color location zero.
    } else {
        b.inst(OP_DECORATE, &[position, 11, 0]); // BuiltIn Position
    }
    for ((set, binding), (_, [_, array, block, _, variable])) in &uniforms {
        b.inst(OP_DECORATE, &[*array, 6, 16]); // ArrayStride
        b.inst(OP_DECORATE, &[*block, 2]); // Block
        b.inst(72, &[*block, 0, 35, 0]); // MemberDecorate Offset
        b.inst(OP_DECORATE, &[*variable, 34, *set]);
        b.inst(OP_DECORATE, &[*variable, 33, *binding]);
    }
    for (key, variable) in images.iter().chain(&samplers) {
        b.inst(OP_DECORATE, &[*variable, 34, key[0]]);
        b.inst(OP_DECORATE, &[*variable, 33, key[1]]);
    }
    b.inst(OP_TYPE_VOID, &[void]);
    b.inst(OP_TYPE_FLOAT, &[float, 32]);
    b.inst(OP_TYPE_INT, &[uint, 32, 0]);
    let word_conversion = shader
        .operations
        .iter()
        .any(|operation| matches!(operation, ShaderOperation::ConvertRich { .. }))
        .then(|| super::word_conversion::WordConversion::declare(&mut b, uint));
    if let Some([signed, pointer, variable]) = vertex_index {
        b.inst(OP_TYPE_INT, &[signed, 32, 1]);
        b.inst(OP_TYPE_POINTER, &[pointer, 1, signed]);
        b.inst(OP_VARIABLE, &[pointer, variable, 1]);
    }
    b.inst(OP_TYPE_VECTOR, &[vector, float, 4]);
    if let Some(
        [
            image,
            sampler,
            sampled,
            image_pointer,
            sampler_pointer,
            coords,
            zero,
            one,
        ],
    ) = texture_types
    {
        b.inst(25, &[image, float, 1, 0, 0, 0, 1, 0]); // TypeImage: 2D, sampled, float.
        b.inst(26, &[sampler]); // TypeSampler
        b.inst(27, &[sampled, image]); // TypeSampledImage
        b.inst(OP_TYPE_POINTER, &[image_pointer, 0, image]); // UniformConstant
        b.inst(OP_TYPE_POINTER, &[sampler_pointer, 0, sampler]);
        b.inst(OP_TYPE_VECTOR, &[coords, float, 2]);
        b.inst(OP_CONSTANT, &[float, zero, 0]);
        b.inst(OP_CONSTANT, &[float, one, 1.0_f32.to_bits()]);
        for variable in images.values() {
            b.inst(OP_VARIABLE, &[image_pointer, *variable, 0]);
        }
        for variable in samplers.values() {
            b.inst(OP_VARIABLE, &[sampler_pointer, *variable, 0]);
        }
    }
    b.inst(OP_TYPE_VECTOR, &[uniform_vector, uint, 4]);
    if let Some([signed, signed_vector, signed_pointer, unsigned_pointer]) = integer_inputs {
        if vertex_index.is_none() {
            b.inst(OP_TYPE_INT, &[signed, 32, 1]);
        }
        b.inst(OP_TYPE_VECTOR, &[signed_vector, signed, 4]);
        b.inst(OP_TYPE_POINTER, &[signed_pointer, 1, signed_vector]);
        b.inst(OP_TYPE_POINTER, &[unsigned_pointer, 1, uniform_vector]);
    }
    b.inst(OP_TYPE_POINTER, &[uniform_word_pointer, 2, uint]);
    for (size, [length, array, block, pointer, variable]) in uniforms.values() {
        b.inst(OP_CONSTANT, &[uint, *length, size / 16]);
        b.inst(28, &[*array, uniform_vector, *length]); // TypeArray
        b.inst(30, &[*block, *array]); // TypeStruct
        b.inst(OP_TYPE_POINTER, &[*pointer, 2, *block]);
        b.inst(OP_VARIABLE, &[*pointer, *variable, 2]);
    }
    b.inst(OP_TYPE_POINTER, &[input_pointer, 1, vector]);
    b.inst(OP_TYPE_POINTER, &[output_pointer, 3, vector]);
    b.inst(OP_TYPE_POINTER, &[output_scalar_pointer, 3, float]);
    if let Some(pointer) = varying_pointer {
        b.inst(OP_TYPE_POINTER, &[pointer, 1, float]);
        for variable in varying_inputs.values() {
            b.inst(OP_VARIABLE, &[pointer, *variable, 1]);
        }
    }
    for variable in varying_outputs.values() {
        b.inst(OP_VARIABLE, &[output_scalar_pointer, *variable, 3]);
    }
    b.inst(OP_TYPE_FUNCTION, &[function_type, void]);
    for (location, input) in &inputs {
        let pointer = match input_types[location] {
            None => input_pointer,
            Some(signed) => {
                let [_, _, signed_pointer, unsigned_pointer] =
                    integer_inputs.expect("integer input types");
                if signed {
                    signed_pointer
                } else {
                    unsigned_pointer
                }
            }
        };
        b.inst(OP_VARIABLE, &[pointer, *input, 1]);
    }
    b.inst(OP_VARIABLE, &[output_pointer, position, 3]);
    for (component, index) in indices.iter().enumerate() {
        b.inst(OP_CONSTANT, &[uint, *index, component as u32]);
    }
    for (mask, bits) in modifier_masks.into_iter().zip([0x7fffffffu32, 0x80000000]) {
        b.inst(OP_CONSTANT, &[uint, mask, bits]);
    }
    // SPIR-V constants belong outside functions. Their register assignment
    // still occurs in program order, so later definitions cannot satisfy an
    // earlier use of an undefined register.
    let mut constants = BTreeMap::new();
    for (ordinal, (vector_index, component)) in &uniform_offsets {
        let ShaderOperation::LoadUniformWord { byte_offset, .. } = shader.operations[*ordinal]
        else {
            unreachable!()
        };
        b.inst(OP_CONSTANT, &[uint, *vector_index, byte_offset / 16]);
        b.inst(OP_CONSTANT, &[uint, *component, (byte_offset % 16) / 4]);
    }
    for (ordinal, operation) in shader.operations.iter().enumerate() {
        if let Some((ty, bits)) = match operation {
            ShaderOperation::ConstantF32 { value, .. } => Some((float, value.to_bits())),
            ShaderOperation::ConstantBits32 { value, .. } => Some((uint, *value)),
            ShaderOperation::LoadVertexIndex { bias, .. } => Some((uint, *bias)),
            _ => None,
        } {
            let id = b.id();
            b.inst(OP_CONSTANT, &[ty, id, bits]);
            constants.insert(ordinal, id);
        }
    }
    if let Some([boolean, thirty_one, thirty_two]) = shift_types {
        b.inst(20, &[boolean]); // OpTypeBool
        b.inst(OP_CONSTANT, &[uint, thirty_one, 31]);
        b.inst(OP_CONSTANT, &[uint, thirty_two, 32]);
    }
    if let Some([eight, byte_mask]) = extract_constants {
        b.inst(OP_CONSTANT, &[uint, eight, 8]);
        b.inst(OP_CONSTANT, &[uint, byte_mask, 255]);
    }
    if let Some(word_find) = &word_find {
        word_find.declare_constants(&mut b);
    }
    b.inst(OP_FUNCTION, &[void, main, 0, function_type]);
    b.inst(OP_LABEL, &[label]);
    let mut registers = BTreeMap::new();
    let mut position_mask = 0u8;
    let mut returned = false;
    for (ordinal, operation) in shader.operations.iter().enumerate() {
        if returned {
            // Do not silently lower a suffix after a terminator.
            if matches!(operation, ShaderOperation::Nop) {
                continue;
            }
            return Err(SpirvCompileError::UnsupportedOperation);
        }
        match operation {
            ShaderOperation::Nop => {}
            ShaderOperation::SampleTexture2d {
                dst,
                image,
                sampler,
                coords,
                components,
                level_zero,
            } => {
                let [
                    image_type,
                    sampler_type,
                    sampled_type,
                    _,
                    _,
                    coords_type,
                    zero,
                    one,
                ] = texture_types.ok_or(SpirvCompileError::UnsupportedOperation)?;
                // Read both source registers before writing any destination;
                // Maxwell allows the sampled result to overwrite coordinates.
                let mut values = Vec::new();
                for coord in coords {
                    values.push(scalar_float(
                        *registers
                            .get(coord)
                            .ok_or(SpirvCompileError::UnsupportedOperation)?,
                        &mut b,
                        float,
                    )?);
                }
                let coordinate = b.id();
                let loaded_image = b.id();
                let loaded_sampler = b.id();
                let sampled = b.id();
                let result = b.id();
                b.inst(80, &[coords_type, coordinate, values[0], values[1]]); // CompositeConstruct
                b.inst(OP_LOAD, &[image_type, loaded_image, images[image]]);
                b.inst(OP_LOAD, &[sampler_type, loaded_sampler, samplers[sampler]]);
                b.inst(86, &[sampled_type, sampled, loaded_image, loaded_sampler]); // SampledImage
                if *level_zero {
                    b.inst(88, &[vector, result, sampled, coordinate, 2, zero]);
                } else {
                    b.inst(87, &[vector, result, sampled, coordinate]);
                }
                for (dst, component) in dst.iter().zip(components) {
                    let value = match component {
                        crate::shader::TextureSampleChannel::Zero => zero,
                        crate::shader::TextureSampleChannel::One => one,
                        crate::shader::TextureSampleChannel::Component(component) => {
                            let value = b.id();
                            b.inst(
                                OP_COMPOSITE_EXTRACT,
                                &[float, value, result, u32::from(*component)],
                            );
                            value
                        }
                    };
                    registers.insert(*dst, Value::Scalar(value));
                }
            }
            ShaderOperation::StoreVarying {
                location,
                component,
                src,
            } => {
                let value = *registers
                    .get(src)
                    .ok_or(SpirvCompileError::UnsupportedOperation)?;
                let value = scalar_float(value, &mut b, float)?;
                b.inst(
                    OP_STORE,
                    &[varying_outputs[&(*location, *component)], value],
                );
            }
            ShaderOperation::Interpolate {
                dst,
                attribute,
                component,
                ..
            } => {
                let value = b.id();
                b.inst(
                    OP_LOAD,
                    &[
                        float,
                        value,
                        varying_inputs[&((attribute - 0x80) / 16, *component)],
                    ],
                );
                registers.insert(*dst, Value::Scalar(value));
            }
            ShaderOperation::LoadVertexIndex { dst, .. } => {
                let [signed, _, variable] =
                    vertex_index.ok_or(SpirvCompileError::UnsupportedOperation)?;
                let value = b.id();
                let bits = b.id();
                let biased = b.id();
                b.inst(OP_LOAD, &[signed, value, variable]);
                b.inst(124, &[uint, bits, value]); // Preserve register bits, not float conversion.
                b.inst(128, &[uint, biased, bits, constants[&ordinal]]); // OpIAdd wraps at 32 bits.
                registers.insert(*dst, Value::Bits(biased));
            }
            ShaderOperation::LoadUniformWord {
                dst, set, binding, ..
            } => {
                let variable = uniforms[&(*set, *binding)].1[4];
                let (vector_index, component) = uniform_offsets[&ordinal];
                let pointer = b.id();
                let value = b.id();
                b.inst(
                    65,
                    &[
                        uniform_word_pointer,
                        pointer,
                        variable,
                        indices[0],
                        vector_index,
                        component,
                    ],
                );
                b.inst(OP_LOAD, &[uint, value, pointer]);
                registers.insert(*dst, Value::Bits(value));
            }
            ShaderOperation::LoadInputVec4 { dst, location } => {
                let value = b.id();
                b.inst(OP_LOAD, &[vector, value, inputs[location]]);
                registers.insert(*dst, Value::Vector(value));
            }
            ShaderOperation::LoadInput {
                dst,
                location,
                component,
            } => {
                if *component >= 4 {
                    return Err(SpirvCompileError::UnsupportedOperation);
                }
                let loaded = b.id();
                let value = b.id();
                b.inst(OP_LOAD, &[vector, loaded, inputs[location]]);
                b.inst(
                    OP_COMPOSITE_EXTRACT,
                    &[float, value, loaded, u32::from(*component)],
                );
                registers.insert(*dst, Value::Scalar(value));
            }
            ShaderOperation::LoadInputInteger {
                dst,
                location,
                component,
                signed,
            } => {
                if *component >= 4 {
                    return Err(SpirvCompileError::UnsupportedOperation);
                }
                let [signed_scalar, signed_vector, _, _] =
                    integer_inputs.ok_or(SpirvCompileError::UnsupportedOperation)?;
                let (scalar, vector) = if *signed {
                    (signed_scalar, signed_vector)
                } else {
                    (uint, uniform_vector)
                };
                let loaded = b.id();
                let value = b.id();
                b.inst(OP_LOAD, &[vector, loaded, inputs[location]]);
                b.inst(
                    OP_COMPOSITE_EXTRACT,
                    &[scalar, value, loaded, u32::from(*component)],
                );
                let bits = if *signed {
                    let bits = b.id();
                    b.inst(124, &[uint, bits, value]);
                    bits
                } else {
                    value
                };
                registers.insert(*dst, Value::Bits(bits));
            }
            ShaderOperation::ConstantF32 { dst, .. } => {
                registers.insert(*dst, Value::Scalar(constants[&ordinal]));
            }
            ShaderOperation::ConstantBits32 { dst, .. } => {
                registers.insert(*dst, Value::Bits(constants[&ordinal]));
            }
            ShaderOperation::Move { dst, src } => {
                let value = *registers
                    .get(src)
                    .ok_or(SpirvCompileError::UnsupportedOperation)?;
                registers.insert(*dst, value);
            }
            ShaderOperation::ConvertRich { dst, src, control } => {
                if !control.is_plain_word_to_f32() {
                    return Err(SpirvCompileError::UnsupportedOperation);
                }
                let source = match registers.get(src).copied() {
                    Some(Value::Bits(bits)) => bits,
                    Some(Value::Scalar(value)) => {
                        let bits = b.id();
                        b.inst(124, &[uint, bits, value]);
                        bits
                    }
                    _ => return Err(SpirvCompileError::UnsupportedOperation),
                };
                let result = word_conversion
                    .as_ref()
                    .ok_or(SpirvCompileError::UnsupportedOperation)?
                    .emit(&mut b, source, *control);
                registers.insert(*dst, Value::Bits(result));
            }
            ShaderOperation::ShiftWord32 {
                dst,
                value,
                amount,
                kind,
                wrap,
            } => {
                use crate::shader::WordShiftKind as K;
                let value = scalar_word(
                    *registers
                        .get(value)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    &mut b,
                    uint,
                )?;
                let amount = scalar_word(
                    *registers
                        .get(amount)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    &mut b,
                    uint,
                )?;
                let [boolean, thirty_one, thirty_two] =
                    shift_types.ok_or(SpirvCompileError::UnsupportedOperation)?;
                // Even the discarded side of OpSelect must not shift by >=32:
                // mask before shifting, then select the Maxwell clamp result.
                let masked = b.id();
                b.inst(199, &[uint, masked, amount, thirty_one]);
                let shifted = b.id();
                let opcode = match kind {
                    K::Left => 196,
                    K::RightLogical => 194,
                    K::RightArithmetic => 195,
                };
                b.inst(opcode, &[uint, shifted, value, masked]);
                let result = if *wrap {
                    shifted
                } else {
                    let in_range = b.id();
                    b.inst(176, &[boolean, in_range, amount, thirty_two]); // OpULessThan
                    let fallback = if *kind == K::RightArithmetic {
                        let sign = b.id();
                        b.inst(195, &[uint, sign, value, thirty_one]);
                        sign
                    } else {
                        indices[0]
                    };
                    let result = b.id();
                    b.inst(169, &[uint, result, in_range, shifted, fallback]); // OpSelect
                    result
                };
                registers.insert(*dst, Value::Bits(result));
            }
            ShaderOperation::FindLeadingWord32 {
                dst,
                value,
                signed,
                invert,
                shift_amount,
            } => {
                let value = scalar_word(
                    *registers
                        .get(value)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    &mut b,
                    uint,
                )?;
                let result = word_find
                    .as_ref()
                    .ok_or(SpirvCompileError::UnsupportedOperation)?
                    .emit(&mut b, value, *signed, *invert, *shift_amount);
                registers.insert(*dst, Value::Bits(result));
            }
            ShaderOperation::CountBits32 { dst, value, invert } => {
                let value = scalar_word(
                    *registers
                        .get(value)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    &mut b,
                    uint,
                )?;
                let source = if *invert {
                    let inverted = b.id();
                    b.inst(200, &[uint, inverted, value]); // OpNot
                    inverted
                } else {
                    value
                };
                let result = b.id();
                b.inst(205, &[uint, result, source]); // OpBitCount
                registers.insert(*dst, Value::Bits(result));
            }
            ShaderOperation::ExtractWord32 {
                dst,
                value,
                control,
                signed,
            } => {
                // Read both operands before replacing dst (Maxwell permits aliasing).
                let value = scalar_word(
                    *registers
                        .get(value)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    &mut b,
                    uint,
                )?;
                let control = scalar_word(
                    *registers
                        .get(control)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    &mut b,
                    uint,
                )?;
                let [boolean, thirty_one, thirty_two] =
                    shift_types.ok_or(SpirvCompileError::UnsupportedOperation)?;
                let [eight, byte_mask] =
                    extract_constants.ok_or(SpirvCompileError::UnsupportedOperation)?;
                let offset = b.id();
                b.inst(199, &[uint, offset, control, byte_mask]);
                let upper = b.id();
                b.inst(194, &[uint, upper, control, eight]);
                let width = b.id();
                b.inst(199, &[uint, width, upper, byte_mask]);
                let offset_in_range = b.id();
                b.inst(176, &[boolean, offset_in_range, offset, thirty_two]);
                let safe_offset = b.id();
                b.inst(
                    169,
                    &[uint, safe_offset, offset_in_range, offset, thirty_one],
                );
                let right = if *signed { 195 } else { 194 };
                let shifted = b.id();
                b.inst(right, &[uint, shifted, value, safe_offset]);
                let fallback = if *signed { shifted } else { indices[0] };
                let extended = b.id();
                b.inst(169, &[uint, extended, offset_in_range, shifted, fallback]);
                // Every executed shift is <=31, even on an unselected branch.
                // Arithmetic shifting first preserves the original sign when
                // the field extends past the source word's most significant bit.
                let narrow = b.id();
                b.inst(176, &[boolean, narrow, width, thirty_two]);
                let padding = b.id();
                b.inst(130, &[uint, padding, thirty_two, width]);
                let safe_padding = b.id();
                b.inst(199, &[uint, safe_padding, padding, thirty_one]);
                let left = b.id();
                b.inst(196, &[uint, left, extended, safe_padding]);
                let extracted = b.id();
                b.inst(right, &[uint, extracted, left, safe_padding]);
                let sized = b.id();
                b.inst(169, &[uint, sized, narrow, extracted, extended]);
                let empty = b.id();
                b.inst(170, &[boolean, empty, width, indices[0]]); // OpIEqual
                let result = b.id();
                b.inst(169, &[uint, result, empty, indices[0], sized]);
                registers.insert(*dst, Value::Bits(result));
            }
            ShaderOperation::IntegerBinary {
                dst,
                op,
                left,
                right,
                left_modifier,
                right_modifier,
                control,
            } => {
                let opcode = integer_opcode(*op, [*left_modifier, *right_modifier], *control)?;
                // Read both sources before replacing dst, which may alias either.
                let mut left = scalar_word(
                    *registers
                        .get(left)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    &mut b,
                    uint,
                )?;
                let mut right = scalar_word(
                    *registers
                        .get(right)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    &mut b,
                    uint,
                )?;
                for (word, modifier) in [(&mut left, left_modifier), (&mut right, right_modifier)] {
                    if modifier.invert {
                        let result = b.id();
                        b.inst(200, &[uint, result, *word]); // OpNot: invert every bit, not numeric negation.
                        *word = result;
                    }
                    if modifier.negate {
                        let result = b.id();
                        b.inst(130, &[uint, result, indices[0], *word]); // OpISub: 0 - word modulo 2^32.
                        *word = result;
                    }
                }
                let result = b.id();
                b.inst(opcode, &[uint, result, left, right]);
                registers.insert(*dst, Value::Bits(result));
            }
            ShaderOperation::ModifiedBinary {
                dst,
                op,
                left,
                right,
                left_modifier,
                right_modifier,
                ..
            } => {
                let left = modified_scalar_float(
                    *registers
                        .get(left)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    *left_modifier,
                    &mut b,
                    float,
                    uint,
                    modifier_masks,
                )?;
                let right = modified_scalar_float(
                    *registers
                        .get(right)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                    *right_modifier,
                    &mut b,
                    float,
                    uint,
                    modifier_masks,
                )?;
                let result = arithmetic[&ordinal];
                b.inst(
                    if *op == crate::shader::BinaryOp::Add {
                        129
                    } else {
                        133
                    },
                    &[float, result, left, right],
                );
                registers.insert(*dst, Value::Scalar(result));
            }
            ShaderOperation::ComposeVec4 { dst, components } => {
                let mut sources = [0; 4];
                for (source, register) in sources.iter_mut().zip(components) {
                    let value = *registers
                        .get(register)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?;
                    *source = scalar_float(value, &mut b, float)?;
                }
                let value = b.id();
                b.inst(
                    OP_COMPOSITE_CONSTRUCT,
                    &[
                        vector, value, sources[0], sources[1], sources[2], sources[3],
                    ],
                );
                registers.insert(*dst, Value::Vector(value));
            }
            ShaderOperation::StoreBuiltin { builtin: 0, src } => {
                let Some(Value::Vector(value)) = registers.get(src) else {
                    return Err(SpirvCompileError::UnsupportedOperation);
                };
                b.inst(OP_STORE, &[position, *value]);
                position_mask = 0xf;
            }
            ShaderOperation::StoreBuiltinComponent {
                builtin: 0,
                component,
                src,
            }
            | ShaderOperation::StoreOutput {
                location: 0,
                component,
                src,
            } => {
                if *component >= 4 {
                    return Err(SpirvCompileError::UnsupportedOperation);
                }
                let value = *registers
                    .get(src)
                    .ok_or(SpirvCompileError::UnsupportedOperation)?;
                let value = scalar_float(value, &mut b, float)?;
                let pointer = b.id();
                b.inst(
                    65,
                    &[
                        output_scalar_pointer,
                        pointer,
                        position,
                        indices[usize::from(*component)],
                    ],
                ); // OpAccessChain
                b.inst(OP_STORE, &[pointer, value]);
                position_mask |= 1 << component;
            }
            ShaderOperation::Return => {
                b.inst(OP_RETURN, &[]);
                returned = true;
            }
            _ => return Err(SpirvCompileError::UnsupportedOperation),
        }
    }
    if position_mask != 0xf || !returned {
        return Err(SpirvCompileError::UnsupportedOperation);
    }
    b.inst(OP_FUNCTION_END, &[]);
    Ok(b.finish())
}

#[cfg(test)]
mod tests {
    #[test]
    fn integer_word_arithmetic_rejects_unimplemented_controls() {
        use crate::shader::{BinaryOp, IntegerControl, ValueModifier};
        let plain = ValueModifier {
            negate: false,
            absolute: false,
            invert: false,
        };
        let control = IntegerControl {
            saturate: false,
            carry_in: false,
            carry_out: false,
            high: false,
            signed: false,
        };
        for signed in [false, true] {
            let control = IntegerControl { signed, ..control };
            assert_eq!(
                super::integer_opcode(BinaryOp::Add, [plain; 2], control),
                Ok(128)
            );
            assert_eq!(
                super::integer_opcode(BinaryOp::Mul, [plain; 2], control),
                Ok(132)
            );
        }
        for flag in 0..4 {
            let changed = IntegerControl {
                saturate: flag == 0,
                carry_in: flag == 1,
                carry_out: flag == 2,
                high: flag == 3,
                ..control
            };
            assert!(super::integer_opcode(BinaryOp::Add, [plain; 2], changed).is_err());
        }
        for source in 0..2 {
            for flag in 1..3 {
                let mut modifiers = [plain; 2];
                modifiers[source] = ValueModifier {
                    negate: flag == 0,
                    absolute: flag == 1,
                    invert: flag == 2,
                };
                assert!(super::integer_opcode(BinaryOp::Mul, modifiers, control).is_err());
            }
        }
        assert!(super::integer_opcode(BinaryOp::ShiftLeft, [plain; 2], control).is_err());
    }

    #[test]
    fn texture_sampling_rejects_invalid_bindings_channels_and_undefined_coordinates() {
        use crate::{ShaderModule, ShaderOperation as S, ShaderStage, spirv};
        let sample = S::SampleTexture2d {
            dst: vec![0, 1, 2, 3],
            image: [1, 4],
            sampler: [1, 5],
            coords: [7, 7],
            components: [0, 1, 2, 3]
                .map(crate::shader::TextureSampleChannel::Component)
                .to_vec(),
            level_zero: false,
        };
        let mut operations = vec![
            S::ConstantBits32 {
                dst: 7,
                value: 0x3f00_0000,
            },
            sample,
        ];
        operations.extend((0..4).map(|component| S::StoreOutput {
            location: 0,
            component,
            src: u32::from(component),
        }));
        operations.push(S::Return);
        let shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Fragment,
            variant_key: vec![],
            operations,
        };
        assert!(spirv::compile(&shader).is_ok());
        let mut constants = shader.clone();
        if let S::SampleTexture2d { components, .. } = &mut constants.operations[1] {
            use crate::shader::TextureSampleChannel as C;
            *components = vec![C::Zero, C::One, C::Component(2), C::One];
        }
        assert!(spirv::compile(&constants).is_ok());
        for mode in 0..6 {
            let mut invalid = shader.clone();
            if let S::SampleTexture2d {
                dst,
                image,
                sampler,
                coords,
                components,
                ..
            } = &mut invalid.operations[1]
            {
                match mode {
                    0 => *sampler = *image,
                    1 => components[0] = crate::shader::TextureSampleChannel::Component(4),
                    2 => {
                        dst.pop();
                    }
                    3 => coords[0] = 6,
                    4 => dst[0] = dst[1],
                    _ => {
                        dst.clear();
                        components.clear();
                    }
                }
            }
            assert!(
                spirv::compile(&invalid).is_err(),
                "invalid texture mode {mode}"
            );
        }
        let mut invalid = shader.clone();
        invalid.operations.insert(
            0,
            S::LoadUniformWord {
                dst: 8,
                set: 1,
                binding: 4,
                byte_offset: 0,
                byte_size: 16,
            },
        );
        assert!(spirv::compile(&invalid).is_err());
        invalid = shader;
        invalid.stage = ShaderStage::Vertex;
        assert!(spirv::compile(&invalid).is_err());
    }
    #[test]
    fn integer_inputs_have_consistent_location_types_and_register_bits() {
        use super::*;
        let mut shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![
                ShaderOperation::LoadInputInteger {
                    dst: 0,
                    location: 0,
                    component: 0,
                    signed: true,
                },
                ShaderOperation::LoadInputInteger {
                    dst: 1,
                    location: 1,
                    component: 0,
                    signed: false,
                },
                ShaderOperation::LoadVertexIndex { dst: 2, bias: 0 },
                ShaderOperation::ConstantF32 { dst: 3, value: 1.0 },
                ShaderOperation::ComposeVec4 {
                    dst: 4,
                    components: [0, 1, 2, 3],
                },
                ShaderOperation::StoreBuiltin { builtin: 0, src: 4 },
                ShaderOperation::Return,
            ],
        };
        assert!(compile(&shader).is_ok());
        for conflicting in [
            ShaderOperation::LoadInput {
                dst: 5,
                location: 0,
                component: 0,
            },
            ShaderOperation::LoadInputInteger {
                dst: 5,
                location: 0,
                component: 0,
                signed: false,
            },
            ShaderOperation::LoadInputInteger {
                dst: 5,
                location: 2,
                component: 4,
                signed: true,
            },
        ] {
            shader.operations.insert(0, conflicting);
            assert!(compile(&shader).is_err());
            shader.operations.remove(0);
        }
        shader.stage = ShaderStage::Fragment;
        assert!(compile(&shader).is_err());
    }

    #[test]
    fn vertex_index_emits_signed_builtin_and_integer_register_bias() {
        use super::*;
        let mut shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![
                ShaderOperation::LoadVertexIndex {
                    dst: 9,
                    bias: u32::MAX,
                },
                ShaderOperation::ConstantF32 { dst: 0, value: 0.0 },
                ShaderOperation::ConstantF32 { dst: 1, value: 1.0 },
                ShaderOperation::ComposeVec4 {
                    dst: 10,
                    components: [0, 0, 9, 1],
                },
                ShaderOperation::StoreBuiltin {
                    builtin: 0,
                    src: 10,
                },
                ShaderOperation::Return,
            ],
        };
        let words = compile(&shader).unwrap().words;
        let mut position = 5;
        let mut decoration = false;
        let mut integer_add = false;
        while position < words.len() {
            let length = (words[position] >> 16) as usize;
            let opcode = (words[position] & 0xffff) as u16;
            let operands = &words[position + 1..position + length];
            decoration |= opcode == OP_DECORATE && operands.len() == 3 && operands[1..] == [11, 42];
            integer_add |= opcode == 128;
            position += length;
        }
        assert!(decoration && integer_add);
        shader.stage = ShaderStage::Fragment;
        assert!(compile(&shader).is_err());
    }
    use super::*;

    #[test]
    fn uniform_words_emit_runtime_loads_and_reject_invalid_ranges() {
        let mut operations = Vec::new();
        for component in 0..4 {
            operations.push(ShaderOperation::LoadUniformWord {
                dst: component,
                set: 2,
                binding: 7,
                byte_offset: 16 + 4 * component,
                byte_size: 32,
            });
            operations.push(ShaderOperation::StoreBuiltinComponent {
                builtin: 0,
                component: component as u8,
                src: component,
            });
        }
        operations.push(ShaderOperation::Return);
        let shader = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations,
        };
        let module = compile(&shader).unwrap();
        let mut cursor = 5;
        let mut loads = 0;
        let mut bitcasts = 0;
        while cursor < module.words.len() {
            match module.words[cursor] & 0xffff {
                61 => loads += 1,
                124 => bitcasts += 1,
                _ => {}
            }
            cursor += (module.words[cursor] >> 16) as usize;
        }
        assert_eq!((loads, bitcasts), (4, 4));
        let mut fragment = shader.clone();
        fragment.stage = ShaderStage::Fragment;
        assert!(compile(&fragment).is_err()); // Position is not a fragment export.
        for operation in &mut fragment.operations {
            if let ShaderOperation::StoreBuiltinComponent { component, src, .. } = operation {
                *operation = ShaderOperation::StoreOutput {
                    location: 0,
                    component: *component,
                    src: *src,
                };
            }
        }
        assert!(compile(&fragment).is_ok());
        fragment.operations.retain(|operation| {
            !matches!(operation, ShaderOperation::StoreOutput { component: 3, .. })
        });
        assert!(compile(&fragment).is_err());
        for (byte_offset, byte_size) in [(1, 32), (32, 32), (0, 0), (0, 17), (0, 0x20000), (0, 48)]
        {
            let mut invalid = shader.clone();
            invalid.operations[0] = ShaderOperation::LoadUniformWord {
                dst: 0,
                set: 2,
                binding: 7,
                byte_offset,
                byte_size,
            };
            assert!(compile(&invalid).is_err());
        }
    }

    #[test]
    fn source_modifiers_use_sign_masks_in_abs_then_neg_order() {
        use crate::shader::ValueModifier;
        for value in [0u32, 0x80000000, 0x7fa12345, 0xffa12345] {
            for flags in 0..4 {
                let mut builder = Builder::new();
                let uint = builder.id();
                let float = builder.id();
                let source = builder.id();
                let masks = [builder.id(), builder.id()];
                builder.inst(OP_CONSTANT, &[uint, source, value]);
                let start = builder.words.len();
                modified_scalar_float(
                    Value::Bits(source),
                    ValueModifier {
                        absolute: flags & 1 != 0,
                        negate: flags & 2 != 0,
                        invert: false,
                    },
                    &mut builder,
                    float,
                    uint,
                    masks,
                )
                .unwrap();
                let mut operations = Vec::new();
                let mut cursor = start;
                while cursor < builder.words.len() {
                    let length = (builder.words[cursor] >> 16) as usize;
                    operations.push((
                        builder.words[cursor] & 0xffff,
                        &builder.words[cursor + 1..cursor + length],
                    ));
                    cursor += length;
                }
                let expected: Vec<_> = [(flags & 1 != 0, 199), (flags & 2 != 0, 198)]
                    .into_iter()
                    .filter_map(|(enabled, opcode)| enabled.then_some(opcode))
                    .chain([124])
                    .collect();
                assert_eq!(
                    operations
                        .iter()
                        .map(|(opcode, _)| *opcode)
                        .collect::<Vec<_>>(),
                    expected
                );
                for (opcode, operands) in operations.iter().filter(|(opcode, _)| *opcode != 124) {
                    assert_eq!(operands[0], uint);
                    assert_eq!(operands[3], masks[usize::from(*opcode == 198)]);
                }
            }
        }
        let mut builder = Builder::new();
        assert!(
            modified_scalar_float(
                Value::Bits(1),
                ValueModifier {
                    absolute: false,
                    negate: false,
                    invert: true,
                },
                &mut builder,
                2,
                3,
                [4, 5]
            )
            .is_err()
        );
    }

    #[test]
    fn float_arithmetic_emits_explicit_modes_and_separate_operations() {
        use crate::shader::{BinaryOp, FloatControl, ValueModifier};
        let plain = ValueModifier {
            negate: false,
            absolute: false,
            invert: false,
        };
        let mut shader = ShaderModule {
            id: 9,
            stage: ShaderStage::Vertex,
            variant_key: Vec::new(),
            operations: vec![
                ShaderOperation::ConstantBits32 {
                    dst: 0,
                    value: 0x3f800000,
                },
                ShaderOperation::ConstantF32 { dst: 1, value: 0.5 },
                ShaderOperation::ModifiedBinary {
                    dst: 2,
                    op: BinaryOp::Add,
                    left: 0,
                    right: 1,
                    left_modifier: plain,
                    right_modifier: plain,
                    float: FloatControl {
                        saturate: false,
                        flush_to_zero: false,
                    },
                },
                ShaderOperation::ModifiedBinary {
                    dst: 3,
                    op: BinaryOp::Mul,
                    left: 2,
                    right: 1,
                    left_modifier: plain,
                    right_modifier: plain,
                    float: FloatControl {
                        saturate: false,
                        flush_to_zero: false,
                    },
                },
            ],
        };
        for component in 0..4 {
            shader
                .operations
                .push(ShaderOperation::StoreBuiltinComponent {
                    builtin: 0,
                    component,
                    src: 3,
                });
        }
        shader.operations.push(ShaderOperation::Return);
        let module = super::super::compile(&shader).unwrap();
        let mut instructions = Vec::new();
        let mut offset = 5;
        while offset < module.words.len() {
            let length = (module.words[offset] >> 16) as usize;
            instructions.push((
                module.words[offset] & 0xffff,
                &module.words[offset + 1..offset + length],
            ));
            offset += length;
        }
        for opcode in [129, 133] {
            assert_eq!(
                instructions.iter().filter(|(op, _)| *op == opcode).count(),
                1
            );
        }
        assert_eq!(
            instructions
                .iter()
                .filter(|(op, args)| *op == 71 && args[1] == 42)
                .count(),
            2
        );
        for mode in [4459, 4461, 4462] {
            assert!(
                instructions
                    .iter()
                    .any(|(op, args)| *op == 16 && args[1..] == [mode, 32])
            );
        }
        if let ShaderOperation::ModifiedBinary { float, .. } = &mut shader.operations[3] {
            float.flush_to_zero = true;
        }
        assert!(super::super::compile(&shader).is_err());
        if let ShaderOperation::ModifiedBinary { float, .. } = &mut shader.operations[2] {
            float.flush_to_zero = true;
        }
        assert!(super::super::compile(&shader).is_ok());
        if let ShaderOperation::ModifiedBinary { float, .. } = &mut shader.operations[3] {
            float.saturate = true;
        }
        assert!(super::super::compile(&shader).is_err());
    }

    #[test]
    fn raw_register_constants_preserve_bits_without_numeric_conversion() {
        let values = [0u32, 0x80000000, 0x7fa12345, 0xff800000];
        let mut operations: Vec<_> = values
            .into_iter()
            .enumerate()
            .map(|(dst, value)| ShaderOperation::ConstantBits32 {
                dst: dst as u32,
                value,
            })
            .collect();
        operations.push(ShaderOperation::Move { dst: 4, src: 0 });
        for component in 0..4 {
            operations.push(ShaderOperation::StoreBuiltinComponent {
                builtin: 0,
                component,
                src: if component == 0 {
                    4
                } else {
                    u32::from(component)
                },
            });
        }
        operations.push(ShaderOperation::Return);
        let shader = ShaderModule {
            id: 104,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations,
        };
        let module = super::super::compile(&shader).unwrap();
        let mut cursor = 5;
        let mut bitcasts = 0;
        let mut constants = Vec::new();
        while cursor < module.words.len() {
            let header = module.words[cursor];
            let words = (header >> 16) as usize;
            match header as u16 {
                OP_CONSTANT => constants.push(module.words[cursor + 3]),
                124 => bitcasts += 1,
                OP_CONVERT_U_TO_F => panic!("register bits were numerically converted"),
                _ => {}
            }
            cursor += words;
        }
        assert_eq!(bitcasts, 4);
        for value in values {
            assert!(constants.contains(&value));
        }
        let mut numeric = shader.clone();
        numeric.operations[0] = ShaderOperation::ConstantU32 { dst: 0, value: 0 };
        assert_eq!(
            super::super::compile(&numeric),
            Err(SpirvCompileError::UnsupportedOperation)
        );
    }

    #[test]
    fn scalar_inputs_require_valid_components_and_typed_composition() {
        let mut shader = ShaderModule {
            id: 102,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![
                ShaderOperation::LoadInput {
                    dst: 0,
                    location: 2,
                    component: 0,
                },
                ShaderOperation::LoadInput {
                    dst: 1,
                    location: 2,
                    component: 1,
                },
                ShaderOperation::ConstantF32 { dst: 2, value: 0.0 },
                ShaderOperation::ConstantF32 { dst: 3, value: 1.0 },
                ShaderOperation::ComposeVec4 {
                    dst: 4,
                    components: [0, 1, 2, 3],
                },
                ShaderOperation::Move { dst: 0, src: 4 },
                ShaderOperation::StoreBuiltin { builtin: 0, src: 0 },
                ShaderOperation::Return,
            ],
        };
        assert!(super::super::compile(&shader).is_ok());
        shader.operations[0] = ShaderOperation::LoadInput {
            dst: 0,
            location: 2,
            component: 4,
        };
        assert_eq!(
            super::super::compile(&shader),
            Err(SpirvCompileError::UnsupportedOperation)
        );
        shader.operations[0] = ShaderOperation::LoadInputVec4 {
            dst: 0,
            location: 2,
        };
        assert_eq!(
            super::super::compile(&shader),
            Err(SpirvCompileError::UnsupportedOperation)
        );
        shader.operations[0] = ShaderOperation::Move { dst: 0, src: 2 };
        assert_eq!(
            super::super::compile(&shader),
            Err(SpirvCompileError::UnsupportedOperation)
        );
    }

    #[test]
    fn explicit_vertex_input_requires_defined_position_and_termination() {
        let mut shader = ShaderModule {
            id: 101,
            stage: ShaderStage::Vertex,
            variant_key: vec![],
            operations: vec![
                ShaderOperation::LoadInputVec4 {
                    dst: 7,
                    location: 2,
                },
                ShaderOperation::StoreBuiltin { builtin: 0, src: 7 },
                ShaderOperation::Return,
            ],
        };
        assert!(super::super::compile(&shader).is_ok());
        shader.operations[1] = ShaderOperation::StoreBuiltin { builtin: 0, src: 8 };
        assert_eq!(
            super::super::compile(&shader),
            Err(SpirvCompileError::UnsupportedOperation)
        );
        shader.operations[1] = ShaderOperation::StoreBuiltin { builtin: 0, src: 7 };
        shader.operations.pop();
        assert_eq!(
            super::super::compile(&shader),
            Err(SpirvCompileError::UnsupportedOperation)
        );
    }
}
