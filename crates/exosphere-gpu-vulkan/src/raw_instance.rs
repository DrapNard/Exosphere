// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Minimal raw Vulkan context for Linux.
//!
//! This module is intentionally below Exosphere's canonical GPU IR. It owns only Vulkan loader,
//! instance, physical-device selection, one graphics-capable logical device and one queue. Window
//! system integration and swapchains remain a later platform layer.

use libloading::{Library, Symbol};
use std::{
    ffi::{CString, c_char, c_void},
    fmt, ptr,
};

pub type VkInstance = *mut c_void;
pub type VkPhysicalDevice = *mut c_void;
pub type VkDevice = *mut c_void;
pub type VkQueue = *mut c_void;
pub type VkCommandPool = *mut c_void;
pub type VkCommandBuffer = *mut c_void;
pub type VkFence = *mut c_void;
pub type VkSemaphore = *mut c_void;
pub type VkResult = i32;

const VK_SUCCESS: VkResult = 0;
const VK_STRUCTURE_TYPE_APPLICATION_INFO: u32 = 0;
const VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO: u32 = 1;
const VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO: u32 = 2;
const VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO: u32 = 3;
const VK_STRUCTURE_TYPE_SUBMIT_INFO: u32 = 4;
const VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO: u32 = 39;
const VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO: u32 = 40;
const VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO: u32 = 42;
const VK_COMMAND_BUFFER_LEVEL_PRIMARY: u32 = 0;
const VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT: u32 = 0x0000_0001;
const VK_QUEUE_GRAPHICS_BIT: u32 = 0x0000_0001;

#[repr(C)]
struct VkApplicationInfo {
    s_type: u32,
    p_next: *const c_void,
    p_application_name: *const c_char,
    application_version: u32,
    p_engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

#[repr(C)]
struct VkInstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    p_application_info: *const VkApplicationInfo,
    enabled_layer_count: u32,
    pp_enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    pp_enabled_extension_names: *const *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VkExtent3D {
    width: u32,
    height: u32,
    depth: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VkQueueFamilyProperties {
    queue_flags: u32,
    queue_count: u32,
    timestamp_valid_bits: u32,
    min_image_transfer_granularity: VkExtent3D,
}

#[repr(C)]
struct VkDeviceQueueCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_family_index: u32,
    queue_count: u32,
    p_queue_priorities: *const f32,
}

#[repr(C)]
struct VkDeviceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_create_info_count: u32,
    p_queue_create_infos: *const VkDeviceQueueCreateInfo,
    enabled_layer_count: u32,
    pp_enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    pp_enabled_extension_names: *const *const c_char,
    p_enabled_features: *const c_void,
}

#[repr(C)]
struct VkCommandPoolCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_family_index: u32,
}

#[repr(C)]
struct VkCommandBufferAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    command_pool: VkCommandPool,
    level: u32,
    command_buffer_count: u32,
}

#[repr(C)]
struct VkCommandBufferBeginInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    p_inheritance_info: *const c_void,
}

#[repr(C)]
struct VkSubmitInfo {
    s_type: u32,
    p_next: *const c_void,
    wait_semaphore_count: u32,
    p_wait_semaphores: *const VkSemaphore,
    p_wait_dst_stage_mask: *const u32,
    command_buffer_count: u32,
    p_command_buffers: *const VkCommandBuffer,
    signal_semaphore_count: u32,
    p_signal_semaphores: *const VkSemaphore,
}

type VkCreateInstance = unsafe extern "system" fn(
    *const VkInstanceCreateInfo,
    *const c_void,
    *mut VkInstance,
) -> VkResult;
type VkDestroyInstance = unsafe extern "system" fn(VkInstance, *const c_void);
type VkEnumerateInstanceVersion = unsafe extern "system" fn(*mut u32) -> VkResult;
type VkEnumeratePhysicalDevices =
    unsafe extern "system" fn(VkInstance, *mut u32, *mut VkPhysicalDevice) -> VkResult;
type VkGetPhysicalDeviceQueueFamilyProperties =
    unsafe extern "system" fn(VkPhysicalDevice, *mut u32, *mut VkQueueFamilyProperties);
type VkCreateDevice = unsafe extern "system" fn(
    VkPhysicalDevice,
    *const VkDeviceCreateInfo,
    *const c_void,
    *mut VkDevice,
) -> VkResult;
type VkDestroyDevice = unsafe extern "system" fn(VkDevice, *const c_void);
type VkGetDeviceQueue = unsafe extern "system" fn(VkDevice, u32, u32, *mut VkQueue);
type VkDeviceWaitIdle = unsafe extern "system" fn(VkDevice) -> VkResult;
type VkCreateCommandPool = unsafe extern "system" fn(
    VkDevice,
    *const VkCommandPoolCreateInfo,
    *const c_void,
    *mut VkCommandPool,
) -> VkResult;
type VkDestroyCommandPool = unsafe extern "system" fn(VkDevice, VkCommandPool, *const c_void);
type VkAllocateCommandBuffers = unsafe extern "system" fn(
    VkDevice,
    *const VkCommandBufferAllocateInfo,
    *mut VkCommandBuffer,
) -> VkResult;
type VkFreeCommandBuffers =
    unsafe extern "system" fn(VkDevice, VkCommandPool, u32, *const VkCommandBuffer);
type VkBeginCommandBuffer =
    unsafe extern "system" fn(VkCommandBuffer, *const VkCommandBufferBeginInfo) -> VkResult;
type VkEndCommandBuffer = unsafe extern "system" fn(VkCommandBuffer) -> VkResult;
type VkQueueSubmit =
    unsafe extern "system" fn(VkQueue, u32, *const VkSubmitInfo, VkFence) -> VkResult;
type VkQueueWaitIdle = unsafe extern "system" fn(VkQueue) -> VkResult;

/// Owns one Vulkan instance and, when available, one graphics-capable device/queue.
pub struct RawVulkanInstance {
    instance: VkInstance,
    pub(crate) physical_device: VkPhysicalDevice,
    pub(crate) device: VkDevice,
    pub(crate) queue: VkQueue,
    pub(crate) queue_family_index: Option<u32>,
    destroy_instance: VkDestroyInstance,
    destroy_device: Option<VkDestroyDevice>,
    device_wait_idle: Option<VkDeviceWaitIdle>,
    create_command_pool: Option<VkCreateCommandPool>,
    destroy_command_pool: Option<VkDestroyCommandPool>,
    allocate_command_buffers: Option<VkAllocateCommandBuffers>,
    free_command_buffers: Option<VkFreeCommandBuffers>,
    begin_command_buffer: Option<VkBeginCommandBuffer>,
    end_command_buffer: Option<VkEndCommandBuffer>,
    queue_submit: Option<VkQueueSubmit>,
    queue_wait_idle: Option<VkQueueWaitIdle>,
    pub(crate) _library: Library,
    api_version: u32,
    physical_device_count: u32,
}

// Handles are externally synchronized by Exosphere and are destroyed on the same owner object.
unsafe impl Send for RawVulkanInstance {}

impl fmt::Debug for RawVulkanInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawVulkanInstance")
            .field("instance", &self.instance)
            .field("device", &self.device)
            .field("queue_family_index", &self.queue_family_index)
            .field("api_version", &format_args!("0x{:08x}", self.api_version))
            .field("physical_device_count", &self.physical_device_count)
            .finish()
    }
}

impl RawVulkanInstance {
    #[cfg(target_os = "linux")]
    pub fn create() -> Result<Self, String> {
        let library = ["libvulkan.so.1", "libvulkan.so"]
            .into_iter()
            .find_map(|name| unsafe { Library::new(name).ok() })
            .ok_or_else(|| "Vulkan loader not found".to_owned())?;

        let create_instance: VkCreateInstance = load(&library, b"vkCreateInstance\0")?;
        let destroy_instance: VkDestroyInstance = load(&library, b"vkDestroyInstance\0")?;
        let enumerate_devices: VkEnumeratePhysicalDevices =
            load(&library, b"vkEnumeratePhysicalDevices\0")?;
        let get_queue_families: VkGetPhysicalDeviceQueueFamilyProperties =
            load(&library, b"vkGetPhysicalDeviceQueueFamilyProperties\0")?;
        let create_device: VkCreateDevice = load(&library, b"vkCreateDevice\0")?;
        let destroy_device: VkDestroyDevice = load(&library, b"vkDestroyDevice\0")?;
        let get_device_queue: VkGetDeviceQueue = load(&library, b"vkGetDeviceQueue\0")?;
        let device_wait_idle: VkDeviceWaitIdle = load(&library, b"vkDeviceWaitIdle\0")?;
        let create_command_pool: VkCreateCommandPool = load(&library, b"vkCreateCommandPool\0")?;
        let destroy_command_pool: VkDestroyCommandPool = load(&library, b"vkDestroyCommandPool\0")?;
        let allocate_command_buffers: VkAllocateCommandBuffers =
            load(&library, b"vkAllocateCommandBuffers\0")?;
        let free_command_buffers: VkFreeCommandBuffers = load(&library, b"vkFreeCommandBuffers\0")?;
        let begin_command_buffer: VkBeginCommandBuffer = load(&library, b"vkBeginCommandBuffer\0")?;
        let end_command_buffer: VkEndCommandBuffer = load(&library, b"vkEndCommandBuffer\0")?;
        let queue_submit: VkQueueSubmit = load(&library, b"vkQueueSubmit\0")?;
        let queue_wait_idle: VkQueueWaitIdle = load(&library, b"vkQueueWaitIdle\0")?;

        let mut api_version = 1u32 << 22; // Vulkan 1.0
        if let Ok(enumerate) =
            unsafe { library.get::<VkEnumerateInstanceVersion>(b"vkEnumerateInstanceVersion\0") }
        {
            let mut reported = 0u32;
            if unsafe { enumerate(&mut reported) } == VK_SUCCESS {
                api_version = reported;
            }
        }

        let app_name = CString::new("Exosphere").expect("static CString");
        let engine_name = CString::new("Exosphere").expect("static CString");
        let app = VkApplicationInfo {
            s_type: VK_STRUCTURE_TYPE_APPLICATION_INFO,
            p_next: ptr::null(),
            p_application_name: app_name.as_ptr(),
            application_version: 1,
            p_engine_name: engine_name.as_ptr(),
            engine_version: 1,
            api_version,
        };
        let info = VkInstanceCreateInfo {
            s_type: VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            p_application_info: &app,
            enabled_layer_count: 0,
            pp_enabled_layer_names: ptr::null(),
            enabled_extension_count: 0,
            pp_enabled_extension_names: ptr::null(),
        };

        let mut instance = ptr::null_mut();
        let result = unsafe { create_instance(&info, ptr::null(), &mut instance) };
        if result != VK_SUCCESS || instance.is_null() {
            return Err(format!("vkCreateInstance failed with VkResult {result}"));
        }

        let setup =
            (|| -> Result<(VkPhysicalDevice, VkDevice, VkQueue, Option<u32>, u32), String> {
                let mut physical_device_count = 0u32;
                let result = unsafe {
                    enumerate_devices(instance, &mut physical_device_count, ptr::null_mut())
                };
                if result != VK_SUCCESS {
                    return Err(format!(
                        "vkEnumeratePhysicalDevices(count) failed with VkResult {result}"
                    ));
                }
                if physical_device_count == 0 {
                    return Ok((ptr::null_mut(), ptr::null_mut(), ptr::null_mut(), None, 0));
                }

                let mut physical_devices = vec![ptr::null_mut(); physical_device_count as usize];
                let result = unsafe {
                    enumerate_devices(
                        instance,
                        &mut physical_device_count,
                        physical_devices.as_mut_ptr(),
                    )
                };
                if result != VK_SUCCESS {
                    return Err(format!(
                        "vkEnumeratePhysicalDevices(list) failed with VkResult {result}"
                    ));
                }
                physical_devices.truncate(physical_device_count as usize);

                for physical in physical_devices {
                    let mut family_count = 0u32;
                    unsafe { get_queue_families(physical, &mut family_count, ptr::null_mut()) };
                    if family_count == 0 {
                        continue;
                    }
                    let mut families =
                        vec![VkQueueFamilyProperties::default(); family_count as usize];
                    unsafe {
                        get_queue_families(physical, &mut family_count, families.as_mut_ptr())
                    };
                    let Some((family_index, _)) =
                        families.iter().enumerate().find(|(_, family)| {
                            family.queue_count > 0
                                && family.queue_flags & VK_QUEUE_GRAPHICS_BIT != 0
                        })
                    else {
                        continue;
                    };

                    let priority = 1.0f32;
                    let queue_info = VkDeviceQueueCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
                        p_next: ptr::null(),
                        flags: 0,
                        queue_family_index: family_index as u32,
                        queue_count: 1,
                        p_queue_priorities: &priority,
                    };
                    let device_info = VkDeviceCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
                        p_next: ptr::null(),
                        flags: 0,
                        queue_create_info_count: 1,
                        p_queue_create_infos: &queue_info,
                        enabled_layer_count: 0,
                        pp_enabled_layer_names: ptr::null(),
                        enabled_extension_count: 0,
                        pp_enabled_extension_names: ptr::null(),
                        p_enabled_features: ptr::null(),
                    };
                    let mut device = ptr::null_mut();
                    let result =
                        unsafe { create_device(physical, &device_info, ptr::null(), &mut device) };
                    if result != VK_SUCCESS || device.is_null() {
                        continue;
                    }
                    let mut queue = ptr::null_mut();
                    unsafe { get_device_queue(device, family_index as u32, 0, &mut queue) };
                    return Ok((
                        physical,
                        device,
                        queue,
                        Some(family_index as u32),
                        physical_device_count,
                    ));
                }

                Ok((
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    None,
                    physical_device_count,
                ))
            })();

        let (physical_device, device, queue, queue_family_index, physical_device_count) =
            match setup {
                Ok(value) => value,
                Err(error) => {
                    unsafe { destroy_instance(instance, ptr::null()) };
                    return Err(error);
                }
            };

        Ok(Self {
            instance,
            physical_device,
            device,
            queue,
            queue_family_index,
            destroy_instance,
            destroy_device: (!device.is_null()).then_some(destroy_device),
            device_wait_idle: (!device.is_null()).then_some(device_wait_idle),
            create_command_pool: (!device.is_null()).then_some(create_command_pool),
            destroy_command_pool: (!device.is_null()).then_some(destroy_command_pool),
            allocate_command_buffers: (!device.is_null()).then_some(allocate_command_buffers),
            free_command_buffers: (!device.is_null()).then_some(free_command_buffers),
            begin_command_buffer: (!device.is_null()).then_some(begin_command_buffer),
            end_command_buffer: (!device.is_null()).then_some(end_command_buffer),
            queue_submit: (!device.is_null()).then_some(queue_submit),
            queue_wait_idle: (!device.is_null()).then_some(queue_wait_idle),
            _library: library,
            api_version,
            physical_device_count,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn create() -> Result<Self, String> {
        Err("raw Vulkan smoke is currently Linux-only".into())
    }

    /// Allocates, records, submits and retires one empty primary command buffer.
    ///
    /// This is intentionally render-pass-free. Its purpose is to validate Exosphere's raw Vulkan
    /// device/queue lifetime and submission path before pipeline/render-target creation is layered on
    /// top. Success means the driver accepted real queue work, not merely instance/device creation.
    pub fn submit_empty_command_buffer(&self) -> Result<(), String> {
        if self.device.is_null() || self.queue.is_null() {
            return Err("no graphics-capable Vulkan logical device/queue is available".into());
        }
        let family = self
            .queue_family_index
            .ok_or_else(|| "graphics queue family is unavailable".to_owned())?;
        let create_pool = self
            .create_command_pool
            .ok_or_else(|| "vkCreateCommandPool is unavailable".to_owned())?;
        let destroy_pool = self
            .destroy_command_pool
            .ok_or_else(|| "vkDestroyCommandPool is unavailable".to_owned())?;
        let allocate = self
            .allocate_command_buffers
            .ok_or_else(|| "vkAllocateCommandBuffers is unavailable".to_owned())?;
        let free = self
            .free_command_buffers
            .ok_or_else(|| "vkFreeCommandBuffers is unavailable".to_owned())?;
        let begin = self
            .begin_command_buffer
            .ok_or_else(|| "vkBeginCommandBuffer is unavailable".to_owned())?;
        let end = self
            .end_command_buffer
            .ok_or_else(|| "vkEndCommandBuffer is unavailable".to_owned())?;
        let submit = self
            .queue_submit
            .ok_or_else(|| "vkQueueSubmit is unavailable".to_owned())?;
        let wait = self
            .queue_wait_idle
            .ok_or_else(|| "vkQueueWaitIdle is unavailable".to_owned())?;

        let pool_info = VkCommandPoolCreateInfo {
            s_type: VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            queue_family_index: family,
        };
        let mut pool = ptr::null_mut();
        let result = unsafe { create_pool(self.device, &pool_info, ptr::null(), &mut pool) };
        if result != VK_SUCCESS || pool.is_null() {
            return Err(format!("vkCreateCommandPool failed with VkResult {result}"));
        }

        let run = (|| -> Result<(), String> {
            let alloc_info = VkCommandBufferAllocateInfo {
                s_type: VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
                p_next: ptr::null(),
                command_pool: pool,
                level: VK_COMMAND_BUFFER_LEVEL_PRIMARY,
                command_buffer_count: 1,
            };
            let mut command_buffer = ptr::null_mut();
            let result = unsafe { allocate(self.device, &alloc_info, &mut command_buffer) };
            if result != VK_SUCCESS || command_buffer.is_null() {
                return Err(format!(
                    "vkAllocateCommandBuffers failed with VkResult {result}"
                ));
            }

            let command_result = (|| -> Result<(), String> {
                let begin_info = VkCommandBufferBeginInfo {
                    s_type: VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                    p_next: ptr::null(),
                    flags: VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
                    p_inheritance_info: ptr::null(),
                };
                let result = unsafe { begin(command_buffer, &begin_info) };
                if result != VK_SUCCESS {
                    return Err(format!(
                        "vkBeginCommandBuffer failed with VkResult {result}"
                    ));
                }
                let result = unsafe { end(command_buffer) };
                if result != VK_SUCCESS {
                    return Err(format!("vkEndCommandBuffer failed with VkResult {result}"));
                }

                let submit_info = VkSubmitInfo {
                    s_type: VK_STRUCTURE_TYPE_SUBMIT_INFO,
                    p_next: ptr::null(),
                    wait_semaphore_count: 0,
                    p_wait_semaphores: ptr::null(),
                    p_wait_dst_stage_mask: ptr::null(),
                    command_buffer_count: 1,
                    p_command_buffers: &command_buffer,
                    signal_semaphore_count: 0,
                    p_signal_semaphores: ptr::null(),
                };
                let result = unsafe { submit(self.queue, 1, &submit_info, ptr::null_mut()) };
                if result != VK_SUCCESS {
                    return Err(format!("vkQueueSubmit failed with VkResult {result}"));
                }
                let result = unsafe { wait(self.queue) };
                if result != VK_SUCCESS {
                    return Err(format!("vkQueueWaitIdle failed with VkResult {result}"));
                }
                Ok(())
            })();

            unsafe { free(self.device, pool, 1, &command_buffer) };
            command_result
        })();

        unsafe { destroy_pool(self.device, pool, ptr::null()) };
        run
    }

    pub fn api_version(&self) -> u32 {
        self.api_version
    }
    pub fn physical_device_count(&self) -> u32 {
        self.physical_device_count
    }
    pub fn has_logical_device(&self) -> bool {
        !self.device.is_null()
    }
    pub fn has_graphics_queue(&self) -> bool {
        !self.queue.is_null()
    }
    pub fn queue_family_index(&self) -> Option<u32> {
        self.queue_family_index
    }
    pub fn api_version_tuple(&self) -> (u32, u32, u32) {
        (
            (self.api_version >> 22) & 0x7f,
            (self.api_version >> 12) & 0x3ff,
            self.api_version & 0xfff,
        )
    }
}

impl Drop for RawVulkanInstance {
    fn drop(&mut self) {
        if !self.device.is_null() {
            if let Some(wait_idle) = self.device_wait_idle {
                let _ = unsafe { wait_idle(self.device) };
            }
            if let Some(destroy_device) = self.destroy_device {
                unsafe { destroy_device(self.device, ptr::null()) };
            }
            self.device = ptr::null_mut();
            self.queue = ptr::null_mut();
        }
        if !self.instance.is_null() {
            unsafe { (self.destroy_instance)(self.instance, ptr::null()) };
            self.instance = ptr::null_mut();
        }
    }
}

fn load<T: Copy>(library: &Library, symbol: &[u8]) -> Result<T, String> {
    let symbol: Symbol<T> = unsafe { library.get(symbol) }.map_err(|error| error.to_string())?;
    Ok(*symbol)
}
