// SPDX-License-Identifier: GPL-3.0-or-later
//! Reads and verifies an NCA PartitionFS section before exposing its files.

use crate::{
    IngestError,
    nca::{FsType, NcaHeader},
    nca_crypto::UserKeyProvider,
    reader::Reader,
};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};

/// Owns verified plaintext PartitionFS bytes. Offsets returned by `index` are relative
/// to `bytes`, so callers never confuse NCA, section and filesystem offsets.
pub struct VerifiedPartition {
    pub bytes: Vec<u8>,
    pub index: crate::pfs0::Pfs0,
}

pub fn read_partition(
    input: &mut (impl Read + Seek),
    nca_offset: u64,
    header: &NcaHeader,
    section_index: usize,
    provider: &UserKeyProvider,
    max_section_bytes: u64,
) -> Result<VerifiedPartition, IngestError> {
    let section = header
        .sections
        .get(section_index)
        .and_then(|section| *section)
        .ok_or(IngestError::Invalid)?;
    let fs = &header.fs_headers[section_index];
    if fs.fs_type != FsType::PartitionFs || fs.hash_type != 2 {
        return Err(IngestError::Unsupported);
    }
    if section.byte_size() > max_section_bytes {
        return Err(IngestError::Unsupported);
    }
    let size = usize::try_from(section.byte_size()).map_err(|_| IngestError::Invalid)?;
    let mut plaintext = Vec::new();
    plaintext
        .try_reserve_exact(size)
        .map_err(|_| IngestError::Invalid)?;
    plaintext.resize(size, 0);
    input.seek(SeekFrom::Start(
        nca_offset
            .checked_add(section.byte_offset())
            .ok_or(IngestError::Invalid)?,
    ))?;
    input.read_exact(&mut plaintext)?;
    provider.decrypt_range(header, section_index, 0, &mut plaintext)?;
    verify_partition(&plaintext, &fs.raw)
}

fn verify_partition(
    section: &[u8],
    fs_header: &[u8; 0x200],
) -> Result<VerifiedPartition, IngestError> {
    let metadata = Reader::new(fs_header);
    let block_size = metadata.u32(0x28)? as usize;
    let layer_count = metadata.u32(0x2c)?;
    if layer_count != 2 || block_size == 0 || !block_size.is_power_of_two() {
        return Err(IngestError::Unsupported);
    }
    let reader = Reader::new(section);
    let region = |offset| -> Result<&[u8], IngestError> {
        let start = usize::try_from(metadata.u64(offset)?).map_err(|_| IngestError::Invalid)?;
        let size = usize::try_from(metadata.u64(offset + 8)?).map_err(|_| IngestError::Invalid)?;
        reader.slice(start, size)
    };
    let hashes = region(0x30)?;
    let filesystem = region(0x40)?;
    if Sha256::digest(hashes).as_slice() != metadata.slice(8, 32)? {
        return Err(IngestError::Integrity("PartitionFS master hash mismatch"));
    }
    let blocks = filesystem.len().div_ceil(block_size);
    if hashes.len() != blocks.checked_mul(32).ok_or(IngestError::Invalid)? {
        return Err(IngestError::Invalid);
    }
    for (data, expected) in filesystem.chunks(block_size).zip(hashes.chunks_exact(32)) {
        if Sha256::digest(data).as_slice() != expected {
            return Err(IngestError::Integrity("PartitionFS block hash mismatch"));
        }
    }
    let index = crate::pfs0::parse(filesystem)?;
    Ok(VerifiedPartition {
        bytes: filesystem.to_vec(),
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_partition() -> (Vec<u8>, [u8; 0x200]) {
        // One hash-table block followed by a valid empty PFS0 filesystem.
        let mut section = vec![0u8; 48];
        section[32..36].copy_from_slice(b"PFS0");
        let hash = Sha256::digest(&section[32..]);
        section[..32].copy_from_slice(&hash);
        let mut header = [0u8; 0x200];
        header[8..40].copy_from_slice(&Sha256::digest(&section[..32]));
        header[0x28..0x2c].copy_from_slice(&16u32.to_le_bytes());
        header[0x2c..0x30].copy_from_slice(&2u32.to_le_bytes());
        header[0x38..0x40].copy_from_slice(&32u64.to_le_bytes());
        header[0x40..0x48].copy_from_slice(&32u64.to_le_bytes());
        header[0x48..0x50].copy_from_slice(&16u64.to_le_bytes());
        (section, header)
    }

    #[test]
    fn verifies_filesystem_and_rejects_corrupted_data() {
        let (mut section, header) = empty_partition();
        assert!(
            verify_partition(&section, &header)
                .unwrap()
                .index
                .entries
                .is_empty()
        );
        section[40] ^= 1;
        assert!(matches!(
            verify_partition(&section, &header),
            Err(IngestError::Integrity("PartitionFS block hash mismatch"))
        ));
    }

    #[test]
    fn rejects_corrupted_hash_table_and_out_of_bounds_regions() {
        let (mut section, mut header) = empty_partition();
        section[0] ^= 1;
        assert!(matches!(
            verify_partition(&section, &header),
            Err(IngestError::Integrity("PartitionFS master hash mismatch"))
        ));
        header[0x40..0x48].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(verify_partition(&section, &header).is_err());
    }
}
