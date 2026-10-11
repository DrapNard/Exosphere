// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! TOML authoring manifests for packages, layers and runtime requirements.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub id: String,
    pub version: String,
    pub package_type: PackageType,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageType {
    Base,
    Update,
    Dlc,
    SwitchMod,
    NativeMod,
    Resources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub id: String,
    pub requirement: String,
}

pub fn parse(input: &str) -> Result<Manifest, toml::de::Error> {
    toml::from_str(input)
}
