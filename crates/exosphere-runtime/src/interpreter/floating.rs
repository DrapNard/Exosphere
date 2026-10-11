// SPDX-License-Identifier: GPL-3.0-or-later
//! Baseline scalar floating-point execution. Remaining FP forms fail explicitly.

use super::{CpuError, condition_holds, read_register, write_register};
use exosphere_horizon::thread::ThreadContext;
mod add;
mod compare;
mod convert_precision;
mod divide;
mod multiply;
mod sqrt;
mod to_integer;

pub(super) fn execute(context: &mut ThreadContext, raw: u32) -> Result<bool, CpuError> {
    if raw & 0xff20_0c00 == 0x1e20_0c00 {
        let kind = (raw >> 22) & 3;
        if kind > 1 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let source = if condition_holds(context.pstate, ((raw >> 12) & 15) as u8) {
            ((raw >> 5) & 31) as usize
        } else {
            ((raw >> 16) & 31) as usize
        };
        let mask = if kind == 1 {
            u64::MAX as u128
        } else {
            u32::MAX as u128
        };
        // FCSEL selects raw bits: NaNs and denormals are not evaluated or quieted.
        // Sampling first also preserves source/destination aliasing semantics.
        context.v[(raw & 31) as usize] = context.v[source] & mask;
        return Ok(true);
    }
    if raw & 0xff3e_7c00 == 0x1e22_4000 {
        let source_kind = (raw >> 22) & 3;
        let destination_kind = (raw >> 15) & 3;
        if !matches!((source_kind, destination_kind), (0, 1) | (1, 0)) {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let source = context.v[((raw >> 5) & 31) as usize] as u64;
        let (bits, flags) = convert_precision::convert(source, source_kind == 1, context.fpcr);
        context.v[(raw & 31) as usize] = u128::from(bits);
        context.fpsr |= flags;
        return Ok(true);
    }
    if raw & 0xff20_0c00 == 0x1e20_0400 {
        let kind = (raw >> 22) & 3;
        if kind > 1 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let nzcv = if condition_holds(context.pstate, ((raw >> 12) & 15) as u8) {
            let left = context.v[((raw >> 5) & 31) as usize] as u64;
            let right = context.v[((raw >> 16) & 31) as usize] as u64;
            let (nzcv, flags) =
                compare::compare(left, right, kind == 1, raw & 16 != 0, context.fpcr);
            context.fpsr |= flags;
            nzcv
        } else {
            // A failed condition writes the immediate flags without evaluating
            // FP operands: even signaling NaNs must not raise a new exception.
            u64::from(raw & 15) << 28
        };
        context.pstate = (context.pstate & !0xf000_0000) | nzcv;
        return Ok(true);
    }
    let unary = raw & 0xff3f_fc00;
    if matches!(unary, 0x1e20_4000 | 0x1e20_c000 | 0x1e21_4000) {
        let kind = (raw >> 22) & 3;
        if kind > 1 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let width = if kind == 1 { 64 } else { 32 };
        let mask = (1u128 << width) - 1;
        let sign = 1u128 << (width - 1);
        let source = context.v[((raw >> 5) & 31) as usize] & mask;
        // Baseline FMOV/FABS/FNEG manipulate bits only: even signaling NaNs
        // and denormals retain their payloads, irrespective of FPCR.DN/FZ.
        let result = match unary {
            0x1e20_4000 => source,
            0x1e20_c000 => source & !sign,
            0x1e21_4000 => source ^ sign,
            _ => unreachable!(),
        };
        context.v[(raw & 31) as usize] = result;
        return Ok(true);
    }
    if raw & 0xff20_fc00 == 0x1e20_1800 {
        let kind = (raw >> 22) & 3;
        if kind > 1 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let left = context.v[((raw >> 5) & 31) as usize] as u64;
        let right = context.v[((raw >> 16) & 31) as usize] as u64;
        let (bits, flags) = divide::divide(left, right, kind == 1, context.fpcr);
        context.v[(raw & 31) as usize] = u128::from(bits);
        context.fpsr |= flags;
        return Ok(true);
    }
    if raw & 0xff20_ec00 == 0x1e20_2800 {
        let kind = (raw >> 22) & 3;
        if kind > 1 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let left = context.v[((raw >> 5) & 31) as usize] as u64;
        let right = context.v[((raw >> 16) & 31) as usize] as u64;
        let (bits, flags) = add::add(left, right, kind == 1, raw & (1 << 12) != 0, context.fpcr);
        context.v[(raw & 31) as usize] = u128::from(bits);
        context.fpsr |= flags;
        return Ok(true);
    }
    if raw & 0xff3f_fc00 == 0x1e21_c000 {
        let kind = (raw >> 22) & 3;
        if kind > 1 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let source = context.v[((raw >> 5) & 31) as usize] as u64;
        let (bits, flags) = sqrt::sqrt(source, kind == 1, context.fpcr);
        context.v[(raw & 31) as usize] = u128::from(bits);
        context.fpsr |= flags;
        return Ok(true);
    }
    if raw & 0x7f3e_fc00 == 0x1e38_0000 {
        let kind = (raw >> 22) & 3;
        if kind > 1 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let wide = raw & (1 << 31) != 0;
        let source = context.v[((raw >> 5) & 31) as usize] as u64;
        let (value, flags) =
            to_integer::convert(source, kind == 1, wide, raw & (1 << 16) != 0, context.fpcr);
        write_register(context, (raw & 31) as u8, value, false, wide);
        context.fpsr |= flags;
        return Ok(true);
    }
    if raw & 0xff20_fc07 == 0x1e20_2000 {
        let kind = (raw >> 22) & 3;
        let zero = raw & 8 != 0;
        let right_register = ((raw >> 16) & 31) as usize;
        if kind > 1 || (zero && right_register != 0) {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let left = context.v[((raw >> 5) & 31) as usize] as u64;
        let right = if zero {
            0
        } else {
            context.v[right_register] as u64
        };
        let (nzcv, flags) = compare::compare(left, right, kind == 1, raw & 16 != 0, context.fpcr);
        context.pstate = (context.pstate & !0xf000_0000) | nzcv;
        context.fpsr |= flags;
        return Ok(true);
    }
    if raw & 0xff20_fc00 == 0x1e20_0800 {
        let kind = (raw >> 22) & 3;
        if kind > 1 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let left = context.v[((raw >> 5) & 31) as usize] as u64;
        let right = context.v[((raw >> 16) & 31) as usize] as u64;
        let (bits, flags) = multiply::multiply(left, right, kind == 1, context.fpcr);
        context.v[(raw & 31) as usize] = u128::from(bits);
        context.fpsr |= flags;
        return Ok(true);
    }
    if raw & 0x7f3e_fc00 == 0x1e22_0000 {
        return integer_conversion(context, raw);
    }
    if raw & 0xff20_1fe0 != 0x1e20_1000 {
        return Ok(false);
    }
    let kind = (raw >> 22) & 3;
    if kind > 1 {
        return Err(CpuError::Unsupported {
            pc: context.pc,
            raw,
        });
    }
    let immediate = ((raw >> 13) & 255) as u8;
    // Scalar writes clear the remainder of the vector; no host float conversion
    // or guest FP exception/rounding operation is involved.
    context.v[(raw & 31) as usize] = u128::from(expand_immediate(immediate, kind == 1));
    Ok(true)
}

fn integer_conversion(context: &mut ThreadContext, raw: u32) -> Result<bool, CpuError> {
    let kind = (raw >> 22) & 3;
    if kind > 1 {
        return Err(CpuError::Unsupported {
            pc: context.pc,
            raw,
        });
    }
    let unsigned = raw & (1 << 16) != 0;
    let wide = raw & (1 << 31) != 0;
    let source = read_register(context, ((raw >> 5) & 31) as u8, false);
    let source = if wide {
        source
    } else {
        u64::from(source as u32)
    };
    let signed = if wide {
        source as i64
    } else {
        i64::from(source as i32)
    };
    let negative = !unsigned && signed < 0;
    let magnitude = if unsigned {
        source
    } else {
        signed.unsigned_abs()
    };
    let (bits, inexact) =
        integer_to_float(magnitude, negative, kind == 1, (context.fpcr >> 22) & 3);
    context.v[(raw & 31) as usize] = u128::from(bits);
    // The supported non-trapping profile accumulates IXC. Integer inputs cannot
    // overflow/underflow S/D or produce NaNs, so no other FP exception is raised.
    if inexact {
        context.fpsr |= 1 << 4;
    }
    Ok(true)
}

fn integer_to_float(magnitude: u64, negative: bool, double: bool, rounding: u32) -> (u64, bool) {
    if magnitude == 0 {
        return (0, false);
    }
    let (fraction_bits, bias, sign_bit) = if double {
        (52, 1023, 63)
    } else {
        (23, 127, 31)
    };
    let mut exponent = 63 - magnitude.leading_zeros();
    let mut inexact = false;
    let mut significand = if exponent <= fraction_bits {
        magnitude << (fraction_bits - exponent)
    } else {
        let discarded_bits = exponent - fraction_bits;
        let retained = magnitude >> discarded_bits;
        let remainder = magnitude & ((1u64 << discarded_bits) - 1);
        inexact = remainder != 0;
        let half = 1u64 << (discarded_bits - 1);
        let increment = match rounding {
            0 => remainder > half || (remainder == half && retained & 1 != 0),
            1 => inexact && !negative, // Towards +infinity.
            2 => inexact && negative,  // Towards -infinity.
            3 => false,                // Towards zero.
            _ => unreachable!(),
        };
        retained + u64::from(increment)
    };
    // Rounding can carry into the next exponent, e.g. u64::MAX -> 2^64.
    if significand == 1u64 << (fraction_bits + 1) {
        significand >>= 1;
        exponent += 1;
    }
    let bits = (u64::from(negative) << sign_bit)
        | (u64::from(exponent + bias) << fraction_bits)
        | (significand & ((1u64 << fraction_bits) - 1));
    (bits, inexact)
}

/// ARM VFPExpandImm for the baseline single/double formats.
pub(super) fn expand_immediate(immediate: u8, double: bool) -> u64 {
    let (exponent_bits, fraction_bits) = if double { (11, 52) } else { (8, 23) };
    let selector = u64::from((immediate >> 6) & 1);
    // Exponent = NOT(b6):Replicate(b6,E-3):b5:b4. The fraction
    // contains b3..b0 followed by zeros; b7 is the sign bit.
    let exponent = ((selector ^ 1) << (exponent_bits - 1))
        | ((selector * ((1 << (exponent_bits - 3)) - 1)) << 2)
        | u64::from((immediate >> 4) & 3);
    (u64::from(immediate >> 7) << (exponent_bits + fraction_bits))
        | (exponent << fraction_bits)
        | (u64::from(immediate & 15) << (fraction_bits - 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_compare_checks_all_conditions_and_fallback_flags() {
        for double in [false, true] {
            for signaling in [false, true] {
                for flags in 0..16 {
                    let n = flags & 8 != 0;
                    let z = flags & 4 != 0;
                    let c = flags & 2 != 0;
                    let v = flags & 1 != 0;
                    let outcomes = [
                        z,
                        !z,
                        c,
                        !c,
                        n,
                        !n,
                        v,
                        !v,
                        c && !z,
                        !c || z,
                        n == v,
                        n != v,
                        !z && n == v,
                        z || n != v,
                        true,
                        true,
                    ];
                    for (condition, passed) in outcomes.into_iter().enumerate() {
                        for fallback in 0..16 {
                            let mut context = ThreadContext::default();
                            context.v[1] = if double {
                                0x3ff0_0000_0000_0000
                            } else {
                                0x3f80_0000
                            };
                            context.pstate = (flags as u64) << 28 | 0x40;
                            context.fpsr = 0x10;
                            let raw = 0x1e22_0420
                                | (u32::from(double) << 22)
                                | (u32::from(signaling) << 4)
                                | (condition as u32) << 12
                                | fallback;
                            assert!(execute(&mut context, raw).unwrap());
                            assert_eq!(
                                context.pstate,
                                if passed {
                                    0x2000_0040
                                } else {
                                    (u64::from(fallback) << 28) | 0x40
                                }
                            );
                            assert_eq!(context.fpsr, 0x10);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn conditional_compare_accumulates_exceptions_only_when_condition_passes() {
        for double in [false, true] {
            let (snan, qnan) = if double {
                (0x7ff0_0000_0000_0001, 0x7ff8_0000_0000_0001)
            } else {
                (0x7f80_0001, 0x7fc0_0001)
            };
            for signaling in [false, true] {
                for (left, nzcv, flags) in [
                    (snan, 0x3000_0000, 1),
                    (qnan, 0x3000_0000, u32::from(signaling)),
                    (1, 0x6000_0000, 0x80),
                ] {
                    for passed in [false, true] {
                        let mut context = ThreadContext::default();
                        context.v[1] = left;
                        context.fpcr = 1 << 24; // FZ
                        context.fpsr = 0x10;
                        context.pstate = if passed { 1 << 30 } else { 0 };
                        let raw =
                            0x1e22_0429 | (u32::from(double) << 22) | (u32::from(signaling) << 4); // EQ
                        assert!(execute(&mut context, raw).unwrap());
                        assert_eq!(context.pstate, if passed { nzcv } else { 0x9000_0000 });
                        assert_eq!(context.fpsr, 0x10 | if passed { flags } else { 0 });
                        assert_eq!(context.v[1], left);
                    }
                }
            }
        }
    }

    #[test]
    fn nearest_integer_conversions_match_independent_host_oracle() {
        let mut sample = 0x1234_5678_9abc_def0u64;
        for value in [0, 1, u64::MAX, i64::MIN as u64, i64::MAX as u64]
            .into_iter()
            .chain((0..10000).map(|_| {
                sample = sample.wrapping_mul(6364136223846793005).wrapping_add(1);
                sample
            }))
        {
            for double in [false, true] {
                let unsigned_expected = if double {
                    (value as f64).to_bits()
                } else {
                    u64::from((value as f32).to_bits())
                };
                assert_eq!(
                    integer_to_float(value, false, double, 0).0,
                    unsigned_expected
                );
                let signed = value as i64;
                let signed_expected = if double {
                    (signed as f64).to_bits()
                } else {
                    u64::from((signed as f32).to_bits())
                };
                assert_eq!(
                    integer_to_float(signed.unsigned_abs(), signed < 0, double, 0).0,
                    signed_expected
                );
            }
        }
    }

    #[test]
    fn all_rounding_modes_handle_ties_sign_and_exponent_carry() {
        for double in [false, true] {
            let precision = if double { 53 } else { 24 };
            let base = 1u64 << precision;
            let low = integer_to_float(base, false, double, 0).0;
            let sign = 1u64 << if double { 63 } else { 31 };
            for (negative, increments) in [(false, [0, 1, 0, 0]), (true, [0, 0, 1, 0])] {
                for (rounding, increment) in increments.into_iter().enumerate() {
                    let (actual, inexact) =
                        integer_to_float(base + 1, negative, double, rounding as u32);
                    assert_eq!(actual, (low + increment) | if negative { sign } else { 0 });
                    assert!(inexact);
                }
            }
            // Odd retained significands round upward on an exact halfway tie.
            assert_eq!(integer_to_float(base + 3, false, double, 0).0, low + 2);
            let (rounded_max, inexact) = integer_to_float(u64::MAX, false, double, 0);
            let expected = if double {
                (2f64.powi(64)).to_bits()
            } else {
                u64::from((2f32.powi(64)).to_bits())
            };
            assert_eq!(rounded_max, expected);
            assert!(inexact);
            assert!(!integer_to_float(1u64 << 63, true, double, 0).1);
        }
    }

    #[test]
    fn conversion_encodings_cover_source_width_signedness_and_sticky_ixc() {
        let mut context = ThreadContext::default();
        context.x[8] = u64::MAX;
        context.pstate = 0xa000_0000;
        for (raw, expected) in [
            (0x9e23_0101, u64::from((u64::MAX as f32).to_bits())),
            (0x9e22_0101, u64::from((-1f32).to_bits())),
            (0x9e63_0101, (u64::MAX as f64).to_bits()),
            (0x9e62_0101, (-1f64).to_bits()),
            (0x1e23_0101, u64::from((u32::MAX as f32).to_bits())),
            (0x1e22_0101, u64::from((-1f32).to_bits())),
            (0x1e63_0101, (u32::MAX as f64).to_bits()),
            (0x1e62_0101, (-1f64).to_bits()),
        ] {
            context.v[1] = u128::MAX;
            execute(&mut context, raw).unwrap();
            assert_eq!(context.v[1], u128::from(expected));
        }
        assert_eq!(context.fpsr, 1 << 4);
        assert_eq!(context.pstate, 0xa000_0000);
        context.fpsr = 1;
        context.fpcr = 1 << 22; // Towards +infinity.
        context.x[8] = (1 << 24) + 1;
        execute(&mut context, 0x9e23_0101).unwrap();
        assert_eq!(context.v[1], u128::from(16777218f32.to_bits()));
        assert_eq!(context.fpsr, 0x11);
        context.sp = 0x8000;
        execute(&mut context, 0x9e23_03e1).unwrap(); // UCVTF S1,XZR
        assert_eq!(context.v[1], 0);
        assert_eq!(context.sp, 0x8000);
        assert_eq!(context.fpsr, 0x11);
        for raw in [0x9ea3_0101, 0x9ee3_0101] {
            assert!(execute(&mut context, raw).is_err());
            assert_eq!(context.v[1], 0);
            assert_eq!(context.fpsr, 0x11);
        }
    }

    #[test]
    fn all_immediates_match_independent_exact_numeric_reference() {
        // These constants are exact binary rationals with only five significant
        // bits. Host arithmetic is used only as an independent test oracle.
        for immediate in 0..=255u8 {
            let exponent = i32::from((immediate >> 4) & 7);
            let exponent = if exponent < 4 {
                exponent + 1
            } else {
                exponent - 7
            };
            let sign = if immediate & 128 == 0 { 1.0 } else { -1.0 };
            let expected = sign * (1.0 + f64::from(immediate & 15) / 16.0) * 2f64.powi(exponent);
            assert_eq!(expand_immediate(immediate, true), expected.to_bits());
            assert_eq!(
                expand_immediate(immediate, false),
                u64::from((expected as f32).to_bits())
            );
        }
    }

    #[test]
    fn scalar_immediates_clear_upper_bits_and_preserve_flags() {
        let mut context = ThreadContext::default();
        context.v[0] = u128::MAX;
        context.fpcr = 0x0700_0000;
        context.fpsr = 0x0800_009f;
        context.pstate = 0xa000_0000;
        execute(&mut context, 0x1e2c_1000).unwrap(); // FMOV S0,#0.5
        assert_eq!(context.v[0], u128::from(0.5f32.to_bits()));
        execute(&mut context, 0x1e6e_1000).unwrap(); // FMOV D0,#1.0
        assert_eq!(context.v[0], u128::from(1.0f64.to_bits()));
        assert_eq!(context.fpcr, 0x0700_0000);
        assert_eq!(context.fpsr, 0x0800_009f);
        assert_eq!(context.pstate, 0xa000_0000);
        let before = context.v[0];
        for raw in [0x1eac_1000, 0x1eec_1000] {
            assert!(execute(&mut context, raw).is_err());
            assert_eq!(context.v[0], before);
        }
    }
}
