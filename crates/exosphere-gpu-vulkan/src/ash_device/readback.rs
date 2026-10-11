// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded synchronous readback of a single RGBA8 color image.

use super::*;

impl AshVulkanDevice {
    /// Initialize a single-layer RGBA8 image with existing pixels, without a
    /// dummy draw or clear. An already allocated image must match its shape.
    pub fn initialize_rgba8(
        &mut self,
        desc: &TextureDesc,
        pixels: &[u8],
    ) -> Result<(), VulkanError> {
        let size = u64::from(desc.width)
            .checked_mul(u64::from(desc.height))
            .and_then(|size| size.checked_mul(4))
            .filter(|size| *size > 0 && *size <= 64 * 1024 * 1024)
            .ok_or(VulkanError::UnsupportedFeature(
                "RGBA8 initialization size budget",
            ))?;
        if desc.format != exosphere_gpu_ir::TextureFormat::Rgba8Unorm
            || desc.depth != 1
            || desc.layers != 1
            || desc.mip_levels != 1
            || desc.samples != 1
            || desc.layout != TextureLayout::Linear
            || pixels.len() as u64 != size
        {
            return Err(VulkanError::UnsupportedFeature(
                "RGBA8 initialization descriptor",
            ));
        }
        if self.images.contains_key(&desc.id) {
            if self.rgba8_readback_extent(desc.id)? != (desc.width, desc.height) {
                return Err(VulkanError::UnsupportedFeature(
                    "RGBA8 initialization shape mismatch",
                ));
            }
            return self.upload_rgba8(desc.id, pixels);
        }
        if !self.pending_writebacks.is_empty() {
            return Err(VulkanError::UnsupportedFeature(
                "image initialization with pending writeback",
            ));
        }
        let mut resources = exosphere_gpu_ir::InMemoryGpuResources::new();
        resources.insert(desc.id, pixels.to_vec(), false);
        self.ensure_image(desc, &mut resources, false)
    }

    /// Replace a completed RGBA8 image with exact tightly packed pixels. Pending
    /// deferred writebacks must be completed first, not overwritten by a refresh.
    pub fn upload_rgba8(&mut self, id: ResourceId, pixels: &[u8]) -> Result<(), VulkanError> {
        let (width, height) = self.rgba8_readback_extent(id)?;
        let size = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|size| size.checked_mul(4))
            .filter(|size| *size > 0 && *size <= 64 * 1024 * 1024)
            .ok_or(VulkanError::UnsupportedFeature("RGBA8 upload size budget"))?;
        if pixels.len() as u64 != size {
            return Err(VulkanError::ResourceData(
                "RGBA8 upload size mismatch".into(),
            ));
        }
        if !self.pending_writebacks.is_empty() {
            return Err(VulkanError::UnsupportedFeature(
                "image upload with pending writeback",
            ));
        }
        let desc = TextureDesc {
            id,
            width,
            height,
            depth: 1,
            layers: 1,
            mip_levels: 1,
            samples: 1,
            format: exosphere_gpu_ir::TextureFormat::Rgba8Unorm,
            layout: TextureLayout::Linear,
        };
        self.upload_image_bytes(&desc, pixels)
    }

    /// Shape of the actual image eligible for RGBA8 readback, not caller-provided
    /// metadata. Allows guest bridges to reject incompatible dimensions first.
    pub fn rgba8_readback_extent(&self, id: ResourceId) -> Result<(u32, u32), VulkanError> {
        let image = self
            .images
            .get(&id)
            .ok_or(VulkanError::MissingResource(id.0))?;
        if image.format != vk::Format::R8G8B8A8_UNORM
            || image.extent.depth != 1
            || image.layers != 1
            || image.mip_levels != 1
            || image.samples != vk::SampleCountFlags::TYPE_1
            || image.layout == vk::ImageLayout::UNDEFINED
        {
            return Err(VulkanError::UnsupportedFeature(
                "RGBA8 readback image shape",
            ));
        }
        Ok((image.extent.width, image.extent.height))
    }

    /// Read the actual device image after previously submitted work. This does
    /// not provide a swapchain or write pixels into guest memory.
    pub fn readback_rgba8(&mut self, id: ResourceId) -> Result<Vec<u8>, VulkanError> {
        self.rgba8_readback_extent(id)?;
        let image = self
            .images
            .get(&id)
            .ok_or(VulkanError::MissingResource(id.0))?;
        let size = u64::from(image.extent.width)
            .checked_mul(u64::from(image.extent.height))
            .and_then(|size| size.checked_mul(4))
            .filter(|size| *size > 0 && *size <= 64 * 1024 * 1024)
            .ok_or(VulkanError::UnsupportedFeature(
                "RGBA8 readback size budget",
            ))?;
        let handle = image.image;
        let extent = image.extent;
        let old_layout = image.layout;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size as usize)
            .map_err(|_| VulkanError::Driver("readback allocation failed".into()))?;
        bytes.resize(size as usize, 0);
        let mut staging = PendingBufferAllocation {
            device: self.device.clone(),
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            mapped: false,
        };
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        staging.buffer = unsafe { self.device.create_buffer(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("readback buffer: {e:?}")))?;
        let requirements = unsafe { self.device.get_buffer_memory_requirements(staging.buffer) };
        let memory_type = self
            .memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                vk::MemoryPropertyFlags::empty(),
            )
            .ok_or_else(|| VulkanError::Driver("readback needs host-coherent memory".into()))?;
        let allocation = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type);
        staging.memory = unsafe { self.device.allocate_memory(&allocation, None) }
            .map_err(|e| VulkanError::Driver(format!("readback memory: {e:?}")))?;
        unsafe {
            self.device
                .bind_buffer_memory(staging.buffer, staging.memory, 0)
        }
        .map_err(|e| VulkanError::Driver(format!("readback bind: {e:?}")))?;
        let command = self.begin_commands()?;
        self.transition_image(command, id, vk::ImageLayout::TRANSFER_SRC_OPTIMAL)?;
        let region = [vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(extent)];
        unsafe {
            self.device.cmd_copy_image_to_buffer(
                command,
                handle,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                staging.buffer,
                &region,
            )
        };
        self.transition_image(command, id, old_layout)?;
        let barrier = [vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .buffer(staging.buffer)
            .size(size)];
        unsafe {
            self.device.cmd_pipeline_barrier2(
                command,
                &vk::DependencyInfo::default().buffer_memory_barriers(&barrier),
            )
        };
        let mut staging = self
            .submit_and_wait_with_staging(command, Some(staging))?
            .ok_or_else(|| VulkanError::Driver("readback staging ownership missing".into()))?;
        let mapped = unsafe {
            self.device
                .map_memory(staging.memory, 0, size, vk::MemoryMapFlags::empty())
        }
        .map_err(|e| VulkanError::Driver(format!("readback map: {e:?}")))?;
        staging.mapped = true;
        bytes.copy_from_slice(unsafe {
            std::slice::from_raw_parts(mapped.cast::<u8>(), size as usize)
        });
        unsafe { self.device.unmap_memory(staging.memory) };
        staging.mapped = false;
        Ok(bytes)
    }
}
