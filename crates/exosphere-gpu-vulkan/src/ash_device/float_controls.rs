// SPDX-License-Identifier: GPL-3.0-or-later
//! Device gates for explicit SPIR-V floating-point execution modes.

use crate::VulkanError;
use ash::vk;

pub(super) fn validate(
    words: &[u32],
    properties: &vk::PhysicalDeviceFloatControlsProperties<'_>,
) -> Result<(), VulkanError> {
    if words.len() < 5 {
        return Err(VulkanError::UnsupportedFeature("truncated shader header"));
    }
    let mut offset = 5;
    let mut requested = std::collections::BTreeMap::new();
    while offset < words.len() {
        let length = (words[offset] >> 16) as usize;
        let end = offset
            .checked_add(length)
            .filter(|end| length != 0 && *end <= words.len())
            .ok_or(VulkanError::UnsupportedFeature(
                "malformed shader instruction length",
            ))?;
        if words[offset] & 0xffff == 16 && length >= 3 {
            let mode = words[offset + 2];
            if (4459..=4463).contains(&mode) {
                if length != 4 {
                    return Err(VulkanError::UnsupportedFeature(
                        "malformed shader float execution mode",
                    ));
                }
                let supported = match (mode, words[offset + 3]) {
                    (4459, 16) => properties.shader_denorm_preserve_float16,
                    (4459, 32) => properties.shader_denorm_preserve_float32,
                    (4459, 64) => properties.shader_denorm_preserve_float64,
                    (4460, 16) => properties.shader_denorm_flush_to_zero_float16,
                    (4460, 32) => properties.shader_denorm_flush_to_zero_float32,
                    (4460, 64) => properties.shader_denorm_flush_to_zero_float64,
                    (4461, 16) => properties.shader_signed_zero_inf_nan_preserve_float16,
                    (4461, 32) => properties.shader_signed_zero_inf_nan_preserve_float32,
                    (4461, 64) => properties.shader_signed_zero_inf_nan_preserve_float64,
                    (4462, 16) => properties.shader_rounding_mode_rte_float16,
                    (4462, 32) => properties.shader_rounding_mode_rte_float32,
                    (4462, 64) => properties.shader_rounding_mode_rte_float64,
                    (4463, 16) => properties.shader_rounding_mode_rtz_float16,
                    (4463, 32) => properties.shader_rounding_mode_rtz_float32,
                    (4463, 64) => properties.shader_rounding_mode_rtz_float64,
                    _ => vk::FALSE,
                };
                if supported != vk::TRUE {
                    return Err(VulkanError::UnsupportedFeature(match mode {
                        4459 => "shader denorm preservation unavailable on device",
                        4460 => "shader denorm flush-to-zero unavailable on device",
                        4461 => "shader signed-zero/Inf/NaN preservation unavailable on device",
                        4462 => "shader round-to-nearest-even unavailable on device",
                        _ => "shader round-toward-zero unavailable on device",
                    }));
                }
                let entry = words[offset + 1];
                let width = words[offset + 3];
                let family = match mode {
                    4459 | 4460 => 0,
                    4462 | 4463 => 1,
                    _ => 2,
                };
                for (&(other_entry, other_width, other_family), &other_mode) in &requested {
                    if entry != other_entry || family != other_family || mode == other_mode {
                        continue;
                    }
                    let independence = if family == 0 {
                        properties.denorm_behavior_independence
                    } else {
                        properties.rounding_mode_independence
                    };
                    let independent_widths = independence
                        == vk::ShaderFloatControlsIndependence::ALL
                        || (independence == vk::ShaderFloatControlsIndependence::TYPE_32_ONLY
                            && (width == 32 || other_width == 32));
                    if width == other_width || !independent_widths {
                        return Err(VulkanError::UnsupportedFeature(
                            "conflicting shader float execution modes",
                        ));
                    }
                }
                requested.insert((entry, width, family), mode);
            }
        }
        offset = end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shader(mode: u32, width: u32) -> Vec<u32> {
        vec![
            0x07230203,
            0x00010000,
            0,
            2,
            0,
            (4 << 16) | 16,
            1,
            mode,
            width,
        ]
    }

    #[test]
    fn unavailable_float_modes_never_fall_back_to_device_defaults() {
        let unavailable = vk::PhysicalDeviceFloatControlsProperties::default();
        for mode in 4459..=4463 {
            for width in [16, 32, 64] {
                assert!(validate(&shader(mode, width), &unavailable).is_err());
            }
        }
        let available = vk::PhysicalDeviceFloatControlsProperties {
            shader_denorm_preserve_float32: vk::TRUE,
            shader_denorm_flush_to_zero_float32: vk::TRUE,
            shader_signed_zero_inf_nan_preserve_float32: vk::TRUE,
            shader_rounding_mode_rte_float32: vk::TRUE,
            shader_rounding_mode_rtz_float32: vk::TRUE,
            ..Default::default()
        };
        for mode in 4459..=4463 {
            assert!(validate(&shader(mode, 32), &available).is_ok());
            for width in [0, 16, 64, 128] {
                assert!(validate(&shader(mode, width), &available).is_err());
            }
        }
        assert!(validate(&shader(7, 32), &unavailable).is_ok());
        let mut conflicting = shader(4459, 32);
        conflicting.extend_from_slice(&shader(4460, 32)[5..]);
        assert!(validate(&conflicting, &available).is_err());
        let mut conflicting = shader(4462, 32);
        conflicting.extend_from_slice(&shader(4463, 32)[5..]);
        assert!(validate(&conflicting, &available).is_err());
        let mut malformed = shader(4459, 32);
        malformed[5] = (3 << 16) | 16;
        assert!(validate(&malformed, &available).is_err());
        for instruction in [0, (5 << 16) | 16, 16] {
            malformed[5] = instruction;
            assert!(validate(&malformed, &available).is_err());
        }
    }
}
