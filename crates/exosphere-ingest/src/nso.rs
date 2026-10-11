// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! NSO0 parser, segment decompressor and integrity verifier.

use crate::{IngestError, reader::Reader};
use sha2::{Digest, Sha256};
pub const NSO_HEADER_SIZE: usize = 0x100;
#[derive(Debug, Clone, Copy)]
pub struct NsoSegment {
    pub file_offset: u32,
    pub memory_offset: u32,
    pub memory_size: u32,
    pub file_size: u32,
    pub compressed: bool,
    pub verify_hash: bool,
    pub expected_hash: [u8; 32],
}
#[derive(Debug, Clone)]
pub struct NsoHeader {
    pub version: u32,
    pub flags: u32,
    pub text: NsoSegment,
    pub rodata: NsoSegment,
    pub data: NsoSegment,
    pub bss_size: u32,
    pub module_id: [u8; 32],
    pub module_name_offset: u32,
    pub module_name_size: u32,
    pub embedded_offset: u32,
    pub embedded_size: u32,
    pub dynstr_offset: u32,
    pub dynstr_size: u32,
    pub dynsym_offset: u32,
    pub dynsym_size: u32,
    pub execute_only_memory: bool,
    pub zbic_compression: bool,
}
#[derive(Debug, Clone)]
pub struct LoadedNso {
    pub header: NsoHeader,
    pub text: Vec<u8>,
    pub rodata: Vec<u8>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSymbol {
    pub name: String,
    /// Link-time value relative to the mapped module base, not an NSO file offset.
    pub value: u64,
    pub size: u64,
}

/// Reads defined ELF64 function symbols from the NSO's bounded .rodata tables.
/// This is diagnostic metadata only: it neither applies nor validates relocations.
pub fn function_symbols(module: &LoadedNso) -> Result<Vec<FunctionSymbol>, IngestError> {
    let header = &module.header;
    if header.dynsym_size == 0 {
        return Ok(Vec::new());
    }
    let reader = Reader::new(&module.rodata);
    let strings = reader.slice(header.dynstr_offset as usize, header.dynstr_size as usize)?;
    let symbols = reader.slice(header.dynsym_offset as usize, header.dynsym_size as usize)?;
    if symbols.len() % 24 != 0 {
        return Err(IngestError::Invalid);
    }
    let mut output = Vec::new();
    let text_start = u64::from(header.text.memory_offset);
    let text_end = text_start
        .checked_add(module.text.len() as u64)
        .ok_or(IngestError::Invalid)?;
    for bytes in symbols.chunks_exact(24) {
        let entry = Reader::new(bytes);
        // Undefined imports and data/TLS symbols must not name a return PC.
        if entry.u8(4)? & 15 != 2 || entry.u16(6)? == 0 {
            continue;
        }
        let value = entry.u64(8)?;
        let size = entry.u64(16)?;
        let end = value.checked_add(size).ok_or(IngestError::Invalid)?;
        if value < text_start || value >= text_end || end > text_end {
            continue;
        }
        let name_offset = entry.u32(0)? as usize;
        let remaining = strings.get(name_offset..).ok_or(IngestError::Invalid)?;
        let length = remaining
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(IngestError::Invalid)?;
        // Names are printed in diagnostics; control characters are not allowed.
        let name = std::str::from_utf8(&remaining[..length]).map_err(|_| IngestError::Invalid)?;
        if name.chars().any(char::is_control) {
            return Err(IngestError::Invalid);
        }
        if !name.is_empty() {
            output.push(FunctionSymbol {
                name: name.into(),
                value,
                size,
            });
        }
    }
    Ok(output)
}
pub fn parse(bytes: &[u8]) -> Result<NsoHeader, IngestError> {
    let r = Reader::new(bytes);
    if r.slice(0, 4)? != b"NSO0" || r.len() < NSO_HEADER_SIZE {
        return Err(IngestError::Invalid);
    }
    let flags = r.u32(0xc)?;
    let mk = |fo, mo, ms, fs, cbit, hbit, ho| -> Result<NsoSegment, IngestError> {
        let s = NsoSegment {
            file_offset: r.u32(fo)?,
            memory_offset: r.u32(mo)?,
            memory_size: r.u32(ms)?,
            file_size: r.u32(fs)?,
            compressed: flags & (1u32 << cbit) != 0u32,
            verify_hash: flags & (1u32 << hbit) != 0u32,
            expected_hash: r.array(ho)?,
        };
        let e = (s.file_offset as usize)
            .checked_add(s.file_size as usize)
            .ok_or(IngestError::Invalid)?;
        if e > bytes.len() {
            return Err(IngestError::Invalid);
        }
        Ok(s)
    };
    Ok(NsoHeader {
        version: r.u32(4)?,
        flags,
        text: mk(0x10, 0x14, 0x18, 0x60, 0, 3, 0xa0)?,
        rodata: mk(0x20, 0x24, 0x28, 0x64, 1, 4, 0xc0)?,
        data: mk(0x30, 0x34, 0x38, 0x68, 2, 5, 0xe0)?,
        bss_size: r.u32(0x3c)?,
        module_id: r.array(0x40)?,
        module_name_offset: r.u32(0x1c)?,
        module_name_size: r.u32(0x2c)?,
        embedded_offset: r.u32(0x88)?,
        embedded_size: r.u32(0x8c)?,
        dynstr_offset: r.u32(0x90)?,
        dynstr_size: r.u32(0x94)?,
        dynsym_offset: r.u32(0x98)?,
        dynsym_size: r.u32(0x9c)?,
        execute_only_memory: flags & (1 << 6) != 0,
        zbic_compression: flags & (1 << 7) != 0,
    })
}
pub fn load(bytes: &[u8]) -> Result<LoadedNso, IngestError> {
    let h = parse(bytes)?;
    if h.zbic_compression {
        return Err(IngestError::Unsupported);
    };
    Ok(LoadedNso {
        text: load_segment(bytes, h.text)?,
        rodata: load_segment(bytes, h.rodata)?,
        data: load_segment(bytes, h.data)?,
        header: h,
    })
}
fn load_segment(bytes: &[u8], s: NsoSegment) -> Result<Vec<u8>, IngestError> {
    let src = &bytes[s.file_offset as usize..s.file_offset as usize + s.file_size as usize];
    let out = if s.compressed {
        lz4_flex::block::decompress(src, s.memory_size as usize)
            .map_err(|_| IngestError::Invalid)?
    } else {
        src.to_vec()
    };
    if out.len() != s.memory_size as usize {
        return Err(IngestError::Invalid);
    }
    if s.verify_hash {
        let got: [u8; 32] = Sha256::digest(&out).into();
        if got != s.expected_hash {
            return Err(IngestError::Integrity("NSO segment SHA-256 mismatch"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod symbol_tests {
    use super::*;

    fn fixture() -> LoadedNso {
        let mut raw = [0u8; NSO_HEADER_SIZE];
        raw[..4].copy_from_slice(b"NSO0");
        raw[0x18..0x1c].copy_from_slice(&128u32.to_le_bytes());
        raw[0x94..0x98].copy_from_slice(&16u32.to_le_bytes());
        raw[0x98..0x9c].copy_from_slice(&16u32.to_le_bytes());
        raw[0x9c..0xa0].copy_from_slice(&72u32.to_le_bytes());
        let mut rodata = vec![0u8; 88];
        rodata[..9].copy_from_slice(b"_Z4testv\0");
        rodata[20] = 0x12; // STB_GLOBAL | STT_FUNC
        rodata[22..24].copy_from_slice(&1u16.to_le_bytes());
        rodata[24..32].copy_from_slice(&64u64.to_le_bytes());
        rodata[32..40].copy_from_slice(&16u64.to_le_bytes());
        rodata[44] = 0x12; // Undefined import: no code range in this module.
        rodata[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        rodata[68] = 0x11; // STT_OBJECT, not a function.
        LoadedNso {
            header: parse(&raw).unwrap(),
            text: vec![0u8; 128],
            rodata,
            data: vec![],
        }
    }

    #[test]
    fn defined_functions_use_rodata_relative_tables_and_module_relative_values() {
        let module = fixture();
        assert_eq!(
            function_symbols(&module).unwrap(),
            vec![FunctionSymbol {
                name: "_Z4testv".into(),
                value: 64,
                size: 16
            }]
        );
    }

    #[test]
    fn malformed_symbol_tables_and_names_fail_boundedly() {
        for case in 0..6 {
            let mut module = fixture();
            match case {
                0 => module.header.dynsym_size -= 1,
                1 => module.header.dynstr_offset = u32::MAX,
                2 => module.rodata[16..20].copy_from_slice(&u32::MAX.to_le_bytes()),
                3 => module.rodata[..16].fill(b'x'),
                4 => module.rodata[0] = b'\n',
                5 => module.rodata[32..40].copy_from_slice(&u64::MAX.to_le_bytes()),
                _ => unreachable!(),
            }
            assert!(function_symbols(&module).is_err());
        }
        let mut module = fixture();
        module.header.dynsym_size = 0;
        assert!(function_symbols(&module).unwrap().is_empty());
    }
}
