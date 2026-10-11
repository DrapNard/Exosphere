// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Canonical descriptor-table records.
//!
//! Tegra texture/image/sampler descriptors contain many Maxwell-specific bitfields.  Exosphere
//! keeps the raw words for lossless replay while promoting only fields whose semantics are known.

use crate::{
    maxwell_state::RawGpuAddress,
    resource::{ResourceId, TextureFormat, TextureLayout},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDescriptor<const N: usize> {
    pub words: [u32; N],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureDescriptor {
    pub raw: RawDescriptor<8>,
    pub resource: Option<ResourceId>,
    pub address: RawGpuAddress,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub depth: Option<u32>,
    pub mip_levels: Option<u32>,
    pub format: TextureFormat,
    pub layout: TextureLayout,
}

impl TextureDescriptor {
    pub fn opaque(words: [u32; 8]) -> Self {
        Self {
            raw: RawDescriptor { words },
            resource: None,
            address: RawGpuAddress::default(),
            width: None,
            height: None,
            depth: None,
            mip_levels: None,
            format: TextureFormat::Unknown(words[0]),
            layout: TextureLayout::Unknown(words[1]),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplerDescriptor {
    pub raw: RawDescriptor<8>,
    pub resource: Option<ResourceId>,
}

impl SamplerDescriptor {
    pub fn opaque(words: [u32; 8]) -> Self {
        Self {
            raw: RawDescriptor { words },
            resource: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DescriptorHeap {
    pub textures: Vec<TextureDescriptor>,
    pub samplers: Vec<SamplerDescriptor>,
}

impl DescriptorHeap {
    pub fn texture(&self, index: u32) -> Option<&TextureDescriptor> {
        self.textures.get(index as usize)
    }
    pub fn sampler(&self, index: u32) -> Option<&SamplerDescriptor> {
        self.samplers.get(index as usize)
    }
}
