// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Stable guest-device plugin ABI.
//!
//! This ABI is intentionally C-shaped so a device plugin may be implemented in Rust, C or C++.
//! The plugin captures/produces host-side events, while the hypervisor/runtime decides how those
//! events are represented to the guest. This avoids hard-coding Linux evdev, Windows Raw Input, etc.

use core::ffi::c_void;
use exosphere_abi::{EXOSPHERE_DEVICE_ABI_VERSION, ExosphereStatus, ExosphereStrView};

/// Broad class used for discovery and UI. Detailed capabilities are reported separately.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Keyboard = 1,
    Mouse = 2,
    Gamepad = 3,
    Storage = 4,
    Network = 5,
    Sensor = 6,
    Audio = 7,
    Display = 8,
    /// Composite keyboard/mouse input provider.
    InputComposite = 9,
    Custom = 0xffff,
}

/// Event type carried by [`DeviceEventV1::event_type`].
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEventType {
    Key = 1,
    MouseMotion = 2,
    MouseButton = 3,
    MouseWheel = 4,
}

/// ABI-stable normalized keyboard event.
///
/// `usage` is a USB HID usage-page key code rather than a Linux evdev key code. This keeps host
/// backends interchangeable and prevents Linux-specific constants from leaking into the runtime.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KeyboardEventV1 {
    pub usage: u32,
    pub pressed: u8,
    pub repeat: u8,
    pub _reserved: [u8; 2],
}

/// Relative pointer movement in host-normalized units.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouseMotionEventV1 {
    pub dx: i32,
    pub dy: i32,
}

/// Mouse button transition. Buttons follow Exosphere's stable numbering, not a host API enum.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouseButtonEventV1 {
    pub button: u32,
    pub pressed: u8,
    pub _reserved: [u8; 3],
}

/// High-resolution wheel movement. 120 units represent one conventional wheel detent.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouseWheelEventV1 {
    pub x: i32,
    pub y: i32,
}

/// One host-to-guest device event.
///
/// `payload` is valid only for the duration of the callback. The payload schema is selected by
/// `event_type`; versioned event payload schemas will be added rather than changing this structure.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceEventV1 {
    pub event_type: u32,
    pub flags: u32,
    pub timestamp_ns: u64,
    pub payload: *const u8,
    pub payload_len: usize,
}

/// Callbacks supplied by the runtime/hypervisor to a device plugin.
#[repr(C)]
pub struct DeviceHostApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub userdata: *mut c_void,

    /// Emits one event toward the guest-facing runtime.
    ///
    /// The runtime must synchronously copy any payload it wants to retain.
    pub emit_event:
        Option<unsafe extern "C" fn(*mut c_void, *const DeviceEventV1) -> ExosphereStatus>,

    /// Requests injection of an interrupt associated with this virtual device.
    pub request_irq: Option<unsafe extern "C" fn(*mut c_void, u32) -> ExosphereStatus>,
}

/// Device plugin vtable returned by `exosphere_device_plugin_query`.
#[repr(C)]
pub struct DevicePluginV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub plugin_name: ExosphereStrView,
    pub device_class: DeviceClass,
    pub userdata: *mut c_void,

    /// Initializes host integration. Called exactly once before `poll`.
    pub start: Option<unsafe extern "C" fn(*mut c_void, *const DeviceHostApiV1) -> ExosphereStatus>,
    /// Polls/pumps the host source. A plugin may instead use an internal thread and make this cheap.
    pub poll: Option<unsafe extern "C" fn(*mut c_void) -> ExosphereStatus>,
    /// Applies a guest-to-device control message, such as LEDs or pointer capture.
    pub control:
        Option<unsafe extern "C" fn(*mut c_void, u32, *const u8, usize) -> ExosphereStatus>,
    /// Stops host integration. Must be safe after a partial `start` failure.
    pub stop: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// Symbol name every dynamic device plugin exports.
pub const DEVICE_QUERY_SYMBOL: &[u8] = b"exosphere_device_plugin_query\0";

/// Function signature of the exported device-plugin query entrypoint.
pub type DevicePluginQueryFn =
    unsafe extern "C" fn(u32, *mut *const DevicePluginV1) -> ExosphereStatus;

/// ABI revision expected by the loader.
pub const DEVICE_ABI_VERSION: u32 = EXOSPHERE_DEVICE_ABI_VERSION;
