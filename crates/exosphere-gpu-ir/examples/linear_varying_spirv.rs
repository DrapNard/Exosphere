// SPDX-License-Identifier: GPL-3.0-or-later
//! Emit a linked scalar varying diagnostic for spirv-val, not a game frame.
use exosphere_gpu_ir::{
    ShaderModule, ShaderOperation as S, ShaderStage, shader::InterpolationMode, spirv,
};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stage = match std::env::args().nth(1).as_deref() {
        Some("vertex") => ShaderStage::Vertex,
        Some("fragment") => ShaderStage::Fragment,
        _ => return Err("usage: linear_varying_spirv <vertex|fragment>".into()),
    };
    let operations = if stage == ShaderStage::Vertex {
        vec![
            S::ConstantF32 {
                dst: 0,
                value: 0.25,
            },
            S::ConstantF32 { dst: 1, value: 1.0 },
            S::ComposeVec4 {
                dst: 2,
                components: [0, 0, 0, 1],
            },
            S::StoreVarying {
                location: 3,
                component: 2,
                src: 0,
            },
            S::StoreBuiltin { builtin: 0, src: 2 },
            S::Return,
        ]
    } else {
        let mut operations = vec![S::Interpolate {
            dst: 0,
            attribute: 0xb8,
            component: 2,
            mode: InterpolationMode::Pass,
            offset: None,
        }];
        operations.extend((0..4).map(|component| S::StoreOutput {
            location: 0,
            component,
            src: 0,
        }));
        operations.push(S::Return);
        operations
    };
    let shader = ShaderModule {
        id: 1,
        stage,
        variant_key: vec![],
        operations,
    };
    std::io::stdout()
        .lock()
        .write_all(&spirv::compile(&shader)?.bytes_le())?;
    Ok(())
}
