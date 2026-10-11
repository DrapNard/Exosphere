// SPDX-License-Identifier: GPL-3.0-or-later
//! Integer-only scalar S/D division for the non-trapping ARMv8.0 profile.

const INVALID: u32 = 1;
const DIVIDE_BY_ZERO: u32 = 1 << 1;
const OVERFLOW: u32 = 1 << 2;
const UNDERFLOW: u32 = 1 << 3;
const INEXACT: u32 = 1 << 4;
const INPUT_DENORMAL: u32 = 1 << 7;

pub(super) fn divide(left: u64, right: u64, double: bool, fpcr: u32) -> (u64, u32) {
    let (fraction, bias, sign_bit) = if double {
        (52, 1023, 63)
    } else {
        (23, 127, 31)
    };
    let sign_mask = 1u64 << sign_bit;
    let fraction_mask = (1u64 << fraction) - 1;
    let infinity = ((2 * bias + 1) as u64) << fraction;
    let quiet = 1u64 << (fraction - 1);
    let default_nan = infinity | quiet;
    let mut flags = 0;
    let mut prepare = |bits: u64| {
        let bits = if double { bits } else { u64::from(bits as u32) };
        let magnitude = bits & !sign_mask;
        if fpcr & (1 << 24) != 0 && magnitude != 0 && magnitude < 1 << fraction {
            flags |= INPUT_DENORMAL;
            bits & sign_mask
        } else {
            bits
        }
    };
    let left = prepare(left);
    let right = prepare(right);
    let left_magnitude = left & !sign_mask;
    let right_magnitude = right & !sign_mask;
    let left_nan = left_magnitude > infinity;
    let right_nan = right_magnitude > infinity;
    let left_signaling = left_nan && left & quiet == 0;
    let right_signaling = right_nan && right & quiet == 0;
    if left_nan || right_nan {
        if left_signaling || right_signaling {
            flags |= INVALID;
        }
        // Signaling operands take priority, then quiet operands in operand order.
        let chosen = if left_signaling {
            left
        } else if right_signaling {
            right
        } else if left_nan {
            left
        } else {
            right
        };
        return (
            if fpcr & (1 << 25) != 0 {
                default_nan
            } else {
                chosen | quiet
            },
            flags,
        );
    }
    let sign = (left ^ right) & sign_mask;
    if (left_magnitude == 0 && right_magnitude == 0)
        || (left_magnitude == infinity && right_magnitude == infinity)
    {
        return (default_nan, flags | INVALID);
    }
    // Infinity / zero is infinity without DZC; only finite nonzero / zero raises it.
    if left_magnitude == infinity {
        return (sign | infinity, flags);
    }
    if right_magnitude == infinity || left_magnitude == 0 {
        return (sign, flags);
    }
    if right_magnitude == 0 {
        return (sign | infinity, flags | DIVIDE_BY_ZERO);
    }
    let normalize = |magnitude: u64| {
        let stored = magnitude >> fraction;
        let significand = (magnitude & fraction_mask) | if stored != 0 { 1 << fraction } else { 0 };
        let shift = fraction - (63 - significand.leading_zeros());
        (
            u128::from(significand) << shift,
            stored.max(1) as i32 - bias - shift as i32,
        )
    };
    let (numerator, left_exponent) = normalize(left_magnitude);
    let (denominator, right_exponent) = normalize(right_magnitude);
    let extra = u32::from(numerator < denominator);
    let mut exponent = left_exponent - right_exponent - extra as i32;
    let tiny = exponent < 1 - bias;
    if tiny && fpcr & (1 << 24) != 0 {
        return (sign, flags | UNDERFLOW);
    }
    // At most 109 bits: division retains three rounding bits and jams any
    // rational remainder into the sticky bit, with no host FP environment.
    let scaled = numerator << (fraction + 3 + extra);
    let mut extended = (scaled / denominator) | u128::from(scaled % denominator != 0);
    if tiny {
        extended = shift_right_sticky(extended, (1 - bias - exponent) as u32);
        exponent = 1 - bias;
    }
    let remainder = extended & 7;
    let mut significand = extended >> 3;
    let mode = (fpcr >> 22) & 3;
    let negative = sign != 0;
    let increment = match mode {
        0 => remainder > 4 || (remainder == 4 && significand & 1 != 0),
        1 => remainder != 0 && !negative,
        2 => remainder != 0 && negative,
        3 => false,
        _ => unreachable!(),
    };
    significand += u128::from(increment);
    if significand == 1u128 << (fraction + 1) {
        significand >>= 1;
        exponent += 1;
    }
    if exponent > bias {
        let to_infinity = mode == 0 || (mode == 1 && !negative) || (mode == 2 && negative);
        return (
            sign | if to_infinity { infinity } else { infinity - 1 },
            flags | OVERFLOW | INEXACT,
        );
    }
    if remainder != 0 {
        flags |= INEXACT | if tiny { UNDERFLOW } else { 0 };
    }
    let stored = if significand < 1u128 << fraction {
        0
    } else {
        (exponent + bias) as u64
    };
    (
        sign | (stored << fraction) | (significand as u64 & fraction_mask),
        flags,
    )
}

fn shift_right_sticky(value: u128, shift: u32) -> u128 {
    if shift >= 128 {
        u128::from(value != 0)
    } else {
        (value >> shift) | u128::from(value & ((1u128 << shift) - 1) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_matches_host_across_precisions() {
        let mut sample = 0x0123_4567_89ab_cdefu64;
        for _ in 0..20000 {
            sample = sample.wrapping_mul(6364136223846793005).wrapping_add(1);
            let left = sample;
            sample = sample.wrapping_mul(6364136223846793005).wrapping_add(1);
            let right = sample;
            for double in [false, true] {
                let expected = if double {
                    (f64::from_bits(left) / f64::from_bits(right)).to_bits()
                } else {
                    u64::from(
                        (f32::from_bits(left as u32) / f32::from_bits(right as u32)).to_bits(),
                    )
                };
                let nan = if double {
                    f64::from_bits(expected).is_nan()
                } else {
                    f32::from_bits(expected as u32).is_nan()
                };
                if !nan {
                    assert_eq!(
                        divide(left, right, double, 0).0,
                        expected,
                        "{left:x}/{right:x}, double={double}"
                    );
                }
            }
        }
    }

    #[test]
    fn exceptions_rounding_and_flush_are_guest_controlled() {
        assert_eq!(divide(0, 0, false, 0), (0x7fc0_0000, 1));
        assert_eq!(divide(0x7f80_0000, 0x7f80_0000, false, 0), (0x7fc0_0000, 1));
        assert_eq!(divide(0x3f80_0000, 0x8000_0000, false, 0), (0xff80_0000, 2));
        assert_eq!(divide(0x7f80_0000, 0, false, 0), (0x7f80_0000, 0));
        assert_eq!(divide(0x7fc0_1234, 0xff80_0042, false, 0), (0xffc0_0042, 1));
        assert_eq!(
            divide(0x7fc0_1234, 0xff80_0042, false, 1 << 25),
            (0x7fc0_0000, 1)
        );
        assert_eq!(divide(1, 0x3f80_0000, false, 0), (1, 0));
        assert_eq!(divide(1, 0x3f80_0000, false, 1 << 24), (0, 0x80));
        assert_eq!(divide(0x0080_0000, 0x4000_0000, false, 0), (0x0040_0000, 0));
        assert_eq!(divide(0x0080_0000, 0x4000_0000, false, 1 << 24), (0, 8));
        assert_eq!(divide(1, 0x4000_0000, false, 0), (0, 0x18));
        assert_eq!(divide(1, 0x4000_0000, false, 1 << 22), (1, 0x18));
        for negative in [false, true] {
            let sign = if negative { 0x8000_0000 } else { 0 };
            for mode in 0..4 {
                let upward = (mode == 1 && !negative) || (mode == 2 && negative);
                let third = if mode == 0 || upward {
                    0x3eaa_aaab
                } else {
                    0x3eaa_aaaa
                };
                assert_eq!(
                    divide(sign | 0x3f80_0000, 0x4040_0000, false, mode << 22),
                    (sign | third, 0x10)
                );
                let infinity = mode == 0 || upward;
                assert_eq!(
                    divide(sign | 0x7f7f_ffff, 0x3f00_0000, false, mode << 22),
                    (
                        sign | if infinity { 0x7f80_0000 } else { 0x7f7f_ffff },
                        0x14
                    )
                );
            }
        }
        assert_eq!(divide(1, 0x7fef_ffff_ffff_ffff, true, 1 << 22), (1, 0x18));
    }
}
