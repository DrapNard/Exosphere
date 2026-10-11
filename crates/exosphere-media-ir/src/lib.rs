// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Project-owned media operations. No FFmpeg/platform objects are canonical here.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub u64);

#[derive(Debug, Clone)]
pub enum MediaJob {
    DecodeVideo {
        input: u64,
        output: SurfaceId,
    },
    EncodeVideo {
        input: SurfaceId,
        output: u64,
    },
    DecodeImage {
        input: u64,
        output: SurfaceId,
    },
    Scale {
        input: SurfaceId,
        output: SurfaceId,
        width: u32,
        height: u32,
    },
    Crop {
        input: SurfaceId,
        output: SurfaceId,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    ColorConvert {
        input: SurfaceId,
        output: SurfaceId,
    },
    Composite {
        inputs: Vec<SurfaceId>,
        output: SurfaceId,
    },
    Blit {
        input: SurfaceId,
        output: SurfaceId,
    },
    Present {
        input: SurfaceId,
    },
}
