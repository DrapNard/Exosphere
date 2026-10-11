// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only NCA RomFS storage with bounded decryption and on-demand IVFC verification.

use crate::{
    IngestError,
    nca::{EncryptionType, FsType, NcaHeader},
    nca_crypto::UserKeyProvider,
    reader::Reader,
};
use exosphere_vfs::{FsError, FsResult, VirtualStorage};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek, SeekFrom},
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy)]
struct Level {
    offset: u64,
    size: u64,
    block_size: u64,
}

/// The source is serialized, but the entire multi-gigabyte title image is never loaded.
/// Key material and decrypted NCA headers intentionally have no Debug representation.
pub struct NcaRomStorage<R> {
    input: Mutex<R>,
    provider: Arc<UserKeyProvider>,
    header: NcaHeader,
    nca_offset: u64,
    section_index: usize,
    levels: Vec<Level>,
    master_hash: [u8; 32],
}

impl<R: Read + Seek + Send> NcaRomStorage<R> {
    pub fn new(
        mut input: R,
        nca_offset: u64,
        header: NcaHeader,
        section_index: usize,
        provider: Arc<UserKeyProvider>,
    ) -> Result<Self, IngestError> {
        let section = header
            .sections
            .get(section_index)
            .and_then(|s| *s)
            .ok_or(IngestError::Invalid)?;
        let fs = &header.fs_headers[section_index];
        if fs.fs_type != FsType::RomFs
            || fs.hash_type != 3
            || !matches!(fs.encryption, EncryptionType::None | EncryptionType::AesCtr)
            || fs.raw[5] != 0
            || fs.raw[0x100..0x140].iter().any(|b| *b != 0)
            || fs.raw[0x148..0x1a0].iter().any(|b| *b != 0)
        {
            return Err(IngestError::Unsupported);
        }
        let reader = Reader::new(&fs.raw);
        if reader.slice(8, 4)? != b"IVFC" || reader.u32(0xc)? != 0x20000 || reader.u32(0x10)? != 32
        {
            return Err(IngestError::Unsupported);
        }
        // max_layers includes the master hash, which is stored in the FS header.
        let count = reader.u32(0x14)?;
        if !(2..=7).contains(&count) {
            return Err(IngestError::Invalid);
        }
        let mut levels: Vec<Level> = Vec::new();
        for i in 0..(count - 1) as usize {
            let at = 0x18 + i * 24;
            let order = reader.u32(at + 16)?;
            // Bound per-block memory and ensure a hash never straddles parent blocks.
            if !(5..=20).contains(&order) {
                return Err(IngestError::Unsupported);
            }
            let level = Level {
                offset: reader.u64(at)?,
                size: reader.u64(at + 8)?,
                block_size: 1u64 << order,
            };
            let end = level
                .offset
                .checked_add(level.size)
                .ok_or(IngestError::Invalid)?;
            if level.size == 0
                || end > section.byte_size()
                || levels.iter().any(|previous| {
                    level.offset < previous.offset + previous.size && previous.offset < end
                })
            {
                return Err(IngestError::Invalid);
            }
            let required_hashes = level
                .size
                .div_ceil(level.block_size)
                .checked_mul(32)
                .ok_or(IngestError::Invalid)?;
            let available = levels.last().map_or(32, |previous| previous.size);
            if required_hashes > available {
                return Err(IngestError::Invalid);
            }
            levels.push(level);
        }
        let section_end = section
            .byte_offset()
            .checked_add(section.byte_size())
            .ok_or(IngestError::Invalid)?;
        let archive_end = nca_offset
            .checked_add(header.content_size)
            .ok_or(IngestError::Invalid)?;
        if section_end > header.content_size || archive_end > input.seek(SeekFrom::End(0))? {
            return Err(IngestError::Invalid);
        }
        let master_hash = reader.array::<32>(0xc8)?;
        let storage = Self {
            input: Mutex::new(input),
            provider,
            header,
            nca_offset,
            section_index,
            levels,
            master_hash,
        };
        // Check the RomFS header through the integrity tree before publishing storage.
        let mut rom_header = [0u8; 0x50];
        storage
            .read_at(0, &mut rom_header)
            .map_err(|_| IngestError::Integrity("RomFS header verification failed"))?;
        let rom = Reader::new(&rom_header);
        if rom.u64(0)? != 0x50 {
            return Err(IngestError::Invalid);
        }
        let size = storage.levels.last().unwrap().size;
        for at in [8, 24, 40, 56] {
            if rom
                .u64(at)?
                .checked_add(rom.u64(at + 8)?)
                .ok_or(IngestError::Invalid)?
                > size
            {
                return Err(IngestError::Invalid);
            }
        }
        if rom.u64(72)? > size {
            return Err(IngestError::Invalid);
        }
        Ok(storage)
    }

    fn read_verified(
        &self,
        input: &mut R,
        index: usize,
        offset: u64,
        output: &mut [u8],
    ) -> FsResult<()> {
        let level = self.levels[index];
        if offset
            .checked_add(output.len() as u64)
            .is_none_or(|end| end > level.size)
        {
            return Err(FsError::InvalidRange);
        }
        let mut done = 0;
        while done < output.len() {
            let position = offset + done as u64;
            let number = position / level.block_size;
            let start = number * level.block_size;
            let valid = (level.size - start).min(level.block_size) as usize;
            // IVFC hashes a full block: the final partial block is zero padded,
            // not filled with bytes from the next physical NCA region.
            let mut block = vec![0u8; level.block_size as usize];
            let relative = level.offset + start;
            let physical = self.nca_offset
                + self.header.sections[self.section_index]
                    .unwrap()
                    .byte_offset()
                + relative;
            input.seek(SeekFrom::Start(physical)).map_err(io_error)?;
            input.read_exact(&mut block[..valid]).map_err(io_error)?;
            self.provider
                .decrypt_range(
                    &self.header,
                    self.section_index,
                    relative,
                    &mut block[..valid],
                )
                .map_err(|e| FsError::Backend(e.to_string()))?;
            let mut expected = self.master_hash;
            if index != 0 {
                self.read_verified(input, index - 1, number * 32, &mut expected)?;
            }
            if Sha256::digest(&block).as_slice() != expected {
                return Err(FsError::Backend("RomFS IVFC block hash mismatch".into()));
            }
            let skip = (position - start) as usize;
            let count = (valid - skip).min(output.len() - done);
            output[done..done + count].copy_from_slice(&block[skip..skip + count]);
            done += count;
        }
        Ok(())
    }
}

impl<R: Read + Seek + Send> VirtualStorage for NcaRomStorage<R> {
    fn size(&self) -> FsResult<u64> {
        Ok(self.levels.last().unwrap().size)
    }
    fn read_at(&self, offset: u64, output: &mut [u8]) -> FsResult<()> {
        if output.len() > 64 * 1024 * 1024
            || offset
                .checked_add(output.len() as u64)
                .is_none_or(|end| end > self.levels.last().unwrap().size)
        {
            return Err(FsError::InvalidRange);
        }
        let mut input = self
            .input
            .lock()
            .map_err(|_| FsError::Backend("RomFS source lock poisoned".into()))?;
        // Integrity failures must not expose a successfully read prefix to the caller.
        let mut verified = Vec::new();
        verified
            .try_reserve_exact(output.len())
            .map_err(|_| FsError::Backend("RomFS read allocation failed".into()))?;
        verified.resize(output.len(), 0);
        self.read_verified(&mut input, self.levels.len() - 1, offset, &mut verified)?;
        output.copy_from_slice(&verified);
        Ok(())
    }
    fn write_at(&self, _offset: u64, _input: &[u8]) -> FsResult<()> {
        Err(FsError::ReadOnly)
    }
    fn set_size(&self, _size: u64) -> FsResult<()> {
        Err(FsError::ReadOnly)
    }
    fn flush(&self) -> FsResult<()> {
        Ok(())
    }
}

fn io_error(error: std::io::Error) -> FsError {
    FsError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const PREFIX: usize = 13;
    const SECTION: usize = 0xc00;
    const DATA: usize = 5 * 128;

    fn fixture(encrypted: bool) -> (Cursor<Vec<u8>>, NcaHeader, Arc<UserKeyProvider>, Vec<u8>) {
        let keys = format!(
            "header_key = {}\nkey_area_key_application_00 = {}\n",
            "00".repeat(32),
            "00".repeat(16)
        );
        let provider = Arc::new(UserKeyProvider::from_reader(Cursor::new(keys)).unwrap());
        let mut bytes = vec![0u8; PREFIX + SECTION + 1024];
        let mut raw = [0u8; crate::nca::HEADER_AND_FS_HEADERS];
        raw[0x200..0x204].copy_from_slice(b"NCA3");
        raw[0x208..0x210].copy_from_slice(&((SECTION + 1024) as u64).to_le_bytes());
        raw[0x240..0x244].copy_from_slice(&6u32.to_le_bytes());
        raw[0x244..0x248].copy_from_slice(&8u32.to_le_bytes());
        let fs = &mut raw[0x400..0x600];
        fs[3] = 3;
        fs[4] = if encrypted { 3 } else { 1 };
        fs[8..12].copy_from_slice(b"IVFC");
        fs[12..16].copy_from_slice(&0x20000u32.to_le_bytes());
        fs[16..20].copy_from_slice(&32u32.to_le_bytes());
        fs[20..24].copy_from_slice(&7u32.to_le_bytes());
        let sizes = [32usize, 32, 32, 32, 96, 300];
        for (i, size) in sizes.iter().enumerate() {
            let at = 24 + i * 24;
            fs[at..at + 8].copy_from_slice(&((i * 128) as u64).to_le_bytes());
            fs[at + 8..at + 16].copy_from_slice(&(*size as u64).to_le_bytes());
            fs[at + 16..at + 20].copy_from_slice(&7u32.to_le_bytes());
        }
        let mut plaintext = vec![0u8; 300];
        plaintext[..8].copy_from_slice(&0x50u64.to_le_bytes());
        plaintext[72..80].copy_from_slice(&0x50u64.to_le_bytes());
        for (i, byte) in plaintext[80..].iter_mut().enumerate() {
            *byte = i as u8;
        }
        let section = &mut bytes[PREFIX + SECTION..];
        section[DATA..DATA + plaintext.len()].copy_from_slice(&plaintext);
        for i in (0..6).rev() {
            for block in 0..sizes[i].div_ceil(128) {
                let mut padded = [0u8; 128];
                let start = block * 128;
                let size = (sizes[i] - start).min(128);
                padded[..size].copy_from_slice(&section[i * 128 + start..i * 128 + start + size]);
                let hash = Sha256::digest(padded);
                if i == 0 {
                    fs[0xc8..0xe8].copy_from_slice(&hash);
                } else {
                    section[(i - 1) * 128 + block * 32..(i - 1) * 128 + block * 32 + 32]
                        .copy_from_slice(&hash);
                }
            }
        }
        let header = crate::nca::parse_plaintext_header(&raw).unwrap();
        if encrypted {
            // AES-CTR is symmetric; the independent crypto-vector tests live in nca_crypto.
            provider.decrypt_range(&header, 0, 0, section).unwrap();
        }
        (Cursor::new(bytes), header, provider, plaintext)
    }

    #[test]
    fn reads_only_romfs_data_with_full_integrity_chain_and_ctr_ranges() {
        for encrypted in [false, true] {
            let (input, header, provider, plaintext) = fixture(encrypted);
            let storage = NcaRomStorage::new(input, PREFIX as u64, header, 0, provider).unwrap();
            assert_eq!(storage.size().unwrap(), 300);
            let mut output = [0u8; 175];
            storage.read_at(125, &mut output).unwrap();
            assert_eq!(output, plaintext[125..]);
            storage.read_at(300, &mut []).unwrap();
            assert!(matches!(storage.write_at(0, &[1]), Err(FsError::ReadOnly)));
            assert!(matches!(storage.set_size(0), Err(FsError::ReadOnly)));
            storage.flush().unwrap();
            assert!(storage.read_at(u64::MAX, &mut [0]).is_err());
            assert!(storage.read_at(301, &mut []).is_err());
        }
    }

    #[test]
    fn corruption_at_every_hash_level_and_truncated_sources_are_rejected() {
        for level in 0..6 {
            let (mut input, header, provider, _) = fixture(false);
            input.get_mut()[PREFIX + SECTION + level * 128] ^= 1;
            assert!(NcaRomStorage::new(input, PREFIX as u64, header, 0, provider).is_err());
        }
        let (mut input, header, provider, _) = fixture(false);
        input.get_mut().pop();
        assert!(NcaRomStorage::new(input, PREFIX as u64, header, 0, provider).is_err());
    }

    #[test]
    fn late_corruption_and_invalid_ranges_leave_output_unchanged() {
        let (input, header, provider, _) = fixture(true);
        let mut storage = NcaRomStorage::new(input, PREFIX as u64, header, 0, provider).unwrap();
        storage.input.get_mut().unwrap().get_mut()[PREFIX + SECTION + DATA + 280] ^= 1;
        let mut output = [0xa5u8; 175];
        assert!(storage.read_at(125, &mut output).is_err());
        assert_eq!(output, [0xa5; 175]);
        assert!(storage.read_at(200, &mut output).is_err());
        assert_eq!(output, [0xa5; 175]);
    }

    #[test]
    fn malformed_layout_and_unsupported_profiles_fail_before_mounting() {
        for at in [0x14, 0x28, 0x30, 0x40, 0x100, 0x148] {
            let (input, mut header, provider, _) = fixture(false);
            // Invalid layer count, enormous block order, overlapping/out-of-range
            // levels and patch/sparse metadata must never become raw storage.
            header.fs_headers[0].raw[at] = 0xff;
            assert!(
                NcaRomStorage::new(input, PREFIX as u64, header, 0, provider).is_err(),
                "offset {at:x}"
            );
        }
    }
}
