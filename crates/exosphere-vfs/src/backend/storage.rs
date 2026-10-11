// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Simple byte-addressable storage backends used by Horizon `IStorage`.

use crate::{FsError, FsResult, VirtualStorage};
use std::sync::RwLock;

#[derive(Debug)]
pub struct MemoryStorage {
    bytes: RwLock<Vec<u8>>,
    writable: bool,
}

impl MemoryStorage {
    pub fn new(bytes: Vec<u8>, writable: bool) -> Self {
        Self {
            bytes: RwLock::new(bytes),
            writable,
        }
    }
}

impl VirtualStorage for MemoryStorage {
    fn size(&self) -> FsResult<u64> {
        Ok(self
            .bytes
            .read()
            .map_err(|_| FsError::Backend("storage lock poisoned".into()))?
            .len() as u64)
    }

    fn read_at(&self, offset: u64, output: &mut [u8]) -> FsResult<()> {
        let bytes = self
            .bytes
            .read()
            .map_err(|_| FsError::Backend("storage lock poisoned".into()))?;
        let start = usize::try_from(offset).map_err(|_| FsError::InvalidRange)?;
        let end = start
            .checked_add(output.len())
            .ok_or(FsError::InvalidRange)?;
        let source = bytes.get(start..end).ok_or(FsError::InvalidRange)?;
        output.copy_from_slice(source);
        Ok(())
    }

    fn write_at(&self, offset: u64, input: &[u8]) -> FsResult<()> {
        if !self.writable {
            return Err(FsError::ReadOnly);
        }
        let mut bytes = self
            .bytes
            .write()
            .map_err(|_| FsError::Backend("storage lock poisoned".into()))?;
        let start = usize::try_from(offset).map_err(|_| FsError::InvalidRange)?;
        let end = start
            .checked_add(input.len())
            .ok_or(FsError::InvalidRange)?;
        let target = bytes.get_mut(start..end).ok_or(FsError::InvalidRange)?;
        target.copy_from_slice(input);
        Ok(())
    }

    fn set_size(&self, size: u64) -> FsResult<()> {
        if !self.writable {
            return Err(FsError::ReadOnly);
        }
        let new_len = usize::try_from(size).map_err(|_| FsError::InvalidRange)?;
        self.bytes
            .write()
            .map_err(|_| FsError::Backend("storage lock poisoned".into()))?
            .resize(new_len, 0);
        Ok(())
    }

    fn flush(&self) -> FsResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_write_obeys_bounds_and_permissions() {
        let storage = MemoryStorage::new(vec![1, 2, 3, 4], true);
        let mut out = [0u8; 2];
        storage.read_at(1, &mut out).unwrap();
        assert_eq!(out, [2, 3]);
        storage.write_at(2, &[9, 8]).unwrap();
        let mut all = [0u8; 4];
        storage.read_at(0, &mut all).unwrap();
        assert_eq!(all, [1, 2, 9, 8]);
        assert!(storage.read_at(3, &mut [0u8; 2]).is_err());
    }
}
