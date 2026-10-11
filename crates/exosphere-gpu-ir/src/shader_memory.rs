// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! GPU-memory abstraction used by shader discovery.
//!
//! The GPU front-end must not know whether memory comes from a real nvmap allocation, an emulator
//! address space, a captured trace, or a unit-test buffer. This trait keeps shader discovery behind
//! a narrow, testable boundary.

use crate::maxwell_state::RawGpuAddress;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuMemoryError {
    Unmapped {
        address: u64,
        size: usize,
    },
    AddressOverflow,
    InvalidShaderConfiguration {
        slot: u8,
        select: u32,
    },
    BackendRead {
        address: u64,
        size: usize,
        reason: String,
    },
    BackendWrite {
        address: u64,
        size: usize,
        reason: String,
    },
}

impl fmt::Display for GpuMemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped { address, size } => write!(
                f,
                "GPU address 0x{address:016x} (size {size}) is not mapped"
            ),
            Self::AddressOverflow => write!(f, "GPU address arithmetic overflow"),
            Self::InvalidShaderConfiguration { slot, select } => write!(
                f,
                "invalid shader selection 0x{select:x} in pipeline slot {slot}"
            ),
            Self::BackendRead {
                address,
                size,
                reason,
            } => write!(
                f,
                "GPU read at 0x{address:x} ({size} bytes) failed: {reason}"
            ),
            Self::BackendWrite {
                address,
                size,
                reason,
            } => write!(
                f,
                "GPU write at 0x{address:x} ({size} bytes) failed: {reason}"
            ),
        }
    }
}
impl std::error::Error for GpuMemoryError {}

pub trait GpuMemory {
    fn read(&self, address: u64, size: usize) -> Result<Vec<u8>, GpuMemoryError>;
}

/// Read/write GPU memory used by runtime resource providers.
///
/// Shader discovery only requires [`GpuMemory`]; backends that must write guest-visible render or
/// transfer results opt into this stronger trait.
pub trait GpuMemoryAccess: GpuMemory {
    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), GpuMemoryError>;
}

/// Describes a shader location relative to the currently programmed Maxwell program region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderLocation {
    pub region: RawGpuAddress,
    pub offset: u32,
    pub size: usize,
}

impl ShaderLocation {
    /// Resolves this location using the common high/low interpretation of the program-region pair.
    /// The caller must only use this helper once its title/trace confirms the pair ordering.
    pub fn resolve_high_low(self) -> Result<u64, GpuMemoryError> {
        self.region
            .high_low()
            .checked_add(u64::from(self.offset))
            .ok_or(GpuMemoryError::AddressOverflow)
    }

    pub fn read_high_low(self, memory: &dyn GpuMemory) -> Result<Vec<u8>, GpuMemoryError> {
        memory.read(self.resolve_high_low()?, self.size)
    }
}

/// Tiny deterministic GPU memory useful for traces and tests.
#[derive(Debug, Clone)]
pub struct LinearGpuMemory {
    base: u64,
    bytes: Vec<u8>,
}

impl LinearGpuMemory {
    pub fn new(base: u64, bytes: Vec<u8>) -> Self {
        Self { base, bytes }
    }
}

impl GpuMemory for LinearGpuMemory {
    fn read(&self, address: u64, size: usize) -> Result<Vec<u8>, GpuMemoryError> {
        let relative = address
            .checked_sub(self.base)
            .ok_or(GpuMemoryError::Unmapped { address, size })?;
        let start = usize::try_from(relative).map_err(|_| GpuMemoryError::AddressOverflow)?;
        let end = start
            .checked_add(size)
            .ok_or(GpuMemoryError::AddressOverflow)?;
        self.bytes
            .get(start..end)
            .map(ToOwned::to_owned)
            .ok_or(GpuMemoryError::Unmapped { address, size })
    }
}

impl GpuMemoryAccess for LinearGpuMemory {
    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), GpuMemoryError> {
        let relative = address
            .checked_sub(self.base)
            .ok_or(GpuMemoryError::Unmapped {
                address,
                size: data.len(),
            })?;
        let start = usize::try_from(relative).map_err(|_| GpuMemoryError::AddressOverflow)?;
        let end = start
            .checked_add(data.len())
            .ok_or(GpuMemoryError::AddressOverflow)?;
        let destination = self
            .bytes
            .get_mut(start..end)
            .ok_or(GpuMemoryError::Unmapped {
                address,
                size: data.len(),
            })?;
        destination.copy_from_slice(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_shader_relative_to_program_region() {
        let memory = LinearGpuMemory::new(0x1_0000_0000, vec![0, 1, 2, 3, 4, 5]);
        let location = ShaderLocation {
            region: RawGpuAddress { a: 1, b: 0 },
            offset: 2,
            size: 3,
        };
        assert_eq!(location.read_high_low(&memory).unwrap(), vec![2, 3, 4]);
    }
}
