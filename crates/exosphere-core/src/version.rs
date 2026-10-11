// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Independently versioned schema/runtime components.

/// Build flavor recorded in packages and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildFlavor {
    Release,
    Debug,
}

/// Version of one serialized schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersion {
    pub major: u16,
    pub minor: u16,
    pub flavor: BuildFlavor,
}

/// Versions that may evolve independently without forcing unrelated package migrations.
#[derive(Debug, Clone)]
pub struct VersionMatrix {
    pub package: SchemaVersion,
    pub cpu_decoded_ir: SchemaVersion,
    pub cpu_semantic_ir: SchemaVersion,
    pub gpu_decode_ir: SchemaVersion,
    pub gpu_semantic_ir: SchemaVersion,
    pub shader_ir: SchemaVersion,
    pub mme_ir: SchemaVersion,
    pub media_ir: SchemaVersion,
    pub resource_ir: SchemaVersion,
    pub symbol_db: SchemaVersion,
    pub runtime_abi: SchemaVersion,
    pub mod_abi: SchemaVersion,
    pub horizon_profile_db: SchemaVersion,
}
