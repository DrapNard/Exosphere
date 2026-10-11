// SPDX-License-Identifier: GPL-3.0-or-later
//! Strict AllocObjCtx transport decoding; the object ID is output only.

use crate::{
    HorizonError, HorizonResult,
    ipc::{IpcBufferDirection, ServiceRequest},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectContextRequest {
    pub class: u32,
    pub flags: u32,
    pub input_index: usize,
    pub output_index: usize,
}

impl ObjectContextRequest {
    pub fn decode(request: &ServiceRequest) -> HorizonResult<Self> {
        let invalid = || HorizonError::IpcProtocol("invalid AllocObjCtx transport".into());
        if request.command_id != 1
            || request.raw_payload.len() != 8
            || u32::from_le_bytes(request.raw_payload[4..8].try_into().unwrap()) != 0xc0104809
        {
            return Err(invalid());
        }
        let mut active = request
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, buffer)| buffer.capacity != 0 || !buffer.data.is_empty());
        let (input_index, input) = active.next().ok_or_else(invalid)?;
        let (output_index, output) = active.next().ok_or_else(invalid)?;
        if active.next().is_some()
            || input.direction != IpcBufferDirection::In
            || input.capacity != 16
            || input.data.len() != 16
            || output.direction != IpcBufferDirection::Out
            || output.capacity < 16
            || !output.data.is_empty()
        {
            return Err(invalid());
        }
        Ok(Self {
            class: u32::from_le_bytes(input.data[..4].try_into().unwrap()),
            flags: u32::from_le_bytes(input.data[4..8].try_into().unwrap()),
            input_index,
            output_index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcBuffer;

    #[test]
    fn object_output_words_and_inactive_slots_do_not_become_inputs() {
        let mut bytes = vec![0; 16];
        bytes[..4].copy_from_slice(&0xb197u32.to_le_bytes());
        bytes[8..16].fill(0xff);
        let inactive = IpcBuffer {
            address: 0,
            capacity: 0,
            direction: IpcBufferDirection::In,
            data: vec![],
        };
        let mut request = ServiceRequest {
            service: "nvdrv".into(),
            command_id: 1,
            values: vec![],
            raw_payload: [1u32.to_le_bytes(), 0xc0104809u32.to_le_bytes()].concat(),
            buffers: vec![
                inactive.clone(),
                IpcBuffer {
                    address: 0,
                    capacity: 16,
                    direction: IpcBufferDirection::In,
                    data: bytes,
                },
                inactive,
                IpcBuffer {
                    address: 0,
                    capacity: 16,
                    direction: IpcBufferDirection::Out,
                    data: vec![],
                },
            ],
        };
        assert_eq!(
            ObjectContextRequest::decode(&request).unwrap(),
            ObjectContextRequest {
                class: 0xb197,
                flags: 0,
                input_index: 1,
                output_index: 3,
            }
        );
        request.buffers[3].capacity = 15;
        assert!(ObjectContextRequest::decode(&request).is_err());
        request.buffers[3].capacity = 16;
        request.buffers[1].data.pop();
        assert!(ObjectContextRequest::decode(&request).is_err());
    }
}
