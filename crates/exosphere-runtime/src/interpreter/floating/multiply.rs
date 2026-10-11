// SPDX-License-Identifier: GPL-3.0-or-later
//! Integer-only ARMv8.0 S/D multiplication in the non-trapping FP profile.

const INVALID: u32 = 1;
const OVERFLOW: u32 = 1 << 2;
const UNDERFLOW: u32 = 1 << 3;
const INEXACT: u32 = 1 << 4;
const INPUT_DENORMAL: u32 = 1 << 7;

pub(super) fn multiply(left: u64, right: u64, double: bool, fpcr: u32) -> (u64, u32) {
    let left = if double { left } else { u64::from(left as u32) };
    let right = if double {
        right
    } else {
        u64::from(right as u32)
    };
    let (fraction, bias, exponent_mask) = if double {
        (52, 1023, 2047)
    } else {
        (23, 127, 255)
    };
    let sign_bit = if double { 63 } else { 31 };
    let fraction_mask = (1u64 << fraction) - 1;
    let infinity = exponent_mask << fraction;
    let quiet_bit = 1u64 << (fraction - 1);
    let default_nan = infinity | quiet_bit;
    let flush = fpcr & (1 << 24) != 0;
    let mut flags = 0;
    let unpack = |bits: u64, flags: &mut u32| {
        let exponent = (bits >> fraction) & exponent_mask;
        let mantissa = bits & fraction_mask;
        let denormal = exponent == 0 && mantissa != 0;
        if denormal && flush {
            *flags |= INPUT_DENORMAL;
        }
        let mantissa = if denormal && flush { 0 } else { mantissa };
        (exponent, mantissa)
    };
    let (left_exp, left_fraction) = unpack(left, &mut flags);
    let (right_exp, right_fraction) = unpack(right, &mut flags);
    let left_nan = left_exp == exponent_mask && left_fraction != 0;
    let right_nan = right_exp == exponent_mask && right_fraction != 0;
    let left_signaling = left_nan && left_fraction & quiet_bit == 0;
    let right_signaling = right_nan && right_fraction & quiet_bit == 0;
    if left_nan || right_nan {
        if left_signaling || right_signaling {
            flags |= INVALID;
        }
        // ARM prioritizes signaling NaNs, then quiet NaNs, in operand order.
        let selected = if left_signaling {
            left
        } else if right_signaling {
            right
        } else if left_nan {
            left
        } else {
            right
        };
        let result = if fpcr & (1 << 25) != 0 {
            default_nan
        } else {
            selected | quiet_bit
        };
        return (result, flags);
    }
    let negative = ((left ^ right) >> sign_bit) & 1 != 0;
    let sign = u64::from(negative) << sign_bit;
    let left_infinite = left_exp == exponent_mask;
    let right_infinite = right_exp == exponent_mask;
    let left_zero = left_exp == 0 && left_fraction == 0;
    let right_zero = right_exp == 0 && right_fraction == 0;
    if (left_infinite && right_zero) || (right_infinite && left_zero) {
        return (default_nan, flags | INVALID);
    }
    if left_infinite || right_infinite {
        return (sign | infinity, flags);
    }
    if left_zero || right_zero {
        return (sign, flags);
    }
    let finite = |exponent: u64, mantissa: u64| {
        let significand = mantissa | if exponent != 0 { 1 << fraction } else { 0 };
        let power = exponent.max(1) as i32 - bias - fraction;
        (u128::from(significand), power)
    };
    let (left_significand, left_power) = finite(left_exp, left_fraction);
    let (right_significand, right_power) = finite(right_exp, right_fraction);
    // A double product needs at most 106 bits, so this multiplication is exact.
    let product = left_significand * right_significand;
    let power = left_power + right_power;
    let leading = 127 - product.leading_zeros() as i32;
    let exponent = leading + power;
    let minimum = 1 - bias;
    if flush && exponent < minimum {
        return (sign, flags | UNDERFLOW);
    }
    let mut encoded_exponent = exponent.max(minimum);
    // Subnormal results round directly at their fixed quantum, without an
    // intermediate normal rounding (which would introduce double rounding).
    let shift = encoded_exponent - fraction - power;
    let rounding = (fpcr >> 22) & 3;
    let (mut significand, inexact) = round_significand(product, shift, negative, rounding);
    if significand >= 1u128 << (fraction + 1) {
        significand >>= 1;
        encoded_exponent += 1;
    }
    if encoded_exponent > bias {
        let to_infinity =
            rounding == 0 || (rounding == 1 && !negative) || (rounding == 2 && negative);
        return (
            sign | if to_infinity { infinity } else { infinity - 1 },
            flags | OVERFLOW | INEXACT,
        );
    }
    if inexact {
        flags |= INEXACT;
        // Baseline ARM detects tininess before rounding, including results
        // that round upward from the subnormal range to the minimum normal.
        if exponent < minimum {
            flags |= UNDERFLOW;
        }
    }
    let exponent_bits = if significand < 1u128 << fraction {
        0
    } else {
        (encoded_exponent + bias) as u64
    };
    (
        sign | (exponent_bits << fraction) | (significand as u64 & fraction_mask),
        flags,
    )
}

pub(super) fn round_significand(
    product: u128,
    shift: i32,
    negative: bool,
    mode: u32,
) -> (u128, bool) {
    if shift <= 0 {
        return (product << (-shift as u32), false);
    }
    let shift = shift as u32;
    let (retained, remainder) = if shift >= 128 {
        (0, product)
    } else {
        (product >> shift, product & ((1u128 << shift) - 1))
    };
    let inexact = remainder != 0;
    let increment = match mode {
        0 if shift <= 128 => {
            let half = 1u128 << (shift - 1);
            remainder > half || (remainder == half && retained & 1 != 0)
        }
        0 | 3 => false,
        1 => inexact && !negative,
        2 => inexact && negative,
        _ => unreachable!(),
    };
    (retained + u128::from(increment), inexact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_products_match_host_for_normal_subnormal_and_extreme_values() {
        let mut state = 0x0123_4567_89ab_cdefu64;
        for _ in 0..20000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let left = state;
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let right = state;
            for double in [false, true] {
                let (left, right) = if double {
                    (left, right)
                } else {
                    (u64::from(left as u32), u64::from(right as u32))
                };
                let expected = if double {
                    (f64::from_bits(left) * f64::from_bits(right)).to_bits()
                } else {
                    u64::from(
                        (f32::from_bits(left as u32) * f32::from_bits(right as u32)).to_bits(),
                    )
                };
                let (actual, _) = multiply(left, right, double, 0);
                let is_nan = if double {
                    f64::from_bits(expected).is_nan()
                } else {
                    f32::from_bits(expected as u32).is_nan()
                };
                if !is_nan {
                    assert_eq!(
                        actual, expected,
                        "left={left:x} right={right:x} double={double}"
                    );
                }
            }
        }
    }

    #[test]
    fn nan_priority_default_nan_invalid_and_signed_zero() {
        let quiet = 0x7fc0_1234;
        let signaling = 0xff80_0042;
        assert_eq!(multiply(quiet, signaling, false, 0), (0xffc0_0042, INVALID));
        assert_eq!(
            multiply(quiet, signaling, false, 1 << 25),
            (0x7fc0_0000, INVALID)
        );
        assert_eq!(multiply(quiet, 0x3f80_0000, false, 0), (quiet, 0));
        assert_eq!(
            multiply(quiet | 0xffff_ffff_0000_0000, 0x3f80_0000, false, 0),
            (quiet, 0)
        );
        assert_eq!(multiply(0x7f80_0000, 0, false, 0), (0x7fc0_0000, INVALID));
        assert_eq!(
            multiply(0x8000_0000, 0x3f80_0000, false, 0),
            (0x8000_0000, 0)
        );
        assert_eq!(
            multiply(0xff80_0000, 0xbf80_0000, false, 0),
            (0x7f80_0000, 0)
        );
    }

    #[test]
    fn subnormal_flush_and_underflow_follow_guest_policy() {
        assert_eq!(multiply(1, 0x3f80_0000, false, 0), (1, 0));
        assert_eq!(
            multiply(1, 0x3f80_0000, false, 1 << 24),
            (0, INPUT_DENORMAL)
        );
        assert_eq!(
            multiply(0x0080_0000, 0x3f00_0000, false, 0),
            (0x0040_0000, 0)
        );
        assert_eq!(
            multiply(0x0080_0000, 0x3f00_0000, false, 1 << 24),
            (0, UNDERFLOW)
        );
        assert_eq!(multiply(1, 0x3f00_0000, false, 0), (0, UNDERFLOW | INEXACT));
        assert_eq!(
            multiply(1, 0x3f00_0000, false, 1 << 22),
            (1, UNDERFLOW | INEXACT)
        );
        assert_eq!(
            multiply(0x0080_0000, 0x3f7f_ffff, false, 0),
            (0x0080_0000, UNDERFLOW | INEXACT)
        );
        // A double product far below the smallest subnormal exercises shifts >128.
        assert_eq!(multiply(1, 1, true, 0), (0, UNDERFLOW | INEXACT));
        assert_eq!(multiply(1, 1, true, 1 << 22), (1, UNDERFLOW | INEXACT));
    }

    #[test]
    fn directed_rounding_and_overflow_respect_sign() {
        for negative in [false, true] {
            let sign = if negative { 0x8000_0000 } else { 0 };
            for mode in 0..4 {
                let up = (mode == 1 && !negative) || (mode == 2 && negative);
                assert_eq!(
                    multiply(0x3f80_0001 | sign, 0x3f80_0001, false, mode << 22),
                    ((0x3f80_0002 + u64::from(up)) | sign, INEXACT)
                );
                let infinity = mode == 0 || up;
                assert_eq!(
                    multiply(0x7f7f_ffff | sign, 0x4000_0000, false, mode << 22),
                    (
                        (if infinity { 0x7f80_0000 } else { 0x7f7f_ffff }) | sign,
                        OVERFLOW | INEXACT
                    )
                );
            }
        }
    }
}
