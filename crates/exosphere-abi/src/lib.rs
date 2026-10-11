// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Stable C ABI primitives shared by the runtime, hypervisors and native plugins.
//!
//! # ABI rule
//! Public Rust types from this crate are `#[repr(C)]` and intentionally contain only fixed-layout
//! fields. High-level Rust SDKs may wrap these types, but must never leak Rust's unstable ABI across
//! a dynamic-library boundary.

#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::{c_char, c_void};

/// Current host/runtime ABI revision.
pub const EXOSPHERE_ABI_VERSION: u32 = 1;
/// Current hypervisor plugin ABI revision.
pub const EXOSPHERE_HYPERVISOR_ABI_VERSION: u32 = 1;
/// Current guest-device plugin ABI revision.
pub const EXOSPHERE_DEVICE_ABI_VERSION: u32 = 1;

/// ABI-stable status returned by FFI functions.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExosphereStatus {
    /// Operation completed successfully.
    Ok = 0,
    /// One or more pointers/arguments were invalid.
    InvalidArgument = -1,
    /// A requested feature is not implemented by this backend.
    Unsupported = -2,
    /// The requested object or capability was not found.
    NotFound = -3,
    /// A host operating-system operation failed.
    HostError = -4,
    /// The plugin/runtime versions are incompatible.
    AbiMismatch = -5,
    /// Catch-all internal error. Prefer a more specific status whenever possible.
    Internal = -127,
}

/// Immutable UTF-8 string view crossing the C ABI.
///
/// `ptr` is not required to be NUL terminated. The receiver must read exactly `len` bytes and must
/// not retain the pointer after the call unless the owning API explicitly says otherwise.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExosphereStrView {
    pub ptr: *const c_char,
    pub len: usize,
}

/// Opaque runtime-owned handle.
///
/// Handles intentionally carry no pointer semantics. `0` is reserved for an invalid/null handle.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExosphereHandle(pub u64);

/// Host allocator exposed to plugins that need to return variable-sized data.
///
/// Allocation/free must be paired through this exact vtable; memory must never be freed by a
/// different CRT/Rust allocator. This is particularly important when C++, Rust and plugins are
/// built with different toolchains.
#[repr(C)]
pub struct ExosphereAllocatorV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub userdata: *mut c_void,
    pub alloc: Option<unsafe extern "C" fn(*mut c_void, usize, usize) -> *mut c_void>,
    pub free: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, usize, usize)>,
}
