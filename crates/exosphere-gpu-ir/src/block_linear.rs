// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Tegra X1 block-linear byte addressing.
//!
//! Maxwell/Tegra surfaces are organized in 64x8-byte GOBs (512 bytes).  GOBs are grouped into
//! blocks whose height/depth are powers of two.  This module intentionally operates on **byte
//! coordinates**, not pixels: callers first convert their texture format to byte/block dimensions.
//! That keeps compressed and uncompressed formats on the same addressing path.

/// One Tegra GOB is 64 bytes wide and eight rows high.
pub const GOB_WIDTH_BYTES: u32 = 64;
pub const GOB_HEIGHT: u32 = 8;
pub const GOB_SIZE: u64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockLinearLayout {
    /// Surface width in bytes after format/block conversion.
    pub width_bytes: u32,
    /// Surface height in rows after format/block conversion.
    pub height_rows: u32,
    /// Surface depth in slices.
    pub depth: u32,
    /// log2(GOBs per block in Y).
    pub block_height_log2: u8,
    /// log2(GOBs per block in Z).
    pub block_depth_log2: u8,
}

impl BlockLinearLayout {
    pub fn new(
        width_bytes: u32,
        height_rows: u32,
        depth: u32,
        block_height_log2: u8,
        block_depth_log2: u8,
    ) -> Self {
        Self {
            width_bytes: width_bytes.max(1),
            height_rows: height_rows.max(1),
            depth: depth.max(1),
            block_height_log2: block_height_log2.min(5),
            block_depth_log2: block_depth_log2.min(5),
        }
    }

    pub fn block_height_gobs(self) -> u32 {
        1u32 << self.block_height_log2
    }
    pub fn block_depth_slices(self) -> u32 {
        1u32 << self.block_depth_log2
    }

    /// Number of bytes needed for the tiled surface including block/GOB padding.
    pub fn storage_size(self) -> u64 {
        let width_gobs = div_ceil(self.width_bytes, GOB_WIDTH_BYTES);
        let height_gobs = div_ceil(self.height_rows, GOB_HEIGHT);
        let block_h = self.block_height_gobs();
        let padded_h = align_up(height_gobs, block_h);
        let block_d = self.block_depth_slices();
        let padded_d = align_up(self.depth, block_d);
        u64::from(width_gobs) * u64::from(padded_h) * u64::from(padded_d) * GOB_SIZE
    }

    /// Translate a byte coordinate `(x, y, z)` into a Tegra block-linear byte offset.
    ///
    /// The intra-GOB swizzle follows the Tegra X1 GOB layout. Coordinates outside the declared
    /// logical surface return `None`; padded storage is deliberately not exposed as logical data.
    pub fn offset(self, x_byte: u32, y: u32, z: u32) -> Option<u64> {
        if x_byte >= self.width_bytes || y >= self.height_rows || z >= self.depth {
            return None;
        }

        let width_gobs = div_ceil(self.width_bytes, GOB_WIDTH_BYTES);
        let block_h = self.block_height_gobs();
        let block_d = self.block_depth_slices();

        let gob_x = x_byte / GOB_WIDTH_BYTES;
        let gob_y = y / GOB_HEIGHT;
        let gob_z = z;

        let block_y = gob_y / block_h;
        let in_block_gob_y = gob_y % block_h;
        let block_z = gob_z / block_d;
        let in_block_z = gob_z % block_d;

        let block_size = u64::from(block_h) * u64::from(block_d) * GOB_SIZE;
        let blocks_per_row = u64::from(width_gobs);
        let height_gobs = div_ceil(self.height_rows, GOB_HEIGHT);
        let block_rows = u64::from(div_ceil(height_gobs, block_h));

        let block_index = u64::from(gob_x)
            + u64::from(block_y) * blocks_per_row
            + u64::from(block_z) * blocks_per_row * block_rows;

        let gob_in_block = u64::from(in_block_gob_y) + u64::from(in_block_z) * u64::from(block_h);
        let gob_base = block_index * block_size + gob_in_block * GOB_SIZE;

        let x = x_byte & 0x3f;
        let y_in_gob = y & 0x7;
        // Tegra X1 GOB byte ordering (64x8 bytes): 32-byte sectors, 16-byte groups and 8-byte
        // rows are interleaved for locality. This is the byte-level form used by Switch tooling.
        let intra = ((x >> 5) as u64) * 256
            + ((y_in_gob >> 1) as u64) * 64
            + (((x >> 4) & 1) as u64) * 32
            + ((y_in_gob & 1) as u64) * 16
            + (x & 0xf) as u64;

        Some(gob_base + intra)
    }
}

pub fn align_up(value: u32, alignment: u32) -> u32 {
    debug_assert!(alignment.is_power_of_two());
    value.saturating_add(alignment - 1) & !(alignment - 1)
}

pub fn div_ceil(value: u32, divisor: u32) -> u32 {
    value.div_ceil(divisor)
}

/// Converts one or more array layers from Tegra block-linear storage into tightly-packed rows.
///
/// `width_bytes` and `height_rows` are already expressed in format blocks, so this works for both
/// ordinary and block-compressed textures. Each source layer starts at the padded block-linear
/// storage size; each destination layer is tightly packed as `width_bytes * height_rows * depth`.
pub fn deswizzle_layers(
    source: &[u8],
    width_bytes: u32,
    height_rows: u32,
    depth: u32,
    layers: u32,
    block_height_log2: u8,
    block_depth_log2: u8,
) -> Option<Vec<u8>> {
    let layout = BlockLinearLayout::new(
        width_bytes,
        height_rows,
        depth,
        block_height_log2,
        block_depth_log2,
    );
    let source_layer_bytes = usize::try_from(layout.storage_size()).ok()?;
    let linear_layer_bytes =
        usize::try_from(u64::from(width_bytes) * u64::from(height_rows) * u64::from(depth.max(1)))
            .ok()?;
    let layer_count = layers.max(1) as usize;
    let required = source_layer_bytes.checked_mul(layer_count)?;
    if source.len() < required {
        return None;
    }
    let mut output = vec![0u8; linear_layer_bytes.checked_mul(layer_count)?];
    for layer in 0..layer_count {
        let source_base = layer.checked_mul(source_layer_bytes)?;
        let dest_base = layer.checked_mul(linear_layer_bytes)?;
        for z in 0..depth.max(1) {
            for y in 0..height_rows {
                for x in 0..width_bytes {
                    let tiled = usize::try_from(layout.offset(x, y, z)?).ok()?;
                    let linear = usize::try_from(
                        (u64::from(z) * u64::from(height_rows) + u64::from(y))
                            * u64::from(width_bytes)
                            + u64::from(x),
                    )
                    .ok()?;
                    output[dest_base + linear] = source[source_base + tiled];
                }
            }
        }
    }
    Some(output)
}

/// Copy tightly packed 2D bytes into an existing tiled layer, preserving padding.
/// Validate all dimensions and slice sizes before modifying the destination.
pub fn swizzle_2d_into(
    source: &[u8],
    destination: &mut [u8],
    layout: BlockLinearLayout,
) -> Option<()> {
    if layout.depth != 1
        || layout.width_bytes == 0
        || layout.height_rows == 0
        || layout.block_height_log2 > 5
        || layout.block_depth_log2 != 0
    {
        return None;
    }
    let linear_size = u64::from(layout.width_bytes).checked_mul(u64::from(layout.height_rows))?;
    let height_gobs = u64::from(layout.height_rows).div_ceil(8);
    let block_height = 1u64 << layout.block_height_log2;
    let padded_height = height_gobs
        .div_ceil(block_height)
        .checked_mul(block_height)?;
    let storage_size = u64::from(layout.width_bytes)
        .div_ceil(64)
        .checked_mul(padded_height)?
        .checked_mul(GOB_SIZE)?;
    if source.len() as u64 != linear_size || (destination.len() as u64) < storage_size {
        return None;
    }
    // Every coordinate is in the validated logical extent; its offset lies in
    // the checked padded footprint. No destination bytes outside it are touched.
    for y in 0..layout.height_rows {
        for x in 0..layout.width_bytes {
            let linear = (u64::from(y) * u64::from(layout.width_bytes) + u64::from(x)) as usize;
            let tiled = layout.offset(x, y, 0)? as usize;
            destination[tiled] = source[linear];
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn swizzle_round_trip_preserves_padding_and_rejects_short_payloads() {
        let layout = super::BlockLinearLayout::new(65, 9, 1, 2, 0);
        let source: Vec<u8> = (0..65 * 9).map(|index| (index % 251) as u8).collect();
        let mut tiled = vec![0xa5; layout.storage_size() as usize];
        assert!(super::swizzle_2d_into(&source[..source.len() - 1], &mut tiled, layout).is_none());
        assert!(tiled.iter().all(|byte| *byte == 0xa5));
        super::swizzle_2d_into(&source, &mut tiled, layout).unwrap();
        let recovered = super::deswizzle_layers(&tiled, 65, 9, 1, 1, 2, 0).unwrap();
        assert_eq!(recovered, source);
        let mut used = vec![false; tiled.len()];
        for y in 0..9 {
            for x in 0..65 {
                used[layout.offset(x, y, 0).unwrap() as usize] = true;
            }
        }
        assert!(
            tiled
                .iter()
                .zip(used)
                .all(|(byte, used)| used || *byte == 0xa5)
        );
    }
    #[test]
    fn ceiling_division_does_not_underestimate_large_widths() {
        assert_eq!(super::div_ceil(u32::MAX, 64), 67_108_864);
        assert_eq!(super::div_ceil(u32::MAX - 15, 64), 67_108_864);
        assert_eq!(super::div_ceil(0, 64), 0);
    }
    use super::*;

    #[test]
    fn deswizzle_identity_sized_gob() {
        let source = (0..512).map(|v| (v & 0xff) as u8).collect::<Vec<_>>();
        let linear = deswizzle_layers(&source, 64, 8, 1, 1, 0, 0).unwrap();
        assert_eq!(linear.len(), 512);
        let layout = BlockLinearLayout::new(64, 8, 1, 0, 0);
        for y in 0..8 {
            for x in 0..64 {
                assert_eq!(
                    linear[(y * 64 + x) as usize],
                    source[layout.offset(x, y, 0).unwrap() as usize]
                );
            }
        }
    }

    #[test]
    fn gob_layout_has_expected_landmarks() {
        let layout = BlockLinearLayout::new(64, 8, 1, 0, 0);
        assert_eq!(layout.storage_size(), 512);
        assert_eq!(layout.offset(0, 0, 0), Some(0));
        assert_eq!(layout.offset(15, 0, 0), Some(15));
        assert_eq!(layout.offset(16, 0, 0), Some(32));
        assert_eq!(layout.offset(32, 0, 0), Some(256));
        assert_eq!(layout.offset(0, 1, 0), Some(16));
        assert_eq!(layout.offset(0, 2, 0), Some(64));
    }

    #[test]
    fn block_height_pads_storage() {
        let layout = BlockLinearLayout::new(64, 9, 1, 2, 0);
        assert_eq!(layout.storage_size(), 4 * GOB_SIZE);
    }
}
