// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded guest execution with explicit Horizon SVC boundaries.

use crate::interpreter::{self, CpuError};
use exosphere_horizon::{
    HorizonKernel,
    svc::{self, SvcArguments},
    thread::ThreadState,
};

fn poll_execution_services(
    kernel: &mut HorizonKernel,
    process_id: u64,
) -> exosphere_horizon::HorizonResult<()> {
    kernel.poll_primary_npad(process_id)?;
    kernel.poll_gpu_process(process_id, 64)?;
    Ok(())
}

/// Executes one thread until its budget, a fault, or a kernel state transition.
/// This is not yet a multi-thread scheduler: waiting/exited threads yield immediately.
pub fn run_thread(
    kernel: &mut HorizonKernel,
    process_id: u64,
    thread_id: u64,
    budget: usize,
) -> Result<(usize, Option<CpuError>), CpuError> {
    if kernel.thread(thread_id)?.process_id != process_id {
        return Err(exosphere_horizon::error::HorizonError::InvalidState(
            "thread belongs to another process".into(),
        )
        .into());
    }
    kernel.poll_synchronization()?;
    if budget > 0 {
        // Waiting threads still allow service input sampling at runner polls.
        if let Err(error) = poll_execution_services(kernel, process_id) {
            return Ok((0, Some(CpuError::Memory(error))));
        }
    }
    let mut context = kernel.thread(thread_id)?.context.clone();
    let mut completed = 0;
    let stop = loop {
        if completed == budget
            || matches!(
                kernel.thread(thread_id)?.state,
                ThreadState::Waiting | ThreadState::Exited
            )
        {
            break None;
        }
        // Bound slices so host input publication remains observable in guest
        // polling loops even when they execute no SVC. No historical inputs are
        // synthesized to catch up after a long-running slice.
        if let Err(error) = poll_execution_services(kernel, process_id) {
            // Publish the CPU context/count reached before this asynchronous
            // service fault, using the same exit path as instruction faults.
            break Some(CpuError::Memory(error));
        }
        let (executed, stop) = interpreter::run(
            &mut context,
            &mut kernel.process_mut(process_id)?.memory,
            (budget - completed).min(4096),
        );
        completed += executed;
        match stop {
            None => continue,
            Some(CpuError::CounterRead {
                register,
                frequency,
                ..
            }) => {
                let value = if frequency {
                    exosphere_horizon::time::SYSTEM_COUNTER_FREQUENCY
                } else {
                    kernel.time.system_counter()
                };
                if register != 31 {
                    context.x[register as usize] = value;
                }
                // MRS is not an exception: retain flags and exclusive monitors.
                context.pc = context.pc.wrapping_add(4);
                completed += 1;
            }
            Some(CpuError::SupervisorCall { number, .. }) => {
                // Exception entry clears the local exclusive monitor.
                context.exclusive = None;
                // Older synthetic dispatch paths are not all guest-ABI compliant yet.
                // Enable only audited calls instead of exposing their test conventions.
                if !matches!(
                    number,
                    0x01 | 0x04
                        | 0x05
                        | 0x06
                        | 0x07
                        | 0x08
                        | 0x09
                        | 0x0a
                        | 0x0c
                        | 0x0d
                        | 0x0e
                        | 0x0f
                        | 0x13
                        | 0x14
                        | 0x15
                        | 0x16
                        | 0x18
                        | 0x1a
                        | 0x1b
                        | 0x1c
                        | 0x1d
                        | 0x1f
                        | 0x21
                        | 0x29
                ) {
                    break Some(CpuError::SupervisorCall {
                        pc: context.pc,
                        number,
                    });
                }
                // Publish the context before dispatch so kernel operations see current registers.
                kernel.thread_mut(thread_id)?.context = context.clone();
                let mut args = SvcArguments::default();
                args.x.copy_from_slice(&context.x[..8]);
                match svc::dispatch_guest(kernel, process_id, thread_id, number, args) {
                    Ok(result) => {
                        context = kernel.thread(thread_id)?.context.clone();
                        if matches!(
                            number,
                            0x01 | 0x04
                                | 0x05
                                | 0x06
                                | 0x08
                                | 0x09
                                | 0x0c
                                | 0x0d
                                | 0x0e
                                | 0x0f
                                | 0x13
                                | 0x14
                                | 0x15
                                | 0x16
                                | 0x18
                                | 0x1a
                                | 0x1b
                                | 0x1c
                                | 0x1f
                                | 0x21
                                | 0x29
                        ) {
                            context.x[0] = u64::from(result.result_code);
                        }
                        if matches!(
                            number,
                            0x01 | 0x06 | 0x08 | 0x0c | 0x0e | 0x15 | 0x18 | 0x1f | 0x29
                        ) {
                            context.x[1] = result.x[1];
                        }
                        if number == 0x0e {
                            context.x[2] = result.x[2];
                        }
                        context.pc = context.pc.wrapping_add(4);
                        completed += 1;
                        // A newly ready thread must be considered before this
                        // caller can spin waiting for it to initialize.
                        if number == 0x09
                            || (matches!(number, 0x1b | 0x1d) && !kernel.scheduler.is_empty())
                        {
                            break None;
                        }
                    }
                    Err(error) => break Some(CpuError::Memory(error)),
                }
            }
            Some(error) => break Some(error),
        }
    };
    kernel.thread_mut(thread_id)?.context = context;
    Ok((completed, stop))
}

#[derive(Debug)]
pub struct ProcessRun {
    pub completed: usize,
    pub last_thread: Option<u64>,
    pub stop: Option<CpuError>,
    /// No ready thread remains; this is suspension, not successful game boot.
    pub idle: bool,
}

/// Serial, bounded execution of the process's ready threads. Priority and FIFO
/// ties come from Horizon; each slice preserves the complete architectural state.
/// This does not implement physical multicore execution or timed idle waiting.
pub fn run_process(
    kernel: &mut HorizonKernel,
    process_id: u64,
    budget: usize,
) -> Result<ProcessRun, CpuError> {
    const QUANTUM: usize = 100_000;
    let mut result = ProcessRun {
        completed: 0,
        last_thread: None,
        stop: None,
        idle: false,
    };
    while result.completed < budget {
        kernel.poll_synchronization()?;
        if let Err(error) = poll_execution_services(kernel, process_id) {
            result.stop = Some(CpuError::Memory(error));
            break;
        }
        let ids = kernel.process(process_id)?.thread_ids.clone();
        let Some(thread_id) = kernel.scheduler.next_matching(|id| ids.contains(&id)) else {
            result.idle = true;
            break;
        };
        if kernel.thread(thread_id)?.state != ThreadState::Ready {
            continue;
        }
        result.last_thread = Some(thread_id);
        let thread = kernel.thread_mut(thread_id)?;
        thread.state = ThreadState::Running;
        // Context switching is an exception boundary for local reservations.
        thread.context.exclusive = None;
        let (completed, stop) = run_thread(
            kernel,
            process_id,
            thread_id,
            QUANTUM.min(budget - result.completed),
        )?;
        result.completed += completed;
        let thread = kernel.thread_mut(thread_id)?;
        if thread.state == ThreadState::Running {
            thread.state = ThreadState::Ready;
            let priority = thread.priority;
            kernel.scheduler.enqueue_with_priority(thread_id, priority);
        }
        if stop.is_some() {
            result.stop = stop;
            break;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_horizon::{
        HorizonProfileId,
        memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
    };

    #[test]
    fn guest_close_invalid_or_stale_handle_returns_result_and_continues() {
        use exosphere_horizon::handle::KernelObject;
        for stale in [false, true] {
            let (mut kernel, pid, tid) = fixture();
            let object = KernelObject::Custom {
                kind: "fixture".into(),
                id: 7,
            };
            let original = kernel
                .process_mut(pid)
                .unwrap()
                .handles
                .insert(object.clone())
                .unwrap();
            kernel.close_handle(pid, original).unwrap();
            let live = kernel
                .process_mut(pid)
                .unwrap()
                .handles
                .insert(object.clone())
                .unwrap();
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .load_bytes(0x1000, &0xd40002c1u32.to_le_bytes())
                .unwrap(); // svc #0x16
            let context = &mut kernel.thread_mut(tid).unwrap().context;
            context.x = [0x1234; 31];
            context.x[0] = if stale { u64::from(original.0) } else { 1 };
            context.v = [u128::MAX; 32];
            context.pstate = 0xa0000040;
            context.fpcr = 0x100;
            context.fpsr = 0x10;
            let before = context.clone();
            let (completed, stop) = run_thread(&mut kernel, pid, tid, 2).unwrap();
            assert_eq!(completed, 2);
            assert!(stop.is_none());
            let after = &kernel.thread(tid).unwrap().context;
            assert_eq!(after.x[0], 0xe401);
            assert_eq!(after.x[1..], before.x[1..]);
            assert_eq!(after.v, before.v);
            assert_eq!(
                (after.pstate, after.fpcr, after.fpsr, after.sp),
                (before.pstate, before.fpcr, before.fpsr, before.sp)
            );
            assert_eq!(after.pc, 0x1008);
            assert_eq!(
                kernel.process(pid).unwrap().handles.get(live).unwrap(),
                &object
            );
        }
    }

    #[test]
    fn guest_arm64_reads_automatically_published_npad_from_native_hid_resource() {
        use exosphere_horizon::{
            handle::Handle,
            ipc::{IpcBuffer, IpcBufferDirection, ServiceRequest},
            layout::ProcessLayout,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        struct Clock(Arc<AtomicU64>);
        impl exosphere_horizon::time::TimeSource for Clock {
            fn monotonic_ns(&self) -> u64 {
                self.0.load(Ordering::Relaxed)
            }
            fn unix_seconds(&self) -> i64 {
                0
            }
        }
        fn send(
            kernel: &mut HorizonKernel,
            pid: u64,
            tid: u64,
            handle: Handle,
            request: &[u8],
        ) -> Vec<u8> {
            let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
            let memory = &mut kernel.process_mut(pid).unwrap().memory;
            memory
                .load_bytes(0x1000, &0xd4000421u32.to_le_bytes())
                .unwrap();
            memory.write(tls, request).unwrap();
            let context = &mut kernel.thread_mut(tid).unwrap().context;
            context.pc = 0x1000;
            context.x[0] = u64::from(handle.0);
            let (count, stop) = run_thread(kernel, pid, tid, 1).unwrap();
            assert_eq!(count, 1);
            assert!(stop.is_none());
            assert_eq!(kernel.thread(tid).unwrap().context.x[0], 0);
            let mut response = vec![0; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut response)
                .unwrap();
            response
        }
        let (mut kernel, pid, tid) = fixture();
        kernel.profile = HorizonProfileId("22.1.0".into());
        let clock = Arc::new(AtomicU64::new(0));
        kernel.time = exosphere_horizon::time::TimeManager::new(Box::new(Clock(clock.clone())));
        kernel.process_mut(pid).unwrap().layout = Some(
            ProcessLayout::diagnostic_39_bit(0x0071_0000_0000, 4096, 0x007f_0000_0000, 4096)
                .unwrap(),
        );
        let identity = kernel.process(pid).unwrap().application.resource_user_id;
        // Configure policy through registered dispatch; resource factories and
        // mapping below go through actual interpreted guest SVC instructions.
        let mut policy = ServiceRequest {
            service: "hid".into(),
            command_id: 103,
            values: Vec::new(),
            raw_payload: identity.to_le_bytes().to_vec(),
            buffers: Vec::new(),
        };
        kernel.services.call(&kernel.profile, &policy).unwrap();
        policy.command_id = 100;
        policy.raw_payload = [1u64.to_le_bytes(), identity.to_le_bytes()].concat();
        kernel.services.call(&kernel.profile, &policy).unwrap();
        policy.command_id = 102;
        policy.raw_payload = identity.to_le_bytes().to_vec();
        policy.buffers = vec![IpcBuffer {
            address: 0,
            capacity: 4,
            direction: IpcBufferDirection::In,
            data: 0u32.to_le_bytes().to_vec(),
        }];
        kernel.services.call(&kernel.profile, &policy).unwrap();
        kernel
            .hid
            .set_primary_npad(exosphere_horizon::hid::NpadState {
                connected: true,
                buttons: 3,
                ..Default::default()
            });
        let root = kernel.connect_service(pid, "hid").unwrap();
        let mut request = [0; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&(12u32 | 0x80000000).to_le_bytes());
        request[8..12].copy_from_slice(&1u32.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        request[48..56].copy_from_slice(&identity.to_le_bytes());
        let response = send(&mut kernel, pid, tid, root, &request);
        let child = Handle(
            exosphere_horizon::hipc::parse(&response)
                .unwrap()
                .handles
                .move_handles[0],
        );
        request = [0; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        let response = send(&mut kernel, pid, tid, child, &request);
        let shared = exosphere_horizon::hipc::parse(&response)
            .unwrap()
            .handles
            .copy_handles[0];
        let base = 0x0018_0000_0000;
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xd4000261u32.to_le_bytes())
            .unwrap();
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.pc = 0x1000;
        context.x[..4].copy_from_slice(&[u64::from(shared), base, 0x40000, 1]);
        let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(count, 1);
        assert!(stop.is_none());
        assert_eq!(kernel.thread(tid).unwrap().context.x[0], 0);
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xf9400085u32.to_le_bytes())
            .unwrap(); // ldr x5, [x4]
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.pc = 0x1000;
        context.x[4] = base + 0x9a28 + 0x20 + 8 + 8;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(count, 1);
        assert!(stop.is_none());
        assert_eq!(kernel.thread(tid).unwrap().context.x[5], 3);
        kernel
            .hid
            .set_primary_npad(exosphere_horizon::hid::NpadState {
                connected: true,
                buttons: 1,
                ..Default::default()
            });
        clock.store(8_000_000, Ordering::Relaxed);
        let idle = run_process(&mut kernel, pid, 1).unwrap();
        assert!(idle.idle);
        assert_eq!(idle.completed, 0);
        assert!(idle.stop.is_none());
        // No guest instruction ran during idle polling, but its next LDR must
        // observe the new sample through the already-mapped memory.
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.pc = 0x1000;
        context.x[4] += 0x30;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(count, 1);
        assert!(stop.is_none());
        assert_eq!(kernel.thread(tid).unwrap().context.x[5], 1);
    }

    #[test]
    fn architectural_counter_uses_kernel_clock_and_preserves_exclusives() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        struct Clock(Arc<AtomicU64>);
        impl exosphere_horizon::time::TimeSource for Clock {
            fn monotonic_ns(&self) -> u64 {
                self.0.load(Ordering::Relaxed)
            }
            fn unix_seconds(&self) -> i64 {
                0
            }
        }
        let (mut kernel, pid, tid) = fixture();
        let nanos = Arc::new(AtomicU64::new(1_000_000_000));
        kernel.time = exosphere_horizon::time::TimeManager::new(Box::new(Clock(nanos.clone())));
        let words = [0xd53b_e000u32, 0xd53b_e021, 0xd53b_e022, 0xd53b_e03f];
        let bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &bytes)
            .unwrap();
        let reservation = exosphere_horizon::thread::ExclusiveReservation {
            address: 0x8000,
            size: 8,
            epoch: 1,
        };
        let thread = kernel.thread_mut(tid).unwrap();
        thread.context.exclusive = Some(reservation);
        thread.context.pstate = 0xa000_0000;
        let sp = thread.context.sp;
        assert_eq!(run_thread(&mut kernel, pid, tid, 2).unwrap().0, 2);
        nanos.store(2_000_000_000, Ordering::Relaxed);
        assert_eq!(run_thread(&mut kernel, pid, tid, 2).unwrap().0, 2);
        let context = &kernel.thread(tid).unwrap().context;
        assert_eq!(&context.x[..3], &[19_200_000, 19_200_000, 38_400_000]);
        assert_eq!(context.pc, 0x1010);
        assert_eq!(context.sp, sp);
        assert_eq!(context.pstate, 0xa000_0000);
        assert_eq!(context.exclusive, Some(reservation));
    }

    #[test]
    fn create_transfer_guest_abi_returns_handle_and_locks_heap_range() {
        use exosphere_horizon::{
            handle::{Handle, KernelObject},
            memory::MemoryPermission,
        };
        let (mut kernel, pid, tid) = fixture();
        let process = kernel.process_mut(pid).unwrap();
        process
            .memory
            .map(
                0x4000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        process
            .memory
            .load_bytes(0x1000, &0xd40002a1u32.to_le_bytes())
            .unwrap();
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.x[..5].copy_from_slice(&[0x8000, 0x4000, PAGE_SIZE, 0, 77]);
        let (completed, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(completed, 1);
        assert!(stop.is_none());
        let context = &kernel.thread(tid).unwrap().context;
        let handle = Handle(context.x[1] as u32);
        assert_eq!(context.x[0], 0);
        assert_eq!(&context.x[2..5], &[PAGE_SIZE, 0, 77]);
        assert_eq!(context.pc, 0x1004);
        assert!(
            matches!(kernel.process(pid).unwrap().handles.get(handle).unwrap(), KernelObject::TransferMemory { owner, base: 0x4000, size: PAGE_SIZE } if *owner == pid)
        );
        assert!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(0x4000, &mut [0])
                .is_err()
        );
        kernel.close_handle(pid, handle).unwrap();
        kernel
            .process(pid)
            .unwrap()
            .memory
            .validate_access(0x4000, 1, MemoryPermission::READ_WRITE)
            .unwrap();
    }

    #[test]
    fn set_priority_guest_svc_updates_policy_and_preserves_argument_registers() {
        use exosphere_horizon::process::ProcessSchedulingCapabilities;
        let (mut kernel, pid, tid) = fixture();
        let process = kernel.process_mut(pid).unwrap();
        process.scheduling = Some(ProcessSchedulingCapabilities {
            core_mask: 7,
            priority_mask: u64::MAX << 24,
        });
        process
            .memory
            .load_bytes(0x1000, &0xd40001a1u32.to_le_bytes())
            .unwrap();
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.x[..4].copy_from_slice(&[0xffff8000, 24, 0x1234, 77]);
        context.pstate = 0xa0000000;
        let (completed, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(completed, 1);
        assert!(stop.is_none());
        let thread = kernel.thread(tid).unwrap();
        assert_eq!(thread.priority, 24);
        assert_eq!(&thread.context.x[..4], &[0, 24, 0x1234, 77]);
        assert_eq!(thread.context.pc, 0x1004);
        assert_eq!(thread.context.pstate, 0xa0000000);
    }

    #[test]
    fn thread_affinity_svc_register_abi_preserves_unrelated_registers() {
        use exosphere_horizon::{process::ProcessSchedulingCapabilities, thread::ThreadAffinity};
        let (mut kernel, pid, tid) = fixture();
        let process = kernel.process_mut(pid).unwrap();
        process.scheduling = Some(ProcessSchedulingCapabilities {
            core_mask: 7,
            priority_mask: u64::MAX << 24,
        });
        process.ideal_core = Some(0);
        // SetThreadCoreMask, then GetThreadCoreMask.
        let words = [0xd40001e1u32, 0xd40001c1];
        let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        process.memory.load_bytes(0x1000, &bytes).unwrap();
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.x[..4].copy_from_slice(&[0xffff8000, 1, 3, 77]);
        context.pstate = 0xa0000000;
        let (completed, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(completed, 1);
        assert!(stop.is_none());
        assert_eq!(
            kernel.thread(tid).unwrap().affinity,
            Some(ThreadAffinity {
                ideal_core: 1,
                mask: 3
            })
        );
        assert_eq!(&kernel.thread(tid).unwrap().context.x[..4], &[0, 1, 3, 77]);
        kernel.thread_mut(tid).unwrap().context.x[2] = 0xffff8000;
        let (completed, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(completed, 1);
        assert!(stop.is_none());
        let context = &kernel.thread(tid).unwrap().context;
        assert_eq!(&context.x[..4], &[0, 1, 3, 77]);
        assert_eq!(context.pc, 0x1008);
        assert_eq!(context.pstate, 0xa0000000);
    }

    #[test]
    fn scheduling_get_info_resumes_through_guest_register_abi() {
        use exosphere_horizon::process::ProcessSchedulingCapabilities;
        for (info_type, expected) in [(0, 7), (1, u64::MAX << 24)] {
            let (mut kernel, pid, tid) = fixture();
            let process = kernel.process_mut(pid).unwrap();
            process.scheduling = Some(ProcessSchedulingCapabilities {
                core_mask: 7,
                priority_mask: u64::MAX << 24,
            });
            process
                .memory
                .load_bytes(0x1000, &0xd4000521u32.to_le_bytes())
                .unwrap();
            let thread = kernel.thread_mut(tid).unwrap();
            thread.context.x[..5].copy_from_slice(&[0x8000, info_type, 0xffff8001, 0, 77]);
            thread.context.pstate = 0xa0000000;
            let (completed, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
            assert_eq!(completed, 1);
            assert!(stop.is_none());
            let context = &kernel.thread(tid).unwrap().context;
            assert_eq!(&context.x[..5], &[0, expected, 0xffff8001, 0, 77]);
            assert_eq!(context.pc, 0x1004);
            assert_eq!(context.pstate, 0xa0000000);
        }
    }

    #[test]
    fn heap_svc_returns_fixed_base_and_preserves_unrelated_registers() {
        let (mut kernel, pid, tid) = fixture();
        let process = kernel.process_mut(pid).unwrap();
        process.layout = Some(
            exosphere_horizon::layout::ProcessLayout::diagnostic_39_bit(
                0x7100000000,
                PAGE_SIZE,
                0x7f00000000,
                PAGE_SIZE,
            )
            .unwrap(),
        );
        process.memory.set_memory_limit(8 << 20).unwrap();
        process
            .memory
            .load_bytes(0x1000, &0xd4000021u32.to_le_bytes())
            .unwrap();
        let thread = kernel.thread_mut(tid).unwrap();
        thread.context.x[..3].copy_from_slice(&[0x8000, 2 << 20, 77]);
        let (completed, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(completed, 1);
        assert!(stop.is_none());
        assert_eq!(
            &kernel.thread(tid).unwrap().context.x[..3],
            &[0, 0x2000000000, 77]
        );
        assert_eq!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .regions()
                .iter()
                .find(|region| region.kind == MemoryKind::Heap)
                .unwrap()
                .size,
            2 << 20
        );
    }

    #[test]
    fn wait_svc_returns_index_in_register_and_preserves_other_arguments() {
        let (mut kernel, pid, tid) = fixture();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xd4000301u32.to_le_bytes())
            .unwrap();
        let event_id = kernel
            .process(pid)
            .unwrap()
            .application
            .notification_event
            .id;
        let handle = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(exosphere_horizon::handle::KernelObject::Event(event_id))
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(0x8004, &handle.0.to_le_bytes())
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(0x8000, &[0x55; 4])
            .unwrap();
        let thread = kernel.thread_mut(tid).unwrap();
        thread.context.x[..4].copy_from_slice(&[0x8000, 0x8004, 1, u64::MAX]);
        let (completed, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(completed, 1);
        assert!(stop.is_none());
        let context = &kernel.thread(tid).unwrap().context;
        assert_eq!(&context.x[..4], &[0, 0, 1, u64::MAX]);
        assert_eq!(context.pc, 0x1004);
        // x0 is not a writable guest out-pointer at the raw SVC boundary.
        let mut bytes = [0; 4];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(0x8000, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0x55; 4]);
    }

    fn fixture() -> (HorizonKernel, u64, u64) {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        let memory = &mut kernel.process_mut(pid).unwrap().memory;
        memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_EXECUTE,
                MemoryKind::Code,
            )
            .unwrap();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Stack,
            )
            .unwrap();
        memory
            .load_bytes(0x1000, &0xd40000c1u32.to_le_bytes())
            .unwrap(); // svc #6
        memory
            .load_bytes(0x1004, &0xd503201fu32.to_le_bytes())
            .unwrap(); // nop
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        (kernel, pid, tid)
    }

    #[test]
    fn guest_condition_wait_and_signal_resume_with_mutex_and_preserved_registers() {
        use exosphere_horizon::handle::KernelObject;
        let (mut kernel, pid, signaler) = fixture();
        let (waiter, wh) = kernel.create_thread(pid, 0x1100, 0x9000, 24).unwrap();
        let memory = &mut kernel.process_mut(pid).unwrap().memory;
        for (address, instruction) in [
            (0x1000, 0xd40003a1u32), // SignalProcessWideKey
            (0x1004, 0xd4000141),    // ExitThread
            (0x1100, 0xd4000381),    // WaitProcessWideKeyAtomic
            (0x1104, 0xd2800169),    // mov x9, #11
            (0x1108, 0xd4000141),    // ExitThread
        ] {
            memory
                .load_bytes(address, &instruction.to_le_bytes())
                .unwrap();
        }
        memory.write(0x8000, &wh.0.to_le_bytes()).unwrap();
        let context = &mut kernel.thread_mut(waiter).unwrap().context;
        context.x[..4].copy_from_slice(&[0x8000, 0x8007, u64::from(wh.0), u64::MAX]);
        context.x[10] = 0xabcd;
        let context = &mut kernel.thread_mut(signaler).unwrap().context;
        context.x[0] = 0x8005;
        context.x[1] = 1;
        kernel.start_thread(signaler).unwrap();
        kernel.start_thread(waiter).unwrap();
        let suspended = run_process(&mut kernel, pid, 1).unwrap();
        assert!(suspended.stop.is_none());
        assert_eq!(suspended.last_thread, Some(waiter));
        assert_eq!(kernel.thread(waiter).unwrap().state, ThreadState::Waiting);
        assert_eq!(kernel.thread(waiter).unwrap().context.pc, 0x1104);
        let resumed = run_process(&mut kernel, pid, 10).unwrap();
        assert_eq!(resumed.completed, 4);
        assert!(resumed.stop.is_none());
        assert!(resumed.idle);
        let context = &kernel.thread(waiter).unwrap().context;
        assert_eq!(context.x[0], 0);
        assert_eq!(context.x[1], 0x8007);
        assert_eq!(context.x[9], 11);
        assert_eq!(context.x[10], 0xabcd);
        assert_eq!(kernel.thread(signaler).unwrap().context.x[0], 0x8005);
        assert!(
            kernel
                .process(pid)
                .unwrap()
                .handles
                .contains_object(&KernelObject::Thread(waiter))
        );
        let mut words = [0; 8];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(0x8000, &mut words)
            .unwrap();
        assert_eq!(u32::from_le_bytes(words[..4].try_into().unwrap()), wh.0);
        assert_eq!(u32::from_le_bytes(words[4..].try_into().unwrap()), 0);
    }

    #[test]
    fn mutex_wait_suspends_at_next_pc_and_unlock_runs_the_waiter() {
        let (mut kernel, pid, owner) = fixture();
        let (waiter, wh) = kernel.create_thread(pid, 0x1100, 0x9000, 24).unwrap();
        // Use a duplicate real handle to exercise object identity rather than a
        // guessed slot value in the guest's lock word.
        let oh = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(exosphere_horizon::handle::KernelObject::Thread(owner))
            .unwrap();
        let memory = &mut kernel.process_mut(pid).unwrap().memory;
        for (address, instruction) in [
            (0x1000, 0xd4000361u32), // ArbitrateUnlock
            (0x1004, 0xd4000141),    // ExitThread
            (0x1100, 0xd4000341),    // ArbitrateLock
            (0x1104, 0xd28000e9),    // mov x9, #7
            (0x1108, 0xd4000141),    // ExitThread
        ] {
            memory
                .load_bytes(address, &instruction.to_le_bytes())
                .unwrap();
        }
        memory
            .write(0x8000, &(oh.0 | (1 << 30)).to_le_bytes())
            .unwrap();
        let context = &mut kernel.thread_mut(waiter).unwrap().context;
        context.x[..3].copy_from_slice(&[u64::from(oh.0), 0x8000, u64::from(wh.0)]);
        context.x[10] = 0x1234;
        kernel.thread_mut(owner).unwrap().context.x[0] = 0x8000;
        kernel.start_thread(owner).unwrap();
        kernel.start_thread(waiter).unwrap();
        let suspended = run_process(&mut kernel, pid, 1).unwrap();
        assert_eq!(suspended.last_thread, Some(waiter));
        assert_eq!(kernel.thread(waiter).unwrap().state, ThreadState::Waiting);
        assert_eq!(kernel.thread(waiter).unwrap().context.pc, 0x1104);
        assert_eq!(kernel.thread(owner).unwrap().priority, 24);
        let resumed = run_process(&mut kernel, pid, 10).unwrap();
        assert!(resumed.stop.is_none());
        assert!(resumed.idle);
        assert_eq!(resumed.completed, 4);
        let context = &kernel.thread(waiter).unwrap().context;
        assert_eq!(context.x[0], 0);
        assert_eq!(context.x[9], 7);
        assert_eq!(context.x[10], 0x1234);
        assert_eq!(kernel.thread(owner).unwrap().priority, 44);
        let mut word = [0; 4];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(0x8000, &mut word)
            .unwrap();
        assert_eq!(u32::from_le_bytes(word), wh.0);
    }

    #[test]
    fn process_runner_starts_child_preserves_context_and_resumes_budget() {
        let (mut kernel, pid, parent) = fixture();
        let (child, handle) = kernel.create_thread(pid, 0x1100, 0x9000, 44).unwrap();
        let memory = &mut kernel.process_mut(pid).unwrap().memory;
        for (address, instruction) in [
            (0x1000, 0xd4000121u32), // StartThread
            (0x1004, 0xd280012a),    // mov x10, #9
            (0x1008, 0xd4000141),    // ExitThread
            (0x1100, 0xd2800540),    // mov x0, #42
            (0x1104, 0xd4000141),    // ExitThread
        ] {
            memory
                .load_bytes(address, &instruction.to_le_bytes())
                .unwrap();
        }
        kernel.thread_mut(parent).unwrap().context.x[0] = u64::from(handle.0);
        kernel.thread_mut(parent).unwrap().context.v[3] = u128::MAX;
        kernel.start_thread(parent).unwrap();
        let first = run_process(&mut kernel, pid, 3).unwrap();
        assert_eq!(first.completed, 3);
        assert!(first.stop.is_none());
        assert!(!first.idle);
        assert_eq!(first.last_thread, Some(child));
        assert_eq!(kernel.thread(child).unwrap().state, ThreadState::Exited);
        assert_eq!(kernel.thread(child).unwrap().context.x[0], 42);
        assert_eq!(kernel.thread(parent).unwrap().context.pc, 0x1004);
        let second = run_process(&mut kernel, pid, 10).unwrap();
        assert_eq!(second.completed, 2);
        assert!(second.stop.is_none());
        assert!(second.idle);
        let context = &kernel.thread(parent).unwrap().context;
        assert_eq!(context.x[10], 9);
        assert_eq!(context.v[3], u128::MAX);
        assert_eq!(kernel.thread(parent).unwrap().state, ThreadState::Exited);
        assert!(kernel.scheduler.is_empty());
    }

    #[test]
    fn shared_memory_svc_changes_only_result_register() {
        use exosphere_horizon::{
            handle::KernelObject, layout::ProcessLayout, shared_memory::SharedMemory,
        };
        let (mut kernel, pid, tid) = fixture();
        kernel.process_mut(pid).unwrap().layout =
            Some(ProcessLayout::diagnostic_39_bit(0x7100000000, 4096, 0x7f00000000, 4096).unwrap());
        let memory = SharedMemory::with_guest_permission(4096, MemoryPermission::READ).unwrap();
        let handle = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::SharedMemory(memory))
            .unwrap();
        for number in [0x13u32, 0x14] {
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .load_bytes(0x1000, &(0xd4000001 | number << 5).to_le_bytes())
                .unwrap();
            let context = &mut kernel.thread_mut(tid).unwrap().context;
            context.pc = 0x1000;
            context.x[..5].copy_from_slice(&[handle.0 as u64, 0x1800000000, 4096, 1, 77]);
            let before = context.clone();
            let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
            assert_eq!(count, 1);
            assert!(stop.is_none());
            let after = &kernel.thread(tid).unwrap().context;
            assert_eq!(after.x[0], 0);
            assert_eq!(after.x[1..], before.x[1..]);
            assert_eq!(after.v, before.v);
            assert_eq!(after.pc, 0x1004);
        }
    }

    #[test]
    fn process_runner_preserves_foreign_ready_queue_and_faulting_thread() {
        let (mut kernel, pid, tid) = fixture();
        let other = kernel.create_process(2, "other");
        let (foreign, _) = kernel.create_thread(other, 0x1000, 0x9000, 4).unwrap();
        kernel.start_thread(foreign).unwrap();
        kernel.start_thread(tid).unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xd4000801u32.to_le_bytes())
            .unwrap(); // unaudited svc #0x40
        let result = run_process(&mut kernel, pid, 10).unwrap();
        assert_eq!(result.completed, 0);
        assert_eq!(result.last_thread, Some(tid));
        assert!(result.stop.is_some());
        assert_eq!(kernel.thread(tid).unwrap().context.pc, 0x1000);
        assert_eq!(kernel.thread(tid).unwrap().state, ThreadState::Ready);
        assert_eq!(kernel.scheduler.next(), Some(foreign));
        assert_eq!(kernel.scheduler.next(), Some(tid));
    }

    #[test]
    fn guest_start_thread_rejects_restarts_and_foreign_handles() {
        use exosphere_horizon::handle::KernelObject;
        let (mut kernel, pid, tid) = fixture();
        let (child, handle) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let mut args = SvcArguments::default();
        args.x[0] = u64::from(handle.0);
        assert_eq!(
            svc::dispatch_guest(&mut kernel, pid, tid, 9, args)
                .unwrap()
                .result_code,
            0
        );
        assert_eq!(kernel.scheduler.len(), 1);
        assert_eq!(
            svc::dispatch_guest(&mut kernel, pid, tid, 9, args)
                .unwrap()
                .result_code,
            0xfa01
        );
        kernel.thread_mut(child).unwrap().state = ThreadState::Waiting;
        assert_eq!(
            svc::dispatch_guest(&mut kernel, pid, tid, 9, args)
                .unwrap()
                .result_code,
            0xfa01
        );
        assert_eq!(kernel.thread(child).unwrap().state, ThreadState::Waiting);
        let other = kernel.create_process(2, "other");
        let (foreign, _) = kernel.create_thread(other, 0x1000, 0x9000, 44).unwrap();
        let borrowed = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::Thread(foreign))
            .unwrap();
        args.x[0] = u64::from(borrowed.0);
        assert_eq!(
            svc::dispatch_guest(&mut kernel, pid, tid, 9, args)
                .unwrap()
                .result_code,
            0xe401
        );
        assert_eq!(kernel.thread(foreign).unwrap().state, ThreadState::Created);
        args.x[0] = u64::MAX;
        assert_eq!(
            svc::dispatch_guest(&mut kernel, pid, tid, 9, args)
                .unwrap()
                .result_code,
            0xe401
        );
    }

    #[test]
    fn create_thread_uses_guest_registers_and_inherits_fp_status() {
        use exosphere_horizon::{process::ProcessSchedulingCapabilities, thread::ThreadState};
        let (mut kernel, pid, tid) = fixture();
        let process = kernel.process_mut(pid).unwrap();
        process.scheduling = Some(ProcessSchedulingCapabilities {
            core_mask: 3,
            priority_mask: u64::MAX << 24,
        });
        process.ideal_core = Some(1);
        process
            .memory
            .load_bytes(0x1000, &0xd4000101u32.to_le_bytes())
            .unwrap();
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.x[..6].copy_from_slice(&[u64::MAX, 0x1004, 0x12345678, 0x9000, 32, 0xffff_fffe]);
        context.fpcr = 0x400000;
        context.fpsr = 0x10;
        let before = context.clone();
        let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(count, 1);
        assert!(stop.is_none());
        let parent = kernel.thread(tid).unwrap();
        assert_eq!(parent.context.x[0], 0);
        assert_eq!(parent.context.pc, 0x1004);
        assert_eq!(parent.context.x[2..], before.x[2..]);
        let child_id = *kernel.process(pid).unwrap().thread_ids.last().unwrap();
        let child = kernel.thread(child_id).unwrap();
        assert_eq!(child.state, ThreadState::Created);
        assert_eq!(
            (child.context.pc, child.context.sp, child.context.x[0]),
            (0x1004, 0x9000, 0x12345678)
        );
        assert_eq!(child.priority, 32);
        assert_eq!(child.affinity.unwrap().ideal_core, 1);
        assert_eq!(
            (child.context.fpcr, child.context.fpsr),
            (before.fpcr, before.fpsr)
        );
        assert_ne!(child.context.tpidrro_el0, parent.context.tpidrro_el0);
        assert!(kernel.scheduler.is_empty());
    }

    #[test]
    fn create_thread_rejects_invalid_grants_without_allocating() {
        use exosphere_horizon::process::ProcessSchedulingCapabilities;
        let (mut kernel, pid, tid) = fixture();
        let process = kernel.process_mut(pid).unwrap();
        process.scheduling = Some(ProcessSchedulingCapabilities {
            core_mask: 2,
            priority_mask: 1 << 32,
        });
        process.ideal_core = Some(1);
        process
            .memory
            .load_bytes(0x1000, &0xd4000101u32.to_le_bytes())
            .unwrap();
        for (priority, core, error) in [
            (32, 0, 0xe201),
            (32, -1, 0xe201),
            (32, 4, 0xe201),
            (23, 1, 0xe001),
            (64, 1, 0xe001),
            (-1, 1, 0xe001),
        ] {
            let context = &mut kernel.thread_mut(tid).unwrap().context;
            context.pc = 0x1000;
            context.x[..6].copy_from_slice(&[
                u64::MAX,
                0x1004,
                7,
                0x9000,
                priority as u32 as u64,
                core as u32 as u64,
            ]);
            let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
            assert_eq!(count, 1);
            assert!(stop.is_none());
            assert_eq!(kernel.thread(tid).unwrap().context.x[0], error);
            assert_eq!(kernel.process(pid).unwrap().thread_ids, [tid]);
        }
    }

    #[test]
    fn get_info_entropy_returns_in_register_without_dereferencing_x0() {
        let (mut kernel, pid, tid) = fixture();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xd4000521u32.to_le_bytes())
            .unwrap(); // svc #0x29
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.x[0] = u64::MAX; // The output pointer exists only in the C wrapper ABI.
        context.x[1] = 11;
        context.x[2] = 0;
        context.x[3] = 2;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(count, 1);
        assert!(stop.is_none());
        let expected = kernel.process_mut(pid).unwrap().random_entropy(2).unwrap();
        let context = &kernel.thread(tid).unwrap().context;
        assert_eq!(&context.x[..4], &[0, expected, 0, 2]);
        assert_eq!(context.pc, 0x1004);
    }

    #[test]
    fn condition_signal_preserves_registers_for_void_svc() {
        let (mut kernel, pid, tid) = fixture();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xd40003a1u32.to_le_bytes())
            .unwrap();
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.x[0] = 0x8000;
        context.x[1] = u32::MAX as u64;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 2).unwrap();
        assert_eq!(count, 2);
        assert!(stop.is_none());
        let context = &kernel.thread(tid).unwrap().context;
        assert_eq!(context.x[0], 0x8000);
        assert_eq!(context.x[1], u32::MAX as u64);
        assert_eq!(context.pc, 0x1008);
    }

    #[test]
    fn named_port_returns_a_real_session_and_bounds_guest_name() {
        let (mut kernel, pid, tid) = fixture();
        let memory = &mut kernel.process_mut(pid).unwrap().memory;
        memory
            .load_bytes(0x1000, &0xd40003e1u32.to_le_bytes())
            .unwrap();
        memory.write(0x8000, b"sm:\0").unwrap();
        kernel.thread_mut(tid).unwrap().context.x[0] = u64::MAX;
        kernel.thread_mut(tid).unwrap().context.x[1] = 0x8000;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(count, 1);
        assert!(stop.is_none());
        let context = &kernel.thread(tid).unwrap().context;
        assert_eq!(context.x[0], 0);
        let handle = exosphere_horizon::handle::Handle(context.x[1] as u32);
        assert!(matches!(
            kernel.process(pid).unwrap().handles.get(handle).unwrap(),
            exosphere_horizon::handle::KernelObject::Session(_)
        ));
        for name in [b"fsp-srv\0".as_slice(), b"abcdefghijkl", b"\xff\0"] {
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(0x8000, name)
                .unwrap();
            let context = &mut kernel.thread_mut(tid).unwrap().context;
            context.pc = 0x1000;
            context.x[1] = 0x8000;
            let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
            assert_eq!(count, 0);
            assert!(stop.is_some());
            assert_eq!(kernel.thread(tid).unwrap().context.pc, 0x1000);
        }
    }

    #[test]
    fn guest_ipc_reads_and_replaces_tls_without_overwriting_thread_state() {
        let (mut kernel, pid, tid) = fixture();
        let session = kernel.connect_service(pid, "sm:").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let memory = &mut kernel.process_mut(pid).unwrap().memory;
        memory
            .load_bytes(0x1000, &0xd4000421u32.to_le_bytes())
            .unwrap();
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        memory.write(tls, &request).unwrap();
        memory.write(tls + 0x100, &[0xa5; 0x100]).unwrap();
        kernel.thread_mut(tid).unwrap().context.x[0] = u64::from(session.0);
        kernel.thread_mut(tid).unwrap().context.x[1] = 77;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(count, 1);
        assert!(stop.is_none());
        let context = &kernel.thread(tid).unwrap().context;
        assert_eq!(context.x[0], 0);
        assert_eq!(context.x[1], 77);
        let mut bytes = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut bytes)
            .unwrap();
        let hipc = exosphere_horizon::hipc::parse(&bytes).unwrap();
        let response = exosphere_horizon::cmif::parse_response(hipc.raw_data).unwrap();
        assert_eq!(response.header.result, 0);
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls + 0x100, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0xa5; 0x100]);
    }

    #[test]
    fn malformed_guest_ipc_stops_without_modifying_tls_or_pc() {
        let (mut kernel, pid, tid) = fixture();
        let session = kernel.connect_service(pid, "sm:").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xd4000421u32.to_le_bytes())
            .unwrap();
        let malformed = [0xff; 0x100];
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &malformed)
            .unwrap();
        kernel.thread_mut(tid).unwrap().context.x[0] = u64::from(session.0);
        let (count, stop) = run_thread(&mut kernel, pid, tid, 1).unwrap();
        assert_eq!(count, 0);
        assert!(stop.is_some());
        assert_eq!(kernel.thread(tid).unwrap().context.pc, 0x1000);
        let mut output = [0; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        assert_eq!(output, malformed);
    }

    #[test]
    fn exit_yields_without_running_the_following_instruction() {
        let (mut kernel, pid, tid) = fixture();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xd4000141u32.to_le_bytes())
            .unwrap(); // svc #10
        kernel.thread_mut(tid).unwrap().context.x[1] = 77;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 10).unwrap();
        assert_eq!(count, 1);
        assert!(stop.is_none());
        let thread = kernel.thread(tid).unwrap();
        assert_eq!(thread.state, ThreadState::Exited);
        assert_eq!(thread.context.pc, 0x1004);
        assert_eq!(thread.context.x[1], 77);
        assert_eq!(run_thread(&mut kernel, pid, tid, 10).unwrap().0, 0);
    }

    #[test]
    fn query_memory_resumes_and_preserves_input_registers() {
        let (mut kernel, pid, tid) = fixture();
        let context = &mut kernel.thread_mut(tid).unwrap().context;
        context.x[0] = 0x8000;
        context.x[2] = 0x1004;
        context.x[3] = 77;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 2).unwrap();
        assert_eq!(count, 2);
        assert!(stop.is_none());
        let context = &kernel.thread(tid).unwrap().context;
        assert_eq!(context.pc, 0x1008);
        assert_eq!(&context.x[..4], &[0, 0, 0x1004, 77]);
        let mut bytes = [0u8; 0x28];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(0x8000, &mut bytes)
            .unwrap();
        assert_eq!(u64::from_le_bytes(bytes[..8].try_into().unwrap()), 0x1000);
        assert_eq!(
            u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            PAGE_SIZE
        );
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 5);
        assert_eq!(&bytes[28..], &[0; 12]);
    }

    #[test]
    fn faulting_query_and_unaudited_svc_do_not_advance() {
        let (mut kernel, pid, tid) = fixture();
        kernel.thread_mut(tid).unwrap().context.x[0] = 0x1000;
        let (count, stop) = run_thread(&mut kernel, pid, tid, 2).unwrap();
        assert_eq!(count, 0);
        assert!(matches!(stop, Some(CpuError::Memory(_))));
        assert_eq!(kernel.thread(tid).unwrap().context.pc, 0x1000);
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .load_bytes(0x1000, &0xd4000041u32.to_le_bytes())
            .unwrap();
        let (count, stop) = run_thread(&mut kernel, pid, tid, 2).unwrap();
        assert_eq!(count, 0);
        assert!(matches!(
            stop,
            Some(CpuError::SupervisorCall { number: 2, .. })
        ));
    }
}
