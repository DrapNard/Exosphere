// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Shader entry-point discovery from MAXWELL_B pipeline slots.
//!
//! `SetPipelineProgram` stores a program offset relative to the current Program Region. Exosphere
//! keeps discovery separate from parsing because the register stream does not provide a universal
//! byte length for every shader program.

use crate::{
    maxwell_state::{DrawStateSnapshot, Maxwell3dState, PipelineShaderState, method},
    shader::ShaderStage,
    shader_memory::GpuMemoryError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveredShader {
    pub slot: u8,
    pub stage: ShaderStage,
    pub gpu_address: u64,
    pub register_count: u32,
    pub binding: u32,
    pub enabled_raw: u32,
}

pub fn discover_graphics_shaders(
    state: &Maxwell3dState,
) -> Result<Vec<DiscoveredShader>, GpuMemoryError> {
    discover_slots(state.program_region.high_low(), &state.pipeline_shaders)
}

/// Discover using the registers captured for this draw, not the final channel state.
pub fn discover_draw_shaders(
    state: &DrawStateSnapshot,
) -> Result<Vec<DiscoveredShader>, GpuMemoryError> {
    let register = |method| state.raw_registers.get(&method).copied().unwrap_or(0);
    let base = (u64::from(register(method::SET_PROGRAM_REGION_A)) << 32)
        | u64::from(register(method::SET_PROGRAM_REGION_B));
    let slots = std::array::from_fn::<_, { method::PIPELINE_COUNT }, _>(|slot| {
        let base = method::PIPELINE_BASE + slot as u16 * method::PIPELINE_STRIDE;
        PipelineShaderState {
            select: register(base),
            program: register(base + 1),
            register_count: register(base + 3),
            binding: register(base + 4),
        }
    });
    discover_slots(base, &slots)
}

fn discover_slots(
    base: u64,
    slots: &[PipelineShaderState],
) -> Result<Vec<DiscoveredShader>, GpuMemoryError> {
    slots
        .iter()
        .enumerate()
        .filter_map(|(slot, p)| {
            // NVB197_SET_PIPELINE_SHADER: enable is bit 0, type is bits 7:4.
            // Disabling a stage does not clear its old program/binding registers.
            if p.select & 1 == 0 {
                return None;
            }
            let invalid = || GpuMemoryError::InvalidShaderConfiguration {
                slot: slot as u8,
                select: p.select,
            };
            if p.select & !0xf1 != 0 {
                return Some(Err(invalid()));
            }
            let stage = match (p.select >> 4) & 0xf {
                0 | 1 => ShaderStage::Vertex,
                2 => ShaderStage::TessControl,
                3 => ShaderStage::TessEvaluation,
                4 => ShaderStage::Geometry,
                5 => ShaderStage::Fragment,
                _ => return Some(Err(invalid())),
            };
            Some(
                base.checked_add(u64::from(p.program))
                    .ok_or(GpuMemoryError::AddressOverflow)
                    .map(|gpu_address| DiscoveredShader {
                        slot: slot as u8,
                        stage,
                        gpu_address,
                        register_count: p.register_count,
                        binding: p.binding,
                        enabled_raw: p.select,
                    }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enable_and_type_fields_control_discovery_not_stale_registers() {
        let mut state = Maxwell3dState::default();
        state.program_region.a = u32::MAX;
        state.program_region.b = u32::MAX;
        state.pipeline_shaders[5] = PipelineShaderState {
            select: 0x50,
            program: 0x200,
            register_count: 32,
            binding: 3,
        };
        // A disabled stale offset must not even participate in address arithmetic.
        assert!(discover_graphics_shaders(&state).unwrap().is_empty());
        state.program_region = Default::default();
        for (kind, stage) in [
            (0, ShaderStage::Vertex),
            (1, ShaderStage::Vertex),
            (2, ShaderStage::TessControl),
            (3, ShaderStage::TessEvaluation),
            (4, ShaderStage::Geometry),
            (5, ShaderStage::Fragment),
        ] {
            state.pipeline_shaders[5].select = (kind << 4) | 1;
            let discovered = discover_graphics_shaders(&state).unwrap();
            assert_eq!(discovered.len(), 1);
            assert_eq!(discovered[0].stage, stage);
            assert_eq!(discovered[0].slot, 5);
            assert_eq!(discovered[0].gpu_address, 0x200);
        }
        for select in [0x61, 0xf1, 0x53, 0x151, 0x80000051] {
            state.pipeline_shaders[5].select = select;
            assert_eq!(
                discover_graphics_shaders(&state),
                Err(GpuMemoryError::InvalidShaderConfiguration { slot: 5, select })
            );
        }
    }

    #[test]
    fn resolves_pipeline_program_against_region() {
        let mut state = Maxwell3dState::default();
        state.program_region.a = 1;
        state.program_region.b = 0;
        state.pipeline_shaders[5].select = 0x51;
        state.pipeline_shaders[5].program = 0x200;
        let found = discover_graphics_shaders(&state).unwrap();
        assert_eq!(found[0].stage, ShaderStage::Fragment);
        assert_eq!(found[0].gpu_address, (1u64 << 32) + 0x200);
        state.program_region.a = u32::MAX;
        state.program_region.b = u32::MAX;
        state.pipeline_shaders[5].program = 1;
        assert_eq!(
            discover_graphics_shaders(&state),
            Err(GpuMemoryError::AddressOverflow)
        );
    }

    #[test]
    fn discovers_and_selects_compile_windows_for_each_draw_snapshot() {
        use crate::{
            engine::{EngineClass, EngineMethodWrite},
            maxwell_state::MaxwellEvent,
        };
        let mut state = Maxwell3dState::default();
        let mut write = |method, argument| {
            state.apply(EngineMethodWrite {
                subchannel: 0,
                class: EngineClass::Graphics,
                method,
                argument,
            })
        };
        let slot = method::PIPELINE_BASE + 5 * method::PIPELINE_STRIDE;
        write(method::SET_PROGRAM_REGION_B, 0x1000);
        write(slot, 0x51);
        write(slot + 1, 0x80);
        write(slot + 3, 12);
        write(slot + 4, 3);
        let first = write(method::DRAW_VERTEX_ARRAY, 3).unwrap();
        write(method::SET_PROGRAM_REGION_B, 0x2000);
        write(slot + 1, 0x100);
        let second = write(method::DRAW_INDEX_BUFFER, 9).unwrap();
        let MaxwellEvent::DrawVertexArray { state: first, .. } = first else {
            unreachable!()
        };
        let MaxwellEvent::DrawIndexBuffer { state: second, .. } = second else {
            unreachable!()
        };
        let first_entry = discover_draw_shaders(&first).unwrap();
        let second_entry = discover_draw_shaders(&second).unwrap();
        assert_eq!(first_entry.len(), 1);
        assert_eq!(first_entry[0].gpu_address, 0x1080);
        assert_eq!(second_entry[0].gpu_address, 0x2100);
        assert_eq!(first_entry[0].stage, ShaderStage::Fragment);
        assert_eq!(first_entry[0].register_count, 12);
        assert_eq!(first_entry[0].binding, 3);
        let memory = crate::shader_memory::LinearGpuMemory::new(0, vec![]);
        assert!(matches!(
            crate::compiler::compile_draw_shaders(&memory, &first, &[]),
            Err(crate::compiler::GpuCompileError::MissingWindow(0x1080))
        ));
        assert!(matches!(
            crate::compiler::compile_draw_shaders(&memory, &second, &[]),
            Err(crate::compiler::GpuCompileError::MissingWindow(0x2100))
        ));
    }
}
