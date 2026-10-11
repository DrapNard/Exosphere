// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Basic-block discovery for AArch64 text.
use crate::{BlockId, DecodedInstruction, DecodedOpcode};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone)]
pub struct DecodedBlock {
    pub id: BlockId,
    pub start: u64,
    pub instructions: Vec<DecodedInstruction>,
}
pub fn build(instructions: &[DecodedInstruction]) -> Vec<DecodedBlock> {
    if instructions.is_empty() {
        return vec![];
    }
    let min = instructions[0].guest_pc;
    let max = instructions.last().unwrap().guest_pc + 4;
    let mut starts = BTreeSet::from([min]);
    for i in instructions {
        match i.opcode {
            DecodedOpcode::BranchImm { target, link } => {
                if target >= min && target < max {
                    starts.insert(target);
                }
                if link {
                    starts.insert(i.guest_pc + 4);
                }
            }
            DecodedOpcode::CondBranch { target, .. }
            | DecodedOpcode::CompareBranch { target, .. }
            | DecodedOpcode::TestBranch { target, .. } => {
                if target >= min && target < max {
                    starts.insert(target);
                }
                starts.insert(i.guest_pc + 4);
            }
            DecodedOpcode::Ret { .. } | DecodedOpcode::BranchReg { .. } => {
                starts.insert(i.guest_pc + 4);
            }
            _ => {}
        }
    }
    let idmap: BTreeMap<u64, BlockId> = starts
        .iter()
        .enumerate()
        .map(|(i, &a)| (a, BlockId(i as u32)))
        .collect();
    let mut out = Vec::new();
    for (&start, &id) in &idmap {
        let end = starts.range((start + 1)..).next().copied().unwrap_or(max);
        let ins: Vec<DecodedInstruction> = instructions
            .iter()
            .filter(|i| i.guest_pc >= start && i.guest_pc < end)
            .cloned()
            .collect();
        if !ins.is_empty() {
            out.push(DecodedBlock {
                id,
                start,
                instructions: ins,
            })
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_text;
    #[test]
    fn splits_conditional_branch() {
        let words = [0x54000040u32, 0xd503201f, 0xd65f03c0]; // b.eq +8, nop, ret
        let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        let blocks = build(&decode_text(&bytes, 0x1000));
        assert!(blocks.len() >= 2);
    }
}
