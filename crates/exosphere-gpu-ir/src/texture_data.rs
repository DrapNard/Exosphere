// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only, tightly packed texture views over live allocation payloads.
//!
//! Callers reserve view IDs in their renderer namespace. A view never owns a
//! copy of guest pixels and never grants write access to its backing allocation.

use std::collections::BTreeMap;

use crate::{
    GpuAddressSpace, GpuResourceData, ResourceDataError, ResourceId,
    texture_backing::{
        TextureBacking, TextureBackingError, TextureStorage, resolve_texture_backing,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureDataError {
    Backing(TextureBackingError),
    Payload(ResourceDataError),
    IdentityCollision(ResourceId),
}

pub struct TextureResourceData<'a> {
    source: &'a mut dyn GpuResourceData,
    views: BTreeMap<ResourceId, TextureBacking>,
}

impl<'a> TextureResourceData<'a> {
    pub fn new(source: &'a mut dyn GpuResourceData) -> Self {
        Self {
            source,
            views: BTreeMap::new(),
        }
    }

    /// Reserve a distinct image ID; allocation IDs cannot double as subviews.
    /// Refresh these descriptors from the current draw/address-space snapshot.
    /// Actual reads remain subject to the source provider's live lease checks.
    pub fn add_texture(
        &mut self,
        id: ResourceId,
        words: [u32; 8],
        space: &GpuAddressSpace,
    ) -> Result<TextureBacking, TextureDataError> {
        if self.views.contains_key(&id) {
            return Err(TextureDataError::IdentityCollision(id));
        }
        match self.source.len(id) {
            Err(ResourceDataError::Missing(_)) => {}
            Ok(_) => return Err(TextureDataError::IdentityCollision(id)),
            Err(error) => return Err(TextureDataError::Payload(error)),
        }
        let backing = resolve_texture_backing(words, space).map_err(TextureDataError::Backing)?;
        self.validate_backing(&backing)
            .map_err(TextureDataError::Payload)?;
        self.views.insert(id, backing.clone());
        Ok(backing)
    }

    fn validate_backing(&self, backing: &TextureBacking) -> Result<(), ResourceDataError> {
        let range = backing.range;
        let size = self.source.len(range.resource)?;
        let end = range
            .offset
            .checked_add(range.length)
            .ok_or(ResourceDataError::SizeOverflow)?;
        if end > size {
            return Err(ResourceDataError::OutOfBounds {
                resource: range.resource,
                offset: range.offset,
                length: range.length,
                size,
            });
        }
        Ok(())
    }

    fn packed_size(backing: &TextureBacking) -> u64 {
        if let Some(last) = backing.mip_levels.last() {
            return last.packed_offset + last.packed_size;
        }
        match backing.storage {
            TextureStorage::Pitch {
                row_bytes, rows, ..
            } => row_bytes * u64::from(rows),
            TextureStorage::BlockLinear(layout) => {
                u64::from(layout.width_bytes) * u64::from(layout.height_rows)
            }
        }
    }
}

impl GpuResourceData for TextureResourceData<'_> {
    fn content_fingerprint(
        &self,
        resource: ResourceId,
        offset: u64,
        length: u64,
    ) -> Result<Option<[u8; 32]>, ResourceDataError> {
        if !self.views.contains_key(&resource) {
            return self.source.content_fingerprint(resource, offset, length);
        }
        let size = self.len(resource)?;
        if offset
            .checked_add(length)
            .ok_or(ResourceDataError::SizeOverflow)?
            > size
        {
            return Err(ResourceDataError::OutOfBounds {
                resource,
                offset,
                length,
                size,
            });
        }
        // A hash of padded allocation bytes is not a hash of the packed view.
        // Returning None forces the backend to read the actual logical bytes.
        Ok(None)
    }

    fn len(&self, resource: ResourceId) -> Result<u64, ResourceDataError> {
        let Some(backing) = self.views.get(&resource) else {
            return self.source.len(resource);
        };
        self.validate_backing(backing)?;
        Ok(Self::packed_size(backing))
    }

    fn read(
        &self,
        resource: ResourceId,
        offset: u64,
        out: &mut [u8],
    ) -> Result<(), ResourceDataError> {
        let Some(backing) = self.views.get(&resource) else {
            return self.source.read(resource, offset, out);
        };
        let size = self.len(resource)?;
        let length = out.len() as u64;
        if offset
            .checked_add(length)
            .ok_or(ResourceDataError::SizeOverflow)?
            > size
        {
            return Err(ResourceDataError::OutOfBounds {
                resource,
                offset,
                length,
                size,
            });
        }
        let mut done = 0;
        while done < out.len() {
            let logical = offset + done as u64;
            let (storage, physical_base, level_logical) = if backing.mip_levels.is_empty() {
                (backing.storage, 0, logical)
            } else {
                let level = backing
                    .mip_levels
                    .iter()
                    .find(|level| {
                        logical >= level.packed_offset
                            && logical < level.packed_offset + level.packed_size
                    })
                    .ok_or(ResourceDataError::SizeOverflow)?;
                (
                    TextureStorage::BlockLinear(level.layout),
                    level.physical_offset,
                    logical - level.packed_offset,
                )
            };
            let (physical, contiguous) = match storage {
                TextureStorage::Pitch {
                    row_bytes,
                    pitch_bytes,
                    ..
                } => {
                    let x = level_logical % row_bytes;
                    (level_logical / row_bytes * pitch_bytes + x, row_bytes - x)
                }
                TextureStorage::BlockLinear(layout) => {
                    let x = (level_logical % u64::from(layout.width_bytes)) as u32;
                    let y = (level_logical / u64::from(layout.width_bytes)) as u32;
                    // Tegra GOBs preserve only a 16-byte horizontal run. Split
                    // there and at logical row boundaries, not at pixel widths.
                    (
                        layout
                            .offset(x, y, 0)
                            .ok_or(ResourceDataError::SizeOverflow)?,
                        u64::from((16 - (x & 15)).min(layout.width_bytes - x)),
                    )
                }
            };
            let count = contiguous.min((out.len() - done) as u64) as usize;
            self.source.read(
                backing.range.resource,
                backing
                    .range
                    .offset
                    .checked_add(physical_base)
                    .and_then(|base| base.checked_add(physical))
                    .ok_or(ResourceDataError::SizeOverflow)?,
                &mut out[done..done + count],
            )?;
            done += count;
        }
        Ok(())
    }

    fn write(
        &mut self,
        resource: ResourceId,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ResourceDataError> {
        if self.views.contains_key(&resource) {
            return Err(ResourceDataError::ReadOnly(resource));
        }
        self.source.write(resource, offset, data)
    }

    fn flush(&mut self, resource: ResourceId) -> Result<(), ResourceDataError> {
        if self.views.contains_key(&resource) {
            return Err(ResourceDataError::ReadOnly(resource));
        }
        self.source.flush(resource)
    }

    fn invalidate(
        &mut self,
        resource: ResourceId,
        offset: u64,
        length: u64,
    ) -> Result<(), ResourceDataError> {
        let Some(backing) = self.views.get(&resource) else {
            return self.source.invalidate(resource, offset, length);
        };
        let size = self.len(resource)?;
        if offset
            .checked_add(length)
            .ok_or(ResourceDataError::SizeOverflow)?
            > size
        {
            return Err(ResourceDataError::OutOfBounds {
                resource,
                offset,
                length,
                size,
            });
        }
        // Conservative invalidation is safe for strided/tiled views. Forwarding
        // the packed offset directly would invalidate unrelated guest bytes.
        self.source.invalidate(
            backing.range.resource,
            backing.range.offset,
            backing.range.length,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        GpuMapPermissions, GpuMapping, InMemoryGpuResources,
        block_linear::{BlockLinearLayout, swizzle_2d_into},
    };

    fn fixture(header: u32, width: u32, height: u32) -> ([u32; 8], GpuAddressSpace) {
        let words = [
            0x08 | 2 << 7 | 2 << 10 | 2 << 13 | 2 << 16,
            0x1200,
            header << 21,
            if header == 2 { 4 } else { 0 },
            (width - 1) | (1 << 23) | (7 << 29),
            (height - 1) | (1 << 31),
            0,
            0,
        ];
        let mut space = GpuAddressSpace::new();
        space
            .map(GpuMapping {
                guest_base: 0x1200,
                size: 4096,
                resource: ResourceId(1),
                resource_offset: 64,
                permissions: GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        (words, space)
    }

    #[test]
    fn tiled_mip_views_pack_all_levels_and_cross_level_boundaries() {
        for compressed in [false, true] {
            let (mut words, space) = fixture(3, 8, 8);
            if compressed {
                words[0] = (words[0] & !0x7f) | 0x24;
            }
            words[3] = 3 << 28 | 3 << 3;
            words[7] = 3 << 4;
            let backing = resolve_texture_backing(words, &space).unwrap();
            assert_eq!(backing.mip_levels.len(), 4);
            let mut bytes = vec![0xcc; 4160];
            let mut expected = Vec::new();
            for (index, level) in backing.mip_levels.iter().enumerate() {
                assert_eq!(level.physical_offset, index as u64 * 512);
                assert_eq!(level.layout.block_height_log2, 0);
                let pixels: Vec<_> = (0..level.packed_size)
                    .map(|n| (n as u8).wrapping_add(index as u8 * 37))
                    .collect();
                swizzle_2d_into(
                    &pixels,
                    &mut bytes[64 + level.physical_offset as usize..],
                    level.layout,
                )
                .unwrap();
                expected.extend(pixels);
            }
            let mut source = InMemoryGpuResources::new();
            source.insert(ResourceId(1), bytes, true);
            let mut provider = TextureResourceData::new(&mut source);
            provider.add_texture(ResourceId(2), words, &space).unwrap();
            assert_eq!(provider.len(ResourceId(2)).unwrap(), expected.len() as u64);
            let mut actual = vec![0; expected.len()];
            provider.read(ResourceId(2), 0, &mut actual).unwrap();
            assert_eq!(actual, expected);
            for offset in 0..expected.len() {
                let length = (expected.len() - offset).min(19);
                provider
                    .read(ResourceId(2), offset as u64, &mut actual[..length])
                    .unwrap();
                assert_eq!(&actual[..length], &expected[offset..offset + length]);
            }
            let last = backing.mip_levels.last().unwrap();
            provider
                .write(ResourceId(1), 64 + last.physical_offset, &[99])
                .unwrap();
            let mut refreshed = [0];
            provider
                .read(ResourceId(2), last.packed_offset, &mut refreshed)
                .unwrap();
            assert_eq!(refreshed, [99]);
        }
    }

    #[test]
    fn pitched_views_read_live_subranges_without_padding_or_write_access() {
        let (words, space) = fixture(2, 7, 3);
        let mut source = InMemoryGpuResources::new();
        source.insert(ResourceId(1), vec![0xcc; 4160], true);
        let mut provider = TextureResourceData::new(&mut source);
        provider.add_texture(ResourceId(2), words, &space).unwrap();
        for row in 0..3 {
            provider
                .write(ResourceId(1), 64 + row * 128, &[row as u8; 28])
                .unwrap();
        }
        assert_eq!(provider.len(ResourceId(2)), Ok(84));
        assert!(
            provider
                .content_fingerprint(ResourceId(1), 64, 28)
                .unwrap()
                .is_some()
        );
        assert_eq!(provider.content_fingerprint(ResourceId(2), 0, 84), Ok(None));
        assert!(provider.content_fingerprint(ResourceId(2), 84, 1).is_err());
        let mut out = [0; 60];
        provider.read(ResourceId(2), 11, &mut out).unwrap();
        assert_eq!(&out[..17], &[0; 17]);
        assert_eq!(&out[17..45], &[1; 28]);
        assert_eq!(&out[45..], &[2; 15]);
        provider.write(ResourceId(1), 64 + 128, &[9; 28]).unwrap();
        provider.read(ResourceId(2), 28, &mut out[..28]).unwrap();
        assert_eq!(&out[..28], &[9; 28]);
        assert_eq!(
            provider.write(ResourceId(2), 0, &[1]),
            Err(ResourceDataError::ReadOnly(ResourceId(2)))
        );
        assert!(provider.read(ResourceId(2), 84, &mut [0]).is_err());
        assert!(provider.read(ResourceId(2), u64::MAX, &mut [0]).is_err());
        assert!(matches!(
            provider.add_texture(ResourceId(1), words, &space),
            Err(TextureDataError::IdentityCollision(_))
        ));
        assert!(matches!(
            provider.add_texture(ResourceId(2), words, &space),
            Err(TextureDataError::IdentityCollision(_))
        ));
    }

    #[test]
    fn tiled_arbitrary_slices_match_linear_bytes_across_gob_boundaries() {
        let (words, space) = fixture(3, 19, 11);
        let layout = BlockLinearLayout::new(76, 11, 1, 0, 0);
        let expected: Vec<_> = (0..836).map(|n| (n * 37) as u8).collect();
        let mut bytes = vec![0xcc; 4160];
        swizzle_2d_into(&expected, &mut bytes[64..], layout).unwrap();
        let mut source = InMemoryGpuResources::new();
        source.insert(ResourceId(1), bytes, false);
        let mut provider = TextureResourceData::new(&mut source);
        provider.add_texture(ResourceId(2), words, &space).unwrap();
        assert_eq!(provider.len(ResourceId(2)), Ok(836));
        for start in 0..=836 {
            let count = (836 - start).min(131);
            let mut out = vec![0; count];
            provider
                .read(ResourceId(2), start as u64, &mut out)
                .unwrap();
            assert_eq!(out, expected[start..start + count]);
        }
    }
}
