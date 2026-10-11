// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Bounds-checked little-endian binary reader used by all ingest parsers.

use crate::IngestError;

/// A zero-copy reader over one immutable input buffer.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Creates a reader over `bytes` without copying them.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
    pub const fn len(&self) -> usize {
        self.bytes.len()
    }
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    pub fn slice(&self, offset: usize, len: usize) -> Result<&'a [u8], IngestError> {
        let end = offset.checked_add(len).ok_or(IngestError::Invalid)?;
        self.bytes.get(offset..end).ok_or(IngestError::Invalid)
    }
    pub fn u8(&self, offset: usize) -> Result<u8, IngestError> {
        self.bytes.get(offset).copied().ok_or(IngestError::Invalid)
    }
    pub fn u16(&self, offset: usize) -> Result<u16, IngestError> {
        Ok(u16::from_le_bytes(
            self.slice(offset, 2)?
                .try_into()
                .map_err(|_| IngestError::Invalid)?,
        ))
    }
    pub fn u32(&self, offset: usize) -> Result<u32, IngestError> {
        Ok(u32::from_le_bytes(
            self.slice(offset, 4)?
                .try_into()
                .map_err(|_| IngestError::Invalid)?,
        ))
    }
    pub fn i32(&self, offset: usize) -> Result<i32, IngestError> {
        Ok(i32::from_le_bytes(
            self.slice(offset, 4)?
                .try_into()
                .map_err(|_| IngestError::Invalid)?,
        ))
    }
    pub fn u64(&self, offset: usize) -> Result<u64, IngestError> {
        Ok(u64::from_le_bytes(
            self.slice(offset, 8)?
                .try_into()
                .map_err(|_| IngestError::Invalid)?,
        ))
    }
    pub fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N], IngestError> {
        self.slice(offset, N)?
            .try_into()
            .map_err(|_| IngestError::Invalid)
    }
    /// Reads a NUL-terminated UTF-8 string bounded by `max_len`.
    pub fn cstr(&self, offset: usize, max_len: usize) -> Result<&'a str, IngestError> {
        let s = self.slice(offset, max_len)?;
        let n = s.iter().position(|b| *b == 0).ok_or(IngestError::Invalid)?;
        std::str::from_utf8(&s[..n]).map_err(|_| IngestError::Invalid)
    }
}
