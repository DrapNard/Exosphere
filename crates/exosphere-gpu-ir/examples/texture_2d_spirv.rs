// SPDX-License-Identifier: GPL-3.0-or-later
//! Emit a canonical 2D texture diagnostic for spirv-val, not a game frame.
use exosphere_gpu_ir::{ShaderModule, ShaderOperation as S, ShaderStage, spirv};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let level_zero = match std::env::args().nth(1).as_deref() {
        Some("implicit") => false,
        Some("zero") => true,
        _ => return Err("usage: texture_2d_spirv <implicit|zero> [constants]".into()),
    };
    use exosphere_gpu_ir::shader::TextureSampleChannel as C;
    let channels = if std::env::args().nth(2).as_deref() == Some("constants") {
        vec![C::Zero, C::One, C::Component(2), C::One]
    } else {
        [2, 0, 1, 3].map(C::Component).to_vec()
    };
    let mut operations = vec![
        S::ConstantF32 { dst: 7, value: 0.5 },
        S::SampleTexture2d {
            dst: vec![0, 1, 2, 3],
            image: [1, 4],
            sampler: [1, 5],
            coords: [7, 7],
            components: channels,
            level_zero,
        },
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
    std::io::stdout()
        .lock()
        .write_all(&spirv::compile(&shader)?.bytes_le())?;
    Ok(())
}
