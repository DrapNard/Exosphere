// SPDX-License-Identifier: GPL-3.0-or-later
//! Guest performance policy. This service never changes host CPU/GPU clocks.

use std::sync::{Arc, Mutex};

use crate::{
    HorizonProfileId, HorizonService,
    error::{HorizonError, HorizonResult},
    ipc::{ReturnedObject, ServiceRequest, ServiceResponse},
};

#[derive(Debug)]
pub struct PerformanceState {
    configurations: [u32; 2],
    overclock_enabled: bool,
}

impl Default for PerformanceState {
    fn default() -> Self {
        // Explicit handheld/docked diagnostic policy, not measured host clock rates.
        Self {
            configurations: [0x00020003, 0x00010001],
            overclock_enabled: false,
        }
    }
}

pub type SharedPerformanceState = Arc<Mutex<PerformanceState>>;

#[derive(Debug)]
pub struct ApmService(pub SharedPerformanceState);

fn word(request: &ServiceRequest, offset: usize) -> HorizonResult<u32> {
    let bytes = request
        .raw_payload
        .get(offset..offset + 4)
        .ok_or_else(|| HorizonError::IpcProtocol("APM requires a u32 argument".into()))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn mode(request: &ServiceRequest) -> HorizonResult<usize> {
    match word(request, 0)? {
        value @ 0..=1 => Ok(value as usize),
        _ => Err(HorizonError::IpcProtocol(
            "invalid APM performance mode".into(),
        )),
    }
}

impl HorizonService for ApmService {
    fn name(&self) -> &'static str {
        "apm"
    }

    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let mut response = ServiceResponse::success();
        match request.command_id {
            0 => response.objects.push(ReturnedObject::ApmSession),
            // The diagnostic machine is handheld. Docking/event transitions are separate work.
            1 => response.raw_payload = 0u32.to_le_bytes().to_vec(),
            6 => {
                let state = self
                    .0
                    .lock()
                    .map_err(|_| HorizonError::InvalidState("APM state lock poisoned".into()))?;
                response.raw_payload = vec![u8::from(state.overclock_enabled)];
            }
            command_id => {
                return Err(HorizonError::UnknownCommand {
                    service: "apm".into(),
                    command_id,
                });
            }
        }
        Ok(response)
    }
}

pub fn dispatch_session(
    shared: &SharedPerformanceState,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    let mut state = shared
        .lock()
        .map_err(|_| HorizonError::InvalidState("APM state lock poisoned".into()))?;
    let mut response = ServiceResponse::success();
    match request.command_id {
        0 => {
            let index = mode(request)?;
            let configuration = word(request, 4)?;
            // IDs are guest policy tokens. Firmware-specific availability/clock tables
            // are not synthesized here; the selected token is retained for readback.
            state.configurations[index] = configuration;
        }
        1 => response.raw_payload = state.configurations[mode(request)?].to_le_bytes().to_vec(),
        2 => {
            state.overclock_enabled = match request.raw_payload.first() {
                Some(0) => false,
                Some(1) => true,
                _ => return Err(HorizonError::IpcProtocol("APM requires a boolean".into())),
            };
        }
        command_id => {
            return Err(HorizonError::UnknownCommand {
                service: "IApmSession".into(),
                command_id,
            });
        }
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command_id: u32, raw_payload: Vec<u8>) -> ServiceRequest {
        ServiceRequest {
            service: "apm".into(),
            command_id,
            raw_payload,
            values: vec![],
            buffers: vec![],
        }
    }

    #[test]
    fn sessions_retain_independent_mode_settings_and_share_overclock_policy() {
        let shared = SharedPerformanceState::default();
        let mut root = ApmService(shared.clone());
        let profile = HorizonProfileId("22.1.0".into());
        assert_eq!(
            root.call(&profile, &request(0, vec![])).unwrap().objects,
            vec![ReturnedObject::ApmSession]
        );
        let mut payload = 1u32.to_le_bytes().to_vec();
        payload.extend(0x20000u32.to_le_bytes());
        dispatch_session(&shared, &request(0, payload)).unwrap();
        assert_eq!(
            dispatch_session(&shared, &request(1, 1u32.to_le_bytes().to_vec()))
                .unwrap()
                .raw_payload,
            0x20000u32.to_le_bytes()
        );
        assert_eq!(
            dispatch_session(&shared, &request(1, 0u32.to_le_bytes().to_vec()))
                .unwrap()
                .raw_payload,
            0x20003u32.to_le_bytes()
        );
        dispatch_session(&shared, &request(2, vec![1])).unwrap();
        assert_eq!(
            root.call(&profile, &request(6, vec![]))
                .unwrap()
                .raw_payload,
            vec![1]
        );
        assert!(dispatch_session(&shared, &request(2, vec![2])).is_err());
        assert!(dispatch_session(&shared, &request(0, vec![0; 4])).is_err());
        assert!(dispatch_session(&shared, &request(1, 2u32.to_le_bytes().to_vec())).is_err());
        assert!(dispatch_session(&shared, &request(99, vec![])).is_err());
    }
}
