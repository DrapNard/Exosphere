// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Dynamic plugin loader.

use crate::device::{DEVICE_ABI_VERSION, DEVICE_QUERY_SYMBOL, DevicePluginQueryFn, DevicePluginV1};
use exosphere_abi::ExosphereStatus;
use libloading::Library;
use std::path::Path;

/// Owns a dynamic library for at least as long as pointers into its exported vtable are used.
pub struct LoadedDevicePlugin {
    _library: Library,
    vtable: *const DevicePluginV1,
}

impl LoadedDevicePlugin {
    /// Loads a plugin and negotiates ABI v1.
    ///
    /// # Safety
    /// Loading arbitrary native plugins executes native code in-process. Callers must only load
    /// trusted plugins. The ABI validates version/layout compatibility, not plugin trustworthiness.
    /// The exported query symbol must use `DevicePluginQueryFn`'s ABI and return
    /// an aligned, fully initialized `DevicePluginV1` table whose storage remains
    /// valid while the library is loaded.
    pub unsafe fn load(path: &Path) -> Result<Self, String> {
        let library = unsafe { Library::new(path) }.map_err(|e| e.to_string())?;
        let query = unsafe { library.get::<DevicePluginQueryFn>(DEVICE_QUERY_SYMBOL) }
            .map_err(|e| e.to_string())?;
        let mut vtable = core::ptr::null();
        let status = unsafe { query(DEVICE_ABI_VERSION, &mut vtable) };
        if status != ExosphereStatus::Ok || vtable.is_null() {
            return Err(format!("plugin rejected ABI: {status:?}"));
        }
        Ok(Self {
            _library: library,
            vtable,
        })
    }

    pub fn vtable(&self) -> &DevicePluginV1 {
        // SAFETY: construction rejects a null vtable and `_library` keeps its storage/code loaded.
        unsafe { &*self.vtable }
    }
}
