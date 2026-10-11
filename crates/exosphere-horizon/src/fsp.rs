// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Initial `fsp-srv` (`nn::fssrv::sf::IFileSystemProxy`) service surface.
//!
//! Returned filesystem objects are materialized by the kernel as native sessions
//! or domain children. This service never fabricates a mounted storage backend.

use crate::{
    HorizonProfileId,
    error::{HorizonError, HorizonResult},
    ipc::{HorizonService, ReturnedObject, ServiceRequest, ServiceResponse},
};

pub mod command {
    pub const SET_CURRENT_PROCESS: u32 = 1;
    pub const OPEN_DATA_FS_BY_CURRENT_PROCESS: u32 = 2;
    pub const OPEN_HOST_FS: u32 = 17;
    pub const OPEN_SD_CARD_FS: u32 = 18;
    pub const CREATE_SAVE_DATA_FS: u32 = 22;
    pub const IS_EXFAT_SUPPORTED: u32 = 27;
    pub const OPEN_SAVE_DATA_FS: u32 = 51;
    pub const OPEN_DATA_STORAGE_BY_CURRENT_PROCESS: u32 = 200;
    pub const GET_GLOBAL_ACCESS_LOG_MODE: u32 = 1005;
}

#[derive(Debug, Default)]
pub struct FspSrvService {
    current_process: Option<u64>,
}
impl FspSrvService {
    pub fn current_process(&self) -> Option<u64> {
        self.current_process
    }
}

impl HorizonService for FspSrvService {
    fn name(&self) -> &'static str {
        "fsp-srv"
    }
    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        match request.command_id {
            command::GET_GLOBAL_ACCESS_LOG_MODE => {
                // Access logging is disabled in the current host policy: neither
                // a debug nor an SD-card access-log sink is installed. Keep setters
                // unsupported until enabling a mode actually connects its sink.
                let mut response = ServiceResponse::success();
                response.raw_payload = 0u32.to_le_bytes().to_vec();
                Ok(response)
            }
            command::SET_CURRENT_PROCESS => {
                let bytes = request.raw_payload.get(..8).ok_or_else(|| {
                    HorizonError::IpcProtocol(
                        "fsp-srv:SetCurrentProcess requires u64 ProcessId".into(),
                    )
                })?;
                self.current_process = Some(u64::from_le_bytes(
                    bytes.try_into().expect("length checked"),
                ));
                Ok(ServiceResponse::success())
            }
            command::IS_EXFAT_SUPPORTED => {
                // Exosphere's VFS does not expose the host filesystem format to the guest. Report
                // support because SD semantics are implemented by the VFS rather than by FAT/exFAT.
                let mut response = ServiceResponse::success();
                response.raw_payload.push(1);
                Ok(response)
            }
            command::OPEN_DATA_FS_BY_CURRENT_PROCESS => {
                let mut response = ServiceResponse::success();
                response.objects.push(ReturnedObject::FileSystem {
                    mount: "rom".into(),
                });
                Ok(response)
            }
            command::OPEN_HOST_FS => {
                let mut response = ServiceResponse::success();
                response.objects.push(ReturnedObject::FileSystem {
                    mount: "host".into(),
                });
                Ok(response)
            }
            command::OPEN_SD_CARD_FS => {
                let mut response = ServiceResponse::success();
                response.objects.push(ReturnedObject::FileSystem {
                    mount: "sdmc".into(),
                });
                Ok(response)
            }
            command::CREATE_SAVE_DATA_FS => {
                // The save backend is provisioned by Exosphere's mount/save manager. Creation is
                // idempotent at this service layer; opening the save returns the actual IFileSystem.
                Ok(ServiceResponse::success())
            }
            command::OPEN_SAVE_DATA_FS => {
                let mut response = ServiceResponse::success();
                response.objects.push(ReturnedObject::FileSystem {
                    mount: "save".into(),
                });
                Ok(response)
            }
            command::OPEN_DATA_STORAGE_BY_CURRENT_PROCESS => {
                // The loader mounts the title data image under this host-neutral storage name.
                // Returning an object description keeps handle allocation process-local in the kernel.
                let mut response = ServiceResponse::success();
                response.objects.push(ReturnedObject::Storage {
                    name: "rom-data".into(),
                    writable: false,
                });
                Ok(response)
            }
            id => Err(HorizonError::UnknownCommand {
                service: self.name().to_owned(),
                command_id: id,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_log_query_reports_disabled_policy_as_exact_u32() {
        let mut service = FspSrvService::default();
        let profile = HorizonProfileId("22.1.0".into());
        let mut request = ServiceRequest {
            service: "fsp-srv".into(),
            command_id: command::GET_GLOBAL_ACCESS_LOG_MODE,
            values: Vec::new(),
            raw_payload: Vec::new(),
            buffers: Vec::new(),
        };
        let response = service.call(&profile, &request).unwrap();
        assert_eq!(response.result_code, 0);
        assert_eq!(response.raw_payload, 0u32.to_le_bytes());
        assert!(response.objects.is_empty());
        // Unsupported enabling/output commands cannot claim that logs were written.
        for command_id in [1004, 1006] {
            request.command_id = command_id;
            assert!(matches!(
                service.call(&profile, &request),
                Err(HorizonError::UnknownCommand { .. })
            ));
        }
        assert_eq!(service.current_process(), None);
    }
}
