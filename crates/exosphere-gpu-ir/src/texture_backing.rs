// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated guest storage for GM200 single-layer 2D TICs.
//!
//! This is storage discovery, not sampling-view validation. Swizzles, sRGB,
//! coordinate normalization and sampler controls still need joint preparation.

use crate::{
    GpuAddressError, GpuAddressSpace, ResolvedGpuRange, TextureDescriptor, TextureLayout,
    block_linear::BlockLinearLayout, texture_layout::linear_footprint,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureStorage {
    Pitch {
        row_bytes: u64,
        pitch_bytes: u64,
        rows: u32,
    },
    BlockLinear(BlockLinearLayout),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureBacking {
    pub descriptor: TextureDescriptor,
    pub range: ResolvedGpuRange,
    pub storage: TextureStorage,
    /// Explicit offsets for block-linear mip chains; empty for single levels.
    pub mip_levels: Vec<TextureMipLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureMipLevel {
    pub layout: BlockLinearLayout,
    pub physical_offset: u64,
    pub packed_offset: u64,
    pub packed_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureBackingError {
    UnsupportedHeader,
    UnsupportedView,
    UnsupportedLayout,
    UnknownFormat,
    InvalidPitch,
    Address(GpuAddressError),
    NotReadable,
}

/// Resolve the pixels, never the TIC allocation, and retain the allocation
/// offset for the eventual resource-provider view. Re-resolve on every draw so
/// VA remaps cannot reuse stale ownership. No guest-sized allocation is made.
/// Layout reference: deko3d source/maxwell/texture_image_control_block.h.
pub fn resolve_texture_backing(
    words: [u32; 8],
    space: &GpuAddressSpace,
) -> Result<TextureBacking, TextureBackingError> {
    use TextureBackingError as E;
    let header = (words[2] >> 21) & 7;
    if !matches!(header, 2 | 3) {
        return Err(E::UnsupportedHeader);
    }
    let layer_base =
        ((words[4] >> 16) & 7) | (((words[2] >> 16) & 31) << 3) | (((words[2] >> 29) & 7) << 8);
    let kind = (words[4] >> 23) & 15;
    let levels = (words[3] >> 28) + 1;
    // Other dimensions, partial mip views, multisampling, sparse storage and physical
    // borders change the footprint. Reject rather than sizing only level zero.
    if !matches!(kind, 1 | 7)
        || layer_base != 0
        || (levels > 1 && (header != 3 || kind != 1))
        || words[4] >> 29 != 7
        || words[5] & 0x7fff_0000 != 0
        || words[7] & 0xfff != (levels - 1) << 4
    {
        return Err(E::UnsupportedView);
    }
    let mut descriptor = crate::tic_tsc::decode_tic(words, None);
    let width = descriptor.width.ok_or(E::UnsupportedView)?;
    let height = descriptor.height.ok_or(E::UnsupportedView)?;
    if levels > u32::BITS - width.max(height).leading_zeros() {
        return Err(E::UnsupportedView);
    }
    let footprint = linear_footprint(
        descriptor.format,
        descriptor.width.ok_or(E::UnsupportedView)?,
        descriptor.height.ok_or(E::UnsupportedView)?,
        1,
        1,
    )
    .ok_or(E::UnknownFormat)?;
    let mut mip_levels = Vec::new();
    let (storage, size) = match descriptor.layout {
        TextureLayout::Linear => {
            if words[1] & 31 != 0 {
                return Err(E::UnsupportedLayout);
            }
            let pitch = u64::from(words[3] & 0xffff) << 5;
            if pitch < footprint.row_bytes {
                return Err(E::InvalidPitch);
            }
            // Include inter-row padding, but do not require unused trailing
            // padding after the final row to belong to the mapping.
            let size = pitch * u64::from(footprint.blocks_y - 1) + footprint.row_bytes;
            (
                TextureStorage::Pitch {
                    row_bytes: footprint.row_bytes,
                    pitch_bytes: pitch,
                    rows: footprint.blocks_y,
                },
                size,
            )
        }
        TextureLayout::BlockLinear {
            block_height_log2,
            block_depth_log2,
            ..
        } => {
            // The shared constructor clamps invalid exponents. Validate before
            // calling it; alternate tile widths/GOB-3D are not implemented.
            if block_height_log2 > 5
                || block_depth_log2 != 0
                || words[3] & 0x0000_fe07 != 0
                || words[1] & 511 != 0
            {
                return Err(E::UnsupportedLayout);
            }
            let layout = BlockLinearLayout::new(
                footprint
                    .row_bytes
                    .try_into()
                    .map_err(|_| E::UnsupportedLayout)?,
                footprint.blocks_y,
                1,
                block_height_log2,
                0,
            );
            if levels == 1 {
                (TextureStorage::BlockLinear(layout), layout.storage_size())
            } else {
                let mut physical_offset = 0u64;
                let mut packed_offset = 0u64;
                for level in 0..levels {
                    let fp = linear_footprint(
                        descriptor.format,
                        crate::texture_layout::mip_extent(width, level),
                        crate::texture_layout::mip_extent(height, level),
                        1,
                        1,
                    )
                    .ok_or(E::UnknownFormat)?;
                    // deko3d adjustTileSize shrinks the GOB height separately
                    // at each level, in compressed block rows rather than texels.
                    let mut block_height = block_height_log2;
                    while block_height > 0 && fp.blocks_y <= 8 << (block_height - 1) {
                        block_height -= 1;
                    }
                    let level_layout = BlockLinearLayout::new(
                        fp.row_bytes.try_into().map_err(|_| E::UnsupportedLayout)?,
                        fp.blocks_y,
                        1,
                        block_height,
                        0,
                    );
                    mip_levels.push(TextureMipLevel {
                        layout: level_layout,
                        physical_offset,
                        packed_offset,
                        packed_size: fp.total_bytes,
                    });
                    physical_offset = physical_offset
                        .checked_add(level_layout.storage_size())
                        .ok_or(E::UnsupportedLayout)?;
                    packed_offset = packed_offset
                        .checked_add(fp.total_bytes)
                        .ok_or(E::UnsupportedLayout)?;
                }
                (
                    TextureStorage::BlockLinear(mip_levels[0].layout),
                    physical_offset,
                )
            }
        }
        TextureLayout::Unknown(_) => return Err(E::UnsupportedLayout),
    };
    let range = space
        .resolve(descriptor.address.high_low(), size)
        .map_err(E::Address)?;
    if !range.permissions.read {
        return Err(E::NotReadable);
    }
    descriptor.resource = Some(range.resource);
    Ok(TextureBacking {
        descriptor,
        range,
        storage,
        mip_levels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GpuMapPermissions, GpuMapping, ResourceId};

    fn tic(header: u32) -> [u32; 8] {
        [
            0x08 | 2 << 7 | 2 << 10 | 2 << 13 | 2 << 16,
            0x1200,
            header << 21,
            if header == 2 { 1 } else { 0 },
            2 | 1 << 23 | 7 << 29,
            1 | 1 << 31,
            0,
            0,
        ]
    }

    fn space(size: u64, id: u64, permissions: GpuMapPermissions) -> GpuAddressSpace {
        let mut space = GpuAddressSpace::default();
        space
            .map(GpuMapping {
                guest_base: 0x1200,
                size,
                resource: ResourceId(id),
                resource_offset: 128,
                permissions,
            })
            .unwrap();
        space
    }

    #[test]
    fn mip_chain_shrinks_gob_height_and_requires_the_full_mapping() {
        let mut words = tic(3);
        words[3] = 6 << 28 | 3 << 3;
        words[4] = 63 | 1 << 23 | 7 << 29;
        words[5] = 63 | 1 << 31;
        words[7] = 6 << 4;
        let backing =
            resolve_texture_backing(words, &space(32768, 9, GpuMapPermissions::READ_ONLY)).unwrap();
        assert_eq!(
            backing
                .mip_levels
                .iter()
                .map(|m| m.layout.block_height_log2)
                .collect::<Vec<_>>(),
            [3, 2, 1, 0, 0, 0, 0]
        );
        assert_eq!(
            backing
                .mip_levels
                .iter()
                .map(|m| m.physical_offset)
                .collect::<Vec<_>>(),
            [0, 16384, 20480, 21504, 22016, 22528, 23040]
        );
        assert_eq!(backing.range.length, 23552);
        assert!(
            resolve_texture_backing(words, &space(23551, 9, GpuMapPermissions::READ_ONLY)).is_err()
        );
        words[7] = 5 << 4;
        assert!(
            resolve_texture_backing(words, &space(32768, 9, GpuMapPermissions::READ_ONLY)).is_err()
        );
    }

    #[test]
    fn pitch_resolves_exact_rows_and_real_suballocation() {
        let backing =
            resolve_texture_backing(tic(2), &space(44, 9, GpuMapPermissions::READ_ONLY)).unwrap();
        assert_eq!(backing.range.offset, 128);
        assert_eq!(backing.range.length, 44);
        assert_eq!(backing.descriptor.resource, Some(ResourceId(9)));
        assert_eq!(
            backing.storage,
            TextureStorage::Pitch {
                row_bytes: 12,
                pitch_bytes: 32,
                rows: 2
            }
        );
        assert!(matches!(
            resolve_texture_backing(tic(2), &space(43, 9, GpuMapPermissions::READ_ONLY)),
            Err(TextureBackingError::Address(_))
        ));
        let mut invalid = tic(2);
        invalid[3] = 0;
        assert_eq!(
            resolve_texture_backing(invalid, &space(44, 9, GpuMapPermissions::READ_ONLY)),
            Err(TextureBackingError::InvalidPitch)
        );
    }

    #[test]
    fn tiled_storage_checks_padding_permissions_and_remaps() {
        let first =
            resolve_texture_backing(tic(3), &space(512, 9, GpuMapPermissions::READ_ONLY)).unwrap();
        assert_eq!(first.range.length, 512);
        let second =
            resolve_texture_backing(tic(3), &space(512, 10, GpuMapPermissions::READ_ONLY)).unwrap();
        assert_ne!(first.range.resource, second.range.resource);
        assert!(matches!(
            resolve_texture_backing(tic(3), &space(511, 9, GpuMapPermissions::READ_ONLY)),
            Err(TextureBackingError::Address(_))
        ));
        assert_eq!(
            resolve_texture_backing(
                tic(3),
                &space(
                    512,
                    9,
                    GpuMapPermissions {
                        read: false,
                        write: true,
                        execute: false
                    }
                )
            ),
            Err(TextureBackingError::NotReadable)
        );
    }

    #[test]
    fn compressed_pitch_uses_block_rows_not_pixel_rows() {
        let mut words = tic(2);
        words[0] = (words[0] & !0x7f) | 0x24; // BC1: 4x4 blocks of eight bytes.
        words[4] = (words[4] & !0xffff) | 6;
        words[5] = (words[5] & !0xffff) | 4;
        let backing =
            resolve_texture_backing(words, &space(48, 9, GpuMapPermissions::READ_ONLY)).unwrap();
        assert_eq!(backing.range.length, 48);
        assert_eq!(
            backing.storage,
            TextureStorage::Pitch {
                row_bytes: 16,
                pitch_bytes: 32,
                rows: 2
            }
        );
        words[0] = (words[0] & !0x7f) | 0x7f;
        assert_eq!(
            resolve_texture_backing(words, &space(48, 9, GpuMapPermissions::READ_ONLY)),
            Err(TextureBackingError::UnknownFormat)
        );
    }

    #[test]
    fn unsupported_storage_controls_never_use_a_smaller_footprint() {
        let space = space(65536, 9, GpuMapPermissions::READ_ONLY);
        for (word, mask) in [
            (1, 32),
            (2, 1 << 16),
            (2, 1 << 29),
            (3, 1),
            (3, 6 << 3),
            (3, 1 << 6),
            (3, 1 << 9),
            (3, 1 << 10),
            (3, 1 << 13),
            (3, 1 << 28),
            (4, 1 << 16),
            (5, 1 << 16),
            (5, 1 << 30),
            (7, 1),
            (7, 1 << 8),
        ] {
            let mut words = tic(3);
            words[word] |= mask;
            assert!(
                resolve_texture_backing(words, &space).is_err(),
                "word {word}, mask {mask:x}"
            );
        }
        for header in [0, 1, 4, 5, 6, 7] {
            assert_eq!(
                resolve_texture_backing(tic(header), &space),
                Err(TextureBackingError::UnsupportedHeader)
            );
        }
    }
}
