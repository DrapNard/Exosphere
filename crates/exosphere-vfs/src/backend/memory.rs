// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Deterministic in-memory filesystem used by tests and synthetic mounts.

use crate::{path::GuestPath, types::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::RwLock,
};

#[derive(Debug, Default)]
pub struct MemoryFileSystem {
    files: RwLock<BTreeMap<String, Vec<u8>>>,
    directories: RwLock<BTreeSet<String>>,
    read_only: bool,
}
impl MemoryFileSystem {
    pub fn new(read_only: bool) -> Self {
        let fs = Self {
            read_only,
            ..Self::default()
        };
        fs.directories.write().unwrap().insert("/".into());
        fs
    }
    pub fn insert_read_only_file(&self, path: &GuestPath, bytes: Vec<u8>) {
        self.files
            .write()
            .unwrap()
            .insert(path.as_str().into(), bytes);
    }
    fn ensure_writable(&self) -> FsResult<()> {
        if self.read_only {
            Err(FsError::ReadOnly)
        } else {
            Ok(())
        }
    }
}
impl VirtualFileSystem for MemoryFileSystem {
    fn stat(&self, path: &GuestPath) -> FsResult<Stat> {
        if let Some(data) = self.files.read().unwrap().get(path.as_str()) {
            return Ok(Stat {
                entry_type: EntryType::File,
                size: data.len() as u64,
                read_only: self.read_only,
            });
        }
        if self.directories.read().unwrap().contains(path.as_str()) {
            return Ok(Stat {
                entry_type: EntryType::Directory,
                size: 0,
                read_only: self.read_only,
            });
        }
        Err(FsError::NotFound(path.as_str().into()))
    }
    fn read_file(&self, path: &GuestPath, offset: u64, output: &mut [u8]) -> FsResult<usize> {
        let files = self.files.read().unwrap();
        let data = files
            .get(path.as_str())
            .ok_or_else(|| FsError::NotFound(path.as_str().into()))?;
        let start = usize::try_from(offset).map_err(|_| FsError::InvalidRange)?;
        if start >= data.len() {
            return Ok(0);
        }
        let len = output.len().min(data.len() - start);
        output[..len].copy_from_slice(&data[start..start + len]);
        Ok(len)
    }
    fn write_file(&self, path: &GuestPath, offset: u64, input: &[u8]) -> FsResult<usize> {
        self.ensure_writable()?;
        let mut files = self.files.write().unwrap();
        let data = files
            .get_mut(path.as_str())
            .ok_or_else(|| FsError::NotFound(path.as_str().into()))?;
        let start = usize::try_from(offset).map_err(|_| FsError::InvalidRange)?;
        let end = start
            .checked_add(input.len())
            .ok_or(FsError::InvalidRange)?;
        if end > data.len() {
            data.resize(end, 0);
        }
        data[start..end].copy_from_slice(input);
        Ok(input.len())
    }
    fn create_file(&self, path: &GuestPath, size: u64) -> FsResult<()> {
        self.ensure_writable()?;
        let len = usize::try_from(size).map_err(|_| FsError::InvalidRange)?;
        let mut files = self.files.write().unwrap();
        if files.contains_key(path.as_str()) {
            return Err(FsError::AlreadyExists(path.as_str().into()));
        }
        files.insert(path.as_str().into(), vec![0; len]);
        Ok(())
    }
    fn delete_file(&self, path: &GuestPath) -> FsResult<()> {
        self.ensure_writable()?;
        self.files
            .write()
            .unwrap()
            .remove(path.as_str())
            .ok_or_else(|| FsError::NotFound(path.as_str().into()))?;
        Ok(())
    }
    fn create_directory(&self, path: &GuestPath) -> FsResult<()> {
        self.ensure_writable()?;
        self.directories
            .write()
            .unwrap()
            .insert(path.as_str().into());
        Ok(())
    }
    fn delete_directory(&self, path: &GuestPath) -> FsResult<()> {
        self.ensure_writable()?;
        self.directories.write().unwrap().remove(path.as_str());
        Ok(())
    }
    fn read_directory(&self, path: &GuestPath) -> FsResult<Vec<DirectoryEntry>> {
        if !self.directories.read().unwrap().contains(path.as_str()) {
            return Err(FsError::NotADirectory);
        }
        let prefix = if path.as_str() == "/" {
            "/".to_owned()
        } else {
            format!("{}/", path.as_str().trim_end_matches('/'))
        };
        let mut out = BTreeMap::<String, DirectoryEntry>::new();
        for (name, data) in self.files.read().unwrap().iter() {
            if let Some(rest) = name.strip_prefix(&prefix) {
                if !rest.contains('/') {
                    out.insert(
                        rest.into(),
                        DirectoryEntry {
                            name: rest.into(),
                            entry_type: EntryType::File,
                            size: data.len() as u64,
                        },
                    );
                }
            }
        }
        for name in self.directories.read().unwrap().iter() {
            if let Some(rest) = name.strip_prefix(&prefix) {
                if !rest.is_empty() && !rest.contains('/') {
                    out.insert(
                        rest.into(),
                        DirectoryEntry {
                            name: rest.into(),
                            entry_type: EntryType::Directory,
                            size: 0,
                        },
                    );
                }
            }
        }
        Ok(out.into_values().collect())
    }
    fn rename(&self, from: &GuestPath, to: &GuestPath) -> FsResult<()> {
        self.ensure_writable()?;
        let mut files = self.files.write().unwrap();
        let data = files
            .remove(from.as_str())
            .ok_or_else(|| FsError::NotFound(from.as_str().into()))?;
        files.insert(to.as_str().into(), data);
        Ok(())
    }
    fn set_file_size(&self, path: &GuestPath, size: u64) -> FsResult<()> {
        self.ensure_writable()?;
        let len = usize::try_from(size).map_err(|_| FsError::InvalidRange)?;
        let mut files = self.files.write().unwrap();
        let data = files
            .get_mut(path.as_str())
            .ok_or_else(|| FsError::NotFound(path.as_str().into()))?;
        data.resize(len, 0);
        Ok(())
    }
}
