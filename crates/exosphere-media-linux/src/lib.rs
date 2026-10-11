// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Linux media backend selector.
//!
//! Planned order: native/hardware path when available, FFmpeg as the general compatibility backend,
//! then software fallback. FFmpeg objects never escape this crate.

use exosphere_media_ir::MediaJob;

pub struct LinuxMediaBackend;
impl LinuxMediaBackend {
    pub fn execute(&mut self, _job: &MediaJob) -> Result<(), String> {
        Err("Linux media backend not wired yet".into())
    }
}
