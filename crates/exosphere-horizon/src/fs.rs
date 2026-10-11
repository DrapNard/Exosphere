// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Horizon filesystem service facade backed by `exosphere-vfs`.

use crate::error::{HorizonError, HorizonResult};
use exosphere_vfs::{GuestPath, MountId, MountTable, VirtualFileSystem, VirtualStorage};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsMountKind {
    Rom,
    Save,
    Cache,
    Sdmc,
    Host,
    System,
}

#[derive(Default)]
pub struct HorizonFs {
    pub mounts: MountTable,
    /// Raw/block-like storages exposed through Horizon `IStorage`. These are deliberately separate
    /// from path-based filesystems: a title can request data storage without receiving directory semantics.
    pub storages: BTreeMap<String, Arc<dyn VirtualStorage>>,
}
impl std::fmt::Debug for HorizonFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HorizonFs")
            .field("mounts", &self.mounts)
            .field("storages", &self.storages.keys().collect::<Vec<_>>())
            .finish()
    }
}
impl HorizonFs {
    pub fn mount(&mut self, name: &str, fs: Arc<dyn VirtualFileSystem>, writable: bool) {
        self.mounts.mount(exosphere_vfs::Mount {
            id: MountId(name.into()),
            fs,
            writable,
        });
    }
    pub fn mount_storage(&mut self, name: &str, storage: Arc<dyn VirtualStorage>) {
        self.storages.insert(name.to_owned(), storage);
    }
    pub fn storage(&self, name: &str) -> HorizonResult<&Arc<dyn VirtualStorage>> {
        self.storages
            .get(name)
            .ok_or_else(|| HorizonError::FileSystem(format!("storage {name} is not mounted")))
    }
    pub fn read(&self, qualified: &str, offset: u64, output: &mut [u8]) -> HorizonResult<usize> {
        let (m, p) = self.mounts.resolve(qualified).map_err(map)?;
        m.fs.read_file(&p, offset, output).map_err(map)
    }
    pub fn write(&self, qualified: &str, offset: u64, input: &[u8]) -> HorizonResult<usize> {
        let (m, p) = self.mounts.resolve(qualified).map_err(map)?;
        if !m.writable {
            return Err(HorizonError::PermissionDenied);
        }
        m.fs.write_file(&p, offset, input).map_err(map)
    }
    pub fn exists(&self, qualified: &str) -> bool {
        self.mounts
            .resolve(qualified)
            .ok()
            .and_then(|(m, p)| m.fs.stat(&p).ok())
            .is_some()
    }
    pub fn normalize_path(&self, path: &str) -> HorizonResult<GuestPath> {
        GuestPath::parse(path).map_err(map)
    }
}
fn map(e: exosphere_vfs::FsError) -> HorizonError {
    HorizonError::FileSystem(e.to_string())
}
