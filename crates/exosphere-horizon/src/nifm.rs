// SPDX-License-Identifier: GPL-3.0-or-later
//! Network-interface factories. Creating an object does not establish connectivity.

use crate::{
    HorizonProfileId,
    error::{HorizonError, HorizonResult},
    ipc::{HorizonService, ReturnedObject, ServiceRequest, ServiceResponse},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NifmObject {
    General,
}

impl NifmObject {
    pub fn name(self) -> &'static str {
        "nifm::IGeneralService"
    }

    pub fn dispatch(self, request: &ServiceRequest) -> HorizonResult<ServiceResponse> {
        // Profiles, requests, connectivity and host networking need their own
        // policy/backend. Never report connection success merely to advance boot.
        Err(HorizonError::UnknownCommand {
            service: self.name().into(),
            command_id: request.command_id,
        })
    }
}

pub struct NifmUserService;
impl HorizonService for NifmUserService {
    fn name(&self) -> &'static str {
        "nifm:u"
    }
    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        match request.command_id {
            4 => {} // Legacy CreateGeneralService has no scalar arguments.
            5 => {
                // The transport validates the PID descriptor. Its transmitted
                // value is not used as authority for process identity.
                request.raw_payload.get(..8).ok_or_else(|| {
                    HorizonError::IpcProtocol(
                        "nifm CreateGeneralService requires a reserved u64".into(),
                    )
                })?;
            }
            command_id => {
                return Err(HorizonError::UnknownCommand {
                    service: self.name().into(),
                    command_id,
                });
            }
        }
        let mut response = ServiceResponse::success();
        response
            .objects
            .push(ReturnedObject::Nifm(NifmObject::General));
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn factories_return_typed_objects_without_claiming_connectivity() {
        let mut service = NifmUserService;
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "nifm:u".into(),
            command_id: 4,
            values: Vec::new(),
            raw_payload: Vec::new(),
            buffers: Vec::new(),
        };
        assert_eq!(
            service.call(&profile, &request).unwrap().objects,
            vec![ReturnedObject::Nifm(NifmObject::General)]
        );
        request.command_id = 5;
        assert!(service.call(&profile, &request).is_err());
        request.raw_payload = 0u64.to_le_bytes().to_vec();
        assert_eq!(
            service.call(&profile, &request).unwrap().objects,
            vec![ReturnedObject::Nifm(NifmObject::General)]
        );
        request.command_id = 0;
        assert!(service.call(&profile, &request).is_err());
        assert!(NifmObject::General.dispatch(&request).is_err());
    }
}
