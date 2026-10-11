// SPDX-License-Identifier: GPL-3.0-or-later
//! Test-only execution of the straight-line integer SPIR-V subset we emit.
//! This is not a SPIR-V validator. Native Vulkan tests remain independent gates.

use std::collections::BTreeMap;

/// Evaluate every instruction, including unselected OpSelect operands. Illegal
/// shift counts fail instead of hiding undefined behavior behind a selection.
pub(super) fn evaluate(words: &[u32], source: u32, input: u32, result: u32) -> u32 {
    let mut values = BTreeMap::from([(source, input)]);
    let mut cursor = 5;
    while cursor < words.len() {
        let length = (words[cursor] >> 16) as usize;
        let opcode = words[cursor] & 0xffff;
        let operands = &words[cursor + 1..cursor + length];
        if opcode == 43 {
            values.insert(operands[1], operands[2]);
        } else if !matches!(opcode, 20 | 21) {
            let a = values[&operands[2]];
            let value = match opcode {
                200 => !a,
                205 => a.count_ones(),
                _ => {
                    let c = values[&operands[3]];
                    match opcode {
                        128 => a.wrapping_add(c),
                        130 => a.wrapping_sub(c),
                        166 => u32::from(a != 0 || c != 0),
                        167 => u32::from(a != 0 && c != 0),
                        169 => {
                            if a != 0 {
                                c
                            } else {
                                values[&operands[4]]
                            }
                        }
                        170 => u32::from(a == c),
                        171 => u32::from(a != c),
                        172 => u32::from(a > c),
                        194..=196 => {
                            assert!(c < 32);
                            match opcode {
                                194 => a >> c,
                                195 => ((a as i32) >> c) as u32,
                                _ => a << c,
                            }
                        }
                        197 => a | c,
                        198 => a ^ c,
                        199 => a & c,
                        _ => panic!("unexpected integer opcode {opcode}"),
                    }
                }
            };
            values.insert(operands[1], value);
        }
        cursor += length;
    }
    values[&result]
}
