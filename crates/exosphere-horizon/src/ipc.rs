// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Host-neutral representation of Horizon IPC traffic.
//!
//! HIPC transport descriptors are normalized into `IpcBuffer` objects before a service sees a
//! request. Services therefore never need to decode descriptor bitfields or touch guest memory.

use crate::{
    HorizonProfileId,
    error::{HorizonError, HorizonResult},
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcValue {
    U32(u32),
    U64(u64),
    Bytes(Vec<u8>),
    Handle(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcBufferDirection {
    In,
    Out,
    InOut,
}

/// Materialized guest IPC buffer.
///
/// `address` and `capacity` describe the original guest mapping. Input bytes are copied into `data`
/// after guest-memory permission checks. Output-only buffers start empty and are written back by
/// the kernel after the service returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcBuffer {
    pub address: u64,
    pub capacity: u64,
    pub direction: IpcBufferDirection,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcBufferWrite {
    /// Index in the request's normalized `buffers` array.
    pub buffer_index: usize,
    pub data: Vec<u8>,
}

/// Kernel object requested by a service response.
///
/// Service implementations never allocate guest handles directly. They describe the object they
/// want returned and `HorizonKernel` materializes a process-local handle after dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnedObject {
    ReadableEvent(crate::event::SharedEvent),
    HidSharedMemory {
        resource_id: u64,
    },
    Hid(crate::hid_service::HidObject),
    Nifm(crate::nifm::NifmObject),
    Vi(crate::vi::ViSession),
    Time(crate::time_service::TimeObject),
    ParentalSession {
        initialized: bool,
    },
    AddOnListEvent,
    AppletMessageEvent,
    Applet(crate::applet::AppletObject),
    ApmSession,
    Logger {
        process_id: u64,
    },
    NamedService(String),
    FileSystem {
        mount: String,
    },
    File {
        mount: String,
        path: String,
        mode: u8,
    },
    Directory {
        mount: String,
        path: String,
        filter: u64,
    },
    Storage {
        name: String,
        writable: bool,
    },
}

#[derive(Debug, Clone)]
pub struct ServiceRequest {
    pub service: String,
    pub command_id: u32,
    pub values: Vec<IpcValue>,
    pub raw_payload: Vec<u8>,
    pub buffers: Vec<IpcBuffer>,
}

#[derive(Debug, Clone)]
pub struct ServiceResponse {
    pub result_code: u32,
    pub values: Vec<IpcValue>,
    pub raw_payload: Vec<u8>,
    pub objects: Vec<ReturnedObject>,
    pub buffer_writes: Vec<IpcBufferWrite>,
}
impl ServiceResponse {
    pub fn success() -> Self {
        Self {
            result_code: 0,
            values: Vec::new(),
            raw_payload: Vec::new(),
            objects: Vec::new(),
            buffer_writes: Vec::new(),
        }
    }
}

pub trait HorizonService: Send {
    fn name(&self) -> &'static str;
    fn call(
        &mut self,
        profile: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse>;
}

#[derive(Default)]
pub struct ServiceRegistry {
    services: BTreeMap<String, Box<dyn HorizonService>>,
}
impl std::fmt::Debug for ServiceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceRegistry")
            .field("services", &self.services.keys().collect::<Vec<_>>())
            .finish()
    }
}
impl ServiceRegistry {
    pub fn register<S: HorizonService + 'static>(&mut self, service: S) {
        self.services
            .insert(service.name().to_owned(), Box::new(service));
    }
    pub fn contains(&self, name: &str) -> bool {
        self.services.contains_key(name)
    }
    pub fn call(
        &mut self,
        profile: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        let service = self
            .services
            .get_mut(&request.service)
            .ok_or_else(|| HorizonError::UnknownService(request.service.clone()))?;
        service.call(profile, request)
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.services.keys().map(String::as_str)
    }
}

#[derive(Debug, Default)]
pub struct SmService;
impl HorizonService for SmService {
    fn name(&self) -> &'static str {
        "sm:"
    }
    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        match request.command_id {
            0 => Ok(ServiceResponse::success()),
            1 => {
                let raw = request.raw_payload.get(..8).ok_or_else(|| {
                    HorizonError::IpcProtocol(
                        "sm:GetServiceHandle requires an 8-byte ServiceName".into(),
                    )
                })?;
                let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
                let name = std::str::from_utf8(&raw[..end]).map_err(|_| {
                    HorizonError::IpcProtocol("service name is not ASCII/UTF-8".into())
                })?;
                if name.is_empty() {
                    return Err(HorizonError::IpcProtocol("service name is empty".into()));
                }
                let mut response = ServiceResponse::success();
                response
                    .objects
                    .push(ReturnedObject::NamedService(name.to_owned()));
                Ok(response)
            }
            id => Err(HorizonError::UnknownCommand {
                service: self.name().to_owned(),
                command_id: id,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IpcDispatchResponse {
    pub version: u16,
    pub result_code: u32,
    pub payload: Vec<u8>,
    pub objects: Vec<ReturnedObject>,
    pub buffer_writes: Vec<IpcBufferWrite>,
}

/// Decodes HIPC + CMIF framing without materializing guest buffers.
///
/// The kernel uses `decode_hipc_cmif_request_with_buffers` internally after descriptor validation;
/// this helper remains useful for transport-only tests.
pub fn decode_hipc_cmif_request(
    service: &str,
    bytes: &[u8],
) -> HorizonResult<(u16, ServiceRequest)> {
    let hipc = crate::hipc::parse(bytes)?;
    let cmif = crate::cmif::parse_request(hipc.raw_data)?;
    Ok((
        cmif.header.version,
        ServiceRequest {
            service: service.to_owned(),
            command_id: cmif.header.method_id,
            values: Vec::new(),
            raw_payload: cmif.arguments.to_vec(),
            buffers: Vec::new(),
        },
    ))
}

pub fn finish_dispatch(version: u16, response: ServiceResponse) -> IpcDispatchResponse {
    let mut payload = response.raw_payload;
    for value in response.values {
        match value {
            IpcValue::U32(value) | IpcValue::Handle(value) => {
                payload.extend_from_slice(&value.to_le_bytes())
            }
            IpcValue::U64(value) => payload.extend_from_slice(&value.to_le_bytes()),
            IpcValue::Bytes(bytes) => payload.extend_from_slice(&bytes),
        }
    }
    IpcDispatchResponse {
        version,
        result_code: response.result_code,
        payload,
        objects: response.objects,
        buffer_writes: response.buffer_writes,
    }
}

pub fn dispatch_hipc_cmif(
    registry: &mut ServiceRegistry,
    profile: &HorizonProfileId,
    service: &str,
    bytes: &[u8],
) -> HorizonResult<IpcDispatchResponse> {
    let (version, request) = decode_hipc_cmif_request(service, bytes)?;
    let response = registry.call(profile, &request)?;
    Ok(finish_dispatch(version, response))
}
