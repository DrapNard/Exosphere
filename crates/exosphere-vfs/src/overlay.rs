// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Read-priority overlay filesystem used for base/update/DLC/mod layering.

use crate::{path::GuestPath, types::*};
use std::sync::Arc;

#[derive(Clone)]
pub struct OverlayLayer {
    pub name: String,
    pub priority: i32,
    pub fs: Arc<dyn VirtualFileSystem>,
    pub writable: bool,
}
#[derive(Default, Clone)]
pub struct OverlayFileSystem {
    layers: Vec<OverlayLayer>,
}
impl OverlayFileSystem {
    pub fn push(&mut self, layer: OverlayLayer) {
        self.layers.push(layer);
        self.layers
            .sort_by_key(|layer| std::cmp::Reverse(layer.priority));
    }
    fn readable(&self, path: &GuestPath) -> FsResult<&OverlayLayer> {
        self.layers
            .iter()
            .find(|l| l.fs.stat(path).is_ok())
            .ok_or_else(|| FsError::NotFound(path.as_str().into()))
    }
    fn writable(&self) -> FsResult<&OverlayLayer> {
        self.layers
            .iter()
            .find(|l| l.writable)
            .ok_or(FsError::ReadOnly)
    }
}
impl VirtualFileSystem for OverlayFileSystem {
    fn stat(&self, p: &GuestPath) -> FsResult<Stat> {
        self.readable(p)?.fs.stat(p)
    }
    fn read_file(&self, p: &GuestPath, o: u64, b: &mut [u8]) -> FsResult<usize> {
        self.readable(p)?.fs.read_file(p, o, b)
    }
    fn write_file(&self, p: &GuestPath, o: u64, b: &[u8]) -> FsResult<usize> {
        self.writable()?.fs.write_file(p, o, b)
    }
    fn create_file(&self, p: &GuestPath, s: u64) -> FsResult<()> {
        self.writable()?.fs.create_file(p, s)
    }
    fn delete_file(&self, p: &GuestPath) -> FsResult<()> {
        self.writable()?.fs.delete_file(p)
    }
    fn create_directory(&self, p: &GuestPath) -> FsResult<()> {
        self.writable()?.fs.create_directory(p)
    }
    fn delete_directory(&self, p: &GuestPath) -> FsResult<()> {
        self.writable()?.fs.delete_directory(p)
    }
    fn read_directory(&self, p: &GuestPath) -> FsResult<Vec<DirectoryEntry>> {
        let mut all = std::collections::BTreeMap::new();
        for l in &self.layers {
            if let Ok(entries) = l.fs.read_directory(p) {
                for e in entries {
                    all.entry(e.name.clone()).or_insert(e);
                }
            }
        }
        Ok(all.into_values().collect())
    }
    fn rename(&self, a: &GuestPath, b: &GuestPath) -> FsResult<()> {
        self.writable()?.fs.rename(a, b)
    }
    fn flush(&self) -> FsResult<()> {
        for l in &self.layers {
            if l.writable {
                l.fs.flush()?;
            }
        }
        Ok(())
    }
}
