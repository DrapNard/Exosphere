// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-client GPU-driver bootstrap and typed device descriptors.
//! Device IOCTL execution is admitted explicitly, not replaced by generic success.

use std::collections::BTreeMap;

pub mod address_space;
pub mod allocation;
pub mod backend;
pub mod object_context;
pub mod submission;
pub mod syncpoint;

use crate::{
    HorizonProfileId,
    error::{HorizonError, HorizonResult},
    handle::KernelObject,
    ipc::{HorizonService, ServiceRequest, ServiceResponse},
};

pub struct NvService;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvDeviceKind {
    Map,
    HostControl,
    GpuControl,
    GpuAddressSpace,
    GpuChannel,
}

impl NvDeviceKind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Map => "/dev/nvmap",
            Self::HostControl => "/dev/nvhost-ctrl",
            Self::GpuControl => "/dev/nvhost-ctrl-gpu",
            Self::GpuAddressSpace => "/dev/nvhost-as-gpu",
            Self::GpuChannel => "/dev/nvhost-gpu",
        }
    }

    fn from_path(path: &str) -> HorizonResult<Self> {
        [
            Self::Map,
            Self::HostControl,
            Self::GpuControl,
            Self::GpuAddressSpace,
            Self::GpuChannel,
        ]
        .into_iter()
        .find(|kind| kind.path() == path)
        .ok_or_else(|| HorizonError::UnknownService(format!("nvdrv device '{path}'")))
    }
}

fn nv_reply(words: &[u32]) -> ServiceResponse {
    let mut reply = ServiceResponse::success();
    reply.raw_payload = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    reply
}

/// Original-Switch GM20B identity, not host Vulkan device capabilities.
/// Layout: libnx nx/include/switch/nvidia/ioctl.h, nvioctl_gpu_characteristics.
/// Reporting guest hardware identity does not admit unsupported GPU execution.
const GM20B_CHARACTERISTICS: [u32; 40] = [
    0x120, 0x0b, 0xa1, 1, // Architecture, implementation, revision, GPCs.
    0x40000, 0, 0, 0, // L2 bytes and dedicated VRAM bytes (u64 each).
    2, 0x20, 0x20000, 0x20000, 0x1b, 0x30000, 1, 0x503, 0x503, 0x80, 0x28,
    0, // SM/SPA, warps, VA bits, reserved.
    0x55, 0, // Guest hardware flags (u64).
    0x902d, 0xb197, 0xb1c0, 0xb06f, 0xa140, 0xb0b5, // Engine classes.
    1, 0, 2, 1, 0, 1, 0x21d70, 0, // FBP/LTC/LTS/GPC topology and fuse masks.
    0x30326d67, 0x62, 0, 0, // "gm20b" and compression-store base (u64 each).
];

fn ioctl_inout_buffer(
    request: &ServiceRequest,
    wire_size: usize,
) -> HorizonResult<(&crate::ipc::IpcBuffer, usize)> {
    use crate::ipc::IpcBufferDirection;
    // AutoSelect retains unused zero-sized pointer/map-alias descriptors.
    // Keep original indices: writeback must target the active descriptor.
    let active: Vec<_> = request
        .buffers
        .iter()
        .enumerate()
        .filter(|(_, buffer)| buffer.capacity != 0 || !buffer.data.is_empty())
        .collect();
    let (input, output_index) = match active.as_slice() {
        [(index, buffer)] if buffer.direction == IpcBufferDirection::InOut => (*buffer, *index),
        [(_, input), (output_index, output)]
            if input.direction == IpcBufferDirection::In
                && output.direction == IpcBufferDirection::Out =>
        {
            (*input, *output_index)
        }
        _ => {
            return Err(HorizonError::IpcProtocol(format!(
                "invalid GPU query buffers: {:?}",
                active
                    .iter()
                    .map(|(_, buffer)| (buffer.direction, buffer.capacity))
                    .collect::<Vec<_>>()
            )));
        }
    };
    if input.data.len() != wire_size
        || input.capacity != wire_size as u64
        || request.buffers[output_index].capacity < wire_size as u64
    {
        return Err(HorizonError::IpcProtocol(
            "truncated GPU query structure".into(),
        ));
    }
    Ok((input, output_index))
}

fn gpu_tpc_masks(request: &ServiceRequest) -> HorizonResult<ServiceResponse> {
    let (input, output_index) = ioctl_inout_buffer(request, 24)?;
    let size = u32::from_le_bytes(input.data[..4].try_into().unwrap());
    if size > 8 {
        return Ok(nv_reply(&[4]));
    }
    // One enabled GPC with two TPCs, consistent with the declared GM20B profile.
    // The second u32 belongs to absent GPC 1 and therefore remains zero.
    let mask = ((1u64 << GM20B_CHARACTERISTICS[8]) - 1).to_le_bytes();
    let mut data = input.data.clone();
    data[16..16 + size as usize].copy_from_slice(&mask[..size as usize]);
    let mut reply = nv_reply(&[0]);
    reply.buffer_writes.push(crate::ipc::IpcBufferWrite {
        buffer_index: output_index,
        data,
    });
    Ok(reply)
}

/// GM20B region/alignment limits in libnx's nvioctl_zcull_info field order.
/// This describes the guest interface; actual ZCULL context binding is separate.
const GM20B_ZCULL_INFO: [u32; 10] = [32, 32, 1024, 2048, 32, 32, 192, 32, 64, 16];

fn ioctl_output_words(request: &ServiceRequest, words: &[u32]) -> HorizonResult<ServiceResponse> {
    use crate::ipc::{IpcBufferDirection, IpcBufferWrite};
    let mut active = request
        .buffers
        .iter()
        .enumerate()
        .filter(|(_, buffer)| buffer.capacity != 0 || !buffer.data.is_empty());
    let Some((index, output)) = active.next() else {
        return Err(HorizonError::IpcProtocol(
            "GPU query requires an output buffer".into(),
        ));
    };
    if active.next().is_some()
        || output.direction != IpcBufferDirection::Out
        || !output.data.is_empty()
        || output.capacity < (words.len() * 4) as u64
    {
        return Err(HorizonError::IpcProtocol(
            "invalid GPU output-only query buffers".into(),
        ));
    }
    let mut reply = nv_reply(&[0]);
    reply.buffer_writes.push(IpcBufferWrite {
        buffer_index: index,
        data: words.iter().flat_map(|word| word.to_le_bytes()).collect(),
    });
    Ok(reply)
}

fn gpu_characteristics(request: &ServiceRequest) -> HorizonResult<ServiceResponse> {
    use crate::ipc::IpcBufferWrite;
    const WIRE_SIZE: usize = 16 + 160;
    let (input, output_index) = ioctl_inout_buffer(request, WIRE_SIZE)?;
    let size = u64::from_le_bytes(input.data[..8].try_into().unwrap());
    let address = u64::from_le_bytes(input.data[8..16].try_into().unwrap());
    if size < 160 || address == 0 {
        return Ok(nv_reply(&[4]));
    }
    // The address is driver ABI metadata, not a second guest-memory destination.
    // All output goes through the validated IPC buffer, including its size header.
    let mut data = Vec::with_capacity(WIRE_SIZE);
    data.extend_from_slice(&160u64.to_le_bytes());
    data.extend_from_slice(&address.to_le_bytes());
    data.extend(
        GM20B_CHARACTERISTICS
            .iter()
            .flat_map(|word| word.to_le_bytes()),
    );
    let mut reply = nv_reply(&[0]);
    reply.buffer_writes.push(IpcBufferWrite {
        buffer_index: output_index,
        data,
    });
    Ok(reply)
}

fn requested_device(request: &ServiceRequest) -> HorizonResult<NvDeviceKind> {
    let [buffer] = request.buffers.as_slice() else {
        return Err(HorizonError::IpcProtocol(
            "nvdrv Open requires one path buffer".into(),
        ));
    };
    if buffer.direction != crate::ipc::IpcBufferDirection::In
        || buffer.capacity != buffer.data.len() as u64
        || buffer.data.is_empty()
        || buffer.data.len() > 256
    {
        return Err(HorizonError::IpcProtocol(
            "invalid nvdrv path buffer".into(),
        ));
    }
    // libnx sends strlen bytes; SDK callers may include the terminator and zero
    // padding. Never read beyond the descriptor or hide nonzero trailing data.
    let end = buffer
        .data
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(buffer.data.len());
    if buffer.data[end..].iter().any(|&byte| byte != 0) {
        return Err(HorizonError::IpcProtocol(
            "nvdrv path has trailing data".into(),
        ));
    }
    let path = std::str::from_utf8(&buffer.data[..end])
        .map_err(|_| HorizonError::IpcProtocol("invalid nvdrv path encoding".into()))?;
    if !path.starts_with("/dev/") || !path.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(HorizonError::IpcProtocol(
            "invalid nvdrv device path".into(),
        ));
    }
    NvDeviceKind::from_path(path)
}
impl HorizonService for NvService {
    fn name(&self) -> &'static str {
        "nvdrv"
    }
    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        Err(HorizonError::UnknownCommand {
            service: "nvdrv".into(),
            command_id: request.command_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_request(path: &[u8]) -> ServiceRequest {
        ServiceRequest {
            service: "nvdrv".into(),
            command_id: 0,
            values: Vec::new(),
            raw_payload: Vec::new(),
            buffers: vec![crate::ipc::IpcBuffer {
                address: 0x1000,
                capacity: path.len() as u64,
                direction: crate::ipc::IpcBufferDirection::In,
                data: path.to_vec(),
            }],
        }
    }

    fn initialized_client() -> NvClient {
        let mut client = NvClient::default();
        let mut request = open_request(b"/dev/nvmap");
        request.command_id = 3;
        client
            .dispatch(
                &request,
                Some(KernelObject::TransferMemory {
                    owner: 1,
                    base: 0x1000,
                    size: 0x1000,
                }),
                42,
            )
            .unwrap();
        client
    }

    #[test]
    fn channel_nvmap_binding_retains_backing_until_last_reference_closes() {
        use crate::memory::{AddressSpace, MemoryKind, MemoryPermission, PAGE_SIZE};
        let mut client = initialized_client();
        let map = client
            .dispatch(&open_request(b"/dev/nvmap"), None, 42)
            .unwrap();
        let map = u32::from_le_bytes(map.raw_payload[..4].try_into().unwrap());
        let mut channels = Vec::new();
        for _ in 0..2 {
            let reply = client
                .dispatch(&open_request(b"/dev/nvhost-gpu"), None, 42)
                .unwrap();
            channels.push(u32::from_le_bytes(
                reply.raw_payload[..4].try_into().unwrap(),
            ));
        }
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        client.map_objects.insert(
            1,
            NvMapObject {
                requested_size: PAGE_SIZE as u32,
                size: PAGE_SIZE,
                owner_descriptor: map,
                backing: Some(NvMapBacking {
                    address: 0x8000,
                    alignment: 4096,
                    flags: 1,
                    kind: 0,
                    heap_mask: 0,
                    lease: memory
                        .pin_device_memory(0x8000, PAGE_SIZE, MemoryPermission::READ_WRITE)
                        .unwrap(),
                }),
            },
        );
        let mut request = open_request(b"/dev/nvmap");
        request.command_id = 1;
        request.raw_payload = [channels[0].to_le_bytes(), 0x40044801u32.to_le_bytes()].concat();
        request.buffers[0].capacity = 4;
        request.buffers[0].data = channels[1].to_le_bytes().to_vec();
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().raw_payload,
            4u32.to_le_bytes()
        );
        assert!(client.channel_map_descriptor(channels[0]).is_none());
        request.buffers[0].data = map.to_le_bytes().to_vec();
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
        assert!(client.dispatch(&request, None, 42).is_err());
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::In;
        assert!(client.dispatch(&request, None, 43).is_err());
        for &channel in &channels {
            request.raw_payload[..4].copy_from_slice(&channel.to_le_bytes());
            assert_eq!(
                client.dispatch(&request, None, 42).unwrap().raw_payload,
                0u32.to_le_bytes()
            );
            assert_eq!(client.channel_map_descriptor(channel), Some(map));
        }
        assert!(client.dispatch(&request, None, 42).is_err());
        request.command_id = 2;
        request.buffers.clear();
        request.raw_payload = map.to_le_bytes().to_vec();
        client.dispatch(&request, None, 42).unwrap();
        assert!(client.map_object(1).is_some());
        assert!(memory.unmap(0x8000, PAGE_SIZE).is_err());
        request.raw_payload = channels[0].to_le_bytes().to_vec();
        client.dispatch(&request, None, 42).unwrap();
        assert!(client.map_object(1).is_some());
        request.raw_payload = channels[1].to_le_bytes().to_vec();
        client.dispatch(&request, None, 42).unwrap();
        assert!(client.map_object(1).is_none());
        assert!(client.channel_map_descriptor(channels[1]).is_none());
        memory.unmap(0x8000, PAGE_SIZE).unwrap();
    }

    #[test]
    fn channel_query_events_are_owned_shared_unsignaled_and_released_on_close() {
        let mut client = initialized_client();
        let opened = client
            .dispatch(&open_request(b"/dev/nvhost-gpu"), None, 42)
            .unwrap();
        let descriptor = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
        let mut request = open_request(b"/dev/nvhost-gpu");
        request.command_id = 4;
        request.buffers.clear();
        let mut events = Vec::new();
        for id in 1u32..=3 {
            request.raw_payload = [descriptor.to_le_bytes(), id.to_le_bytes()].concat();
            let reply = client.dispatch(&request, None, 42).unwrap();
            assert_eq!(reply.raw_payload, 0u32.to_le_bytes());
            let [crate::ipc::ReturnedObject::ReadableEvent(event)] = reply.objects.as_slice()
            else {
                panic!("QueryEvent must return one readable event");
            };
            assert!(!event.is_signaled());
            assert!(!events.contains(event));
            events.push(event.clone());
            assert_eq!(
                client.dispatch(&request, None, 42).unwrap().objects,
                reply.objects
            );
        }
        assert!(client.dispatch(&request, None, 43).is_err());
        assert_eq!(
            initialized_client()
                .dispatch(&request, None, 42)
                .unwrap()
                .raw_payload,
            4u32.to_le_bytes()
        );
        for id in [0u32, 4, u32::MAX] {
            request.raw_payload = [descriptor.to_le_bytes(), id.to_le_bytes()].concat();
            let reply = client.dispatch(&request, None, 42).unwrap();
            assert_eq!(reply.raw_payload, 4u32.to_le_bytes());
            assert!(reply.objects.is_empty());
        }
        request.raw_payload.truncate(7);
        assert!(client.dispatch(&request, None, 42).is_err());
        request.raw_payload = [descriptor.to_le_bytes(), 3u32.to_le_bytes()].concat();
        request.buffers = open_request(b"x").buffers;
        assert!(client.dispatch(&request, None, 42).is_err());
        request.buffers.clear();
        request.command_id = 2;
        request.raw_payload = descriptor.to_le_bytes().to_vec();
        client.dispatch(&request, None, 42).unwrap();
        assert!(client.channel_events.is_empty());
        // Copied handles outlive the descriptor, without becoming signaled.
        assert!(events.iter().all(|event| !event.is_signaled()));
        request.command_id = 4;
        request.raw_payload = [descriptor.to_le_bytes(), 3u32.to_le_bytes()].concat();
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().raw_payload,
            4u32.to_le_bytes()
        );
    }

    #[test]
    fn typed_device_descriptors_are_client_owned_nonrecycled_and_route_ioctls() {
        let mut client = initialized_client();
        let mut other = initialized_client();
        let mut ids = Vec::new();
        for kind in [
            NvDeviceKind::Map,
            NvDeviceKind::HostControl,
            NvDeviceKind::GpuControl,
            NvDeviceKind::GpuAddressSpace,
            NvDeviceKind::GpuChannel,
        ] {
            let reply = client
                .dispatch(&open_request(kind.path().as_bytes()), None, 42)
                .unwrap();
            assert_eq!(reply.raw_payload.len(), 8);
            assert_eq!(&reply.raw_payload[4..8], &0u32.to_le_bytes());
            assert!(reply.objects.is_empty());
            let id = u32::from_le_bytes(reply.raw_payload[..4].try_into().unwrap());
            assert!(!ids.contains(&id));
            assert_eq!(client.device(id), Some(kind));
            assert_eq!(other.device(id), None);
            ids.push(id);
        }
        let mut request = open_request(b"/dev/nvmap");
        request.buffers.clear();
        request.raw_payload = [ids[2].to_le_bytes(), 0x80044701u32.to_le_bytes()].concat();
        request.command_id = 1;
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap_err(),
            HorizonError::UnknownCommand {
                service: "/dev/nvhost-ctrl-gpu".into(),
                command_id: 0x80044701
            }
        );
        assert_eq!(
            other.dispatch(&request, None, 42).unwrap().raw_payload,
            4u32.to_le_bytes()
        );
        request.command_id = 2;
        assert!(client.dispatch(&request, None, 43).is_err());
        assert_eq!(client.device(ids[2]), Some(NvDeviceKind::GpuControl));
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().raw_payload,
            0u32.to_le_bytes()
        );
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().raw_payload,
            4u32.to_le_bytes()
        );
        let reply = client
            .dispatch(&open_request(b"/dev/nvmap\0\0"), None, 42)
            .unwrap();
        let id = u32::from_le_bytes(reply.raw_payload[..4].try_into().unwrap());
        assert!(id > *ids.last().unwrap());
        let mut first = initialized_client();
        let mut second = initialized_client();
        let request = open_request(b"/dev/nvhost-gpu");
        let first_fd = first.dispatch(&request, None, 42).unwrap();
        let second_fd = second.dispatch(&request, None, 42).unwrap();
        let first_fd = u32::from_le_bytes(first_fd.raw_payload[..4].try_into().unwrap());
        let second_fd = u32::from_le_bytes(second_fd.raw_payload[..4].try_into().unwrap());
        assert_eq!(first_fd, second_fd);
        assert_ne!(
            first.channel_identity(first_fd),
            second.channel_identity(second_fd)
        );
        assert_eq!(
            first.channel_identity(first_fd),
            first.clone().channel_identity(first_fd)
        );
    }

    #[test]
    fn nvmap_create_is_owned_bounded_and_released_with_its_device() {
        let mut client = initialized_client();
        let opened = client
            .dispatch(&open_request(b"/dev/nvmap"), None, 42)
            .unwrap();
        let descriptor = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
        let mut request = open_request(b"/dev/nvmap");
        request.command_id = 1;
        request.raw_payload = [descriptor.to_le_bytes(), 0xc0080101u32.to_le_bytes()].concat();
        request.buffers[0].capacity = 8;
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
        request.buffers[0].data = [4097u32.to_le_bytes(), 0u32.to_le_bytes()].concat();
        let reply = client.dispatch(&request, None, 42).unwrap();
        let handle = u32::from_le_bytes(reply.buffer_writes[0].data[4..8].try_into().unwrap());
        assert_eq!(client.map_object(handle).unwrap().size, 8192);
        assert_eq!(client.map_object(handle).unwrap().requested_size, 4097);
        assert!(initialized_client().map_object(handle).is_none());
        assert!(client.dispatch(&request, None, 43).is_err());
        request.buffers[0].capacity = 7;
        assert!(client.dispatch(&request, None, 42).is_err());
        assert_eq!(client.next_map_handle, handle + 1);
        request.buffers[0].capacity = 8;
        request.buffers[0].data[..4].fill(0);
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().raw_payload,
            4u32.to_le_bytes()
        );
        assert_eq!(client.map_objects.len(), 1);
        request.buffers[0].data[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().raw_payload,
            4u32.to_le_bytes()
        );
        request.buffers[0].data[..4].copy_from_slice(&0xfffff000u32.to_le_bytes());
        let reply = client.dispatch(&request, None, 42).unwrap();
        let second = u32::from_le_bytes(reply.buffer_writes[0].data[4..8].try_into().unwrap());
        assert_ne!(handle, second);
        assert_eq!(client.map_object(second).unwrap().size, 0xfffff000);
        client.next_map_handle = u32::MAX;
        assert!(
            client
                .dispatch(&request, None, 42)
                .unwrap()
                .buffer_writes
                .is_empty()
        );
        assert_eq!(client.map_objects.len(), 2);
        request.command_id = 2;
        request.buffers.clear();
        client.dispatch(&request, None, 42).unwrap();
        assert!(client.map_objects.is_empty());
    }

    #[test]
    fn gpu_channels_share_address_space_and_retain_mappings_after_fd_close() {
        use crate::memory::{AddressSpace, MemoryKind, MemoryPermission, PAGE_SIZE};
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let mut client = initialized_client();
        let opened = client
            .dispatch(&open_request(b"/dev/nvhost-as-gpu"), None, 42)
            .unwrap();
        let descriptor = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
        let mut channels = Vec::new();
        for _ in 0..2 {
            let opened = client
                .dispatch(&open_request(b"/dev/nvhost-gpu"), None, 42)
                .unwrap();
            channels.push(u32::from_le_bytes(
                opened.raw_payload[..4].try_into().unwrap(),
            ));
        }
        let mut request = open_request(b"/dev/nvmap");
        request.command_id = 1;
        request.raw_payload = [descriptor.to_le_bytes(), 0x40044101u32.to_le_bytes()].concat();
        request.buffers[0].capacity = 4;
        request.buffers[0].data = channels[0].to_le_bytes().to_vec();
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().raw_payload,
            4u32.to_le_bytes()
        );
        assert!(client.channel_address_space(channels[0]).is_none());
        client
            .gpu_address_spaces
            .get_mut(&descriptor)
            .unwrap()
            .initialize(1, 0x20000)
            .unwrap();
        for &channel in &channels {
            request.buffers[0].data = channel.to_le_bytes().to_vec();
            assert!(client.dispatch(&request, None, 43).is_err());
            assert_eq!(
                client.dispatch(&request, None, 42).unwrap().raw_payload,
                0u32.to_le_bytes()
            );
            assert_eq!(
                client.dispatch(&request, None, 42).unwrap().raw_payload,
                4u32.to_le_bytes()
            );
        }
        request.buffers[0].capacity = 3;
        assert!(client.dispatch(&request, None, 42).is_err());
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
        let space = client.gpu_address_spaces.get_mut(&descriptor).unwrap();
        space.reserve_fixed(0x100000, 1, 4096).unwrap();
        space
            .map_reserved(0x100000, PAGE_SIZE, 0, 4096, &backing)
            .unwrap();
        drop(backing);
        client
            .channel_address_space(channels[0])
            .unwrap()
            .write(&mut memory, 0x100000, &[93])
            .unwrap();
        let mut byte = [0];
        client
            .channel_address_space(channels[1])
            .unwrap()
            .read(&memory, 0x100000, &mut byte)
            .unwrap();
        assert_eq!(byte, [93]);
        request.command_id = 2;
        request.raw_payload = descriptor.to_le_bytes().to_vec();
        request.buffers.clear();
        client.dispatch(&request, None, 42).unwrap();
        assert_eq!(client.device(descriptor), None);
        assert!(memory.unmap(0x4000, PAGE_SIZE).is_err());
        client
            .channel_address_space(channels[1])
            .unwrap()
            .read(&memory, 0x100000, &mut byte)
            .unwrap();
        assert_eq!(byte, [93]);
        request.raw_payload = channels[0].to_le_bytes().to_vec();
        client.dispatch(&request, None, 42).unwrap();
        assert!(client.channel_address_space(channels[0]).is_none());
        assert!(memory.unmap(0x4000, PAGE_SIZE).is_err());
        request.raw_payload = channels[1].to_le_bytes().to_vec();
        client.dispatch(&request, None, 42).unwrap();
        assert!(client.gpu_address_space(descriptor).is_none());
        memory.unmap(0x4000, PAGE_SIZE).unwrap();
    }

    #[test]
    fn fixed_gpu_mapping_keeps_freed_nvmap_backing_until_driver_unmap() {
        use crate::memory::{AddressSpace, MemoryKind, MemoryPermission, PAGE_SIZE};
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let mut client = initialized_client();
        let mut request = open_request(b"/dev/nvmap");
        let map_fd = u32::from_le_bytes(
            client.dispatch(&request, None, 42).unwrap().raw_payload[..4]
                .try_into()
                .unwrap(),
        );
        request.command_id = 1;
        request.raw_payload = [map_fd.to_le_bytes(), 0xc0080101u32.to_le_bytes()].concat();
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
        request.buffers[0].capacity = 8;
        request.buffers[0].data = [8192u32.to_le_bytes(), 0u32.to_le_bytes()].concat();
        let created = client.dispatch(&request, None, 42).unwrap();
        let handle = u32::from_le_bytes(created.buffer_writes[0].data[4..8].try_into().unwrap());
        request.raw_payload[4..8].copy_from_slice(&0xc0200104u32.to_le_bytes());
        request.buffers[0].capacity = 32;
        request.buffers[0].data = vec![0; 32];
        request.buffers[0].data[..4].copy_from_slice(&handle.to_le_bytes());
        request.buffers[0].data[8..12].copy_from_slice(&1u32.to_le_bytes());
        request.buffers[0].data[24..32].copy_from_slice(&0x4000u64.to_le_bytes());
        client
            .dispatch_with_memory(&request, None, 42, &mut memory)
            .unwrap();
        let opened = client
            .dispatch(&open_request(b"/dev/nvhost-as-gpu"), None, 42)
            .unwrap();
        let as_fd = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
        request.raw_payload = [as_fd.to_le_bytes(), 0x40284109u32.to_le_bytes()].concat();
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::In;
        request.buffers[0].capacity = 40;
        request.buffers[0].data = vec![0; 40];
        request.buffers[0].data[..4].copy_from_slice(&1u32.to_le_bytes());
        client.dispatch(&request, None, 42).unwrap();
        request.raw_payload[4..8].copy_from_slice(&0xc0184102u32.to_le_bytes());
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
        request.buffers[0].capacity = 24;
        request.buffers[0].data = [
            1u32.to_le_bytes().as_slice(),
            4096u32.to_le_bytes().as_slice(),
            1u32.to_le_bytes().as_slice(),
            0u32.to_le_bytes().as_slice(),
            0x100000u64.to_le_bytes().as_slice(),
        ]
        .concat();
        client.dispatch(&request, None, 42).unwrap();
        request.raw_payload[4..8].copy_from_slice(&0xc0284106u32.to_le_bytes());
        request.buffers[0].capacity = 40;
        request.buffers[0].data = vec![0; 40];
        request.buffers[0].data[..4].copy_from_slice(&1u32.to_le_bytes());
        request.buffers[0].data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        request.buffers[0].data[8..12].copy_from_slice(&handle.to_le_bytes());
        request.buffers[0].data[16..24].copy_from_slice(&4096u64.to_le_bytes());
        request.buffers[0].data[24..32].copy_from_slice(&8192u64.to_le_bytes());
        request.buffers[0].data[32..40].copy_from_slice(&0x100000u64.to_le_bytes());
        assert!(client.dispatch(&request, None, 42).is_err());
        request.buffers[0].data[24..32].fill(0); // Map the remainder of the object.
        let mapped = client.dispatch(&request, None, 42).unwrap();
        assert_eq!(
            &mapped.buffer_writes[0].data[12..16],
            &4096u32.to_le_bytes()
        );
        assert!(client.dispatch(&request, None, 42).is_err());
        memory.write(0x5000, &[88]).unwrap();
        let mut byte = [0];
        client
            .gpu_address_space(as_fd)
            .unwrap()
            .read(&memory, 0x100000, &mut byte)
            .unwrap();
        assert_eq!(byte, [88]);
        request.raw_payload = [map_fd.to_le_bytes(), 0xc0180105u32.to_le_bytes()].concat();
        request.buffers[0].capacity = 24;
        request.buffers[0].data = vec![0; 24];
        request.buffers[0].data[..4].copy_from_slice(&handle.to_le_bytes());
        let freed = client.dispatch(&request, None, 42).unwrap();
        assert_eq!(&freed.buffer_writes[0].data[8..16], &[0; 8]);
        assert!(memory.unmap(0x4000, 2 * PAGE_SIZE).is_err());
        client
            .gpu_address_space(as_fd)
            .unwrap()
            .read(&memory, 0x100000, &mut byte)
            .unwrap();
        assert_eq!(byte, [88]);
        request.raw_payload = [as_fd.to_le_bytes(), 0xc0084105u32.to_le_bytes()].concat();
        request.buffers[0].capacity = 8;
        request.buffers[0].data = 0x100000u64.to_le_bytes().to_vec();
        client.dispatch(&request, None, 42).unwrap();
        assert!(client.dispatch(&request, None, 42).is_err());
        memory.unmap(0x4000, 2 * PAGE_SIZE).unwrap();
    }

    #[test]
    fn automatic_va_reservation_writes_selected_address_only_after_validation() {
        use crate::ipc::{IpcBuffer, IpcBufferDirection};
        let mut client = initialized_client();
        let mut request = open_request(b"/dev/nvhost-as-gpu");
        let opened = client.dispatch(&request, None, 42).unwrap();
        let descriptor = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
        client
            .gpu_address_spaces
            .get_mut(&descriptor)
            .unwrap()
            .initialize_with_ranges(1, 0x10000, Some((0x100000, 0x8040000, 0x8000000)))
            .unwrap();
        request.command_id = 1;
        request.raw_payload = [descriptor.to_le_bytes(), 0xc0184102u32.to_le_bytes()].concat();
        let mut input = vec![0; 24];
        input[..4].copy_from_slice(&1u32.to_le_bytes());
        input[4..8].copy_from_slice(&4096u32.to_le_bytes());
        input[12..16].fill(0xcc);
        input[16..24].copy_from_slice(&4096u64.to_le_bytes());
        let inactive = IpcBuffer {
            address: 0,
            capacity: 0,
            direction: IpcBufferDirection::In,
            data: vec![],
        };
        request.buffers = vec![
            inactive.clone(),
            IpcBuffer {
                address: 0,
                capacity: 24,
                direction: IpcBufferDirection::In,
                data: input.clone(),
            },
            inactive,
            IpcBuffer {
                address: 0,
                capacity: 24,
                direction: IpcBufferDirection::Out,
                data: vec![],
            },
        ];
        assert!(client.dispatch(&request, None, 43).is_err());
        request.buffers[3].capacity = 23;
        assert!(client.dispatch(&request, None, 42).is_err());
        request.buffers[3].capacity = 24;
        for flags in [2u32, 3, 4, u32::MAX] {
            request.buffers[1].data[8..12].copy_from_slice(&flags.to_le_bytes());
            assert!(client.dispatch(&request, None, 42).is_err());
        }
        request.buffers[1].data = input.clone();
        for address in [0x100000u64, 0x101000] {
            let reply = client.dispatch(&request, None, 42).unwrap();
            assert_eq!(reply.raw_payload, 0u32.to_le_bytes());
            assert_eq!(reply.buffer_writes.len(), 1);
            assert_eq!(reply.buffer_writes[0].buffer_index, 3);
            assert_eq!(&reply.buffer_writes[0].data[..16], &input[..16]);
            assert_eq!(&reply.buffer_writes[0].data[16..24], &address.to_le_bytes());
            assert_eq!(request.buffers[1].data, input);
        }
        // The selected address is an actual reservation, not merely a reply.
        let space = client.gpu_address_spaces.get_mut(&descriptor).unwrap();
        assert!(space.reserve_fixed(0x100000, 1, 4096).is_err());
        space.free_space(0x100000, 1, 4096).unwrap();
        assert_eq!(space.reserve_auto(1, 4096, 4096).unwrap(), 0x100000);
    }

    #[test]
    fn custom_va_regions_query_matches_reservation_policy_and_preserves_transport() {
        use crate::ipc::{IpcBuffer, IpcBufferDirection};
        for page_size in [0x10000u32, 0x20000] {
            let mut client = initialized_client();
            let mut request = open_request(b"/dev/nvhost-as-gpu");
            let reply = client.dispatch(&request, None, 42).unwrap();
            let descriptor = u32::from_le_bytes(reply.raw_payload[..4].try_into().unwrap());
            request.command_id = 1;
            request.raw_payload = [descriptor.to_le_bytes(), 0x40284109u32.to_le_bytes()].concat();
            let mut initialize = vec![0; 40];
            initialize[..4].copy_from_slice(&1u32.to_le_bytes());
            initialize[8..12].copy_from_slice(&page_size.to_le_bytes());
            for (offset, value) in [(16, 0x100000u64), (24, 0x40000000), (32, 0x20000000)] {
                initialize[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            }
            request.buffers[0].capacity = 40;
            request.buffers[0].data = initialize;
            client.dispatch(&request, None, 42).unwrap();
            let space = client.gpu_address_spaces.get_mut(&descriptor).unwrap();
            space.reserve_fixed(0x100000, 1, 4096).unwrap();
            space.reserve_fixed(0x20000000, 1, page_size).unwrap();
            assert!(space.reserve_fixed(0x20000000, 1, 4096).is_err());
            assert!(space.reserve_fixed(0x100000, 1, page_size).is_err());
            assert!(space.reserve_fixed(0x40000000, 1, page_size).is_err());
            request.raw_payload[4..8].copy_from_slice(&0xc0404108u32.to_le_bytes());
            let mut query = vec![0xff; 64];
            query[8..12].fill(0); // Caller count does not replace required size.
            let inactive = IpcBuffer {
                address: 0,
                capacity: 0,
                direction: IpcBufferDirection::In,
                data: vec![],
            };
            request.buffers = vec![
                inactive.clone(),
                IpcBuffer {
                    address: 0,
                    capacity: 64,
                    direction: IpcBufferDirection::In,
                    data: query.clone(),
                },
                inactive,
                IpcBuffer {
                    address: 0,
                    capacity: 64,
                    direction: IpcBufferDirection::Out,
                    data: vec![],
                },
            ];
            let reply = client.dispatch(&request, None, 42).unwrap();
            assert_eq!(reply.raw_payload, 0u32.to_le_bytes());
            assert_eq!(reply.buffer_writes.len(), 1);
            assert_eq!(reply.buffer_writes[0].buffer_index, 3);
            let data = &reply.buffer_writes[0].data;
            assert_eq!(&data[..8], &query[..8]);
            assert_eq!(&data[8..12], &48u32.to_le_bytes());
            assert_eq!(&data[12..16], &query[12..16]);
            for (index, start, end, page) in [
                (0, 0x100000u64, 0x20000000u64, 4096u32),
                (1, 0x20000000, 0x40000000, page_size),
            ] {
                let offset = 16 + index * 24;
                assert_eq!(&data[offset..offset + 8], &start.to_le_bytes());
                assert_eq!(&data[offset + 8..offset + 12], &page.to_le_bytes());
                assert_eq!(&data[offset + 12..offset + 16], &[0; 4]);
                assert_eq!(
                    &data[offset + 16..offset + 24],
                    &((end - start) / u64::from(page)).to_le_bytes()
                );
            }
            assert!(client.dispatch(&request, None, 43).is_err());
            request.buffers[3].capacity = 63;
            assert!(client.dispatch(&request, None, 42).is_err());
        }
    }

    #[test]
    fn default_va_regions_are_not_fabricated_by_the_query() {
        let mut client = initialized_client();
        let mut request = open_request(b"/dev/nvhost-as-gpu");
        let opened = client.dispatch(&request, None, 42).unwrap();
        let descriptor = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
        request.command_id = 1;
        request.raw_payload = [descriptor.to_le_bytes(), 0x40284109u32.to_le_bytes()].concat();
        request.buffers[0].capacity = 40;
        request.buffers[0].data = vec![0; 40];
        client.dispatch(&request, None, 42).unwrap();
        request.raw_payload[4..8].copy_from_slice(&0xc0404108u32.to_le_bytes());
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
        request.buffers[0].capacity = 64;
        request.buffers[0].data = vec![0; 64];
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap_err(),
            HorizonError::UnknownCommand {
                service: NvDeviceKind::GpuAddressSpace.path().into(),
                command_id: 0xc0404108,
            }
        );
        assert_eq!(
            client.gpu_address_space(descriptor).unwrap().regions(),
            None
        );
    }

    #[test]
    fn gpu_address_initialization_retains_page_policy_and_rejects_before_mutation() {
        for requested_page_size in [0u32, 0x10000, 0x20000] {
            let mut client = initialized_client();
            let mut request = open_request(b"/dev/nvhost-as-gpu");
            let opened = client.dispatch(&request, None, 42).unwrap();
            let descriptor = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
            assert_eq!(
                client
                    .gpu_address_space(descriptor)
                    .unwrap()
                    .configuration(),
                None
            );
            request.command_id = 1;
            request.raw_payload = [descriptor.to_le_bytes(), 0x40284109u32.to_le_bytes()].concat();
            request.buffers[0].capacity = 40;
            request.buffers[0].data = vec![0; 40];
            request.buffers[0].data[..4].copy_from_slice(&1u32.to_le_bytes());
            request.buffers[0].data[8..12].copy_from_slice(&requested_page_size.to_le_bytes());
            request.buffers[0].data[16] = 1;
            assert!(client.dispatch(&request, None, 42).is_err());
            assert_eq!(
                client
                    .gpu_address_space(descriptor)
                    .unwrap()
                    .configuration(),
                None
            );
            request.buffers[0].data[16] = 0;
            request.buffers[0].data[8..12].copy_from_slice(&4096u32.to_le_bytes());
            assert!(client.dispatch(&request, None, 42).is_err());
            assert_eq!(
                client
                    .gpu_address_space(descriptor)
                    .unwrap()
                    .configuration(),
                None
            );
            request.buffers[0].data[8..12].copy_from_slice(&requested_page_size.to_le_bytes());
            assert!(client.dispatch(&request, None, 43).is_err());
            let reply = client.dispatch(&request, None, 42).unwrap();
            assert_eq!(reply.raw_payload, 0u32.to_le_bytes());
            assert!(reply.buffer_writes.is_empty());
            let policy = client
                .gpu_address_space(descriptor)
                .unwrap()
                .configuration()
                .unwrap();
            assert_eq!(policy.flags, 1);
            assert_eq!(
                policy.big_page_size,
                if requested_page_size == 0 {
                    0x20000
                } else {
                    requested_page_size
                }
            );
            assert!(client.dispatch(&request, None, 42).is_err());
            assert_eq!(
                client
                    .gpu_address_space(descriptor)
                    .unwrap()
                    .configuration(),
                Some(policy)
            );
            request.raw_payload[4..8].copy_from_slice(&0xc0184102u32.to_le_bytes());
            request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
            request.buffers[0].capacity = 24;
            request.buffers[0].data = [
                1u32.to_le_bytes().as_slice(),
                4096u32.to_le_bytes().as_slice(),
                1u32.to_le_bytes().as_slice(),
                0u32.to_le_bytes().as_slice(),
                0x100000u64.to_le_bytes().as_slice(),
            ]
            .concat();
            let reserved = client.dispatch(&request, None, 42).unwrap();
            assert_eq!(reserved.buffer_writes[0].data, request.buffers[0].data);
            assert!(client.dispatch(&request, None, 42).is_err());
            request.raw_payload[4..8].copy_from_slice(&0xc0104103u32.to_le_bytes());
            request.buffers[0].capacity = 16;
            request.buffers[0].data = [
                0x100000u64.to_le_bytes().as_slice(),
                1u32.to_le_bytes().as_slice(),
                4096u32.to_le_bytes().as_slice(),
            ]
            .concat();
            assert_eq!(
                client.dispatch(&request, None, 42).unwrap().raw_payload,
                0u32.to_le_bytes()
            );
            assert!(client.dispatch(&request, None, 42).is_err());
            request.command_id = 2;
            request.buffers.clear();
            client.dispatch(&request, None, 42).unwrap();
            assert!(client.gpu_address_space(descriptor).is_none());
        }
    }

    #[test]
    fn nvmap_alloc_retains_actual_guest_backing_and_free_respects_device_references() {
        use crate::memory::{AddressSpace, MemoryKind, MemoryPermission, PAGE_SIZE};
        for flags in [0u32, 3] {
            let mut memory = AddressSpace::default();
            memory
                .map(
                    0x4000,
                    PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
            let mut client = initialized_client();
            let mut request = open_request(b"/dev/nvmap");
            let opened = client.dispatch(&request, None, 42).unwrap();
            let descriptor = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
            request.command_id = 1;
            request.raw_payload = [descriptor.to_le_bytes(), 0xc0080101u32.to_le_bytes()].concat();
            request.buffers[0].capacity = 8;
            request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
            request.buffers[0].data = [16u32.to_le_bytes(), 0u32.to_le_bytes()].concat();
            let created = client.dispatch(&request, None, 42).unwrap();
            let handle =
                u32::from_le_bytes(created.buffer_writes[0].data[4..8].try_into().unwrap());
            request.raw_payload[4..8].copy_from_slice(&0xc0200104u32.to_le_bytes());
            request.buffers[0].capacity = 32;
            request.buffers[0].data = vec![0; 32];
            request.buffers[0].data[..4].copy_from_slice(&handle.to_le_bytes());
            request.buffers[0].data[8..12].copy_from_slice(&flags.to_le_bytes());
            request.buffers[0].data[24..32].copy_from_slice(&0x9000u64.to_le_bytes());
            assert!(
                client
                    .dispatch_with_memory(&request, None, 42, &mut memory)
                    .is_err()
            );
            assert!(client.map_object(handle).unwrap().backing.is_none());
            request.buffers[0].data[24..32].copy_from_slice(&0x4000u64.to_le_bytes());
            let allocated = client
                .dispatch_with_memory(&request, None, 42, &mut memory)
                .unwrap();
            assert_eq!(
                &allocated.buffer_writes[0].data[12..16],
                &4096u32.to_le_bytes()
            );
            assert_eq!(
                client
                    .dispatch_with_memory(&request, None, 42, &mut memory)
                    .unwrap()
                    .raw_payload,
                4u32.to_le_bytes()
            );
            let lease = client
                .map_object(handle)
                .unwrap()
                .backing
                .as_ref()
                .unwrap()
                .lease
                .clone();
            memory.write(0x4000, &[71]).unwrap();
            let mut byte = [0];
            memory.read_device_memory(&lease, 0, &mut byte).unwrap();
            assert_eq!(byte, [71]);
            assert_eq!(
                memory.write_device_memory(&lease, 0, &[72]).is_ok(),
                flags & 1 != 0
            );
            assert!(memory.unmap(0x4000, PAGE_SIZE).is_err());
            request.raw_payload[4..8].copy_from_slice(&0xc0180105u32.to_le_bytes());
            request.buffers[0].capacity = 24;
            request.buffers[0].data.truncate(24);
            let freed = client
                .dispatch_with_memory(&request, None, 42, &mut memory)
                .unwrap();
            assert_eq!(&freed.buffer_writes[0].data[8..16], &[0; 8]);
            assert_eq!(
                &freed.buffer_writes[0].data[20..24],
                &((flags >> 1) & 1).to_le_bytes()
            );
            assert!(client.map_object(handle).is_none());
            assert!(memory.unmap(0x4000, PAGE_SIZE).is_err());
            drop(lease);
            memory.unmap(0x4000, PAGE_SIZE).unwrap();
        }
    }

    #[test]
    fn nvmap_size_and_free_reject_foreign_stale_and_short_requests() {
        let mut client = initialized_client();
        let mut request = open_request(b"/dev/nvmap");
        let descriptor = u32::from_le_bytes(
            client.dispatch(&request, None, 42).unwrap().raw_payload[..4]
                .try_into()
                .unwrap(),
        );
        request.command_id = 1;
        request.raw_payload = [descriptor.to_le_bytes(), 0xc0080101u32.to_le_bytes()].concat();
        request.buffers[0].capacity = 8;
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
        request.buffers[0].data = [17u32.to_le_bytes(), 0u32.to_le_bytes()].concat();
        let created = client.dispatch(&request, None, 42).unwrap();
        let handle = u32::from_le_bytes(created.buffer_writes[0].data[4..8].try_into().unwrap());
        request.raw_payload[4..8].copy_from_slice(&0xc00c0109u32.to_le_bytes());
        request.buffers[0].capacity = 12;
        request.buffers[0].data =
            [handle.to_le_bytes(), 1u32.to_le_bytes(), 0u32.to_le_bytes()].concat();
        let reply = client.dispatch(&request, None, 42).unwrap();
        assert_eq!(&reply.buffer_writes[0].data[8..12], &4096u32.to_le_bytes());
        assert_eq!(
            client
                .map_size_or_free(descriptor + 1, &request, false)
                .unwrap()
                .raw_payload,
            4u32.to_le_bytes()
        );
        request.raw_payload[4..8].copy_from_slice(&0xc0180105u32.to_le_bytes());
        request.buffers[0].capacity = 23;
        request.buffers[0].data.resize(24, 0xaa);
        assert!(client.dispatch(&request, None, 42).is_err());
        assert!(client.map_object(handle).is_some());
        request.buffers[0].capacity = 24;
        let reply = client.dispatch(&request, None, 42).unwrap();
        assert_eq!(&reply.buffer_writes[0].data[8..16], &[0; 8]);
        assert_eq!(&reply.buffer_writes[0].data[16..20], &4096u32.to_le_bytes());
        assert_eq!(&reply.buffer_writes[0].data[20..24], &[0; 4]);
        assert!(client.map_object(handle).is_none());
        let stale = client.dispatch(&request, None, 42).unwrap();
        assert_eq!(stale.raw_payload, 4u32.to_le_bytes());
        assert!(stale.buffer_writes.is_empty());
    }

    #[test]
    fn zcull_info_is_output_only_and_preserves_autoselect_indices() {
        let mut request = open_request(b"/dev/nvmap");
        request.buffers[0].data.clear();
        request.buffers[0].capacity = 0;
        request.buffers.push(crate::ipc::IpcBuffer {
            address: 0x2000,
            capacity: 40,
            direction: crate::ipc::IpcBufferDirection::Out,
            data: Vec::new(),
        });
        let reply = ioctl_output_words(&request, &GM20B_ZCULL_INFO).unwrap();
        assert_eq!(reply.raw_payload, 0u32.to_le_bytes());
        assert_eq!(reply.buffer_writes[0].buffer_index, 1);
        assert_eq!(reply.buffer_writes[0].data.len(), 40);
        assert_eq!(&reply.buffer_writes[0].data[24..28], &192u32.to_le_bytes());
        request.buffers[1].capacity = 39;
        assert!(ioctl_output_words(&request, &GM20B_ZCULL_INFO).is_err());
        request.buffers[1].capacity = 40;
        request.buffers[0].capacity = 4;
        assert!(ioctl_output_words(&request, &GM20B_ZCULL_INFO).is_err());
        request.buffers[0].capacity = 0;
        request.buffers[1].direction = crate::ipc::IpcBufferDirection::InOut;
        assert!(ioctl_output_words(&request, &GM20B_ZCULL_INFO).is_err());
    }

    #[test]
    fn tpc_masks_match_guest_topology_and_bound_all_requested_sizes() {
        let mut request = open_request(b"/dev/nvmap");
        request.buffers[0].capacity = 24;
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
        request.buffers[0].data = vec![0xaa; 24];
        for size in 0..=8u32 {
            request.buffers[0].data[..4].copy_from_slice(&size.to_le_bytes());
            let reply = gpu_tpc_masks(&request).unwrap();
            assert_eq!(reply.raw_payload, 0u32.to_le_bytes());
            let output = &reply.buffer_writes[0].data;
            assert_eq!(&output[..16], &request.buffers[0].data[..16]);
            assert_eq!(
                &output[16..16 + size as usize],
                &3u64.to_le_bytes()[..size as usize]
            );
            assert_eq!(
                &output[16 + size as usize..],
                &request.buffers[0].data[16 + size as usize..]
            );
        }
        request.buffers[0].data[..4].copy_from_slice(&9u32.to_le_bytes());
        let reply = gpu_tpc_masks(&request).unwrap();
        assert_eq!(reply.raw_payload, 4u32.to_le_bytes());
        assert!(reply.buffer_writes.is_empty());
        request.buffers[0].data.pop();
        assert!(gpu_tpc_masks(&request).is_err());
    }

    #[test]
    fn gpu_identity_uses_bounded_ipc_output_and_preserves_address_metadata() {
        let mut client = initialized_client();
        let opened = client
            .dispatch(&open_request(b"/dev/nvhost-ctrl-gpu"), None, 42)
            .unwrap();
        let mut request = open_request(b"/dev/nvmap");
        request.command_id = 1;
        request.raw_payload = [
            opened.raw_payload[..4].to_vec(),
            0xc0b04705u32.to_le_bytes().to_vec(),
        ]
        .concat();
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::InOut;
        request.buffers[0].capacity = 176;
        request.buffers[0].data = vec![0; 176];
        request.buffers[0].data[..8].copy_from_slice(&160u64.to_le_bytes());
        request.buffers[0].data[8..16].copy_from_slice(&1u64.to_le_bytes());
        let reply = client.dispatch(&request, None, 42).unwrap();
        assert_eq!(reply.raw_payload, 0u32.to_le_bytes());
        assert_eq!(reply.buffer_writes.len(), 1);
        let output = &reply.buffer_writes[0];
        assert_eq!(output.buffer_index, 0);
        assert_eq!(output.data.len(), 176);
        assert_eq!(&output.data[..16], &request.buffers[0].data[..16]);
        assert_eq!(&output.data[16..20], &0x120u32.to_le_bytes());
        assert_eq!(&output.data[160..168], b"gm20b\0\0\0");
        request.buffers.insert(
            0,
            crate::ipc::IpcBuffer {
                address: 0,
                capacity: 0,
                direction: crate::ipc::IpcBufferDirection::In,
                data: Vec::new(),
            },
        );
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().buffer_writes[0].buffer_index,
            1
        );
        request.buffers.remove(0);
        let mut out_buffer = request.buffers[0].clone();
        out_buffer.direction = crate::ipc::IpcBufferDirection::Out;
        out_buffer.data.clear();
        request.buffers[0].direction = crate::ipc::IpcBufferDirection::In;
        request.buffers.push(out_buffer);
        assert_eq!(
            client.dispatch(&request, None, 42).unwrap().buffer_writes[0].buffer_index,
            1
        );
        request.buffers[1].capacity = 175;
        assert!(client.dispatch(&request, None, 42).is_err());
        request.buffers[1].capacity = 176;
        request.buffers[0].data[8..16].fill(0);
        let invalid = client.dispatch(&request, None, 42).unwrap();
        assert_eq!(invalid.raw_payload, 4u32.to_le_bytes());
        assert!(invalid.buffer_writes.is_empty());
        request.buffers[0].data.pop();
        assert!(client.dispatch(&request, None, 42).is_err());
    }

    #[test]
    fn rejected_device_paths_preserve_registry_and_descriptor_cursor() {
        let mut client = initialized_client();
        for path in [
            b"".as_slice(),
            b"/dev/nvmap\0hidden",
            b"/dev/nvmap\n",
            b"/dev/unknown",
            b"/dev/\xff",
        ] {
            assert!(client.dispatch(&open_request(path), None, 42).is_err());
            assert_eq!(client.next_descriptor, 1);
            assert!(client.devices.is_empty());
        }
        let mut request = open_request(b"/dev/nvmap");
        assert!(NvClient::default().dispatch(&request, None, 42).is_err());
        assert!(client.dispatch(&request, None, 43).is_err());
        request.buffers[0].capacity += 1;
        assert!(client.dispatch(&request, None, 42).is_err());
        assert!(client.devices.is_empty());
        request.buffers[0].capacity -= 1;
        client.next_descriptor = u32::MAX;
        assert!(client.dispatch(&request, None, 42).is_err());
        assert!(client.devices.is_empty());
    }
    #[test]
    fn resource_identity_is_authenticated_and_unsupported_devices_still_stop() {
        let mut client = NvClient::default();
        let mut request = ServiceRequest {
            service: "nvdrv".into(),
            command_id: 8,
            values: vec![],
            raw_payload: 42u64.to_le_bytes().to_vec(),
            buffers: vec![],
        };
        assert!(client.dispatch(&request, None, 42).is_err());
        request.command_id = 3;
        let transfer = KernelObject::TransferMemory {
            owner: 1,
            base: 0x1000,
            size: 0x1000,
        };
        client
            .dispatch(&request, Some(transfer.clone()), 42)
            .unwrap();
        request.command_id = 8;
        assert!(client.dispatch(&request, None, 43).is_err());
        assert_eq!(client.resource_user_id, None);
        request.raw_payload = 43u64.to_le_bytes().to_vec();
        assert!(client.dispatch(&request, None, 43).is_err());
        assert_eq!(client.resource_user_id, None);
        request.raw_payload = 42u64.to_le_bytes().to_vec();
        client.dispatch(&request, None, 42).unwrap();
        assert_eq!(client.resource_user_id, Some(42));
        request.command_id = 0;
        assert!(client.dispatch(&request, None, 42).is_err());
        assert_eq!(client.transfer, Some(transfer));
    }
}

#[derive(Debug, Clone)]
pub struct NvMapObject {
    pub requested_size: u32,
    pub size: u64,
    owner_descriptor: u32,
    pub backing: Option<NvMapBacking>,
}

#[derive(Debug, Clone)]
pub struct NvMapBacking {
    pub address: u64,
    pub alignment: u32,
    pub flags: u32,
    pub kind: u8,
    pub heap_mask: u32,
    pub lease: crate::memory::DeviceMemoryLease,
}

#[derive(Debug, Clone)]
pub struct NvGpuChannelIdentity {
    client: std::sync::Arc<()>,
    descriptor: u32,
}

impl PartialEq for NvGpuChannelIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.descriptor == other.descriptor && std::sync::Arc::ptr_eq(&self.client, &other.client)
    }
}
impl Eq for NvGpuChannelIdentity {}

impl NvGpuChannelIdentity {
    pub(crate) fn descriptor(&self) -> u32 {
        self.descriptor
    }
}

#[derive(Debug, Clone)]
pub struct NvClient {
    identity: std::sync::Arc<()>,
    /// Kernel-owned reference: closing the caller's transfer handle does not unlock it.
    pub transfer: Option<KernelObject>,
    pub resource_user_id: Option<u64>,
    authenticated_owner: Option<u64>,
    devices: BTreeMap<u32, NvDeviceKind>,
    next_descriptor: u32,
    map_objects: BTreeMap<u32, NvMapObject>,
    next_map_handle: u32,
    gpu_address_spaces: BTreeMap<u32, address_space::NvGpuAddressSpace>,
    channel_address_spaces: BTreeMap<u32, u32>,
    channel_map_descriptors: BTreeMap<u32, u32>,
    /// Channel debug/error notifications, shared by repeated QueryEvent handles.
    channel_events: BTreeMap<(u32, u32), crate::event::SharedEvent>,
}

impl Default for NvClient {
    fn default() -> Self {
        Self {
            identity: std::sync::Arc::new(()),
            transfer: None,
            resource_user_id: None,
            authenticated_owner: None,
            devices: BTreeMap::new(),
            next_descriptor: 1,
            map_objects: BTreeMap::new(),
            next_map_handle: 1,
            gpu_address_spaces: BTreeMap::new(),
            channel_address_spaces: BTreeMap::new(),
            channel_map_descriptors: BTreeMap::new(),
            channel_events: BTreeMap::new(),
        }
    }
}

impl NvClient {
    pub(crate) fn channel_identities(&self) -> impl Iterator<Item = NvGpuChannelIdentity> + '_ {
        self.devices
            .iter()
            .filter(|(_, kind)| **kind == NvDeviceKind::GpuChannel)
            .map(|(&descriptor, _)| NvGpuChannelIdentity {
                client: self.identity.clone(),
                descriptor,
            })
    }

    pub(crate) fn is_authenticated_owner(&self, owner: u64) -> bool {
        self.transfer.is_some() && self.authenticated_owner == Some(owner)
    }

    pub(crate) fn channel_identity(&self, descriptor: u32) -> Option<NvGpuChannelIdentity> {
        (self.device(descriptor) == Some(NvDeviceKind::GpuChannel)).then(|| NvGpuChannelIdentity {
            client: self.identity.clone(),
            descriptor,
        })
    }

    pub fn channel_map_descriptor(&self, descriptor: u32) -> Option<u32> {
        self.channel_map_descriptors.get(&descriptor).copied()
    }

    fn release_map_objects_if_unused(&mut self, descriptor: u32) {
        // A channel retains its nvmap client even when the user's map fd closes.
        if self.device(descriptor) != Some(NvDeviceKind::Map)
            && !self
                .channel_map_descriptors
                .values()
                .any(|&map| map == descriptor)
        {
            self.map_objects
                .retain(|_, object| object.owner_descriptor != descriptor);
        }
    }

    fn set_channel_map_descriptor(
        &mut self,
        descriptor: u32,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let active: Vec<_> = request
            .buffers
            .iter()
            .filter(|buffer| buffer.capacity != 0 || !buffer.data.is_empty())
            .collect();
        let [input] = active.as_slice() else {
            return Err(HorizonError::IpcProtocol(
                "SetNvmapFd requires one input".into(),
            ));
        };
        if input.direction != crate::ipc::IpcBufferDirection::In
            || input.capacity != 4
            || input.data.len() != 4
        {
            return Err(HorizonError::IpcProtocol(
                "invalid SetNvmapFd structure".into(),
            ));
        }
        let map = u32::from_le_bytes(input.data[..4].try_into().unwrap());
        if self.device(map) != Some(NvDeviceKind::Map) {
            return Ok(nv_reply(&[4]));
        }
        if self.channel_map_descriptors.contains_key(&descriptor) {
            return Err(HorizonError::InvalidState(
                "repeated SetNvmapFd is not implemented".into(),
            ));
        }
        self.channel_map_descriptors.insert(descriptor, map);
        Ok(nv_reply(&[0]))
    }

    pub fn device(&self, descriptor: u32) -> Option<NvDeviceKind> {
        self.devices.get(&descriptor).copied()
    }

    pub fn map_object(&self, handle: u32) -> Option<&NvMapObject> {
        self.map_objects.get(&handle)
    }

    pub fn gpu_address_space(&self, descriptor: u32) -> Option<&address_space::NvGpuAddressSpace> {
        self.gpu_address_spaces.get(&descriptor)
    }

    pub fn channel_address_space(
        &self,
        descriptor: u32,
    ) -> Option<&address_space::NvGpuAddressSpace> {
        self.channel_address_spaces
            .get(&descriptor)
            .and_then(|space| self.gpu_address_spaces.get(space))
    }

    fn release_gpu_space_if_unused(&mut self, descriptor: u32) {
        // The fd and channels are independent references to the same mutable
        // space. Never snapshot mappings when binding a channel.
        if self.device(descriptor) != Some(NvDeviceKind::GpuAddressSpace)
            && !self
                .channel_address_spaces
                .values()
                .any(|&space| space == descriptor)
        {
            self.gpu_address_spaces.remove(&descriptor);
        }
    }

    fn bind_gpu_channel(
        &mut self,
        descriptor: u32,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let mut active = request
            .buffers
            .iter()
            .filter(|buffer| buffer.capacity != 0 || !buffer.data.is_empty());
        let Some(input) = active.next() else {
            return Err(HorizonError::IpcProtocol(
                "BindChannel requires input".into(),
            ));
        };
        if active.next().is_some()
            || input.direction != crate::ipc::IpcBufferDirection::In
            || input.capacity != 4
            || input.data.len() != 4
        {
            return Err(HorizonError::IpcProtocol(
                "invalid BindChannel structure".into(),
            ));
        }
        let channel = u32::from_le_bytes(input.data[..4].try_into().unwrap());
        if self.device(channel) != Some(NvDeviceKind::GpuChannel)
            || self.channel_address_spaces.contains_key(&channel)
            || self
                .gpu_address_spaces
                .get(&descriptor)
                .and_then(|space| space.configuration())
                .is_none()
        {
            return Ok(nv_reply(&[4]));
        }
        let reply = nv_reply(&[0]);
        self.channel_address_spaces.insert(channel, descriptor);
        Ok(reply)
    }

    fn gpu_map_or_unmap(
        &mut self,
        descriptor: u32,
        request: &ServiceRequest,
        unmap: bool,
    ) -> HorizonResult<ServiceResponse> {
        let (input, output_index) = ioctl_inout_buffer(request, if unmap { 8 } else { 40 })?;
        let mut reply = nv_reply(&[0]);
        let mut data = input.data.clone();
        if unmap {
            let address = u64::from_le_bytes(input.data[..8].try_into().unwrap());
            let space = self
                .gpu_address_spaces
                .get_mut(&descriptor)
                .ok_or(HorizonError::InvalidHandle(descriptor))?;
            space.unmap(address)?;
        } else {
            let word =
                |offset| u32::from_le_bytes(input.data[offset..offset + 4].try_into().unwrap());
            let wide =
                |offset| u64::from_le_bytes(input.data[offset..offset + 8].try_into().unwrap());
            if word(0) != 1 {
                return Err(HorizonError::IpcProtocol(
                    "automatic/cacheable/remapped GPU mappings are not implemented".into(),
                ));
            }
            let object = self
                .map_objects
                .get(&word(8))
                .ok_or(HorizonError::InvalidHandle(word(8)))?;
            let backing = object.backing.as_ref().ok_or_else(|| {
                HorizonError::InvalidState("GPU mapping requires allocated nvmap backing".into())
            })?;
            if word(4) != u32::MAX && word(4) != u32::from(backing.kind) {
                return Err(HorizonError::IpcProtocol(
                    "GPU kind reinterpretation is not implemented".into(),
                ));
            }
            let buffer_offset = wide(16);
            let size = if wide(24) == 0 {
                object
                    .size
                    .checked_sub(buffer_offset)
                    .ok_or(HorizonError::InvalidAddress(buffer_offset))?
            } else {
                wide(24)
            };
            let space = self
                .gpu_address_spaces
                .get_mut(&descriptor)
                .ok_or(HorizonError::InvalidHandle(descriptor))?;
            let page_size = space.map_reserved(wide(32), size, buffer_offset, word(12), backing)?;
            data[12..16].copy_from_slice(&page_size.to_le_bytes());
        }
        reply.buffer_writes.push(crate::ipc::IpcBufferWrite {
            buffer_index: output_index,
            data,
        });
        Ok(reply)
    }

    fn initialize_gpu_address_space(
        &mut self,
        descriptor: u32,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let mut active = request
            .buffers
            .iter()
            .filter(|buffer| buffer.capacity != 0 || !buffer.data.is_empty());
        let Some(input) = active.next() else {
            return Err(HorizonError::IpcProtocol(
                "InitializeEx requires input".into(),
            ));
        };
        if active.next().is_some()
            || input.direction != crate::ipc::IpcBufferDirection::In
            || input.capacity != 40
            || input.data.len() != 40
        {
            return Err(HorizonError::IpcProtocol(
                "invalid InitializeEx input structure".into(),
            ));
        }
        let word = |offset| u32::from_le_bytes(input.data[offset..offset + 4].try_into().unwrap());
        // Address-space sharing requires a separate ownership/lifetime model.
        if word(4) != 0 {
            return Err(HorizonError::IpcProtocol(
                "unsupported inherited GPU address space".into(),
            ));
        }
        let big_page_size = if word(8) == 0 {
            GM20B_CHARACTERISTICS[10]
        } else {
            word(8)
        };
        let space = self
            .gpu_address_spaces
            .get_mut(&descriptor)
            .ok_or(HorizonError::InvalidHandle(descriptor))?;
        let wide = |offset| u64::from_le_bytes(input.data[offset..offset + 8].try_into().unwrap());
        let bounds = (wide(16), wide(24), wide(32));
        if bounds == (0, 0, 0) {
            space.initialize(word(0), big_page_size)?;
        } else {
            space.initialize_with_ranges(word(0), big_page_size, Some(bounds))?;
        }
        Ok(nv_reply(&[0]))
    }

    fn gpu_va_regions(
        &self,
        descriptor: u32,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let (input, output_index) = ioctl_inout_buffer(request, 64)?;
        let space = self
            .gpu_address_spaces
            .get(&descriptor)
            .ok_or(HorizonError::InvalidHandle(descriptor))?;
        let Some(regions) = space.regions() else {
            // Default firmware bounds are not established yet. Do not advertise
            // arbitrary ranges or turn an unresolved query into generic success.
            return Err(HorizonError::UnknownCommand {
                service: NvDeviceKind::GpuAddressSpace.path().into(),
                command_id: 0xc0404108,
            });
        };
        let mut data = input.data.clone();
        data[8..12].copy_from_slice(&48u32.to_le_bytes());
        for (index, region) in regions.iter().enumerate() {
            let offset = 16 + index * 24;
            data[offset..offset + 8].copy_from_slice(&region.start.to_le_bytes());
            data[offset + 8..offset + 12].copy_from_slice(&region.page_size.to_le_bytes());
            data[offset + 12..offset + 16].fill(0);
            let pages = (region.end - region.start) / u64::from(region.page_size);
            data[offset + 16..offset + 24].copy_from_slice(&pages.to_le_bytes());
        }
        let mut reply = nv_reply(&[0]);
        reply.buffer_writes.push(crate::ipc::IpcBufferWrite {
            buffer_index: output_index,
            data,
        });
        Ok(reply)
    }

    fn gpu_reserve_or_free(
        &mut self,
        descriptor: u32,
        request: &ServiceRequest,
        free: bool,
    ) -> HorizonResult<ServiceResponse> {
        let (input, output_index) = ioctl_inout_buffer(request, if free { 16 } else { 24 })?;
        let word = |offset| u32::from_le_bytes(input.data[offset..offset + 4].try_into().unwrap());
        let mut data = input.data.clone();
        let space = self
            .gpu_address_spaces
            .get_mut(&descriptor)
            .ok_or(HorizonError::InvalidHandle(descriptor))?;
        if free {
            let address = u64::from_le_bytes(input.data[..8].try_into().unwrap());
            space.free_space(address, word(8), word(12))?;
        } else {
            let align_or_offset = u64::from_le_bytes(input.data[16..24].try_into().unwrap());
            // libnx AllocSpace's union is alignment on input for automatic
            // requests and the selected GPU VA on output. Sparse stays explicit.
            let address = match word(8) {
                0 => space.reserve_auto(word(0), word(4), align_or_offset)?,
                1 => {
                    space.reserve_fixed(align_or_offset, word(0), word(4))?;
                    align_or_offset
                }
                flags => {
                    return Err(HorizonError::IpcProtocol(format!(
                        "unsupported GPU reservation flags {flags:#x}; sparse reservations are not implemented"
                    )));
                }
            };
            data[16..24].copy_from_slice(&address.to_le_bytes());
        }
        let mut reply = nv_reply(&[0]);
        reply.buffer_writes.push(crate::ipc::IpcBufferWrite {
            buffer_index: output_index,
            data,
        });
        Ok(reply)
    }

    fn create_map_object(
        &mut self,
        descriptor: u32,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let (input, output_index) = ioctl_inout_buffer(request, 8)?;
        let requested_size = u32::from_le_bytes(input.data[..4].try_into().unwrap());
        let size = (u64::from(requested_size) + 4095) & !4095;
        if requested_size == 0 || size > u64::from(u32::MAX) {
            return Ok(nv_reply(&[4]));
        }
        let Some(next) = self.next_map_handle.checked_add(1) else {
            return Ok(nv_reply(&[6]));
        };
        let handle = self.next_map_handle;
        let mut data = input.data.clone();
        data[4..8].copy_from_slice(&handle.to_le_bytes());
        let mut reply = nv_reply(&[0]);
        reply.buffer_writes.push(crate::ipc::IpcBufferWrite {
            buffer_index: output_index,
            data,
        });
        // Create reserves an object identity/size, not GPU backing. Alloc must
        // separately authenticate and retain the caller's actual memory range.
        self.map_objects.insert(
            handle,
            NvMapObject {
                requested_size,
                size,
                owner_descriptor: descriptor,
                backing: None,
            },
        );
        self.next_map_handle = next;
        Ok(reply)
    }

    fn map_size_or_free(
        &mut self,
        descriptor: u32,
        request: &ServiceRequest,
        free: bool,
    ) -> HorizonResult<ServiceResponse> {
        let (input, output_index) = ioctl_inout_buffer(request, if free { 24 } else { 12 })?;
        let handle = u32::from_le_bytes(input.data[..4].try_into().unwrap());
        let Some(object) = self
            .map_objects
            .get(&handle)
            .filter(|object| object.owner_descriptor == descriptor)
        else {
            return Ok(nv_reply(&[4]));
        };
        let mut data = input.data.clone();
        if free {
            let address = object
                .backing
                .as_ref()
                .filter(|backing| backing.lease.is_exclusively_owned())
                .map_or(0, |backing| backing.address);
            data[8..16].copy_from_slice(&address.to_le_bytes());
            data[16..20].copy_from_slice(&(object.size as u32).to_le_bytes());
            let uncached = object
                .backing
                .as_ref()
                .map_or(0, |backing| (backing.flags >> 1) & 1);
            data[20..24].copy_from_slice(&uncached.to_le_bytes());
        } else {
            let parameter = u32::from_le_bytes(input.data[4..8].try_into().unwrap());
            let value = match parameter {
                1 => object.size as u32,
                2 | 5 => {
                    let Some(backing) = &object.backing else {
                        return Ok(nv_reply(&[4]));
                    };
                    if parameter == 2 {
                        backing.alignment
                    } else {
                        u32::from(backing.kind)
                    }
                }
                _ => {
                    return Err(HorizonError::UnknownCommand {
                        service: "nvmap Param".into(),
                        command_id: parameter,
                    });
                }
            };
            data[8..12].copy_from_slice(&value.to_le_bytes());
        }
        let mut reply = nv_reply(&[0]);
        reply.buffer_writes.push(crate::ipc::IpcBufferWrite {
            buffer_index: output_index,
            data,
        });
        if free {
            self.map_objects.remove(&handle);
        }
        Ok(reply)
    }

    pub fn dispatch(
        &mut self,
        request: &ServiceRequest,
        transfer: Option<KernelObject>,
        resource_user_id: u64,
    ) -> HorizonResult<ServiceResponse> {
        self.dispatch_inner(request, transfer, resource_user_id, None)
    }

    pub fn dispatch_with_memory(
        &mut self,
        request: &ServiceRequest,
        transfer: Option<KernelObject>,
        resource_user_id: u64,
        memory: &mut crate::memory::AddressSpace,
    ) -> HorizonResult<ServiceResponse> {
        self.dispatch_inner(request, transfer, resource_user_id, Some(memory))
    }

    fn allocate_map_object(
        &mut self,
        descriptor: u32,
        request: &ServiceRequest,
        memory: &mut crate::memory::AddressSpace,
    ) -> HorizonResult<ServiceResponse> {
        let (input, output_index) = ioctl_inout_buffer(request, 32)?;
        let word = |offset| u32::from_le_bytes(input.data[offset..offset + 4].try_into().unwrap());
        let handle = word(0);
        let Some(object) = self
            .map_objects
            .get_mut(&handle)
            .filter(|object| object.owner_descriptor == descriptor)
        else {
            return Ok(nv_reply(&[4]));
        };
        let flags = word(8);
        let requested_alignment = word(12);
        let alignment = requested_alignment.max(4096);
        let address = u64::from_le_bytes(input.data[24..32].try_into().unwrap());
        if object.backing.is_some()
            || flags & !3 != 0
            || (requested_alignment != 0 && !requested_alignment.is_power_of_two())
            || address == 0
            || address % u64::from(alignment) != 0
        {
            return Ok(nv_reply(&[4]));
        }
        let mut data = input.data.clone();
        data[12..16].copy_from_slice(&alignment.to_le_bytes());
        let mut reply = nv_reply(&[0]);
        reply.buffer_writes.push(crate::ipc::IpcBufferWrite {
            buffer_index: output_index,
            data,
        });
        let permission = if flags & 1 != 0 {
            crate::memory::MemoryPermission::READ_WRITE
        } else {
            crate::memory::MemoryPermission::READ
        };
        let lease = memory.pin_device_memory(address, object.size, permission)?;
        object.backing = Some(NvMapBacking {
            address,
            alignment,
            flags,
            kind: input.data[16],
            heap_mask: word(4),
            lease,
        });
        Ok(reply)
    }

    fn dispatch_inner(
        &mut self,
        request: &ServiceRequest,
        transfer: Option<KernelObject>,
        resource_user_id: u64,
        memory: Option<&mut crate::memory::AddressSpace>,
    ) -> HorizonResult<ServiceResponse> {
        match request.command_id {
            0 | 1 | 2 | 4 | 11 | 12 => {
                if self.transfer.is_none() || self.authenticated_owner != Some(resource_user_id) {
                    return Err(HorizonError::InvalidState(
                        "nvdrv client is uninitialized or foreign".into(),
                    ));
                }
                match request.command_id {
                    4 => {
                        if !request.buffers.is_empty() {
                            return Err(HorizonError::IpcProtocol(
                                "nvdrv QueryEvent does not accept buffers".into(),
                            ));
                        }
                        let args = request.raw_payload.get(..8).ok_or_else(|| {
                            HorizonError::IpcProtocol("truncated nvdrv QueryEvent".into())
                        })?;
                        let descriptor = u32::from_le_bytes(args[..4].try_into().unwrap());
                        let event_id = u32::from_le_bytes(args[4..].try_into().unwrap());
                        if self.device(descriptor) != Some(NvDeviceKind::GpuChannel)
                            || !matches!(event_id, 1..=3)
                        {
                            return Ok(nv_reply(&[4]));
                        }
                        // Querying a debug/error event is not a GPU completion.
                        // New channels have no pending notification. Copies keep
                        // object identity and must not manufacture a signal.
                        let event = self
                            .channel_events
                            .entry((descriptor, event_id))
                            .or_default();
                        let mut reply = nv_reply(&[0]);
                        reply
                            .objects
                            .push(crate::ipc::ReturnedObject::ReadableEvent(event.clone()));
                        return Ok(reply);
                    }
                    0 => {
                        let kind = requested_device(request)?;
                        let next = self.next_descriptor.checked_add(1).ok_or_else(|| {
                            HorizonError::InvalidState(
                                "nvdrv descriptor identities exhausted".into(),
                            )
                        })?;
                        let descriptor = self.next_descriptor;
                        let reply = nv_reply(&[descriptor, 0]); // fd, then NvError; neither is a CMIF handle.
                        self.devices.insert(descriptor, kind);
                        if kind == NvDeviceKind::GpuAddressSpace {
                            self.gpu_address_spaces
                                .insert(descriptor, Default::default());
                        }
                        self.next_descriptor = next;
                        return Ok(reply);
                    }
                    2 => {
                        if !request.buffers.is_empty() {
                            return Err(HorizonError::IpcProtocol(
                                "nvdrv Close does not accept buffers".into(),
                            ));
                        }
                        let bytes = request.raw_payload.get(..4).ok_or_else(|| {
                            HorizonError::IpcProtocol("truncated nvdrv Close".into())
                        })?;
                        let descriptor = u32::from_le_bytes(bytes.try_into().unwrap());
                        let status = if let Some(kind) = self.devices.remove(&descriptor) {
                            if kind == NvDeviceKind::GpuAddressSpace {
                                self.release_gpu_space_if_unused(descriptor);
                            }
                            if kind == NvDeviceKind::GpuChannel {
                                self.channel_events
                                    .retain(|(channel, _), _| *channel != descriptor);
                                if let Some(map) = self.channel_map_descriptors.remove(&descriptor)
                                {
                                    self.release_map_objects_if_unused(map);
                                }
                                if let Some(space) = self.channel_address_spaces.remove(&descriptor)
                                {
                                    self.release_gpu_space_if_unused(space);
                                }
                            }
                            if kind == NvDeviceKind::Map {
                                self.release_map_objects_if_unused(descriptor);
                            }
                            0
                        } else {
                            4
                        };
                        return Ok(nv_reply(&[status]));
                    }
                    _ => {
                        let args = request.raw_payload.get(..8).ok_or_else(|| {
                            HorizonError::IpcProtocol("truncated nvdrv IOCTL".into())
                        })?;
                        let descriptor = u32::from_le_bytes(args[..4].try_into().unwrap());
                        let ioctl = u32::from_le_bytes(args[4..8].try_into().unwrap());
                        let Some(kind) = self.device(descriptor) else {
                            return Ok(nv_reply(&[4]));
                        };
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuChannel
                            && ioctl == 0x40044801
                        {
                            return self.set_channel_map_descriptor(descriptor, request);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuAddressSpace
                            && ioctl == 0x40044101
                        {
                            return self.bind_gpu_channel(descriptor, request);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuAddressSpace
                            && ioctl == 0x40284109
                        {
                            return self.initialize_gpu_address_space(descriptor, request);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuAddressSpace
                            && matches!(ioctl, 0xc0184102 | 0xc0104103)
                        {
                            return self.gpu_reserve_or_free(
                                descriptor,
                                request,
                                ioctl == 0xc0104103,
                            );
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuAddressSpace
                            && matches!(ioctl, 0xc0284106 | 0xc0084105)
                        {
                            return self.gpu_map_or_unmap(descriptor, request, ioctl == 0xc0084105);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::Map
                            && ioctl == 0xc0080101
                        {
                            return self.create_map_object(descriptor, request);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::Map
                            && ioctl == 0xc0200104
                        {
                            let memory = memory.ok_or_else(|| {
                                HorizonError::InvalidState(
                                    "nvmap Alloc requires the caller address space".into(),
                                )
                            })?;
                            return self.allocate_map_object(descriptor, request, memory);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::Map
                            && matches!(ioctl, 0xc0180105 | 0xc00c0109)
                        {
                            return self.map_size_or_free(descriptor, request, ioctl == 0xc0180105);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuControl
                            && ioctl == 0xc0b04705
                        {
                            return gpu_characteristics(request);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuAddressSpace
                            && ioctl == 0xc0404108
                        {
                            return self.gpu_va_regions(descriptor, request);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuControl
                            && ioctl == 0xc0184706
                        {
                            return gpu_tpc_masks(request);
                        }
                        if request.command_id == 1
                            && kind == NvDeviceKind::GpuControl
                            && ioctl == 0x80284702
                        {
                            return ioctl_output_words(request, &GM20B_ZCULL_INFO);
                        }
                        return Err(HorizonError::UnknownCommand {
                            service: kind.path().into(),
                            command_id: ioctl,
                        });
                    }
                }
            }
            3 => {
                if self.transfer.is_some() {
                    return Err(HorizonError::InvalidState(
                        "nvdrv client already initialized".into(),
                    ));
                }
                self.transfer = Some(transfer.ok_or_else(|| {
                    HorizonError::IpcProtocol(
                        "nvdrv Initialize requires validated process/transfer handles".into(),
                    )
                })?);
                self.authenticated_owner = Some(resource_user_id);
            }
            8 => {
                if self.transfer.is_none() || self.authenticated_owner != Some(resource_user_id) {
                    return Err(HorizonError::InvalidState(
                        "nvdrv client is uninitialized or foreign".into(),
                    ));
                }
                let bytes = request.raw_payload.get(..8).ok_or_else(|| {
                    HorizonError::IpcProtocol("nvdrv SetAruid requires a u64".into())
                })?;
                let requested = u64::from_le_bytes(bytes.try_into().unwrap());
                if requested != resource_user_id {
                    return Err(HorizonError::IpcProtocol(
                        "nvdrv resource identity does not belong to caller".into(),
                    ));
                }
                self.resource_user_id = Some(requested);
            }
            _ => {
                return Err(HorizonError::UnknownCommand {
                    service: "nvdrv".into(),
                    command_id: request.command_id,
                });
            }
        }
        let mut response = ServiceResponse::success();
        response.raw_payload.extend_from_slice(&0u32.to_le_bytes()); // NvError_Success, distinct from CMIF result.
        Ok(response)
    }
}
