// SPDX-License-Identifier: GPL-3.0-or-later
//! Baseline S/D addition/subtraction with integer guard, round and sticky bits.

pub(super) fn add(left: u64, right: u64, double: bool, subtract: bool, fpcr: u32) -> (u64, u32) {
    let (fraction_bits, bias, exponent_mask, sign_bit) = if double {
        (52, 1023, 2047, 63)
    } else {
        (23, 127, 255, 31)
    };
    let mask = if double {
        u64::MAX
    } else {
        u64::from(u32::MAX)
    };
    let sign_mask = 1u64 << sign_bit;
    let fraction_mask = (1u64 << fraction_bits) - 1;
    let infinity = exponent_mask << fraction_bits;
    let quiet = 1u64 << (fraction_bits - 1);
    let default_nan = infinity | quiet;
    let mut flags = 0;
    let mut prepare = |bits: u64| {
        let bits = bits & mask;
        let magnitude = bits & !sign_mask;
        if fpcr & (1 << 24) != 0 && magnitude != 0 && magnitude < 1 << fraction_bits {
            flags |= 0x80;
            bits & sign_mask
        } else {
            bits
        }
    };
    let left = prepare(left);
    let right = prepare(right);
    let left_nan = left & !sign_mask > infinity;
    let right_nan = right & !sign_mask > infinity;
    let left_signaling = left_nan && left & quiet == 0;
    let right_signaling = right_nan && right & quiet == 0;
    if left_nan || right_nan {
        if left_signaling || right_signaling {
            flags |= 1;
        }
        // Subtraction does not negate a propagated NaN's sign/payload.
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
    let right = right ^ if subtract { sign_mask } else { 0 };
    let left_negative = left & sign_mask != 0;
    let right_negative = right & sign_mask != 0;
    let left_magnitude = left & !sign_mask;
    let right_magnitude = right & !sign_mask;
    if left_magnitude == infinity || right_magnitude == infinity {
        if left_magnitude == infinity
            && right_magnitude == infinity
            && left_negative != right_negative
        {
            return (default_nan, flags | 1);
        }
        return (
            if left_magnitude == infinity {
                left
            } else {
                right
            },
            flags,
        );
    }
    let rounding = (fpcr >> 22) & 3;
    if left_magnitude == 0 && right_magnitude == 0 {
        let negative = if left_negative == right_negative {
            left_negative
        } else {
            rounding == 2
        };
        return (if negative { sign_mask } else { 0 }, flags);
    }
    let unpack = |magnitude: u64| {
        let exponent = magnitude >> fraction_bits;
        let significand =
            (magnitude & fraction_mask) | if exponent != 0 { 1 << fraction_bits } else { 0 };
        (exponent.max(1) as i32 - bias, u128::from(significand) << 3)
    };
    // Sort before subtraction so the extended result is nonnegative and its
    // sign comes from the larger magnitude, including severe cancellation.
    let (large, small, negative) = if left_magnitude >= right_magnitude {
        (left_magnitude, right_magnitude, left_negative)
    } else {
        (right_magnitude, left_magnitude, right_negative)
    };
    let (mut exponent, large) = unpack(large);
    let (small_exponent, small) = unpack(small);
    let small = shift_right_sticky(small, (exponent - small_exponent) as u32);
    let mut extended = if left_negative == right_negative {
        large + small
    } else {
        large - small
    };
    if extended == 0 {
        return (if rounding == 2 { sign_mask } else { 0 }, flags);
    }
    let target = fraction_bits + 3;
    let leading = 127 - extended.leading_zeros();
    if leading > target {
        extended = shift_right_sticky(extended, leading - target);
        exponent += (leading - target) as i32;
    } else {
        let shift = (target - leading).min((exponent - (1 - bias)) as u32);
        extended <<= shift;
        exponent -= shift as i32;
    }
    let tiny = extended < 1u128 << target;
    let sign = if negative { sign_mask } else { 0 };
    if tiny && fpcr & (1 << 24) != 0 {
        return (sign, flags | 8);
    }
    let remainder = extended & 7;
    let mut significand = extended >> 3;
    let increment = match rounding {
        0 => remainder > 4 || (remainder == 4 && significand & 1 != 0),
        1 => remainder != 0 && !negative,
        2 => remainder != 0 && negative,
        3 => false,
        _ => unreachable!(),
    };
    significand += u128::from(increment);
    if significand == 1u128 << (fraction_bits + 1) {
        significand >>= 1;
        exponent += 1;
    }
    if exponent > bias {
        let to_infinity =
            rounding == 0 || (rounding == 1 && !negative) || (rounding == 2 && negative);
        return (
            sign | if to_infinity { infinity } else { infinity - 1 },
            flags | 0x14,
        );
    }
    if remainder != 0 {
        flags |= 0x10 | if tiny { 8 } else { 0 };
    }
    let stored_exponent = if significand < 1u128 << fraction_bits {
        0
    } else {
        (exponent + bias) as u64
    };
    (
        sign | (stored_exponent << fraction_bits) | (significand as u64 & fraction_mask),
        flags,
    )
}

fn shift_right_sticky(value: u128, shift: u32) -> u128 {
    if shift == 0 {
        value
    } else if shift >= 128 {
        u128::from(value != 0)
    } else {
        (value >> shift) | u128::from(value & ((1u128 << shift) - 1) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_additions_and_subtractions_match_host() {
        let mut sample = 0x0123_4567_89ab_cdefu64;
        for _ in 0..20000 {
            sample = sample.wrapping_mul(6364136223846793005).wrapping_add(1);
            let left = sample;
            sample = sample.wrapping_mul(6364136223846793005).wrapping_add(1);
            let right = sample;
            for double in [false, true] {
                for subtract in [false, true] {
                    let expected = if double {
                        let (left, right) = (f64::from_bits(left), f64::from_bits(right));
                        (if subtract { left - right } else { left + right }).to_bits()
                    } else {
                        let (left, right) =
                            (f32::from_bits(left as u32), f32::from_bits(right as u32));
                        u64::from((if subtract { left - right } else { left + right }).to_bits())
                    };
                    let nan = if double {
                        f64::from_bits(expected).is_nan()
                    } else {
                        f32::from_bits(expected as u32).is_nan()
                    };
                    if !nan {
                        assert_eq!(
                            add(left, right, double, subtract, 0).0,
                            expected,
                            "{left:x} {right:x} double={double} subtract={subtract}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cancellation_ties_nan_and_flush_follow_guest_policy() {
        assert_eq!(add(0x3f80_0000, 0x3f80_0000, false, true, 0), (0, 0));
        assert_eq!(
            add(0x3f80_0000, 0x3f80_0000, false, true, 2 << 22),
            (0x8000_0000, 0)
        );
        assert_eq!(
            add(0x8000_0000, 0x8000_0000, false, false, 0),
            (0x8000_0000, 0)
        );
        assert_eq!(
            add(0x3f80_0000, 0x3380_0000, false, false, 0),
            (0x3f80_0000, 0x10)
        );
        assert_eq!(
            add(0x3f80_0000, 0x3380_0000, false, false, 1 << 22),
            (0x3f80_0001, 0x10)
        );
        assert_eq!(
            add(0x3f80_0001, 0x3380_0000, false, false, 0),
            (0x3f80_0002, 0x10)
        );
        assert_eq!(
            add(0x7f80_0000, 0xff80_0000, false, false, 0),
            (0x7fc0_0000, 1)
        );
        assert_eq!(
            add(0x3f80_0000, 0xffc0_0042, false, true, 0),
            (0xffc0_0042, 0)
        );
        assert_eq!(add(0x0080_0001, 0x0080_0000, false, true, 0), (1, 0));
        assert_eq!(add(0x0080_0001, 0x0080_0000, false, true, 1 << 24), (0, 8));
        assert_eq!(add(1, 0, false, false, 1 << 24), (0, 0x80));
    }
}
