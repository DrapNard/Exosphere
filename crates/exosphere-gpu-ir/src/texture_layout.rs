// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Texture/block layout metadata shared by resource discovery and host backends.
//!
//! This deliberately separates *footprint* calculations from exact Tegra block-linear address
//! swizzling.  Footprints are safe to derive from block dimensions; byte-address swizzling remains
//! a dedicated backend/decoder concern until every Maxwell layout bit has been validated.

use crate::resource::TextureFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatInfo {
    pub block_width: u8,
    pub block_height: u8,
    pub bytes_per_block: u8,
    pub depth: bool,
    pub stencil: bool,
    pub compressed: bool,
}

pub fn format_info(format: TextureFormat) -> Option<FormatInfo> {
    use TextureFormat::*;
    let plain = |bytes: u8| FormatInfo {
        block_width: 1,
        block_height: 1,
        bytes_per_block: bytes,
        depth: false,
        stencil: false,
        compressed: false,
    };
    let compressed = |w: u8, h: u8, bytes: u8| FormatInfo {
        block_width: w,
        block_height: h,
        bytes_per_block: bytes,
        depth: false,
        stencil: false,
        compressed: true,
    };
    Some(match format {
        R8Unorm | R8Uint | R8Sint => plain(1),
        Rg8Unorm | Rg8Uint | Rg8Sint | R16Float => plain(2),
        Rgba8Unorm | Rgba8Srgb | Bgra8Unorm | Bgra8Srgb | Rg16Float | R32Float | R32Uint
        | R32Sint => plain(4),
        Rgba16Float | Rg32Float => plain(8),
        Rgba32Float => plain(16),
        D16Unorm => FormatInfo {
            depth: true,
            ..plain(2)
        },
        D24UnormS8 => FormatInfo {
            depth: true,
            stencil: true,
            ..plain(4)
        },
        D32Float => FormatInfo {
            depth: true,
            ..plain(4)
        },
        D32FloatS8 => FormatInfo {
            depth: true,
            stencil: true,
            ..plain(8)
        },
        Bc1RgbaUnorm | Bc1RgbaSrgb | Bc4Unorm | Bc4Snorm => compressed(4, 4, 8),
        Bc2RgbaUnorm | Bc2RgbaSrgb | Bc3RgbaUnorm | Bc3RgbaSrgb | Bc5Unorm | Bc5Snorm
        | Bc6hUfloat | Bc6hSfloat | Bc7RgbaUnorm | Bc7RgbaSrgb => compressed(4, 4, 16),
        Astc4x4Unorm => compressed(4, 4, 16),
        Astc5x4Unorm => compressed(5, 4, 16),
        Astc5x5Unorm => compressed(5, 5, 16),
        Astc6x5Unorm => compressed(6, 5, 16),
        Astc6x6Unorm => compressed(6, 6, 16),
        Astc8x5Unorm => compressed(8, 5, 16),
        Astc8x6Unorm => compressed(8, 6, 16),
        Astc8x8Unorm => compressed(8, 8, 16),
        Astc10x5Unorm => compressed(10, 5, 16),
        Astc10x6Unorm => compressed(10, 6, 16),
        Astc10x8Unorm => compressed(10, 8, 16),
        Astc10x10Unorm => compressed(10, 10, 16),
        Astc12x10Unorm => compressed(12, 10, 16),
        Astc12x12Unorm => compressed(12, 12, 16),
        Unknown(_) => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureFootprint {
    pub blocks_x: u32,
    pub blocks_y: u32,
    pub row_bytes: u64,
    pub layer_bytes: u64,
    pub total_bytes: u64,
}

pub fn mip_extent(value: u32, level: u32) -> u32 {
    (value >> level).max(1)
}

pub fn linear_footprint(
    format: TextureFormat,
    width: u32,
    height: u32,
    depth: u32,
    layers: u32,
) -> Option<TextureFootprint> {
    let info = format_info(format)?;
    let blocks_x = div_ceil(width.max(1), u32::from(info.block_width));
    let blocks_y = div_ceil(height.max(1), u32::from(info.block_height));
    let row_bytes = u64::from(blocks_x) * u64::from(info.bytes_per_block);
    let layer_bytes = row_bytes
        .checked_mul(u64::from(blocks_y))?
        .checked_mul(u64::from(depth.max(1)))?;
    let total_bytes = layer_bytes.checked_mul(u64::from(layers.max(1)))?;
    Some(TextureFootprint {
        blocks_x,
        blocks_y,
        row_bytes,
        layer_bytes,
        total_bytes,
    })
}

fn div_ceil(value: u32, divisor: u32) -> u32 {
    value.saturating_add(divisor - 1) / divisor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_bc1_footprint_in_blocks() {
        let footprint = linear_footprint(TextureFormat::Bc1RgbaUnorm, 7, 5, 1, 1).unwrap();
        assert_eq!(footprint.blocks_x, 2);
        assert_eq!(footprint.blocks_y, 2);
        assert_eq!(footprint.row_bytes, 16);
        assert_eq!(footprint.total_bytes, 32);
    }
}
