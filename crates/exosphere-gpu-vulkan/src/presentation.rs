// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Window-system neutral presentation contract.
//!
//! Exosphere's Vulkan core must remain usable headlessly. Platform crates may implement this trait
//! with Wayland/X11/Win32/Metal-layer surfaces without teaching canonical GPU IR about windows.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentExtent {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentMode {
    Immediate,
    Mailbox,
    Fifo,
    FifoRelaxed,
}

pub trait VulkanPresentation: Send {
    fn extent(&self) -> PresentExtent;
    fn preferred_mode(&self) -> PresentMode {
        PresentMode::Fifo
    }
    fn recreate(&mut self, extent: PresentExtent) -> Result<(), String>;
    fn present(&mut self) -> Result<(), String>;
}
