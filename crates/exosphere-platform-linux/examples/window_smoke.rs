// SPDX-License-Identifier: GPL-3.0-or-later
//! Window lifecycle diagnostic; no game frame or Vulkan presentation is implied.

use exosphere_platform_linux::window::{WindowClient, run_window};
use winit::{event::WindowEvent, window::Window};

struct Smoke;

impl WindowClient for Smoke {
    fn event(&mut self, window: &Window, event: &WindowEvent) -> Result<(), String> {
        if matches!(event, WindowEvent::RedrawRequested) {
            let extent = window.inner_size();
            println!(
                "Linux window redraw: {}x{} (no renderer attached)",
                extent.width, extent.height
            );
        }
        Ok(())
    }
}

fn main() -> Result<(), String> {
    run_window("Exosphere window lifecycle smoke", Smoke)
}
