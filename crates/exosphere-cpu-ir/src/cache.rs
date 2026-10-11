// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Content-addressed CPU compilation cache keys.
use crate::Function;
/// Stable key for one lifted function plus backend/schema identity.
pub fn function_key(function: &Function, backend: &str, schema_version: u32) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(backend.as_bytes());
    h.update(&schema_version.to_le_bytes());
    h.update(&function.guest_entry.to_le_bytes());
    for b in &function.blocks {
        h.update(&b.guest_start.to_le_bytes());
        h.update(&b.id.0.to_le_bytes());
        for op in &b.operations {
            h.update(format!("{op:?}").as_bytes());
        }
        h.update(format!("{:?}", b.terminator).as_bytes());
    }
    *h.finalize().as_bytes()
}
