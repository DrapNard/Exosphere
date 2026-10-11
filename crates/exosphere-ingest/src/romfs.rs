// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Nintendo Switch RomFS parser.
//!
//! This parser exposes the directory/file metadata tree without copying file payloads. Hash tables
//! surrounding RomFS inside NCA sections are handled by the NCA layer; this module begins at the
//! RomFS header itself.

use crate::{IngestError, reader::Reader};
use std::collections::HashSet;

const NONE: u32 = 0xffff_ffff;
#[derive(Debug, Clone)]
pub struct RomFsFile {
    pub path: String,
    pub data_offset: u64,
    pub size: u64,
}
#[derive(Debug, Clone)]
pub struct RomFs {
    pub file_data_offset: u64,
    pub files: Vec<RomFsFile>,
}

#[derive(Clone, Copy)]
struct Layout {
    dir_meta: u64,
    dir_meta_size: u64,
    file_meta: u64,
    file_meta_size: u64,
    file_data: u64,
}

pub fn parse(bytes: &[u8]) -> Result<RomFs, IngestError> {
    let r = Reader::new(bytes);
    if r.u64(0)? < 0x50 {
        return Err(IngestError::Invalid);
    }
    let l = Layout {
        dir_meta: r.u64(0x18)?,
        dir_meta_size: r.u64(0x20)?,
        file_meta: r.u64(0x38)?,
        file_meta_size: r.u64(0x40)?,
        file_data: r.u64(0x48)?,
    };
    range(bytes, l.dir_meta, l.dir_meta_size)?;
    range(bytes, l.file_meta, l.file_meta_size)?;
    if l.file_data as usize > bytes.len() {
        return Err(IngestError::Invalid);
    }
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    walk_dir(bytes, l, 0, "", &mut seen, &mut files, 0)?;
    Ok(RomFs {
        file_data_offset: l.file_data,
        files,
    })
}
fn range(bytes: &[u8], off: u64, size: u64) -> Result<(), IngestError> {
    let e = off.checked_add(size).ok_or(IngestError::Invalid)? as usize;
    if e > bytes.len() {
        Err(IngestError::Invalid)
    } else {
        Ok(())
    }
}
fn walk_dir(
    bytes: &[u8],
    l: Layout,
    off: u32,
    parent: &str,
    seen: &mut HashSet<u32>,
    out: &mut Vec<RomFsFile>,
    depth: usize,
) -> Result<(), IngestError> {
    if depth > 1024 || !seen.insert(off) {
        return Err(IngestError::Invalid);
    }
    let base = l.dir_meta as usize + off as usize;
    if off as u64 >= l.dir_meta_size {
        return Err(IngestError::Invalid);
    };
    let r = Reader::new(bytes);
    let sibling = r.u32(base + 4)?;
    let child = r.u32(base + 8)?;
    let file = r.u32(base + 0xc)?;
    let name_len = r.u32(base + 0x14)? as usize;
    let name =
        std::str::from_utf8(r.slice(base + 0x18, name_len)?).map_err(|_| IngestError::Invalid)?;
    let path = if name.is_empty() {
        parent.to_owned()
    } else if parent.is_empty() {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    };
    let mut fo = file;
    let mut fseen = HashSet::new();
    while fo != NONE {
        if !fseen.insert(fo) {
            return Err(IngestError::Invalid);
        };
        if fo as u64 >= l.file_meta_size {
            return Err(IngestError::Invalid);
        };
        let fb = l.file_meta as usize + fo as usize;
        let next = r.u32(fb + 4)?;
        let data_off = r.u64(fb + 8)?;
        let size = r.u64(fb + 0x10)?;
        let nlen = r.u32(fb + 0x1c)? as usize;
        let n = std::str::from_utf8(r.slice(fb + 0x20, nlen)?).map_err(|_| IngestError::Invalid)?;
        let abs = l
            .file_data
            .checked_add(data_off)
            .ok_or(IngestError::Invalid)?;
        range(bytes, abs, size)?;
        out.push(RomFsFile {
            path: if path.is_empty() {
                format!("/{n}")
            } else {
                format!("{path}/{n}")
            },
            data_offset: abs,
            size,
        });
        fo = next;
    }
    let mut ch = child;
    while ch != NONE {
        let cb = l.dir_meta as usize + ch as usize;
        let next = r.u32(cb + 4)?;
        walk_dir(bytes, l, ch, &path, seen, out, depth + 1)?;
        ch = next;
    }
    // Siblings are walked by the parent loop; root has no parent loop, so consume it here only when
    // explicitly encountered through metadata corruption-resistant standalone parsing.
    let _ = sibling;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_root_file() {
        // Minimal metadata-only RomFS with one file named "a" and one data byte.
        let mut b = vec![0u8; 0x200];
        b[0..8].copy_from_slice(&0x50u64.to_le_bytes());
        b[0x18..0x20].copy_from_slice(&0x80u64.to_le_bytes());
        b[0x20..0x28].copy_from_slice(&0x18u64.to_le_bytes());
        b[0x38..0x40].copy_from_slice(&0xa0u64.to_le_bytes());
        b[0x40..0x48].copy_from_slice(&0x21u64.to_le_bytes());
        b[0x48..0x50].copy_from_slice(&0x100u64.to_le_bytes());
        // root: no sibling/child, file offset 0, empty name
        b[0x84..0x88].copy_from_slice(&NONE.to_le_bytes());
        b[0x88..0x8c].copy_from_slice(&NONE.to_le_bytes());
        b[0x8c..0x90].copy_from_slice(&0u32.to_le_bytes());
        b[0x94..0x98].copy_from_slice(&0u32.to_le_bytes());
        // file entry @ 0xa0
        b[0xa4..0xa8].copy_from_slice(&NONE.to_le_bytes());
        b[0xa8..0xb0].copy_from_slice(&0u64.to_le_bytes());
        b[0xb0..0xb8].copy_from_slice(&1u64.to_le_bytes());
        b[0xbc..0xc0].copy_from_slice(&1u32.to_le_bytes());
        b[0xc0] = b'a';
        b[0x100] = 42;
        let rom = parse(&b).unwrap();
        assert_eq!(rom.files.len(), 1);
        assert_eq!(rom.files[0].path, "/a");
        assert_eq!(rom.files[0].size, 1);
    }
}
