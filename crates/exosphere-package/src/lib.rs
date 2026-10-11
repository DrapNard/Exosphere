// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! mmap-oriented package metadata skeleton.

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionType {
    CpuCode = 1,
    CpuIr = 2,
    GpuIr = 3,
    ShaderIr = 4,
    SpirV = 5,
    MediaIr = 6,
    ResourceIndex = 7,
    ResourceData = 8,
    Symbols = 9,
    Relocations = 10,
    Dependencies = 11,
    Provenance = 12,
    Debug = 13,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None = 0,
    Zstd = 1,
}

/// Fixed-layout package header. Offsets are always absolute byte offsets from the start of file.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PackageHeaderV1 {
    pub magic: [u8; 8],
    pub format_version: u32,
    pub package_type: u32,
    pub flags: u32,
    pub section_count: u32,
    pub manifest_offset: u64,
    pub object_table_offset: u64,
    pub dependency_table_offset: u64,
    pub provenance_offset: u64,
    pub header_checksum: [u8; 32],
}
