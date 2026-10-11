// SPDX-License-Identifier: GPL-3.0-or-later
//! NVB06F common semaphore registers. Unsupported controls remain explicit.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostSemaphoreOperation {
    pub address: u64,
    pub payload: u32,
    pub control: u32,
    pub address_registers_valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitwiseReduction {
    Xor,
    And,
    Or,
}

impl BitwiseReduction {
    /// NVB06F reduction operation with WFI enabled and a four-byte release.
    /// Signed/unsigned format does not affect bitwise operations. Other bits,
    /// timestamp releases and arithmetic reductions remain unsupported.
    pub fn decode(control: u32) -> Option<Self> {
        match control & !0x8000_0000 {
            0x1100_0010 => Some(Self::Xor),
            0x1900_0010 => Some(Self::And),
            0x2100_0010 => Some(Self::Or),
            _ => None,
        }
    }

    pub fn apply(self, value: u32, payload: u32) -> u32 {
        match self {
            Self::Xor => value ^ payload,
            Self::And => value & payload,
            Self::Or => value | payload,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemaphoreReduction {
    Bitwise(BitwiseReduction),
    Min { signed: bool },
    Max { signed: bool },
    Add,
    Increment,
    Decrement,
}

impl SemaphoreReduction {
    /// Exact Maxwell four-byte, WFI-enabled controls. INC/DEC are unsigned
    /// bounded counters, not arithmetic addition/subtraction of the payload.
    pub fn decode(control: u32) -> Option<Self> {
        if let Some(operation) = BitwiseReduction::decode(control) {
            return Some(Self::Bitwise(operation));
        }
        let signed = control & 0x80000000 == 0;
        match control & !0x80000000 {
            0x01000010 => Some(Self::Min { signed }),
            0x09000010 => Some(Self::Max { signed }),
            0x29000010 => Some(Self::Add),
            0x31000010 if !signed => Some(Self::Increment),
            0x39000010 if !signed => Some(Self::Decrement),
            _ => None,
        }
    }

    pub fn apply(self, value: u32, payload: u32) -> u32 {
        match self {
            Self::Bitwise(operation) => operation.apply(value, payload),
            Self::Min { signed: true } => (value as i32).min(payload as i32) as u32,
            Self::Max { signed: true } => (value as i32).max(payload as i32) as u32,
            Self::Min { signed: false } => value.min(payload),
            Self::Max { signed: false } => value.max(payload),
            Self::Add => value.wrapping_add(payload),
            Self::Increment => {
                if value >= payload {
                    0
                } else {
                    value + 1
                }
            }
            Self::Decrement => {
                if value == 0 || value > payload {
                    payload
                } else {
                    value - 1
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_reductions_preserve_signedness_wrap_and_bounded_counter_rules() {
        for (control, value, payload, expected) in [
            (0x01000010, u32::MAX, 1, u32::MAX),
            (0x81000010, u32::MAX, 1, 1),
            (0x09000010, 0x80000000, 0, 0),
            (0x89000010, 0x80000000, 0, 0x80000000),
            (0x29000010, u32::MAX, 2, 1),
            (0xa9000010, u32::MAX, 2, 1),
            (0xb1000010, 2, 3, 3),
            (0xb1000010, 3, 3, 0),
            (0xb1000010, u32::MAX, u32::MAX, 0),
            (0xb1000010, u32::MAX - 1, u32::MAX, u32::MAX),
            (0xb9000010, 0, 3, 3),
            (0xb9000010, 4, 3, 3),
            (0xb9000010, 3, 3, 2),
            (0xb9000010, 0, 0, 0),
        ] {
            let operation = SemaphoreReduction::decode(control).unwrap();
            assert_eq!(operation.apply(value, payload), expected);
            for invalid in [control & !0x01000000, control | 0x100000, control | 0x1000] {
                assert_eq!(SemaphoreReduction::decode(invalid), None);
            }
        }
        for unsupported in [0x31000010, 0x39000010, 0xc1000010, 4] {
            assert_eq!(SemaphoreReduction::decode(unsupported), None);
        }
    }

    #[test]
    fn bitwise_reduction_control_and_results_preserve_exact_bits() {
        for (control, operation, expected) in [
            (0x11000010, BitwiseReduction::Xor, 0x0ff00ff0),
            (0x19000010, BitwiseReduction::And, 0xf000f000),
            (0x21000010, BitwiseReduction::Or, 0xfff0fff0),
        ] {
            for format in [0, 0x80000000] {
                assert_eq!(BitwiseReduction::decode(control | format), Some(operation));
            }
            assert_eq!(operation.apply(0xf0f0f0f0, 0xff00ff00), expected);
            for invalid in [
                control & !0x01000000,
                control | 0x00100000,
                control | 0x1000,
            ] {
                assert_eq!(BitwiseReduction::decode(invalid), None);
            }
        }
        assert_eq!(BitwiseReduction::decode(4), None);
    }
}

#[derive(Debug, Default)]
pub struct HostSemaphoreState {
    upper: u32,
    lower: u32,
    payload: u32,
}

impl HostSemaphoreState {
    pub fn handles(method: u16) -> bool {
        matches!(method, 4..=7)
    }

    /// Methods are word indices; NVIDIA's documented byte offsets are 0x10..0x1c.
    pub fn apply(&mut self, method: u16, argument: u32) -> Option<HostSemaphoreOperation> {
        match method {
            4 => self.upper = argument,
            5 => self.lower = argument,
            6 => self.payload = argument,
            7 => {
                return Some(HostSemaphoreOperation {
                    address: (u64::from(self.upper) << 32) | u64::from(self.lower),
                    payload: self.payload,
                    control: argument,
                    address_registers_valid: self.upper & !0xff == 0 && self.lower & 3 == 0,
                });
            }
            _ => {}
        }
        None
    }
}
