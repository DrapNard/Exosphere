// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact integer-radicand square root for the baseline non-trapping S/D profile.

pub(super) fn sqrt(bits: u64, double: bool, fpcr: u32) -> (u64, u32) {
    let bits = if double { bits } else { u64::from(bits as u32) };
    let (fraction_bits, bias, exponent_mask, sign_bit) = if double {
        (52, 1023, 2047, 63)
    } else {
        (23, 127, 255, 31)
    };
    let sign = bits & (1u64 << sign_bit);
    let fraction_mask = (1u64 << fraction_bits) - 1;
    let fraction = bits & fraction_mask;
    let exponent = (bits >> fraction_bits) & exponent_mask;
    let quiet = 1u64 << (fraction_bits - 1);
    let default_nan = (exponent_mask << fraction_bits) | quiet;
    if exponent == exponent_mask && fraction != 0 {
        let flags = u32::from(fraction & quiet == 0);
        return (
            if fpcr & (1 << 25) != 0 {
                default_nan
            } else {
                bits | quiet
            },
            flags,
        );
    }
    if exponent == 0 {
        if fraction == 0 {
            return (sign, 0);
        } // Preserve -0.
        if fpcr & (1 << 24) != 0 {
            return (sign, 1 << 7);
        }
    }
    if sign != 0 {
        return (default_nan, 1);
    }
    if exponent == exponent_mask {
        return (bits, 0);
    } // +infinity.

    let significand = fraction
        | if exponent != 0 {
            1u64 << fraction_bits
        } else {
            0
        };
    let power = exponent.max(1) as i32 - bias - fraction_bits as i32;
    let leading = 63 - significand.leading_zeros() as i32;
    let mut result_exponent = (leading + power).div_euclid(2);
    // Scale so the integer root has F+1 significant bits. Even the largest
    // double radicand fits in 106 bits; there is no intermediate FP rounding.
    let shift = power + 2 * (fraction_bits as i32 - result_exponent);
    let radicand = u128::from(significand) << shift;
    let mut root = floor_root(radicand, fraction_bits);
    let remainder = radicand - root * root;
    let inexact = remainder != 0;
    let increment = match (fpcr >> 22) & 3 {
        // Compare R with (root+1/2)^2. Since R is an integer, equality
        // cannot occur: R-root^2 > root is exactly the nearest rounding test.
        0 => remainder > root,
        1 => inexact,
        2 | 3 => false,
        _ => unreachable!(),
    };
    root += u128::from(increment);
    if root == 1u128 << (fraction_bits + 1) {
        root >>= 1;
        result_exponent += 1;
    }
    // The square root of any positive finite S/D operand is normal and finite.
    let result = ((result_exponent + bias) as u64) << fraction_bits | (root as u64 & fraction_mask);
    (result, if inexact { 1 << 4 } else { 0 })
}

fn floor_root(radicand: u128, fraction_bits: u32) -> u128 {
    let mut low = 1u128 << fraction_bits;
    let mut high = low << 1;
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if middle * middle <= radicand {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_roots_match_host_including_subnormal_inputs() {
        let mut sample = 0x1234_5678_9abc_def0u64;
        for bits in [0, 1, 0x0010_0000_0000_0000, 0x7fef_ffff_ffff_ffff]
            .into_iter()
            .chain((0..10000).map(|_| {
                sample = sample.wrapping_mul(6364136223846793005).wrapping_add(1);
                sample
            }))
        {
            for double in [false, true] {
                let bits = if double {
                    bits & 0x7fff_ffff_ffff_ffff
                } else {
                    u64::from(bits as u32 & 0x7fff_ffff)
                };
                let expected = if double {
                    f64::from_bits(bits).sqrt().to_bits()
                } else {
                    u64::from(f32::from_bits(bits as u32).sqrt().to_bits())
                };
                let nan = if double {
                    f64::from_bits(bits).is_nan()
                } else {
                    f32::from_bits(bits as u32).is_nan()
                };
                if !nan {
                    assert_eq!(
                        sqrt(bits, double, 0).0,
                        expected,
                        "bits={bits:x} double={double}"
                    );
                }
            }
        }
    }

    #[test]
    fn roots_handle_nan_sign_zero_flush_and_exactness() {
        assert_eq!(sqrt(0x8000_0000, false, 0), (0x8000_0000, 0));
        assert_eq!(sqrt(0xbf80_0000, false, 0), (0x7fc0_0000, 1));
        assert_eq!(sqrt(0x7f80_0042, false, 0), (0x7fc0_0042, 1));
        assert_eq!(sqrt(0xffc0_0042, false, 0), (0xffc0_0042, 0));
        assert_eq!(sqrt(0xffc0_0042, false, 1 << 25), (0x7fc0_0000, 0));
        assert_eq!(sqrt(0x8000_0001, false, 1 << 24), (0x8000_0000, 0x80));
        assert_eq!(sqrt(0x4080_0000, false, 0), (0x4000_0000, 0));
        assert_eq!(sqrt(0x4000_0000, false, 0), (0x3fb5_04f3, 0x10));
        assert_eq!(sqrt(0x4000_0000, false, 1 << 22), (0x3fb5_04f4, 0x10));
        assert_eq!(
            sqrt(0x4000_0000_0000_0000, true, 0),
            (0x3ff6_a09e_667f_3bcd, 0x10)
        );
        for mode in [2, 3] {
            assert_eq!(
                sqrt(0x4000_0000_0000_0000, true, mode << 22),
                (0x3ff6_a09e_667f_3bcc, 0x10)
            );
        }
    }
}
