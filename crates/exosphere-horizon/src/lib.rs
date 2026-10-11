// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Firmware-versioned Horizon compatibility layer.
//!
//! The kernel model deliberately stays independent of KVM, Vulkan, Linux and CPU codegen. That
//! separation lets compiled guest code call Horizon through a stable runtime boundary while host
//! backends evolve independently.

pub mod aoc;
pub mod apm;
pub mod applet;
pub mod binder;
pub mod bsd;
pub mod buffer_queue;
pub mod cmif;
pub mod display;
pub mod domain;
pub mod error;
pub mod event;
pub mod fs;
pub mod fsp;
pub mod fsp_object;
pub mod graphic_producer;
pub mod handle;
pub mod hid;
pub mod hid_service;
pub mod hipc;
pub mod ipc;
pub mod kernel;
pub mod layout;
pub mod logging;
pub mod memory;
pub mod nifm;
pub mod nvdrv;
pub mod parcel;
pub mod parental;
pub mod process;
pub mod profile;
pub mod scheduler;
pub mod shared_memory;
pub mod svc;
pub mod thread;
pub mod time;
pub mod time_service;
pub mod vi;

pub use error::{HorizonError, HorizonResult};
pub use ipc::{
    HorizonService, IpcBuffer, IpcBufferDirection, IpcBufferWrite, IpcValue, ServiceRequest,
    ServiceResponse,
};
pub use kernel::HorizonKernel;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HorizonProfileId(pub String);
