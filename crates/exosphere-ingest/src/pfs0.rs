// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! PFS0 parser used by NSP/ExeFS-like partition filesystems.

use crate::{IngestError, reader::Reader};

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}
#[derive(Debug, Clone)]
pub struct Pfs0 {
    pub entries: Vec<Entry>,
    pub data_offset: usize,
}

pub fn parse(bytes: &[u8]) -> Result<Pfs0, IngestError> {
    parse_metadata(bytes, bytes.len() as u64)
}

/// Reads the partition index without loading game payloads into memory.
/// Entry offsets are absolute and validated against the complete container length.
pub fn read_index<R: std::io::Read + std::io::Seek>(input: &mut R) -> Result<Pfs0, IngestError> {
    use std::io::{Read, SeekFrom};
    let total_size = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 0x10];
    input.read_exact(&mut header)?;
    if &header[..4] != b"PFS0" {
        return Err(IngestError::Invalid);
    }
    let count = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    let strings_size = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
    let index_size = count
        .checked_mul(0x18)
        .and_then(|size| size.checked_add(0x10))
        .and_then(|size| size.checked_add(strings_size))
        .ok_or(IngestError::Invalid)?;
    if index_size as u64 > total_size {
        return Err(IngestError::Invalid);
    }
    let mut index = Vec::new();
    index
        .try_reserve_exact(index_size)
        .map_err(|_| IngestError::Invalid)?;
    index.extend_from_slice(&header);
    input
        .take((index_size - 0x10) as u64)
        .read_to_end(&mut index)?;
    parse_metadata(&index, total_size)
}

fn parse_metadata(bytes: &[u8], total_size: u64) -> Result<Pfs0, IngestError> {
    let r = Reader::new(bytes);
    if r.slice(0, 4)? != b"PFS0" {
        return Err(IngestError::Invalid);
    }
    let count = r.u32(4)? as usize;
    let strings_size = r.u32(8)? as usize;
    let entries_size = count.checked_mul(0x18).ok_or(IngestError::Invalid)?;
    let strings_off = 0x10usize
        .checked_add(entries_size)
        .ok_or(IngestError::Invalid)?;
    let data_offset = strings_off
        .checked_add(strings_size)
        .ok_or(IngestError::Invalid)?;
    r.slice(strings_off, strings_size)?;
    if data_offset > bytes.len() {
        return Err(IngestError::Invalid);
    }

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let off = 0x10 + i * 0x18;
        let file_off = r.u64(off)?;
        let size = r.u64(off + 8)?;
        let str_off = r.u32(off + 0x10)? as usize;
        if str_off >= strings_size {
            return Err(IngestError::Invalid);
        }
        let st = &bytes[strings_off + str_off..strings_off + strings_size];
        let nul = st
            .iter()
            .position(|&b| b == 0)
            .ok_or(IngestError::Invalid)?;
        let name = std::str::from_utf8(&st[..nul])
            .map_err(|_| IngestError::Invalid)?
            .to_owned();
        let abs = (data_offset as u64)
            .checked_add(file_off)
            .ok_or(IngestError::Invalid)?;
        let end = abs.checked_add(size).ok_or(IngestError::Invalid)?;
        if end > total_size {
            return Err(IngestError::Invalid);
        }
        entries.push(Entry {
            name,
            offset: abs,
            size,
        });
    }
    Ok(Pfs0 {
        entries,
        data_offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn container() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x10 + 0x18 + 6 + 4];
        bytes[..4].copy_from_slice(b"PFS0");
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&6u32.to_le_bytes());
        bytes[0x18..0x20].copy_from_slice(&4u64.to_le_bytes());
        bytes[0x28..0x2e].copy_from_slice(b"a.nca\0");
        bytes
    }

    #[test]
    fn index_does_not_read_payload() {
        let bytes = container();
        let expected = parse(&bytes).unwrap();
        let mut input = Cursor::new(bytes);
        let actual = read_index(&mut input).unwrap();
        assert_eq!(actual.entries[0].offset, expected.entries[0].offset);
        assert_eq!(actual.entries[0].name, "a.nca");
        assert_eq!(input.position(), actual.data_offset as u64);
    }

    #[test]
    fn index_rejects_truncated_payload() {
        let mut bytes = container();
        bytes.pop();
        assert!(read_index(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn index_rejects_overflowing_entry() {
        let mut bytes = container();
        bytes[0x10..0x18].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(read_index(&mut Cursor::new(bytes)).is_err());
    }
}
