// SPDX-License-Identifier: GPL-3.0-or-later
//! Emit a synthetic encoded Maxwell arithmetic vertex shader for spirv-val.
//! This is a compiler diagnostic, not an extracted game shader or substitute frame.

use exosphere_gpu_ir::{ShaderStage, VertexFormat, maxwell_shader, maxwell_vertex, spirv};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let argument = std::env::args().nth(1);
    let constant = argument.as_deref() == Some("--constant");
    let modifiers = match argument.as_deref() {
        None => 0,
        Some("--constant") => 0,
        Some("--modifiers") => (1u64 << 53) | (1 << 54) | (1 << 56) | (1 << 57),
        _ => return Err("usage: vertex_float_spirv [--modifiers|--constant]".into()),
    };
    let operands = (3u64 << 47) | (255 << 39) | (7 << 16) | (255 << 8) | 7;
    let mut bytes = vec![0; maxwell_shader::GRAPHICS_HEADER_SIZE];
    bytes.extend(
        [
            0u64,
            0xefd8000000000000 | operands | (0xa0 << 20),
            // Dynamic X + 1 and Y * 0.5, with explicit FTZ controls.
            if constant {
                // FADD R7, R7, c[3][28], FTZ: offset encoded in four-byte words.
                0x4c58000000000000 | (3 << 34) | (7 << 20) | (1 << 44) | (7 << 16) | (7 << 8) | 7
            } else {
                0x0800000000000000
                    | (0x3f800000u64 << 20)
                    | (1 << 55)
                    | modifiers
                    | (7 << 16)
                    | (7 << 8)
                    | 7
            },
            0x1e00000000000000 | (0x3f000000u64 << 20) | (1 << 53) | (7 << 16) | (8 << 8) | 8,
            0,
            0xeff0000000000000 | operands | (0x70 << 20),
            0xe30000000007000f,
        ]
        .into_iter()
        .flat_map(u64::to_le_bytes),
    );
    let program = maxwell_shader::parse_graphics(ShaderStage::Vertex, &bytes)?;
    let (shader, stats) = maxwell_shader::lift_semantic(1, &program);
    if stats.opaque != 0 {
        return Err("diagnostic shader contains opaque instructions".into());
    }
    let shader = if constant {
        if !shader.operations.iter().any(|operation| {
            matches!(
                operation,
                exosphere_gpu_ir::ShaderOperation::LoadConstantBuffer {
                    slot: 3,
                    offset: 28,
                    address: None,
                    ..
                }
            )
        }) {
            return Err("constant-buffer instruction did not retain its memory operand".into());
        }
        exosphere_gpu_ir::maxwell_constant::lower_constant_interface(
            &shader,
            &[
                exosphere_gpu_ir::maxwell_constant::ConstantBufferInterface {
                    slot: 3,
                    set: 2,
                    binding: 7,
                    byte_size: 32,
                },
            ],
        )?
    } else {
        shader
    };
    let shader = maxwell_vertex::lower_vertex_interface(
        &shader,
        &[maxwell_vertex::VertexAttributeInterface {
            slot: 2,
            location: 2,
            format: VertexFormat::Rgba32Float,
        }],
    )?;
    std::io::stdout()
        .lock()
        .write_all(&spirv::compile(&shader)?.bytes_le())?;
    Ok(())
}
