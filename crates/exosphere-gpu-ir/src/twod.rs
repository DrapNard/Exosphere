// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! FERMI_TWOD_A state tracking.
//!
//! Address/format/geometry registers are typed; the full raw register map is retained as a lossless
//! escape hatch for operations whose trigger semantics are still under validation.

use crate::{
    engine::{EngineClass, EngineMethodWrite},
    maxwell_state::RawGpuAddress,
};
use std::collections::BTreeMap;

pub mod method {
    pub const DST_FORMAT: u16 = 0x80;
    pub const DST_MEMORY_LAYOUT: u16 = 0x81;
    pub const DST_PITCH: u16 = 0x85;
    pub const DST_WIDTH: u16 = 0x86;
    pub const DST_HEIGHT: u16 = 0x87;
    pub const DST_UPPER: u16 = 0x88;
    pub const DST_LOWER: u16 = 0x89;
    pub const SRC_FORMAT: u16 = 0x8C;
    pub const SRC_MEMORY_LAYOUT: u16 = 0x8D;
    pub const SRC_PITCH: u16 = 0x91;
    pub const SRC_WIDTH: u16 = 0x92;
    pub const SRC_HEIGHT: u16 = 0x93;
    pub const SRC_UPPER: u16 = 0x94;
    pub const SRC_LOWER: u16 = 0x95;
    pub const OPERATION: u16 = 0xAB;
    pub const CLIP_X: u16 = 0xA0;
    pub const CLIP_Y: u16 = 0xA1;
    pub const CLIP_W: u16 = 0xA2;
    pub const CLIP_H: u16 = 0xA3;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Surface2D {
    pub address: RawGpuAddress,
    pub format: u32,
    pub memory_layout: u32,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TwoDState {
    pub source: Surface2D,
    pub destination: Surface2D,
    pub operation: u32,
    pub clip: [u32; 4],
    pub registers: BTreeMap<u16, u32>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwoDEvent {
    StateChanged { method: u16 },
    OperationChanged { raw: u32 },
    RegisterWrite { method: u16, argument: u32 },
}

impl TwoDState {
    pub fn apply(&mut self, write: EngineMethodWrite) -> Option<TwoDEvent> {
        if write.class != EngineClass::TwoD {
            return None;
        }
        use method::*;
        self.registers.insert(write.method, write.argument);
        match write.method {
            DST_FORMAT => self.destination.format = write.argument,
            DST_MEMORY_LAYOUT => self.destination.memory_layout = write.argument,
            DST_PITCH => self.destination.pitch = write.argument,
            DST_WIDTH => self.destination.width = write.argument,
            DST_HEIGHT => self.destination.height = write.argument,
            DST_UPPER => self.destination.address.a = write.argument,
            DST_LOWER => self.destination.address.b = write.argument,
            SRC_FORMAT => self.source.format = write.argument,
            SRC_MEMORY_LAYOUT => self.source.memory_layout = write.argument,
            SRC_PITCH => self.source.pitch = write.argument,
            SRC_WIDTH => self.source.width = write.argument,
            SRC_HEIGHT => self.source.height = write.argument,
            SRC_UPPER => self.source.address.a = write.argument,
            SRC_LOWER => self.source.address.b = write.argument,
            CLIP_X => self.clip[0] = write.argument,
            CLIP_Y => self.clip[1] = write.argument,
            CLIP_W => self.clip[2] = write.argument,
            CLIP_H => self.clip[3] = write.argument,
            OPERATION => {
                self.operation = write.argument;
                return Some(TwoDEvent::OperationChanged {
                    raw: write.argument,
                });
            }
            _ => {
                return Some(TwoDEvent::RegisterWrite {
                    method: write.method,
                    argument: write.argument,
                });
            }
        }
        Some(TwoDEvent::StateChanged {
            method: write.method,
        })
    }
}
