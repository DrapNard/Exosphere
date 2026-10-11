// SPDX-License-Identifier: GPL-3.0-or-later
//! BSD client registration and monitoring ownership; sockets are not connected yet.

use crate::{
    HorizonProfileId,
    error::{HorizonError, HorizonResult},
    handle::KernelObject,
    ipc::{HorizonService, ServiceRequest, ServiceResponse},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};

pub struct BsdUserService;
impl HorizonService for BsdUserService {
    fn name(&self) -> &'static str {
        "bsd:u"
    }
    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        Err(unknown(request.command_id))
    }
}

#[derive(Debug)]
pub struct Registration {
    pub transfer: KernelObject,
    pub config: [u32; 8],
    pub process_id: u64,
}

#[derive(Debug, Clone, Default)]
pub struct BsdClient {
    /// Both registering and monitor endpoints retain the same client resources.
    pub registration: Option<Arc<Registration>>,
}

#[derive(Debug, Default)]
pub struct BsdManager {
    // Weak entries let the last session release resources without a separate
    // global client reference that would keep transfer memory locked forever.
    clients: BTreeMap<u64, Weak<Registration>>,
}

impl BsdManager {
    pub fn dispatch(
        &mut self,
        client: &mut BsdClient,
        request: &ServiceRequest,
        transfer: Option<KernelObject>,
        process_id: u64,
    ) -> HorizonResult<ServiceResponse> {
        if !matches!(request.command_id, 0 | 1) {
            return Err(unknown(request.command_id));
        }
        if client.registration.is_some() {
            return Err(HorizonError::InvalidState(
                "BSD endpoint already registered or monitoring".into(),
            ));
        }
        let mut response = ServiceResponse::success();
        let registration = if request.command_id == 0 {
            let raw = request.raw_payload.get(..48).ok_or_else(|| {
                HorizonError::IpcProtocol("BSD registration requires 48 bytes".into())
            })?;
            let mut config = [0u32; 8];
            for (value, bytes) in config.iter_mut().zip(raw[..32].chunks_exact(4)) {
                *value = u32::from_le_bytes(bytes.try_into().unwrap());
            }
            if config[0] != 1 {
                return Err(HorizonError::InvalidState(format!(
                    "unsupported BSD config version {}",
                    config[0]
                )));
            }
            let transfer = transfer.ok_or_else(|| {
                HorizonError::IpcProtocol(
                    "BSD registration requires validated transfer memory".into(),
                )
            })?;
            self.clients
                .retain(|_, registration| registration.strong_count() != 0);
            if self.clients.contains_key(&process_id) {
                return Err(HorizonError::InvalidState(
                    "BSD process already registered".into(),
                ));
            }
            let registration = Arc::new(Registration {
                transfer,
                config,
                process_id,
            });
            self.clients
                .insert(process_id, Arc::downgrade(&registration));
            // RegisterClient returns a u32 registration status, not a client PID.
            // The guest SDK aborts when this status is nonzero even if CMIF succeeds.
            response.raw_payload = 0u32.to_le_bytes().to_vec();
            registration
        } else {
            request.raw_payload.get(..8).ok_or_else(|| {
                HorizonError::IpcProtocol("BSD monitoring requires a reserved u64".into())
            })?;
            // The PID descriptor supplies caller identity through the kernel.
            // The scalar argument is reserved, not permission to monitor another process.
            self.clients
                .get(&process_id)
                .and_then(Weak::upgrade)
                .ok_or_else(|| HorizonError::InvalidState("BSD client is not registered".into()))?
        };
        client.registration = Some(registration);
        Ok(response)
    }
}

fn unknown(command_id: u32) -> HorizonError {
    HorizonError::UnknownCommand {
        service: "bsd:u".into(),
        command_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command_id: u32) -> ServiceRequest {
        let mut raw_payload = vec![0u8; 48];
        raw_payload[..4].copy_from_slice(&1u32.to_le_bytes());
        ServiceRequest {
            service: "bsd:u".into(),
            command_id,
            values: vec![],
            raw_payload,
            buffers: vec![],
        }
    }

    #[test]
    fn registration_monitoring_and_last_reference_have_real_ownership() {
        let mut manager = BsdManager::default();
        let mut client = BsdClient::default();
        let transfer = KernelObject::TransferMemory {
            owner: 7,
            base: 0x8000,
            size: 0x1000,
        };
        let response = manager
            .dispatch(&mut client, &request(0), Some(transfer.clone()), 7)
            .unwrap();
        assert_eq!(response.raw_payload, 0u32.to_le_bytes());
        assert_eq!(client.registration.as_ref().unwrap().transfer, transfer);
        let mut monitor_request = request(1);
        monitor_request.raw_payload = u64::MAX.to_le_bytes().to_vec();
        let mut monitor = BsdClient::default();
        manager
            .dispatch(&mut monitor, &monitor_request, None, 7)
            .unwrap();
        assert!(Arc::ptr_eq(
            client.registration.as_ref().unwrap(),
            monitor.registration.as_ref().unwrap()
        ));
        assert!(
            manager
                .dispatch(&mut BsdClient::default(), &monitor_request, None, 8)
                .is_err()
        );
        let weak = Arc::downgrade(client.registration.as_ref().unwrap());
        drop(client);
        assert!(weak.upgrade().is_some());
        assert!(
            manager
                .dispatch(
                    &mut BsdClient::default(),
                    &request(0),
                    Some(transfer.clone()),
                    7
                )
                .is_err()
        );
        drop(monitor);
        assert!(weak.upgrade().is_none());
        assert!(
            manager
                .dispatch(&mut BsdClient::default(), &monitor_request, None, 7)
                .is_err()
        );
        manager
            .dispatch(&mut BsdClient::default(), &request(0), Some(transfer), 7)
            .unwrap();
    }

    #[test]
    fn malformed_or_foreign_bootstrap_does_not_install_registration() {
        let mut manager = BsdManager::default();
        let mut client = BsdClient::default();
        assert!(manager.dispatch(&mut client, &request(0), None, 1).is_err());
        let mut truncated = request(0);
        truncated.raw_payload.truncate(47);
        assert!(manager.dispatch(&mut client, &truncated, None, 1).is_err());
        let mut monitor = request(1);
        monitor.raw_payload = 2u64.to_le_bytes().to_vec();
        assert!(manager.dispatch(&mut client, &monitor, None, 1).is_err());
        assert!(client.registration.is_none());
        assert!(manager.clients.is_empty());
        assert!(matches!(
            manager.dispatch(&mut client, &request(2), None, 1),
            Err(HorizonError::UnknownCommand { command_id: 2, .. })
        ));
    }
}
