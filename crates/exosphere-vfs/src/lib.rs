// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Host-neutral virtual filesystem used by Horizon `fsp-srv`.
//!
//! This crate deliberately separates the filesystem visible to Switch software from Exosphere's
//! internal CAS/package/build storage. Guest paths are always resolved relative to an explicit
//! mount and can never escape to arbitrary host paths.

pub mod backend;
pub mod mount;
pub mod overlay;
pub mod path;
pub mod persistent_journal;
pub mod save;
pub mod transaction;
pub mod types;

pub use mount::{Mount, MountId, MountTable};
pub use path::GuestPath;
pub use types::{
    DirectoryEntry, EntryType, FsError, FsResult, OpenMode, Stat, VirtualFileSystem, VirtualStorage,
};

pub use transaction::{JournalOperation, SaveTransaction};

pub use persistent_journal::PersistentSaveJournal;
