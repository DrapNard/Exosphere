// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! KEPLER_INLINE_TO_MEMORY_B state tracking.
//!
//! This engine is commonly used to upload small host-provided words into GPU-visible memory.

use crate::{
    engine::{EngineClass, EngineMethodWrite},
    maxwell_state::RawGpuAddress,
};

pub mod method {
    pub const LINE_LENGTH_IN: u16 = 0x60;
    pub const LINE_COUNT: u16 = 0x61;
    pub const OFFSET_OUT_UPPER: u16 = 0x62;
    pub const OFFSET_OUT_LOWER: u16 = 0x63;
    pub const PITCH_OUT: u16 = 0x64;
    pub const LAUNCH_DMA: u16 = 0x6C;
    pub const LOAD_INLINE_DATA: u16 = 0x6D;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineToMemoryState {
    pub destination: RawGpuAddress,
    pub pitch_out: u32,
    pub line_length: u32,
    pub line_count: u32,
    pub launch_flags: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineToMemoryEvent {
    Launch { state: InlineToMemoryState },
    Data { value: u32 },
    Opaque { method: u16, argument: u32 },
}

impl InlineToMemoryState {
    pub fn apply(&mut self, write: EngineMethodWrite) -> Option<InlineToMemoryEvent> {
        if write.class != EngineClass::InlineToMemory {
            return None;
        }
        use method::*;
        match write.method {
            LINE_LENGTH_IN => {
                self.line_length = write.argument;
                None
            }
            LINE_COUNT => {
                self.line_count = write.argument;
                None
            }
            OFFSET_OUT_UPPER => {
                self.destination.a = write.argument;
                None
            }
            OFFSET_OUT_LOWER => {
                self.destination.b = write.argument;
                None
            }
            PITCH_OUT => {
                self.pitch_out = write.argument;
                None
            }
            LAUNCH_DMA => {
                self.launch_flags = write.argument;
                Some(InlineToMemoryEvent::Launch {
                    state: self.clone(),
                })
            }
            LOAD_INLINE_DATA => {
                // Payload belongs to the ordered event stream. Retaining it in
                // channel state would duplicate data and grow across pushbuffers.
                Some(InlineToMemoryEvent::Data {
                    value: write.argument,
                })
            }
            _ => Some(InlineToMemoryEvent::Opaque {
                method: write.method,
                argument: write.argument,
            }),
        }
    }
}
