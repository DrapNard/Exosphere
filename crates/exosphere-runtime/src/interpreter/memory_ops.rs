// SPDX-License-Identifier: GPL-3.0-or-later
//! Integer loads/stores with signed immediate addressing and register pairs.

use super::{CpuError, read_register, write_register};
use exosphere_horizon::{memory::AddressSpace, thread::ThreadContext};

pub(super) fn execute(
    context: &mut ThreadContext,
    memory: &mut AddressSpace,
    raw: u32,
) -> Result<bool, CpuError> {
    if raw & 0x3f20_0000 == 0x3800_0000 || raw & 0x3f20_0c00 == 0x3820_0800 {
        single(context, memory, raw)?;
        return Ok(true);
    }
    if raw & 0x3e00_0000 == 0x2800_0000 {
        pair(context, memory, raw)?;
        return Ok(true);
    }
    Ok(false)
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
    let size = 1usize << (raw >> 30);
    let operation = (raw >> 22) & 3;
    let register_offset = raw & (1 << 21) != 0;
    let mode = if register_offset { 0 } else { (raw >> 10) & 3 };
    let target = (raw & 31) as u8;
    let base_register = ((raw >> 5) & 31) as u8;
    let writeback = mode == 1 || mode == 3;
    if mode == 2
        || (size == 8 && operation > 1)
        || (size == 4 && operation == 3)
        || (writeback && base_register != 31 && base_register == target)
    {
        return Err(unsupported());
    }
    let immediate = if register_offset {
        let offset = read_register(context, ((raw >> 16) & 31) as u8, false);
        let extended = match (raw >> 13) & 7 {
            2 => offset as u32 as u64,        // UXTW
            3 => offset,                      // LSL / UXTX
            6 => offset as i32 as i64 as u64, // SXTW
            7 => offset,                      // SXTX: same 64-bit two's-complement bits
            _ => return Err(unsupported()),
        };
        let shift = if raw & (1 << 12) != 0 { raw >> 30 } else { 0 };
        extended.wrapping_shl(shift) as i64
    } else {
        (((raw >> 12) & 0x1ff) as i64) << 55 >> 55
    };
    let base = read_register(context, base_register, true);
    let updated = base.wrapping_add_signed(immediate);
    let address = if mode == 1 { base } else { updated };
    let mut bytes = [0u8; 8];
    if operation == 0 {
        bytes = read_register(context, target, false).to_le_bytes();
        memory.write(address, &bytes[..size])?;
    } else {
        memory.read(address, &mut bytes[..size])?;
        let mut value = u64::from_le_bytes(bytes);
        if operation >= 2 {
            let shift = 64 - size * 8;
            value = ((value << shift) as i64 >> shift) as u64;
        }
        write_register(context, target, value, false, size == 8 || operation == 2);
    }
    // A faulting memory access must not update the base register.
    if writeback {
        write_register(context, base_register, updated, true, true);
    }
    Ok(())
}

fn pair(context: &mut ThreadContext, memory: &mut AddressSpace, raw: u32) -> Result<(), CpuError> {
    let unsupported = || CpuError::Unsupported {
        pc: context.pc,
        raw,
    };
    let load = raw & (1 << 22) != 0;
    let signed_words = raw >> 30 == 1 && load;
    let size = match raw >> 30 {
        0 => 4,
        1 if load => 4,
        2 => 8,
        _ => return Err(unsupported()),
    };
    let mode = (raw >> 23) & 3;
    let first = (raw & 31) as u8;
    let second = ((raw >> 10) & 31) as u8;
    let base_register = ((raw >> 5) & 31) as u8;
    let writeback = mode == 1 || mode == 3;
    if mode == 0
        || (load && first == second)
        || (writeback && base_register != 31 && (base_register == first || base_register == second))
    {
        return Err(unsupported());
    }
    let immediate = (((raw >> 15) & 0x7f) as i64) << 57 >> 57;
    let base = read_register(context, base_register, true);
    let updated = base.wrapping_add_signed(immediate * size as i64);
    let address = if mode == 1 { base } else { updated };
    let mut bytes = [0u8; 16];
    if load {
        memory.read(address, &mut bytes[..size * 2])?;
        let read_value = |start| {
            let mut value = [0u8; 8];
            value[..size].copy_from_slice(&bytes[start..start + size]);
            let value = u64::from_le_bytes(value);
            if signed_words {
                value as i32 as i64 as u64
            } else {
                value
            }
        };
        // LDPSW reads two words but writes sign-extended X registers.
        write_register(
            context,
            first,
            read_value(0),
            false,
            size == 8 || signed_words,
        );
        write_register(
            context,
            second,
            read_value(size),
            false,
            size == 8 || signed_words,
        );
    } else {
        bytes[..size].copy_from_slice(&read_register(context, first, false).to_le_bytes()[..size]);
        bytes[size..size * 2]
            .copy_from_slice(&read_register(context, second, false).to_le_bytes()[..size]);
        memory.write(address, &bytes[..size * 2])?;
    }
    if writeback {
        write_register(context, base_register, updated, true, true);
    }
    Ok(())
}
