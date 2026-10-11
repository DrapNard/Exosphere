// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! MAXWELL_DMA_COPY_A state tracking.
//!
//! The first implementation records the address/shape registers and emits a launch description.
//! It does not lower the operation to canonical `Command::Copy` until tiling/remap/stride semantics
//! have been validated for that launch mode.

use crate::{
    engine::{EngineClass, EngineMethodWrite},
    maxwell_state::RawGpuAddress,
};

pub mod method {
    pub const LAUNCH_DMA: u16 = 0xC0;
    pub const OFFSET_IN_UPPER: u16 = 0x100;
    pub const OFFSET_IN_LOWER: u16 = 0x101;
    pub const OFFSET_OUT_UPPER: u16 = 0x102;
    pub const OFFSET_OUT_LOWER: u16 = 0x103;
    pub const PITCH_IN: u16 = 0x104;
    pub const PITCH_OUT: u16 = 0x105;
    pub const LINE_LENGTH_IN: u16 = 0x106;
    pub const LINE_COUNT: u16 = 0x107;
    pub const REMAP_CONST_A: u16 = 0x1C0;
    pub const REMAP_CONST_B: u16 = 0x1C1;
    pub const REMAP_COMPONENTS: u16 = 0x1C2;
    pub const DST_BLOCK_SIZE: u16 = 0x1C3;
    pub const DST_WIDTH: u16 = 0x1C4;
    pub const DST_HEIGHT: u16 = 0x1C5;
    pub const SRC_BLOCK_SIZE: u16 = 0x1CA;
    pub const SRC_WIDTH: u16 = 0x1CB;
    pub const SRC_HEIGHT: u16 = 0x1CC;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DmaCopyState {
    pub source: RawGpuAddress,
    pub destination: RawGpuAddress,
    pub pitch_in: u32,
    pub pitch_out: u32,
    pub line_length: u32,
    pub line_count: u32,
    pub remap_const_a: u32,
    pub remap_const_b: u32,
    pub remap_components: u32,
    pub dst_block_size: u32,
    pub dst_width: u32,
    pub dst_height: u32,
    pub src_block_size: u32,
    pub src_width: u32,
    pub src_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaLaunch {
    pub state: DmaCopyState,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinearDmaShape {
    pub rows: u32,
    pub bytes_per_row: u32,
    pub source_stride: u32,
    pub destination_stride: u32,
}

impl DmaLaunch {
    /// Admit only virtual pitch-layout transfers with no remapping, semaphore,
    /// interrupt or cache side effects. B0B5 launch bits follow NVIDIA's class
    /// header; notably transfer type zero means NONE, not a default copy.
    pub fn linear_shape(self) -> Option<LinearDmaShape> {
        const PITCH_LAYOUTS: u32 = (1 << 7) | (1 << 8);
        const MULTI_LINE: u32 = 1 << 9;
        let transfer = self.flags & 3;
        if !matches!(transfer, 1 | 2)
            || self.flags & PITCH_LAYOUTS != PITCH_LAYOUTS
            || self.flags & !(3 | PITCH_LAYOUTS | MULTI_LINE) != 0
            || self.state.line_length == 0
        {
            return None;
        }
        let multi_line = self.flags & MULTI_LINE != 0;
        let rows = if multi_line { self.state.line_count } else { 1 };
        if rows == 0 {
            return None;
        }
        Some(LinearDmaShape {
            rows,
            bytes_per_row: self.state.line_length,
            source_stride: if multi_line { self.state.pitch_in } else { 0 },
            destination_stride: if multi_line { self.state.pitch_out } else { 0 },
        })
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    #[test]
    fn linear_launch_shape_obeys_transfer_layout_and_side_effect_flags() {
        let mut launch = DmaLaunch {
            state: DmaCopyState {
                line_length: 16,
                line_count: 3,
                pitch_in: 32,
                pitch_out: 64,
                ..Default::default()
            },
            flags: 0x181,
        };
        assert_eq!(launch.linear_shape().unwrap().rows, 1);
        launch.flags = 0x382;
        assert_eq!(
            launch.linear_shape(),
            Some(LinearDmaShape {
                rows: 3,
                bytes_per_row: 16,
                source_stride: 32,
                destination_stride: 64,
            })
        );
        for flags in [0, 0x180, 0x183, 0x101, 0x81] {
            launch.flags = flags;
            assert!(launch.linear_shape().is_none());
        }
        for bit in (2..32).filter(|bit| !matches!(bit, 7..=9)) {
            launch.flags = 0x181 | (1 << bit);
            assert!(launch.linear_shape().is_none(), "bit {bit}");
        }
        launch.flags = 0x381;
        launch.state.line_count = 0;
        assert!(launch.linear_shape().is_none());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaEvent {
    Launch(DmaLaunch),
    Opaque { method: u16, argument: u32 },
}

impl DmaCopyState {
    pub fn apply(&mut self, write: EngineMethodWrite) -> Option<DmaEvent> {
        if write.class != EngineClass::DmaCopy {
            return None;
        }
        use method::*;
        match write.method {
            OFFSET_IN_UPPER => {
                self.source.a = write.argument;
                None
            }
            OFFSET_IN_LOWER => {
                self.source.b = write.argument;
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
            PITCH_IN => {
                self.pitch_in = write.argument;
                None
            }
            PITCH_OUT => {
                self.pitch_out = write.argument;
                None
            }
            LINE_LENGTH_IN => {
                self.line_length = write.argument;
                None
            }
            LINE_COUNT => {
                self.line_count = write.argument;
                None
            }
            REMAP_CONST_A => {
                self.remap_const_a = write.argument;
                None
            }
            REMAP_CONST_B => {
                self.remap_const_b = write.argument;
                None
            }
            REMAP_COMPONENTS => {
                self.remap_components = write.argument;
                None
            }
            DST_BLOCK_SIZE => {
                self.dst_block_size = write.argument;
                None
            }
            DST_WIDTH => {
                self.dst_width = write.argument;
                None
            }
            DST_HEIGHT => {
                self.dst_height = write.argument;
                None
            }
            SRC_BLOCK_SIZE => {
                self.src_block_size = write.argument;
                None
            }
            SRC_WIDTH => {
                self.src_width = write.argument;
                None
            }
            SRC_HEIGHT => {
                self.src_height = write.argument;
                None
            }
            LAUNCH_DMA => Some(DmaEvent::Launch(DmaLaunch {
                state: *self,
                flags: write.argument,
            })),
            _ => Some(DmaEvent::Opaque {
                method: write.method,
                argument: write.argument,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dma(method: u16, argument: u32) -> EngineMethodWrite {
        EngineMethodWrite {
            subchannel: 4,
            class: EngineClass::DmaCopy,
            method,
            argument,
        }
    }
    #[test]
    fn launch_snapshots_copy_state() {
        let mut state = DmaCopyState::default();
        state.apply(dma(method::OFFSET_IN_UPPER, 1));
        state.apply(dma(method::OFFSET_IN_LOWER, 2));
        state.apply(dma(method::LINE_LENGTH_IN, 4096));
        state.apply(dma(method::LINE_COUNT, 2));
        let DmaEvent::Launch(launch) = state.apply(dma(method::LAUNCH_DMA, 0x186)).unwrap() else {
            panic!("expected launch")
        };
        assert_eq!(launch.state.source, RawGpuAddress { a: 1, b: 2 });
        assert_eq!(launch.state.line_length, 4096);
        assert_eq!(launch.state.line_count, 2);
    }
}
