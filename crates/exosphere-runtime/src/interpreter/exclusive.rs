// SPDX-License-Identifier: GPL-3.0-or-later
//! Single-register and paired exclusives for the serial reference interpreter.
//! Acquire/release accesses execute in program order; parallel backends must supply
//! their own atomic memory implementation rather than borrowing this serial model.

use super::{CpuError, read_register, write_register};
use exosphere_horizon::{
    memory::AddressSpace,
    thread::{ExclusiveReservation, ThreadContext},
};

pub(super) fn execute(
    context: &mut ThreadContext,
    memory: &mut AddressSpace,
    raw: u32,
) -> Result<bool, CpuError> {
    if raw & 0xffff_f0ff == 0xd503_305f {
        context.exclusive = None;
        return Ok(true);
    }
    if raw & 0x3fa0_0000 == 0x0820_0000 {
        return pair(context, memory, raw);
    }
    if matches!(raw & 0x3fff_fc00, 0x08df_fc00 | 0x089f_fc00) {
        let size = 1usize << (raw >> 30);
        let target = (raw & 31) as u8;
        let address = read_register(context, ((raw >> 5) & 31) as u8, true);
        if address % size as u64 != 0 {
            return Err(exosphere_horizon::error::HorizonError::InvalidAddress(address).into());
        }
        if raw & (1 << 22) != 0 {
            let mut bytes = [0u8; 8];
            memory.read(address, &mut bytes[..size])?;
            write_register(context, target, u64::from_le_bytes(bytes), false, size == 8);
        } else {
            memory.write(
                address,
                &read_register(context, target, false).to_le_bytes()[..size],
            )?;
        }
        return Ok(true);
    }
    if raw & 0x3fa0_7c00 != 0x0800_7c00 {
        return Ok(false);
    }
    let load = raw & (1 << 22) != 0;
    let size = 1usize << (raw >> 30);
    let target = (raw & 31) as u8;
    let base = ((raw >> 5) & 31) as u8;
    let status = ((raw >> 16) & 31) as u8;
    if (load && status != 31) || (!load && status != 31 && (status == target || status == base)) {
        return Err(CpuError::Unsupported {
            pc: context.pc,
            raw,
        });
    }
    let address = read_register(context, base, true);
    if address % size as u64 != 0 {
        return Err(exosphere_horizon::error::HorizonError::InvalidAddress(address).into());
    }
    if load {
        let mut bytes = [0u8; 8];
        memory.read(address, &mut bytes[..size])?;
        write_register(context, target, u64::from_le_bytes(bytes), false, size == 8);
        context.exclusive = Some(ExclusiveReservation {
            address,
            size,
            epoch: memory.write_epoch(),
        });
    } else {
        let expected = ExclusiveReservation {
            address,
            size,
            epoch: memory.write_epoch(),
        };
        let success = context.exclusive == Some(expected);
        if success {
            let bytes = read_register(context, target, false).to_le_bytes();
            memory.write(address, &bytes[..size])?;
        }
        context.exclusive = None;
        write_register(context, status, u64::from(!success), false, false);
    }
    Ok(true)
}

fn pair(
    context: &mut ThreadContext,
    memory: &mut AddressSpace,
    raw: u32,
) -> Result<bool, CpuError> {
    let size_code = raw >> 30;
    let load = raw & (1 << 22) != 0;
    let first = (raw & 31) as u8;
    let second = ((raw >> 10) & 31) as u8;
    let base = ((raw >> 5) & 31) as u8;
    let status = ((raw >> 16) & 31) as u8;
    if size_code < 2
        || (load && (status != 31 || first == second))
        || (!load && (status == first || status == second || (status == base && base != 31)))
    {
        return Err(CpuError::Unsupported {
            pc: context.pc,
            raw,
        });
    }
    let element_size = 1usize << size_code;
    let size = element_size * 2;
    let address = read_register(context, base, true);
    if address % size as u64 != 0 {
        return Err(exosphere_horizon::error::HorizonError::InvalidAddress(address).into());
    }
    let reservation = ExclusiveReservation {
        address,
        size,
        epoch: memory.write_epoch(),
    };
    let mut bytes = [0u8; 16];
    if load {
        // Complete the entire read before updating either destination. The serial
        // backend has no intervening writer; parallel backends need a real paired
        // exclusives implementation rather than two independent atomic accesses.
        memory.read(address, &mut bytes[..size])?;
        let value = |start| {
            let mut element = [0; 8];
            element[..element_size].copy_from_slice(&bytes[start..start + element_size]);
            u64::from_le_bytes(element)
        };
        write_register(context, first, value(0), false, element_size == 8);
        write_register(
            context,
            second,
            value(element_size),
            false,
            element_size == 8,
        );
        context.exclusive = Some(reservation);
    } else {
        let success = context.exclusive == Some(reservation);
        if success {
            bytes[..element_size].copy_from_slice(
                &read_register(context, first, false).to_le_bytes()[..element_size],
            );
            bytes[element_size..size].copy_from_slice(
                &read_register(context, second, false).to_le_bytes()[..element_size],
            );
            // One checked write prevents partial pair updates on a memory fault.
            memory.write(address, &bytes[..size])?;
        }
        context.exclusive = None;
        write_register(context, status, u64::from(!success), false, false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_horizon::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};

    #[test]
    fn pair_exclusives_round_trip_both_widths_and_consume_reservations() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Data,
            )
            .unwrap();
        let mut context = ThreadContext::default();
        context.x[0] = 0x8000;
        context.sp = 0x8000;
        for (load, store, width) in [
            (0xc87f_2c0a, 0xc82c_2c0a, 8),
            (0xc87f_ac0a, 0xc82c_ac0a, 8),
            (0x887f_2fea, 0x882c_2fea, 4),
        ] {
            execute(&mut context, &mut memory, load).unwrap();
            assert_eq!(context.exclusive.unwrap().size, width * 2);
            context.x[10] = 0x0123_4567_89ab_cdef;
            context.x[11] = 0xfedc_ba98_7654_3210;
            let first = context.x[10] & (u64::MAX >> (64 - width * 8));
            let second = context.x[11] & (u64::MAX >> (64 - width * 8));
            execute(&mut context, &mut memory, store).unwrap();
            assert_eq!(context.x[12], 0);
            assert!(context.exclusive.is_none());
            execute(&mut context, &mut memory, store).unwrap();
            assert_eq!(context.x[12], 1);
            execute(&mut context, &mut memory, load).unwrap();
            assert_eq!(context.x[10], first);
            assert_eq!(context.x[11], second);
            memory.write(0x8000, &first.to_le_bytes()[..width]).unwrap();
            execute(&mut context, &mut memory, store).unwrap();
            assert_eq!(context.x[12], 1);
        }
    }

    #[test]
    fn faulting_exclusive_stores_preserve_reservation_status_and_memory() {
        let mut memory = AddressSpace::default();
        memory
            .map(0x8000, PAGE_SIZE, MemoryPermission::READ, MemoryKind::Data)
            .unwrap();
        let mut context = ThreadContext::default();
        context.x[0] = 0x8000;
        for (load, store) in [(0xc87f_2c0a, 0xc82c_2c0a), (0xc85f_7c0a, 0xc80c_7c0a)] {
            execute(&mut context, &mut memory, load).unwrap();
            let reservation = context.exclusive;
            context.x[10] = u64::MAX;
            context.x[11] = u64::MAX;
            context.x[12] = 99;
            let epoch = memory.write_epoch();
            assert!(execute(&mut context, &mut memory, store).is_err());
            assert_eq!(context.exclusive, reservation);
            assert_eq!(context.x[12], 99);
            assert_eq!(memory.write_epoch(), epoch);
            let mut bytes = [0; 16];
            memory.read(0x8000, &mut bytes).unwrap();
            assert_eq!(bytes, [0; 16]);
        }
    }

    #[test]
    fn pair_invalid_encodings_alignment_and_load_fault_preserve_context() {
        let mut memory = AddressSpace::default();
        let mut context = ThreadContext::default();
        context.x[0] = 0x8000;
        context.x[10] = 123;
        context.x[11] = 456;
        for raw in [0xc87f_2c0a, 0x487f_2c0a, 0xc87f_280a, 0xc82a_2c0a] {
            assert!(execute(&mut context, &mut memory, raw).is_err());
            assert_eq!(context.x[10], 123);
            assert_eq!(context.x[11], 456);
            assert!(context.exclusive.is_none());
        }
        context.x[0] = 0x8008;
        assert!(execute(&mut context, &mut memory, 0xc87f_2c0a).is_err());
        assert_eq!(context.x[10], 123);
    }

    #[test]
    fn ordered_and_exclusive_accesses_obey_all_widths_and_alignment() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Data,
            )
            .unwrap();
        let mut context = ThreadContext::default();
        context.x[1] = 0x8000;
        context.x[2] = 0x123456789abcdef0;
        for size in 0..4 {
            let width = 8u32 << size;
            execute(&mut context, &mut memory, 0x089ffc22 | (size << 30)).unwrap();
            execute(&mut context, &mut memory, 0x08dffc20 | (size << 30)).unwrap();
            let expected = context.x[2] & (u64::MAX >> (64 - width));
            assert_eq!(context.x[0], expected);
            execute(&mut context, &mut memory, 0x085f7c20 | (size << 30)).unwrap();
            execute(&mut context, &mut memory, 0x08037c22 | (size << 30)).unwrap();
            assert_eq!(context.x[3], 0);
        }
        context.x[1] = 0x8001;
        let previous = context.x[0];
        assert!(execute(&mut context, &mut memory, 0x885ffc20).is_err());
        assert_eq!(context.x[0], previous);
    }

    #[test]
    fn exclusive_store_succeeds_once_and_same_value_writes_invalidate_it() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Data,
            )
            .unwrap();
        let mut context = ThreadContext::default();
        context.x[1] = 0x8000;
        context.x[2] = 123;
        execute(&mut context, &mut memory, 0x885ffc20).unwrap(); // ldaxr w0,[x1]
        execute(&mut context, &mut memory, 0x8803fc22).unwrap(); // stlxr w3,w2,[x1]
        assert_eq!(context.x[3], 0);
        execute(&mut context, &mut memory, 0x8803fc22).unwrap();
        assert_eq!(context.x[3], 1);
        execute(&mut context, &mut memory, 0x885f7c20).unwrap();
        assert_eq!(context.x[0], 123);
        memory.write(0x8000, &123u32.to_le_bytes()).unwrap();
        execute(&mut context, &mut memory, 0x88037c22).unwrap();
        assert_eq!(context.x[3], 1);
        execute(&mut context, &mut memory, 0x885f7c20).unwrap();
        execute(&mut context, &mut memory, 0xd5033f5f).unwrap();
        assert!(context.exclusive.is_none());
    }
}
