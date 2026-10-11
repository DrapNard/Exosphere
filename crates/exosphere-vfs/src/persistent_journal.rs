// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Crash-recoverable save-data journal.
//!
//! The journal is deliberately independent from the host filesystem layout. Operations are encoded
//! into a compact Exosphere-owned binary stream, `fsync`ed, then applied to the target VFS. A crash
//! between those two steps leaves enough information for `recover()` to replay the transaction.

use crate::{FsError, FsResult, GuestPath, VirtualFileSystem, transaction::JournalOperation};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const MAGIC: &[u8; 8] = b"EXOJNL01";

#[derive(Debug, Clone)]
pub struct PersistentSaveJournal {
    path: PathBuf,
}

impl PersistentSaveJournal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Durably records a complete transaction before the backing VFS is mutated.
    pub fn write_transaction(&self, operations: &[JournalOperation]) -> FsResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let temporary = self.path.with_extension("journal.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(io_error)?;
        file.write_all(MAGIC).map_err(io_error)?;
        file.write_all(&(operations.len() as u32).to_le_bytes())
            .map_err(io_error)?;
        for operation in operations {
            encode_operation(&mut file, operation)?;
        }
        file.sync_all().map_err(io_error)?;
        fs::rename(&temporary, &self.path).map_err(io_error)?;
        if let Some(parent) = self.path.parent() {
            if let Ok(directory) = File::open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(())
    }

    /// Removes a transaction only after the backing VFS has been flushed successfully.
    pub fn clear(&self) -> FsResult<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }

    pub fn load(&self) -> FsResult<Vec<JournalOperation>> {
        let mut file = File::open(&self.path).map_err(io_error)?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic).map_err(io_error)?;
        if &magic != MAGIC {
            return Err(FsError::Backend(
                "invalid Exosphere save journal magic".into(),
            ));
        }
        let count = read_u32(&mut file)? as usize;
        if count > 1_000_000 {
            return Err(FsError::Backend(
                "save journal operation count is unreasonable".into(),
            ));
        }
        let mut operations = Vec::with_capacity(count);
        for _ in 0..count {
            operations.push(decode_operation(&mut file)?);
        }
        Ok(operations)
    }

    /// Replays an interrupted transaction, flushes the backing VFS, then clears the journal.
    pub fn recover(&self, backend: Arc<dyn VirtualFileSystem>) -> FsResult<bool> {
        if !self.path.exists() {
            return Ok(false);
        }
        let operations = self.load()?;
        apply_operations(backend.as_ref(), &operations)?;
        backend.flush()?;
        self.clear()?;
        Ok(true)
    }
}

pub(crate) fn apply_operations(
    backend: &dyn VirtualFileSystem,
    operations: &[JournalOperation],
) -> FsResult<()> {
    for operation in operations {
        match operation {
            JournalOperation::CreateFile { path, size } => backend.create_file(path, *size)?,
            JournalOperation::Write { path, offset, data } => {
                backend.write_file(path, *offset, data)?;
            }
            JournalOperation::DeleteFile { path } => backend.delete_file(path)?,
            JournalOperation::CreateDirectory { path } => backend.create_directory(path)?,
            JournalOperation::DeleteDirectory { path } => backend.delete_directory(path)?,
            JournalOperation::Rename { from, to } => backend.rename(from, to)?,
        }
    }
    Ok(())
}

fn encode_operation(mut writer: impl Write, operation: &JournalOperation) -> FsResult<()> {
    match operation {
        JournalOperation::CreateFile { path, size } => {
            writer.write_all(&[1]).map_err(io_error)?;
            write_path(&mut writer, path)?;
            writer.write_all(&size.to_le_bytes()).map_err(io_error)?;
        }
        JournalOperation::Write { path, offset, data } => {
            writer.write_all(&[2]).map_err(io_error)?;
            write_path(&mut writer, path)?;
            writer.write_all(&offset.to_le_bytes()).map_err(io_error)?;
            writer
                .write_all(&(data.len() as u64).to_le_bytes())
                .map_err(io_error)?;
            writer.write_all(data).map_err(io_error)?;
        }
        JournalOperation::DeleteFile { path } => {
            writer.write_all(&[3]).map_err(io_error)?;
            write_path(&mut writer, path)?;
        }
        JournalOperation::CreateDirectory { path } => {
            writer.write_all(&[4]).map_err(io_error)?;
            write_path(&mut writer, path)?;
        }
        JournalOperation::DeleteDirectory { path } => {
            writer.write_all(&[5]).map_err(io_error)?;
            write_path(&mut writer, path)?;
        }
        JournalOperation::Rename { from, to } => {
            writer.write_all(&[6]).map_err(io_error)?;
            write_path(&mut writer, from)?;
            write_path(&mut writer, to)?;
        }
    }
    Ok(())
}

fn decode_operation(mut reader: impl Read) -> FsResult<JournalOperation> {
    let mut opcode = [0u8; 1];
    reader.read_exact(&mut opcode).map_err(io_error)?;
    Ok(match opcode[0] {
        1 => JournalOperation::CreateFile {
            path: read_path(&mut reader)?,
            size: read_u64(&mut reader)?,
        },
        2 => {
            let path = read_path(&mut reader)?;
            let offset = read_u64(&mut reader)?;
            let len = read_u64(&mut reader)?;
            let len = usize::try_from(len).map_err(|_| FsError::InvalidRange)?;
            if len > 512 * 1024 * 1024 {
                return Err(FsError::Backend(
                    "single journal write exceeds safety limit".into(),
                ));
            }
            let mut data = vec![0u8; len];
            reader.read_exact(&mut data).map_err(io_error)?;
            JournalOperation::Write { path, offset, data }
        }
        3 => JournalOperation::DeleteFile {
            path: read_path(&mut reader)?,
        },
        4 => JournalOperation::CreateDirectory {
            path: read_path(&mut reader)?,
        },
        5 => JournalOperation::DeleteDirectory {
            path: read_path(&mut reader)?,
        },
        6 => JournalOperation::Rename {
            from: read_path(&mut reader)?,
            to: read_path(&mut reader)?,
        },
        other => {
            return Err(FsError::Backend(format!(
                "unknown save journal opcode {other}"
            )));
        }
    })
}

fn write_path(mut writer: impl Write, path: &GuestPath) -> FsResult<()> {
    let bytes = path.as_str().as_bytes();
    let len = u32::try_from(bytes.len()).map_err(|_| FsError::InvalidRange)?;
    writer.write_all(&len.to_le_bytes()).map_err(io_error)?;
    writer.write_all(bytes).map_err(io_error)
}
fn read_path(mut reader: impl Read) -> FsResult<GuestPath> {
    let len = read_u32(&mut reader)? as usize;
    if len > 32 * 1024 {
        return Err(FsError::InvalidPath);
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes).map_err(io_error)?;
    let value = std::str::from_utf8(&bytes).map_err(|_| FsError::InvalidPath)?;
    GuestPath::parse(value)
}
fn read_u32(mut reader: impl Read) -> FsResult<u32> {
    let mut b = [0; 4];
    reader.read_exact(&mut b).map_err(io_error)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64(mut reader: impl Read) -> FsResult<u64> {
    let mut b = [0; 8];
    reader.read_exact(&mut b).map_err(io_error)?;
    Ok(u64::from_le_bytes(b))
}
fn io_error(error: std::io::Error) -> FsError {
    FsError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MemoryFileSystem;

    #[test]
    fn journal_roundtrip_and_recovery() {
        let root = std::env::temp_dir().join(format!("exosphere-journal-{}", std::process::id()));
        let journal = PersistentSaveJournal::new(root.join("save.journal"));
        let backend = Arc::new(MemoryFileSystem::new(false));
        let path = GuestPath::parse("/slot.dat").unwrap();
        let operations = vec![
            JournalOperation::CreateFile {
                path: path.clone(),
                size: 4,
            },
            JournalOperation::Write {
                path: path.clone(),
                offset: 0,
                data: b"SAVE".to_vec(),
            },
        ];
        journal.write_transaction(&operations).unwrap();
        assert!(journal.recover(backend.clone()).unwrap());
        let mut bytes = [0u8; 4];
        backend.read_file(&path, 0, &mut bytes).unwrap();
        assert_eq!(&bytes, b"SAVE");
        let _ = fs::remove_dir_all(root);
    }
}
