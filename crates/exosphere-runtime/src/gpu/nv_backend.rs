// SPDX-License-Identifier: GPL-3.0-or-later
//! Native queue allocation and unsynchronized submission with live execution.

use super::queue::GuestGpuChannel;
use exosphere_horizon::{
    HorizonError, HorizonResult,
    ipc::{IpcBufferWrite, ServiceRequest, ServiceResponse},
    memory::AddressSpace,
    nvdrv::{
        NvGpuChannelIdentity,
        address_space::NvGpuAddressSpace,
        allocation::GpfifoAllocationRequest,
        backend::NvGpuIoctlBackend,
        object_context::ObjectContextRequest,
        submission::{GpfifoSubmissionRequest, SubmissionFence},
        syncpoint::NvGpuSyncpoints,
    },
};

#[derive(Debug)]
struct Channel {
    identity: NvGpuChannelIdentity,
    queue: GuestGpuChannel,
    fence: SubmissionFence,
    graphics_object: Option<u64>,
}

/// Host renderer owned by the driver backend, not by guest IPC handles.
/// Success requires completed rendering and canonical guest writeback. Resource
/// caches must be isolated by the authenticated channel identity supplied here.
pub trait NativeDrawRenderer: std::fmt::Debug {
    fn draw(
        &mut self,
        identity: &NvGpuChannelIdentity,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        event: &exosphere_gpu_ir::MaxwellEvent,
    ) -> Result<(), super::GuestGpuExecutionFailure>;

    /// Idempotent cleanup: teardown may be repeated after process exit or for
    /// an identity that never acquired renderer resources.
    fn close_channel(&mut self, _identity: &NvGpuChannelIdentity) {}
}

#[derive(Debug)]
pub struct NativeGpuBackend {
    channels: Vec<Channel>,
    points: NvGpuSyncpoints,
    maximum_channels: usize,
    next_object_id: u64,
    renderer: Option<Box<dyn NativeDrawRenderer>>,
}

impl NativeGpuBackend {
    fn allocate_graphics_object(
        &mut self,
        identity: NvGpuChannelIdentity,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let decoded = ObjectContextRequest::decode(request)?;
        if decoded.class != u32::from(exosphere_gpu_ir::engine::CLASS_MAXWELL_B)
            || decoded.flags != 0
        {
            return Err(HorizonError::InvalidState(
                "unsupported GPU object class or flags".into(),
            ));
        }
        let channel = self
            .channels
            .iter_mut()
            .find(|channel| channel.identity == identity)
            .ok_or_else(|| {
                HorizonError::InvalidState("GPU object requires an allocated channel queue".into())
            })?;
        if channel.graphics_object.is_some() {
            return Err(HorizonError::InvalidState(
                "GPU graphics object already allocated".into(),
            ));
        }
        let next = self
            .next_object_id
            .checked_add(1)
            .ok_or_else(|| HorizonError::InvalidState("GPU object identities exhausted".into()))?;
        let mut data = request.buffers[decoded.input_index].data.clone();
        let id = self.next_object_id;
        data[8..16].copy_from_slice(&id.to_le_bytes());
        // The object owns this queue's existing per-channel Maxwell decoder
        // state; its ID is neither taken from output bytes nor recycled.
        channel.graphics_object = Some(id);
        self.next_object_id = next;
        let mut response = ServiceResponse::success();
        response.raw_payload = 0u32.to_le_bytes().to_vec();
        response.buffer_writes.push(IpcBufferWrite {
            buffer_index: decoded.output_index,
            data,
        });
        Ok(response)
    }

    /// Consume a bounded amount of actual work; waiting heads retain their
    /// cursor and faults remain durable. No guest fence is synthesized here.
    pub fn poll_channel(
        &mut self,
        identity: &NvGpuChannelIdentity,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        maximum_entries: usize,
    ) -> HorizonResult<usize> {
        Self::poll_channels(
            &mut self.channels,
            identity,
            memory,
            gpu,
            maximum_entries,
            |memory, gpu, draw, event| match self.renderer.as_mut() {
                Some(renderer) => renderer.draw(identity, memory, gpu, draw),
                None => Err(super::GuestGpuExecutionFailure::UnsupportedEvent(event)),
            },
        )
    }

    /// Poll an admitted native channel using synchronous rendering/writeback.
    /// The caller must supply the canonical live address space for `identity`,
    /// just as for transfer-only polling. No submission fence is synthesized.
    pub fn poll_channel_with_draw_handler(
        &mut self,
        identity: &NvGpuChannelIdentity,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        maximum_entries: usize,
        draw: impl FnMut(
            &mut AddressSpace,
            &NvGpuAddressSpace,
            &exosphere_gpu_ir::MaxwellEvent,
            exosphere_gpu_ir::frontend::FrontEndEvent,
        ) -> Result<(), super::GuestGpuExecutionFailure>,
    ) -> HorizonResult<usize> {
        Self::poll_channels(
            &mut self.channels,
            identity,
            memory,
            gpu,
            maximum_entries,
            draw,
        )
    }

    fn poll_channels(
        channels: &mut [Channel],
        identity: &NvGpuChannelIdentity,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        maximum_entries: usize,
        mut draw: impl FnMut(
            &mut AddressSpace,
            &NvGpuAddressSpace,
            &exosphere_gpu_ir::MaxwellEvent,
            exosphere_gpu_ir::frontend::FrontEndEvent,
        ) -> Result<(), super::GuestGpuExecutionFailure>,
    ) -> HorizonResult<usize> {
        if maximum_entries == 0 || maximum_entries > 4096 {
            return Err(HorizonError::InvalidState(
                "invalid GPU polling budget".into(),
            ));
        }
        let channel = channels
            .iter_mut()
            .find(|channel| channel.identity == *identity)
            .ok_or_else(|| {
                HorizonError::InvalidState("GPU channel has no allocated queue".into())
            })?;
        let mut retired = 0;
        for _ in 0..maximum_entries {
            match channel.queue.step_with_draw_handler(memory, gpu, &mut draw) {
                super::queue::ChannelStep::Applied { .. } => retired += 1,
                super::queue::ChannelStep::Idle | super::queue::ChannelStep::Waiting { .. } => {
                    break;
                }
                super::queue::ChannelStep::Stopped => {
                    return Err(HorizonError::InvalidState(format!(
                        "GPU channel stopped: {:?}",
                        channel.queue.fault()
                    )));
                }
            }
        }
        Ok(retired)
    }

    fn submit(
        &mut self,
        identity: NvGpuChannelIdentity,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let channel = self
            .channels
            .iter_mut()
            .find(|channel| channel.identity == identity)
            .ok_or_else(|| {
                HorizonError::InvalidState("GPU channel has no allocated queue".into())
            })?;
        let decoded = GpfifoSubmissionRequest::decode(request, channel.queue.capacity())
            .map_err(|error| HorizonError::IpcProtocol(error.to_string()))?;
        // NVHOST_SUBMIT_GPFIFO_FLAGS_HW_FORMAT is bit 2. Other flags request
        // separate synchronization behavior and must not be silently ignored.
        // https://android.googlesource.com/kernel/tegra.git/+/android-tegra-3.10/drivers/gpu/nvgpu/gk20a/channel_gk20a.c
        if decoded.submission.flags != 4
            || decoded.submission.fence != (SubmissionFence { id: 0, value: 0 })
        {
            return Err(HorizonError::InvalidState(
                "GPU submission synchronization flags are unsupported".into(),
            ));
        }
        let entries: Vec<_> = decoded.submission.entries().collect();
        let input = request
            .buffers
            .iter()
            .find(|buffer| !buffer.data.is_empty())
            .ok_or_else(|| HorizonError::IpcProtocol("missing GPU submission input".into()))?;
        let data = input.data.clone();
        channel
            .queue
            .enqueue(&entries)
            .map_err(|error| HorizonError::InvalidState(error.to_string()))?;
        let mut response = ServiceResponse::success();
        response.raw_payload = 0u32.to_le_bytes().to_vec();
        // No fence was requested: retain its input words. This reply means
        // admission only; poll_channel must execute the actual descriptors.
        response.buffer_writes.push(IpcBufferWrite {
            buffer_index: decoded.output_index,
            data,
        });
        Ok(response)
    }

    pub fn pending_entries(&self, identity: &NvGpuChannelIdentity) -> Option<usize> {
        self.channels
            .iter()
            .find(|channel| channel.identity == *identity)
            .map(|channel| channel.queue.pending_entries())
    }

    pub fn new(maximum_channels: usize) -> HorizonResult<Self> {
        let points = NvGpuSyncpoints::new(maximum_channels)
            .map_err(|error| HorizonError::InvalidState(error.to_string()))?;
        Ok(Self {
            channels: Vec::new(),
            points,
            maximum_channels,
            next_object_id: 1,
            renderer: None,
        })
    }

    /// Install rendering before any channel is admitted, avoiding replacement
    /// of caches while descriptors or partially executed traces are outstanding.
    pub fn with_renderer(
        maximum_channels: usize,
        renderer: Box<dyn NativeDrawRenderer>,
    ) -> HorizonResult<Self> {
        let mut backend = Self::new(maximum_channels)?;
        backend.renderer = Some(renderer);
        Ok(backend)
    }

    fn allocate(
        &mut self,
        identity: NvGpuChannelIdentity,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let decoded = GpfifoAllocationRequest::decode(request, 65_536)
            .map_err(|error| HorizonError::IpcProtocol(error.to_string()))?;
        if decoded.allocation.flags != 1 || decoded.allocation.parameters != [0; 4] {
            return Err(HorizonError::InvalidState(
                "unsupported GPU allocation flags or parameters".into(),
            ));
        }
        if self
            .channels
            .iter()
            .any(|channel| channel.identity == identity)
        {
            return Err(HorizonError::InvalidState(
                "GPU channel already has a command queue".into(),
            ));
        }
        if self.channels.len() >= self.maximum_channels {
            return Err(HorizonError::InvalidState(
                "GPU channel capacity exhausted".into(),
            ));
        }
        let queue = GuestGpuChannel::new(decoded.allocation.entries as usize)
            .map_err(|error| HorizonError::InvalidState(error.to_string()))?;
        self.channels
            .try_reserve(1)
            .map_err(|_| HorizonError::InvalidState("GPU channel allocation failed".into()))?;
        // Prepare writeback before committing synchronization ownership. These
        // are initial allocated counters, never a claim of submitted work.
        let input = request
            .buffers
            .iter()
            .find(|buffer| !buffer.data.is_empty())
            .ok_or_else(|| HorizonError::IpcProtocol("missing GPU allocation input".into()))?;
        let mut data = input.data.clone();
        let fence = self
            .points
            .allocate(identity.clone())
            .map_err(|error| HorizonError::InvalidState(error.to_string()))?;
        data[12..16].copy_from_slice(&fence.id.to_le_bytes());
        data[16..20].copy_from_slice(&fence.value.to_le_bytes());
        self.channels.push(Channel {
            identity,
            queue,
            fence,
            graphics_object: None,
        });
        let mut response = ServiceResponse::success();
        response.raw_payload = 0u32.to_le_bytes().to_vec();
        response.buffer_writes.push(IpcBufferWrite {
            buffer_index: decoded.output_index,
            data,
        });
        Ok(response)
    }
}

impl NvGpuIoctlBackend for NativeGpuBackend {
    fn poll_channel(
        &mut self,
        identity: &NvGpuChannelIdentity,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        maximum_entries: usize,
    ) -> HorizonResult<usize> {
        if self.pending_entries(identity).is_none() {
            return Ok(0);
        }
        NativeGpuBackend::poll_channel(self, identity, memory, gpu, maximum_entries)
    }

    fn close_channel(&mut self, identity: NvGpuChannelIdentity) -> HorizonResult<()> {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.close_channel(&identity);
        }
        if let Some(index) = self
            .channels
            .iter()
            .position(|channel| channel.identity == identity)
        {
            self.points
                .release(&identity, self.channels[index].fence.id)
                .map_err(|error| HorizonError::InvalidState(error.to_string()))?;
            self.channels.remove(index);
        }
        Ok(())
    }

    fn dispatch(
        &mut self,
        identity: NvGpuChannelIdentity,
        _memory: &mut AddressSpace,
        _gpu: &NvGpuAddressSpace,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let ioctl = u32::from_le_bytes(
            request
                .raw_payload
                .get(4..8)
                .ok_or_else(|| HorizonError::IpcProtocol("truncated GPU IOCTL".into()))?
                .try_into()
                .unwrap(),
        );
        if request.command_id == 1 && ioctl == 0xc020481a {
            return self.allocate(identity, request);
        }
        if request.command_id == 1 && ioctl == 0xc0104809 {
            return self.allocate_graphics_object(identity, request);
        }
        if (request.command_id == 1 && ioctl & 0xc000ffff == 0xc0004808)
            || (request.command_id == 11 && ioctl == 0xc018481b)
        {
            return self.submit(identity, request);
        }
        // Unknown operations do not enter the queue or produce a fence reply.
        Err(HorizonError::UnknownCommand {
            service: "/dev/nvhost-gpu".into(),
            command_id: ioctl,
        })
    }
}
