// SPDX-License-Identifier: GPL-3.0-or-later
//! Validate buffer descriptors before handing them to the Vulkan driver.

use crate::VulkanError;
use ash::vk;
use exosphere_gpu_ir::{BufferUsage, DescriptorType};

pub(super) fn validate(
    descriptor: DescriptorType,
    usage: BufferUsage,
    size: u64,
    offset: u64,
    range: u64,
    limits: &vk::PhysicalDeviceLimits,
) -> Result<u64, VulkanError> {
    let (expected_usage, alignment, maximum) = match descriptor {
        DescriptorType::UniformBuffer => (
            BufferUsage::Uniform,
            limits.min_uniform_buffer_offset_alignment,
            limits.max_uniform_buffer_range,
        ),
        DescriptorType::StorageBuffer => (
            BufferUsage::Storage,
            limits.min_storage_buffer_offset_alignment,
            limits.max_storage_buffer_range,
        ),
        _ => return Err(VulkanError::UnsupportedFeature("not a buffer descriptor")),
    };
    if usage != expected_usage
        && !(expected_usage == BufferUsage::Uniform && usage.supports_uniform())
        && !(expected_usage == BufferUsage::Index && usage.supports_index())
    {
        return Err(VulkanError::UnsupportedFeature(
            "buffer descriptor usage mismatch",
        ));
    }
    // Canonical range zero means the remaining allocation, not an empty binding.
    let range = if range == 0 {
        size.saturating_sub(offset)
    } else {
        range
    };
    if offset >= size || range == 0 || offset.checked_add(range).is_none_or(|end| end > size) {
        return Err(VulkanError::UnsupportedFeature(
            "buffer descriptor exceeds allocation",
        ));
    }
    if alignment == 0 || offset % alignment != 0 {
        return Err(VulkanError::UnsupportedFeature(
            "buffer descriptor offset alignment",
        ));
    }
    if range > u64::from(maximum) {
        return Err(VulkanError::UnsupportedFeature(
            "buffer descriptor device range limit",
        ));
    }
    Ok(range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_device_alignment_range_usage_and_allocation() {
        let limits = vk::PhysicalDeviceLimits {
            min_uniform_buffer_offset_alignment: 256,
            max_uniform_buffer_range: 1024,
            min_storage_buffer_offset_alignment: 16,
            max_storage_buffer_range: 4096,
            ..Default::default()
        };
        let uniform = |size, offset, range| {
            validate(
                DescriptorType::UniformBuffer,
                BufferUsage::Uniform,
                size,
                offset,
                range,
                &limits,
            )
        };
        assert_eq!(uniform(512, 256, 0).unwrap(), 256);
        assert_eq!(uniform(2048, 256, 1024).unwrap(), 1024);
        for (size, offset, range) in [
            (0, 0, 0),
            (512, 512, 0),
            (512, 513, 1),
            (512, 1, 4),
            (512, 256, 257),
            (2048, 0, 1025),
            (u64::MAX, 256, u64::MAX),
        ] {
            assert!(uniform(size, offset, range).is_err());
        }
        assert!(
            validate(
                DescriptorType::UniformBuffer,
                BufferUsage::Vertex,
                32,
                0,
                32,
                &limits
            )
            .is_err()
        );
        assert_eq!(
            validate(
                DescriptorType::StorageBuffer,
                BufferUsage::Storage,
                8192,
                16,
                4096,
                &limits
            )
            .unwrap(),
            4096
        );
        assert!(
            validate(
                DescriptorType::StorageBuffer,
                BufferUsage::Storage,
                8192,
                1,
                4096,
                &limits
            )
            .is_err()
        );
        assert!(
            validate(
                DescriptorType::StorageBuffer,
                BufferUsage::Storage,
                8192,
                16,
                4097,
                &limits
            )
            .is_err()
        );
    }
}
