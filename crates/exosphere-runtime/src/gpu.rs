// SPDX-License-Identifier: GPL-3.0-or-later
//! Read live guest GPU command buffers into the existing Maxwell front end.
//! A decoded trace is not backend execution, presentation, or fence completion.

pub mod color_memory;
pub mod index_memory;
pub mod nv_backend;
pub mod pipeline_identity;
pub mod queue;
pub mod resource_memory;
pub mod shader_identity;
pub mod shader_memory;

#[cfg(test)]
mod native_tests;

use exosphere_gpu_ir::{FrontEndTrace, MaxwellFrontEnd, PfifoError};
use exosphere_horizon::{
    HorizonError, memory::AddressSpace, nvdrv::address_space::NvGpuAddressSpace,
};

// GPFIFO carries a 21-bit count of 32-bit pushbuffer words.
const MAX_PUSHBUFFER_WORDS: usize = 0x1fffff;

#[derive(Debug, thiserror::Error)]
pub enum GuestGpuExecutionFailure {
    #[error("incomplete, duplicated or invalid trace event ordering")]
    InvalidTrace,
    #[error("unsupported inline upload flags or geometry")]
    UnsupportedInlineUpload,
    #[error("inline data without a launch, or replacement of an incomplete upload")]
    InvalidInlineSequence,
    #[error("standalone trace ends before its inline upload completes")]
    IncompleteInlineUpload,
    #[error("unsupported FIFO semaphore control or incomplete prior GPU operation")]
    UnsupportedHostSemaphore,
    #[error("FIFO semaphore equality acquire is waiting")]
    SemaphoreWait,
    #[error("unsupported guest GPU event {0:?}")]
    UnsupportedEvent(exosphere_gpu_ir::frontend::FrontEndEvent),
    #[error(transparent)]
    Transfer(GuestGpuDecodeError),
    #[error("native draw failed: {0}")]
    Draw(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// A failure may follow successfully applied transfers. Retrying the entire
/// trace would execute that prefix again; a channel scheduler must retain this
/// position and implement the appropriate guest fault policy.
/// The count includes complete transfers only; partial inline-upload bytes may
/// already be visible even when no upload has completed.
#[derive(Debug, thiserror::Error)]
#[error(
    "guest GPU execution stopped at event {event_index} after {completed_transfers} transfers: {reason}"
)]
pub struct GuestGpuExecutionError {
    pub event_index: usize,
    pub completed_transfers: usize,
    #[source]
    pub reason: GuestGpuExecutionFailure,
}

/// Execute supported events in front-end order. Success means only synchronous
/// memory effects, not a queue fence, rendering or presentation.
pub fn execute_trace(
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    trace: &FrontEndTrace,
) -> Result<usize, GuestGpuExecutionError> {
    let mut executor = GuestGpuExecutor::default();
    let completed_transfers = executor.execute(memory, gpu, trace)?;
    if executor.inline.is_some() {
        return Err(GuestGpuExecutionError {
            event_index: trace.events.len(),
            completed_transfers,
            reason: GuestGpuExecutionFailure::IncompleteInlineUpload,
        });
    }
    Ok(completed_transfers)
}

#[derive(Debug)]
struct InlineUpload {
    destination: u64,
    size: usize,
    written: usize,
}

/// Per-channel execution state, including inline uploads split over descriptors.
#[derive(Debug, Default)]
pub struct GuestGpuExecutor {
    inline: Option<InlineUpload>,
}

impl GuestGpuExecutor {
    pub fn has_pending_inline_upload(&self) -> bool {
        self.inline.is_some()
    }

    fn execute_inline(
        &mut self,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        event: &exosphere_gpu_ir::inline_to_memory::InlineToMemoryEvent,
    ) -> Result<bool, GuestGpuExecutionFailure> {
        use exosphere_gpu_ir::inline_to_memory::InlineToMemoryEvent;
        match event {
            InlineToMemoryEvent::Launch { state } => {
                if self.inline.is_some() {
                    return Err(GuestGpuExecutionFailure::InvalidInlineSequence);
                }
                // A140 bit 0 selects pitch layout. Bits 6 and 12 select a
                // disabled sysmembar and unused semaphore size respectively.
                // Flush, interrupt, semaphore and reduction modes are not implemented.
                if state.launch_flags & 1 == 0
                    || state.launch_flags & !0x1041 != 0
                    || state.line_count != 1
                    || state.line_length == 0
                    || state.line_length % 4 != 0
                    || state.line_length > 64 * 1024 * 1024
                {
                    return Err(GuestGpuExecutionFailure::UnsupportedInlineUpload);
                }
                let destination = state.destination.high_low();
                let size = state.line_length as usize;
                gpu.validate_access(
                    memory,
                    destination,
                    size,
                    exosphere_horizon::memory::MemoryPermission::WRITE,
                )
                .map_err(|error| {
                    GuestGpuExecutionFailure::Transfer(GuestGpuDecodeError::Memory(error))
                })?;
                self.inline = Some(InlineUpload {
                    destination,
                    size,
                    written: 0,
                });
                Ok(false)
            }
            InlineToMemoryEvent::Data { value } => {
                let upload = self
                    .inline
                    .as_mut()
                    .ok_or(GuestGpuExecutionFailure::InvalidInlineSequence)?;
                gpu.write(
                    memory,
                    upload.destination + upload.written as u64,
                    &value.to_le_bytes(),
                )
                .map_err(|error| {
                    GuestGpuExecutionFailure::Transfer(GuestGpuDecodeError::Memory(error))
                })?;
                upload.written += 4;
                let complete = upload.written == upload.size;
                if complete {
                    self.inline = None;
                }
                Ok(complete)
            }
            InlineToMemoryEvent::Opaque { .. } => {
                Err(GuestGpuExecutionFailure::UnsupportedInlineUpload)
            }
        }
    }

    pub fn execute(
        &mut self,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        trace: &FrontEndTrace,
    ) -> Result<usize, GuestGpuExecutionError> {
        self.execute_from(memory, gpu, trace, 0)
    }

    /// Resume at the first unexecuted event. Callers must retain the original
    /// trace and never replay its already-applied prefix after a semaphore wait.
    pub(crate) fn execute_from(
        &mut self,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        trace: &FrontEndTrace,
        start: usize,
    ) -> Result<usize, GuestGpuExecutionError> {
        self.execute_from_with_draw_handler(memory, gpu, trace, start, |_, _, _, event| {
            Err(GuestGpuExecutionFailure::UnsupportedEvent(event))
        })
    }

    /// Execute or resume a trace with a synchronous native draw implementation.
    /// The handler must finish rendering and guest-memory writeback before it
    /// returns success: submitting work alone must not allow a later semaphore
    /// to signal completion. Failed draws stop the trace before its suffix.
    /// `start` must be the first unexecuted event of this same immutable trace.
    /// Draws are not included in the returned count of completed transfers.
    pub fn execute_from_with_draw_handler(
        &mut self,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        trace: &FrontEndTrace,
        start: usize,
        mut draw: impl FnMut(
            &mut AddressSpace,
            &NvGpuAddressSpace,
            &exosphere_gpu_ir::MaxwellEvent,
            exosphere_gpu_ir::frontend::FrontEndEvent,
        ) -> Result<(), GuestGpuExecutionFailure>,
    ) -> Result<usize, GuestGpuExecutionError> {
        use exosphere_gpu_ir::{dma::DmaEvent, frontend::FrontEndEvent};
        if !trace.has_complete_event_order() || start > trace.events.len() {
            return Err(GuestGpuExecutionError {
                event_index: 0,
                completed_transfers: 0,
                reason: GuestGpuExecutionFailure::InvalidTrace,
            });
        }
        let mut completed_transfers = 0;
        for (event_index, event) in trace.events.iter().copied().enumerate().skip(start) {
            let result = match event {
                FrontEndEvent::Graphics(index) => match trace.graphics_events[index] {
                    // These events only report decoded state. Draws carry their
                    // own immutable snapshot; no device operation or fence is
                    // completed by observing the state-change notification.
                    exosphere_gpu_ir::MaxwellEvent::ConstantBufferStateChanged
                    | exosphere_gpu_ir::MaxwellEvent::ColorTargetChanged { .. }
                    | exosphere_gpu_ir::MaxwellEvent::PipelineShaderChanged { .. }
                    | exosphere_gpu_ir::MaxwellEvent::ProgramRegionChanged(_)
                    | exosphere_gpu_ir::MaxwellEvent::Begin { .. }
                    | exosphere_gpu_ir::MaxwellEvent::End => Ok(false),
                    exosphere_gpu_ir::MaxwellEvent::ConstantBufferUpload {
                        address: Some(address),
                        word,
                    } => gpu
                        .write(memory, address, &word.to_le_bytes())
                        .map(|()| true)
                        .map_err(|error| {
                            GuestGpuExecutionFailure::Transfer(GuestGpuDecodeError::Memory(error))
                        }),
                    exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { .. }
                    | exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer { .. } => {
                        if self.inline.is_some() {
                            Err(GuestGpuExecutionFailure::InvalidInlineSequence)
                        } else {
                            draw(memory, gpu, &trace.graphics_events[index], event).map(|()| false)
                        }
                    }
                    _ => Err(GuestGpuExecutionFailure::UnsupportedEvent(event)),
                },
                FrontEndEvent::Dma(index) => match trace.dma_events[index] {
                    DmaEvent::Launch(launch) => execute_dma(memory, gpu, launch)
                        .map(|()| true)
                        .map_err(GuestGpuExecutionFailure::Transfer),
                    DmaEvent::Opaque { .. } => {
                        Err(GuestGpuExecutionFailure::UnsupportedEvent(event))
                    }
                },
                FrontEndEvent::Inline(index) => {
                    self.execute_inline(memory, gpu, &trace.inline_events[index])
                }
                FrontEndEvent::HostSemaphore(index) => {
                    let operation = trace.host_semaphore_operations[index];
                    let reduction = exosphere_gpu_ir::host_semaphore::SemaphoreReduction::decode(
                        operation.control,
                    );
                    // Equality/bit-mask acquire, or four-byte release/semaphore
                    // reduction with WFI enabled. Other controls stay rejected.
                    // Prior supported DMA/inline effects are synchronous; do
                    // not signal while a split upload still needs more data.
                    if (!matches!(operation.control, 1 | 8 | 0x0100_0002) && reduction.is_none())
                        || !operation.address_registers_valid
                        || self.inline.is_some()
                    {
                        Err(GuestGpuExecutionFailure::UnsupportedHostSemaphore)
                    } else if let Some(reduction) = reduction {
                        // CPU/channel execution shares one mutable AddressSpace:
                        // this synchronous read-modify-write cannot interleave
                        // with another modeled transfer. GPU write preflights
                        // all permissions before committing any destination byte.
                        let mut bytes = [0; 4];
                        gpu.read(memory, operation.address, &mut bytes)
                            .and_then(|()| {
                                let value =
                                    reduction.apply(u32::from_le_bytes(bytes), operation.payload);
                                gpu.write(memory, operation.address, &value.to_le_bytes())
                            })
                            .map(|()| false)
                            .map_err(|error| {
                                GuestGpuExecutionFailure::Transfer(GuestGpuDecodeError::Memory(
                                    error,
                                ))
                            })
                    } else if matches!(operation.control, 1 | 8) {
                        let mut value = [0; 4];
                        gpu.read(memory, operation.address, &mut value)
                            .map_err(|error| {
                                GuestGpuExecutionFailure::Transfer(GuestGpuDecodeError::Memory(
                                    error,
                                ))
                            })
                            .and_then(|()| {
                                let value = u32::from_le_bytes(value);
                                let satisfied = if operation.control == 1 {
                                    value == operation.payload
                                } else {
                                    value & operation.payload != 0
                                };
                                if satisfied {
                                    Ok(false)
                                } else {
                                    Err(GuestGpuExecutionFailure::SemaphoreWait)
                                }
                            })
                    } else {
                        gpu.write(memory, operation.address, &operation.payload.to_le_bytes())
                            .map(|()| false)
                            .map_err(|error| {
                                GuestGpuExecutionFailure::Transfer(GuestGpuDecodeError::Memory(
                                    error,
                                ))
                            })
                    }
                }
                FrontEndEvent::HostWaitForIdle { scope } => {
                    // All supported preceding transfers execute synchronously.
                    // Unsupported earlier events already stop in stream order;
                    // a split upload is not a completed transfer/barrier.
                    if scope <= 1 && self.inline.is_none() {
                        Ok(false)
                    } else {
                        Err(GuestGpuExecutionFailure::UnsupportedEvent(event))
                    }
                }
                _ => Err(GuestGpuExecutionFailure::UnsupportedEvent(event)),
            };
            match result {
                Err(reason) => {
                    return Err(GuestGpuExecutionError {
                        event_index,
                        completed_transfers,
                        reason,
                    });
                }
                Ok(complete) => completed_transfers += usize::from(complete),
            }
        }
        Ok(completed_transfers)
    }
}

/// Execute a synchronous virtual pitch-layout DMA launch against live nvmap
/// memory. Pipelined execution, cache effects and semaphore completion require
/// channel scheduling and are explicitly unsupported here.
pub fn execute_dma(
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    launch: exosphere_gpu_ir::dma::DmaLaunch,
) -> Result<(), GuestGpuDecodeError> {
    let shape = launch
        .linear_shape()
        .ok_or(GuestGpuDecodeError::UnsupportedDmaLaunch)?;
    if launch.flags & 3 != 2 || shape.rows > 65_536 {
        return Err(GuestGpuDecodeError::UnsupportedDmaLaunch);
    }
    let mut ranges = Vec::new();
    ranges
        .try_reserve_exact(shape.rows as usize)
        .map_err(|_| GuestGpuDecodeError::InvalidRange)?;
    for row in 0..shape.rows {
        let source = launch
            .state
            .source
            .high_low()
            .checked_add(u64::from(row) * u64::from(shape.source_stride))
            .ok_or(GuestGpuDecodeError::InvalidRange)?;
        let destination = launch
            .state
            .destination
            .high_low()
            .checked_add(u64::from(row) * u64::from(shape.destination_stride))
            .ok_or(GuestGpuDecodeError::InvalidRange)?;
        ranges.push((source, destination, shape.bytes_per_row as usize));
    }
    gpu.copy_ranges(memory, &ranges)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum GuestGpuDecodeError {
    #[error("unsupported DMA launch mode or execution policy")]
    UnsupportedDmaLaunch,
    #[error("unsupported GPFIFO control, privilege, fetch, level or synchronization mode")]
    UnsupportedGpfifoEntry,
    #[error("invalid or oversized guest pushbuffer")]
    InvalidRange,
    #[error(transparent)]
    Memory(#[from] HorizonError),
    #[error(transparent)]
    Packet(#[from] PfifoError),
}

/// One front end per channel, preserving engine state between guest buffers.
#[derive(Debug, Default)]
pub struct GuestGpuDecoder {
    frontend: MaxwellFrontEnd,
}

impl GuestGpuDecoder {
    pub fn decode_gpfifo_entry(
        &mut self,
        memory: &AddressSpace,
        gpu: &NvGpuAddressSpace,
        raw: u64,
    ) -> Result<FrontEndTrace, GuestGpuDecodeError> {
        use exosphere_gpu_ir::gpfifo::GpfifoEntry;
        let entry = GpfifoEntry::decode(raw).map_err(|_| GuestGpuDecodeError::InvalidRange)?;
        if entry.is_nop() {
            return Ok(FrontEndTrace::default());
        }
        let GpfifoEntry::Pushbuffer(entry) = entry else {
            return Err(GuestGpuDecodeError::UnsupportedGpfifoEntry);
        };
        if entry.conditional_fetch || entry.privileged || entry.subroutine || entry.wait {
            return Err(GuestGpuDecodeError::UnsupportedGpfifoEntry);
        }
        self.decode_pushbuffer(memory, gpu, entry.address, entry.word_count as usize)
    }

    pub fn decode_gpfifo_at(
        &mut self,
        memory: &AddressSpace,
        gpu: &NvGpuAddressSpace,
        address: u64,
    ) -> Result<FrontEndTrace, GuestGpuDecodeError> {
        if address % 8 != 0 {
            return Err(GuestGpuDecodeError::InvalidRange);
        }
        let mut entry = [0; 8];
        gpu.read(memory, address, &mut entry)?;
        self.decode_gpfifo_entry(memory, gpu, u64::from_le_bytes(entry))
    }

    pub fn decode_pushbuffer(
        &mut self,
        memory: &AddressSpace,
        gpu: &NvGpuAddressSpace,
        address: u64,
        word_count: usize,
    ) -> Result<FrontEndTrace, GuestGpuDecodeError> {
        if address % 4 != 0 || word_count == 0 || word_count > MAX_PUSHBUFFER_WORDS {
            return Err(GuestGpuDecodeError::InvalidRange);
        }
        let size = word_count
            .checked_mul(4)
            .ok_or(GuestGpuDecodeError::InvalidRange)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| GuestGpuDecodeError::InvalidRange)?;
        bytes.resize(size, 0);
        // Read the whole requested range before touching channel engine state.
        // Commands come from the actual nvmap lease, never from a shadow buffer.
        gpu.read(memory, address, &mut bytes)?;
        let words: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect();
        Ok(self.frontend.process_pushbuffer(&words)?)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fifo_idle_barrier_requires_supported_prefix_and_complete_uploads() {
        use exosphere_gpu_ir::{MaxwellFrontEnd, frontend::FrontEndEvent};
        let mut memory = AddressSpace::default();
        let gpu = NvGpuAddressSpace::default();
        for scope in [0, 1, 2, u32::MAX] {
            let trace = MaxwellFrontEnd::new()
                .process_pushbuffer(&[0x6001001e, scope])
                .unwrap();
            assert!(trace.has_complete_event_order());
            assert_eq!(trace.events, [FrontEndEvent::HostWaitForIdle { scope }]);
            let result = execute_trace(&mut memory, &gpu, &trace);
            if scope <= 1 {
                assert_eq!(result.unwrap(), 0);
            } else {
                assert_eq!(result.unwrap_err().event_index, 0);
            }
        }
        let trace = MaxwellFrontEnd::new()
            .process_pushbuffer(&[0x6001001e, 0])
            .unwrap();
        let mut executor = GuestGpuExecutor {
            inline: Some(InlineUpload {
                destination: 0x100000,
                size: 4,
                written: 0,
            }),
        };
        assert_eq!(
            executor
                .execute(&mut memory, &gpu, &trace)
                .unwrap_err()
                .event_index,
            0
        );
        assert!(executor.has_pending_inline_upload());
        let unknown_prefix = MaxwellFrontEnd::new()
            .process_pushbuffer(&[0x60010100, 42, 0x6001001e, 0])
            .unwrap();
        let error = execute_trace(&mut memory, &gpu, &unknown_prefix).unwrap_err();
        assert_eq!(error.event_index, 0);
        assert_eq!(error.completed_transfers, 0);
    }

    #[test]
    fn fifo_release_writes_live_memory_and_rejects_unimplemented_controls() {
        use exosphere_horizon::{
            memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
            nvdrv::NvMapBacking,
        };
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
        let setup = [0x20030004u32, 0, 0x100800, 0x12345678];
        memory
            .write(
                0x4000,
                &setup
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let mut decoder = GuestGpuDecoder::default();
        let mut executor = GuestGpuExecutor::default();
        let trace = decoder
            .decode_pushbuffer(&memory, &gpu, 0x100000, 4)
            .unwrap();
        assert!(trace.events.is_empty());
        assert_eq!(executor.execute(&mut memory, &gpu, &trace).unwrap(), 0);
        // Registers survive a descriptor boundary before the release operation.
        for control in [
            0x01000002u32,
            2,
            1,
            0x01100002,
            0x11000010,
            0x19000010,
            0x21000010,
            0x91000010,
            0x99000010,
            0xa1000010,
            0x10000010,
            0x11100010,
            0x11001010,
            0x01000010,
            0x81000010,
            0x09000010,
            0x89000010,
            0x29000010,
            0xa9000010,
            0xb1000010,
            0xb9000010,
            0x31000010,
            0x39000010,
        ] {
            memory
                .write(
                    0x4100,
                    &[0x60010007u32.to_le_bytes(), control.to_le_bytes()].concat(),
                )
                .unwrap();
            memory.write(0x4800, &[0xa5; 8]).unwrap();
            let trace = decoder
                .decode_pushbuffer(&memory, &gpu, 0x100100, 2)
                .unwrap();
            assert!(trace.has_complete_event_order());
            let result = executor.execute(&mut memory, &gpu, &trace);
            let mut bytes = [0; 8];
            memory.read(0x4800, &mut bytes).unwrap();
            if let Some(reduction) =
                exosphere_gpu_ir::host_semaphore::SemaphoreReduction::decode(control)
            {
                assert_eq!(result.unwrap(), 0);
                let expected = reduction.apply(0xa5a5a5a5, 0x12345678);
                assert_eq!(&bytes[..4], &expected.to_le_bytes());
                assert_eq!(&bytes[4..], &[0xa5; 4]);
            } else if control == 0x01000002 {
                assert_eq!(result.unwrap(), 0);
                assert_eq!(&bytes[..4], &0x12345678u32.to_le_bytes());
                assert_eq!(&bytes[4..], &[0xa5; 4]);
            } else {
                let reason = result.unwrap_err().reason;
                if control == 1 {
                    assert!(matches!(reason, GuestGpuExecutionFailure::SemaphoreWait));
                } else {
                    assert!(matches!(
                        reason,
                        GuestGpuExecutionFailure::UnsupportedHostSemaphore
                    ));
                }
                assert_eq!(bytes, [0xa5; 8]);
            }
        }
        let readonly_backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 0,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ)
                .unwrap(),
        };
        let mut readonly_gpu = NvGpuAddressSpace::default();
        readonly_gpu
            .map_fixed(0x100000, PAGE_SIZE, 0, &readonly_backing)
            .unwrap();
        for control in [
            0x11000010u32,
            0x01000010,
            0x89000010,
            0x29000010,
            0xb1000010,
            0xb9000010,
        ] {
            memory
                .write(
                    0x4100,
                    &[0x60010007u32.to_le_bytes(), control.to_le_bytes()].concat(),
                )
                .unwrap();
            memory.write(0x4800, &[0xa5; 8]).unwrap();
            let trace = decoder
                .decode_pushbuffer(&memory, &readonly_gpu, 0x100100, 2)
                .unwrap();
            assert!(
                executor
                    .execute(&mut memory, &readonly_gpu, &trace)
                    .is_err()
            );
            let mut unchanged = [0; 8];
            memory.read(0x4800, &mut unchanged).unwrap();
            assert_eq!(unchanged, [0xa5; 8]);
        }
    }

    use super::*;
    use exosphere_horizon::{
        memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
        nvdrv::NvMapBacking,
    };

    #[test]
    fn decodes_live_gpu_mapped_guest_commands_and_preserves_state_on_faults() {
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
        // Nonincrementing SET_OBJECT binds Maxwell B; immediate BEGIN selects triangles.
        let begin =
            u32::from(exosphere_gpu_ir::maxwell_state::method::BEGIN) | (4 << 16) | (4 << 29);
        memory
            .write(
                0x4ff8,
                &[
                    0x60010000u32.to_le_bytes(),
                    0xb197u32.to_le_bytes(),
                    begin.to_le_bytes(),
                ]
                .concat(),
            )
            .unwrap();
        let mut decoder = GuestGpuDecoder::default();
        let entry = 0x100ff8u64 | (3 << 42);
        memory.write(0x4800, &entry.to_le_bytes()).unwrap();
        let trace = decoder.decode_gpfifo_at(&memory, &gpu, 0x100800).unwrap();
        assert_eq!(trace.method_writes, 2);
        assert_eq!(trace.graphics_state.primitive_raw, Some(4));
        for flag in [1u64, 1 << 40, 1 << 41, 1 << 63] {
            assert!(matches!(
                decoder.decode_gpfifo_entry(&memory, &gpu, entry | flag),
                Err(GuestGpuDecodeError::UnsupportedGpfifoEntry)
            ));
        }
        assert!(matches!(
            decoder.decode_gpfifo_entry(&memory, &gpu, 1u64 << 32),
            Err(GuestGpuDecodeError::UnsupportedGpfifoEntry)
        ));
        assert!(decoder.decode_gpfifo_at(&memory, &gpu, 0x100801).is_err());
        assert!(decoder.decode_gpfifo_at(&memory, &gpu, 0x102000).is_err());
        assert!(
            decoder
                .decode_pushbuffer(&memory, &gpu, 0x100fff, 1)
                .is_err()
        );
        assert!(
            decoder
                .decode_pushbuffer(&memory, &gpu, 0x102000, 1)
                .is_err()
        );
        assert!(
            decoder
                .decode_pushbuffer(&memory, &gpu, 0x100000, MAX_PUSHBUFFER_WORDS + 1)
                .is_err()
        );
        memory.write(0x4000, &0x60010000u32.to_le_bytes()).unwrap();
        assert!(matches!(
            decoder.decode_pushbuffer(&memory, &gpu, 0x100000, 1),
            Err(GuestGpuDecodeError::Packet(_))
        ));
        assert_eq!(decoder.frontend.graphics_state().primitive_raw, Some(4));
        let draw = u32::from(exosphere_gpu_ir::maxwell_state::method::DRAW_VERTEX_ARRAY)
            | (3 << 16)
            | (4 << 29);
        memory.write(0x4000, &draw.to_le_bytes()).unwrap();
        let trace = decoder
            .decode_pushbuffer(&memory, &gpu, 0x100000, 1)
            .unwrap();
        assert!(trace.graphics_events.iter().any(|event| matches!(
            event,
            exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { count: 3, .. }
        )));
        assert_eq!(trace.graphics_state.primitive_raw, Some(4));
    }

    #[test]
    fn executes_decoded_dma_against_live_memory_and_rejects_partial_transfers() {
        use exosphere_gpu_ir::dma::{DmaEvent, method};
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
        // Source rows straddle the mapping boundary; destination rows use a
        // different pitch and must remain coherent with the CPU view.
        memory.write(0x4ffc, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        memory
            .write(0x500c, &[9, 10, 11, 12, 13, 14, 15, 16])
            .unwrap();
        let mut words = vec![0x60010000, 0xb0b5];
        for (register, value) in [
            (method::OFFSET_IN_LOWER, 0x100ffc),
            (method::OFFSET_OUT_LOWER, 0x100800),
            (method::PITCH_IN, 16),
            (method::PITCH_OUT, 32),
            (method::LINE_LENGTH_IN, 8),
            (method::LINE_COUNT, 2),
            (method::LAUNCH_DMA, 0x382),
        ] {
            words.extend([0x60010000 | u32::from(register), value]);
        }
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        memory.write(0x4000, &bytes).unwrap();
        let mut trace = GuestGpuDecoder::default()
            .decode_pushbuffer(&memory, &gpu, 0x100000, words.len())
            .unwrap();
        let DmaEvent::Launch(mut launch) = trace.dma_events[0] else {
            panic!("expected DMA launch")
        };
        assert_eq!(execute_trace(&mut memory, &gpu, &trace).unwrap(), 1);
        let mut output = [0; 40];
        memory.read(0x4800, &mut output).unwrap();
        assert_eq!(&output[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&output[8..32], &[0; 24]);
        assert_eq!(&output[32..], &[9, 10, 11, 12, 13, 14, 15, 16]);

        use exosphere_gpu_ir::frontend::FrontEndEvent;
        memory.write(0x4900, &[0x5a; 8]).unwrap();
        trace
            .graphics_events
            .push(exosphere_gpu_ir::MaxwellEvent::DrawVertexArray {
                first: 0,
                count: 3,
                state: Box::default(),
            });
        let mut later = launch;
        later.state.destination.b = 0x100900;
        later.state.line_count = 1;
        trace.dma_events.push(DmaEvent::Launch(later));
        trace
            .events
            .extend([FrontEndEvent::Graphics(0), FrontEndEvent::Dma(1)]);
        let error = execute_trace(&mut memory, &gpu, &trace).unwrap_err();
        assert_eq!(error.event_index, 1);
        assert_eq!(error.completed_transfers, 1);
        assert!(matches!(
            error.reason,
            GuestGpuExecutionFailure::UnsupportedEvent(FrontEndEvent::Graphics(0))
        ));
        let mut untouched = [0; 8];
        memory.read(0x4900, &mut untouched).unwrap();
        assert_eq!(untouched, [0x5a; 8]);

        // The native hook observes prior transfers, and a failure must not run
        // the suffix. Resuming the same trace must not replay that prefix.
        let mut executor = GuestGpuExecutor::default();
        let stopped = executor
            .execute_from_with_draw_handler(
                &mut memory,
                &gpu,
                &trace,
                0,
                |memory, gpu, draw, event| {
                    assert!(matches!(
                        draw,
                        exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { count: 3, .. }
                    ));
                    let mut bytes = [0; 8];
                    gpu.read(memory, 0x100800, &mut bytes).unwrap();
                    assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
                    Err(GuestGpuExecutionFailure::UnsupportedEvent(event))
                },
            )
            .unwrap_err();
        assert_eq!(stopped.event_index, 1);
        assert_eq!(stopped.completed_transfers, 1);
        memory.read(0x4900, &mut untouched).unwrap();
        assert_eq!(untouched, [0x5a; 8]);
        memory.write(0x4800, &[0xa5; 8]).unwrap();
        let mut draws = 0;
        let transfers = executor
            .execute_from_with_draw_handler(
                &mut memory,
                &gpu,
                &trace,
                stopped.event_index,
                |memory, gpu, _, _| {
                    let mut bytes = [0; 8];
                    gpu.read(memory, 0x100800, &mut bytes).unwrap();
                    assert_eq!(bytes, [0xa5; 8]);
                    draws += 1;
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(draws, 1);
        assert_eq!(transfers, 1);
        memory.read(0x4900, &mut untouched).unwrap();
        assert_eq!(untouched, [1, 2, 3, 4, 5, 6, 7, 8]);

        // Malformed public traces must fail before even the first valid copy.
        memory.write(0x4800, &[0xa5; 40]).unwrap();
        for events in [
            vec![FrontEndEvent::Dma(0)],
            vec![
                FrontEndEvent::Dma(0),
                FrontEndEvent::Graphics(0),
                FrontEndEvent::Dma(0),
            ],
            vec![FrontEndEvent::Dma(5)],
        ] {
            trace.events = events;
            assert!(matches!(
                execute_trace(&mut memory, &gpu, &trace).unwrap_err().reason,
                GuestGpuExecutionFailure::InvalidTrace
            ));
            memory.read(0x4800, &mut output).unwrap();
            assert_eq!(output, [0xa5; 40]);
        }

        memory.write(0x4800, &[0xa5; 40]).unwrap();
        launch.state.pitch_out = 0x2000;
        assert!(execute_dma(&mut memory, &gpu, launch).is_err());
        memory.read(0x4800, &mut output).unwrap();
        assert_eq!(output, [0xa5; 40]);
        // A different GPU VA may alias the same source backing.
        gpu.map_fixed(0x200000, PAGE_SIZE, PAGE_SIZE, &backing)
            .unwrap();
        launch.state.destination.b = 0x20000c;
        launch.state.pitch_out = 32;
        assert!(execute_dma(&mut memory, &gpu, launch).is_err());
        launch.flags = 0x381;
        assert!(matches!(
            execute_dma(&mut memory, &gpu, launch),
            Err(GuestGpuDecodeError::UnsupportedDmaLaunch)
        ));
    }
}
