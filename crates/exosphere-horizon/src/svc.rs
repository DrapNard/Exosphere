// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! SVC decoding/dispatch boundary.
//!
//! Numeric IDs are intentionally centralized here. Service behavior can later be selected by the
//! firmware profile DB without forcing CPU backends to understand Horizon kernel implementation.

use crate::{
    error::{HorizonError, HorizonResult},
    handle::Handle,
    kernel::HorizonKernel,
};

mod shared;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvcCall {
    SetHeapSize,
    SetMemoryPermission,
    SetMemoryAttribute,
    MapMemory,
    UnmapMemory,
    QueryMemory,
    ExitProcess,
    CreateThread,
    StartThread,
    ExitThread,
    SleepThread,
    GetThreadPriority,
    SetThreadPriority,
    CreateTransferMemory,
    MapSharedMemory,
    UnmapSharedMemory,
    GetThreadCoreMask,
    SetThreadCoreMask,
    ArbitrateLock,
    ArbitrateUnlock,
    WaitProcessWideKeyAtomic,
    SignalProcessWideKey,
    CloseHandle,
    WaitSynchronization,
    ConnectToNamedPort,
    SendSyncRequest,
    GetThreadId,
    GetProcessId,
    Break,
    OutputDebugString,
    GetInfo,
    Unknown(u16),
}

#[cfg(test)]
mod entropy_tests {
    use super::*;
    use crate::HorizonProfileId;

    #[test]
    fn guest_close_handle_returns_invalid_handle_without_closing_other_objects() {
        use crate::handle::KernelObject;
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "caller");
        let object = KernelObject::Custom {
            kind: "fixture".into(),
            id: 7,
        };
        let live = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(object.clone())
            .unwrap();
        for invalid in [0, 1, 0xffff8000, 0xffff8001, u32::MAX] {
            let mut args = SvcArguments::default();
            args.x[0] = u64::from(invalid);
            let reply = dispatch_guest(&mut kernel, pid, 0, 0x16, args).unwrap();
            assert_eq!(reply.result_code, 0xe401);
            assert_eq!(
                kernel.process(pid).unwrap().handles.get(live).unwrap(),
                &object
            );
            assert_eq!(kernel.process(pid).unwrap().handles.objects().count(), 1);
        }
        let mut args = SvcArguments::default();
        args.x[0] = u64::from(live.0);
        assert_eq!(
            dispatch_guest(&mut kernel, pid, 0, 0x16, args)
                .unwrap()
                .result_code,
            0
        );
        let replacement = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(object.clone())
            .unwrap();
        assert_ne!(replacement, live);
        assert_eq!(
            dispatch_guest(&mut kernel, pid, 0, 0x16, args)
                .unwrap()
                .result_code,
            0xe401
        );
        assert_eq!(
            kernel
                .process(pid)
                .unwrap()
                .handles
                .get(replacement)
                .unwrap(),
            &object
        );
        assert!(matches!(
            dispatch(&mut kernel, pid, 0, 0x16, args),
            Err(HorizonError::InvalidHandle(_))
        ));
        assert!(dispatch_guest(&mut kernel, pid + 99, 0, 0x16, args).is_err());
    }

    #[test]
    fn priority_updates_validate_grants_and_reorder_ready_threads_only() {
        use crate::{
            handle::KernelObject, process::ProcessSchedulingCapabilities, thread::ThreadState,
        };
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "caller");
        kernel.process_mut(pid).unwrap().scheduling = Some(ProcessSchedulingCapabilities {
            core_mask: 7,
            priority_mask: u64::MAX << 24,
        });
        let (first, handle) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let (second, _) = kernel.create_thread(pid, 0x1000, 0x9000, 32).unwrap();
        kernel.start_thread(first).unwrap();
        kernel.start_thread(second).unwrap();
        let mut args = SvcArguments::default();
        args.x[0] = u64::from(handle.0);
        args.x[1] = 24;
        assert_eq!(
            dispatch(&mut kernel, pid, first, 0x0d, args)
                .unwrap()
                .result_code,
            0
        );
        assert_eq!(kernel.scheduler.next(), Some(first));
        assert_eq!(kernel.scheduler.next(), Some(second));
        assert_eq!(kernel.thread(first).unwrap().priority, 24);
        for priority in [-1, 0, 3, 23, 64, i32::MAX] {
            args.x[1] = priority as u32 as u64;
            assert!(dispatch(&mut kernel, pid, first, 0x0d, args).is_err());
            assert_eq!(kernel.thread(first).unwrap().priority, 24);
            assert!(kernel.scheduler.is_empty());
        }
        kernel.thread_mut(first).unwrap().state = ThreadState::Waiting;
        args.x[0] = 0xffff8000;
        args.x[1] = 63;
        dispatch(&mut kernel, pid, first, 0x0d, args).unwrap();
        assert_eq!(kernel.thread(first).unwrap().state, ThreadState::Waiting);
        assert!(kernel.scheduler.is_empty());
        let other = kernel.create_process(2, "other");
        let (foreign, _) = kernel.create_thread(other, 0x1000, 0x9000, 44).unwrap();
        let foreign_handle = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::Thread(foreign))
            .unwrap();
        args.x[0] = u64::from(foreign_handle.0);
        assert!(matches!(
            dispatch(&mut kernel, pid, first, 0x0d, args),
            Err(HorizonError::InvalidHandle(_))
        ));
        kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .close(handle)
            .unwrap();
        args.x[0] = u64::from(handle.0);
        assert!(dispatch(&mut kernel, pid, first, 0x0d, args).is_err());
        args.x[0] = 0xffff8000;
        kernel.process_mut(pid).unwrap().scheduling = None;
        assert!(dispatch(&mut kernel, pid, first, 0x0d, args).is_err());
        assert_eq!(kernel.thread(first).unwrap().priority, 63);
    }

    #[test]
    fn thread_affinity_is_retained_and_checked_against_process_grants() {
        use crate::{
            handle::KernelObject, process::ProcessSchedulingCapabilities, thread::ThreadAffinity,
        };
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "caller");
        let process = kernel.process_mut(pid).unwrap();
        process.scheduling = Some(ProcessSchedulingCapabilities {
            core_mask: 7,
            priority_mask: u64::MAX << 24,
        });
        process.ideal_core = Some(1);
        assert!(kernel.create_thread(pid, 0x1000, 0x9000, 23).is_err());
        assert!(kernel.process(pid).unwrap().thread_ids.is_empty());
        let (tid, handle) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        assert_eq!(
            kernel.thread(tid).unwrap().affinity,
            Some(ThreadAffinity {
                ideal_core: 1,
                mask: 2
            })
        );
        let mut args = SvcArguments::default();
        for (core, mask, expected) in [
            (0, 3, (0, 3)),
            (-3, 5, (0, 5)),
            (-2, u64::MAX, (1, 2)),
            (-1, 7, (-1, 7)),
        ] {
            args.x[..3].copy_from_slice(&[u64::from(handle.0), core as u32 as u64, mask]);
            assert_eq!(
                dispatch(&mut kernel, pid, tid, 0x0f, args)
                    .unwrap()
                    .result_code,
                0
            );
            args.x[2] = 0xffff8000;
            let reply = dispatch(&mut kernel, pid, tid, 0x0e, args).unwrap();
            assert_eq!(reply.x[1], u64::from(expected.0 as u32));
            assert_eq!(reply.x[2], expected.1);
        }
        for (core, mask) in [(0, 0), (0, 8), (3, 7), (1, 1), (-4, 7), (64, 1), (-3, 7)] {
            let before = kernel.thread(tid).unwrap().affinity;
            args.x[..3].copy_from_slice(&[0xffff8000, core as u32 as u64, mask]);
            assert!(dispatch(&mut kernel, pid, tid, 0x0f, args).is_err());
            assert_eq!(kernel.thread(tid).unwrap().affinity, before);
        }
        let other = kernel.create_process(2, "other");
        let (other_tid, _) = kernel.create_thread(other, 0x1000, 0x9000, 44).unwrap();
        let foreign = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::Thread(other_tid))
            .unwrap();
        args.x[..3].copy_from_slice(&[u64::from(foreign.0), 0, 1]);
        assert!(matches!(
            dispatch(&mut kernel, pid, tid, 0x0f, args),
            Err(HorizonError::InvalidHandle(_))
        ));
        kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .close(handle)
            .unwrap();
        args.x[0] = u64::from(handle.0);
        assert!(dispatch(&mut kernel, pid, tid, 0x0f, args).is_err());
    }

    #[test]
    fn thread_priority_resolves_handles_and_rejects_stale_handles() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, handle) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let mut args = SvcArguments::default();
        for value in [u64::from(handle.0), 0xffff8000] {
            args.x[1] = value;
            assert_eq!(
                dispatch(&mut kernel, pid, tid, 0x0c, args).unwrap().x[1],
                44
            );
        }
        kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .close(handle)
            .unwrap();
        args.x[1] = u64::from(handle.0);
        assert!(dispatch(&mut kernel, pid, tid, 0x0c, args).is_err());
    }

    #[test]
    fn scheduling_info_resolves_process_grants_and_rejects_invalid_queries() {
        use crate::{handle::KernelObject, process::ProcessSchedulingCapabilities};
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "caller");
        let other = kernel.create_process(2, "target");
        kernel.process_mut(pid).unwrap().scheduling = Some(ProcessSchedulingCapabilities {
            core_mask: 1,
            priority_mask: 1 << 44,
        });
        kernel.process_mut(other).unwrap().scheduling = Some(ProcessSchedulingCapabilities {
            core_mask: 6,
            priority_mask: u64::MAX << 24,
        });
        let handle = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::Process(other))
            .unwrap();
        let mut args = SvcArguments::default();
        for (target, expected) in [
            (0xffff8001, [1, 1 << 44]),
            (u64::from(handle.0), [6, u64::MAX << 24]),
        ] {
            args.x[2] = target;
            for info_type in 0..=1 {
                args.x[1] = info_type;
                assert_eq!(
                    dispatch(&mut kernel, pid, 0, 0x29, args).unwrap().x[1],
                    expected[info_type as usize]
                );
            }
        }
        args.x[3] = 1;
        assert!(dispatch(&mut kernel, pid, 0, 0x29, args).is_err());
        args.x[3] = 0;
        kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .close(handle)
            .unwrap();
        assert!(dispatch(&mut kernel, pid, 0, 0x29, args).is_err());
        args.x[2] = 0xffff8001;
        kernel.process_mut(pid).unwrap().scheduling = None;
        assert!(dispatch(&mut kernel, pid, 0, 0x29, args).is_err());
    }

    #[test]
    fn layout_info_uses_the_selected_process_and_validates_handle_and_subtype() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(0x1234, "test");
        let layout = crate::layout::ProcessLayout::diagnostic_39_bit(
            0x7100000000,
            0x3000000,
            0x7f00000000,
            0x40000,
        )
        .unwrap();
        kernel.process_mut(pid).unwrap().layout = Some(layout);
        let mut args = SvcArguments::default();
        args.x[2] = 0xffff8001;
        for (info_type, expected) in [
            (2, layout.alias.base),
            (3, layout.alias.size),
            (4, layout.heap.base),
            (5, layout.heap.size),
            (12, layout.aslr.base),
            (13, layout.aslr.size),
            (14, layout.stack.base),
            (15, layout.stack.size),
            (18, 0x1234),
        ] {
            args.x[1] = info_type;
            let result = dispatch(&mut kernel, pid, 0, 0x29, args).unwrap();
            assert_eq!(result.result_code, 0);
            assert_eq!(result.x[1], expected);
        }
        args.x[3] = 1;
        assert!(dispatch(&mut kernel, pid, 0, 0x29, args).is_err());
        args.x[3] = 0;
        args.x[2] = 0;
        assert!(dispatch(&mut kernel, pid, 0, 0x29, args).is_err());
    }

    #[test]
    fn entropy_dispatch_rejects_wrong_handle_subtype_and_info_type() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        let mut args = SvcArguments::default();
        args.x[1] = 11;
        args.x[2] = 1;
        assert!(matches!(
            dispatch(&mut kernel, pid, 0, 0x29, args),
            Err(HorizonError::InvalidHandle(1))
        ));
        args.x[2] = 0;
        args.x[3] = 4;
        assert!(dispatch(&mut kernel, pid, 0, 0x29, args).is_err());
        args.x[3] = 0;
        args.x[1] = 12;
        assert!(dispatch(&mut kernel, pid, 0, 0x29, args).is_err());
        args.x[1] = 11;
        let first = dispatch(&mut kernel, pid, 0, 0x29, args).unwrap();
        let second = dispatch(&mut kernel, pid, 0, 0x29, args).unwrap();
        assert_eq!(first.result_code, 0);
        assert_eq!(first.x, second.x);
    }
}

impl From<u16> for SvcCall {
    fn from(value: u16) -> Self {
        match value {
            0x01 => Self::SetHeapSize,
            0x02 => Self::SetMemoryPermission,
            0x03 => Self::SetMemoryAttribute,
            0x04 => Self::MapMemory,
            0x05 => Self::UnmapMemory,
            0x06 => Self::QueryMemory,
            0x07 => Self::ExitProcess,
            0x08 => Self::CreateThread,
            0x09 => Self::StartThread,
            0x0A => Self::ExitThread,
            0x0B => Self::SleepThread,
            0x0C => Self::GetThreadPriority,
            0x0D => Self::SetThreadPriority,
            0x15 => Self::CreateTransferMemory,
            0x13 => Self::MapSharedMemory,
            0x14 => Self::UnmapSharedMemory,
            0x0E => Self::GetThreadCoreMask,
            0x0F => Self::SetThreadCoreMask,
            0x1D => Self::SignalProcessWideKey,
            0x16 => Self::CloseHandle,
            0x18 => Self::WaitSynchronization,
            0x1a => Self::ArbitrateLock,
            0x1b => Self::ArbitrateUnlock,
            0x1c => Self::WaitProcessWideKeyAtomic,
            0x1F => Self::ConnectToNamedPort,
            0x21 => Self::SendSyncRequest,
            0x25 => Self::GetThreadId,
            0x24 => Self::GetProcessId,
            0x26 => Self::Break,
            0x27 => Self::OutputDebugString,
            0x29 => Self::GetInfo,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SvcArguments {
    pub x: [u64; 8],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SvcResult {
    pub result_code: u32,
    pub x: [u64; 4],
}

/// Guest-facing transport uses the mapped TLS buffer. Direct dispatch retains the
/// synthetic-buffer interface used by independent kernel smoke tests.
pub fn dispatch_guest(
    kernel: &mut HorizonKernel,
    process_id: u64,
    thread_id: u64,
    raw: u16,
    args: SvcArguments,
) -> HorizonResult<SvcResult> {
    if raw == 0x16 {
        let handle = Handle(args.x[0] as u32);
        // Invalid/stale user handles are ordinary Horizon results. Validate
        // before closing so failures in object cleanup remain host errors.
        match kernel.process(process_id)?.handles.get(handle) {
            Ok(_) => {}
            Err(HorizonError::InvalidHandle(_)) => {
                return Ok(SvcResult {
                    result_code: 0xe401,
                    ..Default::default()
                });
            }
            Err(error) => return Err(error),
        }
        kernel.close_handle(process_id, handle)?;
        return Ok(SvcResult::default());
    }
    if raw == 0x1c {
        let result_code = kernel.wait_condition(
            process_id,
            thread_id,
            args.x[0],
            args.x[1] & !3,
            args.x[2] as u32,
            args.x[3] as i64,
        )?;
        return Ok(SvcResult {
            result_code,
            ..Default::default()
        });
    }
    if raw == 0x1d {
        kernel.signal_process_wide_key(process_id, args.x[0] & !3, args.x[1] as u32 as i32)?;
        return Ok(SvcResult::default());
    }
    if raw == 0x1a || raw == 0x1b {
        let result_code = if raw == 0x1a {
            kernel.arbitrate_lock(
                process_id,
                thread_id,
                args.x[0] as u32,
                args.x[1],
                args.x[2] as u32,
            )?
        } else {
            kernel.arbitrate_unlock(process_id, thread_id, args.x[0])?
        };
        return Ok(SvcResult {
            result_code,
            ..Default::default()
        });
    }
    if raw == 0x09 {
        let mut out = SvcResult::default();
        let target = match kernel
            .process(process_id)?
            .handles
            .get(Handle(args.x[0] as u32))
        {
            Ok(crate::handle::KernelObject::Thread(id)) => *id,
            _ => {
                out.result_code = 0xe401;
                return Ok(out);
            }
        };
        let thread = kernel.thread(target)?;
        if thread.process_id != process_id {
            out.result_code = 0xe401;
        } else if thread.state != crate::thread::ThreadState::Created {
            out.result_code = 0xfa01;
        } else {
            kernel.start_thread(target)?;
        }
        return Ok(out);
    }
    if raw == 0x08 {
        // The wrapper's output pointer in x0 is not a kernel input.
        let priority = args.x[4] as u32 as i32;
        let requested_core = args.x[5] as u32 as i32;
        let process = kernel.process(process_id)?;
        let grants = process.scheduling.ok_or_else(|| {
            HorizonError::InvalidState("thread scheduling grants are not configured".into())
        })?;
        let core = if requested_core == -2 {
            i32::from(process.ideal_core.ok_or_else(|| {
                HorizonError::InvalidState("process default core is not configured".into())
            })?)
        } else {
            requested_core
        };
        let mut out = SvcResult::default();
        if !(0..4).contains(&core) || grants.core_mask & (1u64 << core) == 0 {
            out.result_code = 0xe201;
            return Ok(out);
        }
        if !(4..=63).contains(&priority) || grants.priority_mask & (1u64 << priority) == 0 {
            out.result_code = 0xe001;
            return Ok(out);
        }
        let parent = kernel.thread(thread_id)?;
        let (fpcr, fpsr) = (parent.context.fpcr, parent.context.fpsr);
        let (created, handle) = kernel.create_thread_on_core(
            process_id,
            args.x[1],
            args.x[3],
            priority,
            Some(core as u8),
        )?;
        let child = kernel.thread_mut(created)?;
        child.context.x[0] = args.x[2];
        child.context.fpcr = fpcr;
        child.context.fpsr = fpsr;
        out.x[1] = u64::from(handle.0);
        return Ok(out);
    }
    if raw == 0x21 {
        kernel.send_guest_sync_request(process_id, thread_id, Handle(args.x[0] as u32))?;
        return Ok(SvcResult::default());
    }
    dispatch(kernel, process_id, thread_id, raw, args)
}

pub fn dispatch(
    kernel: &mut HorizonKernel,
    process_id: u64,
    thread_id: u64,
    raw: u16,
    args: SvcArguments,
) -> HorizonResult<SvcResult> {
    match SvcCall::from(raw) {
        SvcCall::MapSharedMemory | SvcCall::UnmapSharedMemory => {
            shared::dispatch(kernel, process_id, raw, args)
        }
        SvcCall::MapMemory | SvcCall::UnmapMemory => {
            use crate::memory::PAGE_SIZE;
            let (destination, source, size) = (args.x[0], args.x[1], args.x[2]);
            let process = kernel.process_mut(process_id)?;
            let layout = process.layout.ok_or_else(|| {
                HorizonError::InvalidState("stack mapping layout is not configured".into())
            })?;
            let mut result = SvcResult::default();
            if destination % PAGE_SIZE != 0 || source % PAGE_SIZE != 0 {
                result.result_code = 0xcc01;
            } else if size == 0 || size % PAGE_SIZE != 0 {
                result.result_code = 0xca01;
            } else if source
                .checked_add(size)
                .is_none_or(|end| end > layout.address_limit)
                || destination
                    .checked_add(size)
                    .is_none_or(|end| end > layout.address_limit)
            {
                result.result_code = 0xd401;
            } else if !layout.stack.contains(destination, size) {
                result.result_code = 0xdc01;
            } else {
                let mapping = if raw == 0x04 {
                    process.memory.map_stack_alias(destination, source, size)
                } else {
                    process.memory.unmap_stack_alias(destination, source, size)
                };
                match mapping {
                    Ok(()) => {}
                    Err(
                        HorizonError::InvalidAddress(_)
                        | HorizonError::MemoryOverlap
                        | HorizonError::PermissionDenied,
                    ) => result.result_code = 0xd401,
                    Err(error) => return Err(error),
                }
            }
            Ok(result)
        }
        SvcCall::SetHeapSize => {
            let size = args.x[1];
            let process = kernel.process_mut(process_id)?;
            let layout = process.layout.ok_or_else(|| {
                HorizonError::InvalidState("heap layout is not configured".into())
            })?;
            let mut result = SvcResult::default();
            result.x[1] = layout.heap.base;
            if size % 0x200000 != 0 || size > 0x100000000 {
                result.result_code = 0xca01;
            } else if size > layout.heap.size {
                result.result_code = 0xd001;
            } else {
                if process.memory.memory_limit().is_none() {
                    return Err(HorizonError::InvalidState(
                        "heap memory quota is not configured".into(),
                    ));
                }
                match process.memory.resize_heap(layout.heap.base, size) {
                    Ok(()) => {}
                    Err(HorizonError::MemoryOverlap) => result.result_code = 0xd401,
                    Err(HorizonError::InvalidState(_)) => result.result_code = 0x10801,
                    Err(error) => return Err(error),
                }
            }
            Ok(result)
        }
        SvcCall::WaitSynchronization => {
            let count = args.x[2] as u32 as i32;
            if !(0..=64).contains(&count) {
                return Err(HorizonError::InvalidState(
                    "wait count must be 0..64".into(),
                ));
            }
            let mut handles = Vec::with_capacity(count as usize);
            for index in 0..count as u64 {
                let address = args.x[1]
                    .checked_add(index * 4)
                    .ok_or(HorizonError::InvalidAddress(args.x[1]))?;
                let mut bytes = [0; 4];
                kernel
                    .process(process_id)?
                    .memory
                    .read(address, &mut bytes)?;
                handles.push(Handle(u32::from_le_bytes(bytes)));
            }
            let (result_code, index) =
                kernel.wait_synchronization(process_id, thread_id, &handles, args.x[3] as i64)?;
            let mut result = SvcResult {
                result_code,
                ..Default::default()
            };
            result.x[1] = index as u64;
            Ok(result)
        }
        SvcCall::ConnectToNamedPort => {
            let mut bytes = Vec::new();
            let mut terminated = false;
            for offset in 0..12 {
                let address = args.x[1]
                    .checked_add(offset)
                    .ok_or(HorizonError::InvalidAddress(args.x[1]))?;
                let mut byte = [0];
                kernel
                    .process(process_id)?
                    .memory
                    .read(address, &mut byte)?;
                if byte[0] == 0 {
                    terminated = true;
                    break;
                }
                bytes.push(byte[0]);
            }
            if !terminated {
                return Err(HorizonError::InvalidState(
                    "named port exceeds 11 bytes".into(),
                ));
            }
            let name = std::str::from_utf8(&bytes)
                .map_err(|_| HorizonError::InvalidState("invalid named port encoding".into()))?;
            // sm: is the only registered named port. Services are obtained through
            // its IPC interface, not by treating every service as a named port.
            if name != "sm:" {
                return Err(HorizonError::UnknownService(name.into()));
            }
            let handle = kernel.connect_service(process_id, name)?;
            let mut result = SvcResult::default();
            result.x[1] = u64::from(handle.0);
            Ok(result)
        }
        SvcCall::SignalProcessWideKey => {
            kernel.signal_process_wide_key(process_id, args.x[0], args.x[1] as i32)?;
            Ok(SvcResult::default())
        }
        SvcCall::GetThreadCoreMask | SvcCall::SetThreadCoreMask => {
            let setting = matches!(SvcCall::from(raw), SvcCall::SetThreadCoreMask);
            let handle = if setting { args.x[0] } else { args.x[2] } as u32;
            let target = if handle == 0xffff8000 {
                if kernel.thread(thread_id)?.process_id != process_id {
                    return Err(HorizonError::InvalidHandle(handle));
                }
                thread_id
            } else {
                match kernel.process(process_id)?.handles.get(Handle(handle))? {
                    crate::handle::KernelObject::Thread(id) => *id,
                    _ => return Err(HorizonError::InvalidHandle(handle)),
                }
            };
            if !setting {
                let affinity = kernel.thread(target)?.affinity.ok_or_else(|| {
                    HorizonError::InvalidState("thread affinity is not configured".into())
                })?;
                let mut out = SvcResult::default();
                out.x[1] = u64::from(affinity.ideal_core as u32);
                out.x[2] = affinity.mask;
                return Ok(out);
            }
            if kernel.thread(target)?.process_id != process_id {
                return Err(HorizonError::InvalidHandle(handle));
            }
            let process = kernel.process(process_id)?;
            let grants = process.scheduling.ok_or_else(|| {
                HorizonError::InvalidState("process scheduling grants are not configured".into())
            })?;
            let requested_core = args.x[1] as i32;
            // -2 uses the NPDM default and ignores the supplied mask. -3 keeps
            // the thread's ideal core, but the new mask must still include it.
            let (core, mask) = match requested_core {
                -2 => {
                    let core = process.ideal_core.ok_or_else(|| {
                        HorizonError::InvalidState("process ideal core is not configured".into())
                    })?;
                    if core >= 4 {
                        return Err(HorizonError::InvalidState(
                            "invalid process ideal core".into(),
                        ));
                    }
                    (i32::from(core), 1u64 << core)
                }
                -3 => {
                    let old = kernel.thread(target)?.affinity.ok_or_else(|| {
                        HorizonError::InvalidState("thread affinity is not configured".into())
                    })?;
                    if old.ideal_core < 0 {
                        return Err(HorizonError::InvalidState(
                            "no concrete ideal core to preserve".into(),
                        ));
                    }
                    (old.ideal_core, args.x[2])
                }
                core => (core, args.x[2]),
            };
            if mask == 0
                || mask & !grants.core_mask != 0
                || !(-1..=3).contains(&core)
                || (core >= 0 && mask & (1u64 << core) == 0)
            {
                return Err(HorizonError::InvalidState(
                    "invalid thread core/affinity combination".into(),
                ));
            }
            kernel.thread_mut(target)?.affinity = Some(crate::thread::ThreadAffinity {
                ideal_core: core,
                mask,
            });
            Ok(SvcResult::default())
        }
        SvcCall::CreateTransferMemory => {
            use crate::memory::MemoryPermission;
            let permission = match args.x[3] as u32 {
                0 => MemoryPermission::NONE,
                1 => MemoryPermission::READ,
                3 => MemoryPermission::READ_WRITE,
                _ => return Err(HorizonError::PermissionDenied),
            };
            let handle =
                kernel.create_transfer_memory(process_id, args.x[1], args.x[2], permission)?;
            let mut out = SvcResult::default();
            out.x[1] = u64::from(handle.0);
            Ok(out)
        }
        SvcCall::SetThreadPriority => {
            let handle = args.x[0] as u32;
            let target = if handle == 0xffff8000 {
                thread_id
            } else {
                match kernel.process(process_id)?.handles.get(Handle(handle))? {
                    crate::handle::KernelObject::Thread(id) => *id,
                    _ => return Err(HorizonError::InvalidHandle(handle)),
                }
            };
            if kernel.thread(target)?.process_id != process_id {
                return Err(HorizonError::InvalidHandle(handle));
            }
            let priority = args.x[1] as i32;
            let grants = kernel.process(process_id)?.scheduling.ok_or_else(|| {
                HorizonError::InvalidState("process scheduling grants are not configured".into())
            })?;
            if !(4..=63).contains(&priority) || grants.priority_mask & (1u64 << priority) == 0 {
                return Err(HorizonError::InvalidState(
                    "thread priority exceeds process scheduling grants".into(),
                ));
            }
            kernel.set_thread_base_priority(target, priority)?;
            Ok(SvcResult::default())
        }
        SvcCall::GetThreadPriority => {
            let handle = args.x[1] as u32;
            let target = if handle == 0xffff8000 {
                thread_id
            } else {
                match kernel.process(process_id)?.handles.get(Handle(handle))? {
                    crate::handle::KernelObject::Thread(id) => *id,
                    _ => return Err(HorizonError::InvalidHandle(handle)),
                }
            };
            let mut out = SvcResult::default();
            out.x[1] = kernel.thread(target)?.priority as u32 as u64;
            Ok(out)
        }
        SvcCall::GetInfo => {
            let info_type = args.x[1] as u32;
            let value = if info_type == 11 {
                // Entropy queries use a zero handle, not the current-process pseudo-handle.
                if args.x[2] as u32 != 0 {
                    return Err(HorizonError::InvalidHandle(args.x[2] as u32));
                }
                kernel.process_mut(process_id)?.random_entropy(args.x[3])?
            } else if matches!(info_type, 0..=7 | 12..=15 | 18) && args.x[3] == 0 {
                let handle = args.x[2] as u32;
                let target = if handle == 0xffff8001 {
                    process_id
                } else {
                    match kernel.process(process_id)?.handles.get(Handle(handle))? {
                        crate::handle::KernelObject::Process(id) => *id,
                        _ => return Err(HorizonError::InvalidHandle(handle)),
                    }
                };
                let process = kernel.process(target)?;
                if info_type <= 1 {
                    let scheduling = process.scheduling.ok_or_else(|| {
                        HorizonError::InvalidState(
                            "process scheduling capabilities are not configured".into(),
                        )
                    })?;
                    if info_type == 0 {
                        scheduling.core_mask
                    } else {
                        scheduling.priority_mask
                    }
                } else if info_type == 6 {
                    process.memory.memory_limit().ok_or_else(|| {
                        HorizonError::InvalidState("process memory quota is not configured".into())
                    })?
                } else if info_type == 7 {
                    process.used_memory_size()?
                } else if info_type == 18 {
                    process.program_id
                } else {
                    let layout = process.layout.ok_or_else(|| {
                        HorizonError::InvalidState("process layout not configured".into())
                    })?;
                    match info_type {
                        2 => layout.alias.base,
                        3 => layout.alias.size,
                        4 => layout.heap.base,
                        5 => layout.heap.size,
                        12 => layout.aslr.base,
                        13 => layout.aslr.size,
                        14 => layout.stack.base,
                        15 => layout.stack.size,
                        _ => unreachable!(),
                    }
                }
            } else {
                return Err(HorizonError::InvalidState(format!(
                    "GetInfo type {} subtype {} is not implemented",
                    args.x[1] as u32, args.x[3]
                )));
            };
            let mut out = SvcResult::default();
            out.x[1] = value;
            Ok(out)
        }
        SvcCall::QueryMemory => {
            let memory = &mut kernel.process_mut(process_id)?.memory;
            // The current loader supports the 39-bit AArch64 process layout only.
            let (base, size, region) = memory.query(args.x[2], 1 << 39)?;
            let mut bytes = [0u8; 0x28];
            bytes[..8].copy_from_slice(&base.to_le_bytes());
            bytes[8..16].copy_from_slice(&size.to_le_bytes());
            if let Some(region) = region {
                use crate::memory::{MemoryKind, MemoryPermission};
                let memory_type: u32 = match region.kind {
                    MemoryKind::Code if region.permission.contains(MemoryPermission::EXECUTE) => 3,
                    MemoryKind::Code | MemoryKind::Data => 4,
                    MemoryKind::Heap => 5,
                    MemoryKind::Shared => 6,
                    MemoryKind::Alias => 7,
                    MemoryKind::Stack => 11,
                    MemoryKind::ThreadLocal => 12,
                    // These states need additional metadata before guest exposure.
                    MemoryKind::Device | MemoryKind::Other => {
                        return Err(HorizonError::InvalidState(
                            "unclassified guest memory state".into(),
                        ));
                    }
                };
                let (effective, attributes) = memory
                    .transfer_metadata(args.x[2])
                    .or_else(|| memory.stack_source_metadata(args.x[2]))
                    .unwrap_or((region.permission, 0));
                let permission: u32 = u32::from(effective.contains(MemoryPermission::READ))
                    | (u32::from(effective.contains(MemoryPermission::WRITE)) << 1)
                    | (u32::from(effective.contains(MemoryPermission::EXECUTE)) << 2);
                bytes[0x10..0x14].copy_from_slice(&memory_type.to_le_bytes());
                bytes[0x14..0x18].copy_from_slice(&attributes.to_le_bytes());
                bytes[0x18..0x1c].copy_from_slice(&permission.to_le_bytes());
            } else if args.x[2] >= 1 << 39 {
                bytes[0x10..0x14].copy_from_slice(&0x10u32.to_le_bytes());
            }
            memory.write(args.x[0], &bytes)?;
            Ok(SvcResult::default())
        }
        SvcCall::CloseHandle => {
            kernel.close_handle(process_id, Handle(args.x[0] as u32))?;
            Ok(SvcResult::default())
        }
        SvcCall::GetThreadId => {
            let mut out = SvcResult::default();
            out.x[1] = thread_id;
            Ok(out)
        }
        SvcCall::GetProcessId => {
            let mut out = SvcResult::default();
            out.x[1] = process_id;
            Ok(out)
        }
        SvcCall::ExitThread => {
            kernel.exit_thread(thread_id)?;
            Ok(SvcResult::default())
        }
        SvcCall::ExitProcess => {
            kernel.exit_process(process_id)?;
            Ok(SvcResult::default())
        }
        SvcCall::CreateThread => {
            let entry = args.x[0];
            let argument = args.x[1];
            let stack_top = args.x[2];
            let priority = i32::try_from(args.x[3]).map_err(|_| {
                HorizonError::InvalidState("thread priority does not fit i32".into())
            })?;
            let (created_thread, handle) =
                kernel.create_thread(process_id, entry, stack_top, priority)?;
            kernel.thread_mut(created_thread)?.context.x[0] = argument;
            let mut out = SvcResult::default();
            out.x[1] = u64::from(handle.0);
            Ok(out)
        }
        SvcCall::StartThread => {
            let handle = Handle(args.x[0] as u32);
            let thread = match kernel.process(process_id)?.handles.get(handle)? {
                crate::handle::KernelObject::Thread(id) => *id,
                _ => return Err(HorizonError::InvalidHandle(handle.0)),
            };
            kernel.start_thread(thread)?;
            Ok(SvcResult::default())
        }
        SvcCall::SleepThread => {
            kernel.thread_mut(thread_id)?.state = crate::thread::ThreadState::Waiting;
            Ok(SvcResult::default())
        }
        SvcCall::SendSyncRequest => {
            kernel.send_sync_request(process_id, thread_id, Handle(args.x[0] as u32))?;
            Ok(SvcResult::default())
        }
        SvcCall::Unknown(id) => Err(HorizonError::UnknownSvc(id)),
        other => Err(HorizonError::InvalidState(format!(
            "SVC {other:?} is decoded but not implemented yet"
        ))),
    }
}
