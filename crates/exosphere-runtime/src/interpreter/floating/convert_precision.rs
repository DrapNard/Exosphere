// SPDX-License-Identifier: GPL-3.0-or-later
//! Scalar S/D precision conversion with one integer-only rounding step.

use super::multiply::round_significand;

pub(super) fn convert(bits: u64, source_double: bool, fpcr: u32) -> (u64, u32) {
    let (source_fraction, source_bias, source_exponents, source_sign) = if source_double {
        (52, 1023, 2047, 63)
    } else {
        (23, 127, 255, 31)
    };
    let (fraction, bias, exponents, sign_bit) = if source_double {
        (23, 127, 255, 31)
    } else {
        (52, 1023, 2047, 63)
    };
    let negative = bits & (1u64 << source_sign) != 0;
    let sign = u64::from(negative) << sign_bit;
    let source_mask = (1u64 << source_fraction) - 1;
    let mantissa = bits & source_mask;
    let source_exponent = (bits >> source_fraction) & source_exponents;
    let infinity = exponents << fraction;
    if source_exponent == source_exponents {
        if mantissa == 0 {
            return (sign | infinity, 0);
        }
        let flags = u32::from(mantissa & (1 << (source_fraction - 1)) == 0);
        let quiet = 1u64 << (fraction - 1);
        if fpcr & (1 << 25) != 0 {
            return (infinity | quiet, flags);
        }
        let payload = if source_double {
            mantissa >> 29
        } else {
            mantissa << 29
        };
        return (sign | infinity | payload | quiet, flags);
    }
    if source_exponent == 0 {
        if mantissa == 0 {
            return (sign, 0);
        }
        if fpcr & (1 << 24) != 0 {
            return (sign, 0x80);
        } // FZ input IDC
    }
    let significand = mantissa
        | if source_exponent == 0 {
            0
        } else {
            1 << source_fraction
        };
    let power = source_exponent.max(1) as i32 - source_bias - source_fraction;
    let leading = 63 - significand.leading_zeros() as i32;
    let exponent = leading + power;
    let minimum = 1 - bias;
    if fpcr & (1 << 24) != 0 && exponent < minimum {
        return (sign, 8);
    }
    let mut encoded_exponent = exponent.max(minimum);
    // Subnormals round at their final quantum, avoiding double rounding.
    let shift = encoded_exponent - fraction - power;
    let rounding = (fpcr >> 22) & 3;
    let (mut result, inexact) =
        round_significand(u128::from(significand), shift, negative, rounding);
    if result >= 1u128 << (fraction + 1) {
        result >>= 1;
        encoded_exponent += 1;
    }
    if encoded_exponent > bias {
        let to_infinity =
            rounding == 0 || (rounding == 1 && !negative) || (rounding == 2 && negative);
        return (
            sign | if to_infinity { infinity } else { infinity - 1 },
            0x14,
        );
    }
    let flags = if inexact {
        0x10 | if exponent < minimum { 8 } else { 0 }
    } else {
        0
    };
    let exponent_bits = if result < 1u128 << fraction {
        0
    } else {
        (encoded_exponent + bias) as u64
    };
    (
        sign | (exponent_bits << fraction) | (result as u64 & ((1 << fraction) - 1)),
        flags,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_conversions_match_host_for_normal_subnormal_and_extreme_values() {
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..20000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let double = f64::from_bits(state);
            if !double.is_nan() {
                assert_eq!(
                    convert(state, true, 0).0,
                    u64::from((double as f32).to_bits())
                );
            }
            let single = f32::from_bits(state as u32);
            if !single.is_nan() {
                let (bits, flags) = convert(u64::from(single.to_bits()), false, 0);
                assert_eq!(bits, f64::from(single).to_bits());
                assert_eq!(flags, 0);
            }
        }
    }

    #[test]
    fn rounding_underflow_overflow_and_flush_follow_guest_control() {
        let halfway = 1.0 + 2f64.powi(-24);
        for (mode, expected) in [
            (0, 0x3f800000),
            (1, 0x3f800001),
            (2, 0x3f800000),
            (3, 0x3f800000),
        ] {
            assert_eq!(
                convert(halfway.to_bits(), true, mode << 22),
                (expected, 0x10)
            );
        }
        for (mode, expected) in [
            (0, 0xbf800000),
            (1, 0xbf800000),
            (2, 0xbf800001),
            (3, 0xbf800000),
        ] {
            assert_eq!(
                convert((-halfway).to_bits(), true, mode << 22),
                (expected, 0x10)
            );
        }
        assert_eq!(convert(2f64.powi(-149).to_bits(), true, 0), (1, 0));
        assert_eq!(convert(2f64.powi(-150).to_bits(), true, 0), (0, 0x18));
        assert_eq!(convert(2f64.powi(-150).to_bits(), true, 1 << 22), (1, 0x18));
        assert_eq!(convert(2f64.powi(-149).to_bits(), true, 1 << 24), (0, 8));
        assert_eq!(convert(1, false, 1 << 24), (0, 0x80));
        assert_eq!(convert(f64::MAX.to_bits(), true, 0), (0x7f800000, 0x14));
        assert_eq!(
            convert(f64::MAX.to_bits(), true, 3 << 22),
            (0x7f7fffff, 0x14)
        );
    }

    #[test]
    fn nan_payload_sign_quieting_and_default_nan_are_preserved() {
        assert_eq!(convert(0xfff0000000000001, true, 0), (0xffc00000, 1));
        assert_eq!(convert(0x7fc12345, false, 0), (0x7ff82468a0000000, 0));
        assert_eq!(convert(0xff800001, false, 1 << 25), (0x7ff8000000000000, 1));
        assert_eq!(convert(0x80000000, false, 0), (0x8000000000000000, 0));
    }
}
