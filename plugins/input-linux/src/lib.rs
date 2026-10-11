// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Linux keyboard/mouse guest-device plugin skeleton.
//!
//! The final Linux implementation may capture evdev/libinput events, normalize them here and emit
//! ABI-stable keyboard/mouse events. No Linux input structure is exposed to the runtime.

use core::ffi::c_void;
use exosphere_abi::{ExosphereStatus, ExosphereStrView};
use exosphere_hypervisor::device::{
    DEVICE_ABI_VERSION, DeviceClass, DeviceHostApiV1, DevicePluginV1,
};

static NAME: &[u8] = b"Linux keyboard/mouse input";

unsafe extern "C" fn start(
    _userdata: *mut c_void,
    host: *const DeviceHostApiV1,
) -> ExosphereStatus {
    if host.is_null() {
        return ExosphereStatus::InvalidArgument;
    }
    // TODO: open configured evdev/libinput sources and retain only project-owned normalized state.
    ExosphereStatus::Ok
}
unsafe extern "C" fn poll(_userdata: *mut c_void) -> ExosphereStatus {
    // TODO: drain host events and emit normalized keyboard/mouse payloads through DeviceHostApiV1.
    ExosphereStatus::Ok
}
unsafe extern "C" fn control(
    _userdata: *mut c_void,
    _command: u32,
    _data: *const u8,
    _len: usize,
) -> ExosphereStatus {
    ExosphereStatus::Unsupported
}
unsafe extern "C" fn stop(_userdata: *mut c_void) {}

// This plugin's descriptor only points to immutable static name bytes; userdata is null.
// Keep the Sync assertion local instead of granting it to arbitrary ABI descriptors.
struct StaticPlugin(DevicePluginV1);
unsafe impl Sync for StaticPlugin {}

static PLUGIN: StaticPlugin = StaticPlugin(DevicePluginV1 {
    abi_version: DEVICE_ABI_VERSION,
    struct_size: core::mem::size_of::<DevicePluginV1>() as u32,
    plugin_name: ExosphereStrView {
        ptr: NAME.as_ptr().cast(),
        len: NAME.len(),
    },
    device_class: DeviceClass::InputComposite,
    userdata: core::ptr::null_mut(),
    start: Some(start),
    poll: Some(poll),
    control: Some(control),
    stop: Some(stop),
});

/// Negotiates and returns the plugin ABI.
///
/// # Safety
/// `out` must point to writable storage for one pointer. The returned pointer is static and remains
/// valid until the dynamic library is unloaded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exosphere_device_plugin_query(
    requested_abi: u32,
    out: *mut *const DevicePluginV1,
) -> ExosphereStatus {
    if requested_abi != DEVICE_ABI_VERSION {
        return ExosphereStatus::AbiMismatch;
    }
    if out.is_null() {
        return ExosphereStatus::InvalidArgument;
    }
    unsafe {
        *out = &PLUGIN.0;
    }
    ExosphereStatus::Ok
}
