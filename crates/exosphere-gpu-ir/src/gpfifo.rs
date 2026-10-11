// SPDX-License-Identifier: GPL-3.0-or-later
//! Maxwell B06F GPFIFO descriptors, per NVIDIA's class/clb06f.h entry fields.
//! Control entries are distinct from zero-length command buffers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpfifoPushbuffer {
    pub address: u64,
    pub word_count: u32,
    pub conditional_fetch: bool,
    pub privileged: bool,
    pub subroutine: bool,
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpfifoEntry {
    Pushbuffer(GpfifoPushbuffer),
    Control { operand: u32, opcode: u8, high: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservedGpfifoAddressBit;

impl GpfifoEntry {
    /// Admit only an unconditional, unprivileged, top-level NOP with no wait
    /// or reserved high bits. The low word is an operand, not a memory address.
    pub fn is_nop(self) -> bool {
        matches!(
            self,
            Self::Control {
                opcode: 0,
                high: 0,
                ..
            }
        )
    }

    pub fn decode(raw: u64) -> Result<Self, ReservedGpfifoAddressBit> {
        let low = raw as u32;
        let high = (raw >> 32) as u32;
        let word_count = (high >> 10) & 0x1fffff;
        if word_count == 0 {
            return Ok(Self::Control {
                operand: low,
                opcode: high as u8,
                high,
            });
        }
        if low & 2 != 0 {
            return Err(ReservedGpfifoAddressBit);
        }
        Ok(Self::Pushbuffer(GpfifoPushbuffer {
            address: u64::from(low & !3) | (u64::from(high & 0xff) << 32),
            word_count,
            conditional_fetch: low & 1 != 0,
            privileged: high & (1 << 8) != 0,
            subroutine: high & (1 << 9) != 0,
            wait: high & (1 << 31) != 0,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_full_address_count_and_independent_control_flags() {
        let raw = 0xfffffffd | (0xffffffefu64 << 32);
        let GpfifoEntry::Pushbuffer(entry) = GpfifoEntry::decode(raw).unwrap() else {
            panic!("expected pushbuffer")
        };
        assert_eq!(entry.address, 0xeffffffffc);
        assert_eq!(entry.word_count, 0x1fffff);
        assert!(entry.conditional_fetch && entry.privileged && entry.subroutine && entry.wait);
        assert!(GpfifoEntry::decode(raw | 2).is_err());
        let GpfifoEntry::Pushbuffer(entry) = GpfifoEntry::decode(0x1000 | (1u64 << 42)).unwrap()
        else {
            panic!("expected pushbuffer")
        };
        assert_eq!(entry.address, 0x1000);
        assert_eq!(entry.word_count, 1);
        assert!(!entry.conditional_fetch && !entry.privileged && !entry.subroutine && !entry.wait);
    }

    #[test]
    fn zero_length_is_a_control_entry_and_preserves_unknown_opcodes() {
        for opcode in [0u8, 1, 2, 3, 0xff] {
            assert_eq!(
                GpfifoEntry::decode(0x1234567b | (u64::from(opcode) << 32)).unwrap(),
                GpfifoEntry::Control {
                    operand: 0x1234567b,
                    opcode,
                    high: u32::from(opcode)
                }
            );
        }
    }
}
