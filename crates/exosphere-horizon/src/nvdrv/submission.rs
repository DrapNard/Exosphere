// SPDX-License-Identifier: GPL-3.0-or-later
//! Borrowed decoding of libnx SubmitGpfifo and KickoffPb payloads.
//! Decoding does not admit flags, dereference guest pointers or complete fences.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmissionFence {
    pub id: u32,
    pub value: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct GpfifoSubmission<'a> {
    pub flags: u32,
    pub fence: SubmissionFence,
    entries: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SubmissionDecodeError {
    #[error("invalid GPU submission IOCTL transport")]
    Transport,
    #[error("GPU submission header must contain exactly 24 bytes")]
    Header,
    #[error("GPU submission entry count exceeds the software admission budget")]
    Budget,
    #[error("GPU submission entry payload does not match its declared count")]
    Entries,
}

/// Validated materialized IPC buffers; ownership is checked separately by nvdrv.
#[derive(Debug)]
pub struct GpfifoSubmissionRequest<'a> {
    pub descriptor: u32,
    pub output_index: usize,
    pub submission: GpfifoSubmission<'a>,
}

impl<'a> GpfifoSubmissionRequest<'a> {
    pub fn decode(
        request: &'a crate::ipc::ServiceRequest,
        maximum_entries: usize,
    ) -> Result<Self, SubmissionDecodeError> {
        use crate::ipc::IpcBufferDirection;
        if request.raw_payload.len() != 8 {
            return Err(SubmissionDecodeError::Transport);
        }
        let descriptor = u32::from_le_bytes(request.raw_payload[..4].try_into().unwrap());
        let ioctl = u32::from_le_bytes(request.raw_payload[4..].try_into().unwrap());
        let inline = request.command_id == 1 && ioctl & 0xc000ffff == 0xc0004808;
        let split = request.command_id == 11 && ioctl == 0xc018481b;
        if !inline && !split {
            return Err(SubmissionDecodeError::Transport);
        }
        let wire_size = ((ioctl >> 16) & 0x3fff) as usize;
        // Skip unused AutoSelect slots without losing writeback indices.
        let mut active = request
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, buffer)| buffer.capacity != 0 || !buffer.data.is_empty())
            .peekable();
        let (_, input) = active.next().ok_or(SubmissionDecodeError::Transport)?;
        if input.direction != IpcBufferDirection::In
            || input.data.len() != wire_size
            || input.capacity != wire_size as u64
        {
            return Err(SubmissionDecodeError::Transport);
        }
        let submission = if split {
            let entries = if active
                .peek()
                .is_some_and(|(_, buffer)| buffer.direction == IpcBufferDirection::In)
            {
                let (_, entries) = active.next().ok_or(SubmissionDecodeError::Transport)?;
                if entries.capacity != entries.data.len() as u64 {
                    return Err(SubmissionDecodeError::Transport);
                }
                entries.data.as_slice()
            } else {
                // AutoSelect may omit the empty entry buffer. The declared
                // count must still match zero before accepting this layout.
                &[]
            };
            GpfifoSubmission::split(&input.data, entries, maximum_entries)?
        } else {
            GpfifoSubmission::inline(&input.data, maximum_entries)?
        };
        let (output_index, output) = active.next().ok_or(SubmissionDecodeError::Transport)?;
        if active.next().is_some()
            || output.direction != IpcBufferDirection::Out
            || !output.data.is_empty()
            || output.capacity < wire_size as u64
        {
            return Err(SubmissionDecodeError::Transport);
        }
        Ok(Self {
            descriptor,
            output_index,
            submission,
        })
    }
}

impl<'a> GpfifoSubmission<'a> {
    /// SubmitGpfifo stores entries immediately after its fixed header.
    pub fn inline(bytes: &'a [u8], maximum_entries: usize) -> Result<Self, SubmissionDecodeError> {
        let header = bytes.get(..24).ok_or(SubmissionDecodeError::Header)?;
        Self::split(header, &bytes[24..], maximum_entries)
    }

    /// KickoffPb supplies entries through the separate Ioctl2 input buffer.
    pub fn split(
        header: &[u8],
        entries: &'a [u8],
        maximum_entries: usize,
    ) -> Result<Self, SubmissionDecodeError> {
        if header.len() != 24 {
            return Err(SubmissionDecodeError::Header);
        }
        let word = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
        let count = word(8) as usize;
        if count > maximum_entries {
            return Err(SubmissionDecodeError::Budget);
        }
        if count.checked_mul(8) != Some(entries.len()) {
            return Err(SubmissionDecodeError::Entries);
        }
        // The first eight bytes are a client pointer ignored by this wire ABI.
        // Entries come only from the validated transport buffers, never that pointer.
        Ok(Self {
            flags: word(12),
            fence: SubmissionFence {
                id: word(16),
                value: word(20),
            },
            entries,
        })
    }

    pub fn entries(&self) -> impl ExactSizeIterator<Item = u64> + Clone + '_ {
        self.entries
            .chunks_exact(8)
            .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_split_entries_may_be_inactive_but_nonzero_counts_cannot() {
        use crate::ipc::{IpcBuffer, IpcBufferDirection, ServiceRequest};
        let mut request = ServiceRequest {
            service: "nvdrv".into(),
            command_id: 11,
            values: vec![],
            raw_payload: [3u32.to_le_bytes(), 0xc018481bu32.to_le_bytes()].concat(),
            buffers: vec![
                IpcBuffer {
                    address: 0,
                    capacity: 24,
                    direction: IpcBufferDirection::In,
                    data: vec![0; 24],
                },
                IpcBuffer {
                    address: 0,
                    capacity: 0,
                    direction: IpcBufferDirection::In,
                    data: vec![],
                },
                IpcBuffer {
                    address: 0,
                    capacity: 24,
                    direction: IpcBufferDirection::Out,
                    data: vec![],
                },
            ],
        };
        let decoded = GpfifoSubmissionRequest::decode(&request, 1).unwrap();
        assert_eq!(decoded.output_index, 2);
        assert_eq!(decoded.submission.entries().len(), 0);
        request.buffers[0].data[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            GpfifoSubmissionRequest::decode(&request, 1).unwrap_err(),
            SubmissionDecodeError::Entries,
        );
    }

    #[test]
    fn ipc_transports_validate_directions_sizes_and_preserve_output_indices() {
        use crate::ipc::{IpcBuffer, IpcBufferDirection, ServiceRequest};
        let mut bytes = vec![0; 32];
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        bytes[24..].copy_from_slice(&0x40000001000u64.to_le_bytes());
        let buffer = |direction, data: Vec<u8>, capacity| IpcBuffer {
            address: 0,
            capacity,
            direction,
            data,
        };
        for split in [false, true] {
            let ioctl = if split { 0xc018481bu32 } else { 0xc0204808u32 };
            let mut buffers = vec![buffer(IpcBufferDirection::In, vec![], 0)];
            buffers.push(buffer(
                IpcBufferDirection::In,
                if split {
                    bytes[..24].to_vec()
                } else {
                    bytes.clone()
                },
                if split { 24 } else { 32 },
            ));
            if split {
                buffers.push(buffer(IpcBufferDirection::In, bytes[24..].to_vec(), 8));
            }
            buffers.push(buffer(IpcBufferDirection::Out, vec![], 0));
            let output_index = buffers.len();
            buffers.push(buffer(IpcBufferDirection::Out, vec![], 32));
            let mut request = ServiceRequest {
                service: "nvdrv".into(),
                command_id: if split { 11 } else { 1 },
                values: vec![],
                raw_payload: [7u32.to_le_bytes(), ioctl.to_le_bytes()].concat(),
                buffers,
            };
            let decoded = GpfifoSubmissionRequest::decode(&request, 1).unwrap();
            assert_eq!(decoded.descriptor, 7);
            assert_eq!(decoded.output_index, output_index);
            assert_eq!(decoded.submission.entries().next(), Some(0x40000001000));
            request.buffers[output_index].capacity = 23;
            assert!(GpfifoSubmissionRequest::decode(&request, 1).is_err());
            request.buffers[output_index].capacity = 32;
            request.buffers[1].direction = IpcBufferDirection::Out;
            assert!(GpfifoSubmissionRequest::decode(&request, 1).is_err());
            request.buffers[1].direction = IpcBufferDirection::In;
            request.command_id = 12;
            assert!(GpfifoSubmissionRequest::decode(&request, 1).is_err());
        }
    }

    #[test]
    fn both_transports_preserve_entries_flags_and_fence_without_following_pointer() {
        let mut bytes = [0; 40];
        bytes[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&0x80000001u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&7u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x100000000001u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
        for submission in [
            GpfifoSubmission::inline(&bytes, 2).unwrap(),
            GpfifoSubmission::split(&bytes[..24], &bytes[24..], 2).unwrap(),
        ] {
            assert_eq!(submission.flags, 0x80000001);
            assert_eq!(
                submission.fence,
                SubmissionFence {
                    id: 7,
                    value: u32::MAX
                }
            );
            assert_eq!(
                submission.entries().collect::<Vec<_>>(),
                [0x100000000001, u64::MAX]
            );
        }
        assert_eq!(
            GpfifoSubmission::inline(&bytes, 1).unwrap_err(),
            SubmissionDecodeError::Budget
        );
        assert_eq!(
            GpfifoSubmission::inline(&bytes[..39], 2).unwrap_err(),
            SubmissionDecodeError::Entries
        );
        assert_eq!(
            GpfifoSubmission::split(&bytes[..25], &[], 2).unwrap_err(),
            SubmissionDecodeError::Header
        );
        for length in 0..24 {
            assert_eq!(
                GpfifoSubmission::inline(&bytes[..length], 2).unwrap_err(),
                SubmissionDecodeError::Header
            );
        }
        bytes[8..12].fill(0);
        assert_eq!(
            GpfifoSubmission::inline(&bytes[..24], 0)
                .unwrap()
                .entries()
                .len(),
            0
        );
        assert_eq!(
            GpfifoSubmission::inline(&bytes, 2).unwrap_err(),
            SubmissionDecodeError::Entries
        );
    }
}
