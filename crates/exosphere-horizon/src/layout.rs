// SPDX-License-Identifier: GPL-3.0-or-later
//! Reserved virtual regions for the supported 39-bit process profile.

use crate::error::{HorizonError, HorizonResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualRegion {
    pub base: u64,
    pub size: u64,
}

impl VirtualRegion {
    pub fn contains(self, base: u64, size: u64) -> bool {
        match (base.checked_add(size), self.base.checked_add(self.size)) {
            (Some(end), Some(region_end)) => base >= self.base && end <= region_end,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessLayout {
    pub address_limit: u64,
    pub aslr: VirtualRegion,
    pub alias: VirtualRegion,
    pub heap: VirtualRegion,
    pub stack: VirtualRegion,
    pub tls: VirtualRegion,
}

impl ProcessLayout {
    /// Deterministic diagnostic placement, not yet randomized ASLR. The returned
    /// reservations are authoritative for both loading and subsequent memory SVCs.
    pub fn diagnostic_39_bit(
        code_base: u64,
        code_size: u64,
        stack_top: u64,
        stack_size: u64,
    ) -> HorizonResult<Self> {
        let address_limit = 1 << 39;
        let stack_reservation = 0x8000_0000;
        let stack_base = stack_top
            .checked_sub(stack_reservation)
            .ok_or(HorizonError::InvalidAddress(stack_top))?;
        let layout = Self {
            address_limit,
            aslr: VirtualRegion {
                base: 0x0800_0000,
                size: address_limit - 0x0800_0000,
            },
            alias: VirtualRegion {
                base: 0x1_0000_0000,
                size: 0x10_0000_0000,
            },
            heap: VirtualRegion {
                base: 0x20_0000_0000,
                size: 0x1_8000_0000,
            },
            stack: VirtualRegion {
                base: stack_base,
                size: stack_reservation,
            },
            tls: VirtualRegion {
                base: 0x70_0000_0000,
                size: 0x10_0000,
            },
        };
        let code_end = code_base
            .checked_add(code_size)
            .ok_or(HorizonError::InvalidAddress(code_base))?;
        if code_size == 0
            || stack_size > stack_reservation
            || stack_top >= address_limit
            || !layout.aslr.contains(code_base, code_size)
            || !layout.aslr.contains(stack_base, stack_reservation)
        {
            return Err(HorizonError::InvalidAddress(code_base));
        }
        let reservations = [layout.alias, layout.heap, layout.stack, layout.tls];
        for (index, region) in reservations.iter().enumerate() {
            if code_base < region.base + region.size && code_end > region.base {
                return Err(HorizonError::MemoryOverlap);
            }
            for other in &reservations[..index] {
                if region.base < other.base + other.size && region.base + region.size > other.base {
                    return Err(HorizonError::MemoryOverlap);
                }
            }
        }
        Ok(layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_enclose_stack_and_do_not_overlap_code() {
        let layout =
            ProcessLayout::diagnostic_39_bit(0x7100000000, 0x3000000, 0x7f00000000, 0x40000)
                .unwrap();
        assert!(layout.stack.contains(0x7efffc0000, 0x40000));
        assert!(layout.aslr.contains(layout.heap.base, layout.heap.size));
        assert!(
            ProcessLayout::diagnostic_39_bit(layout.heap.base, 0x1000, 0x7f00000000, 0x40000)
                .is_err()
        );
        assert!(ProcessLayout::diagnostic_39_bit(u64::MAX, 0x1000, 0x7f00000000, 0x40000).is_err());
        assert!(
            ProcessLayout::diagnostic_39_bit(0x7100000000, 0x1000, 0x7f00000000, 0x80001000)
                .is_err()
        );
    }
}
