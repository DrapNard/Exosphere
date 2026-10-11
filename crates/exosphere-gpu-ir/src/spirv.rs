// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! SPIR-V emission for portable Exosphere shader IR.
//!
//! This intentionally supports only semantics Exosphere understands exactly. Maxwell instructions
//! that have not yet been lifted remain `OpaqueMaxwell` and fail compilation instead of producing
//! subtly wrong host shaders.

use crate::shader::{ShaderModule, ShaderOperation, ShaderStage};
use std::fmt;

#[cfg(test)]
mod test_integer;
mod vertex_input;
mod word_conversion;
mod word_find;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpirvModule {
    pub words: Vec<u32>,
}

impl SpirvModule {
    pub const MAGIC: u32 = 0x0723_0203;

    pub fn from_words(words: Vec<u32>) -> Result<Self, &'static str> {
        if words.len() < 5 || words[0] != Self::MAGIC {
            return Err("invalid SPIR-V header");
        }
        Ok(Self { words })
    }

    pub fn bytes_le(&self) -> Vec<u8> {
        self.words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpirvCompileError {
    UnsupportedStage(ShaderStage),
    OpaqueMaxwell,
    UnsupportedOperation,
}

impl fmt::Display for SpirvCompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedStage(stage) => {
                write!(f, "SPIR-V smoke compiler does not support {stage:?}")
            }
            Self::OpaqueMaxwell => write!(f, "shader still contains opaque Maxwell operations"),
            Self::UnsupportedOperation => write!(
                f,
                "shader contains semantic operations not yet supported by the SPIR-V backend"
            ),
        }
    }
}

impl std::error::Error for SpirvCompileError {}

/// Compiles the portable subset of Shader IR into a valid SPIR-V module.
///
/// Outputs must be explicit; missing outputs never produce diagnostic geometry
/// or default colors. Host fixtures can explicitly request `SmokeVertexTriangle`.
pub(crate) fn supports_integer_binary(operation: &ShaderOperation) -> bool {
    match operation {
        ShaderOperation::IntegerBinary {
            op,
            left_modifier,
            right_modifier,
            control,
            ..
        } => vertex_input::integer_opcode(*op, [*left_modifier, *right_modifier], *control).is_ok(),
        _ => false,
    }
}

pub fn compile(shader: &ShaderModule) -> Result<SpirvModule, SpirvCompileError> {
    if shader
        .operations
        .iter()
        .any(|operation| matches!(operation, ShaderOperation::OpaqueMaxwell { .. }))
    {
        return Err(SpirvCompileError::OpaqueMaxwell);
    }
    if shader.stage == ShaderStage::Vertex
        && shader.operations.iter().any(|operation| {
            matches!(
                operation,
                ShaderOperation::LoadInputVec4 { .. }
                    | ShaderOperation::LoadInputInteger { .. }
                    | ShaderOperation::StoreBuiltin { .. }
                    | ShaderOperation::StoreBuiltinComponent { .. }
            )
        })
    {
        return vertex_input::compile(shader);
    }
    if shader.stage == ShaderStage::Fragment
        && shader.operations.iter().any(|operation| {
            matches!(
                operation,
                ShaderOperation::LoadUniformWord { .. }
                    | ShaderOperation::IntegerBinary { .. }
                    | ShaderOperation::ShiftWord32 { .. }
                    | ShaderOperation::ExtractWord32 { .. }
                    | ShaderOperation::CountBits32 { .. }
                    | ShaderOperation::FindLeadingWord32 { .. }
                    | ShaderOperation::ConvertRich { .. }
                    | ShaderOperation::Interpolate { .. }
                    | ShaderOperation::SampleTexture2d { .. }
            )
        })
    {
        return vertex_input::compile(shader);
    }
    if shader.stage == ShaderStage::Fragment
        && shader
            .operations
            .iter()
            .any(|operation| matches!(operation, ShaderOperation::StoreOutput { .. }))
    {
        return compile_constant_fragment(shader);
    }
    match shader.stage {
        ShaderStage::Vertex
            if shader.operations
                == [
                    ShaderOperation::SmokeVertexTriangle,
                    ShaderOperation::Return,
                ] =>
        {
            Ok(build_simple_vertex())
        }
        ShaderStage::Vertex | ShaderStage::Fragment => Err(SpirvCompileError::UnsupportedOperation),
        stage => Err(SpirvCompileError::UnsupportedStage(stage)),
    }
}

fn compile_constant_fragment(shader: &ShaderModule) -> Result<SpirvModule, SpirvCompileError> {
    let mut registers = std::collections::BTreeMap::<u32, u32>::new();
    let mut color = [None; 4];
    let mut returned = false;
    for operation in &shader.operations {
        if returned && !matches!(operation, ShaderOperation::Nop) {
            return Err(SpirvCompileError::UnsupportedOperation);
        }
        match *operation {
            ShaderOperation::Nop => {}
            ShaderOperation::ConstantF32 { dst, value } => {
                registers.insert(dst, value.to_bits());
            }
            ShaderOperation::ConstantBits32 { dst, value } => {
                registers.insert(dst, value);
            }
            ShaderOperation::Move { dst, src } => {
                let value = *registers
                    .get(&src)
                    .ok_or(SpirvCompileError::UnsupportedOperation)?;
                registers.insert(dst, value);
            }
            ShaderOperation::StoreOutput {
                location: 0,
                component,
                src,
            } if component < 4 => {
                color[component as usize] = Some(
                    *registers
                        .get(&src)
                        .ok_or(SpirvCompileError::UnsupportedOperation)?,
                );
            }
            ShaderOperation::Return => returned = true,
            _ => return Err(SpirvCompileError::UnsupportedOperation),
        }
    }
    if !returned || color.iter().any(Option::is_none) {
        return Err(SpirvCompileError::UnsupportedOperation);
    }
    // Constant folding preserves raw register bits and store order. This path
    // supplies no default channels and never treats a definition as an output.
    Ok(build_simple_fragment(
        color.map(|bits| f32::from_bits(bits.unwrap())),
    ))
}

struct Builder {
    words: Vec<u32>,
    next_id: u32,
}

impl Builder {
    fn new() -> Self {
        Self {
            words: vec![SpirvModule::MAGIC, 0x0001_0000, 0, 0, 0],
            next_id: 1,
        }
    }

    fn id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn inst(&mut self, opcode: u16, operands: &[u32]) {
        let word_count = 1u32 + operands.len() as u32;
        self.words.push((word_count << 16) | u32::from(opcode));
        self.words.extend_from_slice(operands);
    }

    fn inst_string(&mut self, opcode: u16, prefix: &[u32], value: &str, suffix: &[u32]) {
        let mut operands = prefix.to_vec();
        operands.extend(pack_string(value));
        operands.extend_from_slice(suffix);
        self.inst(opcode, &operands);
    }

    fn finish(mut self) -> SpirvModule {
        self.words[3] = self.next_id;
        SpirvModule { words: self.words }
    }
}

// SPIR-V opcodes used by the minimal emitter.
const OP_MEMORY_MODEL: u16 = 14;
const OP_ENTRY_POINT: u16 = 15;
const OP_EXECUTION_MODE: u16 = 16;
const OP_CAPABILITY: u16 = 17;
const OP_TYPE_VOID: u16 = 19;
const OP_TYPE_INT: u16 = 21;
const OP_TYPE_FLOAT: u16 = 22;
const OP_TYPE_VECTOR: u16 = 23;
const OP_TYPE_POINTER: u16 = 32;
const OP_TYPE_FUNCTION: u16 = 33;
const OP_CONSTANT: u16 = 43;
const OP_CONSTANT_COMPOSITE: u16 = 44;
const OP_COMPOSITE_CONSTRUCT: u16 = 80;
const OP_COMPOSITE_EXTRACT: u16 = 81;
const OP_FUNCTION: u16 = 54;
const OP_FUNCTION_END: u16 = 56;
const OP_VARIABLE: u16 = 59;
const OP_LOAD: u16 = 61;
const OP_STORE: u16 = 62;
const OP_CONVERT_U_TO_F: u16 = 112;
const OP_FSUB: u16 = 131;
const OP_SHIFT_LEFT_LOGICAL: u16 = 196;
const OP_BITWISE_AND: u16 = 199;
const OP_DECORATE: u16 = 71;
const OP_LABEL: u16 = 248;
const OP_RETURN: u16 = 253;

fn build_simple_vertex() -> SpirvModule {
    // Generate a real triangle from the Vulkan/SPIR-V VertexIndex built-in. Keeping the smoke
    // shader vertex-buffer-free makes the first native draw path intentionally small while still
    // exercising a non-degenerate graphics pipeline.
    //
    //   x = float((VertexIndex << 1) & 2) - 1
    //   y = float(VertexIndex & 2) - 1
    //
    // Produces (-1,-1), (1,-1), (-1,1) for vertices 0, 1 and 2.
    let mut b = Builder::new();
    let void = b.id();
    let u32_ty = b.id();
    let f32_ty = b.id();
    let vec4 = b.id();
    let in_u32_ptr = b.id();
    let out_vec4_ptr = b.id();
    let function_ty = b.id();
    let u32_one = b.id();
    let u32_two = b.id();
    let f32_zero = b.id();
    let f32_one = b.id();
    let vertex_index = b.id();
    let out_position = b.id();
    let main = b.id();
    let label = b.id();
    let index = b.id();
    let shifted = b.id();
    let x_bits = b.id();
    let y_bits = b.id();
    let x_float = b.id();
    let y_float = b.id();
    let x = b.id();
    let y = b.id();
    let position = b.id();

    b.inst(OP_CAPABILITY, &[1]); // Shader
    b.inst(OP_MEMORY_MODEL, &[0, 1]); // Logical, GLSL450
    b.inst_string(
        OP_ENTRY_POINT,
        &[0, main],
        "main",
        &[vertex_index, out_position],
    ); // Vertex
    b.inst(OP_DECORATE, &[vertex_index, 11, 42]); // BuiltIn VertexIndex
    b.inst(OP_DECORATE, &[out_position, 11, 0]); // BuiltIn Position

    b.inst(OP_TYPE_VOID, &[void]);
    b.inst(OP_TYPE_INT, &[u32_ty, 32, 0]);
    b.inst(OP_TYPE_FLOAT, &[f32_ty, 32]);
    b.inst(OP_TYPE_VECTOR, &[vec4, f32_ty, 4]);
    b.inst(OP_TYPE_POINTER, &[in_u32_ptr, 1, u32_ty]); // Input
    b.inst(OP_TYPE_POINTER, &[out_vec4_ptr, 3, vec4]); // Output
    b.inst(OP_TYPE_FUNCTION, &[function_ty, void]);
    b.inst(OP_CONSTANT, &[u32_ty, u32_one, 1]);
    b.inst(OP_CONSTANT, &[u32_ty, u32_two, 2]);
    b.inst(OP_CONSTANT, &[f32_ty, f32_zero, 0.0f32.to_bits()]);
    b.inst(OP_CONSTANT, &[f32_ty, f32_one, 1.0f32.to_bits()]);
    b.inst(OP_VARIABLE, &[in_u32_ptr, vertex_index, 1]);
    b.inst(OP_VARIABLE, &[out_vec4_ptr, out_position, 3]);

    b.inst(OP_FUNCTION, &[void, main, 0, function_ty]);
    b.inst(OP_LABEL, &[label]);
    b.inst(OP_LOAD, &[u32_ty, index, vertex_index]);
    b.inst(OP_SHIFT_LEFT_LOGICAL, &[u32_ty, shifted, index, u32_one]);
    b.inst(OP_BITWISE_AND, &[u32_ty, x_bits, shifted, u32_two]);
    b.inst(OP_BITWISE_AND, &[u32_ty, y_bits, index, u32_two]);
    b.inst(OP_CONVERT_U_TO_F, &[f32_ty, x_float, x_bits]);
    b.inst(OP_CONVERT_U_TO_F, &[f32_ty, y_float, y_bits]);
    b.inst(OP_FSUB, &[f32_ty, x, x_float, f32_one]);
    b.inst(OP_FSUB, &[f32_ty, y, y_float, f32_one]);
    b.inst(
        OP_COMPOSITE_CONSTRUCT,
        &[vec4, position, x, y, f32_zero, f32_one],
    );
    b.inst(OP_STORE, &[out_position, position]);
    b.inst(OP_RETURN, &[]);
    b.inst(OP_FUNCTION_END, &[]);
    b.finish()
}

fn build_simple_fragment(color: [f32; 4]) -> SpirvModule {
    let mut b = Builder::new();
    let void = b.id();
    let f32_ty = b.id();
    let vec4 = b.id();
    let out_vec4_ptr = b.id();
    let function_ty = b.id();
    let c0 = b.id();
    let c1 = b.id();
    let c2 = b.id();
    let c3 = b.id();
    let composite = b.id();
    let out_color = b.id();
    let main = b.id();
    let label = b.id();

    b.inst(OP_CAPABILITY, &[1]);
    b.inst(OP_MEMORY_MODEL, &[0, 1]);
    b.inst_string(OP_ENTRY_POINT, &[4, main], "main", &[out_color]); // Fragment
    b.inst(OP_EXECUTION_MODE, &[main, 7]); // OriginUpperLeft
    b.inst(OP_DECORATE, &[out_color, 30, 0]); // Location 0

    b.inst(OP_TYPE_VOID, &[void]);
    b.inst(OP_TYPE_FLOAT, &[f32_ty, 32]);
    b.inst(OP_TYPE_VECTOR, &[vec4, f32_ty, 4]);
    b.inst(OP_TYPE_POINTER, &[out_vec4_ptr, 3, vec4]);
    b.inst(OP_TYPE_FUNCTION, &[function_ty, void]);
    for (id, value) in [
        (c0, color[0]),
        (c1, color[1]),
        (c2, color[2]),
        (c3, color[3]),
    ] {
        b.inst(OP_CONSTANT, &[f32_ty, id, value.to_bits()]);
    }
    b.inst(OP_CONSTANT_COMPOSITE, &[vec4, composite, c0, c1, c2, c3]);
    b.inst(OP_VARIABLE, &[out_vec4_ptr, out_color, 3]);

    b.inst(OP_FUNCTION, &[void, main, 0, function_ty]);
    b.inst(OP_LABEL, &[label]);
    b.inst(OP_STORE, &[out_color, composite]);
    b.inst(OP_RETURN, &[]);
    b.inst(OP_FUNCTION_END, &[]);
    b.finish()
}

fn pack_string(value: &str) -> Vec<u32> {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(0);
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
    bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("chunk length")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_guest_shaders_never_fall_back_to_smoke_outputs() {
        for stage in [ShaderStage::Vertex, ShaderStage::Fragment] {
            for operations in [
                vec![],
                vec![ShaderOperation::Return],
                vec![
                    ShaderOperation::ConstantVec4 {
                        dst: 0,
                        value: [1.0; 4],
                    },
                    ShaderOperation::Return,
                ],
            ] {
                let shader = ShaderModule {
                    id: 1,
                    stage,
                    variant_key: b"triangle-vs".to_vec(),
                    operations,
                };
                assert_eq!(
                    compile(&shader),
                    Err(SpirvCompileError::UnsupportedOperation)
                );
            }
        }
        let fragment = ShaderModule {
            id: 2,
            stage: ShaderStage::Fragment,
            variant_key: Vec::new(),
            operations: vec![
                ShaderOperation::SmokeVertexTriangle,
                ShaderOperation::Return,
            ],
        };
        assert_eq!(
            compile(&fragment),
            Err(SpirvCompileError::UnsupportedOperation)
        );
    }

    #[test]
    fn constant_fragment_requires_explicit_complete_ordered_output() {
        let mut shader = ShaderModule {
            id: 3,
            stage: ShaderStage::Fragment,
            variant_key: Vec::new(),
            operations: vec![
                ShaderOperation::ConstantBits32 {
                    dst: 7,
                    value: 0x80000000,
                },
                ShaderOperation::Move { dst: 8, src: 7 },
            ],
        };
        for component in 0..4 {
            shader.operations.push(ShaderOperation::StoreOutput {
                location: 0,
                component,
                src: 8,
            });
        }
        shader.operations.push(ShaderOperation::Return);
        let compiled = compile(&shader).unwrap();
        assert_eq!(compiled, build_simple_fragment([-0.0; 4]));
        let mut overwritten = shader.clone();
        overwritten.operations.insert(
            3,
            ShaderOperation::ConstantBits32 {
                dst: 8,
                value: 0x7fa12345,
            },
        );
        assert_eq!(
            compile(&overwritten).unwrap(),
            build_simple_fragment([
                -0.0,
                f32::from_bits(0x7fa12345),
                f32::from_bits(0x7fa12345),
                f32::from_bits(0x7fa12345),
            ])
        );
        for index in [0, 1, 2, 3, 4, 5, 6] {
            let mut invalid = shader.clone();
            invalid.operations.remove(index);
            assert_eq!(
                compile(&invalid),
                Err(SpirvCompileError::UnsupportedOperation)
            );
        }
        for operation in [
            ShaderOperation::StoreOutput {
                location: 1,
                component: 0,
                src: 8,
            },
            ShaderOperation::StoreOutput {
                location: 0,
                component: 4,
                src: 8,
            },
            ShaderOperation::ConstantU32 { dst: 8, value: 1 },
        ] {
            let mut invalid = shader.clone();
            invalid.operations.insert(2, operation);
            assert_eq!(
                compile(&invalid),
                Err(SpirvCompileError::UnsupportedOperation)
            );
        }
        shader
            .operations
            .push(ShaderOperation::ConstantF32 { dst: 9, value: 1.0 });
        assert_eq!(
            compile(&shader),
            Err(SpirvCompileError::UnsupportedOperation)
        );
    }

    #[test]
    fn emits_vertex_and_fragment_headers() {
        let vertex = ShaderModule {
            id: 1,
            stage: ShaderStage::Vertex,
            variant_key: Vec::new(),
            operations: vec![
                ShaderOperation::SmokeVertexTriangle,
                ShaderOperation::Return,
            ],
        };
        let fragment = ShaderModule {
            id: 2,
            stage: ShaderStage::Fragment,
            variant_key: Vec::new(),
            operations: vec![
                ShaderOperation::ConstantF32 {
                    dst: 0,
                    value: 0.25,
                },
                ShaderOperation::StoreOutput {
                    location: 0,
                    component: 0,
                    src: 0,
                },
                ShaderOperation::StoreOutput {
                    location: 0,
                    component: 1,
                    src: 0,
                },
                ShaderOperation::StoreOutput {
                    location: 0,
                    component: 2,
                    src: 0,
                },
                ShaderOperation::StoreOutput {
                    location: 0,
                    component: 3,
                    src: 0,
                },
                ShaderOperation::Return,
            ],
        };
        let vertex = compile(&vertex).unwrap();
        let fragment = compile(&fragment).unwrap();
        assert_eq!(vertex.words[0], SpirvModule::MAGIC);
        assert_eq!(fragment.words[0], SpirvModule::MAGIC);
        assert!(vertex.words.len() > 20);
        assert!(fragment.words.len() > 20);
        // Vertex smoke shader must reference the VertexIndex built-in and dynamic arithmetic,
        // otherwise three vkCmdDraw vertices could collapse to the same position.
        assert!(
            vertex
                .words
                .windows(4)
                .any(|w| w[0] >> 16 == 4 && (w[0] & 0xffff) == OP_DECORATE as u32 && w[3] == 42)
        );
        assert!(
            vertex
                .words
                .iter()
                .any(|word| (word & 0xffff) == OP_COMPOSITE_CONSTRUCT as u32)
        );
    }
}
