// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Sandboxed host-directory backend for save data, cache, SD-card and development mounts.
//!
//! Guest paths are already normalized, but this backend additionally rejects symlinks in the path
//! walk. That prevents a symlink placed inside the sandbox from redirecting a guest access outside
//! the configured root.

use crate::{path::GuestPath, types::*};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct HostDirectoryFileSystem {
    root: PathBuf,
    read_only: bool,
}

impl HostDirectoryFileSystem {
    pub fn new(root: impl Into<PathBuf>, read_only: bool) -> FsResult<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(io_error)?;
        let root = fs::canonicalize(root).map_err(io_error)?;
        Ok(Self { root, read_only })
    }

    fn ensure_writable(&self) -> FsResult<()> {
        if self.read_only {
            Err(FsError::ReadOnly)
        } else {
            Ok(())
        }
    }

    fn host_path(&self, path: &GuestPath) -> FsResult<PathBuf> {
        let mut current = self.root.clone();
        for component in path
            .as_str()
            .trim_start_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
        {
            current.push(component);
            if current.exists() {
                let metadata = fs::symlink_metadata(&current).map_err(io_error)?;
                if metadata.file_type().is_symlink() {
                    return Err(FsError::PermissionDenied);
                }
            }
        }
        Ok(current)
    }

    fn ensure_parent(&self, path: &Path) -> FsResult<()> {
        let parent = path.parent().ok_or(FsError::InvalidPath)?;
        fs::create_dir_all(parent).map_err(io_error)
    }
}

impl VirtualFileSystem for HostDirectoryFileSystem {
    fn stat(&self, path: &GuestPath) -> FsResult<Stat> {
        let host = self.host_path(path)?;
        let metadata = fs::metadata(&host).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                FsError::NotFound(path.as_str().into())
            } else {
                io_error(error)
            }
        })?;
        let entry_type = if metadata.is_file() {
            EntryType::File
        } else if metadata.is_dir() {
            EntryType::Directory
        } else {
            return Err(FsError::Backend("unsupported host entry type".into()));
        };
        Ok(Stat {
            entry_type,
            size: metadata.len(),
            read_only: self.read_only || metadata.permissions().readonly(),
        })
    }

    fn read_file(&self, path: &GuestPath, offset: u64, output: &mut [u8]) -> FsResult<usize> {
        let host = self.host_path(path)?;
        let mut file = fs::File::open(host).map_err(io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        file.read(output).map_err(io_error)
    }

    fn write_file(&self, path: &GuestPath, offset: u64, input: &[u8]) -> FsResult<usize> {
        self.ensure_writable()?;
        let host = self.host_path(path)?;
        self.ensure_parent(&host)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(host)
            .map_err(io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        file.write(input).map_err(io_error)
    }

    fn create_file(&self, path: &GuestPath, size: u64) -> FsResult<()> {
        self.ensure_writable()?;
        let host = self.host_path(path)?;
        self.ensure_parent(&host)?;
        if host.exists() {
            return Err(FsError::AlreadyExists(path.as_str().into()));
        }
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(host)
            .map_err(io_error)?;
        file.set_len(size).map_err(io_error)
    }

    fn delete_file(&self, path: &GuestPath) -> FsResult<()> {
        self.ensure_writable()?;
        fs::remove_file(self.host_path(path)?).map_err(io_error)
    }

    fn create_directory(&self, path: &GuestPath) -> FsResult<()> {
        self.ensure_writable()?;
        fs::create_dir_all(self.host_path(path)?).map_err(io_error)
    }

    fn delete_directory(&self, path: &GuestPath) -> FsResult<()> {
        self.ensure_writable()?;
        fs::remove_dir(self.host_path(path)?).map_err(io_error)
    }

    fn read_directory(&self, path: &GuestPath) -> FsResult<Vec<DirectoryEntry>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.host_path(path)?).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let file_type = entry.file_type().map_err(io_error)?;
            if file_type.is_symlink() {
                continue;
            }
            let metadata = entry.metadata().map_err(io_error)?;
            let entry_type = if file_type.is_dir() {
                EntryType::Directory
            } else if file_type.is_file() {
                EntryType::File
            } else {
                continue;
            };
            out.push(DirectoryEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                entry_type,
                size: metadata.len(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn rename(&self, from: &GuestPath, to: &GuestPath) -> FsResult<()> {
        self.ensure_writable()?;
        let from = self.host_path(from)?;
        let to = self.host_path(to)?;
        self.ensure_parent(&to)?;
        fs::rename(from, to).map_err(io_error)
    }

    fn set_file_size(&self, path: &GuestPath, size: u64) -> FsResult<()> {
        self.ensure_writable()?;
        let file = OpenOptions::new()
            .write(true)
            .open(self.host_path(path)?)
            .map_err(io_error)?;
        file.set_len(size).map_err(io_error)
    }

    fn flush(&self) -> FsResult<()> {
        sync_tree(&self.root)
    }
}

fn sync_tree(root: &Path) -> FsResult<()> {
    fn walk(path: &Path) -> FsResult<()> {
        for entry in fs::read_dir(path).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let ty = entry.file_type().map_err(io_error)?;
            if ty.is_symlink() {
                continue;
            }
            if ty.is_dir() {
                walk(&entry.path())?;
            } else if ty.is_file() {
                fs::File::open(entry.path())
                    .map_err(io_error)?
                    .sync_all()
                    .map_err(io_error)?;
            }
        }
        fs::File::open(path)
            .map_err(io_error)?
            .sync_all()
            .map_err(io_error)
    }
    walk(root)
}

fn io_error(error: std::io::Error) -> FsError {
    FsError::Backend(error.to_string())
}
