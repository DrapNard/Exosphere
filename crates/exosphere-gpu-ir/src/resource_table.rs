// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Stable resource interning for GPU virtual-memory discoveries.
//!
//! A guest GPU address is not a stable identity: the same allocation can be mapped at multiple
//! addresses and mappings can move between runs.  Exosphere interns resources from semantic
//! descriptors and uses the address space only to resolve accesses into those resources.

use std::collections::BTreeMap;

use crate::resource::{BufferDesc, BufferUsage, ResourceId, TextureDesc};

#[derive(Debug, Clone, Default)]
pub struct ResourceTable {
    buffers: BTreeMap<ResourceId, BufferDesc>,
    textures: BTreeMap<ResourceId, TextureDesc>,
}

impl ResourceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn buffers(&self) -> impl Iterator<Item = &BufferDesc> {
        self.buffers.values()
    }
    pub fn textures(&self) -> impl Iterator<Item = &TextureDesc> {
        self.textures.values()
    }

    pub fn intern_buffer(
        &mut self,
        size: u64,
        usage: BufferUsage,
        source_key: &[u8],
    ) -> ResourceId {
        let id = ResourceId(hash_id(
            b"buffer-v1",
            &[&size.to_le_bytes(), &[usage_tag(usage)], source_key],
        ));
        self.buffers
            .entry(id)
            .or_insert(BufferDesc { id, size, usage });
        id
    }

    pub fn intern_texture(&mut self, mut texture: TextureDesc, source_key: &[u8]) -> ResourceId {
        let mut metadata = Vec::with_capacity(32);
        metadata.extend_from_slice(&texture.width.to_le_bytes());
        metadata.extend_from_slice(&texture.height.to_le_bytes());
        metadata.extend_from_slice(&texture.depth.to_le_bytes());
        metadata.extend_from_slice(&texture.layers.to_le_bytes());
        metadata.extend_from_slice(&texture.mip_levels.to_le_bytes());
        metadata.push(texture.samples.max(1));
        metadata.extend_from_slice(&format_tag(texture.format).to_le_bytes());
        metadata.extend_from_slice(&layout_tag(texture.layout).to_le_bytes());
        let id = ResourceId(hash_id(b"texture-v1", &[&metadata, source_key]));
        texture.id = id;
        self.textures.entry(id).or_insert(texture);
        id
    }

    pub fn buffer(&self, id: ResourceId) -> Option<&BufferDesc> {
        self.buffers.get(&id)
    }
    pub fn texture(&self, id: ResourceId) -> Option<&TextureDesc> {
        self.textures.get(&id)
    }
}

fn hash_id(domain: &[u8], parts: &[&[u8]]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let hash = hasher.finalize();
    u64::from_le_bytes(
        hash.as_bytes()[..8]
            .try_into()
            .expect("BLAKE3 output is at least 8 bytes"),
    )
}

fn usage_tag(usage: BufferUsage) -> u8 {
    use BufferUsage::*;
    match usage {
        Vertex => 0,
        VertexUniform => 9,
        IndexVertexUniform => 10,
        Index => 1,
        Uniform => 2,
        Storage => 3,
        TransferSource => 4,
        TransferDestination => 5,
        Indirect => 6,
        Query => 7,
        TransformFeedback => 8,
    }
}

fn format_tag(format: crate::resource::TextureFormat) -> u32 {
    use crate::resource::TextureFormat::*;
    match format {
        R8Unorm => 1,
        R8Uint => 2,
        R8Sint => 3,
        Rg8Unorm => 4,
        Rg8Uint => 5,
        Rg8Sint => 6,
        Rgba8Unorm => 10,
        Rgba8Srgb => 11,
        Bgra8Unorm => 12,
        Bgra8Srgb => 13,
        R16Float => 20,
        Rg16Float => 21,
        Rgba16Float => 22,
        R32Float => 23,
        Rg32Float => 24,
        Rgba32Float => 25,
        R32Uint => 26,
        R32Sint => 27,
        D16Unorm => 30,
        D24UnormS8 => 31,
        D32Float => 32,
        D32FloatS8 => 33,
        Bc1RgbaUnorm => 40,
        Bc1RgbaSrgb => 41,
        Bc2RgbaUnorm => 42,
        Bc2RgbaSrgb => 43,
        Bc3RgbaUnorm => 44,
        Bc3RgbaSrgb => 45,
        Bc4Unorm => 46,
        Bc4Snorm => 47,
        Bc5Unorm => 48,
        Bc5Snorm => 49,
        Bc6hUfloat => 50,
        Bc6hSfloat => 51,
        Bc7RgbaUnorm => 52,
        Bc7RgbaSrgb => 53,
        Astc4x4Unorm => 60,
        Astc5x4Unorm => 61,
        Astc5x5Unorm => 62,
        Astc6x5Unorm => 63,
        Astc6x6Unorm => 64,
        Astc8x5Unorm => 65,
        Astc8x6Unorm => 66,
        Astc8x8Unorm => 67,
        Astc10x5Unorm => 68,
        Astc10x6Unorm => 69,
        Astc10x8Unorm => 70,
        Astc10x10Unorm => 71,
        Astc12x10Unorm => 72,
        Astc12x12Unorm => 73,
        Unknown(raw) => 0x8000_0000 | raw,
    }
}

fn layout_tag(layout: crate::resource::TextureLayout) -> u32 {
    use crate::resource::TextureLayout::*;
    match layout {
        Linear => 0,
        BlockLinear {
            block_height_log2,
            block_depth_log2,
            raw,
        } => {
            0x4000_0000
                | ((u32::from(block_height_log2) & 0x7) << 24)
                | ((u32::from(block_depth_log2) & 0x7) << 27)
                | (raw & 0x00ff_ffff)
        }
        Unknown(raw) => 0x8000_0000 | raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{TextureFormat, TextureLayout};

    #[test]
    fn buffer_identity_is_stable_and_not_address_based() {
        let mut table = ResourceTable::new();
        let a = table.intern_buffer(4096, BufferUsage::Vertex, b"allocation-A");
        let b = table.intern_buffer(4096, BufferUsage::Vertex, b"allocation-A");
        assert_eq!(a, b);
        assert_eq!(table.buffers().count(), 1);
    }

    #[test]
    fn texture_metadata_participates_in_identity() {
        let mut table = ResourceTable::new();
        let base = TextureDesc {
            id: ResourceId(0),
            width: 64,
            height: 64,
            depth: 1,
            layers: 1,
            mip_levels: 1,
            samples: 1,
            format: TextureFormat::Rgba8Unorm,
            layout: TextureLayout::Linear,
        };
        let first = table.intern_texture(base.clone(), b"image-A");
        let mut larger = base;
        larger.width = 128;
        let second = table.intern_texture(larger, b"image-A");
        assert_ne!(first, second);
    }
}
