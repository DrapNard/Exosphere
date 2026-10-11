// SPDX-License-Identifier: GPL-3.0-or-later
//! Native triangle transfer/present/resize diagnostic, not a game rendering test.

use exosphere_platform_linux::{
    input::KeyboardController,
    surface::LinuxVulkanSurface,
    swapchain::{Acquisition, LinuxSwapchain, Presentation},
    window::{WindowClient, run_window},
};
use std::sync::Arc;
use winit::{event::WindowEvent, window::Window};

#[derive(Default)]
struct SwapchainClient {
    swapchain: Option<LinuxSwapchain>,
    input: KeyboardController,
    hid: exosphere_horizon::hid::HidState,
}

impl WindowClient for SwapchainClient {
    fn created(&mut self, window: Arc<Window>) -> Result<(), String> {
        self.swapchain = Some(LinuxSwapchain::create(LinuxVulkanSurface::create(window)?)?);
        println!("Linux swapchain allocated; resize the window to exercise replacement");
        Ok(())
    }

    fn event(&mut self, _window: &Window, event: &WindowEvent) -> Result<(), String> {
        self.input.event(event, &mut self.hid);
        if matches!(event, WindowEvent::Resized(_)) {
            let swapchain = self.swapchain.as_mut().ok_or("missing Linux swapchain")?;
            if swapchain.recreate()? {
                println!(
                    "Linux swapchain replaced: {:?}, {} images",
                    swapchain.extent(),
                    swapchain.image_count()
                );
            } else {
                println!("Linux swapchain suspended for minimized window");
            }
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            let swapchain = self.swapchain.as_mut().ok_or("missing Linux swapchain")?;
            let acquired = swapchain.acquire()?;
            println!("Linux swapchain acquisition: {acquired:?}");
            if matches!(acquired, Acquisition::Ready { .. }) {
                let module = exosphere_gpu_ir::GpuModule::triangle_vulkan_smoke_test(256, 256);
                let plan = exosphere_gpu_vulkan::VulkanBackend::new()
                    .plan(&module)
                    .map_err(|error| error.to_string())?;
                let mut resources = exosphere_gpu_ir::InMemoryGpuResources::new();
                swapchain
                    .execute_plan(&plan, &mut resources)
                    .map_err(|error| error.to_string())?;
                let presented = swapchain.present_image(module.textures[0].id)?;
                println!("Linux swapchain diagnostic presentation: {presented:?}");
                if matches!(presented, Presentation::NeedsRebuild) {
                    swapchain.recreate()?;
                }
            } else if matches!(acquired, Acquisition::NeedsRebuild) {
                swapchain.recreate()?;
            }
        }
        Ok(())
    }
}

fn main() -> Result<(), String> {
    run_window(
        "Exosphere Linux swapchain lifecycle",
        SwapchainClient::default(),
    )
}
