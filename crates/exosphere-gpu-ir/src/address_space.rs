// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! GPU virtual-address mappings.
//!
//! Maxwell command streams mostly traffic in guest GPU virtual addresses.  Canonical IR must not
//! treat those addresses as resource identities because aliases, remaps and subranges are legal.
//! This module owns the translation from guest GPU VA ranges to stable Exosphere resources.

use std::collections::BTreeMap;

use crate::resource::ResourceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuMapPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl GpuMapPermissions {
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
        execute: false,
    };
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        execute: false,
    };
    pub const EXECUTE_READ: Self = Self {
        read: true,
        write: false,
        execute: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuMapping {
    pub guest_base: u64,
    pub size: u64,
    pub resource: ResourceId,
    pub resource_offset: u64,
    pub permissions: GpuMapPermissions,
}

impl GpuMapping {
    pub fn end(self) -> Option<u64> {
        self.guest_base.checked_add(self.size)
    }

    pub fn contains(self, address: u64, length: u64) -> bool {
        let Some(request_end) = address.checked_add(length) else {
            return false;
        };
        let Some(mapping_end) = self.end() else {
            return false;
        };
        address >= self.guest_base && request_end <= mapping_end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedGpuRange {
    pub resource: ResourceId,
    pub offset: u64,
    pub length: u64,
    pub permissions: GpuMapPermissions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuAddressError {
    ZeroSizedMapping,
    AddressOverflow,
    Overlap {
        existing_base: u64,
        existing_size: u64,
    },
    Unmapped {
        address: u64,
        length: u64,
    },
}

impl std::fmt::Display for GpuAddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroSizedMapping => write!(f, "GPU mapping size must be non-zero"),
            Self::AddressOverflow => write!(f, "GPU address range overflow"),
            Self::Overlap {
                existing_base,
                existing_size,
            } => write!(
                f,
                "GPU mapping overlaps existing range at 0x{existing_base:x} (0x{existing_size:x} bytes)"
            ),
            Self::Unmapped { address, length } => {
                write!(f, "unmapped GPU range 0x{address:x}..+0x{length:x}")
            }
        }
    }
}

impl std::error::Error for GpuAddressError {}

#[derive(Debug, Clone, Default)]
pub struct GpuAddressSpace {
    mappings: BTreeMap<u64, GpuMapping>,
}

impl GpuAddressSpace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mappings(&self) -> impl Iterator<Item = &GpuMapping> {
        self.mappings.values()
    }

    pub fn map(&mut self, mapping: GpuMapping) -> Result<(), GpuAddressError> {
        if mapping.size == 0 {
            return Err(GpuAddressError::ZeroSizedMapping);
        }
        let end = mapping.end().ok_or(GpuAddressError::AddressOverflow)?;
        mapping
            .resource_offset
            .checked_add(mapping.size)
            .ok_or(GpuAddressError::AddressOverflow)?;

        if let Some((_, previous)) = self.mappings.range(..=mapping.guest_base).next_back() {
            let previous_end = previous.end().ok_or(GpuAddressError::AddressOverflow)?;
            if previous_end > mapping.guest_base {
                return Err(GpuAddressError::Overlap {
                    existing_base: previous.guest_base,
                    existing_size: previous.size,
                });
            }
        }
        if let Some((_, next)) = self.mappings.range(mapping.guest_base..).next() {
            if next.guest_base < end {
                return Err(GpuAddressError::Overlap {
                    existing_base: next.guest_base,
                    existing_size: next.size,
                });
            }
        }

        self.mappings.insert(mapping.guest_base, mapping);
        Ok(())
    }

    pub fn unmap(&mut self, guest_base: u64) -> Option<GpuMapping> {
        self.mappings.remove(&guest_base)
    }

    pub fn resolve(&self, address: u64, length: u64) -> Result<ResolvedGpuRange, GpuAddressError> {
        let Some((_, mapping)) = self.mappings.range(..=address).next_back() else {
            return Err(GpuAddressError::Unmapped { address, length });
        };
        if !mapping.contains(address, length) {
            return Err(GpuAddressError::Unmapped { address, length });
        }
        Ok(ResolvedGpuRange {
            resource: mapping.resource,
            offset: mapping.resource_offset + (address - mapping.guest_base),
            length,
            permissions: mapping.permissions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_subranges_without_using_address_as_identity() {
        let mut space = GpuAddressSpace::new();
        space
            .map(GpuMapping {
                guest_base: 0x1000,
                size: 0x1000,
                resource: ResourceId(7),
                resource_offset: 0x80,
                permissions: GpuMapPermissions::READ_WRITE,
            })
            .unwrap();
        let resolved = space.resolve(0x1180, 0x20).unwrap();
        assert_eq!(resolved.resource, ResourceId(7));
        assert_eq!(resolved.offset, 0x200);
        assert_eq!(resolved.length, 0x20);
    }

    #[test]
    fn rejects_overlapping_mappings() {
        let mut space = GpuAddressSpace::new();
        assert_eq!(
            space.map(GpuMapping {
                guest_base: 0x8000,
                size: 16,
                resource: ResourceId(3),
                resource_offset: u64::MAX - 8,
                permissions: GpuMapPermissions::READ_ONLY,
            }),
            Err(GpuAddressError::AddressOverflow)
        );
        assert_eq!(space.mappings().count(), 0);
        space
            .map(GpuMapping {
                guest_base: 0x1000,
                size: 0x1000,
                resource: ResourceId(1),
                resource_offset: 0,
                permissions: GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        assert!(matches!(
            space.map(GpuMapping {
                guest_base: 0x1800,
                size: 0x1000,
                resource: ResourceId(2),
                resource_offset: 0,
                permissions: GpuMapPermissions::READ_ONLY
            }),
            Err(GpuAddressError::Overlap { .. })
        ));
    }
}
