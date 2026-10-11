// SPDX-License-Identifier: GPL-3.0-or-later
//! Parental-control session initialization. Unimplemented permission checks fail explicitly.

use crate::{
    HorizonProfileId, HorizonService,
    error::{HorizonError, HorizonResult},
    ipc::{ReturnedObject, ServiceRequest, ServiceResponse},
};

#[derive(Debug)]
pub struct ParentalService;

impl HorizonService for ParentalService {
    fn name(&self) -> &'static str {
        "pctl:a"
    }

    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        if !matches!(request.command_id, 0 | 1) {
            return Err(unknown("pctl:a", request.command_id));
        }
        if request.raw_payload.get(..8) != Some(&[0; 8]) {
            return Err(HorizonError::IpcProtocol(
                "CreateService requires a zero reserved u64".into(),
            ));
        }
        // PID descriptor presence is checked by the transport. Session ownership
        // comes from the kernel caller, never the guest-supplied PID word.
        let mut response = ServiceResponse::success();
        response.objects.push(ReturnedObject::ParentalSession {
            initialized: request.command_id == 0,
        });
        Ok(response)
    }
}

pub fn dispatch(
    initialized: &mut bool,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    match request.command_id {
        1 => *initialized = true,
        command_id => return Err(unknown("IParentalControlService", command_id)),
    }
    Ok(ServiceResponse::success())
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
    #[test]
    fn factory_distinguishes_initialized_and_uninitialized_sessions() {
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "pctl:a".into(),
            command_id: 0,
            raw_payload: vec![0; 8],
            values: vec![],
            buffers: vec![],
        };
        for (command, expected) in [(0, true), (1, false)] {
            request.command_id = command;
            assert_eq!(
                ParentalService.call(&profile, &request).unwrap().objects,
                vec![ReturnedObject::ParentalSession {
                    initialized: expected
                }]
            );
        }
        request.raw_payload = vec![0; 7];
        assert!(ParentalService.call(&profile, &request).is_err());
        let mut initialized = false;
        dispatch(&mut initialized, &request).unwrap();
        assert!(initialized);
        request.command_id = 1001;
        assert!(dispatch(&mut initialized, &request).is_err());
    }
}
