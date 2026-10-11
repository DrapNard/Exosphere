// SPDX-License-Identifier: GPL-3.0-or-later
//! Capability-checked Linux swapchains and synchronous diagnostic presentation.

use crate::surface::LinuxVulkanSurface;
use ash::{khr, vk};
mod presentation;
pub use presentation::Presentation;

pub struct LinuxSwapchain {
    // Field order releases native images before their surface and device.
    allocation: Option<SwapchainAllocation>,
    surface: LinuxVulkanSurface,
}

struct SwapchainAllocation {
    device: ash::Device,
    loader: khr::swapchain::Device,
    maintenance: ash::ext::swapchain_maintenance1::Device,
    handle: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    extent: vk::Extent2D,
    format: vk::Format,
    acquire_fence: vk::Fence,
    acquired: Option<u32>,
    acquisition_failed: bool,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    command: vk::CommandBuffer,
    submit_fence: vk::Fence,
    present_fence: vk::Fence,
    render_finished: vk::Semaphore,
    pending_present: bool,
}

/// Polling acquisition never treats a missing image as a completed frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquisition {
    Suspended,
    NotReady,
    NeedsRebuild,
    Ready { image_index: u32, suboptimal: bool },
}

impl LinuxSwapchain {
    pub fn create(surface: LinuxVulkanSurface) -> Result<Self, String> {
        let allocation = SwapchainAllocation::create(&surface)?;
        Ok(Self {
            allocation: Some(allocation),
            surface,
        })
    }

    /// Rebuild after resize or an out-of-date acquisition/presentation result.
    /// A minimized window suspends allocation; restore events can retry this call.
    /// Errors leave no stale allocation available to a future presenter.
    pub fn recreate(&mut self) -> Result<bool, String> {
        unsafe { self.surface.backend.device().device_wait_idle() }
            .map_err(|error| format!("Linux swapchain retirement: {error:?}"))?;
        // This initial synchronous implementation destroys the old chain before
        // replacement. It avoids both pending users and a retired oldSwapchain
        // being accidentally reused when replacement allocation fails.
        drop(self.allocation.take());
        let size = self.surface.window().inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(false);
        }
        self.allocation = Some(SwapchainAllocation::create(&self.surface)?);
        Ok(true)
    }

    pub fn extent(&self) -> Option<vk::Extent2D> {
        self.allocation.as_ref().map(|allocation| allocation.extent)
    }
    pub fn format(&self) -> Option<vk::Format> {
        self.allocation.as_ref().map(|allocation| allocation.format)
    }
    pub fn image_count(&self) -> usize {
        self.allocation
            .as_ref()
            .map_or(0, |allocation| allocation.images.len())
    }

    /// Execute rendering on the same owned device as this window. Exposing only
    /// this operation prevents callers from replacing the backend while native
    /// surface/swapchain objects still reference its device and instance.
    pub fn execute_plan(
        &mut self,
        plan: &exosphere_gpu_vulkan::plan::VulkanExecutionPlan,
        resources: &mut dyn exosphere_gpu_ir::GpuResourceData,
    ) -> Result<(), exosphere_gpu_vulkan::VulkanError> {
        use exosphere_gpu_vulkan::device::VulkanDevice;
        self.surface
            .backend
            .execute_plan_with_resources(plan, resources)
    }

    /// Poll for one image, then wait for its acquisition fence before exposing
    /// its index. Only one image can be held; presentation must release it before
    /// another acquisition. A recreation also releases the held allocation.
    pub fn acquire(&mut self) -> Result<Acquisition, String> {
        let Some(allocation) = self.allocation.as_mut() else {
            return Ok(Acquisition::Suspended);
        };
        if allocation.acquisition_failed {
            return Err("Linux swapchain acquisition failed; recreate before retrying".into());
        }
        if allocation.acquired.is_some() {
            return Err("Linux swapchain already holds an acquired image".into());
        }
        let acquired = unsafe {
            allocation.loader.acquire_next_image(
                allocation.handle,
                0,
                vk::Semaphore::null(),
                allocation.acquire_fence,
            )
        };
        let (index, suboptimal) = match acquired {
            Ok(image) => image,
            Err(vk::Result::NOT_READY | vk::Result::TIMEOUT) => return Ok(Acquisition::NotReady),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(Acquisition::NeedsRebuild),
            Err(error) => {
                allocation.acquisition_failed = true;
                return Err(format!("Linux swapchain acquisition: {error:?}"));
            }
        };
        // Receiving an index alone does not authorize image access. The fence
        // also proves completion of the previous presentation for this image.
        if let Err(error) = unsafe {
            allocation
                .device
                .wait_for_fences(&[allocation.acquire_fence], true, u64::MAX)
        } {
            allocation.acquisition_failed = true;
            return Err(format!("Linux swapchain acquisition fence: {error:?}"));
        }
        if index as usize >= allocation.images.len() {
            allocation.acquisition_failed = true;
            return Err("Linux swapchain returned an invalid image index".into());
        }
        allocation.acquired = Some(index);
        if let Err(error) = unsafe { allocation.device.reset_fences(&[allocation.acquire_fence]) } {
            allocation.acquisition_failed = true;
            return Err(format!(
                "Linux swapchain acquisition fence reset: {error:?}"
            ));
        }
        Ok(Acquisition::Ready {
            image_index: index,
            suboptimal,
        })
    }

    /// Return an acquired but unsubmitted image without pretending to present it.
    pub fn release_acquired(&mut self) -> Result<(), String> {
        let allocation = self
            .allocation
            .as_mut()
            .ok_or("Linux swapchain is suspended")?;
        if allocation.acquisition_failed {
            return Err("Linux swapchain acquisition failed; recreate before releasing".into());
        }
        let index = allocation
            .acquired
            .ok_or("Linux swapchain has no acquired image")?;
        let indices = [index];
        let info = vk::ReleaseSwapchainImagesInfoEXT::default()
            .swapchain(allocation.handle)
            .image_indices(&indices);
        unsafe { allocation.maintenance.release_swapchain_images(&info) }
            .map_err(|error| format!("Linux swapchain image release: {error:?}"))?;
        allocation.acquired = None;
        Ok(())
    }
}

impl SwapchainAllocation {
    fn create(surface: &LinuxVulkanSurface) -> Result<Self, String> {
        let capabilities = surface.capabilities()?;
        if !capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_DST)
        {
            return Err("Linux surface cannot receive transferred frame images".into());
        }
        let size = surface.window().inner_size();
        if size.width == 0 || size.height == 0 {
            return Err("Linux swapchain creation suspended for zero-sized window".into());
        }
        let extent = select_extent(
            &capabilities,
            vk::Extent2D {
                width: size.width,
                height: size.height,
            },
        )?;
        let formats = surface.formats()?;
        let format = formats
            .iter()
            .find(|format| {
                matches!(
                    format.format,
                    vk::Format::R8G8B8A8_UNORM | vk::Format::B8G8R8A8_UNORM
                ) && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .copied()
            .or_else(|| {
                (formats.len() == 1 && formats[0].format == vk::Format::UNDEFINED).then(|| {
                    vk::SurfaceFormatKHR {
                        format: vk::Format::R8G8B8A8_UNORM,
                        color_space: formats[0].color_space,
                    }
                })
            })
            .ok_or("Linux surface has no supported RGBA8/BGRA8 format")?;
        let alpha = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::INHERIT,
        ]
        .into_iter()
        .find(|alpha| capabilities.supported_composite_alpha.contains(*alpha))
        .ok_or("Linux surface has no supported composite alpha mode")?;
        let mut count = capabilities.min_image_count.saturating_add(1);
        if capabilities.max_image_count != 0 {
            count = count.min(capabilities.max_image_count);
        }
        let loader =
            khr::swapchain::Device::new(surface.backend.instance(), surface.backend.device());
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface.handle())
            .min_image_count(count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(alpha)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true);
        let handle = unsafe { loader.create_swapchain(&info, None) }
            .map_err(|error| format!("Linux swapchain creation: {error:?}"))?;
        // Install ownership before querying images so a query error still
        // destroys the swapchain before its surface and instance.
        let mut created = Self {
            device: surface.backend.device().clone(),
            maintenance: ash::ext::swapchain_maintenance1::Device::new(
                surface.backend.instance(),
                surface.backend.device(),
            ),
            loader,
            handle,
            images: Vec::new(),
            extent,
            format: format.format,
            acquire_fence: vk::Fence::null(),
            acquired: None,
            acquisition_failed: false,
            queue: unsafe {
                surface
                    .backend
                    .device()
                    .get_device_queue(surface.backend.queue_family_index(), 0)
            },
            command_pool: vk::CommandPool::null(),
            command: vk::CommandBuffer::null(),
            submit_fence: vk::Fence::null(),
            present_fence: vk::Fence::null(),
            render_finished: vk::Semaphore::null(),
            pending_present: false,
        };
        created.images = unsafe { created.loader.get_swapchain_images(created.handle) }
            .map_err(|error| format!("Linux swapchain images: {error:?}"))?;
        if created.images.is_empty() {
            return Err("Linux swapchain returned no images".into());
        }
        created.acquire_fence = unsafe {
            created
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
        }
        .map_err(|error| format!("Linux swapchain acquisition fence creation: {error:?}"))?;
        created.create_presentation_resources(surface.backend.queue_family_index())?;
        Ok(created)
    }
}

fn select_extent(
    capabilities: &vk::SurfaceCapabilitiesKHR,
    requested: vk::Extent2D,
) -> Result<vk::Extent2D, String> {
    if requested.width == 0 || requested.height == 0 {
        return Err("Linux swapchain creation suspended for zero-sized window".into());
    }
    let min = capabilities.min_image_extent;
    let max = capabilities.max_image_extent;
    if min.width > max.width || min.height > max.height {
        return Err("Linux surface advertises inconsistent extent limits".into());
    }
    // A fixed surface extent wins over the window size. The sentinel must
    // occur in both dimensions; treating it as a pixel size risks huge allocations.
    let current = capabilities.current_extent;
    let extent = match (current.width == u32::MAX, current.height == u32::MAX) {
        (true, true) => vk::Extent2D {
            width: requested.width.clamp(min.width, max.width),
            height: requested.height.clamp(min.height, max.height),
        },
        (false, false) => current,
        _ => return Err("Linux surface advertises an inconsistent extent sentinel".into()),
    };
    if extent.width == 0 || extent.height == 0 {
        return Err("Linux surface has a zero-sized extent".into());
    }
    if extent.width < min.width
        || extent.width > max.width
        || extent.height < min.height
        || extent.height > max.height
    {
        return Err("Linux surface extent exceeds advertised limits".into());
    }
    Ok(extent)
}

impl Drop for SwapchainAllocation {
    fn drop(&mut self) {
        // Future presentation work must be completed before native teardown.
        unsafe {
            if self.pending_present {
                // A device-idle wait alone does not retire presentation-engine
                // references. The maintenance fence explicitly covers them.
                let _ = self
                    .device
                    .wait_for_fences(&[self.present_fence], true, u64::MAX);
            }
            let _ = self.device.device_wait_idle();
            self.device.destroy_semaphore(self.render_finished, None);
            self.device.destroy_fence(self.present_fence, None);
            self.device.destroy_fence(self.submit_fence, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_fence(self.acquire_fence, None);
            self.loader.destroy_swapchain(self.handle, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(width: u32, height: u32) -> vk::Extent2D {
        vk::Extent2D { width, height }
    }

    fn capabilities() -> vk::SurfaceCapabilitiesKHR {
        vk::SurfaceCapabilitiesKHR::default()
            .current_extent(extent(u32::MAX, u32::MAX))
            .min_image_extent(extent(64, 32))
            .max_image_extent(extent(1920, 1080))
    }

    #[test]
    fn variable_extent_clamps_each_dimension() {
        assert_eq!(
            select_extent(&capabilities(), extent(4000, 16)).unwrap(),
            extent(1920, 32)
        );
        assert_eq!(
            select_extent(&capabilities(), extent(800, 600)).unwrap(),
            extent(800, 600)
        );
    }

    #[test]
    fn fixed_extent_uses_surface_not_window_size() {
        let caps = capabilities().current_extent(extent(1280, 720));
        assert_eq!(
            select_extent(&caps, extent(800, 600)).unwrap(),
            extent(1280, 720)
        );
        assert!(select_extent(&caps, extent(0, 600)).is_err());
    }

    #[test]
    fn malformed_or_suspended_surface_is_rejected() {
        for caps in [
            capabilities().min_image_extent(extent(2000, 32)),
            capabilities().current_extent(extent(u32::MAX, 720)),
            capabilities().current_extent(extent(1280, u32::MAX)),
            capabilities().current_extent(extent(0, 0)),
            capabilities().current_extent(extent(2000, 720)),
        ] {
            assert!(select_extent(&caps, extent(800, 600)).is_err());
        }
    }
}
