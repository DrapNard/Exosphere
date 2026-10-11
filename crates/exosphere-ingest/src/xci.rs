// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Structural XCI parser.
//!
//! Exosphere intentionally treats game-card security/authentication material as opaque. This module
//! only discovers the root HFS0 partition and exposes its entries.

use crate::{
    IngestError,
    hfs0::{self, Hfs0},
};

pub const XCI_HEADER_OFFSET: usize = 0x100;
pub const ROOT_HFS0_OFFSET: usize = 0xF000;

#[derive(Debug, Clone)]
pub struct Xci {
    pub root: Hfs0,
    pub root_offset: usize,
}

pub fn parse(bytes: &[u8]) -> Result<Xci, IngestError> {
    if bytes.get(XCI_HEADER_OFFSET..XCI_HEADER_OFFSET + 4) != Some(b"HEAD") {
        return Err(IngestError::Invalid);
    }
    // Retail images conventionally place the root HFS0 at 0xF000. For resilience, if that position
    // is absent we scan the first MiB on 0x200-byte boundaries rather than trusting one layout.
    let root_offset = if bytes.get(ROOT_HFS0_OFFSET..ROOT_HFS0_OFFSET + 4) == Some(b"HFS0") {
        ROOT_HFS0_OFFSET
    } else {
        (0..bytes.len().min(0x10_0000))
            .step_by(0x200)
            .find(|&o| bytes.get(o..o + 4) == Some(b"HFS0"))
            .ok_or(IngestError::Invalid)?
    };
    Ok(Xci {
        root: hfs0::parse(&bytes[root_offset..])?,
        root_offset,
    })
}
