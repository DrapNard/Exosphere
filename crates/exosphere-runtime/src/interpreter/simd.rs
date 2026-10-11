// SPDX-License-Identifier: GPL-3.0-or-later
//! SIMD bit operations and immediate moves. FP arithmetic remains unsupported.

use super::{CpuError, read_register, write_register};
use exosphere_horizon::thread::ThreadContext;

pub(super) fn execute(context: &mut ThreadContext, raw: u32) -> Result<bool, CpuError> {
    // CNT counts set bits independently in each byte, for either 8B or 16B.
    // Snapshot the source first so in-place operations have identical semantics.
    if raw & 0xbfff_fc00 == 0x0e20_5800 {
        let source = context.v[((raw >> 5) & 31) as usize].to_le_bytes();
        let lanes = if raw & (1 << 30) != 0 { 16 } else { 8 };
        let mut result = [0; 16];
        for (output, input) in result.iter_mut().zip(source).take(lanes) {
            *output = input.count_ones() as u8;
        }
        // Narrow vector writes clear the unused upper half.
        context.v[(raw & 31) as usize] = u128::from_le_bytes(result);
        return Ok(true);
    }
    if raw & 0xbf3f_fc00 == 0x2e30_3800 {
        let size = (raw >> 22) & 3;
        let wide = raw & (1 << 30) != 0;
        // UADDLV supports 8/16-bit lanes and the 128-bit, four-word form.
        // Validate reserved arrangements before modifying any architectural state.
        if size == 3 || size == 2 && !wide {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let bits = 8 << size;
        let lanes = if wide { 128 } else { 64 } / bits;
        let mask = (1u128 << bits) - 1;
        let source = context.v[((raw >> 5) & 31) as usize];
        let sum = (0..lanes)
            .map(|lane| (source >> (lane * bits)) & mask)
            .sum();
        // The scalar result is twice the element width. Even the largest sum
        // fits in 64 bits; all remaining destination bits are cleared.
        context.v[(raw & 31) as usize] = sum;
        return Ok(true);
    }
    if raw & 0x9f20_fc00 == 0x0e20_1c00 {
        vector_logical(context, raw);
        return Ok(true);
    }
    if general_register_move(context, raw) {
        return Ok(true);
    }
    if raw & 0xffe0_fc00 == 0x4e00_1c00 {
        let immediate = (raw >> 16) & 31;
        let size = immediate.trailing_zeros();
        if size > 3 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let bits = 8 << size;
        let index = immediate >> (size + 1);
        let shift = index * bits;
        let mask = (1u128 << bits) - 1;
        let value = u128::from(read_register(context, ((raw >> 5) & 31) as u8, false)) & mask;
        let destination = &mut context.v[(raw & 31) as usize];
        *destination = (*destination & !(mask << shift)) | (value << shift);
        return Ok(true);
    }
    if raw & 0x9ff8_0c00 == 0x0f00_0400 {
        return modified_immediate(context, raw);
    }
    // DUP (general): Q and imm5 select the vector and element widths.
    if raw & 0xbfe0_fc00 != 0x0e00_0c00 {
        return Ok(false);
    }
    let size = ((raw >> 16) & 15).trailing_zeros();
    let vector_bits = if raw & (1 << 30) != 0 { 128 } else { 64 };
    if size > 3 || (size == 3 && vector_bits == 64) {
        return Err(CpuError::Unsupported {
            pc: context.pc,
            raw,
        });
    }
    let element_bits = 8 << size;
    let mask = (1u128 << element_bits) - 1;
    let element = u128::from(read_register(context, ((raw >> 5) & 31) as u8, false)) & mask;
    let mut result = 0;
    for lane in 0..vector_bits / element_bits {
        result |= element << (lane * element_bits);
    }
    // A 64-bit vector write clears the upper half; it must not preserve old lanes.
    context.v[(raw & 31) as usize] = result;
    Ok(true)
}

fn vector_logical(context: &mut ThreadContext, raw: u32) {
    let destination = (raw & 31) as usize;
    // Snapshot all operands before writing: BSL/BIT/BIF also read the old
    // destination, and any of the three register numbers may coincide.
    let old = context.v[destination];
    let left = context.v[((raw >> 5) & 31) as usize];
    let right = context.v[((raw >> 16) & 31) as usize];
    let operation = ((raw >> 29) & 1) * 4 + ((raw >> 22) & 3);
    let result = match operation {
        0 => left & right,
        1 => left & !right,
        2 => left | right,
        3 => left | !right,
        4 => left ^ right,
        5 => (old & left) | (!old & right),
        6 => (old & !right) | (left & right),
        7 => (old & right) | (left & !right),
        _ => unreachable!(),
    };
    context.v[destination] = if raw & (1 << 30) != 0 {
        result
    } else {
        result & u128::from(u64::MAX)
    };
}

fn general_register_move(context: &mut ThreadContext, raw: u32) -> bool {
    let source = ((raw >> 5) & 31) as u8;
    let destination = (raw & 31) as u8;
    // FMOV is a bit transfer: NaNs, signed zero and denormals are never
    // interpreted or rounded, and FPCR/FPSR remain untouched.
    match raw & 0xffff_fc00 {
        0x1e27_0000 => {
            context.v[destination as usize] =
                u128::from(read_register(context, source, false) as u32);
        }
        0x9e67_0000 => {
            context.v[destination as usize] = u128::from(read_register(context, source, false));
        }
        0x1e26_0000 | 0x9e66_0000 => {
            write_register(
                context,
                destination,
                context.v[source as usize] as u64,
                false,
                raw >> 31 != 0,
            );
        }
        0x9eaf_0000 => {
            // The top-lane variant replaces D[1] while preserving D[0].
            let high = u128::from(read_register(context, source, false)) << 64;
            context.v[destination as usize] =
                (context.v[destination as usize] & u128::from(u64::MAX)) | high;
        }
        0x9eae_0000 => {
            write_register(
                context,
                destination,
                (context.v[source as usize] >> 64) as u64,
                false,
                true,
            );
        }
        // FP16 is outside the baseline profile; numeric conversions are separate.
        _ => return false,
    }
    true
}

fn modified_immediate(context: &mut ThreadContext, raw: u32) -> Result<bool, CpuError> {
    let mode = (raw >> 12) & 15;
    let invert = raw & (1 << 29) != 0;
    // Double-precision vector FMOV requires Q=1 (two D lanes).
    if mode == 15 && invert && raw & (1 << 30) == 0 {
        return Err(CpuError::Unsupported {
            pc: context.pc,
            raw,
        });
    }
    let immediate = u64::from(((raw >> 16) & 7) << 5 | ((raw >> 5) & 31));
    let expanded = match mode {
        0..=7 => repeat_lane(immediate << (8 * (mode / 2)), 32),
        8..=11 => repeat_lane(immediate << (8 * ((mode - 8) / 2)), 16),
        // MSL fills the bits below the shifted immediate with ones.
        12 => repeat_lane((immediate << 8) | 0xff, 32),
        13 => repeat_lane((immediate << 16) | 0xffff, 32),
        14 if !invert => repeat_lane(immediate, 8),
        14 => (0..8).fold(0, |mask, byte| {
            mask | if immediate & (1 << byte) != 0 {
                0xff << (byte * 8)
            } else {
                0
            }
        }),
        15 if !invert => repeat_lane(
            super::floating::expand_immediate(immediate as u8, false),
            32,
        ),
        15 => super::floating::expand_immediate(immediate as u8, true),
        _ => unreachable!(),
    };
    let vector_mask = if raw & (1 << 30) != 0 {
        u128::MAX
    } else {
        u128::from(u64::MAX)
    };
    let operand = (u128::from(expanded) | (u128::from(expanded) << 64)) & vector_mask;
    let destination = &mut context.v[(raw & 31) as usize];
    *destination = if mode < 12 && mode & 1 != 0 {
        if invert {
            *destination & !operand
        } else {
            *destination | operand
        }
    } else if invert && mode < 14 {
        !operand
    } else {
        operand
    } & vector_mask;
    Ok(true)
}

fn repeat_lane(value: u64, bits: u32) -> u64 {
    (0..64 / bits).fold(0, |result, lane| result | (value << (lane * bits)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_add_long_across_widens_every_supported_arrangement() {
        // Encodings independently checked with llvm-mc.
        for (raw, bits, lanes) in [
            (0x2e303800, 8, 8),
            (0x6e303841, 8, 16),
            (0x2e703883, 16, 4),
            (0x6e7038c5, 16, 8),
            (0x6eb03907, 32, 4),
        ] {
            let source = ((raw >> 5) & 31) as usize;
            let destination = (raw & 31) as usize;
            for input in [0, u128::MAX, 0x8a57_9c31_e26b_40fd_1023_4567_89ab_cdef] {
                let mut context = ThreadContext {
                    v: [u128::MAX; 32],
                    pstate: 0xa0000040,
                    fpcr: 0x100,
                    fpsr: 0x10,
                    ..Default::default()
                };
                context.v[source] = input;
                let before = context.clone();
                assert!(execute(&mut context, raw).unwrap());
                let bytes = input.to_le_bytes();
                let expected = bytes
                    .chunks_exact(bits / 8)
                    .take(lanes)
                    .map(|lane| {
                        lane.iter().enumerate().fold(0u128, |value, (index, byte)| {
                            value | (u128::from(*byte) << (index * 8))
                        })
                    })
                    .sum::<u128>();
                let mut vectors = before.v;
                vectors[destination] = expected;
                assert_eq!(context.v, vectors);
                assert_eq!(context.x, before.x);
                assert_eq!(
                    (context.pstate, context.fpcr, context.fpsr, context.pc),
                    (before.pstate, before.fpcr, before.fpsr, before.pc)
                );
            }
        }
        let mut context = ThreadContext::default();
        context.v[31] = u128::MAX;
        assert!(execute(&mut context, 0x6e303bff).unwrap());
        assert_eq!(context.v[31], 16 * 255);
    }

    #[test]
    fn unsigned_add_long_reserved_arrangements_are_atomic() {
        for raw in [0x2eb03800, 0x2ef03800, 0x6ef03800] {
            let mut context = ThreadContext {
                v: [u128::MAX; 32],
                pstate: 0xa0000040,
                ..Default::default()
            };
            assert!(matches!(
                execute(&mut context, raw),
                Err(CpuError::Unsupported { .. })
            ));
            assert_eq!(context.v, [u128::MAX; 32]);
            assert_eq!(context.pstate, 0xa0000040);
        }
        let mut context = ThreadContext::default();
        // The signed SADDLV family is not accidentally accepted as unsigned.
        assert!(!execute(&mut context, 0x0e303800).unwrap());
    }

    #[test]
    fn byte_population_count_covers_all_inputs_widths_and_aliasing() {
        for byte in 0u16..=255 {
            for wide in [false, true] {
                for (source, destination) in [(0, 0), (2, 1), (31, 31), (31, 0)] {
                    let mut context = ThreadContext {
                        v: [u128::MAX; 32],
                        x: [0x1234; 31],
                        pstate: 0xa0000040,
                        fpcr: 0x100,
                        fpsr: 0x10,
                        ..Default::default()
                    };
                    let input = std::array::from_fn(|lane| (byte as u8).rotate_left(lane as u32));
                    context.v[source] = u128::from_le_bytes(input);
                    let before = context.clone();
                    let raw = 0x0e20_5800
                        | (u32::from(wide) << 30)
                        | ((source as u32) << 5)
                        | destination as u32;
                    assert!(execute(&mut context, raw).unwrap());
                    let mut expected = [0; 16];
                    for lane in 0..if wide { 16 } else { 8 } {
                        expected[lane] =
                            (0..8).filter(|bit| input[lane] & (1 << bit) != 0).count() as u8;
                    }
                    let mut vectors = before.v;
                    vectors[destination] = u128::from_le_bytes(expected);
                    assert_eq!(context.v, vectors);
                    assert_eq!(context.x, before.x);
                    assert_eq!(context.pstate, before.pstate);
                    assert_eq!(
                        (context.fpcr, context.fpsr, context.pc),
                        (before.fpcr, before.fpsr, before.pc)
                    );
                }
            }
        }
    }

    #[test]
    fn population_count_rejects_non_byte_and_neighbor_encodings() {
        for raw in [0x0e60_5800, 0x0ea0_5800, 0x2e20_5800, 0x5e20_5800] {
            let mut context = ThreadContext {
                v: [u128::MAX; 32],
                ..Default::default()
            };
            assert!(!execute(&mut context, raw).unwrap());
            assert_eq!(context.v, [u128::MAX; 32]);
        }
    }

    #[test]
    fn vector_logical_operations_match_bit_truth_tables_and_aliases() {
        let encodings = [
            0x4e29_1d00,
            0x4e69_1d00,
            0x4ea9_1d00,
            0x4ee9_1d00,
            0x6e29_1d00,
            0x6e69_1d00,
            0x6ea9_1d00,
            0x6ee9_1d00,
        ];
        for (operation, raw) in encodings.into_iter().enumerate() {
            for wide in [false, true] {
                for destination in [0, 8, 9] {
                    let mut context = ThreadContext::default();
                    context.v[0] = 0xaaaa_aaaa_aaaa_aaaa_aaaa_aaaa_aaaa_aaaa;
                    context.v[8] = 0xcccc_cccc_cccc_cccc_cccc_cccc_cccc_cccc;
                    context.v[9] = 0xf0f0_f0f0_f0f0_f0f0_f0f0_f0f0_f0f0_f0f0;
                    context.pstate = 0xa000_0000;
                    let mut expected = 0;
                    for bit in 0..if wide { 128 } else { 64 } {
                        let old = context.v[destination] & (1 << bit) != 0;
                        let left = context.v[8] & (1 << bit) != 0;
                        let right = context.v[9] & (1 << bit) != 0;
                        let selected = match operation {
                            0 => left && right,
                            1 => left && !right,
                            2 => left || right,
                            3 => left || !right,
                            4 => left != right,
                            5 => {
                                if old {
                                    left
                                } else {
                                    right
                                }
                            }
                            6 => {
                                if right {
                                    left
                                } else {
                                    old
                                }
                            }
                            7 => {
                                if !right {
                                    left
                                } else {
                                    old
                                }
                            }
                            _ => unreachable!(),
                        };
                        expected |= u128::from(selected) << bit;
                    }
                    let encoded =
                        (raw & !((1 << 30) | 31)) | ((wide as u32) << 30) | destination as u32;
                    execute(&mut context, encoded).unwrap();
                    assert_eq!(context.v[destination], expected);
                    assert_eq!(context.pstate, 0xa000_0000);
                }
            }
        }
    }

    #[test]
    fn floating_vector_immediates_replicate_exact_bits() {
        let mut context = ThreadContext {
            fpcr: 0x0700_0000,
            fpsr: 0x0800_009f,
            ..Default::default()
        };
        for immediate in 0..256u32 {
            let encoded = ((immediate >> 5) << 16) | ((immediate & 31) << 5);
            for (prefix, double, wide) in [
                (0x0f00_f400, false, false),
                (0x4f00_f400, false, true),
                (0x6f00_f400, true, true),
            ] {
                context.v[0] = u128::MAX;
                execute(&mut context, prefix | encoded).unwrap();
                let bits = super::super::floating::expand_immediate(immediate as u8, double);
                let low = if double { bits } else { repeat_lane(bits, 32) };
                let expected = u128::from(low) | if wide { u128::from(low) << 64 } else { 0 };
                assert_eq!(context.v[0], expected);
            }
        }
        assert_eq!(context.fpcr, 0x0700_0000);
        assert_eq!(context.fpsr, 0x0800_009f);
    }

    #[test]
    fn general_insert_updates_only_selected_lane_for_every_width() {
        let mut context = ThreadContext::default();
        context.x[8] = 0x0123_4567_89ab_cdef;
        for size in 0..4 {
            let bits = 8 << size;
            let mask = (1u128 << bits) - 1;
            for index in 0..128 / bits {
                context.v[0] = u128::MAX;
                let imm5 = (index << (size + 1)) | (1 << size);
                execute(&mut context, 0x4e00_1d00 | (imm5 << 16)).unwrap();
                let shift = index * bits;
                assert_eq!(context.v[0] & !(mask << shift), !(mask << shift));
                assert_eq!(
                    (context.v[0] >> shift) & mask,
                    u128::from(context.x[8]) & mask
                );
            }
        }
        context.v[0] = u128::MAX;
        execute(&mut context, 0x4e18_1fe0).unwrap(); // INS V0.D[1],XZR
        assert_eq!(context.v[0], u128::from(u64::MAX));
        for raw in [0x4e00_1d00, 0x4e10_1d00] {
            assert!(execute(&mut context, raw).is_err());
            assert_eq!(context.v[0], u128::from(u64::MAX));
        }
    }

    #[test]
    fn general_moves_preserve_payload_bits_without_fp_side_effects() {
        let mut context = ThreadContext {
            fpcr: 0x0700_0000,
            fpsr: 0x0800_009f,
            pstate: 0xa000_0000,
            ..Default::default()
        };
        for payload in [0x8000_0000_0000_0000, 0x7ff0_0000_0000_0042, 1, u64::MAX] {
            context.x[8] = payload;
            context.v[0] = u128::MAX;
            execute(&mut context, 0x9e67_0100).unwrap();
            assert_eq!(context.v[0], u128::from(payload));
            context.v[6] = context.v[0];
            execute(&mut context, 0x9e66_00c5).unwrap();
            assert_eq!(context.x[5], payload);
            context.x[2] = payload;
            context.v[1] = u128::MAX;
            execute(&mut context, 0x1e27_0041).unwrap();
            assert_eq!(context.v[1], u128::from(payload as u32));
            context.v[4] = context.v[1];
            context.x[3] = u64::MAX;
            execute(&mut context, 0x1e26_0083).unwrap();
            assert_eq!(context.x[3], u64::from(payload as u32));
        }
        assert_eq!(context.fpcr, 0x0700_0000);
        assert_eq!(context.fpsr, 0x0800_009f);
        assert_eq!(context.pstate, 0xa000_0000);
    }

    #[test]
    fn top_lane_moves_preserve_low_lane_and_zero_register_is_not_sp() {
        let mut context = ThreadContext::default();
        context.v[7] = 0xffff_ffff_ffff_ffff_1234_5678_9abc_def0;
        context.x[9] = 0xdead_beef;
        execute(&mut context, 0x9eaf_0127).unwrap();
        assert_eq!(context.v[7], 0x0000_0000_dead_beef_1234_5678_9abc_def0);
        context.v[11] = context.v[7];
        execute(&mut context, 0x9eae_016a).unwrap();
        assert_eq!(context.x[10], 0xdead_beef);
        context.sp = 0x8000;
        context.v[31] = u128::MAX;
        execute(&mut context, 0x9e67_03ff).unwrap();
        assert_eq!(context.v[31], 0);
        context.v[31] = u128::MAX;
        execute(&mut context, 0x9e66_03ff).unwrap();
        assert_eq!(context.sp, 0x8000);
        assert_eq!(context.v[31], u128::MAX);
        for raw in [0x9e27_0041, 0x1e67_0041, 0x1ee7_0041, 0x9e6f_0041] {
            assert!(!execute(&mut context, raw).unwrap());
            assert_eq!(context.v[31], u128::MAX);
        }
    }

    #[test]
    fn modified_immediates_match_independently_assembled_encodings() {
        let mut context = ThreadContext::default();
        // Encodings checked with clang's AArch64 assembler and llvm-objdump.
        for (raw, expected) in [
            (0x6f00_e400, 0),
            (0x4f05_6561, repeat_lane(0xab00_0000, 32)),
            (0x6f02_a442, repeat_lane(0xbdff, 16)),
            (0x4f00_d643, repeat_lane(0x0012_ffff, 32)),
            (0x4f01_e6e7, 0x3737_3737_3737_3737),
        ] {
            context.v[(raw & 31) as usize] = u128::MAX;
            assert!(execute(&mut context, raw).unwrap());
            assert_eq!(
                context.v[(raw & 31) as usize],
                u128::from(expected) * ((1u128 << 64) + 1)
            );
        }
        execute(&mut context, 0x2f05_e546).unwrap();
        assert_eq!(context.v[6], 0xff00_ff00_ff00_ff00);
    }

    #[test]
    fn immediate_logical_updates_preserve_other_bits_and_clear_upper_half() {
        let mut context = ThreadContext::default();
        context.v[4] = 0x1010_1010_1010_1010_1010_1010_1010_1010;
        execute(&mut context, 0x4f04_3424).unwrap();
        assert_eq!(context.v[4], 0x1010_9110_1010_9110_1010_9110_1010_9110);
        context.v[5] = u128::MAX;
        execute(&mut context, 0x2f02_b445).unwrap();
        assert_eq!(context.v[5], 0xbdff_bdff_bdff_bdff);
        context.v[8] = 42;
        assert!(execute(&mut context, 0x2f03_f608).is_err());
        assert_eq!(context.v[8], 42);
    }

    #[test]
    fn duplicates_each_element_width_and_clears_upper_half() {
        let mut context = ThreadContext::default();
        context.x[8] = 0x1234_5678_9abc_def0;
        for (imm5, expected) in [
            (1, 0xf0f0_f0f0_f0f0_f0f0u64),
            (2, 0xdef0_def0_def0_def0),
            (4, 0x9abc_def0_9abc_def0),
            (8, 0x1234_5678_9abc_def0),
        ] {
            let raw = 0x4e00_0d00 | (imm5 << 16);
            assert!(execute(&mut context, raw).unwrap());
            assert_eq!(
                context.v[0],
                u128::from(expected) | (u128::from(expected) << 64)
            );
            if imm5 != 8 {
                assert!(execute(&mut context, raw & !(1 << 30)).unwrap());
                assert_eq!(context.v[0], u128::from(expected));
            }
        }
    }

    #[test]
    fn zero_register_and_reserved_encodings() {
        let mut context = ThreadContext::default();
        context.v[0] = u128::MAX;
        execute(&mut context, 0x4e08_0fe0).unwrap();
        assert_eq!(context.v[0], 0);
        context.v[0] = 42;
        for raw in [0x0e08_0c00, 0x4e00_0c00, 0x4e10_0c00] {
            assert!(execute(&mut context, raw).is_err());
            assert_eq!(context.v[0], 42);
        }
    }
}
