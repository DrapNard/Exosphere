// SPDX-License-Identifier: GPL-3.0-or-later
//! FCVTZS/FCVTZU: truncation, saturation and cumulative exceptions.

pub(super) fn convert(
    bits: u64,
    double: bool,
    wide: bool,
    unsigned: bool,
    fpcr: u32,
) -> (u64, u32) {
    let (fraction_bits, bias, exponent_mask, sign_bit) = if double {
        (52, 1023, 2047, 63)
    } else {
        (23, 127, 255, 31)
    };
    let negative = bits & (1u64 << sign_bit) != 0;
    let fraction = bits & ((1u64 << fraction_bits) - 1);
    let exponent = (bits >> fraction_bits) & exponent_mask;
    let width = if wide { 64 } else { 32 };
    let maximum = if unsigned {
        u64::MAX >> (64 - width)
    } else {
        (1u64 << (width - 1)) - 1
    };
    let negative_limit = if unsigned { 0 } else { 1u64 << (width - 1) };
    let saturated = if negative {
        negative_limit.wrapping_neg()
    } else {
        maximum
    };
    if exponent == exponent_mask {
        // Both quiet and signaling NaNs convert to zero with IOC.
        return (if fraction != 0 { 0 } else { saturated }, 1);
    }
    if exponent == 0 && fraction == 0 {
        return (0, 0);
    }
    if exponent == 0 && fpcr & (1 << 24) != 0 {
        return (0, 1 << 7);
    }
    let power = exponent.max(1) as i32 - bias;
    if power > 63 {
        return (saturated, 1);
    }
    let significand = fraction
        | if exponent != 0 {
            1u64 << fraction_bits
        } else {
            0
        };
    let (magnitude, inexact) = if power < 0 {
        (0, true)
    } else if power >= fraction_bits {
        (significand << (power - fraction_bits), false)
    } else {
        let discarded = (fraction_bits - power) as u32;
        (
            significand >> discarded,
            significand & ((1u64 << discarded) - 1) != 0,
        )
    };
    let limit = if negative { negative_limit } else { maximum };
    if magnitude > limit {
        // Invalid conversion takes priority: do not also raise inexact.
        return (saturated, 1);
    }
    (
        if negative {
            magnitude.wrapping_neg()
        } else {
            magnitude
        },
        if inexact { 1 << 4 } else { 0 },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_limits_nan_and_flush_flags() {
        for double in [false, true] {
            let bits = |value: f64| {
                if double {
                    value.to_bits()
                } else {
                    u64::from((value as f32).to_bits())
                }
            };
            assert_eq!(convert(bits(1.75), double, true, true, 0), (1, 0x10));
            assert_eq!(convert(bits(-0.75), double, true, true, 0), (0, 0x10));
            assert_eq!(convert(bits(-1.75), double, true, true, 0), (0, 1));
            assert_eq!(
                convert(bits(-1.75), double, true, false, 0),
                (u64::MAX, 0x10)
            );
            assert_eq!(convert(bits(f64::NAN), double, true, true, 0), (0, 1));
            assert_eq!(
                convert(bits(f64::INFINITY), double, true, true, 0),
                (u64::MAX, 1)
            );
            assert_eq!(
                convert(bits(f64::NEG_INFINITY), double, true, false, 0),
                (1 << 63, 1)
            );
            assert_eq!(
                convert(bits(-2f64.powi(63)), double, true, false, 0),
                (1 << 63, 0)
            );
            assert_eq!(
                convert(bits(2f64.powi(64)), double, true, true, 0),
                (u64::MAX, 1)
            );
            assert_eq!(convert(1, double, true, true, 0), (0, 0x10));
            assert_eq!(convert(1, double, true, true, 1 << 24), (0, 0x80));
        }
    }

    #[test]
    fn deterministic_samples_match_saturating_host_casts() {
        let mut sample = 0x0123_4567_89ab_cdefu64;
        for _ in 0..10000 {
            sample = sample.wrapping_mul(6364136223846793005).wrapping_add(1);
            for double in [false, true] {
                let value = if double {
                    f64::from_bits(sample)
                } else {
                    f64::from(f32::from_bits(sample as u32))
                };
                for wide in [false, true] {
                    let mask = if wide { u64::MAX } else { u64::from(u32::MAX) };
                    let signed = if wide {
                        value as i64 as u64
                    } else {
                        value as i32 as u32 as u64
                    };
                    let unsigned = if wide {
                        value as u64
                    } else {
                        u64::from(value as u32)
                    };
                    assert_eq!(convert(sample, double, wide, false, 0).0 & mask, signed);
                    assert_eq!(convert(sample, double, wide, true, 0).0 & mask, unsigned);
                }
            }
        }
    }
}
