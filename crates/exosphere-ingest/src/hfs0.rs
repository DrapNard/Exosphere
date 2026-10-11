// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! HFS0 parser used by XCI game-card partitions.

use crate::{IngestError, reader::Reader};

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub hashed_region_size: u32,
    pub hashed_region_offset: u64,
    pub hash: [u8; 32],
}
#[derive(Debug, Clone)]
pub struct Hfs0 {
    pub entries: Vec<Entry>,
    pub data_offset: usize,
}

pub fn parse(bytes: &[u8]) -> Result<Hfs0, IngestError> {
    let r = Reader::new(bytes);
    if r.slice(0, 4)? != b"HFS0" {
        return Err(IngestError::Invalid);
    }
    let count = r.u32(4)? as usize;
    let strings_size = r.u32(8)? as usize;
    let entries_size = count.checked_mul(0x40).ok_or(IngestError::Invalid)?;
    let strings_off = 0x10usize
        .checked_add(entries_size)
        .ok_or(IngestError::Invalid)?;
    let data_offset = strings_off
        .checked_add(strings_size)
        .ok_or(IngestError::Invalid)?;
    r.slice(strings_off, strings_size)?;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let o = 0x10 + i * 0x40;
        let rel = r.u64(o)?;
        let size = r.u64(o + 8)?;
        let str_off = r.u32(o + 0x10)? as usize;
        let hashed_region_size = r.u32(o + 0x14)?;
        let hashed_region_offset = r.u64(o + 0x18)?;
        let hash = r.array(o + 0x20)?;
        if str_off >= strings_size {
            return Err(IngestError::Invalid);
        }
        let tail = &bytes[strings_off + str_off..strings_off + strings_size];
        let nul = tail
            .iter()
            .position(|b| *b == 0)
            .ok_or(IngestError::Invalid)?;
        let name = std::str::from_utf8(&tail[..nul])
            .map_err(|_| IngestError::Invalid)?
            .to_owned();
        let offset = (data_offset as u64)
            .checked_add(rel)
            .ok_or(IngestError::Invalid)?;
        let end = offset.checked_add(size).ok_or(IngestError::Invalid)? as usize;
        if end > bytes.len() {
            return Err(IngestError::Invalid);
        }
        entries.push(Entry {
            name,
            offset,
            size,
            hashed_region_size,
            hashed_region_offset,
            hash,
        });
    }
    Ok(Hfs0 {
        entries,
        data_offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_one_entry() {
        let mut b = vec![0u8; 0x10 + 0x40 + 8 + 3];
        b[0..4].copy_from_slice(b"HFS0");
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b[8..12].copy_from_slice(&8u32.to_le_bytes());
        b[0x18..0x20].copy_from_slice(&3u64.to_le_bytes());
        b[0x10 + 0x10..0x10 + 0x14].copy_from_slice(&0u32.to_le_bytes());
        b[0x50..0x54].copy_from_slice(b"foo\0");
        b[0x58..0x5b].copy_from_slice(b"abc");
        let h = parse(&b).unwrap();
        assert_eq!(h.entries[0].name, "foo");
        assert_eq!(h.entries[0].size, 3);
    }
}
