// SPDX-License-Identifier: GPL-3.0-or-later
//! Integer-only most-significant non-sign bit search, including empty words.

use super::{Builder, OP_CONSTANT};

pub(super) struct WordFind {
    uint: u32,
    types: [u32; 3],  // Boolean type, word constants 31 and 32.
    small: [u32; 3],  // Existing word constants 0, 1 and 2.
    spread: [u32; 3], // Word constants 4, 8 and 16.
}

impl WordFind {
    pub(super) fn new(b: &mut Builder, uint: u32, types: [u32; 3], small: [u32; 3]) -> Self {
        Self {
            uint,
            types,
            small,
            spread: [b.id(), b.id(), b.id()],
        }
    }

    pub(super) fn declare_constants(&self, b: &mut Builder) {
        for (id, value) in self.spread.into_iter().zip([4, 8, 16]) {
            b.inst(OP_CONSTANT, &[self.uint, id, value]);
        }
    }

    pub(super) fn emit(
        &self,
        b: &mut Builder,
        value: u32,
        signed: bool,
        invert: bool,
        shift_amount: bool,
    ) -> u32 {
        let [boolean, thirty_one, thirty_two] = self.types;
        let [zero, one, two] = self.small;
        let mut word = value;
        if invert {
            let inverted = b.id();
            b.inst(200, &[self.uint, inverted, word]); // OpNot
            word = inverted;
        }
        if signed {
            let sign = b.id();
            b.inst(195, &[self.uint, sign, word, thirty_one]); // OpShiftRightArithmetic
            let normalized = b.id();
            b.inst(198, &[self.uint, normalized, word, sign]); // OpBitwiseXor
            word = normalized;
        }
        // Fill all bits below the highest one. Counting that prefix gives its
        // index + 1, with no optional extended instruction set or float casts.
        // Every shift count is a fixed legal word-sized constant.
        for amount in [one, two, self.spread[0], self.spread[1], self.spread[2]] {
            let shifted = b.id();
            b.inst(194, &[self.uint, shifted, word, amount]);
            let filled = b.id();
            b.inst(197, &[self.uint, filled, word, shifted]);
            word = filled;
        }
        let count = b.id();
        b.inst(205, &[self.uint, count, word]); // OpBitCount
        let index = b.id();
        b.inst(130, &[self.uint, index, count, one]); // Empty: 0 - 1 = 0xffffffff.
        if !shift_amount {
            return index;
        }
        let amount = b.id();
        b.inst(130, &[self.uint, amount, thirty_two, count]);
        let empty = b.id();
        b.inst(170, &[boolean, empty, count, zero]);
        let result = b.id();
        // The empty sentinel is unchanged by shift-amount mode.
        b.inst(169, &[self.uint, result, empty, index, amount]);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_integer::evaluate;
    use super::*;

    #[test]
    fn emitted_find_checks_full_words_against_independent_search() {
        for signed in [false, true] {
            for invert in [false, true] {
                for shift_amount in [false, true] {
                    let mut b = Builder::new();
                    let uint = b.id();
                    b.inst(21, &[uint, 32, 0]);
                    let boolean = b.id();
                    b.inst(20, &[boolean]);
                    let ids = [b.id(), b.id(), b.id(), b.id(), b.id()];
                    for (id, value) in ids.into_iter().zip([0, 1, 2, 31, 32]) {
                        b.inst(OP_CONSTANT, &[uint, id, value]);
                    }
                    let emitter = WordFind::new(
                        &mut b,
                        uint,
                        [boolean, ids[3], ids[4]],
                        [ids[0], ids[1], ids[2]],
                    );
                    emitter.declare_constants(&mut b);
                    let source = b.id();
                    let result = emitter.emit(&mut b, source, signed, invert, shift_amount);
                    let module = b.finish();
                    let samples = [0u32, u32::MAX]
                        .into_iter()
                        .chain((0..32).flat_map(|bit| [1u32 << bit, !(1u32 << bit)]))
                        .chain((0..512u32).map(|n| n.wrapping_mul(2654435761)));
                    for input in samples {
                        let word = if invert { !input } else { input };
                        let negative = signed && word & 0x80000000 != 0;
                        let found = (0..32)
                            .rev()
                            .find(|bit| (word & (1 << bit) != 0) != negative);
                        let expected =
                            found.map_or(u32::MAX, |bit| if shift_amount { 31 - bit } else { bit });
                        assert_eq!(
                            evaluate(&module.words, source, input, result),
                            expected,
                            "{input:08x} signed={signed} invert={invert} shift={shift_amount}"
                        );
                    }
                }
            }
        }
    }
}
