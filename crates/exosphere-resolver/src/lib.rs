// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Immutable base/update/DLC/mod layering resolver.

use exosphere_manifest::Manifest;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("dependency is not satisfied: {0}")]
    MissingDependency(String),
    #[error("two layers conflict: {0}")]
    Conflict(String),
}

#[derive(Debug)]
pub struct EffectiveGame<'a> {
    pub layers: Vec<&'a Manifest>,
}

pub fn resolve<'a>(layers: &'a [Manifest]) -> Result<EffectiveGame<'a>, ResolveError> {
    // TODO: semantic version constraints, override priorities and symbol/resource conflict graphs.
    Ok(EffectiveGame {
        layers: layers.iter().collect(),
    })
}
