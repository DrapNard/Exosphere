// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Read-only VFS adapter for a parsed Switch RomFS image.
//!
//! The adapter never extracts the RomFS to the host filesystem. File contents remain backed by the
//! original immutable image and are exposed through Exosphere's guest VFS. This keeps base/update/
//! DLC layers cheap to mount and makes mod overlays independent from host paths.

use crate::{
    IngestError,
    romfs::{self, RomFsFile},
};
use exosphere_vfs::{
    DirectoryEntry, EntryType, FsError, FsResult, GuestPath, Stat, VirtualFileSystem,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct RomFsFileSystem {
    image: Arc<[u8]>,
    files: BTreeMap<String, RomFsFile>,
    directories: BTreeSet<String>,
}

impl RomFsFileSystem {
    pub fn from_image(image: Arc<[u8]>) -> Result<Self, IngestError> {
        let parsed = romfs::parse(&image)?;
        let mut files = BTreeMap::new();
        let mut directories = BTreeSet::from(["/".to_owned()]);
        for file in parsed.files {
            let path = GuestPath::parse(&file.path)
                .map_err(|_| IngestError::Invalid)?
                .as_str()
                .to_owned();
            let components: Vec<&str> = path
                .trim_start_matches('/')
                .split('/')
                .filter(|part| !part.is_empty())
                .collect();
            let mut current = String::new();
            for component in components.iter().take(components.len().saturating_sub(1)) {
                current.push('/');
                current.push_str(component);
                directories.insert(current.clone());
            }
            files.insert(path, file);
        }
        Ok(Self {
            image,
            files,
            directories,
        })
    }
}

impl VirtualFileSystem for RomFsFileSystem {
    fn stat(&self, path: &GuestPath) -> FsResult<Stat> {
        if let Some(file) = self.files.get(path.as_str()) {
            return Ok(Stat {
                entry_type: EntryType::File,
                size: file.size,
                read_only: true,
            });
        }
        if self.directories.contains(path.as_str()) {
            return Ok(Stat {
                entry_type: EntryType::Directory,
                size: 0,
                read_only: true,
            });
        }
        Err(FsError::NotFound(path.as_str().into()))
    }
    fn read_file(&self, path: &GuestPath, offset: u64, output: &mut [u8]) -> FsResult<usize> {
        let file = self
            .files
            .get(path.as_str())
            .ok_or_else(|| FsError::NotFound(path.as_str().into()))?;
        if offset >= file.size {
            return Ok(0);
        }
        let available = file.size - offset;
        let count = available.min(output.len() as u64) as usize;
        let start = file
            .data_offset
            .checked_add(offset)
            .ok_or(FsError::InvalidRange)? as usize;
        let end = start.checked_add(count).ok_or(FsError::InvalidRange)?;
        let bytes = self.image.get(start..end).ok_or(FsError::InvalidRange)?;
        output[..count].copy_from_slice(bytes);
        Ok(count)
    }
    fn write_file(&self, _: &GuestPath, _: u64, _: &[u8]) -> FsResult<usize> {
        Err(FsError::ReadOnly)
    }
    fn create_file(&self, _: &GuestPath, _: u64) -> FsResult<()> {
        Err(FsError::ReadOnly)
    }
    fn delete_file(&self, _: &GuestPath) -> FsResult<()> {
        Err(FsError::ReadOnly)
    }
    fn create_directory(&self, _: &GuestPath) -> FsResult<()> {
        Err(FsError::ReadOnly)
    }
    fn delete_directory(&self, _: &GuestPath) -> FsResult<()> {
        Err(FsError::ReadOnly)
    }
    fn read_directory(&self, path: &GuestPath) -> FsResult<Vec<DirectoryEntry>> {
        if !self.directories.contains(path.as_str()) {
            return Err(FsError::NotADirectory);
        }
        let prefix = if path.as_str() == "/" {
            "/".to_owned()
        } else {
            format!("{}/", path.as_str().trim_end_matches('/'))
        };
        let mut entries = BTreeMap::<String, DirectoryEntry>::new();
        for (name, file) in &self.files {
            if let Some(rest) = name.strip_prefix(&prefix) {
                if !rest.is_empty() && !rest.contains('/') {
                    entries.insert(
                        rest.into(),
                        DirectoryEntry {
                            name: rest.into(),
                            entry_type: EntryType::File,
                            size: file.size,
                        },
                    );
                }
            }
        }
        for name in &self.directories {
            if let Some(rest) = name.strip_prefix(&prefix) {
                if !rest.is_empty() && !rest.contains('/') {
                    entries.insert(
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
        Ok(entries.into_values().collect())
    }
    fn rename(&self, _: &GuestPath, _: &GuestPath) -> FsResult<()> {
        Err(FsError::ReadOnly)
    }
}
