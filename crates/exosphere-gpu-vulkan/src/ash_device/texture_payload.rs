// SPDX-License-Identifier: GPL-3.0-or-later
//! Admission of guest texture payloads before staging allocation.

use exosphere_gpu_ir::ResourceDataError;

use crate::VulkanError;

pub(super) fn copy_regions(
    desc: &exosphere_gpu_ir::resource::TextureDesc,
    length: u64,
    aspect: ash::vk::ImageAspectFlags,
) -> Result<Vec<ash::vk::BufferImageCopy>, VulkanError> {
    use ash::vk;
    use exosphere_gpu_ir::texture_layout::{linear_footprint, mip_extent};
    super::image_usage::validate_shape(desc)?;
    let mut regions = Vec::new();
    let mut offset = 0u64;
    for mip in 0..desc.mip_levels {
        let width = mip_extent(desc.width, mip);
        let height = mip_extent(desc.height, mip);
        let depth = mip_extent(desc.depth, mip);
        let fp = linear_footprint(desc.format, width, height, depth, desc.layers)
            .ok_or(VulkanError::UnsupportedFeature("mip footprint"))?;
        let end = offset
            .checked_add(fp.total_bytes)
            .ok_or(VulkanError::UnsupportedFeature("mip payload size overflow"))?;
        if end > length {
            return Err(VulkanError::ResourceData(format!(
                "texture payload ends inside mip {mip}"
            )));
        }
        regions.push(
            vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: aspect,
                    mip_level: mip,
                    base_array_layer: 0,
                    layer_count: desc.layers,
                })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth,
                }),
        );
        offset = end;
    }
    Ok(regions)
}

pub(super) fn upload_length(
    length: Result<u64, ResourceDataError>,
    required: bool,
) -> Result<usize, VulkanError> {
    let length = match length {
        Ok(length) => length,
        // Render targets can be created without initial host pixels. A sampled
        // guest input cannot: hiding a failed lookup would sample undefined data.
        Err(ResourceDataError::Missing(_)) if !required => return Ok(0),
        Err(error) => return Err(VulkanError::ResourceData(error.to_string())),
    };
    if required && length == 0 {
        return Err(VulkanError::ResourceData(
            "sampled texture payload is empty".into(),
        ));
    }
    usize::try_from(length).map_err(|_| {
        VulkanError::ResourceData("texture payload length exceeds host address space".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_gpu_ir::ResourceId;

    #[test]
    fn copy_preflight_checks_every_mip_and_preserves_packed_offsets() {
        use ash::vk;
        let mut desc = exosphere_gpu_ir::GpuModule::triangle_vulkan_smoke_test(4, 4)
            .textures
            .remove(0);
        desc.mip_levels = 3;
        let regions = copy_regions(&desc, 84, vk::ImageAspectFlags::COLOR).unwrap();
        assert_eq!(
            regions.iter().map(|r| r.buffer_offset).collect::<Vec<_>>(),
            [0, 64, 80]
        );
        assert_eq!(
            regions
                .iter()
                .map(|r| r.image_extent.width)
                .collect::<Vec<_>>(),
            [4, 2, 1]
        );
        for length in [0, 63, 64, 79, 80, 83] {
            assert!(copy_regions(&desc, length, vk::ImageAspectFlags::COLOR).is_err());
        }
        desc.layers = 2;
        let regions = copy_regions(&desc, 168, vk::ImageAspectFlags::COLOR).unwrap();
        assert_eq!(
            regions.iter().map(|r| r.buffer_offset).collect::<Vec<_>>(),
            [0, 128, 160]
        );
        assert!(regions.iter().all(|r| r.image_subresource.layer_count == 2));
    }

    #[test]
    fn missing_initial_pixels_are_optional_only_for_outputs() {
        let missing = ResourceDataError::Missing(ResourceId(7));
        assert_eq!(upload_length(Err(missing.clone()), false), Ok(0));
        assert!(matches!(
            upload_length(Err(missing), true),
            Err(VulkanError::ResourceData(_))
        ));
        assert_eq!(upload_length(Ok(0), false), Ok(0));
        assert!(upload_length(Ok(0), true).is_err());
        assert_eq!(upload_length(Ok(16), true), Ok(16));
    }

    #[test]
    fn provider_errors_are_never_optional() {
        for required in [false, true] {
            assert_eq!(
                upload_length(Err(ResourceDataError::SizeOverflow), required),
                Err(VulkanError::ResourceData(
                    ResourceDataError::SizeOverflow.to_string()
                ))
            );
        }
    }
}
