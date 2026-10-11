// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Staged save-data transactions.
//!
//! A transaction can remain purely in-memory for temporary/cache data, or attach a
//! `PersistentSaveJournal` for crash recovery. Persistent mode follows the durable ordering:
//! journal → fsync journal → apply VFS mutations → flush VFS → remove journal.

use crate::{
    FsResult, GuestPath, VirtualFileSystem,
    persistent_journal::{PersistentSaveJournal, apply_operations},
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum JournalOperation {
    CreateFile {
        path: GuestPath,
        size: u64,
    },
    Write {
        path: GuestPath,
        offset: u64,
        data: Vec<u8>,
    },
    DeleteFile {
        path: GuestPath,
    },
    CreateDirectory {
        path: GuestPath,
    },
    DeleteDirectory {
        path: GuestPath,
    },
    Rename {
        from: GuestPath,
        to: GuestPath,
    },
}

#[derive(Clone)]
pub struct SaveTransaction {
    backend: Arc<dyn VirtualFileSystem>,
    operations: Vec<JournalOperation>,
    persistent_journal: Option<PersistentSaveJournal>,
}

impl std::fmt::Debug for SaveTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveTransaction")
            .field("operations", &self.operations)
            .field("persistent_journal", &self.persistent_journal)
            .finish_non_exhaustive()
    }
}

impl SaveTransaction {
    pub fn new(backend: Arc<dyn VirtualFileSystem>) -> Self {
        Self {
            backend,
            operations: Vec::new(),
            persistent_journal: None,
        }
    }
    pub fn persistent(backend: Arc<dyn VirtualFileSystem>, journal: PersistentSaveJournal) -> Self {
        Self {
            backend,
            operations: Vec::new(),
            persistent_journal: Some(journal),
        }
    }
    pub fn operations(&self) -> &[JournalOperation] {
        &self.operations
    }
    pub fn create_file(&mut self, path: GuestPath, size: u64) {
        self.operations
            .push(JournalOperation::CreateFile { path, size });
    }
    pub fn write(&mut self, path: GuestPath, offset: u64, data: Vec<u8>) {
        self.operations
            .push(JournalOperation::Write { path, offset, data });
    }
    pub fn delete_file(&mut self, path: GuestPath) {
        self.operations.push(JournalOperation::DeleteFile { path });
    }
    pub fn create_directory(&mut self, path: GuestPath) {
        self.operations
            .push(JournalOperation::CreateDirectory { path });
    }
    pub fn delete_directory(&mut self, path: GuestPath) {
        self.operations
            .push(JournalOperation::DeleteDirectory { path });
    }
    pub fn rename(&mut self, from: GuestPath, to: GuestPath) {
        self.operations.push(JournalOperation::Rename { from, to });
    }

    pub fn commit(mut self) -> FsResult<()> {
        if let Some(journal) = &self.persistent_journal {
            journal.write_transaction(&self.operations)?;
        }
        apply_operations(self.backend.as_ref(), &self.operations)?;
        self.backend.flush()?;
        if let Some(journal) = &self.persistent_journal {
            journal.clear()?;
        }
        self.operations.clear();
        Ok(())
    }

    /// Drops all staged changes. No journal is written until `commit`, so rollback is side-effect free.
    pub fn rollback(mut self) {
        self.operations.clear();
    }
}
