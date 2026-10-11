// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! NCA plaintext/header model and pluggable crypto boundary.
//!
//! Raw NCAs encrypt the header and most sections. Exosphere deliberately does not own console keys:
//! callers provide a `NcaCryptoProvider` implementation backed by user-supplied key material.

use crate::{IngestError, reader::Reader};

pub const HEADER_AND_FS_HEADERS: usize = 0xC00;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    RomFs,
    PartitionFs,
    Unknown(u8),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionType {
    Auto,
    None,
    AesXts,
    AesCtr,
    AesCtrEx,
    AesCtrSkipLayerHash,
    AesCtrExSkipLayerHash,
    Unknown(u8),
}
#[derive(Debug, Clone, Copy)]
pub struct SectionEntry {
    pub media_start: u32,
    pub media_end: u32,
}
impl SectionEntry {
    pub fn byte_offset(self) -> u64 {
        self.media_start as u64 * 0x200
    }
    pub fn byte_size(self) -> u64 {
        (self.media_end - self.media_start) as u64 * 0x200
    }
}
#[derive(Debug, Clone)]
pub struct FsHeader {
    pub version: u16,
    pub fs_type: FsType,
    pub hash_type: u8,
    pub encryption: EncryptionType,
    pub generation: u32,
    pub secure_value: u32,
    pub raw: [u8; 0x200],
}
#[derive(Debug, Clone)]
pub struct NcaHeader {
    pub magic: [u8; 4],
    pub distribution_type: u8,
    pub content_type: u8,
    pub key_generation_old: u8,
    pub key_area_key_index: u8,
    pub content_size: u64,
    pub program_id: u64,
    pub content_index: u32,
    pub sdk_addon_version: u32,
    pub key_generation: u8,
    pub rights_id: [u8; 16],
    pub encrypted_key_area: [u8; 64],
    pub sections: [Option<SectionEntry>; 4],
    pub fs_headers: [FsHeader; 4],
}

/// User-owned cryptographic bridge. Implementations may read a key file or use a hardware-backed
/// provider. Exosphere never persists keys inside packages.
pub trait NcaCryptoProvider {
    /// Produces the plaintext first 0xC00 bytes from a raw NCA header region.
    fn decrypt_header(&self, encrypted: &[u8]) -> Result<[u8; HEADER_AND_FS_HEADERS], IngestError>;
    /// Produces plaintext for one NCA section. `section_index` maps to the header section table.
    fn decrypt_section(
        &self,
        header: &NcaHeader,
        section_index: usize,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, IngestError>;
}

pub fn parse_plaintext_header(bytes: &[u8]) -> Result<NcaHeader, IngestError> {
    if bytes.len() < HEADER_AND_FS_HEADERS {
        return Err(IngestError::Invalid);
    };
    let r = Reader::new(bytes);
    let magic = r.array::<4>(0x200)?;
    if magic != *b"NCA0" && magic != *b"NCA1" && magic != *b"NCA2" && magic != *b"NCA3" {
        return Err(IngestError::Invalid);
    }
    let mut sections: [Option<SectionEntry>; 4] = [None, None, None, None];
    for (i, s) in sections.iter_mut().enumerate() {
        let o = 0x240 + i * 0x10;
        let a = r.u32(o)?;
        let b = r.u32(o + 4)?;
        if b > a {
            *s = Some(SectionEntry {
                media_start: a,
                media_end: b,
            });
        }
    }
    let fs_headers = std::array::from_fn(|i| {
        parse_fs_header(&bytes[0x400 + i * 0x200..0x600 + i * 0x200])
            .expect("validated 0xC00 header")
    });
    Ok(NcaHeader {
        magic,
        distribution_type: r.u8(0x204)?,
        content_type: r.u8(0x205)?,
        key_generation_old: r.u8(0x206)?,
        key_area_key_index: r.u8(0x207)?,
        content_size: r.u64(0x208)?,
        program_id: r.u64(0x210)?,
        content_index: r.u32(0x218)?,
        sdk_addon_version: r.u32(0x21c)?,
        key_generation: r.u8(0x220)?,
        rights_id: r.array(0x230)?,
        encrypted_key_area: r.array(0x300)?,
        sections,
        fs_headers,
    })
}
pub fn parse_with_provider(
    bytes: &[u8],
    provider: &dyn NcaCryptoProvider,
) -> Result<NcaHeader, IngestError> {
    if bytes.len() < HEADER_AND_FS_HEADERS {
        return Err(IngestError::Invalid);
    };
    let plain = provider.decrypt_header(&bytes[..HEADER_AND_FS_HEADERS])?;
    parse_plaintext_header(&plain)
}
fn parse_fs_header(bytes: &[u8]) -> Result<FsHeader, IngestError> {
    let r = Reader::new(bytes);
    let raw = r.array(0)?;
    Ok(FsHeader {
        version: r.u16(0)?,
        fs_type: match r.u8(2)? {
            0 => FsType::RomFs,
            1 => FsType::PartitionFs,
            x => FsType::Unknown(x),
        },
        hash_type: r.u8(3)?,
        encryption: match r.u8(4)? {
            0 => EncryptionType::Auto,
            1 => EncryptionType::None,
            2 => EncryptionType::AesXts,
            3 => EncryptionType::AesCtr,
            4 => EncryptionType::AesCtrEx,
            5 => EncryptionType::AesCtrSkipLayerHash,
            6 => EncryptionType::AesCtrExSkipLayerHash,
            x => EncryptionType::Unknown(x),
        },
        generation: r.u32(0x140)?,
        secure_value: r.u32(0x144)?,
        raw,
    })
}

/// Provider for already-decrypted NCA images, useful for tests and research tooling.
pub struct PlaintextProvider;
impl NcaCryptoProvider for PlaintextProvider {
    fn decrypt_header(&self, encrypted: &[u8]) -> Result<[u8; HEADER_AND_FS_HEADERS], IngestError> {
        encrypted
            .get(..HEADER_AND_FS_HEADERS)
            .ok_or(IngestError::Invalid)?
            .try_into()
            .map_err(|_| IngestError::Invalid)
    }
    fn decrypt_section(
        &self,
        _header: &NcaHeader,
        _section_index: usize,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, IngestError> {
        Ok(encrypted.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_minimal_plain_header() {
        let mut b = [0u8; HEADER_AND_FS_HEADERS];
        b[0x200..0x204].copy_from_slice(b"NCA3");
        b[0x208..0x210].copy_from_slice(&0x2000u64.to_le_bytes());
        for i in 0..4 {
            b[0x400 + i * 0x200..0x402 + i * 0x200].copy_from_slice(&2u16.to_le_bytes());
        }
        let h = parse_plaintext_header(&b).unwrap();
        assert_eq!(&h.magic, b"NCA3");
        assert_eq!(h.content_size, 0x2000);
    }
}
