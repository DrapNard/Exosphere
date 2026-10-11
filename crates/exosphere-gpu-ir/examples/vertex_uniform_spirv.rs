// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic uniform-load diagnostic for spirv-val; not a game shader or frame.

use exosphere_gpu_ir::{ShaderModule, ShaderOperation, ShaderStage, spirv};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fragment = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--fragment") => true,
        _ => return Err("usage: vertex_uniform_spirv [--fragment]".into()),
    };
    let mut operations = Vec::new();
    for component in 0..4 {
        operations.push(ShaderOperation::LoadUniformWord {
            dst: component,
            set: 2,
            binding: 7,
            byte_offset: 16 + component * 4,
            byte_size: 32,
        });
        operations.push(if fragment {
            ShaderOperation::StoreOutput {
                location: 0,
                component: component as u8,
                src: component,
            }
        } else {
            ShaderOperation::StoreBuiltinComponent {
                builtin: 0,
                component: component as u8,
                src: component,
            }
        });
    }
    operations.push(ShaderOperation::Return);
    let shader = ShaderModule {
        id: 1,
        stage: if fragment {
            ShaderStage::Fragment
        } else {
            ShaderStage::Vertex
        },
        variant_key: vec![],
        operations,
    };
    std::io::stdout()
        .lock()
        .write_all(&spirv::compile(&shader)?.bytes_le())?;
    Ok(())
}
