// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Maxwell Compute Queue Metadata (QMD 01.07) partial parser.
//!
//! The 0x100-byte layout follows NVIDIA's published class definition as mirrored/documented by
//! deko3d. Only fields required for safe shader/dispatch discovery are interpreted here; all raw
//! dwords remain available for future coverage.

use crate::maxwell_state::RawGpuAddress;

pub const QMD_SIZE: usize = 0x100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeQmd {
    pub raw: [u32; 64],
    pub program_offset: u32,
    pub grid: [u32; 3],
    pub block: [u32; 3],
    pub shared_memory_size: u32,
    pub register_count: u8,
    pub constant_buffer_valid: u8,
    pub constant_buffers: [QmdConstantBuffer; 8],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QmdConstantBuffer {
    pub address: RawGpuAddress,
    pub size: u32,
    pub invalidate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QmdError {
    WrongSize,
}

pub fn parse(bytes: &[u8]) -> Result<ComputeQmd, QmdError> {
    if bytes.len() < QMD_SIZE {
        return Err(QmdError::WrongSize);
    }
    let mut raw = [0u32; 64];
    for (i, ch) in bytes[..QMD_SIZE].chunks_exact(4).enumerate() {
        raw[i] = u32::from_le_bytes(ch.try_into().expect("qmd dword"));
    }
    let program_offset = raw[0x20 / 4];
    let grid = [raw[0x30 / 4], raw[0x34 / 4] & 0xffff, raw[0x34 / 4] >> 16];
    let block = [
        raw[0x48 / 4] >> 16,
        raw[0x4c / 4] & 0xffff,
        raw[0x4c / 4] >> 16,
    ];
    let shared_memory_size = raw[0x44 / 4] & 0x3ffff;
    let constant_buffer_valid = (raw[0x50 / 4] & 0xff) as u8;
    let register_count = (raw[0xb8 / 4] >> 24) as u8;
    let mut constant_buffers = [QmdConstantBuffer::default(); 8];
    for (i, cb) in constant_buffers.iter_mut().enumerate() {
        let off = 0x74 + i * 8;
        let lo = raw[off / 4];
        let packed = raw[(off + 4) / 4];
        cb.address = RawGpuAddress {
            a: packed & 0xff,
            b: lo,
        };
        cb.invalidate = ((packed >> 14) & 1) != 0;
        cb.size = (packed >> 15) & 0x1ffff;
    }
    Ok(ComputeQmd {
        raw,
        program_offset,
        grid,
        block,
        shared_memory_size,
        register_count,
        constant_buffer_valid,
        constant_buffers,
    })
}

impl ComputeQmd {
    pub fn program_address(&self, program_region: RawGpuAddress) -> u64 {
        program_region
            .high_low()
            .wrapping_add(u64::from(self.program_offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_dispatch_dimensions() {
        let mut b = [0u8; QMD_SIZE];
        b[0x20..0x24].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x30..0x34].copy_from_slice(&4u32.to_le_bytes());
        b[0x34..0x38].copy_from_slice(&((2u32) | (3u32 << 16)).to_le_bytes());
        b[0x48..0x4c].copy_from_slice(&(8u32 << 16).to_le_bytes());
        b[0x4c..0x50].copy_from_slice(&((4u32) | (2u32 << 16)).to_le_bytes());
        let q = parse(&b).unwrap();
        assert_eq!(q.grid, [4, 2, 3]);
        assert_eq!(q.block, [8, 4, 2]);
        assert_eq!(q.program_offset, 0x80);
    }
}
