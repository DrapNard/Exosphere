// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Vulkan 1.3 backend built on `ash`.
//!
//! This is the long-term Vulkan path. The older raw-loader backend remains useful as a tiny smoke
//! path, while this module owns feature negotiation, resource lifetimes, pipeline layouts and
//! general command execution.

use std::{collections::BTreeMap, ffi::CString, ptr};

use ash::{Device, Entry, Instance, vk};
mod buffer_descriptor;
mod float_controls;
mod image_usage;
mod present_blit;
mod readback;
mod sampler;
mod texture_payload;
use exosphere_gpu_ir::{
    BufferDesc, BufferUsage, ComputePipelineDesc, DescriptorBindingDesc, DescriptorType,
    GpuResourceData, PipelineLayoutDesc, QueryKind, ResourceId, SamplerDesc, ShaderStageMask,
    TextureDesc, TextureLayout, deswizzle_layers, format_info, linear_footprint,
};

use crate::{
    VulkanError,
    device::{ExecutionToken, FeatureSupport, VulkanCapabilities, VulkanDevice},
    plan::{VulkanExecutionPlan, VulkanPlanCommand, VulkanShaderModule},
    resource_validation, vk_convert,
};

fn creation_checkpoint(stage: &str) {
    if std::env::var_os("EXOSPHERE_VULKAN_DIAGNOSTICS").is_some() {
        eprintln!("Vulkan backend creation: {stage}");
    }
}

struct BufferObject {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: u64,
    usage: BufferUsage,
    content_fingerprint: Option<[u8; 32]>,
}

/// Owns a not-yet-published buffer through every fallible creation stage.
struct PendingBufferAllocation {
    device: Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: bool,
}

impl Drop for PendingBufferAllocation {
    fn drop(&mut self) {
        unsafe {
            if self.mapped {
                self.device.unmap_memory(self.memory);
            }
            // Destroy the bound object before releasing its backing allocation.
            if self.buffer != vk::Buffer::null() {
                self.device.destroy_buffer(self.buffer, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}

/// A failed host wait does not prove that a submitted upload has retired.
/// Keep its command, fence and staging together until device teardown waits idle.
struct RetainedTextureUpload {
    command: vk::CommandBuffer,
    fence: vk::Fence,
    _staging: Option<PendingBufferAllocation>,
}
/// Releases partial image creation on every early return, in Vulkan lifetime order.
struct PendingImageAllocation {
    device: Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

impl Drop for PendingImageAllocation {
    fn drop(&mut self) {
        unsafe {
            if self.view != vk::ImageView::null() {
                self.device.destroy_image_view(self.view, None);
            }
            if self.image != vk::Image::null() {
                self.device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}

struct ImageObject {
    usage: vk::ImageUsageFlags,
    descriptor: TextureDesc,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    format: vk::Format,
    extent: vk::Extent3D,
    aspect: vk::ImageAspectFlags,
    mip_levels: u32,
    layers: u32,
    samples: vk::SampleCountFlags,
    layout: vk::ImageLayout,
}
struct SamplerObject {
    sampler: vk::Sampler,
    descriptor: SamplerDesc,
}
struct PipelineLayoutObject {
    layout: vk::PipelineLayout,
    set_layouts: Vec<vk::DescriptorSetLayout>,
}
struct QueryPoolObject {
    pool: vk::QueryPool,
    kind: QueryKind,
    capacity: u32,
}

const FRAME_CONTEXT_COUNT: usize = 3;

struct FrameContext {
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    descriptor_pools: Vec<vk::DescriptorPool>,
    last_timeline_value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GraphicsPipelineKey {
    pipeline: u64,
    color_formats: Vec<i32>,
    depth_format: i32,
    stencil_format: i32,
}

#[derive(Debug, Clone, Copy)]
struct BufferDescriptorBinding {
    resource: ResourceId,
    offset: u64,
    range: u64,
}

#[derive(Debug, Default)]
struct DescriptorState {
    buffers: BTreeMap<(u32, u32, u32), BufferDescriptorBinding>,
    images: BTreeMap<(u32, u32, u32), ResourceId>,
    samplers: BTreeMap<(u32, u32, u32), ResourceId>,
    legacy_textures: BTreeMap<u32, ResourceId>,
    legacy_samplers: BTreeMap<u32, ResourceId>,
}

#[derive(Debug, Default)]
struct AttachmentState {
    colors: BTreeMap<u32, ResourceId>,
    depth_stencil: Option<ResourceId>,
    active: bool,
}

/// Real Vulkan 1.3 backend.
pub struct AshVulkanDevice {
    /// Keep the dynamically loaded Vulkan library alive until all device objects are destroyed.
    _entry: Entry,
    instance: Instance,
    physical: vk::PhysicalDevice,
    device: Device,
    queue: vk::Queue,
    queue_family: u32,
    memory: vk::PhysicalDeviceMemoryProperties,
    command_pool: vk::CommandPool,
    frames: Vec<FrameContext>,
    frame_cursor: usize,
    submission_failed: bool,
    retained_texture_uploads: Vec<RetainedTextureUpload>,
    submit_timeline: vk::Semaphore,
    next_timeline_value: u64,
    buffers: BTreeMap<ResourceId, BufferObject>,
    images: BTreeMap<ResourceId, ImageObject>,
    samplers: BTreeMap<ResourceId, SamplerObject>,
    graphics_pipelines: BTreeMap<GraphicsPipelineKey, vk::Pipeline>,
    graphics_pipeline_descriptors: BTreeMap<u64, exosphere_gpu_ir::GraphicsPipelineDesc>,
    compute_pipelines: BTreeMap<u64, vk::Pipeline>,
    pipeline_layouts: BTreeMap<u128, PipelineLayoutObject>,
    shader_modules: BTreeMap<u64, vk::ShaderModule>,
    shader_module_descriptors: BTreeMap<u64, VulkanShaderModule>,
    query_pools: BTreeMap<ResourceId, QueryPoolObject>,
    pipeline_cache: vk::PipelineCache,
    pending_writebacks: BTreeMap<u64, Vec<ResourceId>>,
    guest_semaphores: BTreeMap<u64, u32>,
    guest_syncpoints: BTreeMap<u32, u32>,
    conditional_rendering: Option<ash::ext::conditional_rendering::Device>,
    transform_feedback: Option<ash::ext::transform_feedback::Device>,
    core_features: vk::PhysicalDeviceFeatures,
    index_type_uint8: bool,
}

unsafe impl Send for AshVulkanDevice {}

impl AshVulkanDevice {
    pub fn entry(&self) -> &Entry {
        &self._entry
    }
    pub fn instance(&self) -> &Instance {
        &self.instance
    }
    pub fn device(&self) -> &Device {
        &self.device
    }
    pub fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical
    }
    pub fn queue_family_index(&self) -> u32 {
        self.queue_family
    }
    /// Creates a Vulkan 1.3 device with dynamic rendering and synchronization2 enabled.
    pub fn create() -> Result<Self, String> {
        Self::create_with_pipeline_cache(&[])
    }

    /// Creates the backend and seeds the driver PSO cache with bytes persisted by the runtime.
    pub fn create_with_pipeline_cache(initial_cache: &[u8]) -> Result<Self, String> {
        Self::create_with_extensions(initial_cache, &[], &[])
    }

    /// Enable platform-selected instance/device extensions (for example surface
    /// and swapchain support). Extension names remain borrowed during creation;
    /// callers supply C strings rather than potentially dangling raw pointers.
    /// This does not create a surface or prove queue presentation support.
    pub fn create_with_extensions(
        initial_cache: &[u8],
        instance_extensions: &[&std::ffi::CStr],
        device_extensions: &[&std::ffi::CStr],
    ) -> Result<Self, String> {
        let instance_names: std::collections::BTreeSet<_> =
            instance_extensions.iter().copied().collect();
        let instance_names: Vec<_> = instance_names.iter().map(|name| name.as_ptr()).collect();
        let entry = unsafe { Entry::load() }.map_err(|e| format!("load Vulkan: {e}"))?;
        let app_name = CString::new("Exosphere").unwrap();
        let engine_name = CString::new("Exosphere").unwrap();
        let app = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .engine_name(&engine_name)
            .api_version(vk::API_VERSION_1_3);
        let instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app)
            .enabled_extension_names(&instance_names);
        let instance = unsafe { entry.create_instance(&instance_info, None) }
            .map_err(|e| format!("vkCreateInstance: {e:?}"))?;
        creation_checkpoint("enumerating physical devices");
        let physicals = unsafe { instance.enumerate_physical_devices() }
            .map_err(|e| format!("vkEnumeratePhysicalDevices: {e:?}"))?;
        let mut chosen = None;
        for physical in physicals {
            let families =
                unsafe { instance.get_physical_device_queue_family_properties(physical) };
            if let Some((index, _)) = families.iter().enumerate().find(|(_, f)| {
                f.queue_count > 0
                    && f.queue_flags
                        .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
            }) {
                chosen = Some((physical, index as u32));
                break;
            }
        }
        let (physical, queue_family) =
            chosen.ok_or_else(|| "no Vulkan graphics+compute queue family".to_owned())?;
        let core_features = unsafe { instance.get_physical_device_features(physical) };
        creation_checkpoint("enumerating device extensions");
        let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
            .map_err(|e| format!("vkEnumerateDeviceExtensionProperties: {e:?}"))?;
        let has_ext = |name: &std::ffi::CStr| {
            extensions
                .iter()
                .any(|p| unsafe { std::ffi::CStr::from_ptr(p.extension_name.as_ptr()) } == name)
        };
        let mut conditional_features = vk::PhysicalDeviceConditionalRenderingFeaturesEXT::default();
        let mut xfb_features = vk::PhysicalDeviceTransformFeedbackFeaturesEXT::default();
        let mut supported_v12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut supported_v13 = vk::PhysicalDeviceVulkan13Features::default();
        let has_uint8_extension = has_ext(ash::ext::index_type_uint8::NAME);
        let mut uint8_features = vk::PhysicalDeviceIndexTypeUint8FeaturesEXT::default();
        let wants_swapchain_maintenance =
            device_extensions.contains(&ash::ext::swapchain_maintenance1::NAME);
        let mut swapchain_maintenance =
            vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default();
        let mut features2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut supported_v12)
            .push_next(&mut supported_v13)
            .push_next(&mut conditional_features)
            .push_next(&mut xfb_features);
        if wants_swapchain_maintenance {
            features2 = features2.push_next(&mut swapchain_maintenance);
        }
        if has_uint8_extension {
            features2 = features2.push_next(&mut uint8_features);
        }
        creation_checkpoint("querying required and optional features");
        unsafe { instance.get_physical_device_features2(physical, &mut features2) };
        creation_checkpoint("features queried");
        if wants_swapchain_maintenance && swapchain_maintenance.swapchain_maintenance1 != vk::TRUE {
            unsafe { instance.destroy_instance(None) };
            return Err("Vulkan device lacks requested swapchain maintenance feature".into());
        }
        if supported_v12.timeline_semaphore != vk::TRUE
            || supported_v13.dynamic_rendering != vk::TRUE
            || supported_v13.synchronization2 != vk::TRUE
        {
            unsafe { instance.destroy_instance(None) };
            return Err(
                "Vulkan device lacks required timeline semaphores, dynamic rendering or synchronization2"
                    .into(),
            );
        }
        let has_conditional = has_ext(ash::ext::conditional_rendering::NAME)
            && conditional_features.conditional_rendering == vk::TRUE;
        let has_xfb = has_ext(ash::ext::transform_feedback::NAME)
            && xfb_features.transform_feedback == vk::TRUE;
        let index_type_uint8 = has_uint8_extension && uint8_features.index_type_uint8 == vk::TRUE;
        let mut enabled_names: std::collections::BTreeSet<_> =
            device_extensions.iter().copied().collect();
        for name in &enabled_names {
            if !has_ext(name) {
                unsafe { instance.destroy_instance(None) };
                return Err(format!(
                    "required Vulkan device extension unavailable: {}",
                    name.to_string_lossy()
                ));
            }
        }
        if has_conditional {
            enabled_names.insert(ash::ext::conditional_rendering::NAME);
        }
        if has_xfb {
            enabled_names.insert(ash::ext::transform_feedback::NAME);
        }
        if index_type_uint8 {
            enabled_names.insert(ash::ext::index_type_uint8::NAME);
        }
        let extension_names: Vec<_> = enabled_names.iter().map(|name| name.as_ptr()).collect();
        let priority = [1.0f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priority)];
        let mut v13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        // Core version availability does not automatically enable its optional
        // features. The submission timeline requires explicit feature admission.
        let mut v12 = vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true);
        // The query chain linked these structs to each other and to the queried
        // core structs. Reusing those links in a new push_next chain can create
        // cycles; only feature values, not query-chain pointers, are retained.
        conditional_features.p_next = ptr::null_mut();
        xfb_features.p_next = ptr::null_mut();
        conditional_features.conditional_rendering =
            if has_conditional { vk::TRUE } else { vk::FALSE };
        conditional_features.inherited_conditional_rendering = vk::FALSE;
        xfb_features.transform_feedback = if has_xfb { vk::TRUE } else { vk::FALSE };
        xfb_features.geometry_streams = vk::FALSE;
        let requested_core = vk::PhysicalDeviceFeatures::default()
            .texture_compression_bc(core_features.texture_compression_bc == vk::TRUE)
            .texture_compression_astc_ldr(core_features.texture_compression_astc_ldr == vk::TRUE)
            .sampler_anisotropy(core_features.sampler_anisotropy == vk::TRUE)
            .depth_clamp(core_features.depth_clamp == vk::TRUE)
            .depth_bias_clamp(core_features.depth_bias_clamp == vk::TRUE)
            .fill_mode_non_solid(core_features.fill_mode_non_solid == vk::TRUE)
            .logic_op(core_features.logic_op == vk::TRUE)
            .alpha_to_one(core_features.alpha_to_one == vk::TRUE)
            .geometry_shader(core_features.geometry_shader == vk::TRUE)
            .multi_draw_indirect(core_features.multi_draw_indirect == vk::TRUE)
            .tessellation_shader(core_features.tessellation_shader == vk::TRUE);
        let mut device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&extension_names)
            .enabled_features(&requested_core)
            .push_next(&mut v12)
            .push_next(&mut v13);
        if has_conditional {
            device_info = device_info.push_next(&mut conditional_features);
        }
        if has_xfb {
            device_info = device_info.push_next(&mut xfb_features);
        }
        if wants_swapchain_maintenance {
            // Use a fresh enablement struct rather than retaining query-chain links.
            swapchain_maintenance = vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default()
                .swapchain_maintenance1(true);
            device_info = device_info.push_next(&mut swapchain_maintenance);
        }
        if index_type_uint8 {
            // Fresh feature state avoids carrying links from the query chain.
            uint8_features =
                vk::PhysicalDeviceIndexTypeUint8FeaturesEXT::default().index_type_uint8(true);
            device_info = device_info.push_next(&mut uint8_features);
        }
        creation_checkpoint("creating logical device");
        let device = unsafe { instance.create_device(physical, &device_info, None) }
            .map_err(|e| format!("vkCreateDevice: {e:?}"))?;
        creation_checkpoint("logical device created");
        let conditional_rendering = has_conditional
            .then(|| ash::ext::conditional_rendering::Device::new(&instance, &device));
        let transform_feedback =
            has_xfb.then(|| ash::ext::transform_feedback::Device::new(&instance, &device));
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        let memory = unsafe { instance.get_physical_device_memory_properties(physical) };
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // Kept for tiny helper submissions (for example one-shot image uploads). Main plan execution
        // uses the reusable frame contexts below.
        let command_pool = unsafe { device.create_command_pool(&pool_info, None) }
            .map_err(|e| format!("vkCreateCommandPool: {e:?}"))?;
        let semaphore_type = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(0);
        let mut semaphore_type = semaphore_type;
        let semaphore_info = vk::SemaphoreCreateInfo::default().push_next(&mut semaphore_type);
        let submit_timeline = unsafe { device.create_semaphore(&semaphore_info, None) }
            .map_err(|e| format!("vkCreateSemaphore(timeline): {e:?}"))?;
        let pipeline_cache_info =
            vk::PipelineCacheCreateInfo::default().initial_data(initial_cache);
        let pipeline_cache = unsafe { device.create_pipeline_cache(&pipeline_cache_info, None) }
            .map_err(|e| format!("vkCreatePipelineCache: {e:?}"))?;
        let mut frames = Vec::with_capacity(FRAME_CONTEXT_COUNT);
        for _ in 0..FRAME_CONTEXT_COUNT {
            let frame_pool = unsafe { device.create_command_pool(&pool_info, None) }
                .map_err(|e| format!("vkCreateCommandPool(frame): {e:?}"))?;
            let alloc = vk::CommandBufferAllocateInfo::default()
                .command_pool(frame_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let command_buffer = unsafe { device.allocate_command_buffers(&alloc) }
                .map_err(|e| format!("vkAllocateCommandBuffers(frame): {e:?}"))?[0];
            let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
            let fence = unsafe { device.create_fence(&fence_info, None) }
                .map_err(|e| format!("vkCreateFence(frame): {e:?}"))?;
            frames.push(FrameContext {
                command_pool: frame_pool,
                command_buffer,
                fence,
                descriptor_pools: Vec::new(),
                last_timeline_value: 0,
            });
        }
        Ok(Self {
            _entry: entry,
            instance,
            physical,
            device,
            queue,
            queue_family,
            memory,
            command_pool,
            frames,
            frame_cursor: 0,
            submission_failed: false,
            retained_texture_uploads: Vec::new(),
            submit_timeline,
            next_timeline_value: 1,
            buffers: BTreeMap::new(),
            images: BTreeMap::new(),
            samplers: BTreeMap::new(),
            graphics_pipelines: BTreeMap::new(),
            graphics_pipeline_descriptors: BTreeMap::new(),
            compute_pipelines: BTreeMap::new(),
            pipeline_layouts: BTreeMap::new(),
            shader_modules: BTreeMap::new(),
            shader_module_descriptors: BTreeMap::new(),
            query_pools: BTreeMap::new(),
            pipeline_cache,
            pending_writebacks: BTreeMap::new(),
            guest_semaphores: BTreeMap::new(),
            guest_syncpoints: BTreeMap::new(),
            conditional_rendering,
            transform_feedback,
            core_features,
            index_type_uint8,
        })
    }

    /// Returns the latest value completed by the internal Vulkan timeline.
    pub fn completed_timeline_value(&self) -> Result<u64, VulkanError> {
        unsafe {
            self.device
                .get_semaphore_counter_value(self.submit_timeline)
        }
        .map_err(|e| VulkanError::Driver(format!("vkGetSemaphoreCounterValue: {e:?}")))
    }

    /// Non-blocking completion test for a previously submitted timeline value.
    pub fn is_timeline_complete(&self, value: u64) -> Result<bool, VulkanError> {
        Ok(self.completed_timeline_value()? >= value)
    }

    /// Waits for a specific internal timeline value without stalling unrelated queues.
    pub fn wait_timeline(&self, value: u64, timeout_ns: u64) -> Result<(), VulkanError> {
        let semaphores = [self.submit_timeline];
        let values = [value];
        let info = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        unsafe { self.device.wait_semaphores(&info, timeout_ns) }
            .map_err(|e| VulkanError::Driver(format!("vkWaitSemaphores: {e:?}")))
    }

    /// Serializes the driver pipeline cache so the platform/runtime layer can persist it.
    pub fn pipeline_cache_data(&self) -> Result<Vec<u8>, VulkanError> {
        unsafe { self.device.get_pipeline_cache_data(self.pipeline_cache) }
            .map_err(|e| VulkanError::Driver(format!("vkGetPipelineCacheData: {e:?}")))
    }

    fn ensure_query_pool(
        &mut self,
        id: ResourceId,
        kind: QueryKind,
        required: u32,
    ) -> Result<vk::QueryPool, VulkanError> {
        let required = required.max(1);
        if let Some(q) = self.query_pools.get(&id) {
            if q.kind != kind {
                return Err(VulkanError::UnsupportedFeature(
                    "query pool reused with a different query kind",
                ));
            }
            if q.capacity >= required {
                return Ok(q.pool);
            }
        }
        if let Some(old) = self.query_pools.remove(&id) {
            unsafe { self.device.destroy_query_pool(old.pool, None) }
        }
        let (query_type, statistics) = match kind {
            QueryKind::Occlusion => (
                vk::QueryType::OCCLUSION,
                vk::QueryPipelineStatisticFlags::empty(),
            ),
            QueryKind::Timestamp => (
                vk::QueryType::TIMESTAMP,
                vk::QueryPipelineStatisticFlags::empty(),
            ),
            QueryKind::PipelineStatistics => (
                vk::QueryType::PIPELINE_STATISTICS,
                vk::QueryPipelineStatisticFlags::INPUT_ASSEMBLY_VERTICES
                    | vk::QueryPipelineStatisticFlags::INPUT_ASSEMBLY_PRIMITIVES
                    | vk::QueryPipelineStatisticFlags::VERTEX_SHADER_INVOCATIONS
                    | vk::QueryPipelineStatisticFlags::CLIPPING_INVOCATIONS
                    | vk::QueryPipelineStatisticFlags::CLIPPING_PRIMITIVES
                    | vk::QueryPipelineStatisticFlags::FRAGMENT_SHADER_INVOCATIONS
                    | vk::QueryPipelineStatisticFlags::COMPUTE_SHADER_INVOCATIONS,
            ),
            QueryKind::TransformFeedback => {
                if self.transform_feedback.is_none() {
                    return Err(VulkanError::UnsupportedFeature("VK_EXT_transform_feedback"));
                }
                (
                    vk::QueryType::TRANSFORM_FEEDBACK_STREAM_EXT,
                    vk::QueryPipelineStatisticFlags::empty(),
                )
            }
            QueryKind::Unknown(_) => {
                return Err(VulkanError::UnsupportedFeature("unknown query kind"));
            }
        };
        let info = vk::QueryPoolCreateInfo::default()
            .query_type(query_type)
            .query_count(required)
            .pipeline_statistics(statistics);
        let pool = unsafe { self.device.create_query_pool(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreateQueryPool: {e:?}")))?;
        self.query_pools.insert(
            id,
            QueryPoolObject {
                pool,
                kind,
                capacity: required,
            },
        );
        Ok(pool)
    }

    fn memory_type(
        &self,
        bits: u32,
        required: vk::MemoryPropertyFlags,
        preferred: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        let mut fallback = None;
        for i in 0..self.memory.memory_type_count {
            if bits & (1 << i) == 0 {
                continue;
            }
            let flags = self.memory.memory_types[i as usize].property_flags;
            if flags.contains(required | preferred) {
                return Some(i);
            }
            if fallback.is_none() && flags.contains(required) {
                fallback = Some(i)
            }
        }
        fallback
    }

    fn buffer_usage(&self, desc: &BufferDesc) -> vk::BufferUsageFlags {
        let mut usage = vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST;
        usage |= match desc.usage {
            BufferUsage::Vertex => vk::BufferUsageFlags::VERTEX_BUFFER,
            BufferUsage::VertexUniform => {
                vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::UNIFORM_BUFFER
            }
            BufferUsage::Index => vk::BufferUsageFlags::INDEX_BUFFER,
            BufferUsage::IndexVertexUniform => {
                vk::BufferUsageFlags::INDEX_BUFFER
                    | vk::BufferUsageFlags::VERTEX_BUFFER
                    | vk::BufferUsageFlags::UNIFORM_BUFFER
            }
            BufferUsage::Uniform => vk::BufferUsageFlags::UNIFORM_BUFFER,
            BufferUsage::Storage => vk::BufferUsageFlags::STORAGE_BUFFER,
            BufferUsage::Indirect => vk::BufferUsageFlags::INDIRECT_BUFFER,
            BufferUsage::Query => vk::BufferUsageFlags::TRANSFER_DST,
            BufferUsage::TransformFeedback => {
                if self.transform_feedback.is_some() {
                    vk::BufferUsageFlags::TRANSFORM_FEEDBACK_BUFFER_EXT
                } else {
                    vk::BufferUsageFlags::empty()
                }
            }
            BufferUsage::TransferSource => vk::BufferUsageFlags::TRANSFER_SRC,
            BufferUsage::TransferDestination => vk::BufferUsageFlags::TRANSFER_DST,
        };
        if self.conditional_rendering.is_some() {
            usage |= vk::BufferUsageFlags::CONDITIONAL_RENDERING_EXT
        }
        usage
    }

    fn ensure_buffer(
        &mut self,
        desc: &BufferDesc,
        provider: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        if let Some(existing) = self.buffers.get(&desc.id) {
            // Resource identity must retain its allocation contract. Silently
            // accepting changed size/usage can bind stale or undersized memory.
            if existing.size != desc.size || existing.usage != desc.usage {
                return Err(VulkanError::UnsupportedFeature(
                    "cached buffer descriptor changed",
                ));
            }
            if matches!(
                desc.usage,
                BufferUsage::Index
                    | BufferUsage::Vertex
                    | BufferUsage::Indirect
                    | BufferUsage::Uniform
                    | BufferUsage::VertexUniform
                    | BufferUsage::IndexVertexUniform
            ) {
                let fingerprint = provider
                    .content_fingerprint(desc.id, 0, desc.size)
                    .map_err(|error| VulkanError::ResourceData(error.to_string()))?;
                if fingerprint.is_some() && fingerprint != existing.content_fingerprint {
                    self.refresh_input_buffer(desc.id, provider)?;
                }
            }
            return Ok(());
        }
        let source_len = if matches!(
            desc.usage,
            BufferUsage::Index
                | BufferUsage::Vertex
                | BufferUsage::Indirect
                | BufferUsage::Uniform
                | BufferUsage::VertexUniform
                | BufferUsage::IndexVertexUniform
        ) {
            if desc.size > 64 * 1024 * 1024 {
                return Err(VulkanError::UnsupportedFeature(
                    "draw input upload allocation budget",
                ));
            }
            let available = provider
                .len(desc.id)
                .map_err(|error| VulkanError::ResourceData(error.to_string()))?;
            if available < desc.size {
                return Err(VulkanError::UnsupportedFeature(
                    "draw input upload provider too small",
                ));
            }
            desc.size
        } else {
            provider.len(desc.id).unwrap_or(0).min(desc.size)
        };
        // Read before creating native objects so provider failures cannot leave
        // partially allocated buffers or admit missing index bytes as zeros.
        let mut source = vec![0; source_len as usize];
        if !source.is_empty() {
            provider
                .read(desc.id, 0, &mut source)
                .map_err(|error| VulkanError::ResourceData(error.to_string()))?;
        }
        let info = vk::BufferCreateInfo::default()
            .size(desc.size.max(4))
            .usage(self.buffer_usage(desc))
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { self.device.create_buffer(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreateBuffer: {e:?}")))?;
        let mut allocation = PendingBufferAllocation {
            device: self.device.clone(),
            buffer,
            memory: vk::DeviceMemory::null(),
            mapped: false,
        };
        let req = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let ty = self
            .memory_type(
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE,
                vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or_else(|| VulkanError::Driver("no host-visible memory type".into()))?;
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(ty);
        let memory = unsafe { self.device.allocate_memory(&alloc, None) }
            .map_err(|e| VulkanError::Driver(format!("vkAllocateMemory: {e:?}")))?;
        allocation.memory = memory;
        unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|e| VulkanError::Driver(format!("vkBindBufferMemory: {e:?}")))?;
        if !source.is_empty() {
            let mapped = unsafe {
                self.device
                    .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            }
            .map_err(|e| VulkanError::Driver(format!("vkMapMemory: {e:?}")))?;
            allocation.mapped = true;
            unsafe {
                ptr::copy_nonoverlapping(source.as_ptr(), mapped.cast::<u8>(), source.len());
            };
            let range = vk::MappedMemoryRange::default()
                .memory(memory)
                .offset(0)
                .size(vk::WHOLE_SIZE);
            let flushed = unsafe { self.device.flush_mapped_memory_ranges(&[range]) };
            unsafe { self.device.unmap_memory(memory) };
            allocation.mapped = false;
            if let Err(error) = flushed {
                return Err(VulkanError::Driver(format!(
                    "vkFlushMappedMemoryRanges(buffer upload): {error:?}"
                )));
            }
        }
        self.buffers.insert(
            desc.id,
            BufferObject {
                buffer,
                memory,
                size: desc.size,
                usage: desc.usage,
                content_fingerprint: matches!(
                    desc.usage,
                    BufferUsage::Index
                        | BufferUsage::Vertex
                        | BufferUsage::Indirect
                        | BufferUsage::Uniform
                        | BufferUsage::VertexUniform
                        | BufferUsage::IndexVertexUniform
                )
                .then(|| *blake3::hash(&source).as_bytes()),
            },
        );
        // Ownership has moved into the cache only after complete initialization.
        allocation.buffer = vk::Buffer::null();
        allocation.memory = vk::DeviceMemory::null();
        Ok(())
    }

    /// Explicitly refresh a cached index buffer after guest memory changes.
    /// This serializes outstanding device work; it does not yet implement an
    /// asynchronous guest-memory dirty-range tracker.
    pub fn refresh_index_buffer(
        &mut self,
        id: ResourceId,
        provider: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        let buffer = self
            .buffers
            .get(&id)
            .ok_or(VulkanError::MissingResource(id.0))?;
        if !buffer.usage.supports_index() {
            return Err(VulkanError::UnsupportedFeature(
                "index refresh requires an index buffer",
            ));
        }
        self.refresh_input_buffer(id, provider)
    }

    /// Refresh CPU-supplied draw inputs only after prior device work retires.
    /// GPU-produced storage/output buffers retain their writeback ownership.
    fn refresh_input_buffer(
        &mut self,
        id: ResourceId,
        provider: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        if self.submission_failed {
            return Err(VulkanError::UnsupportedFeature(
                "buffer refresh on stopped device",
            ));
        }
        let buffer = self
            .buffers
            .get(&id)
            .ok_or(VulkanError::MissingResource(id.0))?;
        if !matches!(
            buffer.usage,
            BufferUsage::Index
                | BufferUsage::Vertex
                | BufferUsage::Indirect
                | BufferUsage::Uniform
                | BufferUsage::VertexUniform
                | BufferUsage::IndexVertexUniform
        ) || self
            .pending_writebacks
            .values()
            .any(|ids| ids.contains(&id))
        {
            return Err(VulkanError::UnsupportedFeature(
                "draw input refresh requires a buffer without pending writeback",
            ));
        }
        if buffer.size == 0 || buffer.size > 64 * 1024 * 1024 {
            return Err(VulkanError::UnsupportedFeature(
                "draw input refresh allocation budget",
            ));
        }
        if provider
            .len(id)
            .map_err(|error| VulkanError::ResourceData(error.to_string()))?
            < buffer.size
        {
            return Err(VulkanError::UnsupportedFeature(
                "draw input refresh provider too small",
            ));
        }
        let mut bytes = vec![0; buffer.size as usize];
        provider
            .read(id, 0, &mut bytes)
            .map_err(|error| VulkanError::ResourceData(error.to_string()))?;
        // Cached buffers may be referenced by any queued frame, not just the
        // next reusable frame context. Retire all work before host writes.
        unsafe { self.device.device_wait_idle() }.map_err(|error| {
            VulkanError::Driver(format!("vkDeviceWaitIdle(draw input refresh): {error:?}"))
        })?;
        let mapped = unsafe {
            self.device.map_memory(
                buffer.memory,
                0,
                vk::WHOLE_SIZE,
                vk::MemoryMapFlags::empty(),
            )
        }
        .map_err(|error| {
            VulkanError::Driver(format!("vkMapMemory(draw input refresh): {error:?}"))
        })?;
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len()) };
        // HOST_COHERENT is preferred, not guaranteed by memory-type selection.
        // Whole-allocation flush avoids noncoherent atom-size alignment issues.
        let range = vk::MappedMemoryRange::default()
            .memory(buffer.memory)
            .offset(0)
            .size(vk::WHOLE_SIZE);
        let flushed = unsafe { self.device.flush_mapped_memory_ranges(&[range]) };
        unsafe { self.device.unmap_memory(buffer.memory) };
        flushed.map_err(|error| {
            VulkanError::Driver(format!(
                "vkFlushMappedMemoryRanges(draw input refresh): {error:?}"
            ))
        })?;
        // Hash the bytes actually uploaded, not a separately queried provider
        // value which might have changed between fingerprinting and reading.
        self.buffers
            .get_mut(&id)
            .expect("refreshed buffer exists")
            .content_fingerprint = Some(*blake3::hash(&bytes).as_bytes());
        Ok(())
    }

    fn ensure_image(
        &mut self,
        desc: &TextureDesc,
        provider: &mut dyn GpuResourceData,
        refresh_input: bool,
    ) -> Result<(), VulkanError> {
        image_usage::validate_shape(desc)?;
        if let Some(existing) = self.images.get(&desc.id) {
            // Guest linear/block-linear layout describes payload conversion,
            // not VkImage tiling (always optimal). A host initialization may
            // use linear pixels before the same image receives a tiled guest
            // descriptor. Shape/format/sample changes still require a new ID.
            let mut native_descriptor = desc.clone();
            native_descriptor.layout = existing.descriptor.layout;
            if existing.descriptor != native_descriptor {
                return Err(VulkanError::UnsupportedFeature("image identity changed"));
            }
            if refresh_input {
                if self
                    .pending_writebacks
                    .values()
                    .any(|ids| ids.contains(&desc.id))
                {
                    return Err(VulkanError::UnsupportedFeature(
                        "texture refresh with pending writeback",
                    ));
                }
                // The staging upload waits on this queue before returning;
                // previous device reads retire before their input is replaced.
                self.upload_image_payload(desc, provider, true)?;
            }
            return Ok(());
        }
        let format = vk_convert::texture_format(desc.format)
            .ok_or(VulkanError::UnsupportedFeature("texture format"))?;
        image_usage::validate_compression(format, &self.core_features)?;
        let properties = unsafe {
            self.instance
                .get_physical_device_format_properties(self.physical, format)
        };
        let usage = image_usage::supported(properties.optimal_tiling_features)?;
        let extent = vk::Extent3D {
            width: desc.width,
            height: desc.height,
            depth: desc.depth,
        };
        let info = vk::ImageCreateInfo::default()
            .image_type(if desc.depth > 1 {
                vk::ImageType::TYPE_3D
            } else {
                vk::ImageType::TYPE_2D
            })
            .format(format)
            .extent(extent)
            .mip_levels(desc.mip_levels)
            .array_layers(desc.layers)
            .samples(Self::sample_count(desc.samples)?)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let limits = unsafe {
            self.instance.get_physical_device_image_format_properties(
                self.physical,
                format,
                info.image_type,
                info.tiling,
                usage,
                info.flags,
            )
        }
        .map_err(|_| VulkanError::UnsupportedFeature("image format/usage combination"))?;
        if extent.width > limits.max_extent.width
            || extent.height > limits.max_extent.height
            || extent.depth > limits.max_extent.depth
            || info.mip_levels > limits.max_mip_levels
            || info.array_layers > limits.max_array_layers
            || !limits.sample_counts.contains(info.samples)
        {
            return Err(VulkanError::UnsupportedFeature("image format limits"));
        }
        let image = unsafe { self.device.create_image(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreateImage: {e:?}")))?;
        let mut pending = PendingImageAllocation {
            device: self.device.clone(),
            image,
            memory: vk::DeviceMemory::null(),
            view: vk::ImageView::null(),
        };
        let req = unsafe { self.device.get_image_memory_requirements(image) };
        let ty = self
            .memory_type(
                req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                vk::MemoryPropertyFlags::empty(),
            )
            .ok_or_else(|| VulkanError::Driver("no device-local image memory type".into()))?;
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(ty);
        let memory = unsafe { self.device.allocate_memory(&alloc, None) }
            .map_err(|e| VulkanError::Driver(format!("vkAllocateMemory(image): {e:?}")))?;
        pending.memory = memory;
        unsafe { self.device.bind_image_memory(image, memory, 0) }
            .map_err(|e| VulkanError::Driver(format!("vkBindImageMemory: {e:?}")))?;
        let depth = format == vk::Format::D16_UNORM || format == vk::Format::D32_SFLOAT;
        let ds =
            format == vk::Format::D24_UNORM_S8_UINT || format == vk::Format::D32_SFLOAT_S8_UINT;
        let aspect = if ds {
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL
        } else if depth {
            vk::ImageAspectFlags::DEPTH
        } else {
            vk::ImageAspectFlags::COLOR
        };
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(if desc.depth > 1 {
                vk::ImageViewType::TYPE_3D
            } else if desc.layers > 1 {
                vk::ImageViewType::TYPE_2D_ARRAY
            } else {
                vk::ImageViewType::TYPE_2D
            })
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: aspect,
                base_mip_level: 0,
                level_count: desc.mip_levels,
                base_array_layer: 0,
                layer_count: desc.layers,
            });
        let view = unsafe { self.device.create_image_view(&view_info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreateImageView: {e:?}")))?;
        pending.view = view;
        self.images.insert(
            desc.id,
            ImageObject {
                usage,
                descriptor: desc.clone(),
                image,
                memory,
                view,
                format,
                extent,
                aspect,
                mip_levels: desc.mip_levels,
                layers: desc.layers,
                samples: info.samples,
                layout: vk::ImageLayout::UNDEFINED,
            },
        );
        // Ownership transfers to the device cache only once all native handles
        // exist. Dropping the guard must not release the published image.
        pending.view = vk::ImageView::null();
        pending.image = vk::Image::null();
        pending.memory = vk::DeviceMemory::null();
        if let Err(error) = self.upload_image_payload(desc, provider, refresh_input) {
            // A newly created image has no previous valid contents to preserve.
            // Roll it back on ordinary payload/preparation errors so a retry
            // cannot mistake partial initialization for a cached resource.
            // Uncertain submissions must retain the image until device teardown.
            if !self.submission_failed {
                if let Some(image) = self.images.remove(&desc.id) {
                    drop(PendingImageAllocation {
                        device: self.device.clone(),
                        image: image.image,
                        memory: image.memory,
                        view: image.view,
                    });
                }
            }
            return Err(error);
        }
        Ok(())
    }

    fn upload_image_payload(
        &mut self,
        desc: &TextureDesc,
        provider: &mut dyn GpuResourceData,
        required: bool,
    ) -> Result<(), VulkanError> {
        // Upload guest bytes through staging. Tegra block-linear mip0/layers are deswizzled here;
        // multi-mip payload addressing stays explicit until the frontend supplies per-mip offsets.
        let len = texture_payload::upload_length(provider.len(desc.id), required)?;
        if len > 0 {
            if desc.samples > 1 {
                return Err(VulkanError::UnsupportedFeature(
                    "initial upload to multisampled image",
                ));
            }
            let mut guest = Vec::new();
            guest.try_reserve_exact(len).map_err(|_| {
                VulkanError::ResourceData("texture staging allocation failed".into())
            })?;
            guest.resize(len, 0);
            provider
                .read(desc.id, 0, &mut guest)
                .map_err(|e| VulkanError::ResourceData(e.to_string()))?;
            let upload = match desc.layout {
                TextureLayout::Linear => guest,
                TextureLayout::BlockLinear {
                    block_height_log2,
                    block_depth_log2,
                    ..
                } => {
                    if desc.mip_levels > 1 {
                        return Err(VulkanError::UnsupportedFeature(
                            "block-linear multi-mip payload requires explicit mip offsets",
                        ));
                    }
                    format_info(desc.format).ok_or(VulkanError::UnsupportedFeature(
                        "block-linear texture format",
                    ))?;
                    let fp = linear_footprint(
                        desc.format,
                        desc.width,
                        desc.height,
                        desc.depth,
                        desc.layers,
                    )
                    .ok_or(VulkanError::UnsupportedFeature("block-linear footprint"))?;
                    let width_bytes = u32::try_from(fp.row_bytes).map_err(|_| {
                        VulkanError::UnsupportedFeature("block-linear row too large")
                    })?;
                    let height_rows = fp.blocks_y;
                    deswizzle_layers(
                        &guest,
                        width_bytes,
                        height_rows,
                        desc.depth,
                        desc.layers,
                        block_height_log2,
                        block_depth_log2,
                    )
                    .ok_or_else(|| {
                        VulkanError::ResourceData(
                            "block-linear payload is smaller than its padded Tegra footprint"
                                .into(),
                        )
                    })?
                }
                TextureLayout::Unknown(_) => {
                    return Err(VulkanError::UnsupportedFeature("unknown texture layout"));
                }
            };
            self.upload_image_bytes(desc, &upload)?;
        }
        Ok(())
    }

    fn ensure_sampler(&mut self, desc: &SamplerDesc) -> Result<(), VulkanError> {
        if let Some(existing) = self.samplers.get(&desc.id) {
            return if existing.descriptor == *desc {
                Ok(())
            } else {
                Err(VulkanError::UnsupportedFeature("sampler identity changed"))
            };
        }
        let limits = unsafe { self.instance.get_physical_device_properties(self.physical) }.limits;
        let info = sampler::create_info(
            &desc.state,
            &limits,
            self.core_features.sampler_anisotropy == vk::TRUE,
        )?;
        let sampler = unsafe { self.device.create_sampler(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreateSampler: {e:?}")))?;
        self.samplers.insert(
            desc.id,
            SamplerObject {
                sampler,
                descriptor: *desc,
            },
        );
        Ok(())
    }

    fn create_layout(
        &mut self,
        key: u128,
        layout: &PipelineLayoutDesc,
    ) -> Result<vk::PipelineLayout, VulkanError> {
        if let Some(existing) = self.pipeline_layouts.get(&key) {
            return Ok(existing.layout);
        }
        let mut set_layouts = Vec::new();
        let max_set = layout.descriptors.iter().map(|d| d.set).max();
        for set in 0..=max_set.unwrap_or(0) {
            if max_set.is_none() {
                break;
            }
            let bindings = layout
                .descriptors
                .iter()
                .filter(|d| d.set == set)
                .map(|d| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(d.binding)
                        .descriptor_type(vk_convert::descriptor_type(d.descriptor_type))
                        .descriptor_count(d.count.max(1))
                        .stage_flags(vk_convert::stage_mask(d.stages))
                })
                .collect::<Vec<_>>();
            let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
            let set_layout = unsafe { self.device.create_descriptor_set_layout(&info, None) }
                .map_err(|e| VulkanError::Driver(format!("vkCreateDescriptorSetLayout: {e:?}")))?;
            set_layouts.push(set_layout);
        }
        let push = layout
            .push_constants
            .iter()
            .map(|p| {
                vk::PushConstantRange::default()
                    .stage_flags(vk_convert::stage_mask(p.stages))
                    .offset(p.offset)
                    .size(p.size)
            })
            .collect::<Vec<_>>();
        let info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push);
        let pipeline_layout = unsafe { self.device.create_pipeline_layout(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreatePipelineLayout: {e:?}")))?;
        self.pipeline_layouts.insert(
            key,
            PipelineLayoutObject {
                layout: pipeline_layout,
                set_layouts,
            },
        );
        Ok(pipeline_layout)
    }

    fn begin_frame(&mut self) -> Result<(usize, vk::CommandBuffer), VulkanError> {
        let index = self.frame_cursor % self.frames.len();
        self.frame_cursor = (self.frame_cursor + 1) % self.frames.len();
        let frame = &mut self.frames[index];
        unsafe { self.device.wait_for_fences(&[frame.fence], true, u64::MAX) }
            .map_err(|e| VulkanError::Driver(format!("vkWaitForFences(frame): {e:?}")))?;
        unsafe {
            for pool in frame.descriptor_pools.drain(..) {
                self.device.destroy_descriptor_pool(pool, None)
            }
        }
        unsafe {
            self.device
                .reset_command_pool(frame.command_pool, vk::CommandPoolResetFlags::empty())
        }
        .map_err(|e| VulkanError::Driver(format!("vkResetCommandPool(frame): {e:?}")))?;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe {
            self.device
                .begin_command_buffer(frame.command_buffer, &begin)
        }
        .map_err(|e| VulkanError::Driver(format!("vkBeginCommandBuffer(frame): {e:?}")))?;
        Ok((index, frame.command_buffer))
    }

    fn submit_frame(&mut self, index: usize) -> Result<u64, VulkanError> {
        let frame = &mut self.frames[index];
        unsafe { self.device.end_command_buffer(frame.command_buffer) }
            .map_err(|e| VulkanError::Driver(format!("vkEndCommandBuffer(frame): {e:?}")))?;
        let value = self.next_timeline_value;
        self.next_timeline_value = self.next_timeline_value.saturating_add(1);
        let command_info =
            [vk::CommandBufferSubmitInfo::default().command_buffer(frame.command_buffer)];
        let signal_info = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.submit_timeline)
            .value(value)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let submit = [vk::SubmitInfo2::default()
            .command_buffer_infos(&command_info)
            .signal_semaphore_infos(&signal_info)];
        // Recording may fail without any submission. Keep the frame fence
        // signaled until actual work is about to use it, avoiding a later wait
        // for a submission that never happened.
        unsafe { self.device.reset_fences(&[frame.fence]) }
            .map_err(|e| VulkanError::Driver(format!("vkResetFences(frame): {e:?}")))?;
        if let Err(error) = unsafe { self.device.queue_submit2(self.queue, &submit, frame.fence) } {
            // The reset fence has no successful submission to signal it. Do not
            // permit another frame to wait on it; recovery needs a new device.
            self.submission_failed = true;
            return Err(VulkanError::Driver(format!(
                "vkQueueSubmit2(frame): {error:?}"
            )));
        }
        frame.last_timeline_value = value;
        Ok(value)
    }

    fn begin_commands(&self) -> Result<vk::CommandBuffer, VulkanError> {
        if self.submission_failed {
            return Err(VulkanError::Driver(
                "device requires recreation after submission failure".into(),
            ));
        }
        let alloc = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let cmd = unsafe { self.device.allocate_command_buffers(&alloc) }
            .map_err(|e| VulkanError::Driver(format!("vkAllocateCommandBuffers: {e:?}")))?[0];
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        if let Err(error) = unsafe { self.device.begin_command_buffer(cmd, &begin) } {
            unsafe { self.device.free_command_buffers(self.command_pool, &[cmd]) };
            return Err(VulkanError::Driver(format!(
                "vkBeginCommandBuffer: {error:?}"
            )));
        }
        Ok(cmd)
    }

    #[cfg(test)]
    fn submit_and_wait(&mut self, cmd: vk::CommandBuffer) -> Result<(), VulkanError> {
        self.submit_and_wait_with_staging(cmd, None).map(drop)
    }

    fn submit_and_wait_with_staging(
        &mut self,
        cmd: vk::CommandBuffer,
        staging: Option<PendingBufferAllocation>,
    ) -> Result<Option<PendingBufferAllocation>, VulkanError> {
        if let Err(error) = unsafe { self.device.end_command_buffer(cmd) } {
            self.submission_failed = true;
            unsafe { self.device.free_command_buffers(self.command_pool, &[cmd]) };
            return Err(VulkanError::Driver(format!(
                "vkEndCommandBuffer: {error:?}"
            )));
        }
        let fence_info = vk::FenceCreateInfo::default();
        let fence = match unsafe { self.device.create_fence(&fence_info, None) } {
            Ok(fence) => fence,
            Err(error) => {
                self.submission_failed = true;
                // Nothing was submitted, so the caller's one-shot buffer can
                // be released immediately even though no fence was created.
                unsafe { self.device.free_command_buffers(self.command_pool, &[cmd]) };
                return Err(VulkanError::Driver(format!("vkCreateFence: {error:?}")));
            }
        };
        let cmds = [cmd];
        let submit = [vk::SubmitInfo::default().command_buffers(&cmds)];
        let submitted = unsafe { self.device.queue_submit(self.queue, &submit, fence) };
        if let Err(e) = submitted {
            self.submission_failed = true;
            unsafe {
                self.device.destroy_fence(fence, None);
                self.device.free_command_buffers(self.command_pool, &[cmd])
            };
            return Err(VulkanError::Driver(format!("vkQueueSubmit: {e:?}")));
        }
        let waited = unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) };
        self.finish_texture_upload_wait(cmd, fence, staging, waited)
    }

    fn finish_texture_upload_wait(
        &mut self,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
        staging: Option<PendingBufferAllocation>,
        waited: Result<(), vk::Result>,
    ) -> Result<Option<PendingBufferAllocation>, VulkanError> {
        if let Err(error) = waited {
            // Neither the fence nor the command buffer may be destroyed merely
            // because their host wait failed. Prevent reuse of uncertain layouts.
            self.submission_failed = true;
            self.retained_texture_uploads.push(RetainedTextureUpload {
                command: cmd,
                fence,
                _staging: staging,
            });
            return Err(VulkanError::Driver(format!("vkWaitForFences: {error:?}")));
        }
        unsafe {
            self.device.destroy_fence(fence, None);
            self.device.free_command_buffers(self.command_pool, &[cmd])
        };
        Ok(staging)
    }

    fn transition_image(
        &mut self,
        cmd: vk::CommandBuffer,
        id: ResourceId,
        new_layout: vk::ImageLayout,
    ) -> Result<(), VulkanError> {
        let img = self
            .images
            .get_mut(&id)
            .ok_or(VulkanError::MissingResource(id.0))?;
        if img.layout == new_layout {
            return Ok(());
        }
        let barrier = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
            .old_layout(img.layout)
            .new_layout(new_layout)
            .image(img.image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: img.aspect,
                base_mip_level: 0,
                level_count: img.mip_levels,
                base_array_layer: 0,
                layer_count: img.layers,
            })];
        let dep = vk::DependencyInfo::default().image_memory_barriers(&barrier);
        unsafe { self.device.cmd_pipeline_barrier2(cmd, &dep) };
        img.layout = new_layout;
        Ok(())
    }

    fn upload_image_bytes(&mut self, desc: &TextureDesc, bytes: &[u8]) -> Result<(), VulkanError> {
        let id = desc.id;
        let len = bytes.len() as u64;
        if len == 0 {
            return Ok(());
        }
        let img = self
            .images
            .get(&id)
            .ok_or(VulkanError::MissingResource(id.0))?;
        // Validate the entire copy before allocating staging or recording a
        // transition. A truncated later mip must not change the cached layout.
        let regions = texture_payload::copy_regions(desc, len, img.aspect)?;
        let info = vk::BufferCreateInfo::default()
            .size(len.max(4))
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let staging = unsafe { self.device.create_buffer(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreateBuffer(staging): {e:?}")))?;
        let mut allocation = PendingBufferAllocation {
            device: self.device.clone(),
            buffer: staging,
            memory: vk::DeviceMemory::null(),
            mapped: false,
        };
        let req = unsafe { self.device.get_buffer_memory_requirements(staging) };
        let ty = self
            .memory_type(
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE,
                vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or_else(|| VulkanError::Driver("no host visible staging memory".into()))?;
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(ty);
        let mem = unsafe { self.device.allocate_memory(&alloc, None) }
            .map_err(|e| VulkanError::Driver(format!("vkAllocateMemory(staging): {e:?}")))?;
        allocation.memory = mem;
        unsafe { self.device.bind_buffer_memory(staging, mem, 0) }
            .map_err(|e| VulkanError::Driver(format!("vkBindBufferMemory(staging): {e:?}")))?;
        let mapped = unsafe {
            self.device
                .map_memory(mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
        }
        .map_err(|e| VulkanError::Driver(format!("vkMapMemory(staging): {e:?}")))?;
        allocation.mapped = true;
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len()) };
        // HOST_COHERENT is preferred, not required by memory_type(). Flush the
        // entire mapped allocation so noncoherent atom alignment is respected.
        let range = vk::MappedMemoryRange::default()
            .memory(mem)
            .offset(0)
            .size(vk::WHOLE_SIZE);
        unsafe { self.device.flush_mapped_memory_ranges(&[range]) }.map_err(|error| {
            VulkanError::Driver(format!(
                "vkFlushMappedMemoryRanges(texture upload): {error:?}"
            ))
        })?;
        unsafe { self.device.unmap_memory(mem) };
        allocation.mapped = false;
        let cmd = self.begin_commands()?;
        self.transition_image(cmd, id, vk::ImageLayout::TRANSFER_DST_OPTIMAL)?;
        let img = self
            .images
            .get(&id)
            .ok_or(VulkanError::MissingResource(id.0))?;
        unsafe {
            self.device.cmd_copy_buffer_to_image(
                cmd,
                staging,
                img.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            )
        };
        self.transition_image(cmd, id, vk::ImageLayout::GENERAL)?;
        drop(self.submit_and_wait_with_staging(cmd, Some(allocation))?);
        Ok(())
    }

    fn prepare_resources(
        &mut self,
        plan: &VulkanExecutionPlan,
        provider: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        let usage = resource_validation::analyze(plan)?;
        for desc in &plan.buffers {
            self.ensure_buffer(desc, provider)?;
        }
        for desc in &plan.textures {
            // GPU-produced images retain ownership. Only read-only inputs are
            // refreshed from guest bytes; read/write sequencing needs separate
            // dirty tracking rather than overwriting a pending GPU result.
            let refresh = usage.read_textures.contains(&desc.id)
                && !usage.written_textures.contains(&desc.id);
            self.ensure_image(desc, provider, refresh)?;
        }
        for desc in &plan.samplers {
            self.ensure_sampler(desc)?;
        }
        Ok(())
    }

    fn graphics_layout_key(id: u64) -> u128 {
        id as u128
    }
    fn compute_layout_key(id: u64) -> u128 {
        (1u128 << 64) | id as u128
    }

    fn ensure_shader_module(
        &mut self,
        shader: &VulkanShaderModule,
    ) -> Result<vk::ShaderModule, VulkanError> {
        if self
            .shader_module_descriptors
            .get(&shader.id)
            .is_some_and(|cached| cached != shader)
        {
            return Err(VulkanError::UnsupportedFeature("shader identity changed"));
        }
        let mut float_properties = vk::PhysicalDeviceFloatControlsProperties::default();
        let mut properties =
            vk::PhysicalDeviceProperties2::default().push_next(&mut float_properties);
        // Vulkan 1.3 supplies these properties without optional device feature
        // enabling. A requested execution mode still needs its per-width support.
        unsafe {
            self.instance
                .get_physical_device_properties2(self.physical, &mut properties)
        };
        float_controls::validate(&shader.spirv.words, &float_properties)?;
        if let Some(module) = self.shader_modules.get(&shader.id) {
            return Ok(*module);
        }
        let info = vk::ShaderModuleCreateInfo::default().code(&shader.spirv.words);
        let module = unsafe { self.device.create_shader_module(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreateShaderModule: {e:?}")))?;
        self.shader_modules.insert(shader.id, module);
        self.shader_module_descriptors
            .insert(shader.id, shader.clone());
        Ok(module)
    }

    fn sample_count(samples: u8) -> Result<vk::SampleCountFlags, VulkanError> {
        Ok(match samples {
            1 => vk::SampleCountFlags::TYPE_1,
            2 => vk::SampleCountFlags::TYPE_2,
            4 => vk::SampleCountFlags::TYPE_4,
            8 => vk::SampleCountFlags::TYPE_8,
            16 => vk::SampleCountFlags::TYPE_16,
            32 => vk::SampleCountFlags::TYPE_32,
            64 => vk::SampleCountFlags::TYPE_64,
            _ => return Err(VulkanError::UnsupportedFeature("sample count")),
        })
    }

    fn logic_op(op: exosphere_gpu_ir::LogicOp) -> Result<vk::LogicOp, VulkanError> {
        use exosphere_gpu_ir::LogicOp::*;
        Ok(match op {
            Clear => vk::LogicOp::CLEAR,
            And => vk::LogicOp::AND,
            AndReverse => vk::LogicOp::AND_REVERSE,
            Copy => vk::LogicOp::COPY,
            AndInverted => vk::LogicOp::AND_INVERTED,
            NoOp => vk::LogicOp::NO_OP,
            Xor => vk::LogicOp::XOR,
            Or => vk::LogicOp::OR,
            Nor => vk::LogicOp::NOR,
            Equivalent => vk::LogicOp::EQUIVALENT,
            Invert => vk::LogicOp::INVERT,
            OrReverse => vk::LogicOp::OR_REVERSE,
            CopyInverted => vk::LogicOp::COPY_INVERTED,
            OrInverted => vk::LogicOp::OR_INVERTED,
            Nand => vk::LogicOp::NAND,
            Set => vk::LogicOp::SET,
            Unknown(_) => return Err(VulkanError::UnsupportedFeature("logic op")),
        })
    }

    fn stencil_state(
        value: exosphere_gpu_ir::StencilFaceState,
    ) -> Result<vk::StencilOpState, VulkanError> {
        Ok(vk::StencilOpState::default()
            .fail_op(
                vk_convert::stencil(value.fail)
                    .ok_or(VulkanError::UnsupportedFeature("stencil fail op"))?,
            )
            .pass_op(
                vk_convert::stencil(value.pass)
                    .ok_or(VulkanError::UnsupportedFeature("stencil pass op"))?,
            )
            .depth_fail_op(
                vk_convert::stencil(value.depth_fail)
                    .ok_or(VulkanError::UnsupportedFeature("stencil depth-fail op"))?,
            )
            .compare_op(
                vk_convert::compare(value.compare)
                    .ok_or(VulkanError::UnsupportedFeature("stencil compare op"))?,
            )
            .compare_mask(value.compare_mask)
            .write_mask(value.write_mask)
            .reference(value.reference))
    }

    fn attachment_formats(
        &self,
        state: &AttachmentState,
    ) -> Result<(Vec<vk::Format>, vk::Format, vk::Format), VulkanError> {
        let mut colors = Vec::new();
        if let Some(max) = state.colors.keys().max().copied() {
            for slot in 0..=max {
                colors.push(if let Some(id) = state.colors.get(&slot) {
                    self.images
                        .get(id)
                        .ok_or(VulkanError::MissingResource(id.0))?
                        .format
                } else {
                    vk::Format::UNDEFINED
                });
            }
        }
        let mut depth = vk::Format::UNDEFINED;
        let mut stencil = vk::Format::UNDEFINED;
        if let Some(id) = state.depth_stencil {
            let image = self
                .images
                .get(&id)
                .ok_or(VulkanError::MissingResource(id.0))?;
            if image.aspect.contains(vk::ImageAspectFlags::DEPTH) {
                depth = image.format;
            }
            if image.aspect.contains(vk::ImageAspectFlags::STENCIL) {
                stencil = image.format;
            }
        }
        Ok((colors, depth, stencil))
    }

    fn ensure_graphics_pipeline(
        &mut self,
        plan: &VulkanExecutionPlan,
        id: u64,
        attachments: &AttachmentState,
    ) -> Result<vk::Pipeline, VulkanError> {
        let desc = plan
            .graphics_pipelines
            .iter()
            .find(|p| p.id == id)
            .ok_or(VulkanError::MissingPipeline(id))?
            .clone();
        let expected_samples = Self::sample_count(desc.fixed_function.multisample.raster_samples)?;
        // IDs name immutable pipeline state. Replacing a cached object could
        // invalidate in-flight work; silently reusing it would render old state.
        if self
            .graphics_pipeline_descriptors
            .get(&id)
            .is_some_and(|cached| cached != &desc)
        {
            return Err(VulkanError::UnsupportedFeature(
                "graphics pipeline identity changed",
            ));
        }
        for id in attachments
            .colors
            .values()
            .chain(attachments.depth_stencil.iter())
        {
            let image = self
                .images
                .get(id)
                .ok_or(VulkanError::MissingResource(id.0))?;
            if image.samples != expected_samples {
                return Err(VulkanError::Driver(format!(
                    "attachment {} sample count does not match pipeline",
                    id.0
                )));
            }
        }
        let (color_formats, depth_format, stencil_format) = self.attachment_formats(attachments)?;
        let key = GraphicsPipelineKey {
            pipeline: id,
            color_formats: color_formats.iter().map(|f| f.as_raw()).collect(),
            depth_format: depth_format.as_raw(),
            stencil_format: stencil_format.as_raw(),
        };
        if let Some(p) = self.graphics_pipelines.get(&key) {
            return Ok(*p);
        }
        let bias = desc.fixed_function.raster.depth_bias_factors;
        let bias_constant = f32::from_bits(bias.constant_bits);
        let bias_slope = f32::from_bits(bias.slope_bits);
        let bias_clamp = f32::from_bits(bias.clamp_bits);
        if desc.fixed_function.raster.depth_bias {
            if ![bias_constant, bias_slope, bias_clamp]
                .into_iter()
                .all(f32::is_finite)
            {
                return Err(VulkanError::UnsupportedFeature("non-finite depth bias"));
            }
            if bias_clamp != 0.0 && self.core_features.depth_bias_clamp != vk::TRUE {
                return Err(VulkanError::UnsupportedFeature("depth bias clamp"));
            }
        }
        if desc.fixed_function.raster.depth_clamp && self.core_features.depth_clamp != vk::TRUE {
            return Err(VulkanError::UnsupportedFeature("depthClamp"));
        }
        if !matches!(
            desc.fixed_function.raster.polygon_mode,
            exosphere_gpu_ir::PolygonMode::Fill
        ) && self.core_features.fill_mode_non_solid != vk::TRUE
        {
            return Err(VulkanError::UnsupportedFeature("fillModeNonSolid"));
        }
        if desc.fixed_function.blend.logic_op_enable && self.core_features.logic_op != vk::TRUE {
            return Err(VulkanError::UnsupportedFeature("logicOp"));
        }
        if desc.fixed_function.multisample.alpha_to_one
            && self.core_features.alpha_to_one != vk::TRUE
        {
            return Err(VulkanError::UnsupportedFeature("alphaToOne"));
        }
        if desc.geometry_shader.is_some() && self.core_features.geometry_shader != vk::TRUE {
            return Err(VulkanError::UnsupportedFeature("geometryShader"));
        }
        if (desc.tess_control_shader.is_some() || desc.tess_eval_shader.is_some())
            && self.core_features.tessellation_shader != vk::TRUE
        {
            return Err(VulkanError::UnsupportedFeature("tessellationShader"));
        }

        let mut stages = Vec::new();
        let main = CString::new("main").unwrap();
        let stage_ids = [
            (desc.vertex_shader, exosphere_gpu_ir::ShaderStage::Vertex),
            (
                desc.tess_control_shader.unwrap_or(0),
                exosphere_gpu_ir::ShaderStage::TessControl,
            ),
            (
                desc.tess_eval_shader.unwrap_or(0),
                exosphere_gpu_ir::ShaderStage::TessEvaluation,
            ),
            (
                desc.geometry_shader.unwrap_or(0),
                exosphere_gpu_ir::ShaderStage::Geometry,
            ),
            (
                desc.fragment_shader,
                exosphere_gpu_ir::ShaderStage::Fragment,
            ),
        ];
        for (shader_id, stage) in stage_ids {
            if shader_id == 0 {
                continue;
            }
            let shader = plan
                .shaders
                .iter()
                .find(|s| s.id == shader_id)
                .ok_or(VulkanError::MissingShader(shader_id))?;
            let module = self.ensure_shader_module(shader)?;
            stages.push(
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk_convert::shader_stage(stage))
                    .module(module)
                    .name(&main),
            );
        }
        let bindings = desc
            .vertex_input
            .bindings
            .iter()
            .map(|b| vk::VertexInputBindingDescription {
                binding: b.binding,
                stride: b.stride,
                input_rate: vk_convert::input_rate(b.input_rate),
            })
            .collect::<Vec<_>>();
        let mut attrs = Vec::new();
        for a in &desc.vertex_input.attributes {
            attrs.push(vk::VertexInputAttributeDescription {
                location: a.location,
                binding: a.binding,
                format: vk_convert::vertex_format(a.format)
                    .ok_or(VulkanError::UnsupportedFeature("vertex format"))?,
                offset: a.offset,
            });
        }
        let vertex = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let topology = vk_convert::topology(desc.topology)
            .ok_or(VulkanError::UnsupportedFeature("primitive topology"))?;
        if desc.primitive_restart && !desc.topology.supports_standard_restart() {
            return Err(VulkanError::UnsupportedFeature(
                "primitive restart topology",
            ));
        }
        let input = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(topology)
            .primitive_restart_enable(desc.primitive_restart);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .depth_clamp_enable(desc.fixed_function.raster.depth_clamp)
            .rasterizer_discard_enable(desc.fixed_function.raster.rasterizer_discard)
            .polygon_mode(
                vk_convert::polygon(desc.fixed_function.raster.polygon_mode)
                    .ok_or(VulkanError::UnsupportedFeature("polygon mode"))?,
            )
            .cull_mode(
                vk_convert::cull(desc.fixed_function.raster.cull)
                    .ok_or(VulkanError::UnsupportedFeature("cull mode"))?,
            )
            .front_face(
                vk_convert::front_face(desc.fixed_function.raster.front_face)
                    .ok_or(VulkanError::UnsupportedFeature("front face"))?,
            )
            .depth_bias_enable(desc.fixed_function.raster.depth_bias)
            .depth_bias_constant_factor(if desc.fixed_function.raster.depth_bias {
                bias_constant
            } else {
                0.0
            })
            .depth_bias_slope_factor(if desc.fixed_function.raster.depth_bias {
                bias_slope
            } else {
                0.0
            })
            .depth_bias_clamp(if desc.fixed_function.raster.depth_bias {
                bias_clamp
            } else {
                0.0
            })
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(Self::sample_count(
                desc.fixed_function.multisample.raster_samples,
            )?)
            .sample_shading_enable(false)
            .alpha_to_coverage_enable(desc.fixed_function.multisample.alpha_to_coverage)
            .alpha_to_one_enable(desc.fixed_function.multisample.alpha_to_one)
            .sample_mask(std::slice::from_ref(
                &desc.fixed_function.multisample.sample_mask,
            ));
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(desc.fixed_function.depth_stencil.depth_test)
            .depth_write_enable(desc.fixed_function.depth_stencil.depth_write)
            .depth_compare_op(
                vk_convert::compare(desc.fixed_function.depth_stencil.depth_compare)
                    .ok_or(VulkanError::UnsupportedFeature("depth compare"))?,
            )
            .stencil_test_enable(desc.fixed_function.depth_stencil.stencil_test)
            .front(Self::stencil_state(
                desc.fixed_function.depth_stencil.front,
            )?)
            .back(Self::stencil_state(desc.fixed_function.depth_stencil.back)?);
        let mut color_attachments = Vec::new();
        for slot in 0..color_formats.len() {
            let b = desc
                .fixed_function
                .blend
                .targets
                .get(slot)
                .copied()
                .unwrap_or_default();
            color_attachments.push(
                vk::PipelineColorBlendAttachmentState::default()
                    .blend_enable(b.enable)
                    .src_color_blend_factor(
                        vk_convert::blend_factor(b.src_color)
                            .ok_or(VulkanError::UnsupportedFeature("blend factor"))?,
                    )
                    .dst_color_blend_factor(
                        vk_convert::blend_factor(b.dst_color)
                            .ok_or(VulkanError::UnsupportedFeature("blend factor"))?,
                    )
                    .color_blend_op(
                        vk_convert::blend_op(b.color_op)
                            .ok_or(VulkanError::UnsupportedFeature("blend op"))?,
                    )
                    .src_alpha_blend_factor(
                        vk_convert::blend_factor(b.src_alpha)
                            .ok_or(VulkanError::UnsupportedFeature("blend factor"))?,
                    )
                    .dst_alpha_blend_factor(
                        vk_convert::blend_factor(b.dst_alpha)
                            .ok_or(VulkanError::UnsupportedFeature("blend factor"))?,
                    )
                    .alpha_blend_op(
                        vk_convert::blend_op(b.alpha_op)
                            .ok_or(VulkanError::UnsupportedFeature("blend op"))?,
                    )
                    .color_write_mask(vk::ColorComponentFlags::from_raw(u32::from(
                        b.color_write_mask & 0xf,
                    ))),
            );
        }
        let blend_constants = desc.fixed_function.blend.constants.map(f32::from_bits);
        let blend = vk::PipelineColorBlendStateCreateInfo::default()
            .logic_op_enable(desc.fixed_function.blend.logic_op_enable)
            .logic_op(Self::logic_op(desc.fixed_function.blend.logic_op)?)
            .attachments(&color_attachments)
            .blend_constants(blend_constants);
        let dynamics = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamics);
        let layout = self.create_layout(Self::graphics_layout_key(id), &desc.layout)?;
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(depth_format)
            .stencil_attachment_format(stencil_format);
        let mut tess = vk::PipelineTessellationStateCreateInfo::default();
        if let exosphere_gpu_ir::PrimitiveTopology::PatchList(points) = desc.topology {
            tess = tess.patch_control_points(u32::from(points.max(1)));
        }
        let mut info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex)
            .input_assembly_state(&input)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        if matches!(
            desc.topology,
            exosphere_gpu_ir::PrimitiveTopology::PatchList(_)
        ) {
            info = info.tessellation_state(&tess);
        }
        let pipelines = unsafe {
            self.device
                .create_graphics_pipelines(self.pipeline_cache, &[info], None)
        }
        .map_err(|(created, e)| {
            unsafe {
                for p in created {
                    self.device.destroy_pipeline(p, None)
                }
            };
            VulkanError::Driver(format!("vkCreateGraphicsPipelines: {e:?}"))
        })?;
        let pipeline = pipelines[0];
        self.graphics_pipelines.insert(key, pipeline);
        self.graphics_pipeline_descriptors.insert(id, desc);
        Ok(pipeline)
    }

    fn ensure_compute_pipeline(
        &mut self,
        plan: &VulkanExecutionPlan,
        id: u64,
    ) -> Result<vk::Pipeline, VulkanError> {
        if let Some(p) = self.compute_pipelines.get(&id) {
            return Ok(*p);
        }
        let desc: ComputePipelineDesc = plan
            .compute_pipelines
            .iter()
            .find(|p| p.id == id)
            .ok_or(VulkanError::MissingPipeline(id))?
            .clone();
        let shader = plan
            .shaders
            .iter()
            .find(|s| s.id == desc.compute_shader)
            .ok_or(VulkanError::MissingShader(desc.compute_shader))?;
        let module = self.ensure_shader_module(shader)?;
        let main = CString::new("main").unwrap();
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(&main);
        let layout = self.create_layout(Self::compute_layout_key(id), &desc.layout)?;
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout);
        let pipelines = unsafe {
            self.device
                .create_compute_pipelines(self.pipeline_cache, &[info], None)
        }
        .map_err(|(created, e)| {
            unsafe {
                for p in created {
                    self.device.destroy_pipeline(p, None)
                }
            };
            VulkanError::Driver(format!("vkCreateComputePipelines: {e:?}"))
        })?;
        let pipeline = pipelines[0];
        self.compute_pipelines.insert(id, pipeline);
        Ok(pipeline)
    }

    fn create_descriptor_pool(&self) -> Result<vk::DescriptorPool, VulkanError> {
        let count = 4096;
        let sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: count,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: count,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: count,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: count,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: count,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: count,
            },
        ];
        let info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(4096)
            .pool_sizes(&sizes);
        unsafe { self.device.create_descriptor_pool(&info, None) }
            .map_err(|e| VulkanError::Driver(format!("vkCreateDescriptorPool: {e:?}")))
    }

    fn allocate_descriptor_sets(
        &self,
        pools: &mut Vec<vk::DescriptorPool>,
        layouts: &[vk::DescriptorSetLayout],
    ) -> Result<Vec<vk::DescriptorSet>, VulkanError> {
        if layouts.is_empty() {
            return Ok(Vec::new());
        }
        if pools.is_empty() {
            pools.push(self.create_descriptor_pool()?);
        }
        loop {
            let pool = *pools.last().unwrap();
            let info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(layouts);
            match unsafe { self.device.allocate_descriptor_sets(&info) } {
                Ok(sets) => return Ok(sets),
                Err(vk::Result::ERROR_OUT_OF_POOL_MEMORY)
                | Err(vk::Result::ERROR_FRAGMENTED_POOL) => {
                    pools.push(self.create_descriptor_pool()?)
                }
                Err(e) => {
                    return Err(VulkanError::Driver(format!(
                        "vkAllocateDescriptorSets: {e:?}"
                    )));
                }
            }
        }
    }

    fn descriptor_binding_buffer<'a>(
        state: &'a DescriptorState,
        d: &DescriptorBindingDesc,
        array: u32,
    ) -> Option<&'a BufferDescriptorBinding> {
        state.buffers.get(&(d.set, d.binding, array))
    }
    fn descriptor_binding_image(
        state: &DescriptorState,
        d: &DescriptorBindingDesc,
        array: u32,
    ) -> Option<ResourceId> {
        state
            .images
            .get(&(d.set, d.binding, array))
            .copied()
            .or_else(|| state.legacy_textures.get(&(d.binding + array)).copied())
    }
    fn descriptor_binding_sampler(
        state: &DescriptorState,
        d: &DescriptorBindingDesc,
        array: u32,
    ) -> Option<ResourceId> {
        state
            .samplers
            .get(&(d.set, d.binding, array))
            .copied()
            .or_else(|| state.legacy_samplers.get(&(d.binding + array)).copied())
    }

    fn bind_descriptors(
        &mut self,
        cmd: vk::CommandBuffer,
        bind_point: vk::PipelineBindPoint,
        layout_key: u128,
        layout_desc: &PipelineLayoutDesc,
        state: &DescriptorState,
        pools: &mut Vec<vk::DescriptorPool>,
    ) -> Result<(), VulkanError> {
        if layout_desc.descriptors.is_empty() {
            return Ok(());
        }
        let layout_obj = self
            .pipeline_layouts
            .get(&layout_key)
            .ok_or(VulkanError::UnsupportedFeature("pipeline layout missing"))?;
        let set_layouts = layout_obj.set_layouts.clone();
        let pipeline_layout = layout_obj.layout;
        let sets = self.allocate_descriptor_sets(pools, &set_layouts)?;
        for d in &layout_desc.descriptors {
            let set = *sets
                .get(d.set as usize)
                .ok_or(VulkanError::UnsupportedFeature("descriptor set index"))?;
            for array in 0..d.count.max(1) {
                match d.descriptor_type {
                    DescriptorType::UniformBuffer | DescriptorType::StorageBuffer => {
                        let b = Self::descriptor_binding_buffer(state, d, array)
                            .ok_or(VulkanError::UnsupportedFeature("unbound buffer descriptor"))?;
                        let object = self
                            .buffers
                            .get(&b.resource)
                            .ok_or(VulkanError::MissingResource(b.resource.0))?;
                        let limits =
                            unsafe { self.instance.get_physical_device_properties(self.physical) }
                                .limits;
                        let range = buffer_descriptor::validate(
                            d.descriptor_type,
                            object.usage,
                            object.size,
                            b.offset,
                            b.range,
                            &limits,
                        )?;
                        let info = [vk::DescriptorBufferInfo::default()
                            .buffer(object.buffer)
                            .offset(b.offset)
                            .range(range)];
                        let write = [vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(d.binding)
                            .dst_array_element(array)
                            .descriptor_type(vk_convert::descriptor_type(d.descriptor_type))
                            .buffer_info(&info)];
                        unsafe { self.device.update_descriptor_sets(&write, &[]) };
                    }
                    DescriptorType::SampledImage | DescriptorType::StorageImage => {
                        let id = Self::descriptor_binding_image(state, d, array)
                            .ok_or(VulkanError::UnsupportedFeature("unbound image descriptor"))?;
                        let image = self
                            .images
                            .get(&id)
                            .ok_or(VulkanError::MissingResource(id.0))?;
                        let required = if d.descriptor_type == DescriptorType::StorageImage {
                            vk::ImageUsageFlags::STORAGE
                        } else {
                            vk::ImageUsageFlags::SAMPLED
                        };
                        image_usage::require(image.usage, required)?;
                        let info = [vk::DescriptorImageInfo::default()
                            .image_view(image.view)
                            .image_layout(vk::ImageLayout::GENERAL)];
                        let write = [vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(d.binding)
                            .dst_array_element(array)
                            .descriptor_type(vk_convert::descriptor_type(d.descriptor_type))
                            .image_info(&info)];
                        unsafe { self.device.update_descriptor_sets(&write, &[]) };
                    }
                    DescriptorType::Sampler => {
                        let id = Self::descriptor_binding_sampler(state, d, array).ok_or(
                            VulkanError::UnsupportedFeature("unbound sampler descriptor"),
                        )?;
                        let sampler = self
                            .samplers
                            .get(&id)
                            .ok_or(VulkanError::MissingResource(id.0))?;
                        let info = [vk::DescriptorImageInfo::default().sampler(sampler.sampler)];
                        let write = [vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(d.binding)
                            .dst_array_element(array)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .image_info(&info)];
                        unsafe { self.device.update_descriptor_sets(&write, &[]) };
                    }
                    DescriptorType::CombinedImageSampler => {
                        let image_id = Self::descriptor_binding_image(state, d, array)
                            .ok_or(VulkanError::UnsupportedFeature("unbound combined image"))?;
                        let sampler_id = Self::descriptor_binding_sampler(state, d, array)
                            .ok_or(VulkanError::UnsupportedFeature("unbound combined sampler"))?;
                        let image = self
                            .images
                            .get(&image_id)
                            .ok_or(VulkanError::MissingResource(image_id.0))?;
                        let sampler = self
                            .samplers
                            .get(&sampler_id)
                            .ok_or(VulkanError::MissingResource(sampler_id.0))?;
                        let info = [vk::DescriptorImageInfo::default()
                            .sampler(sampler.sampler)
                            .image_view(image.view)
                            .image_layout(vk::ImageLayout::GENERAL)];
                        let write = [vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(d.binding)
                            .dst_array_element(array)
                            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                            .image_info(&info)];
                        unsafe { self.device.update_descriptor_sets(&write, &[]) };
                    }
                }
            }
        }
        unsafe {
            self.device
                .cmd_bind_descriptor_sets(cmd, bind_point, pipeline_layout, 0, &sets, &[])
        };
        Ok(())
    }

    fn begin_dynamic_rendering(
        &mut self,
        cmd: vk::CommandBuffer,
        state: &mut AttachmentState,
        width: u32,
        height: u32,
    ) -> Result<(), VulkanError> {
        if state.active {
            return Err(VulkanError::Driver("nested dynamic rendering".into()));
        }
        let max_slot = state.colors.keys().max().copied();
        let mut colors = Vec::new();
        if let Some(max) = max_slot {
            for slot in 0..=max {
                if let Some(id) = state.colors.get(&slot).copied() {
                    image_usage::require(
                        self.images
                            .get(&id)
                            .ok_or(VulkanError::MissingResource(id.0))?
                            .usage,
                        vk::ImageUsageFlags::COLOR_ATTACHMENT,
                    )?;
                    self.transition_image(cmd, id, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)?;
                    let image = self
                        .images
                        .get(&id)
                        .ok_or(VulkanError::MissingResource(id.0))?;
                    colors.push(
                        vk::RenderingAttachmentInfo::default()
                            .image_view(image.view)
                            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                            .load_op(vk::AttachmentLoadOp::LOAD)
                            .store_op(vk::AttachmentStoreOp::STORE),
                    );
                } else {
                    colors.push(
                        vk::RenderingAttachmentInfo::default()
                            .image_view(vk::ImageView::null())
                            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                            .load_op(vk::AttachmentLoadOp::DONT_CARE)
                            .store_op(vk::AttachmentStoreOp::DONT_CARE),
                    );
                }
            }
        }
        let mut depth_info = None;
        let mut stencil_info = None;
        if let Some(id) = state.depth_stencil {
            image_usage::require(
                self.images
                    .get(&id)
                    .ok_or(VulkanError::MissingResource(id.0))?
                    .usage,
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            )?;
            self.transition_image(cmd, id, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)?;
            let image = self
                .images
                .get(&id)
                .ok_or(VulkanError::MissingResource(id.0))?;
            if image.aspect.contains(vk::ImageAspectFlags::DEPTH) {
                depth_info = Some(
                    vk::RenderingAttachmentInfo::default()
                        .image_view(image.view)
                        .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                        .load_op(vk::AttachmentLoadOp::LOAD)
                        .store_op(vk::AttachmentStoreOp::STORE),
                );
            }
            if image.aspect.contains(vk::ImageAspectFlags::STENCIL) {
                stencil_info = Some(
                    vk::RenderingAttachmentInfo::default()
                        .image_view(image.view)
                        .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                        .load_op(vk::AttachmentLoadOp::LOAD)
                        .store_op(vk::AttachmentStoreOp::STORE),
                );
            }
        }
        let mut rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D { width, height },
            })
            .layer_count(1)
            .color_attachments(&colors);
        if let Some(ref depth) = depth_info {
            rendering = rendering.depth_attachment(depth);
        }
        if let Some(ref stencil) = stencil_info {
            rendering = rendering.stencil_attachment(stencil);
        }
        unsafe { self.device.cmd_begin_rendering(cmd, &rendering) };
        state.active = true;
        Ok(())
    }

    fn validate_push_constants(
        layout: &PipelineLayoutDesc,
        stages: ShaderStageMask,
        offset: u32,
        len: u32,
    ) -> bool {
        let end = offset.saturating_add(len);
        layout.push_constants.iter().any(|r| {
            offset >= r.offset
                && end <= r.offset.saturating_add(r.size)
                && (r.stages.0 & stages.0) == stages.0
        })
    }

    fn submit_core(
        &mut self,
        plan: &VulkanExecutionPlan,
        provider: &mut dyn GpuResourceData,
    ) -> Result<ExecutionToken, VulkanError> {
        if self.submission_failed {
            return Err(VulkanError::Driver(
                "backend stopped after failed GPU submission; recreate device".into(),
            ));
        }
        // Cached pipelines may bypass shader creation. Validate immutable code
        // identities before uploads or command execution, not only on cache miss.
        let mut seen = BTreeMap::new();
        for shader in &plan.shaders {
            if self
                .shader_module_descriptors
                .get(&shader.id)
                .is_some_and(|cached| cached != shader)
                || seen
                    .insert(shader.id, shader)
                    .is_some_and(|previous| previous != shader)
            {
                return Err(VulkanError::UnsupportedFeature("shader identity changed"));
            }
        }
        for command in &plan.commands {
            if let VulkanPlanCommand::SetViewport { viewport } = command {
                let limits =
                    unsafe { self.instance.get_physical_device_properties(self.physical) }.limits;
                let [lower, upper] = limits.viewport_bounds_range;
                if ![
                    viewport.x,
                    viewport.y,
                    viewport.width,
                    viewport.height,
                    viewport.min_depth,
                    viewport.max_depth,
                ]
                .into_iter()
                .all(f32::is_finite)
                    || viewport.width <= 0.0
                    || viewport.width > limits.max_viewport_dimensions[0] as f32
                    || viewport.height.abs() > limits.max_viewport_dimensions[1] as f32
                    || viewport.x < lower
                    || viewport.x + viewport.width > upper
                    || viewport.y < lower
                    || viewport.y > upper
                    || viewport.y + viewport.height < lower
                    || viewport.y + viewport.height > upper
                    || !(0.0..=1.0).contains(&viewport.min_depth)
                    || !(0.0..=1.0).contains(&viewport.max_depth)
                {
                    return Err(VulkanError::UnsupportedFeature(
                        "viewport bounds or depth range",
                    ));
                }
            }
            if let VulkanPlanCommand::DrawIndirect { draw_count, .. } = command {
                let limits =
                    unsafe { self.instance.get_physical_device_properties(self.physical) }.limits;
                if (*draw_count > 1 && self.core_features.multi_draw_indirect != vk::TRUE)
                    || *draw_count > limits.max_draw_indirect_count
                {
                    return Err(VulkanError::UnsupportedFeature(
                        "indirect draw count capability",
                    ));
                }
            }
        }
        self.prepare_resources(plan, provider)?;
        let image_layouts: Vec<_> = self
            .images
            .iter()
            .map(|(&id, image)| (id, image.layout))
            .collect();
        let (frame_index, cmd) = self.begin_frame()?;
        let mut bound_graphics = None::<u64>;
        let mut bound_compute = None::<u64>;
        let mut last_bind_point = None::<vk::PipelineBindPoint>;
        let mut descriptors = DescriptorState::default();
        let mut attachments = AttachmentState::default();
        let mut descriptor_pools = Vec::<vk::DescriptorPool>::new();

        let record_result = (|| -> Result<ExecutionToken, VulkanError> {
            for op in &plan.commands {
                match op {
                    VulkanPlanCommand::BindVertexBuffer {
                        slot,
                        buffer,
                        offset,
                    } => {
                        let b = self
                            .buffers
                            .get(buffer)
                            .ok_or(VulkanError::MissingResource(buffer.0))?;
                        unsafe {
                            self.device
                                .cmd_bind_vertex_buffers(cmd, *slot, &[b.buffer], &[*offset])
                        };
                    }
                    VulkanPlanCommand::BindIndexBuffer {
                        buffer,
                        offset,
                        index_type,
                    } => {
                        if *index_type == exosphere_gpu_ir::IndexType::U8 && !self.index_type_uint8
                        {
                            return Err(VulkanError::UnsupportedFeature(
                                "uint8 index feature not enabled",
                            ));
                        }
                        let b = self
                            .buffers
                            .get(buffer)
                            .ok_or(VulkanError::MissingResource(buffer.0))?;
                        let ty = vk_convert::index_type(*index_type)
                            .ok_or(VulkanError::UnsupportedFeature("index type"))?;
                        unsafe {
                            self.device
                                .cmd_bind_index_buffer(cmd, b.buffer, *offset, ty)
                        };
                    }
                    VulkanPlanCommand::BindGraphicsPipeline { pipeline } => {
                        if !plan.graphics_pipelines.iter().any(|p| p.id == *pipeline) {
                            return Err(VulkanError::MissingPipeline(*pipeline));
                        }
                        bound_graphics = Some(*pipeline);
                        last_bind_point = Some(vk::PipelineBindPoint::GRAPHICS);
                    }
                    VulkanPlanCommand::BindComputePipeline { pipeline } => {
                        if !plan.compute_pipelines.iter().any(|p| p.id == *pipeline) {
                            return Err(VulkanError::MissingPipeline(*pipeline));
                        }
                        bound_compute = Some(*pipeline);
                        last_bind_point = Some(vk::PipelineBindPoint::COMPUTE);
                    }
                    VulkanPlanCommand::SetViewport { viewport } => {
                        let v = vk::Viewport {
                            x: viewport.x,
                            y: viewport.y,
                            width: viewport.width,
                            height: viewport.height,
                            min_depth: viewport.min_depth,
                            max_depth: viewport.max_depth,
                        };
                        unsafe { self.device.cmd_set_viewport(cmd, 0, &[v]) };
                    }
                    VulkanPlanCommand::SetScissor { scissor } => {
                        let r = vk::Rect2D {
                            offset: vk::Offset2D {
                                x: scissor.x,
                                y: scissor.y,
                            },
                            extent: vk::Extent2D {
                                width: scissor.width,
                                height: scissor.height,
                            },
                        };
                        unsafe { self.device.cmd_set_scissor(cmd, 0, &[r]) };
                    }
                    VulkanPlanCommand::SetRenderTarget { slot, texture } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "changing render target during dynamic rendering",
                            ));
                        }
                        attachments.colors.insert(*slot, *texture);
                    }
                    VulkanPlanCommand::SetDepthStencilTarget { texture } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "changing depth target during dynamic rendering",
                            ));
                        }
                        attachments.depth_stencil = Some(*texture);
                    }
                    VulkanPlanCommand::BeginRendering { width, height } => {
                        self.begin_dynamic_rendering(cmd, &mut attachments, *width, *height)?
                    }
                    VulkanPlanCommand::EndRendering => {
                        if !attachments.active {
                            return Err(VulkanError::Driver(
                                "EndRendering without BeginRendering".into(),
                            ));
                        }
                        unsafe { self.device.cmd_end_rendering(cmd) };
                        attachments.active = false;
                    }

                    // Legacy Maxwell slot bindings remain supported. Once reflection is known the frontend
                    // should prefer the explicit set/binding commands below.
                    VulkanPlanCommand::BindTexture { slot, texture } => {
                        self.transition_image(cmd, *texture, vk::ImageLayout::GENERAL)?;
                        descriptors.legacy_textures.insert(*slot, *texture);
                    }
                    VulkanPlanCommand::BindSampler { slot, sampler } => {
                        descriptors.legacy_samplers.insert(*slot, *sampler);
                    }
                    VulkanPlanCommand::BindBufferDescriptor {
                        set,
                        binding,
                        array_element,
                        buffer,
                        offset,
                        range,
                    } => {
                        descriptors.buffers.insert(
                            (*set, *binding, *array_element),
                            BufferDescriptorBinding {
                                resource: *buffer,
                                offset: *offset,
                                range: *range,
                            },
                        );
                    }
                    VulkanPlanCommand::BindImageDescriptor {
                        set,
                        binding,
                        array_element,
                        texture,
                    } => {
                        self.transition_image(cmd, *texture, vk::ImageLayout::GENERAL)?;
                        descriptors
                            .images
                            .insert((*set, *binding, *array_element), *texture);
                    }
                    VulkanPlanCommand::BindSamplerDescriptor {
                        set,
                        binding,
                        array_element,
                        sampler,
                    } => {
                        descriptors
                            .samplers
                            .insert((*set, *binding, *array_element), *sampler);
                    }
                    VulkanPlanCommand::PushConstants {
                        stages,
                        offset,
                        data,
                    } => {
                        if data.len() % 4 != 0 || offset % 4 != 0 {
                            return Err(VulkanError::UnsupportedFeature(
                                "unaligned push constants",
                            ));
                        }
                        let (layout_key, layout_desc) = match last_bind_point {
                            Some(vk::PipelineBindPoint::GRAPHICS) => {
                                let id = bound_graphics.ok_or(VulkanError::UnsupportedFeature(
                                    "push constants without graphics pipeline",
                                ))?;
                                let p = plan
                                    .graphics_pipelines
                                    .iter()
                                    .find(|p| p.id == id)
                                    .ok_or(VulkanError::MissingPipeline(id))?;
                                (Self::graphics_layout_key(id), &p.layout)
                            }
                            Some(vk::PipelineBindPoint::COMPUTE) => {
                                let id = bound_compute.ok_or(VulkanError::UnsupportedFeature(
                                    "push constants without compute pipeline",
                                ))?;
                                let p = plan
                                    .compute_pipelines
                                    .iter()
                                    .find(|p| p.id == id)
                                    .ok_or(VulkanError::MissingPipeline(id))?;
                                (Self::compute_layout_key(id), &p.layout)
                            }
                            _ => {
                                return Err(VulkanError::UnsupportedFeature(
                                    "push constants without bound pipeline",
                                ));
                            }
                        };
                        if !Self::validate_push_constants(
                            layout_desc,
                            *stages,
                            *offset,
                            data.len() as u32,
                        ) {
                            return Err(VulkanError::UnsupportedFeature("push constant range"));
                        }
                        let layout = self.create_layout(layout_key, layout_desc)?;
                        unsafe {
                            self.device.cmd_push_constants(
                                cmd,
                                layout,
                                vk_convert::stage_mask(*stages),
                                *offset,
                                data,
                            )
                        };
                    }

                    VulkanPlanCommand::Draw {
                        vertices,
                        instances,
                        first_vertex,
                    } => {
                        if !attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "draw outside dynamic rendering",
                            ));
                        }
                        let id = bound_graphics.ok_or(VulkanError::UnsupportedFeature(
                            "draw without graphics pipeline",
                        ))?;
                        let desc = plan
                            .graphics_pipelines
                            .iter()
                            .find(|p| p.id == id)
                            .ok_or(VulkanError::MissingPipeline(id))?;
                        let pipeline = self.ensure_graphics_pipeline(plan, id, &attachments)?;
                        unsafe {
                            self.device.cmd_bind_pipeline(
                                cmd,
                                vk::PipelineBindPoint::GRAPHICS,
                                pipeline,
                            )
                        };
                        self.bind_descriptors(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            Self::graphics_layout_key(id),
                            &desc.layout,
                            &descriptors,
                            &mut descriptor_pools,
                        )?;
                        unsafe {
                            self.device
                                .cmd_draw(cmd, *vertices, *instances, *first_vertex, 0)
                        };
                    }
                    VulkanPlanCommand::DrawIndexed {
                        indices,
                        instances,
                        first_index,
                        vertex_offset,
                    } => {
                        if !attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "indexed draw outside dynamic rendering",
                            ));
                        }
                        let id = bound_graphics.ok_or(VulkanError::UnsupportedFeature(
                            "indexed draw without graphics pipeline",
                        ))?;
                        let desc = plan
                            .graphics_pipelines
                            .iter()
                            .find(|p| p.id == id)
                            .ok_or(VulkanError::MissingPipeline(id))?;
                        let pipeline = self.ensure_graphics_pipeline(plan, id, &attachments)?;
                        unsafe {
                            self.device.cmd_bind_pipeline(
                                cmd,
                                vk::PipelineBindPoint::GRAPHICS,
                                pipeline,
                            )
                        };
                        self.bind_descriptors(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            Self::graphics_layout_key(id),
                            &desc.layout,
                            &descriptors,
                            &mut descriptor_pools,
                        )?;
                        unsafe {
                            self.device.cmd_draw_indexed(
                                cmd,
                                *indices,
                                *instances,
                                *first_index,
                                *vertex_offset,
                                0,
                            )
                        };
                    }
                    VulkanPlanCommand::DrawIndirect {
                        buffer,
                        offset,
                        draw_count,
                        stride,
                        indexed,
                    } => {
                        if !attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "indirect draw outside dynamic rendering",
                            ));
                        }
                        let id = bound_graphics.ok_or(VulkanError::UnsupportedFeature(
                            "indirect draw without graphics pipeline",
                        ))?;
                        let desc = plan
                            .graphics_pipelines
                            .iter()
                            .find(|p| p.id == id)
                            .ok_or(VulkanError::MissingPipeline(id))?;
                        let pipeline = self.ensure_graphics_pipeline(plan, id, &attachments)?;
                        unsafe {
                            self.device.cmd_bind_pipeline(
                                cmd,
                                vk::PipelineBindPoint::GRAPHICS,
                                pipeline,
                            )
                        };
                        self.bind_descriptors(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            Self::graphics_layout_key(id),
                            &desc.layout,
                            &descriptors,
                            &mut descriptor_pools,
                        )?;
                        let b = self
                            .buffers
                            .get(buffer)
                            .ok_or(VulkanError::MissingResource(buffer.0))?;
                        unsafe {
                            if *indexed {
                                self.device.cmd_draw_indexed_indirect(
                                    cmd,
                                    b.buffer,
                                    *offset,
                                    *draw_count,
                                    if *stride == 0 { 20 } else { *stride },
                                )
                            } else {
                                self.device.cmd_draw_indirect(
                                    cmd,
                                    b.buffer,
                                    *offset,
                                    *draw_count,
                                    if *stride == 0 { 16 } else { *stride },
                                )
                            }
                        };
                    }
                    VulkanPlanCommand::Dispatch { x, y, z } => {
                        let id = bound_compute.ok_or(VulkanError::UnsupportedFeature(
                            "dispatch without compute pipeline",
                        ))?;
                        let desc = plan
                            .compute_pipelines
                            .iter()
                            .find(|p| p.id == id)
                            .ok_or(VulkanError::MissingPipeline(id))?;
                        let pipeline = self.ensure_compute_pipeline(plan, id)?;
                        unsafe {
                            self.device.cmd_bind_pipeline(
                                cmd,
                                vk::PipelineBindPoint::COMPUTE,
                                pipeline,
                            )
                        };
                        self.bind_descriptors(
                            cmd,
                            vk::PipelineBindPoint::COMPUTE,
                            Self::compute_layout_key(id),
                            &desc.layout,
                            &descriptors,
                            &mut descriptor_pools,
                        )?;
                        unsafe { self.device.cmd_dispatch(cmd, *x, *y, *z) };
                    }
                    VulkanPlanCommand::DispatchIndirect { buffer, offset } => {
                        let id = bound_compute.ok_or(VulkanError::UnsupportedFeature(
                            "indirect dispatch without compute pipeline",
                        ))?;
                        let desc = plan
                            .compute_pipelines
                            .iter()
                            .find(|p| p.id == id)
                            .ok_or(VulkanError::MissingPipeline(id))?;
                        let pipeline = self.ensure_compute_pipeline(plan, id)?;
                        unsafe {
                            self.device.cmd_bind_pipeline(
                                cmd,
                                vk::PipelineBindPoint::COMPUTE,
                                pipeline,
                            )
                        };
                        self.bind_descriptors(
                            cmd,
                            vk::PipelineBindPoint::COMPUTE,
                            Self::compute_layout_key(id),
                            &desc.layout,
                            &descriptors,
                            &mut descriptor_pools,
                        )?;
                        let b = self
                            .buffers
                            .get(buffer)
                            .ok_or(VulkanError::MissingResource(buffer.0))?;
                        unsafe { self.device.cmd_dispatch_indirect(cmd, b.buffer, *offset) };
                    }

                    VulkanPlanCommand::CopyBuffer {
                        source,
                        destination,
                        source_offset,
                        destination_offset,
                        bytes,
                    } => {
                        if *bytes == 0
                            || (source == destination && source_offset == destination_offset)
                        {
                            continue;
                        }
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "buffer copy during rendering",
                            ));
                        }
                        let s = self
                            .buffers
                            .get(source)
                            .ok_or(VulkanError::MissingResource(source.0))?;
                        let d = self
                            .buffers
                            .get(destination)
                            .ok_or(VulkanError::MissingResource(destination.0))?;
                        let region = [vk::BufferCopy {
                            src_offset: *source_offset,
                            dst_offset: *destination_offset,
                            size: *bytes,
                        }];
                        unsafe {
                            self.device
                                .cmd_copy_buffer(cmd, s.buffer, d.buffer, &region)
                        };
                    }
                    VulkanPlanCommand::FillBuffer {
                        destination,
                        value,
                        bytes,
                    } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "buffer fill during rendering",
                            ));
                        }
                        let d = self
                            .buffers
                            .get(destination)
                            .ok_or(VulkanError::MissingResource(destination.0))?;
                        unsafe {
                            self.device
                                .cmd_fill_buffer(cmd, d.buffer, 0, *bytes, *value)
                        };
                    }
                    VulkanPlanCommand::PipelineBarrier { scope }
                    | VulkanPlanCommand::CacheFlush { scope }
                    | VulkanPlanCommand::InvalidateCaches { scope } => {
                        let (src_stage, dst_stage, dst_access) = match scope {
                            exosphere_gpu_ir::SyncScope::Queue => (
                                vk::PipelineStageFlags2::ALL_GRAPHICS
                                    | vk::PipelineStageFlags2::COMPUTE_SHADER
                                    | vk::PipelineStageFlags2::TRANSFER,
                                vk::PipelineStageFlags2::ALL_GRAPHICS
                                    | vk::PipelineStageFlags2::COMPUTE_SHADER
                                    | vk::PipelineStageFlags2::TRANSFER,
                                vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
                            ),
                            exosphere_gpu_ir::SyncScope::Device => (
                                vk::PipelineStageFlags2::ALL_COMMANDS,
                                vk::PipelineStageFlags2::ALL_COMMANDS,
                                vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
                            ),
                            exosphere_gpu_ir::SyncScope::HostVisible => (
                                vk::PipelineStageFlags2::ALL_COMMANDS,
                                vk::PipelineStageFlags2::HOST,
                                vk::AccessFlags2::HOST_READ | vk::AccessFlags2::HOST_WRITE,
                            ),
                        };
                        let barrier = [vk::MemoryBarrier2::default()
                            .src_stage_mask(src_stage)
                            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                            .dst_stage_mask(dst_stage)
                            .dst_access_mask(dst_access)];
                        let dep = vk::DependencyInfo::default().memory_barriers(&barrier);
                        unsafe { self.device.cmd_pipeline_barrier2(cmd, &dep) };
                    }
                    VulkanPlanCommand::CopyImage {
                        source,
                        destination,
                    } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "image copy during rendering",
                            ));
                        }
                        self.transition_image(cmd, *source, vk::ImageLayout::TRANSFER_SRC_OPTIMAL)?;
                        self.transition_image(
                            cmd,
                            *destination,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        )?;
                        let s = self
                            .images
                            .get(source)
                            .ok_or(VulkanError::MissingResource(source.0))?;
                        let d = self
                            .images
                            .get(destination)
                            .ok_or(VulkanError::MissingResource(destination.0))?;
                        let extent = vk::Extent3D {
                            width: s.extent.width.min(d.extent.width),
                            height: s.extent.height.min(d.extent.height),
                            depth: s.extent.depth.min(d.extent.depth),
                        };
                        let r = [vk::ImageCopy::default()
                            .src_subresource(vk::ImageSubresourceLayers {
                                aspect_mask: s.aspect,
                                mip_level: 0,
                                base_array_layer: 0,
                                layer_count: s.layers.min(d.layers),
                            })
                            .dst_subresource(vk::ImageSubresourceLayers {
                                aspect_mask: d.aspect,
                                mip_level: 0,
                                base_array_layer: 0,
                                layer_count: s.layers.min(d.layers),
                            })
                            .extent(extent)];
                        unsafe {
                            self.device.cmd_copy_image(
                                cmd,
                                s.image,
                                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                                d.image,
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                &r,
                            )
                        };
                    }
                    VulkanPlanCommand::Blit {
                        source,
                        destination,
                        filter,
                    } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature("blit during rendering"));
                        }
                        self.transition_image(cmd, *source, vk::ImageLayout::TRANSFER_SRC_OPTIMAL)?;
                        self.transition_image(
                            cmd,
                            *destination,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        )?;
                        let s = self
                            .images
                            .get(source)
                            .ok_or(VulkanError::MissingResource(source.0))?;
                        let d = self
                            .images
                            .get(destination)
                            .ok_or(VulkanError::MissingResource(destination.0))?;
                        let r = [vk::ImageBlit::default()
                            .src_subresource(vk::ImageSubresourceLayers {
                                aspect_mask: s.aspect,
                                mip_level: 0,
                                base_array_layer: 0,
                                layer_count: 1,
                            })
                            .src_offsets([
                                vk::Offset3D { x: 0, y: 0, z: 0 },
                                vk::Offset3D {
                                    x: s.extent.width as i32,
                                    y: s.extent.height as i32,
                                    z: s.extent.depth as i32,
                                },
                            ])
                            .dst_subresource(vk::ImageSubresourceLayers {
                                aspect_mask: d.aspect,
                                mip_level: 0,
                                base_array_layer: 0,
                                layer_count: 1,
                            })
                            .dst_offsets([
                                vk::Offset3D { x: 0, y: 0, z: 0 },
                                vk::Offset3D {
                                    x: d.extent.width as i32,
                                    y: d.extent.height as i32,
                                    z: d.extent.depth as i32,
                                },
                            ])];
                        let f = match filter {
                            exosphere_gpu_ir::FilterMode::Nearest => vk::Filter::NEAREST,
                            exosphere_gpu_ir::FilterMode::Linear => vk::Filter::LINEAR,
                        };
                        unsafe {
                            self.device.cmd_blit_image(
                                cmd,
                                s.image,
                                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                                d.image,
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                &r,
                                f,
                            )
                        };
                    }
                    VulkanPlanCommand::Resolve {
                        source,
                        destination,
                    } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "resolve during rendering",
                            ));
                        }
                        self.transition_image(cmd, *source, vk::ImageLayout::TRANSFER_SRC_OPTIMAL)?;
                        self.transition_image(
                            cmd,
                            *destination,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        )?;
                        let s = self
                            .images
                            .get(source)
                            .ok_or(VulkanError::MissingResource(source.0))?;
                        let d = self
                            .images
                            .get(destination)
                            .ok_or(VulkanError::MissingResource(destination.0))?;
                        let r = [vk::ImageResolve::default()
                            .src_subresource(vk::ImageSubresourceLayers {
                                aspect_mask: s.aspect,
                                mip_level: 0,
                                base_array_layer: 0,
                                layer_count: s.layers.min(d.layers),
                            })
                            .dst_subresource(vk::ImageSubresourceLayers {
                                aspect_mask: d.aspect,
                                mip_level: 0,
                                base_array_layer: 0,
                                layer_count: s.layers.min(d.layers),
                            })
                            .extent(vk::Extent3D {
                                width: s.extent.width.min(d.extent.width),
                                height: s.extent.height.min(d.extent.height),
                                depth: 1,
                            })];
                        unsafe {
                            self.device.cmd_resolve_image(
                                cmd,
                                s.image,
                                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                                d.image,
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                &r,
                            )
                        };
                    }
                    VulkanPlanCommand::ClearColor { target, value } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "clear image during rendering; use attachment clear lowering",
                            ));
                        }
                        self.transition_image(cmd, *target, vk::ImageLayout::TRANSFER_DST_OPTIMAL)?;
                        let i = self
                            .images
                            .get(target)
                            .ok_or(VulkanError::MissingResource(target.0))?;
                        let c = vk::ClearColorValue { uint32: *value };
                        let range = [vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: i.mip_levels,
                            base_array_layer: 0,
                            layer_count: i.layers,
                        }];
                        unsafe {
                            self.device.cmd_clear_color_image(
                                cmd,
                                i.image,
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                &c,
                                &range,
                            )
                        };
                    }
                    VulkanPlanCommand::ClearDepthStencil {
                        target,
                        depth_bits,
                        stencil,
                        ..
                    } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "clear depth/stencil during rendering; use attachment clear lowering",
                            ));
                        }
                        self.transition_image(cmd, *target, vk::ImageLayout::TRANSFER_DST_OPTIMAL)?;
                        let i = self
                            .images
                            .get(target)
                            .ok_or(VulkanError::MissingResource(target.0))?;
                        let c = vk::ClearDepthStencilValue {
                            depth: f32::from_bits(*depth_bits),
                            stencil: *stencil,
                        };
                        let range = [vk::ImageSubresourceRange {
                            aspect_mask: i.aspect,
                            base_mip_level: 0,
                            level_count: i.mip_levels,
                            base_array_layer: 0,
                            layer_count: i.layers,
                        }];
                        unsafe {
                            self.device.cmd_clear_depth_stencil_image(
                                cmd,
                                i.image,
                                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                                &c,
                                &range,
                            )
                        };
                    }
                    VulkanPlanCommand::SignalSemaphore { address, payload } => {
                        self.guest_semaphores
                            .entry(*address)
                            .and_modify(|v| *v = (*v).max(*payload))
                            .or_insert(*payload);
                    }
                    VulkanPlanCommand::WaitSemaphore { address, payload } => {
                        let current = self.guest_semaphores.get(address).copied().unwrap_or(0);
                        if current < *payload {
                            return Err(VulkanError::Driver(format!(
                                "guest semaphore {address:#x} wait {payload} is unsatisfied (current {current})"
                            )));
                        }
                    }
                    VulkanPlanCommand::IncrementSyncpoint { id, amount } => {
                        let value = self.guest_syncpoints.entry(*id).or_insert(0);
                        *value = value.saturating_add(*amount);
                    }
                    VulkanPlanCommand::WaitSyncpoint { id, threshold } => {
                        let current = self.guest_syncpoints.get(id).copied().unwrap_or(0);
                        if current < *threshold {
                            return Err(VulkanError::Driver(format!(
                                "guest syncpoint {id} wait {threshold} is unsatisfied (current {current})"
                            )));
                        }
                    }
                    VulkanPlanCommand::ResetQuery { pool, first, count } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "reset query during rendering",
                            ));
                        }
                        // Reset does not encode the kind, so an existing pool is required. If the first
                        // operation is ResetQuery, defer the reset until BeginQuery/WriteTimestamp creates it.
                        if let Some(q) = self.query_pools.get(pool) {
                            unsafe {
                                self.device
                                    .cmd_reset_query_pool(cmd, q.pool, *first, *count)
                            }
                        }
                    }
                    VulkanPlanCommand::BeginQuery { pool, index, kind } => {
                        if matches!(kind, QueryKind::Timestamp) {
                            return Err(VulkanError::UnsupportedFeature(
                                "timestamps use WriteTimestamp, not BeginQuery",
                            ));
                        }
                        let q = self.ensure_query_pool(*pool, *kind, index.saturating_add(1))?;
                        if matches!(kind, QueryKind::TransformFeedback) {
                            let ext = self.transform_feedback.as_ref().ok_or(
                                VulkanError::UnsupportedFeature("VK_EXT_transform_feedback"),
                            )?;
                            unsafe {
                                (ext.fp().cmd_begin_query_indexed_ext)(
                                    cmd,
                                    q,
                                    *index,
                                    vk::QueryControlFlags::empty(),
                                    0,
                                )
                            }
                        } else {
                            unsafe {
                                self.device.cmd_begin_query(
                                    cmd,
                                    q,
                                    *index,
                                    vk::QueryControlFlags::empty(),
                                )
                            }
                        };
                    }
                    VulkanPlanCommand::EndQuery { pool, index, kind } => {
                        if matches!(kind, QueryKind::Timestamp) {
                            return Err(VulkanError::UnsupportedFeature(
                                "timestamps use WriteTimestamp, not EndQuery",
                            ));
                        }
                        let q = self.ensure_query_pool(*pool, *kind, index.saturating_add(1))?;
                        if matches!(kind, QueryKind::TransformFeedback) {
                            let ext = self.transform_feedback.as_ref().ok_or(
                                VulkanError::UnsupportedFeature("VK_EXT_transform_feedback"),
                            )?;
                            unsafe { (ext.fp().cmd_end_query_indexed_ext)(cmd, q, *index, 0) }
                        } else {
                            unsafe { self.device.cmd_end_query(cmd, q, *index) }
                        };
                    }
                    VulkanPlanCommand::WriteTimestamp { pool, index } => {
                        let q = self.ensure_query_pool(
                            *pool,
                            QueryKind::Timestamp,
                            index.saturating_add(1),
                        )?;
                        unsafe {
                            self.device.cmd_write_timestamp2(
                                cmd,
                                vk::PipelineStageFlags2::ALL_COMMANDS,
                                q,
                                *index,
                            )
                        };
                    }
                    VulkanPlanCommand::CopyQueryResults {
                        pool,
                        destination,
                        first,
                        count,
                        offset,
                    } => {
                        if attachments.active {
                            return Err(VulkanError::UnsupportedFeature(
                                "query result copy during rendering",
                            ));
                        }
                        let q =
                            self.query_pools
                                .get(pool)
                                .ok_or(VulkanError::UnsupportedFeature(
                                    "query result copy before query pool creation",
                                ))?;
                        let dst = self
                            .buffers
                            .get(destination)
                            .ok_or(VulkanError::MissingResource(destination.0))?;
                        unsafe {
                            self.device.cmd_copy_query_pool_results(
                                cmd,
                                q.pool,
                                *first,
                                *count,
                                dst.buffer,
                                *offset,
                                8,
                                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
                            )
                        };
                    }
                    VulkanPlanCommand::BeginConditional {
                        buffer,
                        offset,
                        mode,
                    } => {
                        let ext = self.conditional_rendering.as_ref().ok_or(
                            VulkanError::UnsupportedFeature("VK_EXT_conditional_rendering"),
                        )?;
                        let b = self
                            .buffers
                            .get(buffer)
                            .ok_or(VulkanError::MissingResource(buffer.0))?;
                        let flags = match mode {
                            exosphere_gpu_ir::ConditionalMode::Inverted => {
                                vk::ConditionalRenderingFlagsEXT::INVERTED
                            }
                            exosphere_gpu_ir::ConditionalMode::Wait
                            | exosphere_gpu_ir::ConditionalMode::NoWait
                            | exosphere_gpu_ir::ConditionalMode::ByRegion => {
                                vk::ConditionalRenderingFlagsEXT::empty()
                            }
                            exosphere_gpu_ir::ConditionalMode::Unknown(_) => {
                                return Err(VulkanError::UnsupportedFeature(
                                    "unknown conditional mode",
                                ));
                            }
                        };
                        let info = vk::ConditionalRenderingBeginInfoEXT::default()
                            .buffer(b.buffer)
                            .offset(*offset)
                            .flags(flags);
                        unsafe { (ext.fp().cmd_begin_conditional_rendering_ext)(cmd, &info) };
                    }
                    VulkanPlanCommand::EndConditional => {
                        let ext = self.conditional_rendering.as_ref().ok_or(
                            VulkanError::UnsupportedFeature("VK_EXT_conditional_rendering"),
                        )?;
                        unsafe { (ext.fp().cmd_end_conditional_rendering_ext)(cmd) }
                    }
                    VulkanPlanCommand::BeginTransformFeedback { buffers } => {
                        let ext = self
                            .transform_feedback
                            .as_ref()
                            .ok_or(VulkanError::UnsupportedFeature("VK_EXT_transform_feedback"))?;
                        let mut handles = Vec::with_capacity(buffers.len());
                        let mut offsets = Vec::with_capacity(buffers.len());
                        let mut sizes = Vec::with_capacity(buffers.len());
                        for (id, offset, size) in buffers {
                            let b = self
                                .buffers
                                .get(id)
                                .ok_or(VulkanError::MissingResource(id.0))?;
                            handles.push(b.buffer);
                            offsets.push(*offset);
                            sizes.push(*size)
                        }
                        unsafe {
                            (ext.fp().cmd_bind_transform_feedback_buffers_ext)(
                                cmd,
                                0,
                                handles.len() as u32,
                                handles.as_ptr(),
                                offsets.as_ptr(),
                                sizes.as_ptr(),
                            );
                            (ext.fp().cmd_begin_transform_feedback_ext)(
                                cmd,
                                0,
                                0,
                                ptr::null(),
                                ptr::null(),
                            )
                        };
                    }
                    VulkanPlanCommand::EndTransformFeedback => {
                        let ext = self
                            .transform_feedback
                            .as_ref()
                            .ok_or(VulkanError::UnsupportedFeature("VK_EXT_transform_feedback"))?;
                        unsafe {
                            (ext.fp().cmd_end_transform_feedback_ext)(
                                cmd,
                                0,
                                0,
                                ptr::null(),
                                ptr::null(),
                            )
                        }
                    }
                }
            }
            if attachments.active {
                return Err(VulkanError::Driver(
                    "command stream ended inside dynamic rendering".into(),
                ));
            }
            // Descriptor pools must outlive every descriptor set referenced by this submission.
            self.frames[frame_index]
                .descriptor_pools
                .append(&mut descriptor_pools);
            let timeline = self.submit_frame(frame_index)?;
            let usage = resource_validation::analyze(plan)?;
            for id in &usage.written_buffers {
                if let Some(buffer) = self.buffers.get_mut(id) {
                    buffer.content_fingerprint = None;
                }
            }
            self.pending_writebacks
                .insert(timeline, usage.written_buffers.into_iter().collect());
            Ok(ExecutionToken {
                timeline_value: timeline,
            })
        })();

        if let Err(error) = &record_result {
            if !self.submission_failed {
                // Layout updates described unsubmitted commands, not changes
                // executed by the GPU. Restore the last actual layout state.
                for (id, layout) in image_layouts {
                    if let Some(image) = self.images.get_mut(&id) {
                        image.layout = layout;
                    }
                }
            }
            unsafe {
                for pool in descriptor_pools.drain(..) {
                    self.device.destroy_descriptor_pool(pool, None)
                }
            }
            return Err(error.clone());
        }

        record_result
    }

    fn writeback_resources(
        &self,
        ids: &[ResourceId],
        provider: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        for &id in ids {
            if let Some(buf) = self.buffers.get(&id) {
                let len = provider.len(id).unwrap_or(buf.size).min(buf.size);
                if len == 0 {
                    continue;
                }
                let ptr = unsafe {
                    self.device
                        .map_memory(buf.memory, 0, len, vk::MemoryMapFlags::empty())
                }
                .map_err(|e| VulkanError::Driver(format!("vkMapMemory(readback): {e:?}")))?;
                let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
                let write_result = provider
                    .write(id, 0, bytes)
                    .map_err(|e| VulkanError::ResourceData(e.to_string()));
                // Guest writeback may reject the range. Always release the
                // native mapping before propagating that error to the caller.
                unsafe { self.device.unmap_memory(buf.memory) };
                write_result?;
                provider
                    .flush(id)
                    .map_err(|e| VulkanError::ResourceData(e.to_string()))?;
            }
        }
        Ok(())
    }
}

impl VulkanDevice for AshVulkanDevice {
    fn name(&self) -> &str {
        "ash-vulkan-1.3"
    }
    fn capabilities(&self) -> VulkanCapabilities {
        VulkanCapabilities {
            graphics: FeatureSupport::Native,
            compute: FeatureSupport::Native,
            transfer: FeatureSupport::Native,
            indirect: FeatureSupport::Native,
            queries: FeatureSupport::Native,
            conditional_rendering: if self.conditional_rendering.is_some() {
                FeatureSupport::Native
            } else {
                FeatureSupport::Unsupported
            },
            transform_feedback: if self.transform_feedback.is_some() {
                FeatureSupport::Native
            } else {
                FeatureSupport::Unsupported
            },
            synchronization: FeatureSupport::Native,
            compressed_textures: FeatureSupport::Native,
        }
    }
    fn execute_plan(&mut self, _: &VulkanExecutionPlan) -> Result<(), VulkanError> {
        Err(VulkanError::ResourceData(
            "Ash backend requires resource provider".into(),
        ))
    }
    fn execute_plan_with_resources(
        &mut self,
        plan: &VulkanExecutionPlan,
        resources: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        let token = self.submit_plan_with_resources(plan, resources)?;
        self.complete_execution(token, resources)
    }
    fn submit_plan_with_resources(
        &mut self,
        plan: &VulkanExecutionPlan,
        resources: &mut dyn GpuResourceData,
    ) -> Result<ExecutionToken, VulkanError> {
        resource_validation::validate_payloads(plan, resources)?;
        self.submit_core(plan, resources)
    }
    fn is_execution_complete(&self, token: ExecutionToken) -> Result<bool, VulkanError> {
        if token.timeline_value == 0 {
            return Ok(true);
        }
        self.is_timeline_complete(token.timeline_value)
    }
    fn complete_execution(
        &mut self,
        token: ExecutionToken,
        resources: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        if token.timeline_value == 0 {
            return Ok(());
        }
        self.wait_timeline(token.timeline_value, u64::MAX)?;
        let ids = self
            .pending_writebacks
            .remove(&token.timeline_value)
            .unwrap_or_default();
        self.writeback_resources(&ids, resources)
    }
}

impl Drop for AshVulkanDevice {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            // Drain while VkDevice and the one-shot command pool are alive.
            // Device loss also permits releasing resources during teardown.
            for upload in self.retained_texture_uploads.drain(..) {
                self.device.destroy_fence(upload.fence, None);
                self.device
                    .free_command_buffers(self.command_pool, &[upload.command]);
                drop(upload);
            }
            for frame in &mut self.frames {
                for pool in frame.descriptor_pools.drain(..) {
                    self.device.destroy_descriptor_pool(pool, None)
                }
                self.device.destroy_fence(frame.fence, None);
                self.device.destroy_command_pool(frame.command_pool, None);
            }
            self.device
                .destroy_pipeline_cache(self.pipeline_cache, None);
            self.device.destroy_semaphore(self.submit_timeline, None);
            for q in self.query_pools.values() {
                self.device.destroy_query_pool(q.pool, None)
            }
            for p in self.graphics_pipelines.values() {
                self.device.destroy_pipeline(*p, None)
            }
            for p in self.compute_pipelines.values() {
                self.device.destroy_pipeline(*p, None)
            }
            for l in self.pipeline_layouts.values() {
                self.device.destroy_pipeline_layout(l.layout, None);
                for set in &l.set_layouts {
                    self.device.destroy_descriptor_set_layout(*set, None)
                }
            }
            for m in self.shader_modules.values() {
                self.device.destroy_shader_module(*m, None)
            }
            for s in self.samplers.values() {
                self.device.destroy_sampler(s.sampler, None)
            }
            for i in self.images.values() {
                self.device.destroy_image_view(i.view, None);
                self.device.destroy_image(i.image, None);
                self.device.free_memory(i.memory, None)
            }
            for b in self.buffers.values() {
                self.device.destroy_buffer(b.buffer, None);
                self.device.free_memory(b.memory, None)
            }
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None)
        }
    }
}
