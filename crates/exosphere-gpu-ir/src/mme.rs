// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Maxwell Macro Microcode Engine program extraction.
//!
//! Exosphere currently treats MME words as a separate ISA. Calls are resolved to their uploaded
//! start address and a bounded word slice; semantic execution can be replaced independently without
//! changing MAXWELL_B state tracking.

use crate::maxwell_state::MmeState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmeProgram {
    pub macro_index: u16,
    pub start_word: u32,
    pub words: Vec<u32>,
}

pub fn extract_program(state: &MmeState, index: u16, max_words: usize) -> Option<MmeProgram> {
    let start = *state.start_addresses.get(index as usize)? as usize;
    if start >= state.instruction_ram.len() {
        return None;
    }
    let end = (start.saturating_add(max_words)).min(state.instruction_ram.len());
    Some(MmeProgram {
        macro_index: index,
        start_word: start as u32,
        words: state.instruction_ram[start..end].to_vec(),
    })
}
