// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Stable cache keys for portable GPU artifacts.

use blake3::Hasher;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GpuCacheKey(pub [u8; 32]);

pub fn key(domain: &str, schema_version: u32, source: &[u8], variant: &[u8]) -> GpuCacheKey {
    let mut h = Hasher::new();
    h.update(b"EXOSPHERE-GPU\0");
    h.update(domain.as_bytes());
    h.update(&schema_version.to_le_bytes());
    h.update(&(source.len() as u64).to_le_bytes());
    h.update(source);
    h.update(&(variant.len() as u64).to_le_bytes());
    h.update(variant);
    GpuCacheKey(*h.finalize().as_bytes())
}
