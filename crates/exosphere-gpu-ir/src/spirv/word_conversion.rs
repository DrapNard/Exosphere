// SPDX-License-Identifier: GPL-3.0-or-later
//! Integer-only I32/U32-to-F32 emission. No optional float-control capability
//! is needed: rounding is applied to the significand before constructing bits.

use super::Builder;
use crate::shader::{ConversionControl, RoundingMode, ScalarType};

pub(super) struct WordConversion {
    uint: u32,
    boolean: u32,
    constants: [u32; 10],
}

impl WordConversion {
    pub(super) fn declare(b: &mut Builder, uint: u32) -> Self {
        let boolean = b.id();
        b.inst(20, &[boolean]); // OpTypeBool
        let constants = [0, 1, 2, 4, 8, 16, 23, 127, 0x80000000, 0x800000].map(|value| {
            let id = b.id();
            b.inst(43, &[uint, id, value]); // OpConstant
            id
        });
        Self {
            uint,
            boolean,
            constants,
        }
    }

    fn binary(&self, b: &mut Builder, opcode: u16, boolean: bool, a: u32, c: u32) -> u32 {
        let id = b.id();
        b.inst(
            opcode,
            &[if boolean { self.boolean } else { self.uint }, id, a, c],
        );
        id
    }

    fn select(&self, b: &mut Builder, condition: u32, yes: u32, no: u32) -> u32 {
        let id = b.id();
        b.inst(169, &[self.uint, id, condition, yes, no]); // OpSelect
        id
    }

    pub(super) fn emit(&self, b: &mut Builder, source: u32, control: ConversionControl) -> u32 {
        debug_assert!(control.is_plain_word_to_f32());
        let [
            zero,
            one,
            two,
            four,
            eight,
            sixteen,
            twenty_three,
            bias,
            sign_mask,
            hidden_bit,
        ] = self.constants;
        let sign = if control.src == ScalarType::I32 {
            self.binary(b, 199, false, source, sign_mask) // OpBitwiseAnd
        } else {
            zero
        };
        let negative = self.binary(b, 171, true, sign, zero); // OpINotEqual
        let negated = self.binary(b, 130, false, zero, source); // OpISub wraps, including INT_MIN.
        let magnitude = self.select(b, negative, negated, source);
        let is_zero = self.binary(b, 170, true, magnitude, zero);

        // Smearing all bits below the highest set bit makes popcount equal to
        // floor(log2(magnitude))+1 without extensions or float instructions.
        let mut smeared = magnitude;
        for shift in [one, two, four, eight, sixteen] {
            let shifted = self.binary(b, 194, false, smeared, shift); // OpShiftRightLogical
            smeared = self.binary(b, 197, false, smeared, shifted); // OpBitwiseOr
        }
        let count = b.id();
        b.inst(205, &[self.uint, count, smeared]); // OpBitCount
        let highest = self.binary(b, 130, false, count, one);
        let exponent = self.select(b, is_zero, zero, highest);
        let needs_rounding = self.binary(b, 172, true, exponent, twenty_three); // OpUGreaterThan
        let right_unclamped = self.binary(b, 130, false, exponent, twenty_three);
        let right = self.select(b, needs_rounding, right_unclamped, zero);
        let left_unclamped = self.binary(b, 130, false, twenty_three, exponent);
        let left = self.select(b, needs_rounding, zero, left_unclamped);
        let truncated = self.binary(b, 194, false, magnitude, right);
        let unit = self.binary(b, 196, false, one, right); // OpShiftLeftLogical
        let remainder_mask = self.binary(b, 130, false, unit, one);
        let remainder = self.binary(b, 199, false, magnitude, remainder_mask);
        let inexact = self.binary(b, 171, true, remainder, zero);
        let increment = match control.rounding {
            RoundingMode::NearestEven => {
                // Clamp before emitting the shift: OpSelect does not prevent
                // evaluation of an out-of-range shift in its unselected arm.
                let half_shift = self.binary(b, 130, false, right, one);
                let half_shift = self.select(b, needs_rounding, half_shift, zero);
                let halfway = self.binary(b, 196, false, one, half_shift);
                let above = self.binary(b, 172, true, remainder, halfway);
                let tie = self.binary(b, 170, true, remainder, halfway);
                let low_bit = self.binary(b, 199, false, truncated, one);
                let odd = self.binary(b, 171, true, low_bit, zero);
                let tie_odd = self.binary(b, 167, true, tie, odd); // OpLogicalAnd
                let round_up = self.binary(b, 166, true, above, tie_odd); // OpLogicalOr
                self.select(b, round_up, one, zero)
            }
            RoundingMode::TowardZero => zero,
            RoundingMode::TowardPositive => {
                let nonnegative = self.binary(b, 170, true, sign, zero);
                let round_up = self.binary(b, 167, true, inexact, nonnegative);
                self.select(b, round_up, one, zero)
            }
            RoundingMode::TowardNegative => {
                let round_up = self.binary(b, 167, true, inexact, negative);
                self.select(b, round_up, one, zero)
            }
            RoundingMode::None => unreachable!(),
        };
        let rounded = self.binary(b, 128, false, truncated, increment); // OpIAdd
        let significand = self.binary(b, 196, false, rounded, left);
        let exponent_bias = self.binary(b, 128, false, exponent, bias);
        let exponent_bits = self.binary(b, 196, false, exponent_bias, twenty_three);
        let encoded = self.binary(b, 128, false, exponent_bits, significand);
        let encoded = self.binary(b, 130, false, encoded, hidden_bit);
        let encoded = self.select(b, is_zero, zero, encoded);
        self.binary(b, 197, false, encoded, sign)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_integer::evaluate;
    use super::*;

    #[test]
    fn emitted_integer_conversion_matches_independent_float_oracle() {
        for src in [ScalarType::U32, ScalarType::I32] {
            for rounding in [
                RoundingMode::NearestEven,
                RoundingMode::TowardZero,
                RoundingMode::TowardPositive,
                RoundingMode::TowardNegative,
            ] {
                let mut b = Builder::new();
                let uint = b.id();
                b.inst(21, &[uint, 32, 0]);
                let source = b.id();
                let emitter = WordConversion::declare(&mut b, uint);
                let control = ConversionControl {
                    src,
                    dst: ScalarType::F32,
                    rounding,
                    saturate: false,
                    flush_to_zero: false,
                    absolute: false,
                    negate: false,
                    raw_flags: 0,
                };
                let result = emitter.emit(&mut b, source, control);
                let module = b.finish();
                let samples = [
                    0,
                    1,
                    2,
                    16777215,
                    16777216,
                    16777217,
                    16777219,
                    0x7fffffff,
                    0x80000000,
                    0x80000001,
                    u32::MAX,
                ]
                .into_iter()
                .chain((0..512u32).map(|n| n.wrapping_mul(2654435761)));
                for input in samples {
                    let exact = if src == ScalarType::I32 {
                        f64::from(input as i32)
                    } else {
                        f64::from(input)
                    };
                    let nearest = exact as f32;
                    let expected = match rounding {
                        RoundingMode::TowardPositive if f64::from(nearest) < exact => {
                            nearest.next_up()
                        }
                        RoundingMode::TowardNegative if f64::from(nearest) > exact => {
                            nearest.next_down()
                        }
                        RoundingMode::TowardZero if f64::from(nearest).abs() > exact.abs() => {
                            if exact < 0.0 {
                                nearest.next_up()
                            } else {
                                nearest.next_down()
                            }
                        }
                        _ => nearest,
                    };
                    assert_eq!(
                        evaluate(&module.words, source, input, result),
                        expected.to_bits(),
                        "input {input:08x}, {src:?}, {rounding:?}"
                    );
                }
            }
        }
    }
}
