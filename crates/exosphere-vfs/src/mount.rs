// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Guest mount table.

use crate::{
    path::GuestPath,
    types::{FsError, FsResult, VirtualFileSystem},
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MountId(pub String);

#[derive(Clone)]
pub struct Mount {
    pub id: MountId,
    pub fs: Arc<dyn VirtualFileSystem>,
    pub writable: bool,
}
impl std::fmt::Debug for Mount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mount")
            .field("id", &self.id)
            .field("writable", &self.writable)
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct MountTable {
    mounts: BTreeMap<MountId, Mount>,
}
impl MountTable {
    pub fn mount(&mut self, mount: Mount) {
        self.mounts.insert(mount.id.clone(), mount);
    }
    pub fn unmount(&mut self, id: &MountId) -> Option<Mount> {
        self.mounts.remove(id)
    }
    pub fn get(&self, id: &MountId) -> FsResult<&Mount> {
        self.mounts
            .get(id)
            .ok_or_else(|| FsError::NotFound(id.0.clone()))
    }
    pub fn resolve<'a>(&'a self, qualified: &str) -> FsResult<(&'a Mount, GuestPath)> {
        let (name, rest) = qualified.split_once(':').ok_or(FsError::InvalidPath)?;
        let mount = self.get(&MountId(name.to_owned()))?;
        Ok((mount, GuestPath::parse(rest)?))
    }
    pub fn ids(&self) -> impl Iterator<Item = &MountId> {
        self.mounts.keys()
    }
}
