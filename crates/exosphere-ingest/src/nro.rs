// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! NRO0 parser.

use crate::{IngestError, reader::Reader};

pub const NRO_HEADER_SIZE: usize = 0x80;

#[derive(Debug, Clone, Copy)]
pub struct Segment {
    pub memory_offset: u32,
    pub size: u32,
}

#[derive(Debug, Clone)]
pub struct NroHeader {
    pub version: u32,
    pub file_size: u32,
    pub flags: u32,
    pub text: Segment,
    pub rodata: Segment,
    pub data: Segment,
    pub bss_size: u32,
    pub module_id: [u8; 32],
    pub dso_handle_offset: u32,
    pub embedded_offset: u32,
    pub embedded_size: u32,
    pub dynstr_offset: u32,
    pub dynstr_size: u32,
    pub dynsym_offset: u32,
    pub dynsym_size: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct AssetSection {
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct NroAssets {
    pub icon: AssetSection,
    pub nacp: AssetSection,
    pub romfs: AssetSection,
}

#[derive(Debug, Clone)]
pub struct Nro<'a> {
    pub header: NroHeader,
    pub text: &'a [u8],
    pub rodata: &'a [u8],
    pub data: &'a [u8],
    pub assets: Option<NroAssets>,
}

pub fn parse(bytes: &[u8]) -> Result<Nro<'_>, IngestError> {
    let r = Reader::new(bytes);
    if r.slice(0x10, 4)? != b"NRO0" {
        return Err(IngestError::Invalid);
    }
    if r.len() < NRO_HEADER_SIZE {
        return Err(IngestError::Invalid);
    }

    let header = NroHeader {
        version: r.u32(0x14)?,
        file_size: r.u32(0x18)?,
        flags: r.u32(0x1c)?,
        text: Segment {
            memory_offset: r.u32(0x20)?,
            size: r.u32(0x24)?,
        },
        rodata: Segment {
            memory_offset: r.u32(0x28)?,
            size: r.u32(0x2c)?,
        },
        data: Segment {
            memory_offset: r.u32(0x30)?,
            size: r.u32(0x34)?,
        },
        bss_size: r.u32(0x38)?,
        module_id: r.array(0x40)?,
        dso_handle_offset: r.u32(0x60)?,
        embedded_offset: r.u32(0x68)?,
        embedded_size: r.u32(0x6c)?,
        dynstr_offset: r.u32(0x70)?,
        dynstr_size: r.u32(0x74)?,
        dynsym_offset: r.u32(0x78)?,
        dynsym_size: r.u32(0x7c)?,
    };
    let declared = header.file_size as usize;
    if declared < NRO_HEADER_SIZE || declared > bytes.len() {
        return Err(IngestError::Invalid);
    }

    // Assets may follow the codebin but are not executable segment storage.
    // Bound segment reads to the declared program, not the entire container.
    let r = Reader::new(&bytes[..declared]);
    let segments = [header.text, header.rodata, header.data];
    for (index, left) in segments.iter().enumerate() {
        if left.size == 0 {
            continue;
        }
        let left_start = u64::from(left.memory_offset);
        let left_end = left_start + u64::from(left.size);
        for right in &segments[index + 1..] {
            let right_start = u64::from(right.memory_offset);
            let right_end = right_start + u64::from(right.size);
            if right.size != 0 && left_start < right_end && right_start < left_end {
                return Err(IngestError::Invalid);
            }
        }
    }
    let text = r.slice(
        header.text.memory_offset as usize,
        header.text.size as usize,
    )?;
    let rodata = r.slice(
        header.rodata.memory_offset as usize,
        header.rodata.size as usize,
    )?;
    let data = r.slice(
        header.data.memory_offset as usize,
        header.data.size as usize,
    )?;

    let assets = parse_assets(bytes, declared)?;
    Ok(Nro {
        header,
        text,
        rodata,
        data,
        assets,
    })
}

fn parse_assets(bytes: &[u8], asset_offset: usize) -> Result<Option<NroAssets>, IngestError> {
    let tail = bytes.get(asset_offset..).ok_or(IngestError::Invalid)?;
    if tail.get(..4) != Some(b"ASET") {
        return Ok(None);
    }
    let r = Reader::new(tail);
    if r.u32(4)? != 0 {
        return Err(IngestError::Unsupported);
    }
    let section = |off: usize| -> Result<AssetSection, IngestError> {
        Ok(AssetSection {
            offset: r.u64(off)?,
            size: r.u64(off + 8)?,
        })
    };
    let assets = NroAssets {
        icon: section(0x08)?,
        nacp: section(0x18)?,
        romfs: section(0x28)?,
    };
    for s in [assets.icon, assets.nacp, assets.romfs] {
        if s.size == 0 {
            continue;
        }
        let end = s.offset.checked_add(s.size).ok_or(IngestError::Invalid)?;
        if end > tail.len() as u64 {
            return Err(IngestError::Invalid);
        }
    }
    Ok(Some(assets))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut bytes = vec![0; 192];
        bytes[0x10..0x14].copy_from_slice(b"NRO0");
        bytes[0x18..0x1c].copy_from_slice(&192u32.to_le_bytes());
        for (offset, start, size) in [(0x20, 0u32, 128u32), (0x28, 128, 32), (0x30, 160, 32)] {
            bytes[offset..offset + 4].copy_from_slice(&start.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&size.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn segment_bounds_exclude_trailing_assets_and_reject_overlap() {
        let mut bytes = fixture();
        bytes.extend_from_slice(&[0xaa; 64]);
        let nro = parse(&bytes).unwrap();
        assert_eq!(
            (nro.text.len(), nro.rodata.len(), nro.data.len()),
            (128, 32, 32)
        );
        for offset in [0x20, 0x28, 0x30] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 4].copy_from_slice(&192u32.to_le_bytes());
            assert!(
                parse(&invalid).is_err(),
                "segment {offset:x} used trailing bytes"
            );
        }
        for start in [0u32, 127, 159] {
            let mut invalid = bytes.clone();
            invalid[0x28..0x2c].copy_from_slice(&start.to_le_bytes());
            assert!(parse(&invalid).is_err(), "overlapping rodata {start}");
        }
        bytes[0x2c..0x30].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse(&bytes).is_ok(), "empty segments do not overlap");
    }

    #[test]
    fn asset_bounds_use_relative_wide_offsets_without_truncation() {
        let mut bytes = fixture();
        bytes.extend_from_slice(&[0; 64]);
        bytes[192..196].copy_from_slice(b"ASET");
        bytes[200..208].copy_from_slice(&56u64.to_le_bytes());
        bytes[208..216].copy_from_slice(&8u64.to_le_bytes());
        let nro = parse(&bytes).unwrap();
        assert_eq!(nro.assets.unwrap().icon.offset, 56);
        assert_eq!(nro.data.len(), 32);
        for offset in [57, 1u64 << 32, u64::MAX] {
            bytes[200..208].copy_from_slice(&offset.to_le_bytes());
            assert!(parse(&bytes).is_err(), "asset offset {offset}");
        }
        bytes.truncate(196);
        assert!(parse(&bytes).is_err(), "truncated ASET header");
        assert!(parse_assets(&bytes, usize::MAX).is_err());
    }
}
