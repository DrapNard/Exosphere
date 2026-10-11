// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared-memory SVC ABI and admission checks, before any mapping mutation.

use super::*;
use crate::{
    handle::KernelObject,
    memory::{MemoryPermission, PAGE_SIZE},
};

pub(super) fn dispatch(
    kernel: &mut HorizonKernel,
    pid: u64,
    raw: u16,
    args: SvcArguments,
) -> HorizonResult<SvcResult> {
    let (address, size) = (args.x[1], args.x[2]);
    let result = |result_code| {
        Ok(SvcResult {
            result_code,
            ..Default::default()
        })
    };
    if address % PAGE_SIZE != 0 {
        return result(0xcc01);
    }
    if size == 0 || size % PAGE_SIZE != 0 {
        return result(0xca01);
    }
    let Some(end) = address.checked_add(size) else {
        return result(0xd401);
    };
    let permission = if raw == 0x13 {
        match args.x[3] as u32 {
            1 => MemoryPermission::READ,
            3 => MemoryPermission::READ_WRITE,
            _ => return result(0xce01),
        }
    } else {
        MemoryPermission::NONE
    };
    let process = kernel.process_mut(pid)?;
    let memory = match process.handles.get(Handle(args.x[0] as u32)) {
        Ok(KernelObject::SharedMemory(memory)) => memory.clone(),
        _ => return result(0xe401),
    };
    let layout = process.layout.ok_or_else(|| {
        HorizonError::InvalidState("shared memory layout is not configured".into())
    })?;
    // Shared memory uses the ASLR region, excluding heap and alias reservations.
    let overlaps = |region: crate::layout::VirtualRegion| {
        address < region.base + region.size && end > region.base
    };
    if !layout.aslr.contains(address, size) || overlaps(layout.heap) || overlaps(layout.alias) {
        return result(0xdc01);
    }
    if size != memory.size() as u64 {
        return result(0xca01);
    }
    if raw == 0x13 && permission != memory.guest_permission() {
        return result(0xd801);
    }
    let operation = if raw == 0x13 {
        process.memory.map_shared(address, &memory, permission)
    } else {
        process.memory.unmap_shared(address, &memory)
    };
    match operation {
        Ok(()) => result(0),
        Err(
            HorizonError::InvalidAddress(_)
            | HorizonError::MemoryOverlap
            | HorizonError::PermissionDenied,
        ) => result(0xd401),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_unmap_validate_permission_size_identity_and_retained_bytes() {
        let mut kernel = HorizonKernel::new(crate::HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "shared");
        let layout =
            crate::layout::ProcessLayout::diagnostic_39_bit(0x7100000000, 4096, 0x7f00000000, 4096)
                .unwrap();
        kernel.process_mut(pid).unwrap().layout = Some(layout);
        let memory =
            crate::shared_memory::SharedMemory::with_guest_permission(4096, MemoryPermission::READ)
                .unwrap();
        let handle = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::SharedMemory(memory.clone()))
            .unwrap();
        let mut args = SvcArguments {
            x: [handle.0 as u64, 0x1800000000, 4096, 1, 0, 0, 0, 0],
        };
        for (index, value, code) in [
            (1, 1, 0xcc01),
            (2, 0, 0xca01),
            (3, 2, 0xce01),
            (3, 3, 0xd801),
            (2, 8192, 0xca01),
            (1, layout.heap.base, 0xdc01),
            (0, 0, 0xe401),
        ] {
            let mut invalid = args;
            invalid.x[index] = value;
            assert_eq!(
                dispatch(&mut kernel, pid, 0x13, invalid)
                    .unwrap()
                    .result_code,
                code
            );
            assert!(kernel.process(pid).unwrap().memory.regions().is_empty());
        }
        assert_eq!(
            dispatch(&mut kernel, pid, 0x13, args).unwrap().result_code,
            0
        );
        assert_eq!(
            dispatch(&mut kernel, pid, 0x13, args).unwrap().result_code,
            0xd401
        );
        memory.write(0, &[42]).unwrap();
        let mut byte = [0];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(args.x[1], &mut byte)
            .unwrap();
        assert_eq!(byte, [42]);
        let wrong = crate::shared_memory::SharedMemory::new(4096).unwrap();
        let wrong = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::SharedMemory(wrong))
            .unwrap();
        args.x[0] = wrong.0 as u64;
        assert_eq!(
            dispatch(&mut kernel, pid, 0x14, args).unwrap().result_code,
            0xd401
        );
        args.x[0] = handle.0 as u64;
        assert_eq!(
            dispatch(&mut kernel, pid, 0x14, args).unwrap().result_code,
            0
        );
        assert!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(args.x[1], &mut byte)
                .is_err()
        );
    }
}
