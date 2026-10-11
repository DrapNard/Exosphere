// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Canonical resource metadata and opaque fallback.

use exosphere_core::{ObjectId, ResourceId};

#[derive(Debug, Clone)]
pub struct Resource {
    pub resource_id: ResourceId,
    pub object_id: ObjectId,
    pub normalized_path: String,
    pub kind: ResourceKind,
    pub original_format: Option<String>,
    pub payload: ResourcePayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Texture,
    Audio,
    Model,
    Animation,
    StructuredData,
    Localization,
    ScriptData,
    Archive,
    Opaque,
}

#[derive(Debug, Clone)]
pub enum ResourcePayload {
    /// Canonical/typed representations will be added per resource family.
    Typed(Vec<u8>),
    /// Byte-exact fallback used when the format is not understood yet.
    Opaque(Vec<u8>),
}
