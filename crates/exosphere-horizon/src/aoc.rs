// SPDX-License-Identifier: GPL-3.0-or-later
//! Installed add-on discovery. Only caller-supplied, validated catalog entries are advertised.

use crate::{
    HorizonProfileId, HorizonService,
    error::{HorizonError, HorizonResult},
    event::KernelEvent,
    ipc::{IpcBufferDirection, IpcBufferWrite, ReturnedObject, ServiceRequest, ServiceResponse},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct AddOnCatalog {
    installed: BTreeMap<u64, BTreeSet<u32>>,
    pub changed_event: KernelEvent,
}

impl Default for AddOnCatalog {
    fn default() -> Self {
        // Reserved global identity, distinct from process-owned applet event IDs.
        Self {
            installed: BTreeMap::new(),
            changed_event: KernelEvent::new(u64::MAX, false),
        }
    }
}

impl AddOnCatalog {
    /// Call only after content and its user-provided entitlement have been validated.
    pub fn register_validated(&mut self, application_id: u64, index: u32) {
        if self
            .installed
            .entry(application_id)
            .or_default()
            .insert(index)
        {
            self.changed_event.signal();
        }
    }

    pub fn dispatch(
        &self,
        application_id: u64,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let mut response = ServiceResponse::success();
        match request.command_id {
            2 => {
                require(request, 8)?;
                let count = self.installed.get(&application_id).map_or(0, BTreeSet::len);
                response.raw_payload = (count as u32).to_le_bytes().to_vec();
            }
            3 => {
                let bytes = require(request, 16)?;
                let offset = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
                let requested = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
                let (buffer_index, buffer) = request
                    .buffers
                    .iter()
                    .enumerate()
                    .find(|(_, buffer)| {
                        matches!(
                            buffer.direction,
                            IpcBufferDirection::Out | IpcBufferDirection::InOut
                        )
                    })
                    .ok_or_else(|| {
                        HorizonError::IpcProtocol(
                            "ListAddOnContent requires an output buffer".into(),
                        )
                    })?;
                let capacity = usize::try_from(buffer.capacity / 4).unwrap_or(usize::MAX);
                if requested > capacity {
                    return Err(HorizonError::IpcProtocol(
                        "add-on list exceeds output buffer".into(),
                    ));
                }
                let mut data = Vec::new();
                if let Some(entries) = self.installed.get(&application_id) {
                    for index in entries.iter().skip(offset).take(requested) {
                        data.extend_from_slice(&index.to_le_bytes());
                    }
                }
                response.raw_payload = ((data.len() / 4) as u32).to_le_bytes().to_vec();
                response
                    .buffer_writes
                    .push(IpcBufferWrite { buffer_index, data });
            }
            8 => response.objects.push(ReturnedObject::AddOnListEvent),
            command_id => {
                return Err(HorizonError::UnknownCommand {
                    service: "aoc:u".into(),
                    command_id,
                });
            }
        }
        Ok(response)
    }
}

fn require(request: &ServiceRequest, size: usize) -> HorizonResult<&[u8]> {
    request
        .raw_payload
        .get(..size)
        .ok_or_else(|| HorizonError::IpcProtocol("truncated add-on arguments".into()))
}

#[derive(Debug)]
pub struct AddOnService;

impl HorizonService for AddOnService {
    fn name(&self) -> &'static str {
        "aoc:u"
    }
    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        // Count/List need the kernel's authenticated caller, not a guest PID word.
        if request.command_id != 8 {
            return Err(HorizonError::InvalidState(
                "add-on query requires kernel caller context".into(),
            ));
        }
        let mut response = ServiceResponse::success();
        response.objects.push(ReturnedObject::AddOnListEvent);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcBuffer;
    #[test]
    fn catalog_is_empty_until_validated_registration_and_lists_sorted_pages() {
        let mut catalog = AddOnCatalog::default();
        let mut request = ServiceRequest {
            service: "aoc:u".into(),
            command_id: 2,
            raw_payload: vec![0; 8],
            values: vec![],
            buffers: vec![],
        };
        assert_eq!(
            catalog.dispatch(1, &request).unwrap().raw_payload,
            0u32.to_le_bytes()
        );
        assert!(!catalog.changed_event.signaled);
        for index in [7, 2, 7] {
            catalog.register_validated(1, index);
        }
        assert!(catalog.changed_event.signaled);
        assert_eq!(
            catalog.dispatch(1, &request).unwrap().raw_payload,
            2u32.to_le_bytes()
        );
        assert_eq!(
            catalog.dispatch(2, &request).unwrap().raw_payload,
            0u32.to_le_bytes()
        );
        request.command_id = 3;
        request.raw_payload = vec![0; 16];
        request.raw_payload[..4].copy_from_slice(&1u32.to_le_bytes());
        request.raw_payload[4..8].copy_from_slice(&1u32.to_le_bytes());
        request.buffers.push(IpcBuffer {
            address: 0x8000,
            capacity: 4,
            direction: IpcBufferDirection::Out,
            data: vec![],
        });
        assert_eq!(
            catalog.dispatch(1, &request).unwrap().buffer_writes[0].data,
            7u32.to_le_bytes()
        );
        request.buffers[0].capacity = 3;
        assert!(catalog.dispatch(1, &request).is_err());
        request.command_id = 99;
        assert!(catalog.dispatch(1, &request).is_err());
    }
}
