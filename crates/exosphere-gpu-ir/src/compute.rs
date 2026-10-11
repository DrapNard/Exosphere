// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Stateful MAXWELL_COMPUTE_B front-end.
//!
//! Compute launches on Maxwell are QMD-driven. Exosphere therefore records the register state that
//! identifies/feeds QMD submission instead of inventing a host dispatch size from incomplete method
//! information. Once a QMD has been fetched from guest GPU memory, the same state can be lowered into
//! canonical `Command::Dispatch`.

use crate::engine::{EngineClass, EngineMethodWrite};

pub mod method {
    pub const LINE_LENGTH_IN: u16 = 0x60;
    pub const LINE_COUNT: u16 = 0x61;
    pub const OFFSET_OUT_UPPER: u16 = 0x62;
    pub const OFFSET_OUT_LOWER: u16 = 0x63;
    pub const PITCH_OUT: u16 = 0x64;
    pub const SET_CWD_CONTROL: u16 = 0x90;
    pub const SET_CWD_REF_COUNTER: u16 = 0x92;
    pub const SET_QMD_VERSION: u16 = 0xA2;
    pub const SEND_PCAS_A: u16 = 0xAD;
    pub const SEND_PCAS_B: u16 = 0xAE;
    pub const SEND_SIGNALING_PCAS_B: u16 = 0xAF;
    pub const SET_PROGRAM_REGION_A: u16 = 0x582;
    pub const SET_PROGRAM_REGION_B: u16 = 0x583;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComputeState {
    pub line_length: u32,
    pub line_count: u32,
    pub offset_out_upper: u32,
    pub offset_out_lower: u32,
    pub pitch_out: u32,
    pub cwd_control: u32,
    pub cwd_ref_counter: u32,
    pub qmd_version: u32,
    pub pcas_a: u32,
    pub pcas_b: u32,
    pub program_region_a: u32,
    pub program_region_b: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeEvent {
    QmdSubmit {
        signaling: bool,
        pcas_a: u32,
        pcas_b: u32,
    },
    Opaque {
        method: u16,
        argument: u32,
    },
}

impl ComputeState {
    pub fn apply(&mut self, write: EngineMethodWrite) -> Option<ComputeEvent> {
        if write.class != EngineClass::Compute {
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
                self.offset_out_upper = write.argument;
                None
            }
            OFFSET_OUT_LOWER => {
                self.offset_out_lower = write.argument;
                None
            }
            PITCH_OUT => {
                self.pitch_out = write.argument;
                None
            }
            SET_CWD_CONTROL => {
                self.cwd_control = write.argument;
                None
            }
            SET_CWD_REF_COUNTER => {
                self.cwd_ref_counter = write.argument;
                None
            }
            SET_QMD_VERSION => {
                self.qmd_version = write.argument;
                None
            }
            SEND_PCAS_A => {
                self.pcas_a = write.argument;
                None
            }
            SEND_PCAS_B => {
                self.pcas_b = write.argument;
                Some(ComputeEvent::QmdSubmit {
                    signaling: false,
                    pcas_a: self.pcas_a,
                    pcas_b: self.pcas_b,
                })
            }
            SEND_SIGNALING_PCAS_B => {
                self.pcas_b = write.argument;
                Some(ComputeEvent::QmdSubmit {
                    signaling: true,
                    pcas_a: self.pcas_a,
                    pcas_b: self.pcas_b,
                })
            }
            SET_PROGRAM_REGION_A => {
                self.program_region_a = write.argument;
                None
            }
            SET_PROGRAM_REGION_B => {
                self.program_region_b = write.argument;
                None
            }
            _ => Some(ComputeEvent::Opaque {
                method: write.method,
                argument: write.argument,
            }),
        }
    }
}
