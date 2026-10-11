// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded synchronous command consumption, without guest fence emulation.

use std::collections::VecDeque;

use super::{GuestGpuDecodeError, GuestGpuDecoder, GuestGpuExecutionError, GuestGpuExecutor};
use exosphere_gpu_ir::gpfifo::GpfifoEntry;
use exosphere_horizon::nvdrv::syncpoint::{NvGpuSyncpoints, SyncpointReservation};
use exosphere_horizon::{memory::AddressSpace, nvdrv::address_space::NvGpuAddressSpace};

/// Host-side synchronization policy; native IOCTL flags are not interpreted here.
pub struct SyncpointAdmission<'a> {
    pub points: &'a mut NvGpuSyncpoints,
    pub id: u32,
    pub increments: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum QueueAdmissionError {
    #[error("invalid command queue capacity")]
    InvalidCapacity,
    #[error("command queue capacity exhausted")]
    Full,
    #[error("command queue sequence exhausted")]
    SequenceExhausted,
    #[error("command channel is stopped after a fault")]
    Stopped,
    #[error(transparent)]
    Entry(#[from] GuestGpuDecodeError),
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelFault {
    #[error(transparent)]
    Decode(GuestGpuDecodeError),
    #[error(transparent)]
    Execution(GuestGpuExecutionError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStep {
    Idle,
    Applied { transfers: usize },
    Stopped,
    Waiting { transfers: usize },
}

/// Software channel state for subsequent nvdrv integration. GPU mappings are
/// borrowed from the canonical owner at consumption time, never snapshotted.
/// A fault retains the head and prevents replay of already-applied effects.
#[derive(Debug)]
pub struct GuestGpuChannel {
    decoder: GuestGpuDecoder,
    executor: GuestGpuExecutor,
    entries: VecDeque<u64>,
    capacity: usize,
    fault: Option<ChannelFault>,
    binding: Option<exosphere_horizon::nvdrv::NvGpuChannelIdentity>,
    pending_trace: Option<(exosphere_gpu_ir::FrontEndTrace, usize)>,
    admitted_entries: u64,
    retired_entries: u64,
    reservations: VecDeque<(u64, SyncpointReservation)>,
}

impl GuestGpuChannel {
    pub fn new(capacity: usize) -> Result<Self, QueueAdmissionError> {
        // This is a software resource bound, not a reported hardware queue size.
        if capacity == 0 || capacity > 65_536 {
            return Err(QueueAdmissionError::InvalidCapacity);
        }
        Ok(Self {
            decoder: GuestGpuDecoder::default(),
            executor: GuestGpuExecutor::default(),
            entries: VecDeque::new(),
            capacity,
            fault: None,
            binding: None,
            pending_trace: None,
            admitted_entries: 0,
            retired_entries: 0,
            reservations: VecDeque::new(),
        })
    }

    pub fn pending_entries(&self) -> usize {
        self.entries.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Monotonic submission boundary, not a guest fence value. A driver can
    /// record this after admission and compare it with `retired_entries`.
    pub fn admitted_entries(&self) -> u64 {
        self.admitted_entries
    }

    /// Counts only fully executed descriptors. Waiting and faulted heads do
    /// not retire, even when a prefix already produced memory side effects.
    pub fn retired_entries(&self) -> u64 {
        self.retired_entries
    }

    pub fn has_pending_inline_upload(&self) -> bool {
        self.executor.has_pending_inline_upload()
    }

    pub fn fault(&self) -> Option<&ChannelFault> {
        self.fault.as_ref()
    }

    /// Atomically reserve synchronization and admit a host-decoded batch.
    /// Admission failure cancels its ticket; success does not complete it.
    pub fn enqueue_synchronized_bound(
        &mut self,
        kernel: &mut exosphere_horizon::kernel::HorizonKernel,
        process_id: u64,
        session_handle: exosphere_horizon::handle::Handle,
        descriptor: u32,
        entries: &[u64],
        sync: SyncpointAdmission<'_>,
    ) -> exosphere_horizon::HorizonResult<exosphere_horizon::nvdrv::submission::SubmissionFence>
    {
        kernel.with_gpu_channel(process_id, session_handle, descriptor, |identity, _, _| {
            let invalid = |reason: String| exosphere_horizon::HorizonError::InvalidState(reason);
            if self
                .binding
                .as_ref()
                .is_some_and(|owner| *owner != identity)
            {
                return Err(invalid("command queue belongs to another channel".into()));
            }
            if self.reservations.len() >= 4096 {
                return Err(invalid("pending synchronization capacity exhausted".into()));
            }
            self.reservations
                .try_reserve(1)
                .map_err(|_| invalid("pending synchronization allocation failed".into()))?;
            let ticket = sync
                .points
                .reserve(&identity, sync.id, sync.increments)
                .map_err(|error| invalid(error.to_string()))?;
            if let Err(error) = self.enqueue(entries) {
                // No intervening reservation exists: this is the newest ticket.
                sync.points
                    .cancel_latest(&identity, &ticket)
                    .map_err(|error| invalid(error.to_string()))?;
                return Err(invalid(error.to_string()));
            }
            let fence = ticket.fence();
            self.reservations.push_back((self.admitted_entries, ticket));
            self.binding = Some(identity);
            Ok(fence)
        })?
    }

    /// Publish completion only for boundaries already retired by execution.
    /// A failed completion retains its ticket for diagnosis/retry, without replay.
    pub fn retire_syncpoints_bound(
        &mut self,
        kernel: &mut exosphere_horizon::kernel::HorizonKernel,
        process_id: u64,
        session_handle: exosphere_horizon::handle::Handle,
        descriptor: u32,
        points: &mut NvGpuSyncpoints,
    ) -> exosphere_horizon::HorizonResult<usize> {
        kernel.with_gpu_channel(process_id, session_handle, descriptor, |identity, _, _| {
            if self.binding.as_ref() != Some(&identity) {
                return Err(exosphere_horizon::HorizonError::InvalidState(
                    "command queue belongs to another channel".into(),
                ));
            }
            let mut completed = 0;
            // Inline uploads may span descriptors. Executing their header is
            // not evidence that the pending data transfer has completed.
            if self.has_pending_inline_upload() {
                return Ok(completed);
            }
            while let Some((boundary, ticket)) = self.reservations.front() {
                if *boundary > self.retired_entries {
                    break;
                }
                points.complete(&identity, ticket).map_err(|error| {
                    exosphere_horizon::HorizonError::InvalidState(error.to_string())
                })?;
                self.reservations.pop_front();
                completed += 1;
            }
            Ok(completed)
        })?
    }

    /// Validate materialized nvdrv transport and admit its batch. The returned
    /// index identifies the reply buffer, but admission alone must not write a
    /// success reply or imply completion of queued GPU work.
    pub fn enqueue_request_bound(
        &mut self,
        kernel: &mut exosphere_horizon::kernel::HorizonKernel,
        process_id: u64,
        session_handle: exosphere_horizon::handle::Handle,
        request: &exosphere_horizon::ipc::ServiceRequest,
    ) -> exosphere_horizon::HorizonResult<usize> {
        let decoded = exosphere_horizon::nvdrv::submission::GpfifoSubmissionRequest::decode(
            request,
            self.capacity,
        )
        .map_err(|error| exosphere_horizon::HorizonError::IpcProtocol(error.to_string()))?;
        self.enqueue_submission_bound(
            kernel,
            process_id,
            session_handle,
            decoded.descriptor,
            &decoded.submission,
        )?;
        Ok(decoded.output_index)
    }

    /// Admit a decoded wire batch without synthesizing guest synchronization.
    /// This host boundary does not return an IOCTL reply or a completion fence.
    pub fn enqueue_submission_bound(
        &mut self,
        kernel: &mut exosphere_horizon::kernel::HorizonKernel,
        process_id: u64,
        session_handle: exosphere_horizon::handle::Handle,
        descriptor: u32,
        submission: &exosphere_horizon::nvdrv::submission::GpfifoSubmission<'_>,
    ) -> exosphere_horizon::HorizonResult<()> {
        if submission.flags != 0 || submission.fence.id != 0 || submission.fence.value != 0 {
            return Err(exosphere_horizon::HorizonError::InvalidState(
                "GPU submission synchronization is not implemented".into(),
            ));
        }
        self.enqueue_iterator_bound(
            kernel,
            process_id,
            session_handle,
            descriptor,
            submission.entries(),
        )
    }

    /// Authenticate the canonical channel before admitting descriptors. Failed
    /// admission leaves both the queue and its owner unchanged.
    pub fn enqueue_bound(
        &mut self,
        kernel: &mut exosphere_horizon::kernel::HorizonKernel,
        process_id: u64,
        session_handle: exosphere_horizon::handle::Handle,
        descriptor: u32,
        entries: &[u64],
    ) -> exosphere_horizon::HorizonResult<()> {
        self.enqueue_iterator_bound(
            kernel,
            process_id,
            session_handle,
            descriptor,
            entries.iter().copied(),
        )
    }

    fn enqueue_iterator_bound(
        &mut self,
        kernel: &mut exosphere_horizon::kernel::HorizonKernel,
        process_id: u64,
        session_handle: exosphere_horizon::handle::Handle,
        descriptor: u32,
        entries: impl ExactSizeIterator<Item = u64> + Clone,
    ) -> exosphere_horizon::HorizonResult<()> {
        kernel.with_gpu_channel(process_id, session_handle, descriptor, |identity, _, _| {
            if self
                .binding
                .as_ref()
                .is_some_and(|binding| *binding != identity)
            {
                return Err(exosphere_horizon::HorizonError::InvalidState(
                    "command queue belongs to another channel".into(),
                ));
            }
            self.enqueue_iterator(entries).map_err(|error| {
                exosphere_horizon::HorizonError::InvalidState(error.to_string())
            })?;
            self.binding = Some(identity);
            Ok(())
        })?
    }

    /// Execute against the actual nvdrv session and channel, preserving identity
    /// across IPC aliases and refusing to reuse decoder state for another channel.
    pub fn step_bound(
        &mut self,
        kernel: &mut exosphere_horizon::kernel::HorizonKernel,
        process_id: u64,
        session_handle: exosphere_horizon::handle::Handle,
        descriptor: u32,
    ) -> exosphere_horizon::HorizonResult<ChannelStep> {
        self.step_bound_with_draw_handler(
            kernel,
            process_id,
            session_handle,
            descriptor,
            |_, _, _, event| Err(super::GuestGpuExecutionFailure::UnsupportedEvent(event)),
        )
    }

    /// Resolve the canonical channel before exposing live memory to a native
    /// draw handler. An IPC alias cannot substitute another channel's state.
    pub fn step_bound_with_draw_handler(
        &mut self,
        kernel: &mut exosphere_horizon::kernel::HorizonKernel,
        process_id: u64,
        session_handle: exosphere_horizon::handle::Handle,
        descriptor: u32,
        mut draw: impl FnMut(
            &mut AddressSpace,
            &NvGpuAddressSpace,
            &exosphere_gpu_ir::MaxwellEvent,
            exosphere_gpu_ir::frontend::FrontEndEvent,
        ) -> Result<(), super::GuestGpuExecutionFailure>,
    ) -> exosphere_horizon::HorizonResult<ChannelStep> {
        kernel.with_gpu_channel(
            process_id,
            session_handle,
            descriptor,
            |identity, memory, gpu| {
                if self
                    .binding
                    .as_ref()
                    .is_some_and(|binding| *binding != identity)
                {
                    return Err(exosphere_horizon::HorizonError::InvalidState(
                        "command queue belongs to another channel".into(),
                    ));
                }
                self.binding = Some(identity);
                Ok(self.step_with_draw_handler(memory, gpu, &mut draw))
            },
        )?
    }

    /// Admit the entire batch or none. Capture descriptors, not command bytes:
    /// earlier DMA commands may modify the pushbuffer of a later entry.
    pub fn enqueue(&mut self, entries: &[u64]) -> Result<(), QueueAdmissionError> {
        self.enqueue_iterator(entries.iter().copied())
    }

    fn enqueue_iterator(
        &mut self,
        entries: impl ExactSizeIterator<Item = u64> + Clone,
    ) -> Result<(), QueueAdmissionError> {
        if self.fault.is_some() {
            return Err(QueueAdmissionError::Stopped);
        }
        if entries.len() > self.capacity - self.entries.len() {
            return Err(QueueAdmissionError::Full);
        }
        let admitted_entries = self
            .admitted_entries
            .checked_add(entries.len() as u64)
            .ok_or(QueueAdmissionError::SequenceExhausted)?;
        // Validate a borrowed pass before reserving or committing descriptors.
        for raw in entries.clone() {
            let entry = GpfifoEntry::decode(raw).map_err(|_| GuestGpuDecodeError::InvalidRange)?;
            if entry.is_nop() {
                continue;
            }
            let GpfifoEntry::Pushbuffer(entry) = entry else {
                return Err(GuestGpuDecodeError::UnsupportedGpfifoEntry.into());
            };
            if entry.conditional_fetch || entry.privileged || entry.subroutine || entry.wait {
                return Err(GuestGpuDecodeError::UnsupportedGpfifoEntry.into());
            }
        }
        self.entries
            .try_reserve(entries.len())
            .map_err(|_| QueueAdmissionError::Full)?;
        self.entries.extend(entries);
        self.admitted_entries = admitted_entries;
        Ok(())
    }

    /// Consume one entry, stopping durably on any decode/execution failure. No
    /// syncpoint or fence is advanced: that requires separate driver integration.
    pub fn step(&mut self, memory: &mut AddressSpace, gpu: &NvGpuAddressSpace) -> ChannelStep {
        self.step_with_draw_handler(memory, gpu, |_, _, _, event| {
            Err(super::GuestGpuExecutionFailure::UnsupportedEvent(event))
        })
    }

    /// Retire a descriptor only after the synchronous draw handler completes.
    /// The handler has the same rendering/writeback contract as the executor;
    /// admission or native queue submission alone is not completion.
    pub fn step_with_draw_handler(
        &mut self,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        draw: impl FnMut(
            &mut AddressSpace,
            &NvGpuAddressSpace,
            &exosphere_gpu_ir::MaxwellEvent,
            exosphere_gpu_ir::frontend::FrontEndEvent,
        ) -> Result<(), super::GuestGpuExecutionFailure>,
    ) -> ChannelStep {
        if self.fault.is_some() {
            return ChannelStep::Stopped;
        }
        let Some(&entry) = self.entries.front() else {
            return ChannelStep::Idle;
        };
        if self.pending_trace.is_none() {
            match self.decoder.decode_gpfifo_entry(memory, gpu, entry) {
                Ok(trace) => self.pending_trace = Some((trace, 0)),
                Err(error) => {
                    self.fault = Some(ChannelFault::Decode(error));
                    return ChannelStep::Stopped;
                }
            }
        }
        let (trace, start) = self.pending_trace.as_mut().expect("head trace was decoded");
        match self
            .executor
            .execute_from_with_draw_handler(memory, gpu, trace, *start, draw)
        {
            Ok(transfers) => {
                self.pending_trace = None;
                self.entries.pop_front();
                // Admission checked the lifetime sequence bound before any
                // descriptor was committed, so retirement cannot overflow.
                self.retired_entries += 1;
                ChannelStep::Applied { transfers }
            }
            Err(error) => {
                if matches!(error.reason, super::GuestGpuExecutionFailure::SemaphoreWait) {
                    *start = error.event_index;
                    return ChannelStep::Waiting {
                        transfers: error.completed_transfers,
                    };
                }
                self.fault = Some(ChannelFault::Execution(error));
                ChannelStep::Stopped
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_gpu_ir::dma::method;
    use exosphere_horizon::{
        memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
        nvdrv::NvMapBacking,
    };

    #[test]
    fn draw_handler_controls_descriptor_retirement_and_durable_faults() {
        use exosphere_gpu_ir::{FrontEndTrace, MaxwellEvent, frontend::FrontEndEvent};

        for succeeds in [false, true] {
            let mut memory = AddressSpace::default();
            let gpu = NvGpuAddressSpace::default();
            let mut channel = GuestGpuChannel::new(1).unwrap();
            channel.enqueue(&[0]).unwrap();
            // Isolate scheduler ownership from pushbuffer decoding: the head
            // descriptor already has its immutable, validated draw trace.
            let mut trace = FrontEndTrace::default();
            trace.graphics_events.push(MaxwellEvent::DrawVertexArray {
                first: 0,
                count: 3,
                state: Box::default(),
            });
            trace.events.push(FrontEndEvent::Graphics(0));
            channel.pending_trace = Some((trace, 0));
            let mut calls = 0;
            let result = channel.step_with_draw_handler(&mut memory, &gpu, |_, _, draw, event| {
                assert!(matches!(
                    draw,
                    MaxwellEvent::DrawVertexArray { count: 3, .. }
                ));
                calls += 1;
                if succeeds {
                    Ok(())
                } else {
                    Err(super::super::GuestGpuExecutionFailure::UnsupportedEvent(
                        event,
                    ))
                }
            });
            assert_eq!(calls, 1);
            if succeeds {
                assert_eq!(result, ChannelStep::Applied { transfers: 0 });
                assert_eq!(channel.retired_entries(), 1);
                assert_eq!(channel.pending_entries(), 0);
                assert!(channel.fault().is_none());
            } else {
                assert_eq!(result, ChannelStep::Stopped);
                assert_eq!(channel.retired_entries(), 0);
                assert_eq!(channel.pending_entries(), 1);
                assert!(channel.fault().is_some());
                assert_eq!(
                    channel.step_with_draw_handler(&mut memory, &gpu, |_, _, _, _| {
                        panic!("a durable fault must not retry the draw")
                    }),
                    ChannelStep::Stopped
                );
            }
        }
    }

    #[test]
    fn queued_reductions_unblock_other_channels_without_replaying_prefixes() {
        for (control, initial, payload, succeeds) in [
            (0x29000010u32, 0u32, 7u32, true),
            (0x81000010, 8, 7, true),
            (0x09000010, 0x80000000, 7, true),
            (0x89000010, 0, 7, true),
            (0xb1000010, 6, 7, true),
            (0xb9000010, 8, 7, true),
            (0x11000010, 0, 7, true),
            (0x31000010, 6, 7, false),
        ] {
            let mut memory = AddressSpace::default();
            memory
                .map(
                    0x4000,
                    PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
            let backing = NvMapBacking {
                address: 0x4000,
                alignment: 4096,
                flags: 1,
                kind: 0,
                heap_mask: 0,
                lease: memory
                    .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ_WRITE)
                    .unwrap(),
            };
            let mut gpu = NvGpuAddressSpace::default();
            gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
            // A non-idempotent prefix makes accidental replay observable. The
            // suffix marker must stay untouched until the real acquire succeeds.
            let consumer_words = [
                0x20040004u32,
                0,
                0x100800,
                1,
                0x29000010,
                0x20040004,
                0,
                0x100804,
                7,
                1,
                0x20040004,
                0,
                0x100808,
                99,
                0x01000002,
            ];
            let producer_words = [0x20040004u32, 0, 0x100804, payload, control];
            for (address, words) in [
                (0x4000, consumer_words.as_slice()),
                (0x4100, producer_words.as_slice()),
            ] {
                memory
                    .write(
                        address,
                        &words
                            .iter()
                            .copied()
                            .flat_map(u32::to_le_bytes)
                            .collect::<Vec<_>>(),
                    )
                    .unwrap();
            }
            memory.write(0x4804, &initial.to_le_bytes()).unwrap();
            let mut consumer = GuestGpuChannel::new(1).unwrap();
            let mut producer = GuestGpuChannel::new(1).unwrap();
            consumer
                .enqueue(&[0x100000 | ((consumer_words.len() as u64) << 42)])
                .unwrap();
            producer
                .enqueue(&[0x100100 | ((producer_words.len() as u64) << 42)])
                .unwrap();
            let values = |memory: &AddressSpace| {
                let mut bytes = [0; 12];
                memory.read(0x4800, &mut bytes).unwrap();
                std::array::from_fn::<_, 3, _>(|index| {
                    u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
                })
            };
            for _ in 0..3 {
                assert_eq!(
                    consumer.step(&mut memory, &gpu),
                    ChannelStep::Waiting { transfers: 0 }
                );
                assert_eq!(values(&memory), [1, initial, 0]);
                assert_eq!(consumer.retired_entries(), 0);
                assert_eq!(producer.retired_entries(), 0);
            }
            if succeeds {
                assert_eq!(
                    producer.step(&mut memory, &gpu),
                    ChannelStep::Applied { transfers: 0 }
                );
                assert_eq!(values(&memory), [1, 7, 0]);
                assert_eq!(producer.retired_entries(), 1);
                assert_eq!(
                    consumer.step(&mut memory, &gpu),
                    ChannelStep::Applied { transfers: 0 }
                );
                assert_eq!(values(&memory), [1, 7, 99]);
                assert_eq!(consumer.retired_entries(), 1);
                assert_eq!(consumer.step(&mut memory, &gpu), ChannelStep::Idle);
            } else {
                assert_eq!(producer.step(&mut memory, &gpu), ChannelStep::Stopped);
                assert_eq!(producer.retired_entries(), 0);
                assert_eq!(
                    consumer.step(&mut memory, &gpu),
                    ChannelStep::Waiting { transfers: 0 }
                );
                assert_eq!(values(&memory), [1, initial, 0]);
            }
        }
    }

    #[test]
    fn control_nops_need_no_mapping_and_do_not_admit_other_controls() {
        let mut memory = AddressSpace::default();
        let gpu = NvGpuAddressSpace::default();
        let mut channel = GuestGpuChannel::new(2).unwrap();
        channel.enqueue(&[0, u64::from(u32::MAX)]).unwrap();
        assert_eq!(channel.admitted_entries(), 2);
        assert_eq!(channel.retired_entries(), 0);
        for remaining in [1, 0] {
            assert_eq!(
                channel.step(&mut memory, &gpu),
                ChannelStep::Applied { transfers: 0 }
            );
            assert_eq!(channel.pending_entries(), remaining);
            assert_eq!(channel.retired_entries(), 2 - remaining as u64);
            assert!(channel.fault().is_none());
        }
        assert_eq!(channel.step(&mut memory, &gpu), ChannelStep::Idle);
        for high in [1u32, 2, 3, 0xff, 1 << 8, 1 << 9, 1 << 31] {
            assert!(channel.enqueue(&[0, u64::from(high) << 32]).is_err());
            assert_eq!(channel.pending_entries(), 0);
            assert_eq!(channel.admitted_entries(), 2);
        }
    }

    #[test]
    fn sequence_exhaustion_rejects_admission_without_mutation() {
        let mut channel = GuestGpuChannel::new(2).unwrap();
        channel.admitted_entries = u64::MAX;
        channel.retired_entries = u64::MAX;
        assert!(matches!(
            channel.enqueue(&[0]),
            Err(QueueAdmissionError::SequenceExhausted)
        ));
        assert_eq!(channel.pending_entries(), 0);
        assert_eq!(channel.admitted_entries(), u64::MAX);
        assert_eq!(channel.retired_entries(), u64::MAX);
        channel.enqueue(&[]).unwrap();
    }

    #[test]
    fn semaphore_resume_uses_current_mapping_and_faults_if_it_disappears() {
        for remap in [false, true] {
            let mut memory = AddressSpace::default();
            memory
                .map(
                    0x4000,
                    2 * PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
            let backing = NvMapBacking {
                address: 0x4000,
                alignment: 4096,
                flags: 1,
                kind: 0,
                heap_mask: 0,
                lease: memory
                    .pin_device_memory(0x4000, 2 * PAGE_SIZE, MemoryPermission::READ_WRITE)
                    .unwrap(),
            };
            let mut gpu = NvGpuAddressSpace::default();
            gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
            let words = [0x20040004u32, 0, 0x100800, 7, 1];
            memory
                .write(
                    0x4000,
                    &words
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            let mut channel = GuestGpuChannel::new(1).unwrap();
            channel.enqueue(&[0x100000 | (5 << 42)]).unwrap();
            assert_eq!(
                channel.step(&mut memory, &gpu),
                ChannelStep::Waiting { transfers: 0 }
            );

            gpu.unmap(0x100000).unwrap();
            assert_eq!(channel.admitted_entries(), 1);
            assert_eq!(channel.retired_entries(), 0);
            // The retained command must read its semaphore from the new backing,
            // not the old lease or a snapshot captured before suspension.
            memory.write(0x5800, &7u32.to_le_bytes()).unwrap();
            if remap {
                gpu.map_fixed(0x100000, PAGE_SIZE, PAGE_SIZE, &backing)
                    .unwrap();
                assert_eq!(
                    channel.step(&mut memory, &gpu),
                    ChannelStep::Applied { transfers: 0 }
                );
                assert_eq!(channel.pending_entries(), 0);
                assert_eq!(channel.retired_entries(), 1);
                assert!(channel.fault().is_none());
            } else {
                assert_eq!(channel.step(&mut memory, &gpu), ChannelStep::Stopped);
                assert!(matches!(channel.fault(), Some(ChannelFault::Execution(_))));
                assert_eq!(channel.pending_entries(), 1);
                assert_eq!(channel.retired_entries(), 0);
                // Reintroducing valid memory cannot silently restart a faulted channel.
                gpu.map_fixed(0x100000, PAGE_SIZE, PAGE_SIZE, &backing)
                    .unwrap();
                assert_eq!(channel.step(&mut memory, &gpu), ChannelStep::Stopped);
            }
        }
    }

    #[test]
    fn semaphore_wait_resumes_without_replaying_its_applied_prefix() {
        for (control, signal) in [(1, 7u32), (8, 2u32)] {
            let mut memory = AddressSpace::default();
            memory
                .map(
                    0x4000,
                    PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
            let backing = NvMapBacking {
                address: 0x4000,
                alignment: 4096,
                flags: 1,
                kind: 0,
                heap_mask: 0,
                lease: memory
                    .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ_WRITE)
                    .unwrap(),
            };
            let mut gpu = NvGpuAddressSpace::default();
            gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
            let words = [
                0x20040004u32,
                0,
                0x100800,
                42,
                0x01000002,
                0x20040004,
                0,
                0x100804,
                7,
                control,
                0x20040004,
                0,
                0x100808,
                99,
                0x01000002,
            ];
            memory
                .write(
                    0x4000,
                    &words
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            let mut channel = GuestGpuChannel::new(1).unwrap();
            channel
                .enqueue(&[0x100000 | ((words.len() as u64) << 42)])
                .unwrap();
            assert_eq!(
                channel.step(&mut memory, &gpu),
                ChannelStep::Waiting { transfers: 0 }
            );
            assert_eq!(channel.pending_entries(), 1);
            assert!(channel.fault().is_none());
            let mut value = [0; 4];
            memory.read(0x4800, &mut value).unwrap();
            assert_eq!(u32::from_le_bytes(value), 42);
            memory.write(0x4800, &55u32.to_le_bytes()).unwrap();
            // A nonzero value with no bits in the payload mask must still wait.
            memory.write(0x4804, &8u32.to_le_bytes()).unwrap();
            assert_eq!(
                channel.step(&mut memory, &gpu),
                ChannelStep::Waiting { transfers: 0 }
            );
            memory.read(0x4800, &mut value).unwrap();
            assert_eq!(u32::from_le_bytes(value), 55);
            memory.read(0x4808, &mut value).unwrap();
            assert_eq!(u32::from_le_bytes(value), 0);
            memory.write(0x4804, &signal.to_le_bytes()).unwrap();
            assert_eq!(
                channel.step(&mut memory, &gpu),
                ChannelStep::Applied { transfers: 0 }
            );
            assert_eq!(channel.pending_entries(), 0);
            assert!(channel.fault().is_none());
            memory.read(0x4800, &mut value).unwrap();
            assert_eq!(u32::from_le_bytes(value), 55);
            memory.read(0x4808, &mut value).unwrap();
            assert_eq!(u32::from_le_bytes(value), 99);
        }
    }

    #[test]
    fn inline_upload_survives_descriptor_boundaries_and_writes_live_memory() {
        use exosphere_gpu_ir::inline_to_memory::method;
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, 2 * PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
        gpu.map_fixed(0x101000, PAGE_SIZE, PAGE_SIZE, &backing)
            .unwrap();
        let mut words = vec![0x60010000, 0xa140];
        for (register, value) in [
            (method::OFFSET_OUT_LOWER, 0x100ffc),
            (method::LINE_LENGTH_IN, 8),
            (method::LINE_COUNT, 1),
            (method::LAUNCH_DMA, 0x1001),
            (method::LOAD_INLINE_DATA, 0x11223344),
        ] {
            words.extend([0x60010000 | u32::from(register), value]);
        }
        memory
            .write(
                0x4000,
                &words
                    .iter()
                    .flat_map(|word| word.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let tail = [0x60010000 | u32::from(method::LOAD_INLINE_DATA), 0x55667788];
        memory
            .write(
                0x4080,
                &tail
                    .iter()
                    .flat_map(|word| word.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let mut channel = GuestGpuChannel::new(2).unwrap();
        channel
            .enqueue(&[
                0x100000 | ((words.len() as u64) << 42),
                0x100080 | (2 << 42),
            ])
            .unwrap();
        assert_eq!(
            channel.step(&mut memory, &gpu),
            ChannelStep::Applied { transfers: 0 }
        );
        assert!(channel.has_pending_inline_upload());
        let mut bytes = [0; 8];
        memory.read(0x4ffc, &mut bytes).unwrap();
        assert_eq!(bytes, [0x44, 0x33, 0x22, 0x11, 0, 0, 0, 0]);
        assert_eq!(
            channel.step(&mut memory, &gpu),
            ChannelStep::Applied { transfers: 1 }
        );
        assert!(!channel.has_pending_inline_upload());
        memory.read(0x4ffc, &mut bytes).unwrap();
        assert_eq!(bytes, [0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55]);
        let trace = GuestGpuDecoder::default()
            .decode_pushbuffer(&memory, &gpu, 0x100000, words.len())
            .unwrap();
        memory.write(0x4ffc, &[0xa5; 8]).unwrap();
        for flags in [0, 3, 0x11, 0x101, 0x10001] {
            let mut unsupported = trace.clone();
            let exosphere_gpu_ir::inline_to_memory::InlineToMemoryEvent::Launch { state } =
                &mut unsupported.inline_events[0]
            else {
                unreachable!()
            };
            state.launch_flags = flags;
            assert!(super::super::execute_trace(&mut memory, &gpu, &unsupported).is_err());
            memory.read(0x4ffc, &mut bytes).unwrap();
            assert_eq!(bytes, [0xa5; 8]);
        }
        // Excess data cannot silently extend an upload into neighboring memory.
        channel.enqueue(&[0x100080 | (2 << 42)]).unwrap();
        assert_eq!(channel.step(&mut memory, &gpu), ChannelStep::Stopped);
        assert!(matches!(
            channel.fault(),
            Some(ChannelFault::Execution(GuestGpuExecutionError {
                reason: super::super::GuestGpuExecutionFailure::InvalidInlineSequence,
                ..
            }))
        ));
        let mut untouched = [0; 4];
        memory.read(0x5004, &mut untouched).unwrap();
        assert_eq!(untouched, [0; 4]);
    }

    #[test]
    fn consumes_live_commands_in_order_and_never_replays_a_faulted_prefix() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                3 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, 3 * PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, 3 * PAGE_SIZE, 0, &backing).unwrap();
        let later_command = u32::from(method::LINE_LENGTH_IN) | (8 << 16) | (4 << 29);
        memory.write(0x5000, &later_command.to_le_bytes()).unwrap();
        let mut words = vec![0x60010000, 0xb0b5];
        for (register, value) in [
            (method::OFFSET_IN_LOWER, 0x101000),
            (method::OFFSET_OUT_LOWER, 0x102000),
            (method::LINE_LENGTH_IN, 4),
            (method::LAUNCH_DMA, 0x182),
        ] {
            words.extend([0x60010000 | u32::from(register), value]);
        }
        memory
            .write(
                0x4000,
                &words
                    .iter()
                    .flat_map(|word| word.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let first = 0x100000 | ((words.len() as u64) << 42);
        let later = 0x102000 | (1 << 42);
        let mut channel = GuestGpuChannel::new(2).unwrap();
        assert_eq!(channel.step(&mut memory, &gpu), ChannelStep::Idle);
        assert!(channel.enqueue(&[first, 1u64 << 32]).is_err());
        assert_eq!(channel.pending_entries(), 0);
        channel.enqueue(&[first, later]).unwrap();
        assert!(matches!(
            channel.enqueue(&[later]),
            Err(QueueAdmissionError::Full)
        ));
        assert_eq!(channel.pending_entries(), 2);
        assert_eq!(
            channel.step(&mut memory, &gpu),
            ChannelStep::Applied { transfers: 1 }
        );
        // The second entry was initially zero-filled; its bytes must be fetched
        // after the first transfer, using the retained engine binding.
        assert_eq!(
            channel.step(&mut memory, &gpu),
            ChannelStep::Applied { transfers: 0 }
        );
        assert_eq!(channel.pending_entries(), 0);

        let opaque = 0xfffu32 | (4 << 29);
        memory
            .write(0x4000 + (words.len() * 4) as u64, &opaque.to_le_bytes())
            .unwrap();
        let faulting = 0x100000 | (((words.len() + 1) as u64) << 42);
        let mut channel = GuestGpuChannel::new(2).unwrap();
        channel.enqueue(&[faulting, later]).unwrap();
        assert_eq!(channel.step(&mut memory, &gpu), ChannelStep::Stopped);
        assert!(matches!(
            channel.fault(),
            Some(ChannelFault::Execution(GuestGpuExecutionError {
                completed_transfers: 1,
                ..
            }))
        ));
        assert_eq!(channel.pending_entries(), 2);
        memory.write(0x6000, &[0xa5; 4]).unwrap();
        assert_eq!(channel.step(&mut memory, &gpu), ChannelStep::Stopped);
        let mut bytes = [0; 4];
        memory.read(0x6000, &mut bytes).unwrap();
        assert_eq!(bytes, [0xa5; 4]);
        assert!(matches!(
            channel.enqueue(&[]),
            Err(QueueAdmissionError::Stopped)
        ));
    }
}
