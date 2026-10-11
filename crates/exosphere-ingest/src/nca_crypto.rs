// SPDX-License-Identifier: GPL-3.0-or-later
//! User-supplied NCA header decryption. Key material is never included in diagnostics.

use crate::{
    IngestError,
    nca::{HEADER_AND_FS_HEADERS, NcaCryptoProvider, NcaHeader},
};
use aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray},
};

/// Header-key provider. Deliberately does not implement Debug or serialization.
pub struct UserKeyProvider {
    header_key: [u8; 32],
    named_keys: std::collections::BTreeMap<String, [u8; 16]>,
    title_keys: std::collections::BTreeMap<[u8; 16], [u8; 16]>,
}

impl UserKeyProvider {
    pub fn from_key_file(path: impl AsRef<std::path::Path>) -> Result<Self, IngestError> {
        let file = std::fs::File::open(path)?;
        Self::from_reader(std::io::BufReader::new(file))
    }

    pub fn from_reader(reader: impl std::io::BufRead) -> Result<Self, IngestError> {
        let mut header_key = None;
        let mut header_key_source = None;
        let mut named_keys = std::collections::BTreeMap::new();
        for line in reader.lines() {
            let line = line?;
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                return Err(IngestError::Invalid);
            };
            let name = name.trim();
            if name == "header_key_source" {
                if header_key_source.is_some() {
                    return Err(IngestError::Invalid);
                }
                header_key_source = Some(decode_hex(value.trim())?);
                continue;
            }
            // These dump metadata entries are not the revision's usable KEK.
            // This provider has no source-to-KEK derivation, so treat them like
            // other unconsumed fields rather than forcing them into AES keys.
            if name == "master_kek_source" || name.starts_with("master_kek_source_") {
                continue;
            }
            if name.starts_with("titlekek_")
                || name.starts_with("key_area_key_")
                || name.starts_with("master_key_")
                || name.starts_with("master_kek_")
                || matches!(
                    name,
                    "aes_kek_generation_source" | "aes_key_generation_source" | "header_kek_source"
                )
            {
                if named_keys
                    .insert(name.to_owned(), decode_hex(value.trim())?)
                    .is_some()
                {
                    return Err(IngestError::Invalid);
                }
                continue;
            }
            if name != "header_key" {
                continue;
            }
            if header_key.is_some() {
                return Err(IngestError::Invalid);
            }
            header_key = Some(decode_hex(value.trim())?);
        }
        derive_nca_keys(&mut named_keys)?;
        let derived_header = derive_header_key(&named_keys, header_key_source);
        if let (Some(supplied), Some(derived)) = (header_key, derived_header)
            && supplied != derived
        {
            return Err(IngestError::Integrity(
                "conflicting supplied and derived NCA header keys",
            ));
        }
        Ok(Self {
            header_key: header_key
                .or(derived_header)
                .ok_or(IngestError::MissingKey)?,
            named_keys,
            title_keys: std::collections::BTreeMap::new(),
        })
    }

    /// Imports a common ticket using the matching title KEK. Personalized RSA tickets
    /// require a separate console-owned provider and are rejected here.
    pub fn import_ticket(&mut self, bytes: &[u8]) -> Result<(), IngestError> {
        let reader = crate::reader::Reader::new(bytes);
        let signature = reader.u32(0)?;
        let body = match signature {
            0x10000 | 0x10003 => 0x240,
            0x10001 | 0x10004 => 0x140,
            0x10002 | 0x10005 => 0x80,
            0x10006 => 0x40,
            _ => return Err(IngestError::Unsupported),
        };
        reader.slice(body, 0x180)?;
        if reader.u8(body + 0x140)? != 2 {
            return Err(IngestError::Unsupported);
        }
        if reader.u8(body + 0x141)? != 0 {
            return Err(IngestError::Unsupported);
        }
        // Switch common-ticket key generation is at body+0x146 (absolute 0x286
        // for RSA-2048 tickets), unlike the preceding reserved byte.
        let revision = reader.u8(body + 0x146)?.saturating_sub(1);
        let key = self
            .named_keys
            .get(&format!("titlekek_{revision:02x}"))
            .ok_or(IngestError::MissingKey)?;
        let cipher = Aes128::new_from_slice(key).map_err(|_| IngestError::Invalid)?;
        let mut title_key = GenericArray::from(reader.array::<16>(body + 0x40)?);
        cipher.decrypt_block(&mut title_key);
        let rights_id = reader.array::<16>(body + 0x160)?;
        let title_key: [u8; 16] = title_key.into();
        if let Some(previous) = self.title_keys.get(&rights_id) {
            if previous != &title_key {
                return Err(IngestError::Integrity("conflicting ticket title keys"));
            }
        }
        self.title_keys.insert(rights_id, title_key);
        Ok(())
    }

    /// Decrypts any bounded range; offsets are relative to this NCA section.
    pub fn decrypt_range(
        &self,
        header: &NcaHeader,
        section_index: usize,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<(), IngestError> {
        use crate::nca::EncryptionType;
        let section = header
            .sections
            .get(section_index)
            .and_then(|section| *section)
            .ok_or(IngestError::Invalid)?;
        if offset
            .checked_add(bytes.len() as u64)
            .ok_or(IngestError::Invalid)?
            > section.byte_size()
        {
            return Err(IngestError::Invalid);
        }
        let fs = &header.fs_headers[section_index];
        if fs.encryption == EncryptionType::None {
            return Ok(());
        }
        if fs.encryption != EncryptionType::AesCtr {
            return Err(IngestError::Unsupported);
        }
        let key = self.content_key(header)?;
        let cipher = Aes128::new_from_slice(&key).map_err(|_| IngestError::Invalid)?;
        let absolute = section
            .byte_offset()
            .checked_add(offset)
            .ok_or(IngestError::Invalid)?;
        // CTR uses a reversed filesystem nonce followed by the NCA-relative
        // block address, not the NSP offset or the section-relative address.
        let mut position = 0;
        while position < bytes.len() {
            let address = absolute
                .checked_add(position as u64)
                .ok_or(IngestError::Invalid)?;
            let mut counter = [0u8; 16];
            for (out, byte) in counter[..8]
                .iter_mut()
                .zip(fs.raw[0x140..0x148].iter().rev())
            {
                *out = *byte;
            }
            counter[8..].copy_from_slice(&(address / 16).to_be_bytes());
            let mut stream = GenericArray::from(counter);
            cipher.encrypt_block(&mut stream);
            let skip = (address % 16) as usize;
            let count = (16 - skip).min(bytes.len() - position);
            for i in 0..count {
                bytes[position + i] ^= stream[skip + i];
            }
            position += count;
        }
        Ok(())
    }

    fn content_key(&self, header: &NcaHeader) -> Result<[u8; 16], IngestError> {
        if header.rights_id != [0u8; 16] {
            Ok(*self
                .title_keys
                .get(&header.rights_id)
                .ok_or(IngestError::MissingKey)?)
        } else {
            let revision = header
                .key_generation
                .max(header.key_generation_old)
                .saturating_sub(1);
            let kind = match header.key_area_key_index {
                0 => "application",
                1 => "ocean",
                2 => "system",
                _ => return Err(IngestError::Invalid),
            };
            let kek = self
                .named_keys
                .get(&format!("key_area_key_{kind}_{revision:02x}"))
                .ok_or(IngestError::MissingKey)?;
            let cipher = Aes128::new_from_slice(kek).map_err(|_| IngestError::Invalid)?;
            let mut key = GenericArray::from(
                <[u8; 16]>::try_from(&header.encrypted_key_area[32..48]).unwrap(),
            );
            cipher.decrypt_block(&mut key);
            Ok(key.into())
        }
    }
}

fn decrypt_key_block(key: [u8; 16], source: [u8; 16]) -> [u8; 16] {
    let cipher = Aes128::new(GenericArray::from_slice(&key));
    let mut block = GenericArray::from(source);
    cipher.decrypt_block(&mut block);
    block.into()
}

fn derive_header_key(
    keys: &std::collections::BTreeMap<String, [u8; 16]>,
    source: Option<[u8; 32]>,
) -> Option<[u8; 32]> {
    let source = source?;
    // Header encryption always uses master revision 00, independently of the
    // archive's content-key generation. Both XTS key halves are wrapped by one KEK.
    let kek = decrypt_key_block(
        *keys.get("master_key_00")?,
        *keys.get("aes_kek_generation_source")?,
    );
    let source_kek = decrypt_key_block(kek, *keys.get("header_kek_source")?);
    let header_kek = decrypt_key_block(source_kek, *keys.get("aes_key_generation_source")?);
    let mut header_key = [0; 32];
    for (wrapped, output) in source.chunks_exact(16).zip(header_key.chunks_exact_mut(16)) {
        output.copy_from_slice(&decrypt_key_block(header_kek, wrapped.try_into().unwrap()));
    }
    Some(header_key)
}

fn retain_derived_key(
    keys: &mut std::collections::BTreeMap<String, [u8; 16]>,
    name: String,
    derived: [u8; 16],
) -> Result<(), IngestError> {
    if keys.get(&name).is_some_and(|supplied| *supplied != derived) {
        return Err(IngestError::Integrity(
            "conflicting supplied and derived NCA keys",
        ));
    }
    keys.insert(name, derived);
    Ok(())
}

fn derive_nca_keys(
    keys: &mut std::collections::BTreeMap<String, [u8; 16]>,
) -> Result<(), IngestError> {
    // Derive only from the exact user-supplied master/KEK revision. Older
    // revisions cannot manufacture newer keys; absent prerequisites stay absent.
    for revision in 0..=u8::MAX {
        // A dump may contain the exact revision's intermediate KEK instead of
        // its final master. Never substitute another revision or built-in key.
        if let (Some(kek), Some(source)) = (
            keys.get(&format!("master_kek_{revision:02x}")).copied(),
            keys.get("master_key_source").copied(),
        ) {
            retain_derived_key(
                keys,
                format!("master_key_{revision:02x}"),
                decrypt_key_block(kek, source),
            )?;
        }
        let Some(master) = keys.get(&format!("master_key_{revision:02x}")).copied() else {
            continue;
        };
        if let Some(source) = keys.get("titlekek_source").copied() {
            retain_derived_key(
                keys,
                format!("titlekek_{revision:02x}"),
                decrypt_key_block(master, source),
            )?;
        }
        let (Some(kek_source), Some(key_source)) = (
            keys.get("aes_kek_generation_source").copied(),
            keys.get("aes_key_generation_source").copied(),
        ) else {
            continue;
        };
        let kek = decrypt_key_block(master, kek_source);
        for kind in ["application", "ocean", "system"] {
            if let Some(source) = keys.get(&format!("key_area_key_{kind}_source")).copied() {
                let source_kek = decrypt_key_block(kek, source);
                retain_derived_key(
                    keys,
                    format!("key_area_key_{kind}_{revision:02x}"),
                    decrypt_key_block(source_kek, key_source),
                )?;
            }
        }
    }
    Ok(())
}

fn decode_hex<const N: usize>(text: &str) -> Result<[u8; N], IngestError> {
    if text.len() != N * 2 {
        return Err(IngestError::Invalid);
    }
    let mut bytes = [0u8; N];
    for (pair, out) in text.as_bytes().chunks_exact(2).zip(&mut bytes) {
        let hi = (pair[0] as char).to_digit(16).ok_or(IngestError::Invalid)?;
        let lo = (pair[1] as char).to_digit(16).ok_or(IngestError::Invalid)?;
        *out = ((hi << 4) | lo) as u8;
    }
    Ok(bytes)
}

fn multiply_tweak(tweak: &mut [u8; 16]) {
    let mut carry = 0;
    for byte in tweak.iter_mut() {
        let next = *byte >> 7;
        *byte = (*byte << 1) | carry;
        carry = next;
    }
    if carry != 0 {
        tweak[0] ^= 0x87;
    }
}

fn decrypt_sector(data: &mut [u8], number: u128, data_cipher: &Aes128, tweak_cipher: &Aes128) {
    // Nintendo stores the sector index in big-endian order before AES tweak encryption.
    let mut tweak = GenericArray::from(number.to_be_bytes());
    tweak_cipher.encrypt_block(&mut tweak);
    let mut tweak: [u8; 16] = tweak.into();
    for bytes in data.chunks_exact_mut(16) {
        let mut block = GenericArray::from(<[u8; 16]>::try_from(&*bytes).unwrap());
        for (byte, mask) in block.iter_mut().zip(tweak) {
            *byte ^= mask;
        }
        data_cipher.decrypt_block(&mut block);
        for ((out, byte), mask) in bytes.iter_mut().zip(block).zip(tweak) {
            *out = byte ^ mask;
        }
        multiply_tweak(&mut tweak);
    }
}

impl NcaCryptoProvider for UserKeyProvider {
    fn decrypt_header(&self, encrypted: &[u8]) -> Result<[u8; HEADER_AND_FS_HEADERS], IngestError> {
        let mut plain: [u8; HEADER_AND_FS_HEADERS] = encrypted
            .get(..HEADER_AND_FS_HEADERS)
            .ok_or(IngestError::Invalid)?
            .try_into()
            .map_err(|_| IngestError::Invalid)?;
        let data_cipher =
            Aes128::new_from_slice(&self.header_key[..16]).map_err(|_| IngestError::Invalid)?;
        let tweak_cipher =
            Aes128::new_from_slice(&self.header_key[16..]).map_err(|_| IngestError::Invalid)?;
        for (sector, bytes) in plain.chunks_exact_mut(0x200).enumerate() {
            decrypt_sector(bytes, sector as u128, &data_cipher, &tweak_cipher);
        }
        match &plain[0x200..0x204] {
            b"NCA3" => {}
            b"NCA2" => {
                for (encrypted, bytes) in encrypted[0x400..0xc00]
                    .chunks_exact(0x200)
                    .zip(plain[0x400..].chunks_exact_mut(0x200))
                {
                    bytes.copy_from_slice(encrypted);
                    decrypt_sector(bytes, 0, &data_cipher, &tweak_cipher);
                }
            }
            _ => return Err(IngestError::Integrity("NCA header magic/key mismatch")),
        }
        use sha2::{Digest, Sha256};
        // Check filesystem header integrity. RSA signature authentication is a separate gate.
        for section in 0..4 {
            let table = 0x240 + section * 0x10;
            let start = u32::from_le_bytes(plain[table..table + 4].try_into().unwrap());
            let end = u32::from_le_bytes(plain[table + 4..table + 8].try_into().unwrap());
            if end < start {
                return Err(IngestError::Invalid);
            }
            if start == end {
                continue;
            }
            let fs_offset = 0x400 + section * 0x200;
            let expected = &plain[0x280 + section * 32..0x2a0 + section * 32];
            let actual = Sha256::digest(&plain[fs_offset..fs_offset + 0x200]);
            if actual.as_slice() != expected {
                return Err(IngestError::Integrity(
                    "NCA filesystem header hash mismatch",
                ));
            }
        }
        Ok(plain)
    }

    fn decrypt_section(
        &self,
        header: &NcaHeader,
        section_index: usize,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, IngestError> {
        let section = header
            .sections
            .get(section_index)
            .and_then(|section| *section)
            .ok_or(IngestError::Invalid)?;
        if encrypted.len() as u64 != section.byte_size() {
            return Err(IngestError::Invalid);
        }
        let mut plain = encrypted.to_vec();
        self.decrypt_range(header, section_index, 0, &mut plain)?;
        Ok(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encrypt(key: [u8; 16], plain: [u8; 16]) -> [u8; 16] {
        let mut block = GenericArray::from(plain);
        Aes128::new(GenericArray::from_slice(&key)).encrypt_block(&mut block);
        block.into()
    }

    #[test]
    fn derives_header_xts_halves_and_requires_master_revision_zero() {
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let (master, kek, source_kek, header_kek) = ([1; 16], [2; 16], [3; 16], [4; 16]);
        let mut expected = [5; 32];
        expected[16..].fill(6);
        let mut wrapped = encrypt(header_kek, [5; 16]).to_vec();
        wrapped.extend(encrypt(header_kek, [6; 16]));
        let text = format!(
            "master_key_00 = {}\nmaster_key_01 = {}\naes_kek_generation_source = {}\naes_key_generation_source = {}\nheader_kek_source = {}\nheader_key_source = {}\n",
            hex(&master),
            hex(&master),
            hex(&encrypt(master, kek)),
            hex(&encrypt(source_kek, header_kek)),
            hex(&encrypt(kek, source_kek)),
            hex(&wrapped)
        );
        let parse = |text: &str| UserKeyProvider::from_reader(std::io::Cursor::new(text));
        assert_eq!(parse(&text).unwrap().header_key, expected);
        assert!(parse(&format!("{text}header_key = {}\n", hex(&expected))).is_ok());
        assert!(matches!(
            parse(&format!("{text}header_key = {}\n", "00".repeat(32))),
            Err(IngestError::Integrity(_))
        ));
        for prerequisite in [
            "master_key_00",
            "aes_kek_generation_source",
            "aes_key_generation_source",
            "header_kek_source",
            "header_key_source",
        ] {
            let incomplete = text
                .lines()
                .filter(|line| !line.starts_with(prerequisite))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(matches!(parse(&incomplete), Err(IngestError::MissingKey)));
            assert!(parse(&format!("{incomplete}\nheader_key = {}\n", hex(&expected))).is_ok());
        }
        assert!(matches!(
            parse(&format!("{text}header_key_source = {}\n", hex(&wrapped))),
            Err(IngestError::Invalid)
        ));
    }

    #[test]
    fn master_kek_source_metadata_is_not_a_direct_kek() {
        let header = format!("header_key = {}\n", "00".repeat(32));
        let text = format!(
            "{header}master_kek_source_15 = {}\nmaster_kek_source = unused metadata\nmaster_kek_15 = {}\n",
            "ab".repeat(17),
            "cd".repeat(16),
        );
        let parse = |text: &str| UserKeyProvider::from_reader(std::io::Cursor::new(text));
        let provider = parse(&text).unwrap();
        assert_eq!(provider.named_keys["master_kek_15"], [0xcd; 16]);
        assert!(!provider.named_keys.contains_key("master_kek_source_15"));
        assert!(!provider.named_keys.contains_key("master_kek_source"));
        assert!(!provider.named_keys.contains_key("master_key_15"));
        // Ignoring unused metadata must not relax actual KEK width or duplicate checks.
        assert!(matches!(
            parse(&format!("{header}master_kek_15 = {}\n", "ab".repeat(17))),
            Err(IngestError::Invalid)
        ));
        assert!(matches!(
            parse(&format!("{text}master_kek_15 = {}\n", "cd".repeat(16))),
            Err(IngestError::Invalid)
        ));
    }

    #[test]
    fn derives_master_from_exact_user_kek_and_rejects_conflicts() {
        let hex = |key: &[u8]| {
            key.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let (kek, master, title) = ([17; 16], [34; 16], [51; 16]);
        let header = format!("header_key = {}\n", "00".repeat(32));
        let wrapped = encrypt(kek, master);
        let text = format!(
            "{header}master_kek_14 = {}\nmaster_key_source = {}\ntitlekek_source = {}\n",
            hex(&kek),
            hex(&wrapped),
            hex(&encrypt(master, title))
        );
        let parse = |text: &str| UserKeyProvider::from_reader(std::io::Cursor::new(text));
        let provider = parse(&text).unwrap();
        assert_eq!(provider.named_keys["master_key_14"], master);
        assert_eq!(provider.named_keys["titlekek_14"], title);
        assert!(!provider.named_keys.contains_key("master_key_15"));
        assert!(parse(&format!("{text}master_key_14 = {}\n", hex(&master))).is_ok());
        assert!(matches!(
            parse(&format!("{text}master_key_14 = {}\n", hex(&[99; 16]))),
            Err(IngestError::Integrity(_))
        ));
        let without_source = format!("{header}master_kek_14 = {}\n", hex(&kek));
        assert!(
            !parse(&without_source)
                .unwrap()
                .named_keys
                .contains_key("master_key_14")
        );
        let newer = format!("{text}master_kek_15 = {}\n", hex(&kek));
        assert_eq!(parse(&newer).unwrap().named_keys["master_key_15"], master);
        assert!(matches!(
            parse(&format!("{text}master_kek_14 = {}\n", hex(&kek))),
            Err(IngestError::Invalid)
        ));
        assert!(matches!(
            parse(&format!("{header}master_kek_15 = 00\n")),
            Err(IngestError::Invalid)
        ));
    }

    #[test]
    fn derives_exact_master_revision_without_inventing_missing_keys() {
        fn parse(
            keys: &std::collections::BTreeMap<String, [u8; 16]>,
        ) -> Result<UserKeyProvider, IngestError> {
            let mut text = format!("header_key = {}\n", "00".repeat(32));
            for (name, key) in keys {
                let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
                text.push_str(&format!("{name} = {hex}\n"));
            }
            UserKeyProvider::from_reader(std::io::Cursor::new(text))
        }
        // Synthetic wrapping fixtures use encryption as an independent inverse
        // oracle for each derivation step; no console keys are test artifacts.
        let (master, kek, source_kek, expected, title) =
            ([1; 16], [2; 16], [3; 16], [4; 16], [5; 16]);
        let mut keys = std::collections::BTreeMap::from([
            ("master_key_14".into(), master),
            ("aes_kek_generation_source".into(), encrypt(master, kek)),
            (
                "aes_key_generation_source".into(),
                encrypt(source_kek, expected),
            ),
            ("titlekek_source".into(), encrypt(master, title)),
        ]);
        for kind in ["application", "ocean", "system"] {
            keys.insert(
                format!("key_area_key_{kind}_source"),
                encrypt(kek, source_kek),
            );
        }
        let provider = parse(&keys).unwrap();
        for kind in ["application", "ocean", "system"] {
            assert_eq!(
                provider.named_keys[&format!("key_area_key_{kind}_14")],
                expected
            );
            assert!(
                !provider
                    .named_keys
                    .contains_key(&format!("key_area_key_{kind}_15"))
            );
        }
        assert_eq!(provider.named_keys["titlekek_14"], title);
        assert!(!provider.named_keys.contains_key("titlekek_15"));
        keys.insert("key_area_key_application_14".into(), expected);
        assert!(parse(&keys).is_ok());
        keys.insert("key_area_key_application_14".into(), [9; 16]);
        assert!(matches!(parse(&keys), Err(IngestError::Integrity(_))));
        keys.remove("key_area_key_application_14");
        keys.remove("aes_key_generation_source");
        let incomplete = parse(&keys).unwrap();
        assert!(
            !incomplete
                .named_keys
                .contains_key("key_area_key_application_14")
        );
        assert_eq!(incomplete.named_keys["titlekek_14"], title);
    }

    #[test]
    fn common_ticket_uses_its_generation_and_rejects_personalized_keys() {
        let mut provider = UserKeyProvider {
            header_key: [0u8; 32],
            named_keys: Default::default(),
            title_keys: Default::default(),
        };
        let kek = [7u8; 16];
        provider.named_keys.insert("titlekek_02".into(), kek);
        let title_key = [9u8; 16];
        let rights_id = [11u8; 16];
        let mut encrypted = GenericArray::from(title_key);
        Aes128::new_from_slice(&kek)
            .unwrap()
            .encrypt_block(&mut encrypted);
        let mut ticket = vec![0u8; 0x2c0];
        ticket[..4].copy_from_slice(&0x10004u32.to_le_bytes());
        ticket[0x180..0x190].copy_from_slice(&encrypted);
        ticket[0x280] = 2;
        ticket[0x286] = 3;
        ticket[0x2a0..0x2b0].copy_from_slice(&rights_id);
        provider.import_ticket(&ticket).unwrap();
        assert_eq!(provider.title_keys.get(&rights_id), Some(&title_key));
        ticket[0x281] = 1;
        assert!(matches!(
            provider.import_ticket(&ticket),
            Err(IngestError::Unsupported)
        ));
        assert!(provider.import_ticket(&ticket[..0x180]).is_err());
    }

    #[test]
    fn xts_matches_independent_openssl_vector() {
        // Generated with Python cryptography's OpenSSL-backed AES-XTS implementation.
        // Key bytes 0..31, plaintext bytes 0..31, tweak = 5u128.to_be_bytes().
        let mut encrypted = [
            0xbb, 0x62, 0x82, 0x96, 0x6b, 0xcd, 0xe7, 0x0d, 0xf1, 0xf6, 0x03, 0xe5, 0x0a, 0x5a,
            0x4e, 0xba, 0xab, 0x10, 0x89, 0xa0, 0x6d, 0x67, 0x5e, 0x03, 0x26, 0x3b, 0x36, 0x78,
            0x3d, 0x57, 0x2c, 0x72,
        ];
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let data_cipher = Aes128::new_from_slice(&key[..16]).unwrap();
        let tweak_cipher = Aes128::new_from_slice(&key[16..]).unwrap();
        decrypt_sector(&mut encrypted, 5, &data_cipher, &tweak_cipher);
        assert_eq!(encrypted, std::array::from_fn(|i| i as u8));
    }

    #[test]
    fn header_rejects_wrong_key_and_truncation() {
        let provider = UserKeyProvider {
            header_key: [0u8; 32],
            named_keys: Default::default(),
            title_keys: Default::default(),
        };
        assert!(matches!(
            provider.decrypt_header(&[0u8; 32]),
            Err(IngestError::Invalid)
        ));
        assert!(matches!(
            provider.decrypt_header(&[0u8; HEADER_AND_FS_HEADERS]),
            Err(IngestError::Integrity(_))
        ));
    }

    #[test]
    fn rejects_missing_duplicate_and_malformed_header_keys() {
        assert!(matches!(
            UserKeyProvider::from_reader(&b"other = 00\n"[..]),
            Err(IngestError::MissingKey)
        ));
        assert!(UserKeyProvider::from_reader(&b"header_key = secret\n"[..]).is_err());
        let key = "00".repeat(32);
        let text = format!("header_key = {key}\nheader_key = {key}\n");
        assert!(UserKeyProvider::from_reader(text.as_bytes()).is_err());
    }

    #[test]
    fn tweak_carries_and_reduces() {
        let mut tweak = [0u8; 16];
        tweak[15] = 0x80;
        multiply_tweak(&mut tweak);
        assert_eq!(tweak[0], 0x87);
        assert!(tweak[1..].iter().all(|byte| *byte == 0));
    }
}
