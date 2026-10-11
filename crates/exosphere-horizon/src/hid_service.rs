// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed HID resource factories, separate from normalized host input state.

use crate::{
    HorizonProfileId,
    error::{HorizonError, HorizonResult},
    ipc::{HorizonService, ReturnedObject, ServiceRequest, ServiceResponse},
};

/// Host-dispatch staging capacity for HID pointer descriptors, in bytes.
pub(crate) const POINTER_BUFFER_SIZE: u16 = 0x400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidObject {
    AppletResource { resource_id: u64 },
}

impl HidObject {
    pub fn name(self) -> &'static str {
        "hid::IAppletResource"
    }

    pub fn dispatch(self, request: &ServiceRequest) -> HorizonResult<ServiceResponse> {
        if request.command_id == 0 {
            let Self::AppletResource { resource_id } = self;
            let mut response = ServiceResponse::success();
            response
                .objects
                .push(ReturnedObject::HidSharedMemory { resource_id });
            return Ok(response);
        }
        Err(HorizonError::UnknownCommand {
            service: self.name().into(),
            command_id: request.command_id,
        })
    }
}

/// Application input policy; activation alone does not connect a controller.
#[derive(Debug, Default)]
pub struct HidService {
    active_npad_resources: std::collections::BTreeSet<u64>,
    active_debug_pad_resources: std::collections::BTreeSet<u64>,
    supported_styles: std::collections::BTreeMap<u64, u32>,
    supported_ids: std::collections::BTreeMap<u64, Vec<u32>>,
    style_events: std::collections::BTreeMap<(u64, u32), crate::event::SharedEvent>,
    published_primary_styles: std::collections::BTreeMap<u64, bool>,
}

impl HidService {
    fn primary_full_key_allowed(&self, resource_id: u64) -> bool {
        self.npad_active(resource_id)
            && self
                .supported_styles
                .get(&resource_id)
                .is_some_and(|styles| styles & 1 != 0)
            && self
                .supported_ids
                .get(&resource_id)
                .is_some_and(|ids| ids.contains(&0))
    }
    pub fn npad_active(&self, resource_id: u64) -> bool {
        self.active_npad_resources.contains(&resource_id)
    }

    pub fn debug_pad_active(&self, resource_id: u64) -> bool {
        self.active_debug_pad_resources.contains(&resource_id)
    }
}

/// Shared policy is the same state mutated by native HID IPC, not a copied
/// configuration that can drift from what the application requested.
#[derive(Debug, Default, Clone)]
pub(crate) struct SharedHidService(std::sync::Arc<std::sync::Mutex<HidService>>);

impl SharedHidService {
    pub(crate) fn primary_publication_completed(
        &self,
        resource_id: u64,
        connected: bool,
    ) -> HorizonResult<()> {
        let mut service = self
            .0
            .lock()
            .map_err(|_| HorizonError::InvalidState("HID policy lock poisoned".into()))?;
        let previous = service
            .published_primary_styles
            .insert(resource_id, connected)
            .unwrap_or(false);
        if previous != connected {
            if let Some(event) = service.style_events.get(&(resource_id, 0)) {
                event.set_signaled(true);
            }
        }
        Ok(())
    }
    pub(crate) fn primary_full_key_allowed(&self, resource_id: u64) -> HorizonResult<bool> {
        let service = self
            .0
            .lock()
            .map_err(|_| HorizonError::InvalidState("HID policy lock poisoned".into()))?;
        Ok(service.primary_full_key_allowed(resource_id))
    }
}

impl HorizonService for SharedHidService {
    fn name(&self) -> &'static str {
        "hid"
    }

    fn call(
        &mut self,
        profile: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        self.0
            .lock()
            .map_err(|_| HorizonError::InvalidState("HID policy lock poisoned".into()))?
            .call(profile, request)
    }
}

impl HorizonService for HidService {
    fn name(&self) -> &'static str {
        "hid"
    }

    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        if !matches!(request.command_id, 0 | 1 | 100 | 101 | 102 | 103 | 106) {
            return Err(HorizonError::UnknownCommand {
                service: self.name().into(),
                command_id: request.command_id,
            });
        }
        // SetSupportedNpadStyleSet places a u32 plus alignment padding before Aruid.
        let identity_offset = if matches!(request.command_id, 100 | 106) {
            8
        } else {
            0
        };
        let bytes = request
            .raw_payload
            .get(identity_offset..identity_offset + 8)
            .ok_or_else(|| {
                HorizonError::IpcProtocol("HID resource identity is truncated".into())
            })?;
        // The kernel validates this identity against its caller and requires a
        // PID descriptor before invoking the factory in native/domain transport.
        let resource_id = u64::from_le_bytes(bytes.try_into().unwrap());
        if request.command_id == 106 {
            if request.raw_payload.len() < 24 || !request.buffers.is_empty() {
                return Err(HorizonError::IpcProtocol(
                    "invalid HID style-event arguments".into(),
                ));
            }
            let id = u32::from_le_bytes(request.raw_payload[..4].try_into().unwrap());
            if !matches!(id, 0..=7 | 0x10 | 0x20) {
                return Err(HorizonError::InvalidState(
                    "unsupported HID controller ID".into(),
                ));
            }
            // The trailing client pointer is protocol metadata, never dereferenced.
            let event = self
                .style_events
                .entry((resource_id, id))
                .or_default()
                .clone();
            let mut response = ServiceResponse::success();
            response.objects.push(ReturnedObject::ReadableEvent(event));
            return Ok(response);
        }
        if request.command_id == 102 {
            let [buffer] = request.buffers.as_slice() else {
                return Err(HorizonError::IpcProtocol(
                    "HID controller IDs require one input buffer".into(),
                ));
            };
            if buffer.direction != crate::ipc::IpcBufferDirection::In
                || buffer.capacity != buffer.data.len() as u64
                || buffer.data.len() % 4 != 0
                || buffer.data.len() > 40
            {
                return Err(HorizonError::IpcProtocol(
                    "invalid HID controller-ID buffer".into(),
                ));
            }
            let mut ids = Vec::new();
            for bytes in buffer.data.chunks_exact(4) {
                let id = u32::from_le_bytes(bytes.try_into().unwrap());
                if !matches!(id, 0..=7 | 0x10 | 0x20) {
                    return Err(HorizonError::InvalidState(
                        "unsupported HID controller ID".into(),
                    ));
                }
                ids.push(id);
            }
            // Commit only after the entire list has passed validation.
            self.supported_ids.insert(resource_id, ids);
            return Ok(ServiceResponse::success());
        }
        if request.command_id == 100 {
            let styles = u32::from_le_bytes(request.raw_payload[..4].try_into().unwrap());
            self.supported_styles.insert(resource_id, styles);
            return Ok(ServiceResponse::success());
        }
        if request.command_id == 101 {
            let styles = self
                .supported_styles
                .get(&resource_id)
                .copied()
                .unwrap_or(0);
            let mut response = ServiceResponse::success();
            response.raw_payload = styles.to_le_bytes().to_vec();
            return Ok(response);
        }
        if request.command_id == 1 {
            self.active_debug_pad_resources.insert(resource_id);
            return Ok(ServiceResponse::success());
        }
        if request.command_id == 103 {
            self.active_npad_resources.insert(resource_id);
            return Ok(ServiceResponse::success());
        }
        let mut response = ServiceResponse::success();
        response
            .objects
            .push(ReturnedObject::Hid(HidObject::AppletResource {
                resource_id,
            }));
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_event_is_retained_scoped_and_signaled_only_after_style_changes() {
        let mut service = SharedHidService::default();
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "hid".into(),
            command_id: 106,
            values: Vec::new(),
            buffers: Vec::new(),
            raw_payload: [
                0u64.to_le_bytes(),
                7u64.to_le_bytes(),
                u64::MAX.to_le_bytes(),
            ]
            .concat(),
        };
        let get = |response: ServiceResponse| {
            let [ReturnedObject::ReadableEvent(event)] = response.objects.as_slice() else {
                panic!("HID style event must return a copied readable event");
            };
            event.clone()
        };
        let event = get(service.call(&profile, &request).unwrap());
        let alias = get(service.call(&profile, &request).unwrap());
        assert_eq!(event, alias);
        request.raw_payload[8..16].copy_from_slice(&8u64.to_le_bytes());
        let foreign = get(service.call(&profile, &request).unwrap());
        assert_ne!(event, foreign);
        service.primary_publication_completed(7, false).unwrap();
        assert!(!event.is_signaled());
        service.primary_publication_completed(7, true).unwrap();
        assert!(alias.is_signaled());
        assert!(!foreign.is_signaled());
        alias.set_signaled(false);
        service.primary_publication_completed(7, true).unwrap();
        assert!(!event.is_signaled());
        service.primary_publication_completed(7, false).unwrap();
        assert!(event.is_signaled());
        request.raw_payload.resize(16, 0);
        assert!(service.call(&profile, &request).is_err());
    }

    #[test]
    fn registry_updates_the_same_policy_used_by_publication() {
        let shared = SharedHidService::default();
        let mut registry = crate::ipc::ServiceRegistry::default();
        registry.register(shared.clone());
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "hid".into(),
            command_id: 103,
            values: Vec::new(),
            raw_payload: 7u64.to_le_bytes().to_vec(),
            buffers: Vec::new(),
        };
        registry.call(&profile, &request).unwrap();
        assert!(!shared.primary_full_key_allowed(7).unwrap());
        request.command_id = 100;
        request.raw_payload = [1u64.to_le_bytes(), 7u64.to_le_bytes()].concat();
        registry.call(&profile, &request).unwrap();
        assert!(!shared.primary_full_key_allowed(7).unwrap());
        request.command_id = 102;
        request.raw_payload = 7u64.to_le_bytes().to_vec();
        request.buffers = vec![crate::ipc::IpcBuffer {
            address: 0,
            direction: crate::ipc::IpcBufferDirection::In,
            capacity: 4,
            data: 0u32.to_le_bytes().to_vec(),
        }];
        registry.call(&profile, &request).unwrap();
        assert!(shared.primary_full_key_allowed(7).unwrap());
        assert!(!shared.primary_full_key_allowed(8).unwrap());
        request.buffers[0].data = 0xffu32.to_le_bytes().to_vec();
        assert!(registry.call(&profile, &request).is_err());
        assert!(shared.primary_full_key_allowed(7).unwrap());
        request.command_id = 100;
        request.buffers.clear();
        request.raw_payload = [2u64.to_le_bytes(), 7u64.to_le_bytes()].concat();
        registry.call(&profile, &request).unwrap();
        assert!(!shared.primary_full_key_allowed(7).unwrap());
    }

    #[test]
    fn resource_factory_retains_identity_without_claiming_input_publication() {
        let mut service = HidService::default();
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "hid".into(),
            command_id: 0,
            values: Vec::new(),
            raw_payload: 7u64.to_le_bytes().to_vec(),
            buffers: Vec::new(),
        };
        let object = HidObject::AppletResource { resource_id: 7 };
        assert_eq!(
            service.call(&profile, &request).unwrap().objects,
            [ReturnedObject::Hid(object)]
        );
        for size in 0..8 {
            request.raw_payload.resize(size, 0);
            assert!(service.call(&profile, &request).is_err());
        }
        request.command_id = 100;
        assert!(service.call(&profile, &request).is_err());
        request.command_id = 0;
        assert_eq!(
            object.dispatch(&request).unwrap().objects,
            [ReturnedObject::HidSharedMemory { resource_id: 7 }]
        );
    }

    #[test]
    fn npad_activation_is_application_scoped_and_rejects_truncated_arguments() {
        let mut service = HidService::default();
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "hid".into(),
            command_id: 103,
            values: Vec::new(),
            raw_payload: 7u64.to_le_bytes().to_vec(),
            buffers: Vec::new(),
        };
        for length in 0..8 {
            request.raw_payload = 7u64.to_le_bytes()[..length].to_vec();
            assert!(service.call(&profile, &request).is_err());
            assert!(!service.npad_active(7));
        }
        request.raw_payload = 7u64.to_le_bytes().to_vec();
        for _ in 0..2 {
            let response = service.call(&profile, &request).unwrap();
            assert_eq!(response.result_code, 0);
            assert!(response.objects.is_empty());
        }
        assert!(service.npad_active(7));
        assert!(!service.npad_active(8));
    }

    #[test]
    fn debug_pad_activation_does_not_activate_npad_or_other_applications() {
        let mut service = HidService::default();
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "hid".into(),
            command_id: 1,
            values: Vec::new(),
            raw_payload: Vec::new(),
            buffers: Vec::new(),
        };
        for length in 0..8 {
            request.raw_payload = 9u64.to_le_bytes()[..length].to_vec();
            assert!(service.call(&profile, &request).is_err());
            assert!(!service.debug_pad_active(9));
        }
        request.raw_payload = 9u64.to_le_bytes().to_vec();
        for _ in 0..2 {
            assert_eq!(service.call(&profile, &request).unwrap().result_code, 0);
        }
        assert!(service.debug_pad_active(9));
        assert!(!service.debug_pad_active(10));
        assert!(!service.npad_active(9));
    }

    #[test]
    fn supported_style_policy_roundtrips_without_connecting_devices() {
        let mut service = HidService::default();
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "hid".into(),
            command_id: 100,
            values: Vec::new(),
            raw_payload: Vec::new(),
            buffers: Vec::new(),
        };
        let mut payload = 0x8000001fu32.to_le_bytes().to_vec();
        payload.extend_from_slice(&0x12345678u32.to_le_bytes());
        payload.extend_from_slice(&7u64.to_le_bytes());
        for length in 0..16 {
            request.raw_payload = payload[..length].to_vec();
            assert!(service.call(&profile, &request).is_err());
            assert!(service.supported_styles.is_empty());
        }
        request.raw_payload = payload;
        service.call(&profile, &request).unwrap();
        request.command_id = 101;
        request.raw_payload = 7u64.to_le_bytes().to_vec();
        assert_eq!(
            service.call(&profile, &request).unwrap().raw_payload,
            0x8000001fu32.to_le_bytes()
        );
        request.raw_payload = 8u64.to_le_bytes().to_vec();
        assert_eq!(
            service.call(&profile, &request).unwrap().raw_payload,
            0u32.to_le_bytes()
        );
        assert!(!service.npad_active(7));
    }

    #[test]
    fn controller_ids_commit_only_complete_valid_input_lists() {
        use crate::ipc::{IpcBuffer, IpcBufferDirection};
        let mut service = HidService::default();
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "hid".into(),
            command_id: 102,
            values: Vec::new(),
            raw_payload: 7u64.to_le_bytes().to_vec(),
            buffers: Vec::new(),
        };
        let ids = [0u32, 7, 0x10, 0x20];
        let valid = IpcBuffer {
            address: 0,
            capacity: 16,
            direction: IpcBufferDirection::In,
            data: ids.into_iter().flat_map(u32::to_le_bytes).collect(),
        };
        request.buffers = vec![valid.clone()];
        service.call(&profile, &request).unwrap();
        assert_eq!(service.supported_ids[&7], ids);
        for invalid in [
            IpcBuffer {
                direction: IpcBufferDirection::Out,
                ..valid.clone()
            },
            IpcBuffer {
                capacity: 15,
                ..valid.clone()
            },
            IpcBuffer {
                capacity: 1,
                data: vec![0],
                ..valid.clone()
            },
            IpcBuffer {
                capacity: 44,
                data: vec![0; 44],
                ..valid.clone()
            },
            IpcBuffer {
                capacity: 4,
                data: 0x21u32.to_le_bytes().to_vec(),
                ..valid.clone()
            },
        ] {
            request.buffers = vec![invalid];
            assert!(service.call(&profile, &request).is_err());
            assert_eq!(service.supported_ids[&7], ids);
        }
        request.raw_payload = 8u64.to_le_bytes().to_vec();
        request.buffers = vec![IpcBuffer {
            capacity: 0,
            data: Vec::new(),
            ..valid
        }];
        service.call(&profile, &request).unwrap();
        assert!(service.supported_ids[&8].is_empty());
        assert_eq!(service.supported_ids[&7], ids);
    }
}
