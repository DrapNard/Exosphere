// SPDX-License-Identifier: GPL-3.0-or-later
//! Retail lm protocol: logger sessions accept packets without exposing a host log sink.

use crate::{
    HorizonProfileId, HorizonService,
    error::{HorizonError, HorizonResult},
    ipc::{IpcBufferDirection, ReturnedObject, ServiceRequest, ServiceResponse},
};

#[derive(Debug)]
pub struct LogService;

impl HorizonService for LogService {
    fn name(&self) -> &'static str {
        "lm"
    }

    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        if request.command_id != 0 {
            return Err(HorizonError::UnknownCommand {
                service: "lm".into(),
                command_id: request.command_id,
            });
        }
        let bytes = request
            .raw_payload
            .get(..8)
            .ok_or_else(|| HorizonError::IpcProtocol("OpenLogger requires a process ID".into()))?;
        let process_id = u64::from_le_bytes(bytes.try_into().unwrap());
        let mut response = ServiceResponse::success();
        response.objects.push(ReturnedObject::Logger { process_id });
        Ok(response)
    }
}

#[derive(Debug, Clone)]
pub struct Logger {
    pub process_id: u64,
    pub packet_count: u64,
    pub byte_count: u64,
}

#[derive(Debug, Default)]
pub struct LogManager {
    pub destination_mask: u32,
}

impl LogManager {
    pub fn dispatch(
        &mut self,
        logger: &mut Logger,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        match request.command_id {
            0 => {
                let buffer = request
                    .buffers
                    .iter()
                    .find(|buffer| {
                        matches!(
                            buffer.direction,
                            IpcBufferDirection::In | IpcBufferDirection::InOut
                        )
                    })
                    .ok_or_else(|| {
                        HorizonError::IpcProtocol("ILogger::Log requires an input buffer".into())
                    })?;
                // Retail lm discards packets. Keep only bounded counters for diagnostics,
                // never message contents or allocations proportional to logged traffic.
                logger.packet_count = logger.packet_count.saturating_add(1);
                logger.byte_count = logger.byte_count.saturating_add(buffer.data.len() as u64);
            }
            1 => {
                let bytes = request.raw_payload.get(..4).ok_or_else(|| {
                    HorizonError::IpcProtocol("SetDestination requires a mask".into())
                })?;
                self.destination_mask = u32::from_le_bytes(bytes.try_into().unwrap());
            }
            command_id => {
                return Err(HorizonError::UnknownCommand {
                    service: "ILogger".into(),
                    command_id,
                });
            }
        }
        Ok(ServiceResponse::success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcBuffer;

    fn request(command_id: u32) -> ServiceRequest {
        ServiceRequest {
            service: "ILogger".into(),
            command_id,
            values: vec![],
            raw_payload: vec![],
            buffers: vec![],
        }
    }

    #[test]
    fn logger_is_typed_and_retail_packet_processing_retains_only_counters() {
        let mut service = LogService;
        let mut open = request(0);
        open.raw_payload = 123u64.to_le_bytes().to_vec();
        let response = service
            .call(&HorizonProfileId("22.1.0".into()), &open)
            .unwrap();
        assert_eq!(
            response.objects,
            vec![ReturnedObject::Logger { process_id: 123 }]
        );
        let mut logger = Logger {
            process_id: 123,
            packet_count: 0,
            byte_count: 0,
        };
        let mut manager = LogManager::default();
        let mut log = request(0);
        assert!(manager.dispatch(&mut logger, &log).is_err());
        log.buffers.push(IpcBuffer {
            address: 0x8000,
            capacity: 4,
            direction: IpcBufferDirection::In,
            data: vec![1, 2, 3, 4],
        });
        manager.dispatch(&mut logger, &log).unwrap();
        assert_eq!((logger.packet_count, logger.byte_count), (1, 4));
        let mut destination = request(1);
        destination.raw_payload = 0xffffu32.to_le_bytes().to_vec();
        manager.dispatch(&mut logger, &destination).unwrap();
        assert_eq!(manager.destination_mask, 0xffff);
        assert!(manager.dispatch(&mut logger, &request(99)).is_err());
    }
}
