// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! CMIF request/response headers carried inside HIPC raw data.

use crate::error::{HorizonError, HorizonResult};

pub const SFCI_MAGIC: u32 = u32::from_le_bytes(*b"SFCI");
pub const SFCO_MAGIC: u32 = u32::from_le_bytes(*b"SFCO");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CmifInHeader {
    pub version: u16,
    pub method_id: u32,
    pub token: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CmifOutHeader {
    pub version: u16,
    pub result: u32,
    /// Firmware 14+ uses this field as InterfaceId. Older profiles treat it as reserved.
    pub interface_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmifRequest<'a> {
    pub header: CmifInHeader,
    pub arguments: &'a [u8],
}

pub fn parse_request(raw_data: &[u8]) -> HorizonResult<CmifRequest<'_>> {
    // HIPC guarantees 16 bytes total padding around the CMIF body. Depending on alignment this can
    // put the SFCI header at one of the first 16 byte offsets. Scan only that bounded prefix rather
    // than accepting an arbitrary embedded signature.
    let max_start = raw_data.len().saturating_sub(16).min(16);
    let start = (0..=max_start)
        .find(|offset| raw_data.get(*offset..*offset + 4) == Some(&b"SFCI"[..]))
        .ok_or_else(|| {
            HorizonError::IpcProtocol("CMIF SFCI header not found in aligned raw data".into())
        })?;

    let header = raw_data
        .get(start..start + 16)
        .ok_or_else(|| HorizonError::IpcProtocol("CMIF input header is truncated".into()))?;
    let magic = u32::from_le_bytes(header[0..4].try_into().expect("length checked"));
    if magic != SFCI_MAGIC {
        return Err(HorizonError::IpcProtocol("invalid CMIF input magic".into()));
    }
    let version = u16::from_le_bytes(header[4..6].try_into().expect("length checked"));
    let method_id = u32::from_le_bytes(header[8..12].try_into().expect("length checked"));
    let token = u32::from_le_bytes(header[12..16].try_into().expect("length checked"));
    Ok(CmifRequest {
        header: CmifInHeader {
            version,
            method_id,
            token,
        },
        arguments: &raw_data[start + 16..],
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmifResponse<'a> {
    pub header: CmifOutHeader,
    pub payload: &'a [u8],
}

pub fn parse_response(raw_data: &[u8]) -> HorizonResult<CmifResponse<'_>> {
    let max_start = raw_data.len().saturating_sub(16).min(16);
    let start = (0..=max_start)
        .find(|offset| raw_data.get(*offset..*offset + 4) == Some(&b"SFCO"[..]))
        .ok_or_else(|| {
            HorizonError::IpcProtocol("CMIF SFCO header not found in aligned raw data".into())
        })?;
    let header = raw_data
        .get(start..start + 16)
        .ok_or_else(|| HorizonError::IpcProtocol("CMIF output header is truncated".into()))?;
    let version = u16::from_le_bytes(header[4..6].try_into().expect("length checked"));
    let result = u32::from_le_bytes(header[8..12].try_into().expect("length checked"));
    let interface_id = u32::from_le_bytes(header[12..16].try_into().expect("length checked"));
    Ok(CmifResponse {
        header: CmifOutHeader {
            version,
            result,
            interface_id,
        },
        payload: &raw_data[start + 16..],
    })
}

/// Encodes a CMIF response body. HIPC descriptor/padding framing is handled separately.
pub fn encode_response(header: CmifOutHeader, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + payload.len());
    out.extend_from_slice(&SFCO_MAGIC.to_le_bytes());
    out.extend_from_slice(&header.version.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&header.result.to_le_bytes());
    out.extend_from_slice(&header.interface_id.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cmif_input() {
        let mut bytes = vec![0; 16];
        bytes[0..4].copy_from_slice(b"SFCI");
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        bytes[8..12].copy_from_slice(&42u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&7u32.to_le_bytes());
        let request = parse_request(&bytes).unwrap();
        assert_eq!(request.header.method_id, 42);
        assert_eq!(request.header.token, 7);
    }
}
