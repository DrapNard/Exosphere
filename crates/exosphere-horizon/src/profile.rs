// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Versioned Horizon compatibility profiles.
//!
//! Service behavior changes are keyed by explicit firmware profiles rather than scattered version
//! comparisons. Profiles can later be loaded from generated tables without changing kernel APIs.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FirmwareVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl FirmwareVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceProfile {
    pub name: String,
    pub command_ids: BTreeSet<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HorizonProfile {
    pub id: String,
    pub firmware: FirmwareVersion,
    pub services: BTreeMap<String, ServiceProfile>,
}

impl HorizonProfile {
    pub fn generic() -> Self {
        Self {
            id: "generic".into(),
            firmware: FirmwareVersion::new(0, 0, 0),
            services: BTreeMap::new(),
        }
    }

    pub fn service_supports(&self, service: &str, command_id: u32) -> bool {
        self.services
            .get(service)
            .is_some_and(|profile| profile.command_ids.contains(&command_id))
    }
}

#[derive(Debug, Default, Clone)]
pub struct ProfileDatabase {
    profiles: BTreeMap<String, HorizonProfile>,
}

impl ProfileDatabase {
    pub fn insert(&mut self, profile: HorizonProfile) {
        self.profiles.insert(profile.id.clone(), profile);
    }
    pub fn get(&self, id: &str) -> Option<&HorizonProfile> {
        self.profiles.get(id)
    }
    pub fn latest(&self) -> Option<&HorizonProfile> {
        self.profiles
            .values()
            .max_by_key(|profile| profile.firmware)
    }
}
