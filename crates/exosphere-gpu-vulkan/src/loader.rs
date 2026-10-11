// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Dynamic Vulkan loader discovery.
//!
//! This avoids a hard link-time dependency on libvulkan. The actual instance/device backend can be
//! developed behind the same execution-plan interface while `doctor` already reports loader state.

use libloading::Library;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VulkanLoaderStatus {
    Available { library: String },
    Unavailable,
}

#[cfg(target_os = "linux")]
pub fn probe() -> VulkanLoaderStatus {
    for candidate in ["libvulkan.so.1", "libvulkan.so"] {
        // SAFETY: Loading a shared library is inherently unsafe because constructors may execute.
        // Candidates are fixed platform Vulkan loader names, not user-controlled paths. We keep the
        // library only long enough to verify that it can be opened; no foreign function is called.
        if unsafe { Library::new(candidate) }.is_ok() {
            return VulkanLoaderStatus::Available {
                library: candidate.to_owned(),
            };
        }
    }
    VulkanLoaderStatus::Unavailable
}

#[cfg(not(target_os = "linux"))]
pub fn probe() -> VulkanLoaderStatus {
    VulkanLoaderStatus::Unavailable
}
