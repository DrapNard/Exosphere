// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! TIC/TSC table reads through the backend-neutral [`GpuMemory`] abstraction.

use crate::{
    descriptors::{SamplerDescriptor, TextureDescriptor},
    shader_memory::{GpuMemory, GpuMemoryError},
    tic_tsc::{DecodedSamplerState, decode_tic, decode_tsc},
};

pub const DESCRIPTOR_SIZE: u64 = 0x20;

pub fn read_words(memory: &dyn GpuMemory, address: u64) -> Result<[u32; 8], GpuMemoryError> {
    address
        .checked_add(DESCRIPTOR_SIZE)
        .ok_or(GpuMemoryError::AddressOverflow)?;
    let bytes = memory.read(address, DESCRIPTOR_SIZE as usize)?;
    if bytes.len() != DESCRIPTOR_SIZE as usize {
        return Err(GpuMemoryError::BackendRead {
            address,
            size: DESCRIPTOR_SIZE as usize,
            reason: format!(
                "descriptor provider returned {} bytes, expected 32",
                bytes.len()
            ),
        });
    }
    let mut words = [0u32; 8];
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        words[index] = u32::from_le_bytes(chunk.try_into().expect("descriptor word"));
    }
    Ok(words)
}

pub fn read_tic(
    memory: &dyn GpuMemory,
    base: u64,
    index: u32,
) -> Result<TextureDescriptor, GpuMemoryError> {
    let address = base
        .checked_add(u64::from(index) * DESCRIPTOR_SIZE)
        .ok_or(GpuMemoryError::AddressOverflow)?;
    Ok(decode_tic(read_words(memory, address)?, None))
}

pub fn read_tsc(
    memory: &dyn GpuMemory,
    base: u64,
    index: u32,
) -> Result<(SamplerDescriptor, DecodedSamplerState), GpuMemoryError> {
    let address = base
        .checked_add(u64::from(index) * DESCRIPTOR_SIZE)
        .ok_or(GpuMemoryError::AddressOverflow)?;
    Ok(decode_tsc(read_words(memory, address)?, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Memory(usize);
    impl GpuMemory for Memory {
        fn read(&self, _: u64, size: usize) -> Result<Vec<u8>, GpuMemoryError> {
            assert_eq!(size, 32);
            Ok((0..self.0).map(|i| i as u8).collect())
        }
    }

    #[test]
    fn descriptor_reads_require_exact_length_and_nonoverflowing_range() {
        let words = read_words(&Memory(32), 0).unwrap();
        assert_eq!(words[0], 0x0302_0100);
        assert_eq!(words[7], 0x1f1e_1d1c);
        for length in [0, 4, 31, 33, 36] {
            assert!(matches!(
                read_words(&Memory(length), 0),
                Err(GpuMemoryError::BackendRead { .. })
            ));
        }
        assert_eq!(
            read_words(&Memory(32), u64::MAX - 31),
            Err(GpuMemoryError::AddressOverflow)
        );
        assert_eq!(
            read_tic(&Memory(32), u64::MAX - 15, 1),
            Err(GpuMemoryError::AddressOverflow)
        );
        assert_eq!(
            read_tsc(&Memory(32), u64::MAX - 15, 0),
            Err(GpuMemoryError::AddressOverflow)
        );
    }
}
