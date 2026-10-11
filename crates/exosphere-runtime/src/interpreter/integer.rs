// SPDX-License-Identifier: GPL-3.0-or-later
//! Register ALU operations, with width and shift validation before state mutation.

use super::{CpuError, condition_holds, read_register, set_arithmetic_flags, write_register};
use exosphere_horizon::thread::ThreadContext;

fn shift(value: u64, kind: u32, amount: u32, is64: bool) -> u64 {
    if is64 {
        match kind {
            0 => value << amount,
            1 => value >> amount,
            2 => ((value as i64) >> amount) as u64,
            _ => value.rotate_right(amount),
        }
    } else {
        let value = value as u32;
        (match kind {
            0 => value << amount,
            1 => value >> amount,
            2 => ((value as i32) >> amount) as u32,
            _ => value.rotate_right(amount),
        }) as u64
    }
}

pub(super) fn execute(context: &mut ThreadContext, raw: u32) -> Result<bool, CpuError> {
    if raw & 0x7fa0_0000 == 0x1380_0000 {
        let is64 = raw & (1 << 31) != 0;
        let width = if is64 { 64 } else { 32 };
        let amount = (raw >> 10) & 63;
        if ((raw >> 22) & 1 != u32::from(is64)) || amount >= width {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let high = read_register(context, ((raw >> 5) & 31) as u8, false);
        let low = read_register(context, ((raw >> 16) & 31) as u8, false);
        let mask = if is64 { u64::MAX } else { u64::from(u32::MAX) };
        // EXTR selects a width-sized window from Rn:Rm. A u128 concatenation
        // also handles shift zero without a host shift by 32 or 64.
        let pair = (u128::from(high & mask) << width) | u128::from(low & mask);
        write_register(
            context,
            (raw & 31) as u8,
            (pair >> amount) as u64,
            false,
            is64,
        );
        return Ok(true);
    }
    if matches!(raw & 0xffe0_fc00, 0x9b40_7c00 | 0x9bc0_7c00) {
        let lhs = read_register(context, ((raw >> 5) & 31) as u8, false);
        let rhs = read_register(context, ((raw >> 16) & 31) as u8, false);
        let value = if raw & (1 << 23) != 0 {
            ((u128::from(lhs) * u128::from(rhs)) >> 64) as u64
        } else {
            ((i128::from(lhs as i64) * i128::from(rhs as i64)) >> 64) as u64
        };
        write_register(context, (raw & 31) as u8, value, false, true);
        return Ok(true);
    }
    if raw & 0x7fe0_f000 == 0x1ac0_2000 {
        let is64 = raw & (1 << 31) != 0;
        let value = read_register(context, ((raw >> 5) & 31) as u8, false);
        // Register shifts use only the low log2(width) bits of the shift register.
        let amount =
            read_register(context, ((raw >> 16) & 31) as u8, false) & if is64 { 63 } else { 31 };
        let result = shift(value, (raw >> 10) & 3, amount as u32, is64);
        write_register(context, (raw & 31) as u8, result, false, is64);
        return Ok(true);
    }
    if raw & 0x7fff_0000 == 0x5ac0_0000 {
        let is64 = raw & (1 << 31) != 0;
        let operation = (raw >> 10) & 63;
        if operation > 5 || (!is64 && operation == 3) {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let source = read_register(context, ((raw >> 5) & 31) as u8, false);
        let source = if is64 { source } else { source as u32 as u64 };
        let value = match operation {
            0 => {
                if is64 {
                    source.reverse_bits()
                } else {
                    (source as u32).reverse_bits() as u64
                }
            }
            1 => ((source & 0x00ff_00ff_00ff_00ff) << 8) | ((source >> 8) & 0x00ff_00ff_00ff_00ff),
            2 => {
                // REV32 reverses bytes independently in each 32-bit lane.
                u64::from((source as u32).swap_bytes())
                    | (u64::from(((source >> 32) as u32).swap_bytes()) << 32)
            }
            3 => source.swap_bytes(),
            4 => {
                if is64 {
                    source.leading_zeros() as u64
                } else {
                    (source as u32).leading_zeros() as u64
                }
            }
            5 => {
                // CLS excludes the sign bit from the count of matching leading bits.
                if is64 {
                    let normalized = if source >> 63 != 0 { !source } else { source };
                    u64::from(normalized.leading_zeros() - 1)
                } else {
                    let source = source as u32;
                    let normalized = if source >> 31 != 0 { !source } else { source };
                    u64::from(normalized.leading_zeros() - 1)
                }
            }
            _ => unreachable!(),
        };
        write_register(context, (raw & 31) as u8, value, false, is64);
        return Ok(true);
    }
    if raw & 0x3fe0_0410 == 0x3a40_0000 {
        if !condition_holds(context.pstate, ((raw >> 12) & 15) as u8) {
            context.pstate = (context.pstate & !0xf000_0000) | (u64::from(raw & 15) << 28);
            return Ok(true);
        }
        let is64 = raw & (1 << 31) != 0;
        let sub = raw & (1 << 30) != 0;
        let lhs = read_register(context, ((raw >> 5) & 31) as u8, false);
        let rhs = if raw & (1 << 11) != 0 {
            u64::from((raw >> 16) & 31)
        } else {
            read_register(context, ((raw >> 16) & 31) as u8, false)
        };
        let (lhs, rhs) = if is64 {
            (lhs, rhs)
        } else {
            (lhs as u32 as u64, rhs as u32 as u64)
        };
        let result = if sub {
            lhs.wrapping_sub(rhs)
        } else {
            lhs.wrapping_add(rhs)
        };
        set_arithmetic_flags(
            context,
            lhs,
            rhs,
            if is64 { result } else { result as u32 as u64 },
            sub,
            is64,
        );
        return Ok(true);
    }
    if raw & 0xff60_0000 == 0x9b20_0000 {
        let lhs = read_register(context, ((raw >> 5) & 31) as u8, false) as u32;
        let rhs = read_register(context, ((raw >> 16) & 31) as u8, false) as u32;
        let product = if raw & (1 << 23) != 0 {
            u64::from(lhs) * u64::from(rhs)
        } else {
            (i64::from(lhs as i32) * i64::from(rhs as i32)) as u64
        };
        let accumulator = read_register(context, ((raw >> 10) & 31) as u8, false);
        let result = if raw & (1 << 15) != 0 {
            accumulator.wrapping_sub(product)
        } else {
            accumulator.wrapping_add(product)
        };
        write_register(context, (raw & 31) as u8, result, false, true);
        return Ok(true);
    }
    if raw & 0x7fe0_f800 == 0x1ac0_0800 {
        let is64 = raw & (1 << 31) != 0;
        let signed = raw & (1 << 10) != 0;
        let lhs = read_register(context, ((raw >> 5) & 31) as u8, false);
        let rhs = read_register(context, ((raw >> 16) & 31) as u8, false);
        let value = match (signed, is64) {
            // AArch64 returns zero for division by zero and wraps MIN / -1.
            // Neither case raises an exception or updates NZCV.
            (true, true) => {
                if rhs == 0 {
                    0
                } else {
                    (lhs as i64).wrapping_div(rhs as i64) as u64
                }
            }
            (true, false) => {
                if rhs as u32 == 0 {
                    0
                } else {
                    (lhs as i32).wrapping_div(rhs as i32) as u32 as u64
                }
            }
            (false, true) => lhs.checked_div(rhs).unwrap_or(0),
            (false, false) => (lhs as u32).checked_div(rhs as u32).unwrap_or(0) as u64,
        };
        write_register(context, (raw & 31) as u8, value, false, is64);
        return Ok(true);
    }
    if raw & 0x1fe0_0000 == 0x0b20_0000 {
        let is64 = raw & (1 << 31) != 0;
        let set_flags = raw & (1 << 29) != 0;
        let sub = raw & (1 << 30) != 0;
        let shift = (raw >> 10) & 7;
        if shift > 4 {
            return Err(CpuError::Unsupported {
                pc: context.pc,
                raw,
            });
        }
        let option = (raw >> 13) & 7;
        let bits = 8u32 << (option & 3);
        let source = read_register(context, ((raw >> 16) & 31) as u8, false);
        let rhs = if option & 4 != 0 {
            ((source << (64 - bits)) as i64 >> (64 - bits)) as u64
        } else {
            source & (u64::MAX >> (64 - bits))
        }
        .wrapping_shl(shift);
        let lhs = read_register(context, ((raw >> 5) & 31) as u8, true);
        let (lhs, rhs) = if is64 {
            (lhs, rhs)
        } else {
            (lhs as u32 as u64, rhs as u32 as u64)
        };
        let result = if sub {
            lhs.wrapping_sub(rhs)
        } else {
            lhs.wrapping_add(rhs)
        };
        let result = if is64 { result } else { result as u32 as u64 };
        if set_flags {
            set_arithmetic_flags(context, lhs, rhs, result, sub, is64);
        }
        write_register(context, (raw & 31) as u8, result, !set_flags, is64);
        return Ok(true);
    }
    if raw & 0x3fe0_0800 == 0x1a80_0000 {
        let is64 = raw & (1 << 31) != 0;
        let value = if condition_holds(context.pstate, ((raw >> 12) & 15) as u8) {
            read_register(context, ((raw >> 5) & 31) as u8, false)
        } else {
            let mut value = read_register(context, ((raw >> 16) & 31) as u8, false);
            if raw & (1 << 30) != 0 {
                value = !value;
            }
            if raw & (1 << 10) != 0 {
                value = value.wrapping_add(1);
            }
            value
        };
        write_register(context, (raw & 31) as u8, value, false, is64);
        return Ok(true);
    }
    if raw & 0x7fe0_0000 == 0x1b00_0000 {
        let lhs = read_register(context, ((raw >> 5) & 31) as u8, false);
        let rhs = read_register(context, ((raw >> 16) & 31) as u8, false);
        let accumulator = read_register(context, ((raw >> 10) & 31) as u8, false);
        let product = lhs.wrapping_mul(rhs);
        let result = if raw & (1 << 15) != 0 {
            accumulator.wrapping_sub(product)
        } else {
            accumulator.wrapping_add(product)
        };
        write_register(
            context,
            (raw & 31) as u8,
            result,
            false,
            raw & (1 << 31) != 0,
        );
        return Ok(true);
    }
    let logical = raw & 0x1f00_0000 == 0x0a00_0000;
    let arithmetic = raw & 0x1f20_0000 == 0x0b00_0000;
    if !logical && !arithmetic {
        return Ok(false);
    }
    let is64 = raw & (1 << 31) != 0;
    let kind = (raw >> 22) & 3;
    let amount = (raw >> 10) & 63;
    if (!is64 && amount >= 32) || (arithmetic && kind == 3) {
        return Err(CpuError::Unsupported {
            pc: context.pc,
            raw,
        });
    }
    let destination = (raw & 31) as u8;
    let lhs = read_register(context, ((raw >> 5) & 31) as u8, false);
    let lhs = if is64 { lhs } else { lhs as u32 as u64 };
    let rhs = shift(
        read_register(context, ((raw >> 16) & 31) as u8, false),
        kind,
        amount,
        is64,
    );
    let value = if logical {
        let rhs = if raw & (1 << 21) != 0 { !rhs } else { rhs };
        match (raw >> 29) & 3 {
            0 | 3 => lhs & rhs,
            1 => lhs | rhs,
            _ => lhs ^ rhs,
        }
    } else if raw & (1 << 30) != 0 {
        lhs.wrapping_sub(rhs)
    } else {
        lhs.wrapping_add(rhs)
    };
    let value = if is64 { value } else { value as u32 as u64 };
    if raw & (1 << 29) != 0 {
        if logical && (raw >> 29) & 3 == 3 {
            context.pstate = (context.pstate & !0xf0000000)
                | ((value == 0) as u64) << 30
                | ((value >> if is64 { 63 } else { 31 }) & 1) << 31;
        } else if arithmetic {
            set_arithmetic_flags(context, lhs, rhs, value, raw & (1 << 30) != 0, is64);
        }
    }
    write_register(context, destination, value, false, is64);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_multiply_returns_upper_signed_or_unsigned_half() {
        let mut context = ThreadContext::default();
        for (raw, lhs, rhs, expected) in [
            (0x9bc27c20, u64::MAX, u64::MAX, u64::MAX - 1),
            (0x9b427c20, u64::MAX, u64::MAX, 0),
            (0x9b427c20, u64::MAX, 2, u64::MAX),
            (0x9b427c20, i64::MIN as u64, i64::MIN as u64, 1 << 62),
            (0x9bc27c20, 0, u64::MAX, 0),
        ] {
            context.x[1] = lhs;
            context.x[2] = rhs;
            execute(&mut context, raw).unwrap();
            assert_eq!(context.x[0], expected);
        }
    }

    #[test]
    fn variable_shifts_mask_amount_and_obey_signed_width() {
        let mut context = ThreadContext::default();
        context.x[1] = 0x8000000080000001;
        context.x[2] = 65;
        for (raw, expected) in [
            (0x9ac22020, 0x100000002),
            (0x9ac22420, 0x4000000040000000),
            (0x9ac22820, 0xc000000040000000),
            (0x9ac22c20, 0xc000000040000000),
            (0x1ac22020, 2),
            (0x1ac22420, 0x40000000),
            (0x1ac22820, 0xc0000000),
            (0x1ac22c20, 0xc0000000),
        ] {
            execute(&mut context, raw).unwrap();
            assert_eq!(context.x[0], expected);
        }
    }

    #[test]
    fn bit_reverse_byte_reverse_and_leading_counts_obey_width() {
        let mut context = ThreadContext {
            pstate: 0xa0000000,
            ..Default::default()
        };
        for (raw, source, expected) in [
            (0xdac00020, 1, 1 << 63),
            (0x5ac00020, 0xaaaaaaaa00000001, 1 << 31),
            (0xdac00420, 0x0123456789abcdef, 0x23016745ab89efcd),
            (0xdac00820, 0x0123456789abcdef, 0x67452301efcdab89),
            (0xdac00c20, 0x0123456789abcdef, 0xefcdab8967452301),
            (0x5ac00820, 0xaaaaaaaa01234567, 0x67452301),
            (0xdac01020, 0, 64),
            (0x5ac01020, 1 << 32, 32),
            (0xdac01420, u64::MAX, 63),
            (0x5ac01420, 0xffffffff, 31),
            (0xdac01420, 1 << 62, 0),
            (0x5ac01420, 0x80000000, 0),
        ] {
            context.x[1] = source;
            execute(&mut context, raw).unwrap();
            assert_eq!(context.x[0], expected);
            assert_eq!(context.pstate, 0xa0000000);
        }
        assert!(execute(&mut context, 0x5ac00c20).is_err());
    }

    #[test]
    fn conditional_compare_uses_fallback_flags_or_actual_arithmetic() {
        let mut context = ThreadContext::default();
        context.x[1] = 5;
        context.x[2] = 5;
        context.pstate = 0x123;
        execute(&mut context, 0xfa42_002a).unwrap(); // ccmp x1,x2,#10,eq
        assert_eq!(context.pstate, 0xa0000123);
        context.pstate = 0x40000123;
        execute(&mut context, 0xfa42_002a).unwrap();
        assert_eq!(context.pstate, 0x60000123);
        context.x[1] = 0xffffffff;
        execute(&mut context, 0x3a41_0820).unwrap(); // ccmn w1,#1,#0,eq
        assert_eq!(context.pstate, 0x60000123);
        assert_eq!(context.x[1], 0xffffffff);
    }

    #[test]
    fn long_multiply_extends_operands_before_accumulating() {
        let mut context = ThreadContext::default();
        context.x[1] = u64::MAX;
        context.x[2] = 3;
        context.x[3] = 7;
        for (raw, expected) in [
            (0x9ba2_0c20, 0x300000004),                    // UMADDL
            (0x9b22_0c20, 4),                              // SMADDL
            (0x9ba2_8c20, 7u64.wrapping_sub(0x2fffffffd)), // UMSUBL
            (0x9b22_8c20, 10),                             // SMSUBL
        ] {
            execute(&mut context, raw).unwrap();
            assert_eq!(context.x[0], expected);
        }
    }

    #[test]
    fn division_handles_zero_overflow_and_signed_truncation() {
        let mut context = ThreadContext {
            pstate: 0xf0000000,
            ..Default::default()
        };
        for (raw, lhs, rhs, expected) in [
            (0x9ac2_0820, 100, 7, 14),
            (0x1ac2_0820, 0xffffffff00000064, 0xaaaaaaaa00000007, 14),
            (0x9ac2_0c20, (-100i64) as u64, 7, (-14i64) as u64),
            (0x1ac2_0c20, (-100i64) as u64, 7, (-14i32) as u32 as u64),
            (0x9ac2_0c20, i64::MIN as u64, u64::MAX, i64::MIN as u64),
            (0x1ac2_0c20, 0x80000000, 0xffffffff, 0x80000000),
            (0x9ac2_0820, 100, 0, 0),
            (0x9ac2_0c20, 100, 0, 0),
            (0x1ac2_0820, 100, 1 << 32, 0),
            (0x1ac2_0c20, 100, 1 << 32, 0),
        ] {
            context.x[1] = lhs;
            context.x[2] = rhs;
            assert!(execute(&mut context, raw).unwrap());
            assert_eq!(context.x[0], expected);
            assert_eq!(context.pstate, 0xf0000000);
        }
    }

    #[test]
    fn extended_arithmetic_uses_stack_and_signed_subwords() {
        let mut context = ThreadContext {
            sp: 0x9000,
            ..Default::default()
        };
        context.x[1] = 0xffffffff;
        execute(&mut context, 0x8b21_c3e0).unwrap(); // add x0,sp,w1,sxtw
        assert_eq!(context.x[0], 0x8fff);
        context.x[1] = 0xff;
        execute(&mut context, 0x8b21_83ff).unwrap(); // add sp,sp,w1,sxtb
        assert_eq!(context.sp, 0x8fff);
        let previous = context.sp;
        assert!(execute(&mut context, 0x8b21_97ff).is_err()); // reserved shift #5
        assert_eq!(context.sp, previous);
    }

    #[test]
    fn conditional_select_variants_preserve_flags() {
        let mut context = ThreadContext::default();
        context.x[1] = 12;
        context.x[2] = 7;
        for (variant, expected) in [
            (0, 7),
            (1 << 10, 8),
            (1 << 30, !7u32 as u64),
            ((1 << 30) | (1 << 10), (-7i32) as u32 as u64),
        ] {
            let raw = 0x1a82_0020 | variant; // EQ with Z clear selects transformed W2.
            execute(&mut context, raw).unwrap();
            assert_eq!(context.x[0], expected);
            assert_eq!(context.pstate, 0);
            context.pstate = 1 << 30;
            execute(&mut context, raw).unwrap();
            assert_eq!(context.x[0], 12);
            assert_eq!(context.pstate, 1 << 30);
            context.pstate = 0;
        }
        execute(&mut context, 0x1a9f_17e9).unwrap(); // CSET W9, EQ.
        assert_eq!(context.x[9], 0);
        context.pstate = 1 << 30;
        execute(&mut context, 0x1a9f_17e9).unwrap();
        assert_eq!(context.x[9], 1);
    }

    #[test]
    fn multiply_accumulate_wraps_to_register_width() {
        let mut context = ThreadContext::default();
        context.x[1] = u64::MAX;
        context.x[2] = 3;
        context.x[3] = 7;
        // MADD X0, X1, X2, X3 and MSUB W0, W1, W2, W3.
        execute(&mut context, 0x9b02_0c20).unwrap();
        assert_eq!(context.x[0], 4);
        execute(&mut context, 0x1b02_8c20).unwrap();
        assert_eq!(context.x[0], 10);
        // MUL is the MADD alias with the zero register as accumulator.
        execute(&mut context, 0x1b02_7c20).unwrap();
        assert_eq!(context.x[0], 0xffff_fffd);
    }
}
