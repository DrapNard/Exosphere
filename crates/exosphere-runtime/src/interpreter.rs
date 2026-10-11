// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact baseline execution for the initial scalar AArch64 subset.
//! Unsupported instructions stop before modifying the CPU context.

use exosphere_cpu_ir::{DecodedOpcode, MoveWideKind, decode::decode_one_at};
use exosphere_horizon::{error::HorizonError, memory::AddressSpace, thread::ThreadContext};
mod bitmask;
mod exclusive;
mod floating;
mod integer;
mod memory_ops;
mod simd;
mod simd_memory;
mod system;

#[derive(Debug, thiserror::Error)]
pub enum CpuError {
    #[error(transparent)]
    Memory(#[from] HorizonError),
    #[error("unsupported ARM64 instruction {raw:08x} at 0x{pc:x}")]
    Unsupported { pc: u64, raw: u32 },
    #[error("Horizon SVC 0x{number:x} at 0x{pc:x} requires kernel dispatch")]
    SupervisorCall { pc: u64, number: u16 },
    #[error("architectural counter read at 0x{pc:x} requires the kernel time source")]
    CounterRead {
        pc: u64,
        register: u8,
        frequency: bool,
    },
}

fn read_register(context: &ThreadContext, register: u8, stack_pointer: bool) -> u64 {
    if register == 31 {
        if stack_pointer { context.sp } else { 0 }
    } else {
        context.x[register as usize]
    }
}

fn write_register(
    context: &mut ThreadContext,
    register: u8,
    value: u64,
    stack_pointer: bool,
    is64: bool,
) {
    let value = if is64 { value } else { value as u32 as u64 };
    if register == 31 {
        if stack_pointer {
            context.sp = value;
        }
    } else {
        context.x[register as usize] = value;
    }
}

fn set_arithmetic_flags(
    context: &mut ThreadContext,
    lhs: u64,
    rhs: u64,
    result: u64,
    sub: bool,
    is64: bool,
) {
    let sign = 1u64 << if is64 { 63 } else { 31 };
    let negative = result & sign != 0;
    let zero = result == 0;
    let carry = if sub {
        lhs >= rhs
    } else if is64 {
        lhs.overflowing_add(rhs).1
    } else {
        lhs + rhs > u32::MAX as u64
    };
    let overflow = if sub {
        ((lhs ^ rhs) & (lhs ^ result) & sign) != 0
    } else {
        (!(lhs ^ rhs) & (lhs ^ result) & sign) != 0
    };
    context.pstate = (context.pstate & !0xf000_0000)
        | ((negative as u64) << 31)
        | ((zero as u64) << 30)
        | ((carry as u64) << 29)
        | ((overflow as u64) << 28);
}

fn condition_holds(pstate: u64, condition: u8) -> bool {
    let negative = pstate & (1 << 31) != 0;
    let zero = pstate & (1 << 30) != 0;
    let carry = pstate & (1 << 29) != 0;
    let overflow = pstate & (1 << 28) != 0;
    let result = match condition >> 1 {
        0 => zero,
        1 => carry,
        2 => negative,
        3 => overflow,
        4 => carry && !zero,
        5 => negative == overflow,
        6 => negative == overflow && !zero,
        _ => true,
    };
    if condition & 1 != 0 && condition != 15 {
        !result
    } else {
        result
    }
}

pub fn step(context: &mut ThreadContext, memory: &mut AddressSpace) -> Result<(), CpuError> {
    let pc = context.pc;
    let raw = memory.fetch_instruction(pc)?;
    if system::prefetch_hint(raw) {
        context.pc = pc.wrapping_add(4);
        return Ok(());
    }
    let unsupported = || CpuError::Unsupported { pc, raw };
    let mut next_pc = pc.wrapping_add(4);
    match decode_one_at(raw, pc) {
        DecodedOpcode::Nop => {}
        DecodedOpcode::CondBranch { target, cond } => {
            if condition_holds(context.pstate, cond) {
                next_pc = target;
            }
        }
        DecodedOpcode::CompareBranch {
            target,
            rt,
            nonzero,
            is64,
        } => {
            let value = read_register(context, rt, false);
            let value = if is64 { value } else { value as u32 as u64 };
            if (value != 0) == nonzero {
                next_pc = target;
            }
        }
        DecodedOpcode::TestBranch {
            target,
            rt,
            bit,
            nonzero,
        } => {
            if (read_register(context, rt, false) & (1u64 << bit) != 0) == nonzero {
                next_pc = target;
            }
        }
        DecodedOpcode::BranchImm { target, link } => {
            if link {
                context.x[30] = next_pc;
            }
            next_pc = target;
        }
        DecodedOpcode::BranchReg { rn, link } => {
            if rn == 31 {
                return Err(unsupported());
            }
            next_pc = context.x[rn as usize];
            if link {
                context.x[30] = pc.wrapping_add(4);
            }
        }
        DecodedOpcode::Ret { rn } => {
            if rn == 31 {
                return Err(unsupported());
            }
            next_pc = context.x[rn as usize];
        }
        DecodedOpcode::Adr { rd, target, .. } => write_register(context, rd, target, false, true),
        DecodedOpcode::MoveWide {
            rd,
            imm,
            kind,
            is64,
        } => {
            let shift = ((raw >> 21) & 3) * 16;
            if !is64 && shift >= 32 {
                return Err(unsupported());
            }
            let value = match kind {
                MoveWideKind::MovZ => imm,
                MoveWideKind::MovN => !imm,
                MoveWideKind::MovK => {
                    (read_register(context, rd, false) & !(0xffffu64 << shift)) | imm
                }
            };
            write_register(context, rd, value, false, is64);
        }
        DecodedOpcode::AddSubImm {
            rd,
            rn,
            imm,
            sub,
            set_flags,
            is64,
        } => {
            // Bit 23 is reserved in this encoding; the structural decoder is broader.
            if raw & (1 << 23) != 0 {
                return Err(unsupported());
            }
            let lhs = read_register(context, rn, true);
            let lhs = if is64 { lhs } else { lhs as u32 as u64 };
            let value = if sub {
                lhs.wrapping_sub(imm)
            } else {
                lhs.wrapping_add(imm)
            };
            let value = if is64 { value } else { value as u32 as u64 };
            if set_flags {
                set_arithmetic_flags(context, lhs, imm, value, sub, is64);
            }
            write_register(context, rd, value, !set_flags, is64);
        }
        DecodedOpcode::LoadStoreUnsigned {
            rt,
            rn,
            offset,
            load,
            size_bits,
        } => {
            let operation = (raw >> 22) & 3;
            // SIMD and PRFM share this structural class but are not integer transfers.
            if raw & (1 << 26) != 0
                || (size_bits == 64 && operation > 1)
                || (size_bits == 32 && operation == 3)
            {
                return Err(unsupported());
            }
            let address = read_register(context, rn, true).wrapping_add(offset);
            let size = (size_bits / 8) as usize;
            let mut bytes = [0u8; 8];
            if load || operation >= 2 {
                memory.read(address, &mut bytes[..size])?;
                let mut value = u64::from_le_bytes(bytes);
                if operation >= 2 {
                    let shift = 64 - size_bits;
                    value = ((value << shift) as i64 >> shift) as u64;
                }
                write_register(context, rt, value, false, size_bits == 64 || operation == 2);
            } else {
                bytes = read_register(context, rt, false).to_le_bytes();
                memory.write(address, &bytes[..size])?;
            }
        }
        DecodedOpcode::LogicalImm {
            rd, rn, op, is64, ..
        } => {
            let width = if is64 { 64 } else { 32 };
            let (mask, _) = bitmask::decode(
                (raw >> 22) & 1,
                (raw >> 16) & 63,
                (raw >> 10) & 63,
                width,
                true,
            )
            .ok_or_else(unsupported)?;
            let source = read_register(context, rn, false);
            let value = match op {
                0 | 3 => source & mask,
                1 => source | mask,
                2 => source ^ mask,
                _ => return Err(unsupported()),
            };
            if op == 3 {
                let value = if is64 { value } else { value as u32 as u64 };
                context.pstate = (context.pstate & !0xf0000000)
                    | ((value == 0) as u64) << 30
                    | ((value >> (width - 1)) & 1) << 31;
            }
            write_register(context, rd, value, op != 3, is64);
        }
        DecodedOpcode::Unknown if raw & 0x1f80_0000 == 0x1300_0000 => {
            let is64 = raw & (1 << 31) != 0;
            let destination = (raw & 31) as u8;
            let source = ((raw >> 5) & 31) as u8;
            let value = bitmask::apply(
                read_register(context, source, false),
                read_register(context, destination, false),
                (raw >> 22) & 1,
                (raw >> 16) & 63,
                (raw >> 10) & 63,
                if is64 { 64 } else { 32 },
                (raw >> 29) & 3,
            )
            .ok_or_else(unsupported)?;
            write_register(context, destination, value, false, is64);
        }
        DecodedOpcode::Unknown if raw & 0x7fe0_ffe0 == 0x2a00_03e0 => {
            // MOV Wd/Wm or Xd/Xm is the unshifted ORR alias with Rn=ZR.
            let source = ((raw >> 16) & 31) as u8;
            let destination = (raw & 31) as u8;
            let value = read_register(context, source, false);
            write_register(context, destination, value, false, raw & (1 << 31) != 0);
        }
        DecodedOpcode::Svc { imm } => return Err(CpuError::SupervisorCall { pc, number: imm }),
        DecodedOpcode::Unknown => {
            if !integer::execute(context, raw)?
                && !exclusive::execute(context, memory, raw)?
                && !memory_ops::execute(context, memory, raw)?
                && !simd_memory::execute(context, memory, raw)?
                && !simd::execute(context, raw)?
                && !floating::execute(context, raw)?
                && !system::execute(context, raw)?
            {
                return Err(unsupported());
            }
        }
    }
    context.pc = next_pc;
    Ok(())
}

/// Runs at most `budget` instructions. The returned count excludes the instruction
/// that stopped execution; its PC remains available for diagnosis or kernel dispatch.
pub fn run(
    context: &mut ThreadContext,
    memory: &mut AddressSpace,
    budget: usize,
) -> (usize, Option<CpuError>) {
    for completed in 0..budget {
        if let Err(error) = step(context, memory) {
            return (completed, Some(error));
        }
    }
    (budget, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_vector_population_count_sequence_reduces_without_byte_overflow() {
        let (mut context, mut memory) = fixture(&[0x0e205800, 0x2e303800]);
        context.v[0] = u128::MAX;
        context.pstate = 0xa0000040;
        context.fpsr = 0x10;
        step(&mut context, &mut memory).unwrap();
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], 64);
        assert_eq!(context.pc, 0x1008);
        assert_eq!((context.pstate, context.fpsr), (0xa0000040, 0x10));
        // Widening matters: sixteen maximum bytes sum to 4080, not 240.
        let (mut context, mut memory) = fixture(&[0x6e303800]);
        context.v[0] = u128::MAX;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], 4080);
    }

    #[test]
    fn byte_population_count_executes_observed_game_opcode_and_wide_form() {
        // Independently checked with llvm-mc: CNT V0.8B,V0.8B; CNT V1.16B,V2.16B.
        let (mut context, mut memory) = fixture(&[0x0e205800, 0x4e205841]);
        context.v[0] = u128::MAX;
        context.v[2] = u128::from_le_bytes([
            0, 1, 3, 7, 15, 31, 63, 127, 255, 128, 192, 224, 240, 248, 252, 254,
        ]);
        context.pstate = 0xa0000040;
        context.fpsr = 0x10;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], 0x0808080808080808);
        step(&mut context, &mut memory).unwrap();
        assert_eq!(
            context.v[1].to_le_bytes(),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(context.pc, 0x1008);
        assert_eq!((context.pstate, context.fpsr), (0xa0000040, 0x10));
    }
    use exosphere_horizon::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};

    #[test]
    fn precision_conversion_executes_guest_encoding_and_preserves_unrelated_state() {
        let (mut context, mut memory) = fixture(&[0x1e22c001, 0x1e624022]);
        context.v[0] = (u128::MAX << 32) | u128::from(1.5f32.to_bits());
        context.v[1] = u128::MAX;
        context.v[2] = u128::MAX;
        context.x = [0x1234; 31];
        context.pstate = 0xa0000040;
        context.fpsr = 1;
        let source = context.v[0];
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[1], u128::from(1.5f64.to_bits()));
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[2], u128::from(1.5f32.to_bits()));
        assert_eq!(context.v[0], source);
        assert_eq!(context.x, [0x1234; 31]);
        assert_eq!(context.pstate, 0xa0000040);
        assert_eq!(context.fpsr, 1);
        assert_eq!(context.pc, 0x1008);
        for raw in [
            0x1e224000, 0x1e62c000, 0x1e23c000, 0x1ee24000, 0x1ea2c000, 0x1e234000,
        ] {
            let (mut context, mut memory) = fixture(&[raw]);
            context.v = [u128::MAX; 32];
            context.fpsr = 0x10;
            assert!(matches!(
                step(&mut context, &mut memory),
                Err(CpuError::Unsupported { .. })
            ));
            assert_eq!(context.v, [u128::MAX; 32]);
            assert_eq!(context.fpsr, 0x10);
            assert_eq!(context.pc, 0x1000);
        }
    }

    #[test]
    fn conditional_fp_compare_executes_guest_encoding_and_rejects_unsupported_precision() {
        let (mut context, mut memory) = fixture(&[0x1e205500, 0x1e3f07ff]);
        context.v[8] = (u128::MAX << 32) | u128::from(1f32.to_bits());
        context.v[0] = u128::from(2f32.to_bits());
        context.v[31] = 0x7f800001; // sNaN: skipped when EQ fails
        context.pstate = 0x40;
        context.fpsr = 0x10;
        let vectors = context.v;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.pstate, 0x80000040);
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.pstate, 0xf0000040);
        assert_eq!(context.fpsr, 0x10);
        assert_eq!(context.v, vectors);
        assert_eq!(context.pc, 0x1008);
        for raw in [0x1ea05500, 0x1ee05500] {
            let (mut context, mut memory) = fixture(&[raw]);
            context.pstate = 0x80000000; // condition false still requires valid precision
            context.fpsr = 0x10;
            assert!(matches!(
                step(&mut context, &mut memory),
                Err(CpuError::Unsupported { .. })
            ));
            assert_eq!(context.pc, 0x1000);
            assert_eq!(context.pstate, 0x80000000);
            assert_eq!(context.fpsr, 0x10);
        }
    }

    #[test]
    fn prefetch_forms_preserve_state_and_do_not_access_hinted_memory() {
        use exosphere_horizon::thread::ExclusiveReservation;
        let fixtures = [
            0xf9800261, 0xf9bffff4, 0xf890006a, 0xf88ff3e0, 0xf8a25820, 0xf8bf6be2, 0xf8a4c870,
            0xf8a4f868, 0xd8008000,
        ];
        for raw in fixtures {
            let (mut context, mut memory) = fixture(&[raw]);
            context.x = [u64::MAX; 31];
            context.v = [u128::MAX; 32];
            context.sp = u64::MAX;
            context.pstate = 0xf000_0040;
            context.fpcr = 0x400000;
            context.fpsr = 0x10;
            context.exclusive = Some(ExclusiveReservation {
                address: 0x5000,
                size: 8,
                epoch: memory.write_epoch(),
            });
            let before = context.clone();
            let epoch = memory.write_epoch();
            step(&mut context, &mut memory).unwrap();
            assert_eq!(context.pc, 0x1004);
            assert_eq!(context.x, before.x);
            assert_eq!(context.v, before.v);
            assert_eq!(context.sp, before.sp);
            assert_eq!(context.pstate, before.pstate);
            assert_eq!((context.fpcr, context.fpsr), (before.fpcr, before.fpsr));
            assert_eq!(context.exclusive, before.exclusive);
            assert_eq!(memory.write_epoch(), epoch);
        }
        // Every immediate hint value is accepted, including unnamed hints;
        // register encodings belonging to newer RPRFM stay unsupported.
        for hint in 0..32 {
            for base in [0xf9800000, 0xf8800000, 0xd8000000] {
                assert!(system::prefetch_hint(base | hint));
            }
            for option in 0..8 {
                let raw = 0xf8a00800 | (option << 13) | hint;
                assert_eq!(system::prefetch_hint(raw), option & 2 != 0 && hint < 24);
            }
        }
    }

    #[test]
    fn prefetch_does_not_hide_load_faults_or_reserved_addressing() {
        for raw in [0xf9400260, 0xf9000260] {
            // ordinary LDR/STR, not hints
            let (mut context, mut memory) = fixture(&[raw]);
            context.x[19] = u64::MAX;
            assert!(matches!(
                step(&mut context, &mut memory),
                Err(CpuError::Memory(_))
            ));
            assert_eq!(context.pc, 0x1000);
        }
        for raw in [0xf8800400, 0xf8800c00, 0xf8a00800, 0xf8a06818] {
            let (mut context, mut memory) = fixture(&[raw]);
            context.x = [123; 31];
            assert!(matches!(
                step(&mut context, &mut memory),
                Err(CpuError::Unsupported { .. })
            ));
            assert_eq!(context.pc, 0x1000);
            assert_eq!(context.x, [123; 31]);
        }
    }

    #[test]
    fn extract_pair_handles_all_shifts_widths_aliasing_and_rotation() {
        for is64 in [false, true] {
            let width = if is64 { 64 } else { 32 };
            let mask = if is64 { u64::MAX } else { u64::from(u32::MAX) };
            for amount in 0..width {
                for destination in [0, 1, 2, 31] {
                    let raw = 0x1380_0000
                        | if is64 { 0x8040_0000 } else { 0 }
                        | (2 << 16)
                        | (amount << 10)
                        | (1 << 5)
                        | destination;
                    let (mut context, mut memory) = fixture(&[raw]);
                    context.x[1] = 0xfedc_ba98_7654_3210;
                    context.x[2] = 0x1234_5678_9abc_def0;
                    context.pstate = 0xa000_0040;
                    context.sp = 0x9876;
                    let before = context.x;
                    // Independent bit-by-bit window oracle, not concatenation.
                    let mut expected = 0u64;
                    for bit in 0..width {
                        let position = bit + amount;
                        let source_bit = if position < width {
                            (before[2] >> position) & 1
                        } else {
                            (before[1] >> (position - width)) & 1
                        };
                        expected |= source_bit << bit;
                    }
                    step(&mut context, &mut memory).unwrap();
                    if destination != 31 {
                        assert_eq!(context.x[destination as usize], expected & mask);
                    }
                    for (register, previous) in before.iter().enumerate() {
                        if register != destination as usize {
                            assert_eq!(context.x[register], *previous);
                        }
                    }
                    assert_eq!(context.pc, 0x1004);
                    assert_eq!(context.pstate, 0xa000_0040);
                    assert_eq!(context.sp, 0x9876);
                }
            }
        }
        let (mut context, mut memory) = fixture(&[0x93c3_4463]); // ror x3, x3, #17
        context.x[3] = 0x1234_5678_9abc_def0;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.x[3], 0x1234_5678_9abc_def0u64.rotate_right(17));
    }

    #[test]
    fn extract_guest_encoding_and_zero_register_sources() {
        let (mut context, mut memory) = fixture(&[0x138c_2222]);
        context.x[17] = 0xffff_ffff_1234_5678;
        context.x[12] = 0xaaaa_aaaa_9abc_def0;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.x[2], 0x789a_bcde);
        for (raw, expected) in [
            (0x93c2_27e0, 0x007f_ffff_ffff_ffff),
            (0x93df_2420, 0xff80_0000_0000_0000),
        ] {
            let (mut context, mut memory) = fixture(&[raw]);
            context.x[1] = u64::MAX;
            context.x[2] = u64::MAX;
            context.sp = 123;
            step(&mut context, &mut memory).unwrap();
            assert_eq!(context.x[0], expected);
            assert_eq!(context.sp, 123);
        }
    }

    #[test]
    fn extract_reserved_encodings_leave_context_unchanged() {
        for raw in [
            0x13c2_0020,
            0x9382_0020,
            0x1382_8020,
            0x13a2_0020,
            0x3382_0020,
            0x5382_0020,
        ] {
            let (mut context, mut memory) = fixture(&[raw]);
            context.x = [u64::MAX; 31];
            context.pstate = 0xf000_0000;
            context.sp = 0x9000;
            assert!(matches!(
                step(&mut context, &mut memory),
                Err(CpuError::Unsupported { .. })
            ));
            assert_eq!(context.x, [u64::MAX; 31]);
            assert_eq!(context.pstate, 0xf000_0000);
            assert_eq!(context.sp, 0x9000);
            assert_eq!(context.pc, 0x1000);
        }
    }

    #[test]
    fn floating_select_preserves_payloads_for_every_condition_and_alias() {
        for kind in [0u32, 1] {
            for flags in 0..16u64 {
                let n = flags & 8 != 0;
                let z = flags & 4 != 0;
                let c = flags & 2 != 0;
                let v = flags & 1 != 0;
                let expected = [
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
                for (condition, select_first) in expected.into_iter().enumerate() {
                    for destination in [0u32, 1, 2, 31] {
                        let raw = 0x1e200c00
                            | kind << 22
                            | 2 << 16
                            | (condition as u32) << 12
                            | 1 << 5
                            | destination;
                        let (mut context, mut memory) = fixture(&[raw]);
                        context.v[1] = (u128::MAX << 64) | 0x7ff000007f800001;
                        context.v[2] = (u128::MAX << 64) | 0x8000000000000001;
                        context.fpcr = (1 << 25) | (1 << 24);
                        context.fpsr = 0x9f;
                        context.pstate = flags << 28 | 0x123;
                        let before = context.clone();
                        step(&mut context, &mut memory).unwrap();
                        let mask = if kind == 1 {
                            u64::MAX as u128
                        } else {
                            u32::MAX as u128
                        };
                        let mut vectors = before.v;
                        vectors[destination as usize] =
                            before.v[if select_first { 1 } else { 2 }] & mask;
                        assert_eq!(context.v, vectors);
                        assert_eq!(context.x, before.x);
                        assert_eq!(context.pstate, before.pstate);
                        assert_eq!(context.fpcr, before.fpcr);
                        assert_eq!(context.fpsr, before.fpsr);
                        assert_eq!(context.pc, before.pc + 4);
                    }
                }
            }
        }
    }

    #[test]
    fn floating_select_executes_guest_word_and_rejects_unsupported_widths() {
        for carry in [false, true] {
            let (mut context, mut memory) = fixture(&[0x1e223c21]);
            context.v[1] = u128::MAX;
            context.v[2] = 0x80000001;
            context.pstate = if carry { 1 << 29 } else { 0 };
            step(&mut context, &mut memory).unwrap();
            assert_eq!(
                context.v[1],
                if carry { 0x80000001 } else { u32::MAX as u128 }
            );
        }
        for kind in [2u32, 3] {
            let raw = 0x1e223c21 | kind << 22;
            let (mut context, mut memory) = fixture(&[raw]);
            context.v[1] = u128::MAX;
            context.pstate = 0xf0000000;
            let before = context.clone();
            assert!(matches!(
                step(&mut context, &mut memory),
                Err(CpuError::Unsupported { .. })
            ));
            assert_eq!(context.pc, before.pc);
            assert_eq!(context.v, before.v);
            assert_eq!(context.pstate, before.pstate);
        }
    }

    fn fixture(words: &[u32]) -> (ThreadContext, AddressSpace) {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_EXECUTE,
                MemoryKind::Code,
            )
            .unwrap();
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        memory.load_bytes(0x1000, &bytes).unwrap();
        let context = ThreadContext {
            pc: 0x1000,
            ..Default::default()
        };
        (context, memory)
    }

    #[test]
    fn scalar_unary_moves_preserve_payloads_and_flags() {
        // Independently assembled FMOV/FABS/FNEG S0,S1 and D0,D1.
        for (base, double) in [
            (0x1e20_4020, false),
            (0x1e20_c020, false),
            (0x1e21_4020, false),
            (0x1e60_4020, true),
            (0x1e60_c020, true),
            (0x1e61_4020, true),
        ] {
            let sign = if double { 1u64 << 63 } else { 1u64 << 31 };
            let infinity = if double {
                0x7ff0_0000_0000_0000
            } else {
                0x7f80_0000
            };
            let quiet = if double { 1u64 << 51 } else { 1u64 << 22 };
            for magnitude in [0, 1, infinity, infinity | 0x42, infinity | quiet | 0x42] {
                for negative in [false, true] {
                    for alias in [false, true] {
                        let bits = magnitude | if negative { sign } else { 0 };
                        let word = if alias { base & !0x3e0 } else { base };
                        let (mut context, mut memory) = fixture(&[word]);
                        let source = usize::from(!alias);
                        context.v[0] = u128::MAX;
                        let width = if double { 64 } else { 32 };
                        context.v[source] = (u128::MAX << width) | u128::from(bits);
                        context.fpcr = (3 << 22) | (1 << 24) | (1 << 25);
                        context.fpsr = 0x9f;
                        context.pstate = 0xa000_0000;
                        let before = context.clone();
                        step(&mut context, &mut memory).unwrap();
                        let expected = match base & 0xff3f_fc00 {
                            0x1e20_4000 => bits,
                            0x1e20_c000 => bits & !sign,
                            0x1e21_4000 => bits ^ sign,
                            _ => unreachable!(),
                        };
                        assert_eq!(context.v[0], u128::from(expected));
                        if !alias {
                            assert_eq!(context.v[1], before.v[1]);
                        }
                        assert_eq!(context.fpcr, before.fpcr);
                        assert_eq!(context.fpsr, before.fpsr);
                        assert_eq!(context.pstate, before.pstate);
                        assert_eq!(context.x, before.x);
                        assert_eq!(context.pc, 0x1004);
                    }
                }
            }
        }
    }

    #[test]
    fn unsupported_scalar_unary_precisions_do_not_mutate_context() {
        for word in [
            0x1ea0_4020,
            0x1ee0_4020,
            0x1ea0_c020,
            0x1ee0_c020,
            0x1ea1_4020,
            0x1ee1_4020,
        ] {
            let (mut context, mut memory) = fixture(&[word]);
            context.v[0] = u128::MAX;
            context.v[1] = 42;
            context.fpsr = 0x9f;
            let before = context.clone();
            assert!(
                matches!(step(&mut context, &mut memory), Err(CpuError::Unsupported { raw, .. }) if raw == word)
            );
            assert_eq!(context.v, before.v);
            assert_eq!(context.x, before.x);
            assert_eq!(context.pc, before.pc);
            assert_eq!(context.sp, before.sp);
            assert_eq!(context.fpcr, before.fpcr);
            assert_eq!(context.fpsr, before.fpsr);
            assert_eq!(context.pstate, before.pstate);
            assert_eq!(context.exclusive, before.exclusive);
            assert_eq!(context.tpidr_el0, before.tpidr_el0);
            assert_eq!(context.tpidrro_el0, before.tpidrro_el0);
        }
    }

    #[test]
    fn scalar_division_executes_with_source_destination_aliasing() {
        // Guest FDIV S0,S1,S0, followed by FDIV D0,D1,D0.
        let (mut context, mut memory) = fixture(&[0x1e20_1820, 0x1e60_1820]);
        context.v[0] = (u128::MAX << 32) | u128::from(2f32.to_bits());
        context.v[1] = u128::from(3f32.to_bits());
        context.pstate = 0xa000_0000;
        context.fpsr = 1;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], u128::from(1.5f32.to_bits()));
        context.v[0] = 0;
        context.v[1] = u128::from(1f64.to_bits());
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], u128::from(f64::INFINITY.to_bits()));
        assert_eq!(context.fpsr, 3);
        assert_eq!(context.pstate, 0xa000_0000);
        assert_eq!(context.pc, 0x1008);
    }

    #[test]
    fn scalar_add_and_subtract_execute_in_place_without_changing_nzcv() {
        // Actual FADD S0,S0,S0 boundary, followed by FSUB D0,D0,D1.
        let (mut context, mut memory) = fixture(&[0x1e20_2800, 0x1e61_3800]);
        context.v[0] = u128::from(1.5f32.to_bits());
        context.pstate = 0xa000_0000;
        context.fpsr = 1;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], u128::from(3f32.to_bits()));
        context.v[0] = u128::from(1f64.to_bits());
        context.v[1] = u128::from(1f64.to_bits());
        context.fpcr = 2 << 22;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], u128::from((-0f64).to_bits()));
        assert_eq!(context.fpsr, 1);
        assert_eq!(context.pstate, 0xa000_0000);
        assert_eq!(context.pc, 0x1008);
    }

    #[test]
    fn scalar_sqrt_executes_in_place_and_preserves_non_fp_flags() {
        let (mut context, mut memory) = fixture(&[0x1e21_c000, 0x1e61_c000]);
        context.v[0] = (u128::MAX << 32) | u128::from(4f32.to_bits());
        context.pstate = 0xa000_0000;
        context.fpsr = 1;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], u128::from(2f32.to_bits()));
        context.v[0] = u128::from(2f64.to_bits());
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], u128::from(2f64.sqrt().to_bits()));
        assert_eq!(context.fpsr, 0x11);
        assert_eq!(context.pstate, 0xa000_0000);
        assert_eq!(context.pc, 0x1008);
    }

    #[test]
    fn vector_orr_move_alias_uses_vector_registers_through_decoder() {
        let (mut context, mut memory) = fixture(&[0x4ea8_1d00]);
        context.v[8] = 0x0123456789abcdef_fedcba9876543210;
        context.x[8] = 42;
        context.x[0] = 99;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[0], context.v[8]);
        assert_eq!(context.x[0], 99);
        assert_eq!(context.pc, 0x1004);
    }

    #[test]
    fn float_to_integer_preserves_vectors_and_accumulates_exceptions() {
        // Actual FCVTZU X9,S2 boundary, then FCVTZS W9,S2 and FCVTZU XZR,S2.
        let (mut context, mut memory) = fixture(&[0x9e39_0049, 0x1e38_0049, 0x9e39_005f]);
        context.v[2] = u128::from(3.75f32.to_bits());
        context.fpsr = 1;
        context.pstate = 0xa000_0000;
        context.sp = 0x8000;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.x[9], 3);
        assert_eq!(context.fpsr, 0x11);
        context.v[2] = u128::from((-1.75f32).to_bits());
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.x[9], u64::from(u32::MAX));
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.sp, 0x8000);
        assert_eq!(context.v[2], u128::from((-1.75f32).to_bits()));
        assert_eq!(context.fpsr, 0x11);
        assert_eq!(context.pstate, 0xa000_0000);
    }

    #[test]
    fn scalar_compare_zero_and_signaling_nan_update_only_flags() {
        // FCMP S2,#0.0; FCMPE D1,D0.
        let (mut context, mut memory) = fixture(&[0x1e20_2048, 0x1e60_2030]);
        context.v[2] = u128::from((-1f32).to_bits());
        context.v[1] = 0x7ff8_0000_0000_1234;
        context.pstate = 0x1234;
        context.fpsr = 0x10;
        let vectors = context.v;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.pstate, 0x8000_1234);
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.pstate, 0x3000_1234);
        assert_eq!(context.fpsr, 0x11);
        assert_eq!(context.v, vectors);
        assert_eq!(context.pc, 0x1008);
    }

    #[test]
    fn scalar_multiply_uses_vector_bank_and_accumulates_fp_flags() {
        // FMUL S2,S1,S0; FMUL D2,D1,D0.
        let (mut context, mut memory) = fixture(&[0x1e20_0822, 0x1e60_0822]);
        context.v[0] = u128::from(0.5f32.to_bits());
        context.v[1] = u128::from(3.0f32.to_bits());
        context.v[2] = u128::MAX;
        context.fpsr = 1;
        context.pstate = 0xa000_0000;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[2], u128::from(1.5f32.to_bits()));
        context.v[0] = u128::from(0.5f64.to_bits());
        context.v[1] = 1; // Smallest double subnormal: half rounds to zero.
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.v[2], 0);
        assert_eq!(context.fpsr, 0x19);
        assert_eq!(context.pstate, 0xa000_0000);
        assert_eq!(context.pc, 0x1008);
    }

    #[test]
    fn vector_unsigned_transfers_use_simd_bank_through_decoder() {
        // Actual next guest boundary: STR Q0,[X8]; LDR Q1,[X8].
        let (mut context, mut memory) = fixture(&[0x3d80_0100, 0x3dc0_0101]);
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Data,
            )
            .unwrap();
        context.x[8] = 0x8000;
        context.x[0] = 42;
        context.x[1] = 99;
        context.v[0] = 0x0123456789abcdef_fedcba9876543210;
        let (completed, error) = run(&mut context, &mut memory, 2);
        assert!(error.is_none(), "{error:?}");
        assert_eq!(completed, 2);
        assert_eq!(context.pc, 0x1008);
        assert_eq!(context.v[1], context.v[0]);
        assert_eq!(context.x[0], 42);
        assert_eq!(context.x[1], 99);
    }

    #[test]
    fn barriers_advance_pc_and_isb_fetches_updated_instruction() {
        // DSB ISH; DMB ISH; ISB SY; MOVZ X0,#1.
        let (mut context, mut memory) =
            fixture(&[0xd503_3b9f, 0xd503_3bbf, 0xd503_3fdf, 0xd280_0020]);
        assert_eq!(run(&mut context, &mut memory, 2).0, 2);
        assert_eq!(context.pc, 0x1008);
        // The loader path updates code independently of guest RX permissions.
        memory
            .load_bytes(0x100c, &0xd280_0040u32.to_le_bytes())
            .unwrap();
        assert!(run(&mut context, &mut memory, 2).1.is_none());
        assert_eq!(context.x[0], 2);
        assert_eq!(context.pc, 0x1010);
    }

    #[test]
    fn arithmetic_flags_control_the_branch() {
        // movz x0,#1; subs xzr,x0,#1; b.eq +8; movz x1,#7; nop
        let (mut context, mut memory) =
            fixture(&[0xd2800020, 0xf100041f, 0x54000040, 0xd28000e1, 0xd503201f]);
        assert!(run(&mut context, &mut memory, 4).1.is_none());
        assert_eq!(context.pc, 0x1014);
        assert_eq!(context.x[1], 0);
        assert_eq!(context.pstate & 0xf0000000, 0x60000000);
    }

    #[test]
    fn narrow_writes_zero_extend_and_register_31_is_not_sp() {
        let (mut context, mut memory) = fixture(&[0x12800000, 0xd280003f]);
        context.sp = 0x8000;
        assert!(run(&mut context, &mut memory, 2).1.is_none());
        assert_eq!(context.x[0], 0xffffffff);
        assert_eq!(context.sp, 0x8000);
    }

    #[test]
    fn supervisor_calls_and_unknown_instructions_preserve_pc() {
        let (mut context, mut memory) = fixture(&[0xd4000421]);
        assert!(matches!(
            step(&mut context, &mut memory),
            Err(CpuError::SupervisorCall { number: 0x21, .. })
        ));
        assert_eq!(context.pc, 0x1000);
        memory
            .load_bytes(0x1000, &0xffffffffu32.to_le_bytes())
            .unwrap();
        assert!(matches!(
            step(&mut context, &mut memory),
            Err(CpuError::Unsupported { .. })
        ));
        assert_eq!(context.pc, 0x1000);
    }

    #[test]
    fn fetching_requires_execute_permission() {
        let (mut context, mut memory) = fixture(&[0xd503201f]);
        memory
            .protect(0x1000, PAGE_SIZE, MemoryPermission::READ_WRITE)
            .unwrap();
        assert!(step(&mut context, &mut memory).is_err());
        assert_eq!(context.pc, 0x1000);
    }

    #[test]
    fn signed_immediate_loads_extend_to_selected_register_width() {
        let (mut context, mut memory) = fixture(&[0x39800420, 0x39c00422, 0x79800423, 0xb9800424]);
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Data,
            )
            .unwrap();
        memory
            .write(0x8000, &[0, 0x80, 0, 0x80, 0, 0, 0, 0x80])
            .unwrap();
        context.x[1] = 0x8000;
        assert!(run(&mut context, &mut memory, 4).1.is_none());
        assert_eq!(context.x[0], 0xffffffffffffff80);
        assert_eq!(context.x[2], 0xffffff80);
        assert_eq!(context.x[3], 0xffffffffffff8000);
        assert_eq!(context.x[4], 0xffffffff80000000);
    }

    #[test]
    fn shifted_alu_and_signed_word_extension() {
        // add x2,x0,x1; sxtw x3,w2; and x4,x3,#0xff
        let (mut context, mut memory) = fixture(&[0x8b010002, 0x93407c43, 0x92401c64]);
        context.x[0] = 0x7fffffff;
        context.x[1] = 1;
        assert!(run(&mut context, &mut memory, 3).1.is_none());
        assert_eq!(context.x[2], 0x80000000);
        assert_eq!(context.x[3], 0xffffffff80000000);
        assert_eq!(context.x[4], 0);
    }

    #[test]
    fn register_offset_extends_index_and_loaded_word() {
        // ldrsw x0,[x2,x0,lsl #2]; ldrsw x1,[x2,w3,sxtw #2]
        let (mut context, mut memory) = fixture(&[0xb8a07840, 0xb8a3d841]);
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Stack,
            )
            .unwrap();
        memory.write(0x8000, &0x80000000u32.to_le_bytes()).unwrap();
        context.x[2] = 0x8004;
        context.x[0] = u64::MAX;
        context.x[3] = 0x12345678ffffffff;
        assert!(run(&mut context, &mut memory, 2).1.is_none());
        assert_eq!(context.x[0], 0xffffffff80000000);
        assert_eq!(context.x[1], 0xffffffff80000000);
        assert_eq!(context.x[2], 0x8004);
    }

    #[test]
    fn pair_stack_writeback_and_unscaled_byte_store() {
        // stp x0,x1,[sp,#-16]!; ldp x2,x3,[sp],#16; sturb w1,[x4,#-1]
        let (mut context, mut memory) = fixture(&[0xa9bf07e0, 0xa8c10fe2, 0x381ff081]);
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Stack,
            )
            .unwrap();
        context.sp = 0x9000;
        context.x[0] = 0x1122334455667788;
        context.x[1] = 0x99;
        context.x[4] = 0x8010;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.sp, 0x8ff0);
        assert!(run(&mut context, &mut memory, 2).1.is_none());
        assert_eq!(context.sp, 0x9000);
        assert_eq!(context.x[2], context.x[0]);
        assert_eq!(context.x[3], context.x[1]);
        let mut byte = [0];
        memory.read(0x800f, &mut byte).unwrap();
        assert_eq!(byte, [0x99]);
    }

    #[test]
    fn signed_word_pairs_extend_both_results_in_every_addressing_mode() {
        // Encodings independently assembled with clang for AArch64.
        for (raw, base_register, base, expected_base, second) in [
            (0x6941a660, 19usize, 0x8000, 0x8000, 9usize),
            (0x69ff0440, 2, 0x8014, 0x800c, 1),
            (0x68c107e0, 31, 0x800c, 0x8014, 1),
        ] {
            for (a, b) in [
                (0x80000000u32, 0x7fffffffu32),
                (0xffffffff, 0),
                (1, 0x80000001),
            ] {
                let (mut context, mut memory) = fixture(&[raw]);
                memory
                    .map(
                        0x8000,
                        PAGE_SIZE,
                        MemoryPermission::READ_WRITE,
                        MemoryKind::Stack,
                    )
                    .unwrap();
                memory
                    .write(0x800c, &[a.to_le_bytes(), b.to_le_bytes()].concat())
                    .unwrap();
                if base_register == 31 {
                    context.sp = base;
                } else {
                    context.x[base_register] = base;
                }
                context.x[0] = u64::MAX;
                context.x[second] = u64::MAX;
                step(&mut context, &mut memory).unwrap();
                assert_eq!(context.x[0], a as i32 as i64 as u64);
                assert_eq!(context.x[second], b as i32 as i64 as u64);
                assert_eq!(
                    if base_register == 31 {
                        context.sp
                    } else {
                        context.x[base_register]
                    },
                    expected_base
                );
                assert_eq!(context.pc, 0x1004);
            }
        }
    }

    #[test]
    fn signed_word_pair_zero_register_does_not_change_sp() {
        let (mut context, mut memory) = fixture(&[0x6940045f]);
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        memory
            .write(
                0x8000,
                &[0u32.to_le_bytes(), u32::MAX.to_le_bytes()].concat(),
            )
            .unwrap();
        context.x[2] = 0x8000;
        context.sp = 0x9000;
        step(&mut context, &mut memory).unwrap();
        assert_eq!(context.x[1], u64::MAX);
        assert_eq!(context.sp, 0x9000);
    }

    #[test]
    fn signed_word_pair_faults_and_reserved_forms_preserve_registers() {
        for raw in [0x69ff0440, 0x69bf0440, 0x69400040, 0x69ff0442, 0x68400440] {
            let (mut context, mut memory) = fixture(&[raw]);
            context.x[0] = 17;
            context.x[1] = 18;
            context.x[2] = 0x8004;
            // The first word is mapped, but the second word of a pre-index
            // load crosses into unmapped memory. Neither destination may change.
            memory
                .map(
                    0x7000,
                    PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
            let registers = context.x;
            assert!(step(&mut context, &mut memory).is_err());
            assert_eq!(context.x, registers);
            assert_eq!(context.pc, 0x1000);
        }
    }

    #[test]
    fn faulting_pair_does_not_write_back_stack() {
        let (mut context, mut memory) = fixture(&[0xa9bf07e0]);
        context.sp = 0x9000;
        assert!(step(&mut context, &mut memory).is_err());
        assert_eq!(context.sp, 0x9000);
        assert_eq!(context.pc, 0x1000);
    }
}
