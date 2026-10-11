// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only guest time objects backed by the kernel's injectable time source.

use crate::{
    HorizonProfileId,
    error::{HorizonError, HorizonResult},
    ipc::{HorizonService, ReturnedObject, ServiceRequest, ServiceResponse},
    time::TimeManager,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeObject {
    UserSystem,
    NetworkSystem,
    Steady,
    TimeZone,
    LocalSystem,
}

impl TimeObject {
    pub fn name(self) -> &'static str {
        match self {
            Self::UserSystem => "IUserSystemClock",
            Self::NetworkSystem => "INetworkSystemClock",
            Self::Steady => "ISteadyClock",
            Self::TimeZone => "ITimeZoneService",
            Self::LocalSystem => "ILocalSystemClock",
        }
    }
}

pub struct TimeService;
impl HorizonService for TimeService {
    fn name(&self) -> &'static str {
        "time:u"
    }
    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let object = match request.command_id {
            0 => TimeObject::UserSystem,
            1 => TimeObject::NetworkSystem,
            2 => TimeObject::Steady,
            3 => TimeObject::TimeZone,
            4 => TimeObject::LocalSystem,
            _ => return Err(unknown("time:u", request.command_id)),
        };
        let mut response = ServiceResponse::success();
        response.objects.push(ReturnedObject::Time(object));
        Ok(response)
    }
}

fn unknown(service: &str, command_id: u32) -> HorizonError {
    HorizonError::UnknownCommand {
        service: service.into(),
        command_id,
    }
}

pub fn dispatch(
    time: &TimeManager,
    object: TimeObject,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    let mut response = ServiceResponse::success();
    match (object, request.command_id) {
        (TimeObject::Steady, 0) => {
            let point = time.steady_clock();
            // The IPC ABI uses signed whole seconds, unlike the internal ns API.
            let seconds = (point.time_point_ns / 1_000_000_000) as i64;
            response
                .raw_payload
                .extend_from_slice(&seconds.to_le_bytes());
            response.raw_payload.extend_from_slice(&point.source_id);
        }
        (TimeObject::UserSystem | TimeObject::LocalSystem, 0) => {
            // Diagnostic policy: user/local time comes from the configured source's
            // wall clock. It is not evidence of network synchronization or RTC emulation.
            response
                .raw_payload
                .extend_from_slice(&time.unix_seconds().to_le_bytes());
        }
        (TimeObject::UserSystem | TimeObject::LocalSystem, 2) => {
            let point = time.steady_clock();
            let seconds = (point.time_point_ns / 1_000_000_000) as i64;
            let offset = time.unix_seconds().checked_sub(seconds).ok_or_else(|| {
                HorizonError::InvalidState("system clock context offset overflow".into())
            })?;
            response
                .raw_payload
                .extend_from_slice(&offset.to_le_bytes());
            response
                .raw_payload
                .extend_from_slice(&seconds.to_le_bytes());
            response.raw_payload.extend_from_slice(&point.source_id);
        }
        // Network synchronization, timezone rules, clock setters and shared
        // memory require real policy/state; do not fabricate success for them.
        _ => return Err(unknown(object.name(), request.command_id)),
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FixedTime;
    impl crate::time::TimeSource for FixedTime {
        fn monotonic_ns(&self) -> u64 {
            12_999_999_999
        }
        fn unix_seconds(&self) -> i64 {
            1_700_000_000
        }
    }
    fn request(id: u32) -> ServiceRequest {
        ServiceRequest {
            service: "time:u".into(),
            command_id: id,
            values: vec![],
            raw_payload: vec![],
            buffers: vec![],
        }
    }
    #[test]
    fn factory_returns_distinct_typed_clock_objects() {
        let mut service = TimeService;
        for (id, object) in [
            TimeObject::UserSystem,
            TimeObject::NetworkSystem,
            TimeObject::Steady,
            TimeObject::TimeZone,
            TimeObject::LocalSystem,
        ]
        .into_iter()
        .enumerate()
        {
            let response = service
                .call(&HorizonProfileId("22.1.0".into()), &request(id as u32))
                .unwrap();
            assert_eq!(response.objects, vec![ReturnedObject::Time(object)]);
        }
        assert!(
            service
                .call(&HorizonProfileId("22.1.0".into()), &request(20))
                .is_err()
        );
    }
    #[test]
    fn clock_reads_share_source_identity_and_serialize_seconds_and_offsets() {
        let time = TimeManager::new(Box::new(FixedTime));
        let steady = dispatch(&time, TimeObject::Steady, &request(0))
            .unwrap()
            .raw_payload;
        assert_eq!(steady.len(), 24);
        assert_eq!(i64::from_le_bytes(steady[..8].try_into().unwrap()), 12);
        for object in [TimeObject::UserSystem, TimeObject::LocalSystem] {
            let current = dispatch(&time, object, &request(0)).unwrap().raw_payload;
            assert_eq!(
                i64::from_le_bytes(current.try_into().unwrap()),
                1_700_000_000
            );
            let context = dispatch(&time, object, &request(2)).unwrap().raw_payload;
            assert_eq!(context.len(), 32);
            assert_eq!(
                i64::from_le_bytes(context[..8].try_into().unwrap()),
                1_699_999_988
            );
            assert_eq!(&context[8..], &steady);
            assert!(dispatch(&time, object, &request(1)).is_err());
        }
        assert!(dispatch(&time, TimeObject::NetworkSystem, &request(0)).is_err());
        assert!(dispatch(&time, TimeObject::TimeZone, &request(0)).is_err());
    }
}
