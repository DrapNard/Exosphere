// SPDX-License-Identifier: GPL-3.0-or-later
//! AllocGpfifoEx2 wire decoding; no queue, syncpoint or completion is synthesized.

use crate::ipc::{IpcBufferDirection, ServiceRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpfifoAllocation {
    pub entries: u32,
    pub flags: u32,
    /// Unknown input words retained in their ABI order, excluding the output fence.
    pub parameters: [u32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AllocationDecodeError {
    #[error("invalid AllocGpfifoEx2 IOCTL transport")]
    Transport,
    #[error("AllocGpfifoEx2 requires exactly 32 input bytes")]
    Size,
    #[error("GPFIFO allocation exceeds the software queue capacity policy")]
    Capacity,
}

impl GpfifoAllocation {
    pub fn decode(bytes: &[u8], maximum_entries: usize) -> Result<Self, AllocationDecodeError> {
        if bytes.len() != 32 {
            return Err(AllocationDecodeError::Size);
        }
        let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let entries = word(0);
        // This decoder's caller supplies an admission policy, not a presumed
        // hardware queue limit. Zero cannot create a runtime command queue.
        if entries == 0 || entries as u64 > maximum_entries as u64 {
            return Err(AllocationDecodeError::Capacity);
        }
        Ok(Self {
            entries,
            flags: word(4),
            parameters: [word(8), word(20), word(24), word(28)],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpfifoAllocationRequest {
    pub descriptor: u32,
    pub output_index: usize,
    pub allocation: GpfifoAllocation,
}

impl GpfifoAllocationRequest {
    pub fn decode(
        request: &ServiceRequest,
        maximum_entries: usize,
    ) -> Result<Self, AllocationDecodeError> {
        if request.command_id != 1 || request.raw_payload.len() != 8 {
            return Err(AllocationDecodeError::Transport);
        }
        let descriptor = u32::from_le_bytes(request.raw_payload[..4].try_into().unwrap());
        let ioctl = u32::from_le_bytes(request.raw_payload[4..].try_into().unwrap());
        if ioctl != 0xc020481a {
            return Err(AllocationDecodeError::Transport);
        }
        let mut buffers = request
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, buffer)| buffer.capacity != 0 || !buffer.data.is_empty());
        let (_, input) = buffers.next().ok_or(AllocationDecodeError::Transport)?;
        let (output_index, output) = buffers.next().ok_or(AllocationDecodeError::Transport)?;
        if buffers.next().is_some()
            || input.direction != IpcBufferDirection::In
            || input.capacity != 32
            || output.direction != IpcBufferDirection::Out
            || output.capacity < 32
            || !output.data.is_empty()
        {
            return Err(AllocationDecodeError::Transport);
        }
        Ok(Self {
            descriptor,
            output_index,
            allocation: GpfifoAllocation::decode(&input.data, maximum_entries)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcBuffer;

    #[test]
    fn allocation_retains_inputs_without_treating_output_fence_as_input() {
        let bytes: Vec<_> = [512u32, 1, 7, u32::MAX, u32::MAX, 11, 13, 17]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        assert_eq!(
            GpfifoAllocation::decode(&bytes, 512).unwrap(),
            GpfifoAllocation {
                entries: 512,
                flags: 1,
                parameters: [7, 11, 13, 17],
            }
        );
        assert_eq!(
            GpfifoAllocation::decode(&bytes, 511),
            Err(AllocationDecodeError::Capacity)
        );
        assert_eq!(
            GpfifoAllocation::decode(&bytes[..31], 512),
            Err(AllocationDecodeError::Size)
        );
        let mut request = ServiceRequest {
            service: "nvdrv".into(),
            command_id: 1,
            values: vec![],
            raw_payload: [3u32.to_le_bytes(), 0xc020481au32.to_le_bytes()].concat(),
            buffers: vec![
                IpcBuffer {
                    address: 0,
                    capacity: 0,
                    direction: IpcBufferDirection::In,
                    data: vec![],
                },
                IpcBuffer {
                    address: 0,
                    capacity: 32,
                    direction: IpcBufferDirection::In,
                    data: bytes,
                },
                IpcBuffer {
                    address: 0,
                    capacity: 0,
                    direction: IpcBufferDirection::Out,
                    data: vec![],
                },
                IpcBuffer {
                    address: 0,
                    capacity: 32,
                    direction: IpcBufferDirection::Out,
                    data: vec![],
                },
            ],
        };
        let decoded = GpfifoAllocationRequest::decode(&request, 512).unwrap();
        assert_eq!(decoded.descriptor, 3);
        assert_eq!(decoded.output_index, 3);
        request.buffers[3].capacity = 31;
        assert_eq!(
            GpfifoAllocationRequest::decode(&request, 512),
            Err(AllocationDecodeError::Transport)
        );
    }
}
