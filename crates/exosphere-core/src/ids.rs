// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Stable and runtime-local identifier types.

/// Stable logical resource identity: BLAKE3(filename + content + schema version).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId(pub [u8; 32]);

/// CAS object identity: BLAKE3(content + object schema version).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(pub [u8; 32]);

/// Compact process-local handle. It is intentionally not persistent across runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimeId(pub u64);
