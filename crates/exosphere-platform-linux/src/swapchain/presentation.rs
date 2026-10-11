// SPDX-License-Identifier: GPL-3.0-or-later
//! Synchronous diagnostic presentation with explicit presentation retirement.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    Queued { suboptimal: bool },
    NeedsRebuild,
}

impl LinuxSwapchain {
    /// Clear and present the acquired image. This is a host diagnostic, not
    /// translation of game rendering. Completion retires native references;
    /// it does not prove the monitor has displayed the image.
    pub fn present_clear(&mut self, rgba: [f32; 4]) -> Result<Presentation, String> {
        if rgba.iter().any(|value| !value.is_finite()) {
            return Err("Linux presentation color must be finite".into());
        }
        let allocation = self
            .allocation
            .as_mut()
            .ok_or("Linux swapchain is suspended")?;
        if allocation.acquisition_failed {
            return Err("Linux swapchain failed; recreate before presenting".into());
        }
        let index = allocation
            .acquired
            .ok_or("Linux swapchain has no acquired image")?;
        let device = allocation.device.clone();
        let result = allocation.record_and_present(index, |command, image, _format, _extent| {
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            unsafe {
                device.cmd_clear_color_image(
                    command,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &vk::ClearColorValue { float32: rgba },
                    &[range],
                )
            };
            Ok(())
        });
        if result.is_err() {
            // No retry may reset a pending fence or signal an unconsumed semaphore.
            allocation.acquisition_failed = true;
        }
        result
    }

    /// Transfer a rendered backend image directly to the acquired window image.
    /// Scaling uses nearest filtering; RGBA/BGRA conversion is handled by Vulkan.
    pub fn present_image(
        &mut self,
        source: exosphere_gpu_ir::ResourceId,
    ) -> Result<Presentation, String> {
        let allocation = self
            .allocation
            .as_mut()
            .ok_or("Linux swapchain is suspended")?;
        if allocation.acquisition_failed {
            return Err("Linux swapchain failed; recreate before presenting".into());
        }
        let index = allocation
            .acquired
            .ok_or("Linux swapchain has no acquired image")?;
        let backend = &mut self.surface.backend;
        let result = allocation.record_and_present(index, |command, image, format, extent| {
            // Both objects belong to the surface backend. Acquisition has been
            // waited, target layout is established, and submission/retirement
            // below finishes before either owned allocation can be changed.
            unsafe { backend.record_present_blit(command, source, image, format, extent) }
                .map_err(|error| error.to_string())
        });
        if result.is_err() {
            allocation.acquisition_failed = true;
        }
        result
    }
}

impl SwapchainAllocation {
    pub(super) fn create_presentation_resources(
        &mut self,
        queue_family: u32,
    ) -> Result<(), String> {
        let info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        self.command_pool = unsafe { self.device.create_command_pool(&info, None) }
            .map_err(|error| format!("Linux presentation command pool: {error:?}"))?;
        let alloc = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        self.command = unsafe { self.device.allocate_command_buffers(&alloc) }
            .map_err(|error| format!("Linux presentation command buffer: {error:?}"))?[0];
        self.submit_fence = unsafe {
            self.device
                .create_fence(&vk::FenceCreateInfo::default(), None)
        }
        .map_err(|error| format!("Linux presentation submission fence: {error:?}"))?;
        self.present_fence = unsafe {
            self.device
                .create_fence(&vk::FenceCreateInfo::default(), None)
        }
        .map_err(|error| format!("Linux presentation retirement fence: {error:?}"))?;
        self.render_finished = unsafe {
            self.device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        }
        .map_err(|error| format!("Linux presentation semaphore: {error:?}"))?;
        Ok(())
    }

    fn record_and_present(
        &mut self,
        index: u32,
        record: impl FnOnce(
            vk::CommandBuffer,
            vk::Image,
            vk::Format,
            vk::Extent2D,
        ) -> Result<(), String>,
    ) -> Result<Presentation, String> {
        unsafe {
            self.device
                .reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())
        }
        .map_err(|error| format!("Linux presentation recording reset: {error:?}"))?;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { self.device.begin_command_buffer(self.command, &begin) }
            .map_err(|error| format!("Linux presentation recording begin: {error:?}"))?;
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let image = self.images[index as usize];
        let before = [vk::ImageMemoryBarrier2::default()
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            // Acquisition was host-waited. Discard the entire previous image.
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image).subresource_range(range)];
        let after = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)];
        unsafe {
            self.device.cmd_pipeline_barrier2(
                self.command,
                &vk::DependencyInfo::default().image_memory_barriers(&before),
            );
        }
        record(self.command, image, self.format, self.extent)?;
        unsafe {
            self.device.cmd_pipeline_barrier2(
                self.command,
                &vk::DependencyInfo::default().image_memory_barriers(&after),
            );
        }
        unsafe { self.device.end_command_buffer(self.command) }
            .map_err(|error| format!("Linux presentation recording end: {error:?}"))?;
        let commands = [vk::CommandBufferSubmitInfo::default().command_buffer(self.command)];
        let signal = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.render_finished)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let submit = [vk::SubmitInfo2::default()
            .command_buffer_infos(&commands)
            .signal_semaphore_infos(&signal)];
        unsafe {
            self.device
                .queue_submit2(self.queue, &submit, self.submit_fence)
        }
        .map_err(|error| format!("Linux presentation submission: {error:?}"))?;
        let fences = [self.present_fence];
        let mut retirement = vk::SwapchainPresentFenceInfoEXT::default().fences(&fences);
        let chains = [self.handle];
        let indices = [index];
        let waits = [self.render_finished];
        let present = vk::PresentInfoKHR::default()
            .swapchains(&chains)
            .image_indices(&indices)
            .wait_semaphores(&waits)
            .push_next(&mut retirement);
        let result = unsafe { self.loader.queue_present(self.queue, &present) };
        // These rejection results still enqueue the semaphore waits and retirement.
        let queued = presentation_was_enqueued(result);
        self.pending_present = queued;
        if queued {
            self.acquired = None;
            unsafe { self.device.wait_for_fences(&fences, true, u64::MAX) }
                .map_err(|error| format!("Linux presentation retirement: {error:?}"))?;
            self.pending_present = false;
        }
        // Submission completion also protects command-buffer reuse. If present
        // failed before enqueueing, its unsignaled fence must never be waited on.
        unsafe {
            self.device
                .wait_for_fences(&[self.submit_fence], true, u64::MAX)
        }
        .map_err(|error| format!("Linux presentation submission completion: {error:?}"))?;
        match result {
            Ok(suboptimal) => {
                unsafe {
                    self.device
                        .reset_fences(&[self.submit_fence, self.present_fence])
                }
                .map_err(|error| format!("Linux presentation fence reset: {error:?}"))?;
                Ok(Presentation::Queued { suboptimal })
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.acquisition_failed = true;
                Ok(Presentation::NeedsRebuild)
            }
            Err(error) => Err(format!("Linux queue presentation: {error:?}")),
        }
    }
}

fn presentation_was_enqueued(result: Result<bool, vk::Result>) -> bool {
    result.is_ok()
        || matches!(
            result,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR
                | vk::Result::ERROR_SURFACE_LOST_KHR
                | vk::Result::ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_rejection_still_requires_retirement() {
        for result in [
            Ok(false),
            Ok(true),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR),
            Err(vk::Result::ERROR_SURFACE_LOST_KHR),
            Err(vk::Result::ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT),
        ] {
            assert!(presentation_was_enqueued(result));
        }
    }

    #[test]
    fn failed_enqueue_does_not_wait_for_unsignaled_present_fence() {
        for error in [
            vk::Result::ERROR_OUT_OF_HOST_MEMORY,
            vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
            vk::Result::ERROR_DEVICE_LOST,
        ] {
            assert!(!presentation_was_enqueued(Err(error)));
        }
    }
}
