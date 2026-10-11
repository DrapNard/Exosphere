// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed VI bootstrap and display metadata. Binder/presentation remain separate.

use crate::{
    HorizonProfileId,
    display::{DisplayId, DisplayManager},
    error::{HorizonError, HorizonResult},
    ipc::{HorizonService, ReturnedObject, ServiceRequest, ServiceResponse},
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Privilege {
    Application,
    System,
    Manager,
}

pub struct ViService(pub Privilege);
impl HorizonService for ViService {
    fn name(&self) -> &'static str {
        match self.0 {
            Privilege::Application => "vi:u",
            Privilege::System => "vi:s",
            Privilege::Manager => "vi:m",
        }
    }
    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let command = match self.0 {
            Privilege::Application => 0,
            Privilege::System => 1,
            Privilege::Manager => 2,
        };
        if request.command_id != command {
            return Err(unknown(self.name(), request.command_id));
        }
        let policy = u32::from_le_bytes(argument(request, 4)?.try_into().unwrap());
        if policy > 1 || (self.0 == Privilege::Application && policy != 0) {
            return Err(HorizonError::IpcProtocol(
                "invalid VI display-service policy".into(),
            ));
        }
        let mut response = ServiceResponse::success();
        response.objects.push(ReturnedObject::Vi(ViSession::new(
            ViKind::Application,
            self.0,
        )));
        Ok(response)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViKind {
    Application,
    System,
    Manager,
    BinderRelay,
    BinderIndirect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViSession {
    pub kind: ViKind,
    pub privilege: Privilege,
    opened: BTreeMap<DisplayId, u32>,
}
impl ViSession {
    pub fn new(kind: ViKind, privilege: Privilege) -> Self {
        Self {
            kind,
            privilege,
            opened: BTreeMap::new(),
        }
    }
    pub fn name(&self) -> &'static str {
        match self.kind {
            ViKind::Application => "IApplicationDisplayService",
            ViKind::System => "ISystemDisplayService",
            ViKind::Manager => "IManagerDisplayService",
            ViKind::BinderRelay => "IHOSBinderDriverRelay",
            ViKind::BinderIndirect => "IHOSBinderDriverIndirect",
        }
    }
    pub fn dispatch_owned(
        &mut self,
        display: &mut DisplayManager,
        owner: u64,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        if self.kind == ViKind::Application && request.command_id == 2101 {
            if !request.buffers.is_empty() {
                return Err(HorizonError::IpcProtocol(
                    "SetLayerScalingMode does not accept buffers".into(),
                ));
            }
            // u32 mode, four-byte ABI padding, then the full u64 layer identity.
            let args = argument(request, 16)?;
            let mode = crate::display::LayerScalingMode::try_from(u32::from_le_bytes(
                args[..4].try_into().unwrap(),
            ))?;
            let layer = u64::from_le_bytes(args[8..16].try_into().unwrap());
            display.set_managed_layer_scaling(layer, owner, mode)?;
            return Ok(ServiceResponse::success());
        }
        if self.kind == ViKind::BinderRelay && matches!(request.command_id, 0 | 3) {
            return crate::binder::transact(display, owner, request);
        }
        if self.kind == ViKind::BinderRelay && request.command_id == 2 {
            if !request.buffers.is_empty() {
                return Err(HorizonError::IpcProtocol(
                    "GetNativeHandle does not accept buffers".into(),
                ));
            }
            let args = argument(request, 8)?;
            let id = i32::from_le_bytes(args[..4].try_into().unwrap());
            let selector = u32::from_le_bytes(args[4..8].try_into().unwrap());
            if selector != 0x0f {
                return Err(HorizonError::IpcProtocol(
                    "unsupported producer native handle type".into(),
                ));
            }
            let event = display
                .producers
                .get_mut(id, owner)?
                .queue
                .availability_event();
            let mut response = ServiceResponse::success();
            response.objects.push(ReturnedObject::ReadableEvent(event));
            return Ok(response);
        }
        if self.kind == ViKind::BinderRelay && request.command_id == 1 {
            if !request.buffers.is_empty() {
                return Err(HorizonError::IpcProtocol(
                    "AdjustRefcount does not accept buffers".into(),
                ));
            }
            let args = argument(request, 12)?;
            let id = i32::from_le_bytes(args[..4].try_into().unwrap());
            let delta = i32::from_le_bytes(args[4..8].try_into().unwrap());
            let kind = match u32::from_le_bytes(args[8..12].try_into().unwrap()) {
                0 => crate::graphic_producer::ReferenceKind::Weak,
                1 => crate::graphic_producer::ReferenceKind::Strong,
                _ => {
                    return Err(HorizonError::IpcProtocol(
                        "invalid Binder reference kind".into(),
                    ));
                }
            };
            display.producers.adjust_reference(id, owner, kind, delta)?;
            return Ok(ServiceResponse::success());
        }
        if self.kind != ViKind::Application || request.command_id != 2020 {
            return self.dispatch(display, request);
        }
        let args = argument(request, 80)?;
        let end = args[..64]
            .iter()
            .position(|&byte| byte == 0)
            .ok_or_else(|| HorizonError::IpcProtocol("VI display name is not terminated".into()))?;
        let name = std::str::from_utf8(&args[..end])
            .map_err(|_| HorizonError::IpcProtocol("invalid VI display name".into()))?;
        let layer = u64::from_le_bytes(args[64..72].try_into().unwrap());
        let aruid = u64::from_le_bytes(args[72..80].try_into().unwrap());
        if aruid != owner {
            return Err(HorizonError::PermissionDenied);
        }
        let layer_info = display
            .managed_layer(layer, owner)
            .ok_or_else(|| HorizonError::InvalidState("unknown or foreign managed layer".into()))?;
        let info = display
            .display(layer_info.display)
            .ok_or_else(|| HorizonError::InvalidState("layer display does not exist".into()))?;
        if info.name != name || !self.opened.contains_key(&info.id) {
            return Err(HorizonError::InvalidState(
                "layer display is not open on this session".into(),
            ));
        }
        let [buffer] = request.buffers.as_slice() else {
            return Err(HorizonError::IpcProtocol(
                "OpenLayer requires one output buffer".into(),
            ));
        };
        if buffer.direction != crate::ipc::IpcBufferDirection::Out
            || buffer.capacity < crate::graphic_producer::NATIVE_WINDOW_SIZE as u64
        {
            return Err(HorizonError::IpcProtocol(
                "OpenLayer native window output is too small or not writable".into(),
            ));
        }
        let bytes = display.open_managed_native_window(layer, owner)?;
        let mut response = ServiceResponse::success();
        response.raw_payload = (bytes.len() as u64).to_le_bytes().to_vec();
        response.buffer_writes.push(crate::ipc::IpcBufferWrite {
            buffer_index: 0,
            data: bytes,
        });
        Ok(response)
    }

    pub fn dispatch(
        &mut self,
        display: &DisplayManager,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        if self.kind != ViKind::Application {
            return Err(unknown(self.name(), request.command_id));
        }
        let mut response = ServiceResponse::success();
        match request.command_id {
            100..=103 => {
                let (kind, required) = match request.command_id {
                    100 => (ViKind::BinderRelay, Privilege::Application),
                    101 => (ViKind::System, Privilege::System),
                    102 => (ViKind::Manager, Privilege::Manager),
                    103 => (ViKind::BinderIndirect, Privilege::System),
                    _ => unreachable!(),
                };
                if self.privilege < required {
                    return Err(HorizonError::PermissionDenied);
                }
                response
                    .objects
                    .push(ReturnedObject::Vi(Self::new(kind, self.privilege)));
            }
            1010 => {
                let bytes = argument(request, 64)?;
                let end = bytes.iter().position(|byte| *byte == 0).ok_or_else(|| {
                    HorizonError::IpcProtocol("VI display name is not terminated".into())
                })?;
                let name = std::str::from_utf8(&bytes[..end])
                    .map_err(|_| HorizonError::IpcProtocol("invalid VI display name".into()))?;
                let info = display
                    .displays()
                    .find(|info| info.name == name)
                    .ok_or_else(|| HorizonError::InvalidState("display does not exist".into()))?;
                let count = self
                    .opened
                    .get(&info.id)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| {
                        HorizonError::InvalidState("display open count exhausted".into())
                    })?;
                self.opened.insert(info.id, count);
                response
                    .raw_payload
                    .extend_from_slice(&info.id.0.to_le_bytes());
            }
            1020 | 1102 => {
                let id = DisplayId(u64::from_le_bytes(
                    argument(request, 8)?.try_into().unwrap(),
                ));
                let count = self.opened.get(&id).copied().ok_or_else(|| {
                    HorizonError::InvalidState("display is not open on this session".into())
                })?;
                if request.command_id == 1020 {
                    if count == 1 {
                        self.opened.remove(&id);
                    } else {
                        self.opened.insert(id, count - 1);
                    }
                } else {
                    let info = display.display(id).ok_or_else(|| {
                        HorizonError::InvalidState("display does not exist".into())
                    })?;
                    // Guest resolution fields are signed 64-bit values, not the
                    // host-neutral display model's u32 dimensions.
                    response
                        .raw_payload
                        .extend_from_slice(&i64::from(info.width).to_le_bytes());
                    response
                        .raw_payload
                        .extend_from_slice(&i64::from(info.height).to_le_bytes());
                }
            }
            _ => return Err(unknown(self.name(), request.command_id)),
        }
        Ok(response)
    }
}

fn argument(request: &ServiceRequest, size: usize) -> HorizonResult<&[u8]> {
    request
        .raw_payload
        .get(..size)
        .ok_or_else(|| HorizonError::IpcProtocol("truncated VI arguments".into()))
}

#[cfg(test)]
mod layer_tests {
    use super::*;
    use crate::ipc::{IpcBuffer, IpcBufferDirection};

    #[test]
    fn scaling_policy_validates_owner_mode_and_full_layer_identity_before_mutation() {
        use crate::display::LayerScalingMode;
        let mut display = DisplayManager::default();
        let layer = display.create_managed_layer(7).unwrap();
        let mut session = ViSession::new(ViKind::Application, Privilege::Application);
        let mut request = ServiceRequest {
            service: session.name().into(),
            command_id: 2101,
            values: Vec::new(),
            raw_payload: [
                2u32.to_le_bytes().as_slice(),
                &[0xff; 4],
                &layer.id.to_le_bytes(),
            ]
            .concat(),
            buffers: Vec::new(),
        };
        assert!(session.dispatch_owned(&mut display, 8, &request).is_err());
        request.raw_payload.truncate(15);
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        request.raw_payload.push(0);
        for mode in [1u32, 3, 5, u32::MAX] {
            request.raw_payload[..4].copy_from_slice(&mode.to_le_bytes());
            assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
            assert_eq!(
                display.managed_layer(layer.id, 7).unwrap().scaling_mode,
                None
            );
        }
        request.raw_payload[..4].copy_from_slice(&2u32.to_le_bytes());
        request.raw_payload[8..16].copy_from_slice(&(layer.id | (1u64 << 32)).to_le_bytes());
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        request.raw_payload[8..16].copy_from_slice(&layer.id.to_le_bytes());
        request.buffers.push(IpcBuffer {
            address: 0,
            capacity: 0,
            direction: IpcBufferDirection::In,
            data: Vec::new(),
        });
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        request.buffers.clear();
        for (raw, mode) in [
            (2u32, LayerScalingMode::FitToLayer),
            (4, LayerScalingMode::PreserveAspectRatio),
            (0, LayerScalingMode::None),
        ] {
            request.raw_payload[..4].copy_from_slice(&raw.to_le_bytes());
            let reply = session.dispatch_owned(&mut display, 7, &request).unwrap();
            assert_eq!(reply.result_code, 0);
            assert!(reply.objects.is_empty() && reply.raw_payload.is_empty());
            let info = display.managed_layer(layer.id, 7).unwrap();
            assert_eq!(info.scaling_mode, Some(mode));
            assert_eq!((info.width, info.height), (layer.width, layer.height));
        }
        display.destroy_layer(layer.id);
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
    }

    #[test]
    fn native_handle_validates_selector_owner_and_returns_same_queue_event() {
        let mut display = DisplayManager::default();
        let id = display.producers.create(1, 7).unwrap();
        let expected = display
            .producers
            .get_mut(id, 7)
            .unwrap()
            .queue
            .availability_event();
        let mut session = ViSession::new(ViKind::BinderRelay, Privilege::Application);
        let mut request = ServiceRequest {
            service: session.name().into(),
            command_id: 2,
            values: Vec::new(),
            raw_payload: [id.to_le_bytes(), 0x0fu32.to_le_bytes()].concat(),
            buffers: Vec::new(),
        };
        assert!(session.dispatch_owned(&mut display, 8, &request).is_err());
        request.raw_payload.truncate(7);
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        request.raw_payload.push(0);
        request.raw_payload[4..8].fill(0);
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        request.raw_payload[4..8].copy_from_slice(&0x0fu32.to_le_bytes());
        for _ in 0..2 {
            let response = session.dispatch_owned(&mut display, 7, &request).unwrap();
            assert_eq!(
                response.objects,
                vec![ReturnedObject::ReadableEvent(expected.clone())]
            );
            assert!(response.raw_payload.is_empty());
        }
    }

    #[test]
    fn binder_reference_arguments_validate_before_mutating_owned_producer() {
        let mut display = DisplayManager::default();
        let id = display.producers.create(1, 7).unwrap();
        let mut session = ViSession::new(ViKind::BinderRelay, Privilege::Application);
        let mut request = ServiceRequest {
            service: session.name().into(),
            command_id: 1,
            values: Vec::new(),
            raw_payload: [id.to_le_bytes(), 1i32.to_le_bytes(), 0u32.to_le_bytes()].concat(),
            buffers: Vec::new(),
        };
        assert!(session.dispatch_owned(&mut display, 8, &request).is_err());
        request.raw_payload.truncate(11);
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        request.raw_payload.push(0);
        request.raw_payload[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        assert_eq!(
            display.producers.get_mut(id, 7).unwrap().reference_counts(),
            (0, 0)
        );
        request.raw_payload[8..12].fill(0);
        request.buffers.push(IpcBuffer {
            address: 0,
            capacity: 0,
            direction: IpcBufferDirection::In,
            data: Vec::new(),
        });
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        request.buffers.clear();
        let response = session.dispatch_owned(&mut display, 7, &request).unwrap();
        assert_eq!(response.result_code, 0);
        assert!(response.objects.is_empty() && response.raw_payload.is_empty());
        assert_eq!(
            display.producers.get_mut(id, 7).unwrap().reference_counts(),
            (1, 0)
        );
        request.command_id = 3;
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
    }

    #[test]
    fn open_layer_exports_actual_producer_after_validating_display_owner_and_capacity() {
        let mut display = DisplayManager::default();
        let layer = display.create_managed_layer(7).unwrap();
        let mut session = ViSession::new(ViKind::Application, Privilege::Application);
        let mut request = ServiceRequest {
            service: session.name().into(),
            command_id: 1010,
            values: Vec::new(),
            raw_payload: vec![0; 64],
            buffers: Vec::new(),
        };
        request.raw_payload[..7].copy_from_slice(b"Default");
        session.dispatch(&display, &request).unwrap();
        request.command_id = 2020;
        request
            .raw_payload
            .extend_from_slice(&layer.id.to_le_bytes());
        request.raw_payload.extend_from_slice(&7u64.to_le_bytes());
        request.buffers.push(IpcBuffer {
            address: 0x1000,
            capacity: crate::graphic_producer::NATIVE_WINDOW_SIZE as u64,
            direction: IpcBufferDirection::Out,
            data: Vec::new(),
        });
        assert!(session.dispatch_owned(&mut display, 8, &request).is_err());
        request.buffers[0].capacity = crate::graphic_producer::NATIVE_WINDOW_SIZE as u64 - 1;
        assert!(session.dispatch_owned(&mut display, 7, &request).is_err());
        request.buffers[0].capacity = crate::graphic_producer::NATIVE_WINDOW_SIZE as u64;
        let response = session.dispatch_owned(&mut display, 7, &request).unwrap();
        assert_eq!(
            response.raw_payload,
            (crate::graphic_producer::NATIVE_WINDOW_SIZE as u64).to_le_bytes()
        );
        let parcel = crate::parcel::Parcel::parse(&response.buffer_writes[0].data).unwrap();
        let id = i32::from_le_bytes(parcel.payload[8..12].try_into().unwrap());
        let producer = display.producers.get_mut(id, 7).unwrap();
        assert_eq!(producer.layer, layer.id);
        producer.queue.connect().unwrap();
        assert_eq!(producer.queue.dequeue().unwrap(), None);
    }
}
fn unknown(service: &str, command_id: u32) -> HorizonError {
    HorizonError::UnknownCommand {
        service: service.into(),
        command_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(command_id: u32, raw_payload: Vec<u8>) -> ServiceRequest {
        ServiceRequest {
            service: "vi:m".into(),
            command_id,
            raw_payload,
            values: vec![],
            buffers: vec![],
        }
    }
    #[test]
    fn factories_and_child_objects_enforce_service_privileges() {
        let profile = HorizonProfileId("22.1.0".into());
        let display = DisplayManager::default();
        for (privilege, command) in [
            (Privilege::Application, 0),
            (Privilege::System, 1),
            (Privilege::Manager, 2),
        ] {
            let mut factory = ViService(privilege);
            let response = factory
                .call(&profile, &request(command, 0u32.to_le_bytes().to_vec()))
                .unwrap();
            let ReturnedObject::Vi(mut application) = response.objects[0].clone() else {
                panic!("VI factory must return typed application service");
            };
            for (id, required) in [
                (100, Privilege::Application),
                (101, Privilege::System),
                (102, Privilege::Manager),
                (103, Privilege::System),
            ] {
                assert_eq!(
                    application.dispatch(&display, &request(id, vec![])).is_ok(),
                    privilege >= required
                );
            }
            assert!(factory.call(&profile, &request(command, vec![])).is_err());
            assert!(
                factory
                    .call(&profile, &request(command, 2u32.to_le_bytes().to_vec()))
                    .is_err()
            );
            assert!(factory.call(&profile, &request(99, vec![])).is_err());
        }
    }
    #[test]
    fn display_reads_use_existing_metadata_and_balanced_session_opens() {
        let display = DisplayManager::default();
        let mut application = ViSession::new(ViKind::Application, Privilege::Manager);
        let mut name = vec![0; 64];
        name[..7].copy_from_slice(b"Default");
        for _ in 0..2 {
            let reply = application
                .dispatch(&display, &request(1010, name.clone()))
                .unwrap();
            assert_eq!(reply.raw_payload, 1u64.to_le_bytes());
        }
        let id = 1u64.to_le_bytes().to_vec();
        let reply = application
            .dispatch(&display, &request(1102, id.clone()))
            .unwrap();
        assert_eq!(
            reply.raw_payload,
            [1280i64.to_le_bytes(), 720i64.to_le_bytes()].concat()
        );
        let before = application.clone();
        assert!(
            application
                .dispatch(&display, &request(1010, vec![b'X'; 64]))
                .is_err()
        );
        assert!(
            application
                .dispatch(&display, &request(1020, 99u64.to_le_bytes().to_vec()))
                .is_err()
        );
        assert_eq!(application, before);
        application
            .dispatch(&display, &request(1020, id.clone()))
            .unwrap();
        application
            .dispatch(&display, &request(1102, id.clone()))
            .unwrap();
        application
            .dispatch(&display, &request(1020, id.clone()))
            .unwrap();
        assert!(application.dispatch(&display, &request(1102, id)).is_err());
        assert!(
            ViSession::new(ViKind::BinderRelay, Privilege::Manager)
                .dispatch(&display, &request(0, vec![]))
                .is_err()
        );
    }
}
