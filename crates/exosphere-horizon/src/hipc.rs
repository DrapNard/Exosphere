// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Horizon IPC (HIPC) wire-format parsing.
//!
//! This module owns only transport framing. It decodes native handles plus pointer/map/receive-list
//! descriptors exactly enough for the kernel to materialize guest buffers. CMIF/TIPC decoding stays
//! in separate modules so services never depend on HIPC bitfield layouts.

use crate::error::{HorizonError, HorizonResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum HipcMessageType {
    Invalid = 0,
    InvokeMethod = 1,
    Release = 2,
    InvokeManagerMethod = 3,
    Invoke2MethodOld = 4,
    Invoke2ManagerMethodOld = 5,
    Invoke2Method = 6,
    Invoke2ManagerMethod = 7,
    Unknown = 0xffff,
}
impl From<u16> for HipcMessageType {
    fn from(value: u16) -> Self {
        match value {
            0 => Self::Invalid,
            1 => Self::InvokeMethod,
            2 => Self::Release,
            3 => Self::InvokeManagerMethod,
            4 => Self::Invoke2MethodOld,
            5 => Self::Invoke2ManagerMethodOld,
            6 => Self::Invoke2Method,
            7 => Self::Invoke2ManagerMethod,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HipcHeader {
    pub message_type: HipcMessageType,
    pub raw_tag: u16,
    pub pointer_count: u8,
    pub send_count: u8,
    pub receive_count: u8,
    pub exchange_count: u8,
    pub raw_word_count: u16,
    pub receive_list_count: u8,
    pub receive_list_offset: u16,
    pub has_special_header: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HipcSpecialHeader {
    pub send_pid: bool,
    pub copy_handle_count: u8,
    pub move_handle_count: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HipcHandles {
    pub process_id: Option<u64>,
    pub copy_handles: Vec<u32>,
    pub move_handles: Vec<u32>,
}

/// HIPC pointer descriptor (X descriptor).
///
/// Pointer descriptors are commonly used for small input/output buffers that fit in a service's
/// pointer buffer. `index` is the receive index carried by Horizon's marshaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HipcPointerDescriptor {
    pub index: u8,
    pub address: u64,
    pub size: u16,
}

/// HIPC map-alias descriptor (A/B/W descriptor depending on the array it came from).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HipcMapDescriptor {
    pub address: u64,
    pub size: u64,
    pub transfer_attributes: u8,
}

/// Receive-list descriptor (C descriptor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HipcReceiveListDescriptor {
    pub address: u64,
    pub size: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HipcMessage<'a> {
    pub header: HipcHeader,
    pub special: Option<HipcSpecialHeader>,
    pub handles: HipcHandles,
    pub pointers: Vec<HipcPointerDescriptor>,
    pub send_buffers: Vec<HipcMapDescriptor>,
    pub receive_buffers: Vec<HipcMapDescriptor>,
    pub exchange_buffers: Vec<HipcMapDescriptor>,
    pub receive_list: Vec<HipcReceiveListDescriptor>,
    pub raw_data: &'a [u8],
}

/// Parses one HIPC command buffer.
///
/// All address/size fields are bounds-independent guest values. Bounds and permissions are checked
/// later against the calling process's `AddressSpace`; parsing a descriptor never grants access.
pub fn parse(bytes: &[u8]) -> HorizonResult<HipcMessage<'_>> {
    if bytes.len() < 8 {
        return Err(protocol("HIPC header is truncated"));
    }
    let h0 = read_u32(bytes, 0)?;
    let h1 = read_u32(bytes, 4)?;
    let raw_tag = (h0 & 0xffff) as u16;
    let header = HipcHeader {
        message_type: HipcMessageType::from(raw_tag),
        raw_tag,
        pointer_count: ((h0 >> 16) & 0xf) as u8,
        send_count: ((h0 >> 20) & 0xf) as u8,
        receive_count: ((h0 >> 24) & 0xf) as u8,
        exchange_count: ((h0 >> 28) & 0xf) as u8,
        raw_word_count: (h1 & 0x3ff) as u16,
        receive_list_count: ((h1 >> 10) & 0xf) as u8,
        receive_list_offset: ((h1 >> 20) & 0x7ff) as u16,
        has_special_header: (h1 >> 31) != 0,
    };

    let mut cursor = 8usize;
    let mut handles = HipcHandles::default();
    let special = if header.has_special_header {
        let value = read_u32(bytes, cursor)?;
        cursor += 4;
        let decoded = HipcSpecialHeader {
            send_pid: value & 1 != 0,
            copy_handle_count: ((value >> 1) & 0xf) as u8,
            move_handle_count: ((value >> 5) & 0xf) as u8,
        };
        if decoded.send_pid {
            handles.process_id = Some(read_u64(bytes, cursor)?);
            cursor += 8;
        }
        for _ in 0..decoded.copy_handle_count {
            handles.copy_handles.push(read_u32(bytes, cursor)?);
            cursor += 4;
        }
        for _ in 0..decoded.move_handle_count {
            handles.move_handles.push(read_u32(bytes, cursor)?);
            cursor += 4;
        }
        Some(decoded)
    } else {
        None
    };

    let mut pointers = Vec::with_capacity(header.pointer_count as usize);
    for _ in 0..header.pointer_count {
        pointers.push(decode_pointer(bytes, cursor)?);
        cursor = checked_advance(cursor, 8, bytes.len())?;
    }
    let mut send_buffers = Vec::with_capacity(header.send_count as usize);
    for _ in 0..header.send_count {
        send_buffers.push(decode_map(bytes, cursor)?);
        cursor = checked_advance(cursor, 12, bytes.len())?;
    }
    let mut receive_buffers = Vec::with_capacity(header.receive_count as usize);
    for _ in 0..header.receive_count {
        receive_buffers.push(decode_map(bytes, cursor)?);
        cursor = checked_advance(cursor, 12, bytes.len())?;
    }
    let mut exchange_buffers = Vec::with_capacity(header.exchange_count as usize);
    for _ in 0..header.exchange_count {
        exchange_buffers.push(decode_map(bytes, cursor)?);
        cursor = checked_advance(cursor, 12, bytes.len())?;
    }

    let raw_len = usize::from(header.raw_word_count) * 4;
    let raw_end = cursor
        .checked_add(raw_len)
        .ok_or_else(|| protocol("HIPC raw-data size overflow"))?;
    if raw_end > bytes.len() {
        return Err(protocol("HIPC raw data is truncated"));
    }
    let raw_data = &bytes[cursor..raw_end];

    // Explicit C descriptors are addressed by Header1.ReceiveListOffset. Count values 0/1 have
    // special semantics; for v0.1 we expose only descriptors physically present in the command
    // buffer and leave the inlined pointer-buffer case to the service marshaller.
    let explicit_receive_count = match header.receive_list_count {
        0 | 1 => 0,
        2 => 1,
        n => usize::from(n - 2),
    };
    let mut receive_list = Vec::with_capacity(explicit_receive_count);
    if explicit_receive_count > 0 {
        let mut c_cursor = usize::from(header.receive_list_offset) * 4;
        for _ in 0..explicit_receive_count {
            receive_list.push(decode_receive_list(bytes, c_cursor)?);
            c_cursor = checked_advance(c_cursor, 8, bytes.len())?;
        }
    }

    Ok(HipcMessage {
        header,
        special,
        handles,
        pointers,
        send_buffers,
        receive_buffers,
        exchange_buffers,
        receive_list,
        raw_data,
    })
}

fn decode_pointer(bytes: &[u8], offset: usize) -> HorizonResult<HipcPointerDescriptor> {
    let d0 = read_u32(bytes, offset)?;
    let d1 = read_u32(bytes, offset + 4)?;
    let index = (d0 & 0xf) as u8;
    let address =
        u64::from(d1) | (u64::from((d0 >> 12) & 0xf) << 32) | (u64::from((d0 >> 6) & 0x7) << 36);
    let size = (d0 >> 16) as u16;
    Ok(HipcPointerDescriptor {
        index,
        address,
        size,
    })
}

fn decode_map(bytes: &[u8], offset: usize) -> HorizonResult<HipcMapDescriptor> {
    let d0 = read_u32(bytes, offset)?;
    let d1 = read_u32(bytes, offset + 4)?;
    let d2 = read_u32(bytes, offset + 8)?;
    let size = u64::from(d0) | (u64::from((d2 >> 24) & 0xf) << 32);
    let address =
        u64::from(d1) | (u64::from((d2 >> 28) & 0xf) << 32) | (u64::from((d2 >> 2) & 0x7) << 36);
    Ok(HipcMapDescriptor {
        address,
        size,
        transfer_attributes: (d2 & 0x3) as u8,
    })
}

fn decode_receive_list(bytes: &[u8], offset: usize) -> HorizonResult<HipcReceiveListDescriptor> {
    let d0 = read_u32(bytes, offset)?;
    let d1 = read_u32(bytes, offset + 4)?;
    let address = u64::from(d0) | (u64::from(d1 & 0x7f) << 32);
    let size = (d1 >> 16) as u16;
    Ok(HipcReceiveListDescriptor { address, size })
}

fn checked_advance(cursor: usize, amount: usize, len: usize) -> HorizonResult<usize> {
    let next = cursor
        .checked_add(amount)
        .ok_or_else(|| protocol("HIPC descriptor offset overflow"))?;
    if next > len {
        return Err(protocol("HIPC descriptor array is truncated"));
    }
    Ok(next)
}
fn read_u32(bytes: &[u8], offset: usize) -> HorizonResult<u32> {
    let data = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| protocol("HIPC u32 is truncated"))?;
    Ok(u32::from_le_bytes(data.try_into().expect("length checked")))
}
fn read_u64(bytes: &[u8], offset: usize) -> HorizonResult<u64> {
    let data = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| protocol("HIPC u64 is truncated"))?;
    Ok(u64::from_le_bytes(data.try_into().expect("length checked")))
}
fn protocol(message: impl Into<String>) -> HorizonError {
    HorizonError::IpcProtocol(message.into())
}

/// Builds a simple HIPC response containing only raw CMIF data and no descriptors/handles.
pub fn encode_simple_cmif_response(cmif: &[u8]) -> Vec<u8> {
    let raw = response_raw_data(cmif, 8);
    let raw_words = (raw.len() / 4).min(0x3ff) as u32;
    let h0 = HipcMessageType::InvokeMethod as u32;
    let h1 = raw_words;
    let mut out = Vec::with_capacity(8 + raw.len());
    out.extend_from_slice(&h0.to_le_bytes());
    out.extend_from_slice(&h1.to_le_bytes());
    out.extend_from_slice(&raw);
    out
}

/// Builds a CMIF response with native copy/move handles.
pub fn encode_cmif_response_with_handles(
    cmif: &[u8],
    copy_handles: &[u32],
    move_handles: &[u32],
) -> HorizonResult<Vec<u8>> {
    if copy_handles.len() > 0xf || move_handles.len() > 0xf {
        return Err(protocol(
            "too many native handles for one HIPC special header",
        ));
    }
    let raw = response_raw_data(cmif, 12 + (copy_handles.len() + move_handles.len()) * 4);
    if raw.len() / 4 > 0x3ff {
        return Err(protocol("CMIF response exceeds HIPC raw-word limit"));
    }
    let h0 = HipcMessageType::InvokeMethod as u32;
    let h1 = (raw.len() as u32 / 4) | (1u32 << 31);
    let special = ((copy_handles.len() as u32) << 1) | ((move_handles.len() as u32) << 5);
    let mut out =
        Vec::with_capacity(12 + (copy_handles.len() + move_handles.len()) * 4 + raw.len());
    out.extend_from_slice(&h0.to_le_bytes());
    out.extend_from_slice(&h1.to_le_bytes());
    out.extend_from_slice(&special.to_le_bytes());
    for handle in copy_handles {
        out.extend_from_slice(&handle.to_le_bytes());
    }
    for handle in move_handles {
        out.extend_from_slice(&handle.to_le_bytes());
    }
    out.extend_from_slice(&raw);
    Ok(out)
}

/// CMIF begins at a 16-byte-aligned absolute message offset. HIPC reserves a
/// total of 16 padding bytes around it, not 16 bytes exclusively before it.
fn response_raw_data(cmif: &[u8], header_size: usize) -> Vec<u8> {
    let front_padding = (16 - header_size % 16) % 16;
    let mut raw = vec![0; front_padding];
    raw.extend_from_slice(cmif);
    raw.resize(raw.len() + 16 - front_padding, 0);
    while raw.len() % 4 != 0 {
        raw.push(0);
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_handle_response_matches_native_wire_layout_without_our_parser() {
        // libnx hipc.h places the special header at byte 8, then copy
        // handles before move handles. Check literal bytes independently of
        // our parser so a shared encoder/decoder mistake cannot pass.
        let cmif = crate::cmif::encode_response(
            crate::cmif::CmifOutHeader {
                version: 0,
                result: 0,
                interface_id: 0,
            },
            &0u32.to_le_bytes(),
        );
        let response = encode_cmif_response_with_handles(&cmif, &[0x0010_0007], &[]).unwrap();
        assert_eq!(
            &response[..16],
            &[
                1, 0, 0, 0, // InvokeMethod response
                9, 0, 0, 128, // 36 raw bytes and a special header
                2, 0, 0, 0, // one copy handle, no PID or move handles
                7, 0, 16, 0,
            ]
        );
        assert_eq!(&response[16..36], cmif.as_slice());
        assert_eq!(&response[36..], &[0; 16]);
    }

    #[test]
    fn response_cmif_is_aligned_for_each_handle_header_size() {
        let cmif = crate::cmif::encode_response(
            crate::cmif::CmifOutHeader {
                version: 0,
                result: 0,
                interface_id: 0,
            },
            &[],
        );
        let plain = encode_simple_cmif_response(&cmif);
        assert_eq!(&plain[16..20], b"SFCO");
        for count in 0..5 {
            let handles = vec![1; count];
            let response = encode_cmif_response_with_handles(&cmif, &[], &handles).unwrap();
            let header_size = 12 + count * 4;
            let aligned = (header_size + 15) & !15;
            assert_eq!(&response[aligned..aligned + 4], b"SFCO");
            let parsed = parse(&response).unwrap();
            assert_eq!(parsed.raw_data.len(), cmif.len() + 16);
            assert_eq!(
                crate::cmif::parse_response(parsed.raw_data)
                    .unwrap()
                    .header
                    .result,
                0
            );
        }
    }
    #[test]
    fn decodes_map_descriptor() {
        let address = 0x12_3456_7890u64;
        let size = 0x3_0000_0040u64;
        let d0 = size as u32;
        let d1 = address as u32;
        let d2 = (((address >> 36) as u32 & 0x7) << 2)
            | (((size >> 32) as u32 & 0xf) << 24)
            | (((address >> 32) as u32 & 0xf) << 28);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&d0.to_le_bytes());
        bytes.extend_from_slice(&d1.to_le_bytes());
        bytes.extend_from_slice(&d2.to_le_bytes());
        let descriptor = decode_map(&bytes, 0).unwrap();
        assert_eq!(descriptor.address, address);
        assert_eq!(descriptor.size, size);
    }
}
