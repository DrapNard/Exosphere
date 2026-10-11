// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Linux audio platform boundary.
//!
//! PipeWire is the preferred modern host path. The guest-facing audio model belongs in the runtime,
//! while PipeWire handles and callbacks remain private to this crate.

#[derive(Default)]
pub struct LinuxAudioBackend;
impl LinuxAudioBackend {
    pub fn new() -> Self {
        Self
    }
    pub fn backend_name(&self) -> &'static str {
        "PipeWire (planned)"
    }
}
