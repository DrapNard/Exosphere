// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only Vulkan device metadata, without device creation or command submission.

use ash::{Entry, vk};
use std::ffi::CStr;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let entry = unsafe { Entry::load()? };
    let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_0);
    let info = vk::InstanceCreateInfo::default().application_info(&app);
    let instance = unsafe { entry.create_instance(&info, None)? };
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        for device in unsafe { instance.enumerate_physical_devices()? } {
            let properties = unsafe { instance.get_physical_device_properties(device) };
            let name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) };
            println!(
                "device={} API={}.{}.{} type={:?}",
                name.to_string_lossy(),
                vk::api_version_major(properties.api_version),
                vk::api_version_minor(properties.api_version),
                vk::api_version_patch(properties.api_version),
                properties.device_type,
            );
        }
        Ok(())
    })();
    // Destroy the instance on both success and metadata-query failure, while
    // the dynamically loaded entry library remains alive.
    unsafe { instance.destroy_instance(None) };
    result
}
