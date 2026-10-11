// SPDX-License-Identifier: GPL-3.0-or-later
//! Scalar FP comparison without host arithmetic or host exception state.

pub(super) fn compare(
    left: u64,
    right: u64,
    double: bool,
    signal_all_nans: bool,
    fpcr: u32,
) -> (u64, u32) {
    let (sign_bit, fraction_bits, exponent_mask) = if double {
        (63, 52, 2047)
    } else {
        (31, 23, 255)
    };
    let sign_mask = 1u64 << sign_bit;
    let magnitude_mask = sign_mask - 1;
    let infinity = exponent_mask << fraction_bits;
    let quiet_bit = 1u64 << (fraction_bits - 1);
    let mut flags = 0;
    let mut prepare = |bits: u64| {
        let bits = bits & (sign_mask | magnitude_mask);
        let magnitude = bits & magnitude_mask;
        if fpcr & (1 << 24) != 0 && magnitude != 0 && magnitude < 1 << fraction_bits {
            flags |= 1 << 7; // FZ input-denormal exception accumulates IDC.
            bits & sign_mask
        } else {
            bits
        }
    };
    let left = prepare(left);
    let right = prepare(right);
    let left_magnitude = left & magnitude_mask;
    let right_magnitude = right & magnitude_mask;
    let left_nan = left_magnitude > infinity;
    let right_nan = right_magnitude > infinity;
    if left_nan || right_nan {
        if signal_all_nans
            || (left_nan && left & quiet_bit == 0)
            || (right_nan && right & quiet_bit == 0)
        {
            flags |= 1; // IOC: FCMP signals only sNaN; FCMPE signals any NaN.
        }
        return (0x3000_0000, flags); // Unordered: N=0,Z=0,C=1,V=1.
    }
    // Both signs of zero compare equal. Finite magnitudes and infinities have
    // monotonic encodings, with the order reversed for negative operands.
    if left == right || (left_magnitude == 0 && right_magnitude == 0) {
        return (0x6000_0000, flags);
    }
    let left_negative = left & sign_mask != 0;
    let right_negative = right & sign_mask != 0;
    let less = if left_negative != right_negative {
        left_negative
    } else if left_negative {
        left_magnitude > right_magnitude
    } else {
        left_magnitude < right_magnitude
    };
    (if less { 0x8000_0000 } else { 0x2000_0000 }, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_comparisons_match_independent_host_oracle() {
        let values = [
            f64::NEG_INFINITY,
            -100.0,
            -1.0,
            -0.0,
            0.0,
            f64::from_bits(1),
            1.0,
            100.0,
            f64::INFINITY,
        ];
        for left in values {
            for right in values {
                for double in [false, true] {
                    let (left, right) = if double {
                        (left, right)
                    } else {
                        (f64::from(left as f32), f64::from(right as f32))
                    };
                    let bits = |value: f64| {
                        if double {
                            value.to_bits()
                        } else {
                            u64::from((value as f32).to_bits())
                        }
                    };
                    let expected = if left == right {
                        0x6000_0000
                    } else if left < right {
                        0x8000_0000
                    } else {
                        0x2000_0000
                    };
                    assert_eq!(
                        compare(bits(left), bits(right), double, false, 0),
                        (expected, 0)
                    );
                }
            }
        }
    }

    #[test]
    fn nan_signaling_and_flush_policy_are_explicit() {
        for (nan, signaling) in [(0x7fc0_1234, false), (0xff80_0042, true)] {
            for signal_all in [false, true] {
                let expected = (0x3000_0000, u32::from(signaling || signal_all));
                assert_eq!(compare(nan, 0, false, signal_all, 0), expected);
                assert_eq!(compare(0, nan, false, signal_all, 0), expected);
            }
        }
        assert_eq!(compare(1, 0, false, false, 0), (0x2000_0000, 0));
        assert_eq!(compare(1, 0, false, false, 1 << 24), (0x6000_0000, 0x80));
        assert_eq!(
            compare(0x8000_0001, 0, false, false, 1 << 24),
            (0x6000_0000, 0x80)
        );
        assert_eq!(
            compare(0xffff_ffff_7fc0_1234, 0, false, false, 0),
            (0x3000_0000, 0)
        );
    }
}
