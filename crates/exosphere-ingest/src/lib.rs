// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Exosphere input normalization boundary for Switch executable/container/filesystem formats.
//!
//! Decryption keys and proprietary content are always user-provided; this crate stores no keys.

pub mod exefs;
pub mod hfs0;
pub mod nca;
pub mod nro;
pub mod nso;
pub mod pfs0;
pub mod reader;
pub mod romfs;
pub mod romfs_vfs;
pub mod xci;
use thiserror::Error;
pub mod nca_crypto;
pub mod nca_partition;
pub mod nca_rom_storage;
pub mod npdm;
#[derive(Debug, Error)]
pub enum IngestError {
    #[error("unsupported input or feature")]
    Unsupported,
    #[error("input is truncated or structurally invalid")]
    Invalid,
    #[error("integrity verification failed: {0}")]
    Integrity(&'static str),
    #[error("required user-provided key is unavailable")]
    MissingKey,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Nro,
    Nso,
    Nca,
    Nsp,
    Pfs0,
    Hfs0,
    Xci,
    RomFs,
    ExeFs,
    Unknown,
}
pub fn identify(bytes: &[u8]) -> InputKind {
    if bytes.get(0x10..0x14) == Some(b"NRO0".as_slice()) {
        return InputKind::Nro;
    }
    if bytes.get(0..4) == Some(b"NSO0".as_slice()) {
        return InputKind::Nso;
    }
    if bytes.get(0..4) == Some(b"PFS0".as_slice()) {
        return InputKind::Pfs0;
    }
    if bytes.get(0..4) == Some(b"HFS0".as_slice()) {
        return InputKind::Hfs0;
    }
    let nca = bytes.get(0x200..0x204);
    if nca == Some(b"NCA0".as_slice())
        || nca == Some(b"NCA1".as_slice())
        || nca == Some(b"NCA2".as_slice())
        || nca == Some(b"NCA3".as_slice())
    {
        return InputKind::Nca;
    }
    if bytes.get(0x100..0x104) == Some(b"HEAD".as_slice()) {
        return InputKind::Xci;
    }
    if bytes.len() >= 0x50 && bytes.get(0..8) == Some(0x50u64.to_le_bytes().as_slice()) {
        return InputKind::RomFs;
    }
    InputKind::Unknown
}
