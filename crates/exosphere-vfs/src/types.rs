// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Common VFS interfaces.

use crate::path::GuestPath;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FsError {
    #[error("invalid guest path")]
    InvalidPath,
    #[error("guest path attempts to escape its mount")]
    PathEscapesMount,
    #[error("entry not found: {0}")]
    NotFound(String),
    #[error("entry already exists: {0}")]
    AlreadyExists(String),
    #[error("filesystem is read-only")]
    ReadOnly,
    #[error("permission denied")]
    PermissionDenied,
    #[error("not a file")]
    NotAFile,
    #[error("not a directory")]
    NotADirectory,
    #[error("invalid offset or size")]
    InvalidRange,
    #[error("backend failure: {0}")]
    Backend(String),
}
pub type FsResult<T> = Result<T, FsError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub entry_type: EntryType,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub entry_type: EntryType,
    pub size: u64,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenMode(pub u8);
impl OpenMode {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const APPEND: Self = Self(1 << 2);
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// File-oriented guest filesystem boundary corresponding to Horizon's `IFileSystem` family.
pub trait VirtualFileSystem: Send + Sync {
    fn stat(&self, path: &GuestPath) -> FsResult<Stat>;
    fn read_file(&self, path: &GuestPath, offset: u64, output: &mut [u8]) -> FsResult<usize>;
    fn write_file(&self, path: &GuestPath, offset: u64, input: &[u8]) -> FsResult<usize>;
    fn create_file(&self, path: &GuestPath, size: u64) -> FsResult<()>;
    fn delete_file(&self, path: &GuestPath) -> FsResult<()>;
    fn create_directory(&self, path: &GuestPath) -> FsResult<()>;
    fn delete_directory(&self, path: &GuestPath) -> FsResult<()>;
    fn read_directory(&self, path: &GuestPath) -> FsResult<Vec<DirectoryEntry>>;
    fn rename(&self, from: &GuestPath, to: &GuestPath) -> FsResult<()>;
    /// Resizes a file. Backends that cannot resize return `PermissionDenied`.
    fn set_file_size(&self, _path: &GuestPath, _size: u64) -> FsResult<()> {
        Err(FsError::PermissionDenied)
    }
    /// Returns free bytes visible to the guest when the backend can provide a meaningful value.
    fn free_space(&self, _path: &GuestPath) -> FsResult<u64> {
        Ok(u64::MAX)
    }
    /// Returns total bytes visible to the guest when the backend can provide a meaningful value.
    fn total_space(&self, _path: &GuestPath) -> FsResult<u64> {
        Ok(u64::MAX)
    }
    fn flush(&self) -> FsResult<()> {
        Ok(())
    }
}

/// Block-oriented guest storage boundary corresponding to Horizon's `IStorage` family.
pub trait VirtualStorage: Send + Sync {
    fn size(&self) -> FsResult<u64>;
    fn read_at(&self, offset: u64, output: &mut [u8]) -> FsResult<()>;
    fn write_at(&self, offset: u64, input: &[u8]) -> FsResult<()>;
    /// Resizes storage when the concrete backend supports it. Read-only/fixed images reject this.
    fn set_size(&self, _size: u64) -> FsResult<()> {
        Err(FsError::PermissionDenied)
    }
    fn flush(&self) -> FsResult<()>;
}
