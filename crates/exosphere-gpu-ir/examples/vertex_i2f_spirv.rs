// SPDX-License-Identifier: GPL-3.0-or-later
//! Encoded dynamic I2F diagnostic for spirv-val, not a game shader or frame.

use exosphere_gpu_ir::{ShaderStage, VertexFormat, maxwell_shader, maxwell_vertex, spirv};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let rounding: u64 = arguments
        .next()
        .ok_or("expected rounding encoding 0..3")?
        .parse()?;
    let signed: u64 = arguments
        .next()
        .ok_or("expected signed flag 0 or 1")?
        .parse()?;
    let integer_input = match arguments.next().as_deref() {
        None => false,
        Some("--integer-input") => true,
        _ => return Err("expected --integer-input or no further argument".into()),
    };
    if rounding > 3 || signed > 1 || arguments.next().is_some() {
        return Err(
            "usage: vertex_i2f_spirv <rounding:0..3> <signed:0|1> [--integer-input]".into(),
        );
    }
    let operands = (3u64 << 47) | (255 << 39) | (7 << 16) | (255 << 8) | 7;
    let conversion = 0x5cb8000000000000u64
        | (2 << 8)
        | (2 << 10)
        | (signed << 13)
        | (7 << 16)
        | (9 << 20)
        | 9
        | (rounding << 39);
    let mut bytes = vec![0; maxwell_shader::GRAPHICS_HEADER_SIZE];
    bytes.extend(
        [
            0u64,
            0xefd8000000000000 | operands | (0x80 << 20),
            conversion,
            0xeff0000000000000 | operands | (0x70 << 20),
            0,
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
    let shader = maxwell_vertex::lower_vertex_interface(
        &shader,
        &[maxwell_vertex::VertexAttributeInterface {
            slot: 0,
            location: 0,
            format: if integer_input {
                if signed != 0 {
                    VertexFormat::Rgba32Sint
                } else {
                    VertexFormat::Rgba32Uint
                }
            } else {
                VertexFormat::Rgba32Float
            },
        }],
    )?;
    std::io::stdout()
        .lock()
        .write_all(&spirv::compile(&shader)?.bytes_le())?;
    Ok(())
}
