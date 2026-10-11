// SPDX-License-Identifier: GPL-3.0-or-later
//! SIMD/FP memory transfers. Vector and address registers belong to separate banks.

use super::{CpuError, read_register, write_register};
use exosphere_horizon::{memory::AddressSpace, thread::ThreadContext};

pub(super) fn execute(
    context: &mut ThreadContext,
    memory: &mut AddressSpace,
    raw: u32,
) -> Result<bool, CpuError> {
    if raw & 0x3f00_0000 == 0x3d00_0000
        || raw & 0x3f20_0000 == 0x3c00_0000
        || raw & 0x3f20_0c00 == 0x3c20_0800
    {
        single(context, memory, raw)?;
        return Ok(true);
    }
    if raw & 0x3e00_0000 != 0x2c00_0000 {
        return Ok(false);
    }
    let size_code = raw >> 30;
    let load = raw & (1 << 22) != 0;
    let first = (raw & 31) as usize;
    let second = ((raw >> 10) & 31) as usize;
    if size_code == 3 || (load && first == second) {
        return Err(CpuError::Unsupported {
            pc: context.pc,
            raw,
        });
    }
    let size = 4usize << size_code;
    let mode = (raw >> 23) & 3;
    let base_register = ((raw >> 5) & 31) as u8;
    let base = read_register(context, base_register, true);
    let immediate = (((raw >> 15) & 0x7f) as i64) << 57 >> 57;
    let updated = base.wrapping_add_signed(immediate * size as i64);
    let address = if mode == 1 { base } else { updated };
    let mut bytes = [0u8; 32];
    if load {
        // Read the complete pair before changing either register or the base.
        memory.read(address, &mut bytes[..size * 2])?;
        let value = |start| {
            let mut register = [0u8; 16];
            register[..size].copy_from_slice(&bytes[start..start + size]);
            u128::from_le_bytes(register)
        };
        context.v[first] = value(0);
        context.v[second] = value(size);
    } else {
        bytes[..size].copy_from_slice(&context.v[first].to_le_bytes()[..size]);
        bytes[size..size * 2].copy_from_slice(&context.v[second].to_le_bytes()[..size]);
        memory.write(address, &bytes[..size * 2])?;
    }
    if mode == 1 || mode == 3 {
        write_register(context, base_register, updated, true, true);
    }
    Ok(true)
}

fn single(
    context: &mut ThreadContext,
    memory: &mut AddressSpace,
    raw: u32,
) -> Result<(), CpuError> {
    let unsupported = || CpuError::Unsupported {
        pc: context.pc,
        raw,
    };
    let size_code = raw >> 30;
    let operation = (raw >> 22) & 3;
    // Opc<1> selects Q transfers only when size is zero; other combinations
    // are reserved. B/H/S/D use the size field directly.
    if operation >= 2 && size_code != 0 {
        return Err(unsupported());
    }
    let scale = size_code + if operation >= 2 { 4 } else { 0 };
    let size = 1usize << scale;
    let unsigned_offset = raw & (1 << 24) != 0;
    let register_offset = !unsigned_offset && raw & (1 << 21) != 0;
    let mode = if unsigned_offset || register_offset {
        0
    } else {
        (raw >> 10) & 3
    };
    if mode == 2 {
        return Err(unsupported());
    }
    let offset = if unsigned_offset {
        u64::from((raw >> 10) & 0xfff) << scale
    } else if register_offset {
        let value = read_register(context, ((raw >> 16) & 31) as u8, false);
        let extended = match (raw >> 13) & 7 {
            2 => u64::from(value as u32),
            3 | 7 => value,
            6 => value as i32 as i64 as u64,
            _ => return Err(unsupported()),
        };
        extended.wrapping_shl(if raw & (1 << 12) != 0 { scale } else { 0 })
    } else {
        ((((raw >> 12) & 0x1ff) as i64) << 55 >> 55) as u64
    };
    let base_register = ((raw >> 5) & 31) as u8;
    let base = read_register(context, base_register, true);
    let updated = base.wrapping_add(offset);
    let address = if mode == 1 { base } else { updated };
    let target = (raw & 31) as usize;
    if operation & 1 != 0 {
        // Narrow scalar loads zero the remaining vector bits, including upper D.
        let mut bytes = [0; 16];
        memory.read(address, &mut bytes[..size])?;
        context.v[target] = u128::from_le_bytes(bytes);
    } else {
        memory.write(address, &context.v[target].to_le_bytes()[..size])?;
    }
    // Faulting transfers leave the address register unchanged. Matching Rn/Rt
    // numbers are legal here because they refer to different register banks.
    if mode == 1 || mode == 3 {
        write_register(context, base_register, updated, true, true);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_horizon::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};

    fn writable_memory() -> AddressSpace {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Stack,
            )
            .unwrap();
        memory
    }

    #[test]
    fn singles_round_trip_all_widths_and_addressing_modes() {
        let mut memory = writable_memory();
        for scale in 0..=4 {
            let size_code = if scale == 4 { 0 } else { scale };
            let operation = if scale == 4 { 2 } else { 0 };
            let width = 1usize << scale;
            for mode in [0, 1, 3] {
                let mut context = ThreadContext::default();
                context.x[0] = 0x8100;
                context.v[0] = 0x0123456789abcdef_fedcba9876543210;
                let store = 0x3c00_0000
                    | (size_code << 30)
                    | (operation << 22)
                    | (0x1f0 << 12)
                    | (mode << 10);
                execute(&mut context, &mut memory, store).unwrap();
                let address = if mode == 1 { 0x8100 } else { 0x80f0 };
                assert_eq!(context.x[0], if mode == 0 { 0x8100 } else { 0x80f0 });
                let mut actual = [0; 16];
                memory.read(address, &mut actual[..width]).unwrap();
                assert_eq!(&actual[..width], &context.v[0].to_le_bytes()[..width]);
                context.x[0] = address;
                context.v[0] = u128::MAX;
                // Unsigned-offset load with zero offset and matching bank numbers.
                let load = 0x3d40_0000 | (size_code << 30) | (operation << 22);
                execute(&mut context, &mut memory, load).unwrap();
                assert_eq!(context.v[0], u128::from_le_bytes(actual));
                assert_eq!(context.x[0], address);
            }
        }
    }

    #[test]
    fn assembled_single_encodings_cover_stack_and_extended_offsets() {
        let mut memory = writable_memory();
        let mut context = ThreadContext::default();
        context.v[0] = 0x0123456789abcdef_fedcba9876543210;
        context.x[22] = 0x8100;
        // Actual Snipperclips instruction: STR Q0,[X22,#48]! (pre-index).
        execute(&mut context, &mut memory, 0x3c83_0ec0).unwrap();
        assert_eq!(context.x[22], 0x8130);
        context.x[22] = 0x8140;
        context.v[0] = 0;
        execute(&mut context, &mut memory, 0x3cdf_0ec0).unwrap();
        assert_eq!(context.x[22], 0x8130);
        assert_eq!(context.v[0], 0x0123456789abcdef_fedcba9876543210);
        context.sp = 0x8200;
        context.v[3] = 0xab;
        execute(&mut context, &mut memory, 0x3d00_1fe3).unwrap();
        context.x[4] = 0x81fd;
        context.v[4] = u128::MAX;
        execute(&mut context, &mut memory, 0x7d40_1484).unwrap();
        assert_eq!(context.v[4], 0xab);
        context.x[6] = 0x8310;
        context.x[7] = 0xffff_fffe;
        context.v[6] = 0x12345678;
        execute(&mut context, &mut memory, 0xfc27_d8c6).unwrap();
        let mut actual = [0; 8];
        memory.read(0x8300, &mut actual).unwrap();
        assert_eq!(u64::from_le_bytes(actual), 0x12345678);
        context.x[8] = 0x8100;
        context.x[9] = 3;
        execute(&mut context, &mut memory, 0x3ce9_7908).unwrap();
        assert_eq!(context.v[8], context.v[0]);
    }

    #[test]
    fn single_faults_and_reserved_forms_do_not_update_registers_or_memory() {
        let mut memory = writable_memory();
        let mut context = ThreadContext::default();
        context.x[22] = 0x8ff8;
        context.v[0] = u128::MAX;
        for raw in [0x3c83_0ec0, 0x3cdf_06c0, 0x7c83_0ec0, 0x3c83_0ac0] {
            assert!(execute(&mut context, &mut memory, raw).is_err());
            assert_eq!(context.x[22], 0x8ff8);
            assert_eq!(context.v[0], u128::MAX);
        }
        let mut actual = [0; 16];
        memory.read(0x8ff0, &mut actual).unwrap();
        assert_eq!(actual, [0; 16]);
    }

    #[test]
    fn pairs_round_trip_all_widths_with_stack_writeback() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Stack,
            )
            .unwrap();
        for size_code in 0..3 {
            let size = 4u64 << size_code;
            let mut context = ThreadContext {
                sp: 0x9000,
                ..Default::default()
            };
            context.v[0] = 0x0123456789abcdef_fedcba9876543210;
            context.v[1] = !context.v[0];
            let store =
                0x2d00_0000 | (size_code << 30) | (3 << 23) | (126 << 15) | (1 << 10) | (31 << 5);
            execute(&mut context, &mut memory, store).unwrap();
            assert_eq!(context.sp, 0x9000 - size * 2);
            context.v[2] = u128::MAX;
            context.v[3] = u128::MAX;
            let load = 0x2c00_0000
                | (size_code << 30)
                | (1 << 23)
                | (1 << 22)
                | (2 << 15)
                | (3 << 10)
                | (31 << 5)
                | 2;
            execute(&mut context, &mut memory, load).unwrap();
            let mask = u128::MAX >> (128 - size * 8);
            assert_eq!(context.v[2], context.v[0] & mask);
            assert_eq!(context.v[3], context.v[1] & mask);
            assert_eq!(context.sp, 0x9000);
        }
    }

    #[test]
    fn fault_and_reserved_encoding_preserve_context() {
        let mut memory = AddressSpace::default();
        let mut context = ThreadContext {
            sp: 0x9000,
            ..Default::default()
        };
        context.v[0] = 123;
        context.v[1] = 456;
        for raw in [0xadc1_07e0, 0xed00_07e0, 0xad40_03e0] {
            assert!(execute(&mut context, &mut memory, raw).is_err());
            assert_eq!(context.sp, 0x9000);
            assert_eq!(context.v[0], 123);
            assert_eq!(context.v[1], 456);
        }
    }
}
