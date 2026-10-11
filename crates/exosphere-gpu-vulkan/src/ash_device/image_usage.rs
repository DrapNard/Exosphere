// SPDX-License-Identifier: GPL-3.0-or-later
//! Image usages are limited by optimal-tiling format capabilities, not by
//! whether a format happens to be a color format (compressed and sRGB differ).

use crate::VulkanError;
use ash::vk;

/// Validate canonical dimensions before allocating or consulting the image cache.
/// Zero is not a default: replacing it with one changes the guest resource.
pub(super) fn validate_shape(
    desc: &exosphere_gpu_ir::resource::TextureDesc,
) -> Result<(), VulkanError> {
    if [
        desc.width,
        desc.height,
        desc.depth,
        desc.layers,
        desc.mip_levels,
    ]
    .contains(&0)
        || !matches!(desc.samples, 1 | 2 | 4 | 8 | 16 | 32 | 64)
    {
        return Err(VulkanError::UnsupportedFeature(
            "invalid image dimensions or samples",
        ));
    }
    // A complete mip chain ends at 1x1x1. Integer arithmetic also handles
    // u32::MAX without rounding or overflowing a power-of-two calculation.
    let max_mips = u32::BITS - desc.width.max(desc.height).max(desc.depth).leading_zeros();
    if desc.mip_levels > max_mips
        || (desc.depth > 1 && desc.layers != 1)
        || (desc.samples > 1 && (desc.depth != 1 || desc.mip_levels != 1))
    {
        return Err(VulkanError::UnsupportedFeature(
            "invalid image shape combination",
        ));
    }
    Ok(())
}

pub(super) fn supported(
    features: vk::FormatFeatureFlags,
) -> Result<vk::ImageUsageFlags, VulkanError> {
    let mut usage = vk::ImageUsageFlags::empty();
    for (feature, flag) in [
        (
            vk::FormatFeatureFlags::TRANSFER_SRC,
            vk::ImageUsageFlags::TRANSFER_SRC,
        ),
        (
            vk::FormatFeatureFlags::TRANSFER_DST,
            vk::ImageUsageFlags::TRANSFER_DST,
        ),
        (
            vk::FormatFeatureFlags::SAMPLED_IMAGE,
            vk::ImageUsageFlags::SAMPLED,
        ),
        (
            vk::FormatFeatureFlags::STORAGE_IMAGE,
            vk::ImageUsageFlags::STORAGE,
        ),
        (
            vk::FormatFeatureFlags::COLOR_ATTACHMENT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT,
        ),
        (
            vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        ),
    ] {
        if features.contains(feature) {
            usage |= flag;
        }
    }
    // Staging, readback and descriptors currently rely on these three usages.
    require(
        usage,
        vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST
            | vk::ImageUsageFlags::SAMPLED,
    )?;
    Ok(usage)
}

pub(super) fn require(
    actual: vk::ImageUsageFlags,
    required: vk::ImageUsageFlags,
) -> Result<(), VulkanError> {
    if actual.contains(required) {
        Ok(())
    } else {
        Err(VulkanError::UnsupportedFeature(
            "image format does not support requested usage",
        ))
    }
}

pub(super) fn validate_compression(
    format: vk::Format,
    enabled: &vk::PhysicalDeviceFeatures,
) -> Result<(), VulkanError> {
    let raw = format.as_raw();
    if (vk::Format::BC1_RGB_UNORM_BLOCK.as_raw()..=vk::Format::BC7_SRGB_BLOCK.as_raw())
        .contains(&raw)
        && enabled.texture_compression_bc != vk::TRUE
    {
        return Err(VulkanError::UnsupportedFeature("BC texture compression"));
    }
    if (vk::Format::ASTC_4X4_UNORM_BLOCK.as_raw()..=vk::Format::ASTC_12X12_SRGB_BLOCK.as_raw())
        .contains(&raw)
        && enabled.texture_compression_astc_ldr != vk::TRUE
    {
        return Err(VulkanError::UnsupportedFeature(
            "ASTC LDR texture compression",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_image_shape_never_defaults_zero_or_extends_mip_chains() {
        let base = exosphere_gpu_ir::GpuModule::triangle_vulkan_smoke_test(4, 4)
            .textures
            .remove(0);
        assert!(validate_shape(&base).is_ok());
        for field in 0..6 {
            let mut desc = base.clone();
            match field {
                0 => desc.width = 0,
                1 => desc.height = 0,
                2 => desc.depth = 0,
                3 => desc.layers = 0,
                4 => desc.mip_levels = 0,
                _ => desc.samples = 0,
            }
            assert!(validate_shape(&desc).is_err(), "zero field {field}");
        }
        let mut desc = base;
        desc.mip_levels = 3;
        assert!(validate_shape(&desc).is_ok());
        desc.mip_levels = 4;
        assert!(validate_shape(&desc).is_err());
        desc.width = u32::MAX;
        desc.mip_levels = 32;
        assert!(validate_shape(&desc).is_ok());
        desc.mip_levels = 33;
        assert!(validate_shape(&desc).is_err());
        desc.mip_levels = 1;
        desc.depth = 2;
        desc.layers = 2;
        assert!(validate_shape(&desc).is_err());
        desc.layers = 1;
        assert!(validate_shape(&desc).is_ok());
        desc.samples = 4;
        assert!(validate_shape(&desc).is_err());
        desc.depth = 1;
        assert!(validate_shape(&desc).is_ok());
        desc.mip_levels = 2;
        assert!(validate_shape(&desc).is_err());
        desc.mip_levels = 1;
        desc.samples = 3;
        assert!(validate_shape(&desc).is_err());
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_initial_payload_failure_rolls_back_image_but_refresh_preserves_it() {
        use exosphere_gpu_ir::{GpuModule, InMemoryGpuResources};
        let mut device = super::super::AshVulkanDevice::create().unwrap();
        let desc = GpuModule::triangle_vulkan_smoke_test(4, 4)
            .textures
            .remove(0);
        let mut resources = InMemoryGpuResources::new();
        // Missing, empty and truncated initial payloads each allocate a native
        // image but must remove it again before reporting the original error.
        assert!(device.ensure_image(&desc, &mut resources, true).is_err());
        assert!(!device.images.contains_key(&desc.id));
        for length in [0, 1, 63] {
            resources.insert(desc.id, vec![0; length], false);
            assert!(device.ensure_image(&desc, &mut resources, true).is_err());
            assert!(!device.images.contains_key(&desc.id));
            assert!(!device.submission_failed);
        }
        let pixels = [17, 43, 91, 255].repeat(16);
        resources.insert(desc.id, pixels.clone(), false);
        device.ensure_image(&desc, &mut resources, true).unwrap();
        let handle = device.images[&desc.id].image;
        let layout = device.images[&desc.id].layout;
        assert_eq!(device.readback_rgba8(desc.id).unwrap(), pixels);
        resources.insert(desc.id, vec![0; 63], false);
        assert!(device.ensure_image(&desc, &mut resources, true).is_err());
        assert_eq!(device.images[&desc.id].image, handle);
        assert_eq!(device.images[&desc.id].layout, layout);
        assert_eq!(device.readback_rgba8(desc.id).unwrap(), pixels);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_failed_wait_retains_command_fence_and_staging_until_teardown() {
        use super::super::PendingBufferAllocation;
        let mut device = super::super::AshVulkanDevice::create().unwrap();
        let command = device.begin_commands().unwrap();
        let fence = unsafe {
            device
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
        }
        .unwrap();
        let buffer = unsafe {
            device.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(16)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC),
                None,
            )
        }
        .unwrap();
        let staging = PendingBufferAllocation {
            device: device.device.clone(),
            buffer,
            memory: vk::DeviceMemory::null(),
            mapped: false,
        };
        unsafe {
            device.device.end_command_buffer(command).unwrap();
            let commands = [command];
            let submits = [vk::SubmitInfo::default().command_buffers(&commands)];
            device
                .device
                .queue_submit(device.queue, &submits, fence)
                .unwrap();
            device
                .device
                .wait_for_fences(&[fence], true, u64::MAX)
                .unwrap();
        }
        // Inject the host result after a real, completed empty submission.
        // This tests ownership policy, not an actual driver/device-loss fault.
        assert!(
            device
                .finish_texture_upload_wait(
                    command,
                    fence,
                    Some(staging),
                    Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY)
                )
                .is_err()
        );
        assert!(device.submission_failed);
        assert_eq!(device.retained_texture_uploads.len(), 1);
        let retained = &device.retained_texture_uploads[0];
        assert_eq!(retained.command, command);
        assert_eq!(retained.fence, fence);
        assert_eq!(retained._staging.as_ref().unwrap().buffer, buffer);
        assert!(unsafe { device.device.get_fence_status(fence) }.unwrap());
        assert!(device.begin_commands().is_err());
        // Drop waits idle and drains all retained objects before VkDevice.
        drop(device);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_invalid_image_shape_never_enters_cache_and_id_can_recover() {
        use exosphere_gpu_ir::{GpuModule, InMemoryGpuResources};
        let mut device = super::super::AshVulkanDevice::create().unwrap();
        let base = GpuModule::triangle_vulkan_smoke_test(4, 4)
            .textures
            .remove(0);
        let mut resources = InMemoryGpuResources::new();
        for field in 0..9 {
            let mut desc = base.clone();
            match field {
                0 => desc.width = 0,
                1 => desc.height = 0,
                2 => desc.depth = 0,
                3 => desc.layers = 0,
                4 => desc.mip_levels = 0,
                5 => desc.samples = 0,
                6 => desc.mip_levels = 4,
                7 => {
                    desc.depth = 2;
                    desc.layers = 2;
                }
                _ => {
                    desc.samples = 4;
                    desc.mip_levels = 2;
                }
            }
            // No payload exists. Shape errors must win before a provider read
            // or native allocation publishes a misleading cached identity.
            assert_eq!(
                device.ensure_image(&desc, &mut resources, true),
                validate_shape(&desc)
            );
            assert!(!device.images.contains_key(&desc.id));
        }
        resources.insert(base.id, vec![0; 4 * 4 * 4], false);
        device.ensure_image(&base, &mut resources, true).unwrap();
        assert_eq!(device.images[&base.id].descriptor, base);
        let original_layout = device.images[&base.id].layout;
        let original_image = device.images[&base.id].image;
        assert!(device.upload_image_bytes(&base, &[0; 63]).is_err());
        assert_eq!(device.images[&base.id].layout, original_layout);
        assert_eq!(device.images[&base.id].image, original_image);
        device.ensure_image(&base, &mut resources, true).unwrap();
        let mut invalid = base.clone();
        invalid.width = 0;
        assert_eq!(
            device.ensure_image(&invalid, &mut resources, true),
            validate_shape(&invalid)
        );
        assert_eq!(device.images[&base.id].descriptor, base);
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_bc_images_upload_or_reject_missing_compression_feature() {
        use exosphere_gpu_ir::{GpuModule, InMemoryGpuResources, ResourceId, TextureFormat};
        let mut device = super::super::AshVulkanDevice::create().unwrap();
        let mut desc = GpuModule::triangle_vulkan_smoke_test(4, 4)
            .textures
            .remove(0);
        let mut resources = InMemoryGpuResources::new();
        for (index, format) in [TextureFormat::Bc1RgbaUnorm, TextureFormat::Bc1RgbaSrgb]
            .into_iter()
            .enumerate()
        {
            desc.id = ResourceId(900 + index as u64);
            desc.format = format;
            // One BC1 block, all selectors zero: opaque red endpoint.
            resources.insert(desc.id, vec![0, 0xf8, 0, 0, 0, 0, 0, 0], false);
            let result = device.ensure_image(&desc, &mut resources, true);
            if device.core_features.texture_compression_bc != vk::TRUE {
                assert_eq!(
                    result,
                    Err(VulkanError::UnsupportedFeature("BC texture compression"))
                );
                assert!(!device.images.contains_key(&desc.id));
            } else {
                result.unwrap();
                let image = &device.images[&desc.id];
                assert!(image.usage.contains(vk::ImageUsageFlags::SAMPLED));
                assert!(!image.usage.intersects(
                    vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::STORAGE
                ));
                assert_eq!(image.layout, vk::ImageLayout::GENERAL);
            }
        }
    }

    #[test]
    fn sampled_only_formats_never_gain_attachment_or_storage_usage() {
        let features = vk::FormatFeatureFlags::TRANSFER_SRC
            | vk::FormatFeatureFlags::TRANSFER_DST
            | vk::FormatFeatureFlags::SAMPLED_IMAGE;
        let usage = supported(features).unwrap();
        assert!(
            !usage.intersects(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::STORAGE)
        );
        assert!(require(usage, vk::ImageUsageFlags::STORAGE).is_err());
        assert!(supported(vk::FormatFeatureFlags::SAMPLED_IMAGE).is_err());
        assert!(
            supported(features | vk::FormatFeatureFlags::COLOR_ATTACHMENT)
                .unwrap()
                .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        );
    }

    #[test]
    fn compression_requires_enabled_family_feature() {
        let disabled = vk::PhysicalDeviceFeatures::default();
        assert!(validate_compression(vk::Format::BC1_RGBA_UNORM_BLOCK, &disabled).is_err());
        assert!(validate_compression(vk::Format::ASTC_4X4_UNORM_BLOCK, &disabled).is_err());
        assert!(validate_compression(vk::Format::R8G8B8A8_UNORM, &disabled).is_ok());
        let bc = disabled.texture_compression_bc(true);
        assert!(validate_compression(vk::Format::BC7_SRGB_BLOCK, &bc).is_ok());
        assert!(validate_compression(vk::Format::ASTC_4X4_UNORM_BLOCK, &bc).is_err());
    }
}
