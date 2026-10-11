// SPDX-License-Identifier: GPL-3.0-or-later
//! Native Linux Vulkan surfaces; swapchain and image presentation are separate.

use ash::{khr, vk};
use exosphere_gpu_vulkan::ash_device::AshVulkanDevice;
use std::sync::Arc;
use winit::{
    raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle},
    window::Window,
};

pub struct LinuxVulkanSurface {
    pub(crate) backend: AshVulkanDevice,
    loader: khr::surface::Instance,
    surface: vk::SurfaceKHR,
    // A native surface must never outlive its display/window handles.
    window: Arc<Window>,
}

impl LinuxVulkanSurface {
    pub fn create(window: Arc<Window>) -> Result<Self, String> {
        let display = window
            .display_handle()
            .map_err(|error| error.to_string())?
            .as_raw();
        let handle = window
            .window_handle()
            .map_err(|error| error.to_string())?
            .as_raw();
        let extension = match (display, handle) {
            (RawDisplayHandle::Wayland(_), RawWindowHandle::Wayland(_)) => {
                khr::wayland_surface::NAME
            }
            (RawDisplayHandle::Xlib(_), RawWindowHandle::Xlib(_)) => khr::xlib_surface::NAME,
            (RawDisplayHandle::Xcb(_), RawWindowHandle::Xcb(_)) => khr::xcb_surface::NAME,
            _ => return Err("unsupported Linux display/window handle pair".into()),
        };
        let backend = AshVulkanDevice::create_with_extensions(
            &[],
            &[
                khr::surface::NAME,
                extension,
                khr::get_surface_capabilities2::NAME,
                ash::ext::surface_maintenance1::NAME,
            ],
            &[khr::swapchain::NAME, ash::ext::swapchain_maintenance1::NAME],
        )?;
        let instance = backend.instance();
        let entry = backend.entry();
        let surface = unsafe {
            match (display, handle) {
                (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
                    khr::wayland_surface::Instance::new(entry, instance).create_wayland_surface(
                        &vk::WaylandSurfaceCreateInfoKHR::default()
                            .display(display.display.as_ptr())
                            .surface(window.surface.as_ptr()),
                        None,
                    )
                }
                (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(window)) => {
                    let display = display.display.ok_or("missing Xlib display")?;
                    khr::xlib_surface::Instance::new(entry, instance).create_xlib_surface(
                        &vk::XlibSurfaceCreateInfoKHR::default()
                            .dpy(display.as_ptr().cast())
                            .window(window.window),
                        None,
                    )
                }
                (RawDisplayHandle::Xcb(display), RawWindowHandle::Xcb(window)) => {
                    let connection = display.connection.ok_or("missing Xcb connection")?;
                    khr::xcb_surface::Instance::new(entry, instance).create_xcb_surface(
                        &vk::XcbSurfaceCreateInfoKHR::default()
                            .connection(connection.as_ptr().cast())
                            .window(window.window.get()),
                        None,
                    )
                }
                _ => unreachable!(),
            }
        }
        .map_err(|error| format!("Linux Vulkan surface: {error:?}"))?;
        let loader = khr::surface::Instance::new(entry, instance);
        let created = Self {
            backend,
            loader,
            surface,
            window,
        };
        let supported = unsafe {
            created.loader.get_physical_device_surface_support(
                created.backend.physical_device(),
                created.backend.queue_family_index(),
                created.surface,
            )
        }
        .map_err(|error| format!("Vulkan presentation support: {error:?}"))?;
        if !supported {
            return Err("selected Vulkan queue cannot present to this Linux surface".into());
        }
        Ok(created)
    }

    pub fn window(&self) -> &Window {
        &self.window
    }
    pub fn backend(&self) -> &AshVulkanDevice {
        &self.backend
    }
    pub fn handle(&self) -> vk::SurfaceKHR {
        self.surface
    }
    pub fn capabilities(&self) -> Result<vk::SurfaceCapabilitiesKHR, String> {
        unsafe {
            self.loader.get_physical_device_surface_capabilities(
                self.backend.physical_device(),
                self.surface,
            )
        }
        .map_err(|error| format!("Vulkan surface capabilities: {error:?}"))
    }
    pub fn formats(&self) -> Result<Vec<vk::SurfaceFormatKHR>, String> {
        unsafe {
            self.loader
                .get_physical_device_surface_formats(self.backend.physical_device(), self.surface)
        }
        .map_err(|error| format!("Vulkan surface formats: {error:?}"))
    }
}

impl Drop for LinuxVulkanSurface {
    fn drop(&mut self) {
        // The surface is destroyed before the backend's instance and window.
        unsafe { self.loader.destroy_surface(self.surface, None) };
    }
}
