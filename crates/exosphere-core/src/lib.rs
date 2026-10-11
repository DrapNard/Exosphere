// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Core project-owned types. This crate must remain host-platform independent.

pub mod ids;
pub mod version;

pub use ids::{ObjectId, ResourceId, RuntimeId};
pub use version::{BuildFlavor, SchemaVersion, VersionMatrix};
