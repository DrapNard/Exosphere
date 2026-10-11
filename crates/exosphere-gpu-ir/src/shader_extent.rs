// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded graphics program extents from supported reachable control flow.

use crate::{
    compiler::ShaderWindow,
    maxwell_semantics::{SemanticDisposition, decode_semantic},
    maxwell_shader::{
        GRAPHICS_HEADER_SIZE, MaxwellInstruction, MaxwellOpcodeFamily, classify_instruction,
    },
    shader_memory::{GpuMemory, GpuMemoryError},
};

#[derive(Debug)]
pub enum ShaderExtentError {
    InvalidBudget,
    Memory(GpuMemoryError),
    UnexpectedReadSize { expected: usize, actual: usize },
    UnsupportedControlFlow { offset: u32 },
    InvalidBranchTarget { offset: u32, target: u32 },
    MissingTermination,
}

impl std::fmt::Display for ShaderExtentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "graphics shader extent discovery failed: {self:?}")
    }
}

impl std::error::Error for ShaderExtentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Memory(error) => Some(error),
            _ => None,
        }
    }
}

/// The budget bounds inspection, not the shader's inferred allocation size.
/// Branches/calls/reconvergence and unknown opcodes require a separate CFG walk.
/// A conditional EXIT cannot terminate the window while other lanes fall through.
pub fn discover_linear_graphics_window(
    memory: &dyn GpuMemory,
    address: u64,
    maximum_bytes: usize,
) -> Result<ShaderWindow, ShaderExtentError> {
    discover_window(memory, address, maximum_bytes, false)
}

/// Follow certified direct PC-relative BRA edges and predicate fallthrough.
/// Calls, indirect targets and reconvergence still require a broader CFG model.
/// Cycles are inspected once, not executed. At least one reachable EXIT is required;
/// this certifies an extent, not program termination or executable SPIR-V support.
pub fn discover_direct_graphics_window(
    memory: &dyn GpuMemory,
    address: u64,
    maximum_bytes: usize,
) -> Result<ShaderWindow, ShaderExtentError> {
    discover_window(memory, address, maximum_bytes, true)
}

fn discover_window(
    memory: &dyn GpuMemory,
    address: u64,
    maximum_bytes: usize,
    direct_branches: bool,
) -> Result<ShaderWindow, ShaderExtentError> {
    if !(GRAPHICS_HEADER_SIZE + 16..=64 * 1024 * 1024).contains(&maximum_bytes) {
        return Err(ShaderExtentError::InvalidBudget);
    }
    address
        .checked_add(maximum_bytes as u64)
        .ok_or(ShaderExtentError::Memory(GpuMemoryError::AddressOverflow))?;
    let read = |offset: usize, size: usize| {
        let bytes = memory
            .read(address + offset as u64, size)
            .map_err(ShaderExtentError::Memory)?;
        if bytes.len() != size {
            return Err(ShaderExtentError::UnexpectedReadSize {
                expected: size,
                actual: bytes.len(),
            });
        }
        Ok(bytes)
    };
    read(0, GRAPHICS_HEADER_SIZE)?;
    let mut pending = vec![GRAPHICS_HEADER_SIZE + 8];
    let mut visited = std::collections::BTreeSet::new();
    let mut extent = GRAPHICS_HEADER_SIZE;
    let mut terminated = false;
    while let Some(offset) = pending.pop() {
        if !visited.insert(offset) {
            continue;
        }
        if offset + 8 > maximum_bytes {
            return Err(ShaderExtentError::MissingTermination);
        }
        let scheduler = GRAPHICS_HEADER_SIZE + (offset - GRAPHICS_HEADER_SIZE) / 32 * 32;
        read(scheduler, 8)?;
        let bytes = read(offset, 8)?;
        extent = extent.max(offset + 8);
        let instruction = MaxwellInstruction {
            offset: offset as u32,
            raw: u64::from_le_bytes(bytes.try_into().expect("checked word length")),
        };
        let decoded = classify_instruction(instruction);
        let semantic = decode_semantic(instruction);
        // The structural opcode table is incomplete for constant MOV, MOV32I
        // and non-register I2F/IPA/TEX. Admit certified non-control-flow forms only.
        let certified_move = semantic.disposition == SemanticDisposition::Ready
            && matches!(
                semantic.opcode,
                Some(
                    crate::maxwell_semantics::MaxwellSemanticOpcode::Mov
                        | crate::maxwell_semantics::MaxwellSemanticOpcode::Mov32I
                )
            );
        let certified_data = matches!(
            semantic.opcode,
            Some(
                crate::maxwell_semantics::MaxwellSemanticOpcode::I2F
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Interpolate
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Tex
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::IAdd
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::IAdd32I
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::IMul
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::IMul32I
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Lop
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Lop32I
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Shl
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Shr
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Bfe
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Popc
                    | crate::maxwell_semantics::MaxwellSemanticOpcode::Flo
            )
        ) && crate::maxwell_semantics::lower_semantic(&semantic).lossless;
        if decoded.opcode == crate::maxwell_shader::MaxwellOpcode::Unknown
            && !certified_move
            && !certified_data
        {
            return Err(ShaderExtentError::UnsupportedControlFlow {
                offset: instruction.offset,
            });
        }
        let mut fallthrough = true;
        match decoded.family {
            MaxwellOpcodeFamily::Branch
                if direct_branches
                    && decoded.opcode == crate::maxwell_shader::MaxwellOpcode::Bra =>
            {
                // Only the ordinary always-condition BRA form is certified.
                // Relative displacement and the predicate guard are variable.
                if !crate::maxwell_semantics::valid_direct_branch_encoding(instruction.raw) {
                    return Err(ShaderExtentError::UnsupportedControlFlow {
                        offset: instruction.offset,
                    });
                }
                let target = decoded.branch_target.expect("direct BRA target");
                if target < (GRAPHICS_HEADER_SIZE + 8) as u32
                    || target as usize + 8 > maximum_bytes
                    || (target as usize - GRAPHICS_HEADER_SIZE) % 8 != 0
                    || (target as usize - GRAPHICS_HEADER_SIZE) % 32 == 0
                {
                    return Err(ShaderExtentError::InvalidBranchTarget {
                        offset: instruction.offset,
                        target,
                    });
                }
                pending.push(target as usize);
                fallthrough = semantic.predicate.is_some();
            }
            MaxwellOpcodeFamily::Branch
            | MaxwellOpcodeFamily::CallReturn
            | MaxwellOpcodeFamily::Synchronize => {
                return Err(ShaderExtentError::UnsupportedControlFlow {
                    offset: instruction.offset,
                });
            }
            MaxwellOpcodeFamily::Exit => {
                if semantic.disposition != SemanticDisposition::Ready {
                    return Err(ShaderExtentError::UnsupportedControlFlow {
                        offset: instruction.offset,
                    });
                }
                terminated = true;
                if semantic.predicate.is_none() {
                    fallthrough = false;
                }
            }
            MaxwellOpcodeFamily::Unknown if !certified_move && !certified_data => {
                return Err(ShaderExtentError::UnsupportedControlFlow {
                    offset: instruction.offset,
                });
            }
            _ => {}
        }
        if fallthrough {
            let next = offset + 8;
            // Sequential execution skips the next bundle's scheduling word.
            pending.push(if (next - GRAPHICS_HEADER_SIZE) % 32 == 0 {
                next + 8
            } else {
                next
            });
        }
    }
    if !terminated {
        return Err(ShaderExtentError::MissingTermination);
    }
    // The compiler consumes a contiguous prefix, including unreachable gaps.
    // Require that complete prefix to be readable; CFG admission does not certify
    // that the current linear lifter can execute every program in that prefix.
    read(0, extent)?;
    Ok(ShaderWindow {
        gpu_address: address,
        bytes: extent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader_memory::LinearGpuMemory;

    fn memory(words: &[u64]) -> LinearGpuMemory {
        let mut bytes = vec![0; GRAPHICS_HEADER_SIZE];
        bytes.extend(words.iter().flat_map(|word| word.to_le_bytes()));
        LinearGpuMemory::new(0x1000, bytes)
    }

    const EXIT: u64 = 0xe30000000007000f;

    #[test]
    fn certified_2d_tex_extent_keeps_discovery_separate_from_execution_support() {
        let raw =
            0xc038000000000000u64 | (255 << 20) | (1 << 29) | (7 << 16) | (15 << 31) | (8 << 8);
        for discover in [
            discover_linear_graphics_window,
            discover_direct_graphics_window,
        ] {
            for lod in [0, 1] {
                assert_eq!(
                    discover(&memory(&[0, raw | (lod << 55), EXIT]), 0x1000, 104)
                        .unwrap()
                        .bytes,
                    104
                );
            }
            for invalid in [raw | (1 << 28), raw | (2 << 55), raw | (1 << 54)] {
                // A recognized texture instruction can have known fallthrough
                // without executable semantics. Extent discovery is not a
                // promise that the instruction can be emitted as SPIR-V.
                assert!(
                    !crate::maxwell_semantics::lower_semantic(&decode_semantic(
                        MaxwellInstruction {
                            offset: 88,
                            raw: invalid
                        }
                    ))
                    .lossless
                );
            }
        }
    }

    #[test]
    fn certified_i2f_sources_have_fallthrough_without_admitting_reserved_fields() {
        for opcode in [
            0x5cb8000000000000u64,
            0x4cb8000000000000,
            0x38b8000000000000,
            0x39b8000000000000,
        ] {
            let raw = opcode | (2 << 8) | (2 << 10) | (7 << 16) | 9;
            for rounding in 0..4u64 {
                let guest = memory(&[0, raw | (rounding << 39), EXIT]);
                for direct in [false, true] {
                    assert_eq!(
                        discover_window(&guest, 0x1000, 104, direct)
                            .unwrap_or_else(|error| panic!("opcode {opcode:016x}, rounding {rounding}, direct {direct}: {error:?}"))
                            .bytes,
                        104
                    );
                }
            }
            // For structurally unknown source forms, semantic certification
            // must reject condition-code effects and reserved opcode bits.
            if opcode != 0x5cb8000000000000 {
                for bit in [12, 47, 48, 50] {
                    assert!(matches!(
                        discover_direct_graphics_window(
                            &memory(&[0, raw | (1 << bit), EXIT]),
                            0x1000,
                            104
                        ),
                        Err(ShaderExtentError::UnsupportedControlFlow { offset: 88 })
                    ));
                }
            }
        }
    }

    fn branch(from: u32, target: u32, conditional: bool) -> u64 {
        let relative = (i64::from(target) - i64::from(from) - 8) as u64 & 0xffffff;
        0xe24000000000000f | (relative << 20) | (if conditional { 0 } else { 7 } << 16)
    }

    #[test]
    fn direct_edges_include_targets_beyond_an_earlier_exit_and_handle_cycles() {
        let guest = memory(&[0, branch(88, 120, true), EXIT, 0, 0, EXIT]);
        assert_eq!(
            discover_direct_graphics_window(&guest, 0x1000, 128)
                .unwrap()
                .bytes,
            128
        );
        let guest = memory(&[0, branch(88, 88, true), EXIT]);
        assert_eq!(
            discover_direct_graphics_window(&guest, 0x1000, 104)
                .unwrap()
                .bytes,
            104
        );
        assert!(matches!(
            discover_direct_graphics_window(&memory(&[0, branch(88, 88, false)]), 0x1000, 104),
            Err(ShaderExtentError::MissingTermination)
        ));
    }

    #[test]
    fn branch_targets_must_be_in_budget_aligned_instructions() {
        for target in [0, 80, 89, 112, 128, u32::MAX] {
            assert!(matches!(
                discover_direct_graphics_window(
                    &memory(&[0, branch(88, target, false), EXIT]),
                    0x1000,
                    128
                ),
                Err(ShaderExtentError::InvalidBranchTarget { .. })
            ));
        }
    }

    #[test]
    fn scheduler_and_conditional_exit_do_not_end_the_program() {
        let guest = memory(&[EXIT, EXIT & !(7 << 16), 0, EXIT, u64::MAX]);
        let window = discover_linear_graphics_window(&guest, 0x1000, 120).unwrap();
        assert_eq!(window.gpu_address, 0x1000);
        assert_eq!(window.bytes, 112);
        // Unreachable trailing garbage is not inspected or admitted as shader code.
    }

    #[test]
    fn control_flow_invalid_exit_missing_termination_and_unmapped_reads_reject() {
        for raw in [0xe24000000007000f, EXIT ^ 1, u64::MAX] {
            assert!(matches!(
                discover_linear_graphics_window(&memory(&[0, raw, EXIT]), 0x1000, 104),
                Err(ShaderExtentError::UnsupportedControlFlow { offset: 88 })
            ));
        }
        assert!(matches!(
            discover_linear_graphics_window(&memory(&[0, 0, 0]), 0x1000, 104),
            Err(ShaderExtentError::MissingTermination)
        ));
        assert!(matches!(
            discover_linear_graphics_window(&memory(&[0]), 0x1000, 104),
            Err(ShaderExtentError::Memory(GpuMemoryError::Unmapped { .. }))
        ));
        assert!(matches!(
            discover_linear_graphics_window(&memory(&[]), 0x1000, 80),
            Err(ShaderExtentError::InvalidBudget)
        ));
    }
}
