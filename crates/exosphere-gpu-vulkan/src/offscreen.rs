// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Minimal real Vulkan graphics draw used by the Phase 5 smoke path.
//!
//! This module deliberately renders to an offscreen RGBA8 image. Window-system integration and
//! swapchains belong to the platform layer; proving `Shader IR -> SPIR-V -> VkPipeline -> vkCmdDraw`
//! does not require a window and therefore remains usable in CI/headless Linux environments.
//!
//! The Vulkan ABI declarations below are a deliberately tiny subset of Vulkan 1.0. Their layouts
//! follow the Khronos Vulkan headers. Canonical Exosphere GPU IR never stores these structures.

use crate::{
    plan::{VulkanExecutionPlan, VulkanPlanCommand},
    raw_instance::RawVulkanInstance,
};
use exosphere_gpu_ir::{GpuResourceData, ResourceId, ShaderStage};
use libloading::{Library, Symbol};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CString, c_char, c_void},
    mem, ptr,
};

type VkResult = i32;
type VkDeviceSize = u64;
type VkImage = *mut c_void;
type VkBuffer = *mut c_void;
type VkDeviceMemory = *mut c_void;
type VkImageView = *mut c_void;
type VkRenderPass = *mut c_void;
type VkFramebuffer = *mut c_void;
type VkShaderModule = *mut c_void;
type VkPipelineLayout = *mut c_void;
type VkPipeline = *mut c_void;
type VkPipelineCache = *mut c_void;
type VkCommandPool = *mut c_void;
type VkCommandBuffer = *mut c_void;
type VkFence = *mut c_void;

const VK_SUCCESS: VkResult = 0;
const VK_STRUCTURE_TYPE_SUBMIT_INFO: u32 = 4;
const VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO: u32 = 5;
const VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO: u32 = 12;
const VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO: u32 = 14;
const VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO: u32 = 15;
const VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO: u32 = 16;
const VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO: u32 = 18;
const VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO: u32 = 19;
const VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO: u32 = 20;
const VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO: u32 = 22;
const VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO: u32 = 23;
const VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO: u32 = 24;
const VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: u32 = 26;
const VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO: u32 = 28;
const VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO: u32 = 30;
const VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO: u32 = 37;
const VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO: u32 = 38;
const VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO: u32 = 39;
const VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO: u32 = 40;
const VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO: u32 = 42;
const VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO: u32 = 43;

const VK_IMAGE_TYPE_2D: u32 = 1;
const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
const VK_SAMPLE_COUNT_1_BIT: u32 = 1;
const VK_IMAGE_TILING_OPTIMAL: u32 = 0;
const VK_IMAGE_USAGE_TRANSFER_SRC_BIT: u32 = 0x1;
const VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT: u32 = 0x10;
const VK_BUFFER_USAGE_TRANSFER_SRC_BIT: u32 = 0x1;
const VK_BUFFER_USAGE_TRANSFER_DST_BIT: u32 = 0x2;
const VK_SHARING_MODE_EXCLUSIVE: u32 = 0;
const VK_IMAGE_LAYOUT_UNDEFINED: u32 = 0;
const VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL: u32 = 2;
const VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL: u32 = 6;
const VK_IMAGE_VIEW_TYPE_2D: u32 = 1;
const VK_IMAGE_ASPECT_COLOR_BIT: u32 = 0x1;
const VK_COMPONENT_SWIZZLE_IDENTITY: u32 = 0;
const VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT: u32 = 0x1;
const VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT: u32 = 0x2;
const VK_MEMORY_PROPERTY_HOST_COHERENT_BIT: u32 = 0x4;
const VK_ATTACHMENT_LOAD_OP_CLEAR: u32 = 1;
const VK_ATTACHMENT_STORE_OP_STORE: u32 = 0;
const VK_ATTACHMENT_LOAD_OP_DONT_CARE: u32 = 2;
const VK_ATTACHMENT_STORE_OP_DONT_CARE: u32 = 1;
const VK_PIPELINE_BIND_POINT_GRAPHICS: u32 = 0;
const VK_SHADER_STAGE_VERTEX_BIT: u32 = 0x1;
const VK_SHADER_STAGE_FRAGMENT_BIT: u32 = 0x10;
const VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST: u32 = 3;
const VK_POLYGON_MODE_FILL: u32 = 0;
const VK_CULL_MODE_NONE: u32 = 0;
const VK_FRONT_FACE_COUNTER_CLOCKWISE: u32 = 0;
const VK_COLOR_COMPONENT_R_BIT: u32 = 0x1;
const VK_COLOR_COMPONENT_G_BIT: u32 = 0x2;
const VK_COLOR_COMPONENT_B_BIT: u32 = 0x4;
const VK_COLOR_COMPONENT_A_BIT: u32 = 0x8;
const VK_COMMAND_BUFFER_LEVEL_PRIMARY: u32 = 0;
const VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT: u32 = 0x1;
const VK_PIPELINE_STAGE_TRANSFER_BIT: u32 = 0x0000_1000;
const VK_ACCESS_TRANSFER_READ_BIT: u32 = 0x0000_0800;
const VK_ACCESS_TRANSFER_WRITE_BIT: u32 = 0x0000_1000;
const VK_STRUCTURE_TYPE_MEMORY_BARRIER: u32 = 46;
const VK_SUBPASS_CONTENTS_INLINE: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkExtent2D {
    width: u32,
    height: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkExtent3D {
    width: u32,
    height: u32,
    depth: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkOffset2D {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkRect2D {
    offset: VkOffset2D,
    extent: VkExtent2D,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkViewport {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    min_depth: f32,
    max_depth: f32,
}

#[repr(C)]
struct VkImageCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    image_type: u32,
    format: u32,
    extent: VkExtent3D,
    mip_levels: u32,
    array_layers: u32,
    samples: u32,
    tiling: u32,
    usage: u32,
    sharing_mode: u32,
    queue_family_index_count: u32,
    p_queue_family_indices: *const u32,
    initial_layout: u32,
}
#[repr(C)]
struct VkBufferCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    size: VkDeviceSize,
    usage: u32,
    sharing_mode: u32,
    queue_family_index_count: u32,
    p_queue_family_indices: *const u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkImageSubresourceLayers {
    aspect_mask: u32,
    mip_level: u32,
    base_array_layer: u32,
    layer_count: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkOffset3D {
    x: i32,
    y: i32,
    z: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkBufferImageCopy {
    buffer_offset: VkDeviceSize,
    buffer_row_length: u32,
    buffer_image_height: u32,
    image_subresource: VkImageSubresourceLayers,
    image_offset: VkOffset3D,
    image_extent: VkExtent3D,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkBufferCopy {
    src_offset: VkDeviceSize,
    dst_offset: VkDeviceSize,
    size: VkDeviceSize,
}
#[repr(C)]
struct VkMemoryBarrier {
    s_type: u32,
    p_next: *const c_void,
    src_access_mask: u32,
    dst_access_mask: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkMemoryRequirements {
    size: VkDeviceSize,
    alignment: VkDeviceSize,
    memory_type_bits: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkMemoryType {
    property_flags: u32,
    heap_index: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkMemoryHeap {
    size: VkDeviceSize,
    flags: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkPhysicalDeviceMemoryProperties {
    memory_type_count: u32,
    memory_types: [VkMemoryType; 32],
    memory_heap_count: u32,
    memory_heaps: [VkMemoryHeap; 16],
}
#[repr(C)]
struct VkMemoryAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    allocation_size: VkDeviceSize,
    memory_type_index: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkComponentMapping {
    r: u32,
    g: u32,
    b: u32,
    a: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkImageSubresourceRange {
    aspect_mask: u32,
    base_mip_level: u32,
    level_count: u32,
    base_array_layer: u32,
    layer_count: u32,
}
#[repr(C)]
struct VkImageViewCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    image: VkImage,
    view_type: u32,
    format: u32,
    components: VkComponentMapping,
    subresource_range: VkImageSubresourceRange,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkAttachmentDescription {
    flags: u32,
    format: u32,
    samples: u32,
    load_op: u32,
    store_op: u32,
    stencil_load_op: u32,
    stencil_store_op: u32,
    initial_layout: u32,
    final_layout: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkAttachmentReference {
    attachment: u32,
    layout: u32,
}
#[repr(C)]
struct VkSubpassDescription {
    flags: u32,
    pipeline_bind_point: u32,
    input_attachment_count: u32,
    p_input_attachments: *const VkAttachmentReference,
    color_attachment_count: u32,
    p_color_attachments: *const VkAttachmentReference,
    p_resolve_attachments: *const VkAttachmentReference,
    p_depth_stencil_attachment: *const VkAttachmentReference,
    preserve_attachment_count: u32,
    p_preserve_attachments: *const u32,
}
#[repr(C)]
struct VkRenderPassCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    attachment_count: u32,
    p_attachments: *const VkAttachmentDescription,
    subpass_count: u32,
    p_subpasses: *const VkSubpassDescription,
    dependency_count: u32,
    p_dependencies: *const c_void,
}
#[repr(C)]
struct VkFramebufferCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    render_pass: VkRenderPass,
    attachment_count: u32,
    p_attachments: *const VkImageView,
    width: u32,
    height: u32,
    layers: u32,
}
#[repr(C)]
struct VkShaderModuleCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    code_size: usize,
    p_code: *const u32,
}
#[repr(C)]
struct VkPipelineShaderStageCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    stage: u32,
    module: VkShaderModule,
    p_name: *const c_char,
    p_specialization_info: *const c_void,
}
#[repr(C)]
struct VkPipelineLayoutCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    set_layout_count: u32,
    p_set_layouts: *const *mut c_void,
    push_constant_range_count: u32,
    p_push_constant_ranges: *const c_void,
}
#[repr(C)]
struct VkPipelineVertexInputStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    vertex_binding_description_count: u32,
    p_vertex_binding_descriptions: *const c_void,
    vertex_attribute_description_count: u32,
    p_vertex_attribute_descriptions: *const c_void,
}
#[repr(C)]
struct VkPipelineInputAssemblyStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    topology: u32,
    primitive_restart_enable: u32,
}
#[repr(C)]
struct VkPipelineViewportStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    viewport_count: u32,
    p_viewports: *const VkViewport,
    scissor_count: u32,
    p_scissors: *const VkRect2D,
}
#[repr(C)]
struct VkPipelineRasterizationStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    depth_clamp_enable: u32,
    rasterizer_discard_enable: u32,
    polygon_mode: u32,
    cull_mode: u32,
    front_face: u32,
    depth_bias_enable: u32,
    depth_bias_constant_factor: f32,
    depth_bias_clamp: f32,
    depth_bias_slope_factor: f32,
    line_width: f32,
}
#[repr(C)]
struct VkPipelineMultisampleStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    rasterization_samples: u32,
    sample_shading_enable: u32,
    min_sample_shading: f32,
    p_sample_mask: *const u32,
    alpha_to_coverage_enable: u32,
    alpha_to_one_enable: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkPipelineColorBlendAttachmentState {
    blend_enable: u32,
    src_color_blend_factor: u32,
    dst_color_blend_factor: u32,
    color_blend_op: u32,
    src_alpha_blend_factor: u32,
    dst_alpha_blend_factor: u32,
    alpha_blend_op: u32,
    color_write_mask: u32,
}
#[repr(C)]
struct VkPipelineColorBlendStateCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    logic_op_enable: u32,
    logic_op: u32,
    attachment_count: u32,
    p_attachments: *const VkPipelineColorBlendAttachmentState,
    blend_constants: [f32; 4],
}
#[repr(C)]
struct VkGraphicsPipelineCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    stage_count: u32,
    p_stages: *const VkPipelineShaderStageCreateInfo,
    p_vertex_input_state: *const VkPipelineVertexInputStateCreateInfo,
    p_input_assembly_state: *const VkPipelineInputAssemblyStateCreateInfo,
    p_tessellation_state: *const c_void,
    p_viewport_state: *const VkPipelineViewportStateCreateInfo,
    p_rasterization_state: *const VkPipelineRasterizationStateCreateInfo,
    p_multisample_state: *const VkPipelineMultisampleStateCreateInfo,
    p_depth_stencil_state: *const c_void,
    p_color_blend_state: *const VkPipelineColorBlendStateCreateInfo,
    p_dynamic_state: *const c_void,
    layout: VkPipelineLayout,
    render_pass: VkRenderPass,
    subpass: u32,
    base_pipeline_handle: VkPipeline,
    base_pipeline_index: i32,
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
#[derive(Clone, Copy)]
union VkClearColorValue {
    float32: [f32; 4],
    int32: [i32; 4],
    uint32: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy)]
union VkClearValue {
    color: VkClearColorValue,
    depth_stencil: [u32; 2],
}
#[repr(C)]
struct VkRenderPassBeginInfo {
    s_type: u32,
    p_next: *const c_void,
    render_pass: VkRenderPass,
    framebuffer: VkFramebuffer,
    render_area: VkRect2D,
    clear_value_count: u32,
    p_clear_values: *const VkClearValue,
}
#[repr(C)]
struct VkSubmitInfo {
    s_type: u32,
    p_next: *const c_void,
    wait_semaphore_count: u32,
    p_wait_semaphores: *const *mut c_void,
    p_wait_dst_stage_mask: *const u32,
    command_buffer_count: u32,
    p_command_buffers: *const VkCommandBuffer,
    signal_semaphore_count: u32,
    p_signal_semaphores: *const *mut c_void,
}

type VkCreateImage = unsafe extern "system" fn(
    *mut c_void,
    *const VkImageCreateInfo,
    *const c_void,
    *mut VkImage,
) -> VkResult;
type VkCreateBuffer = unsafe extern "system" fn(
    *mut c_void,
    *const VkBufferCreateInfo,
    *const c_void,
    *mut VkBuffer,
) -> VkResult;
type VkDestroyBuffer = unsafe extern "system" fn(*mut c_void, VkBuffer, *const c_void);
type VkGetBufferMemoryRequirements =
    unsafe extern "system" fn(*mut c_void, VkBuffer, *mut VkMemoryRequirements);
type VkBindBufferMemory =
    unsafe extern "system" fn(*mut c_void, VkBuffer, VkDeviceMemory, VkDeviceSize) -> VkResult;
type VkMapMemory = unsafe extern "system" fn(
    *mut c_void,
    VkDeviceMemory,
    VkDeviceSize,
    VkDeviceSize,
    u32,
    *mut *mut c_void,
) -> VkResult;
type VkUnmapMemory = unsafe extern "system" fn(*mut c_void, VkDeviceMemory);
type VkDestroyImage = unsafe extern "system" fn(*mut c_void, VkImage, *const c_void);
type VkGetImageMemoryRequirements =
    unsafe extern "system" fn(*mut c_void, VkImage, *mut VkMemoryRequirements);
type VkGetPhysicalDeviceMemoryProperties =
    unsafe extern "system" fn(*mut c_void, *mut VkPhysicalDeviceMemoryProperties);
type VkAllocateMemory = unsafe extern "system" fn(
    *mut c_void,
    *const VkMemoryAllocateInfo,
    *const c_void,
    *mut VkDeviceMemory,
) -> VkResult;
type VkFreeMemory = unsafe extern "system" fn(*mut c_void, VkDeviceMemory, *const c_void);
type VkBindImageMemory =
    unsafe extern "system" fn(*mut c_void, VkImage, VkDeviceMemory, VkDeviceSize) -> VkResult;
type VkCreateImageView = unsafe extern "system" fn(
    *mut c_void,
    *const VkImageViewCreateInfo,
    *const c_void,
    *mut VkImageView,
) -> VkResult;
type VkDestroyImageView = unsafe extern "system" fn(*mut c_void, VkImageView, *const c_void);
type VkCreateRenderPass = unsafe extern "system" fn(
    *mut c_void,
    *const VkRenderPassCreateInfo,
    *const c_void,
    *mut VkRenderPass,
) -> VkResult;
type VkDestroyRenderPass = unsafe extern "system" fn(*mut c_void, VkRenderPass, *const c_void);
type VkCreateFramebuffer = unsafe extern "system" fn(
    *mut c_void,
    *const VkFramebufferCreateInfo,
    *const c_void,
    *mut VkFramebuffer,
) -> VkResult;
type VkDestroyFramebuffer = unsafe extern "system" fn(*mut c_void, VkFramebuffer, *const c_void);
type VkCreateShaderModule = unsafe extern "system" fn(
    *mut c_void,
    *const VkShaderModuleCreateInfo,
    *const c_void,
    *mut VkShaderModule,
) -> VkResult;
type VkDestroyShaderModule = unsafe extern "system" fn(*mut c_void, VkShaderModule, *const c_void);
type VkCreatePipelineLayout = unsafe extern "system" fn(
    *mut c_void,
    *const VkPipelineLayoutCreateInfo,
    *const c_void,
    *mut VkPipelineLayout,
) -> VkResult;
type VkDestroyPipelineLayout =
    unsafe extern "system" fn(*mut c_void, VkPipelineLayout, *const c_void);
type VkCreateGraphicsPipelines = unsafe extern "system" fn(
    *mut c_void,
    VkPipelineCache,
    u32,
    *const VkGraphicsPipelineCreateInfo,
    *const c_void,
    *mut VkPipeline,
) -> VkResult;
type VkDestroyPipeline = unsafe extern "system" fn(*mut c_void, VkPipeline, *const c_void);
type VkCreateCommandPool = unsafe extern "system" fn(
    *mut c_void,
    *const VkCommandPoolCreateInfo,
    *const c_void,
    *mut VkCommandPool,
) -> VkResult;
type VkDestroyCommandPool = unsafe extern "system" fn(*mut c_void, VkCommandPool, *const c_void);
type VkAllocateCommandBuffers = unsafe extern "system" fn(
    *mut c_void,
    *const VkCommandBufferAllocateInfo,
    *mut VkCommandBuffer,
) -> VkResult;
type VkBeginCommandBuffer =
    unsafe extern "system" fn(VkCommandBuffer, *const VkCommandBufferBeginInfo) -> VkResult;
type VkEndCommandBuffer = unsafe extern "system" fn(VkCommandBuffer) -> VkResult;
type VkCmdBeginRenderPass =
    unsafe extern "system" fn(VkCommandBuffer, *const VkRenderPassBeginInfo, u32);
type VkCmdEndRenderPass = unsafe extern "system" fn(VkCommandBuffer);
type VkCmdBindPipeline = unsafe extern "system" fn(VkCommandBuffer, u32, VkPipeline);
type VkCmdDraw = unsafe extern "system" fn(VkCommandBuffer, u32, u32, u32, u32);
type VkCmdCopyImageToBuffer = unsafe extern "system" fn(
    VkCommandBuffer,
    VkImage,
    u32,
    VkBuffer,
    u32,
    *const VkBufferImageCopy,
);
type VkCmdCopyBuffer =
    unsafe extern "system" fn(VkCommandBuffer, VkBuffer, VkBuffer, u32, *const VkBufferCopy);
type VkCmdFillBuffer =
    unsafe extern "system" fn(VkCommandBuffer, VkBuffer, VkDeviceSize, VkDeviceSize, u32);
type VkCmdPipelineBarrier = unsafe extern "system" fn(
    VkCommandBuffer,
    u32,
    u32,
    u32,
    u32,
    *const VkMemoryBarrier,
    u32,
    *const c_void,
    u32,
    *const c_void,
);
type VkQueueSubmit =
    unsafe extern "system" fn(*mut c_void, u32, *const VkSubmitInfo, VkFence) -> VkResult;
type VkQueueWaitIdle = unsafe extern "system" fn(*mut c_void) -> VkResult;

/// Summary returned by the real offscreen graphics smoke path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffscreenDrawReport {
    pub width: u32,
    pub height: u32,
    pub vertices: u32,
    pub shader_modules: u32,
    /// Number of read-back pixels whose red channel is clearly above the dark clear color.
    pub bright_pixels: u64,
}

impl RawVulkanInstance {
    /// Creates a complete graphics pipeline and submits one real offscreen draw.
    ///
    /// The method intentionally accepts Exosphere's Vulkan execution plan rather than canonical GPU
    /// IR. That preserves the compiler boundary: `GpuModule -> VulkanExecutionPlan` remains testable
    /// without Vulkan, while this method owns every raw driver object and lifetime.
    pub fn execute_offscreen_plan(
        &self,
        plan: &VulkanExecutionPlan,
    ) -> Result<OffscreenDrawReport, String> {
        crate::device::validate_baseline_plan(plan).map_err(|error| error.to_string())?;
        if self.device.is_null() || self.queue.is_null() || self.physical_device.is_null() {
            return Err("no graphics-capable Vulkan device is available".into());
        }

        let (width, height) = plan
            .commands
            .iter()
            .find_map(|command| {
                if let VulkanPlanCommand::BeginRendering { width, height } = command {
                    Some((*width, *height))
                } else {
                    None
                }
            })
            .ok_or_else(|| "Vulkan plan has no BeginRendering command".to_owned())?;
        if width == 0 || height == 0 {
            return Err("offscreen target dimensions must be non-zero".into());
        }
        if width > 4096 || height > 4096 {
            return Err("offscreen smoke target is limited to 4096x4096".into());
        }
        let readback_size = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "offscreen readback size overflow".to_owned())?;

        let vertices = plan
            .commands
            .iter()
            .find_map(|command| {
                if let VulkanPlanCommand::Draw { vertices, .. } = command {
                    Some(*vertices)
                } else {
                    None
                }
            })
            .ok_or_else(|| "Vulkan plan has no Draw command".to_owned())?;

        let vertex = plan
            .shaders
            .iter()
            .find(|shader| shader.stage == ShaderStage::Vertex)
            .ok_or_else(|| "Vulkan plan has no vertex shader".to_owned())?;
        let fragment = plan
            .shaders
            .iter()
            .find(|shader| shader.stage == ShaderStage::Fragment)
            .ok_or_else(|| "Vulkan plan has no fragment shader".to_owned())?;

        let f = VulkanFns::load(&self._library)?;
        let mut resources = OwnedResources::new(self.device, self.queue, &f);

        // 1. Offscreen color image and device-local backing memory.
        let image_info = VkImageCreateInfo {
            s_type: VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            image_type: VK_IMAGE_TYPE_2D,
            format: VK_FORMAT_R8G8B8A8_UNORM,
            extent: VkExtent3D {
                width,
                height,
                depth: 1,
            },
            mip_levels: 1,
            array_layers: 1,
            samples: VK_SAMPLE_COUNT_1_BIT,
            tiling: VK_IMAGE_TILING_OPTIMAL,
            usage: VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT,
            sharing_mode: VK_SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            p_queue_family_indices: ptr::null(),
            initial_layout: VK_IMAGE_LAYOUT_UNDEFINED,
        };
        check(
            unsafe {
                (f.create_image)(self.device, &image_info, ptr::null(), &mut resources.image)
            },
            "vkCreateImage",
        )?;
        if resources.image.is_null() {
            return Err("vkCreateImage returned a null handle".into());
        }

        let mut requirements = VkMemoryRequirements::default();
        unsafe {
            (f.get_image_memory_requirements)(self.device, resources.image, &mut requirements)
        };
        let mut memory_properties = VkPhysicalDeviceMemoryProperties::default();
        unsafe { (f.get_memory_properties)(self.physical_device, &mut memory_properties) };
        let memory_type_index = choose_memory_type(
            requirements.memory_type_bits,
            &memory_properties,
            VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT,
        )
        .ok_or_else(|| "no compatible Vulkan memory type for color image".to_owned())?;
        let alloc_info = VkMemoryAllocateInfo {
            s_type: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
            p_next: ptr::null(),
            allocation_size: requirements.size,
            memory_type_index,
        };
        check(
            unsafe {
                (f.allocate_memory)(self.device, &alloc_info, ptr::null(), &mut resources.memory)
            },
            "vkAllocateMemory",
        )?;
        check(
            unsafe { (f.bind_image_memory)(self.device, resources.image, resources.memory, 0) },
            "vkBindImageMemory",
        )?;

        // 2. View + render pass + framebuffer.
        let view_info = VkImageViewCreateInfo {
            s_type: VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            image: resources.image,
            view_type: VK_IMAGE_VIEW_TYPE_2D,
            format: VK_FORMAT_R8G8B8A8_UNORM,
            components: VkComponentMapping {
                r: VK_COMPONENT_SWIZZLE_IDENTITY,
                g: VK_COMPONENT_SWIZZLE_IDENTITY,
                b: VK_COMPONENT_SWIZZLE_IDENTITY,
                a: VK_COMPONENT_SWIZZLE_IDENTITY,
            },
            subresource_range: VkImageSubresourceRange {
                aspect_mask: VK_IMAGE_ASPECT_COLOR_BIT,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
        };
        check(
            unsafe {
                (f.create_image_view)(
                    self.device,
                    &view_info,
                    ptr::null(),
                    &mut resources.image_view,
                )
            },
            "vkCreateImageView",
        )?;

        let attachment = VkAttachmentDescription {
            flags: 0,
            format: VK_FORMAT_R8G8B8A8_UNORM,
            samples: VK_SAMPLE_COUNT_1_BIT,
            load_op: VK_ATTACHMENT_LOAD_OP_CLEAR,
            store_op: VK_ATTACHMENT_STORE_OP_STORE,
            stencil_load_op: VK_ATTACHMENT_LOAD_OP_DONT_CARE,
            stencil_store_op: VK_ATTACHMENT_STORE_OP_DONT_CARE,
            initial_layout: VK_IMAGE_LAYOUT_UNDEFINED,
            final_layout: VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        };
        let color_ref = VkAttachmentReference {
            attachment: 0,
            layout: VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
        };
        let subpass = VkSubpassDescription {
            flags: 0,
            pipeline_bind_point: VK_PIPELINE_BIND_POINT_GRAPHICS,
            input_attachment_count: 0,
            p_input_attachments: ptr::null(),
            color_attachment_count: 1,
            p_color_attachments: &color_ref,
            p_resolve_attachments: ptr::null(),
            p_depth_stencil_attachment: ptr::null(),
            preserve_attachment_count: 0,
            p_preserve_attachments: ptr::null(),
        };
        let render_pass_info = VkRenderPassCreateInfo {
            s_type: VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            attachment_count: 1,
            p_attachments: &attachment,
            subpass_count: 1,
            p_subpasses: &subpass,
            dependency_count: 0,
            p_dependencies: ptr::null(),
        };
        check(
            unsafe {
                (f.create_render_pass)(
                    self.device,
                    &render_pass_info,
                    ptr::null(),
                    &mut resources.render_pass,
                )
            },
            "vkCreateRenderPass",
        )?;

        let attachments = [resources.image_view];
        let framebuffer_info = VkFramebufferCreateInfo {
            s_type: VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            render_pass: resources.render_pass,
            attachment_count: 1,
            p_attachments: attachments.as_ptr(),
            width,
            height,
            layers: 1,
        };
        check(
            unsafe {
                (f.create_framebuffer)(
                    self.device,
                    &framebuffer_info,
                    ptr::null(),
                    &mut resources.framebuffer,
                )
            },
            "vkCreateFramebuffer",
        )?;

        // Readback buffer: this turns the smoke path into a raster-output test rather than merely
        // checking that the driver accepted vkCmdDraw. HOST_COHERENT avoids explicit cache flush/invalidate.
        let readback_info = VkBufferCreateInfo {
            s_type: VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            size: readback_size,
            usage: VK_BUFFER_USAGE_TRANSFER_DST_BIT,
            sharing_mode: VK_SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            p_queue_family_indices: ptr::null(),
        };
        check(
            unsafe {
                (f.create_buffer)(
                    self.device,
                    &readback_info,
                    ptr::null(),
                    &mut resources.readback_buffer,
                )
            },
            "vkCreateBuffer(readback)",
        )?;
        let mut readback_requirements = VkMemoryRequirements::default();
        unsafe {
            (f.get_buffer_memory_requirements)(
                self.device,
                resources.readback_buffer,
                &mut readback_requirements,
            )
        };
        let readback_type = choose_memory_type_strict(
            readback_requirements.memory_type_bits,
            &memory_properties,
            VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
        )
        .ok_or_else(|| "no host-visible coherent Vulkan memory type for readback".to_owned())?;
        let readback_alloc = VkMemoryAllocateInfo {
            s_type: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
            p_next: ptr::null(),
            allocation_size: readback_requirements.size,
            memory_type_index: readback_type,
        };
        check(
            unsafe {
                (f.allocate_memory)(
                    self.device,
                    &readback_alloc,
                    ptr::null(),
                    &mut resources.readback_memory,
                )
            },
            "vkAllocateMemory(readback)",
        )?;
        check(
            unsafe {
                (f.bind_buffer_memory)(
                    self.device,
                    resources.readback_buffer,
                    resources.readback_memory,
                    0,
                )
            },
            "vkBindBufferMemory(readback)",
        )?;

        // 3. Host-ready SPIR-V -> VkShaderModule -> fixed-function graphics pipeline.
        resources.vertex_shader = create_shader_module(self.device, &f, &vertex.spirv.words)?;
        resources.fragment_shader = create_shader_module(self.device, &f, &fragment.spirv.words)?;
        let layout_info = VkPipelineLayoutCreateInfo {
            s_type: VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            set_layout_count: 0,
            p_set_layouts: ptr::null(),
            push_constant_range_count: 0,
            p_push_constant_ranges: ptr::null(),
        };
        check(
            unsafe {
                (f.create_pipeline_layout)(
                    self.device,
                    &layout_info,
                    ptr::null(),
                    &mut resources.pipeline_layout,
                )
            },
            "vkCreatePipelineLayout",
        )?;

        let main_name = CString::new("main").expect("static shader entry point");
        let stages = [
            VkPipelineShaderStageCreateInfo {
                s_type: VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                p_next: ptr::null(),
                flags: 0,
                stage: VK_SHADER_STAGE_VERTEX_BIT,
                module: resources.vertex_shader,
                p_name: main_name.as_ptr(),
                p_specialization_info: ptr::null(),
            },
            VkPipelineShaderStageCreateInfo {
                s_type: VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                p_next: ptr::null(),
                flags: 0,
                stage: VK_SHADER_STAGE_FRAGMENT_BIT,
                module: resources.fragment_shader,
                p_name: main_name.as_ptr(),
                p_specialization_info: ptr::null(),
            },
        ];
        let vertex_input = VkPipelineVertexInputStateCreateInfo {
            s_type: VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            vertex_binding_description_count: 0,
            p_vertex_binding_descriptions: ptr::null(),
            vertex_attribute_description_count: 0,
            p_vertex_attribute_descriptions: ptr::null(),
        };
        let input_assembly = VkPipelineInputAssemblyStateCreateInfo {
            s_type: VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            topology: VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
            primitive_restart_enable: 0,
        };
        let viewport = VkViewport {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = VkRect2D {
            offset: VkOffset2D { x: 0, y: 0 },
            extent: VkExtent2D { width, height },
        };
        let viewport_state = VkPipelineViewportStateCreateInfo {
            s_type: VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            viewport_count: 1,
            p_viewports: &viewport,
            scissor_count: 1,
            p_scissors: &scissor,
        };
        let raster = VkPipelineRasterizationStateCreateInfo {
            s_type: VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            depth_clamp_enable: 0,
            rasterizer_discard_enable: 0,
            polygon_mode: VK_POLYGON_MODE_FILL,
            cull_mode: VK_CULL_MODE_NONE,
            front_face: VK_FRONT_FACE_COUNTER_CLOCKWISE,
            depth_bias_enable: 0,
            depth_bias_constant_factor: 0.0,
            depth_bias_clamp: 0.0,
            depth_bias_slope_factor: 0.0,
            line_width: 1.0,
        };
        let multisample = VkPipelineMultisampleStateCreateInfo {
            s_type: VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            rasterization_samples: VK_SAMPLE_COUNT_1_BIT,
            sample_shading_enable: 0,
            min_sample_shading: 0.0,
            p_sample_mask: ptr::null(),
            alpha_to_coverage_enable: 0,
            alpha_to_one_enable: 0,
        };
        let blend_attachment = VkPipelineColorBlendAttachmentState {
            blend_enable: 0,
            src_color_blend_factor: 0,
            dst_color_blend_factor: 0,
            color_blend_op: 0,
            src_alpha_blend_factor: 0,
            dst_alpha_blend_factor: 0,
            alpha_blend_op: 0,
            color_write_mask: VK_COLOR_COMPONENT_R_BIT
                | VK_COLOR_COMPONENT_G_BIT
                | VK_COLOR_COMPONENT_B_BIT
                | VK_COLOR_COMPONENT_A_BIT,
        };
        let color_blend = VkPipelineColorBlendStateCreateInfo {
            s_type: VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            logic_op_enable: 0,
            logic_op: 0,
            attachment_count: 1,
            p_attachments: &blend_attachment,
            blend_constants: [0.0; 4],
        };
        let pipeline_info = VkGraphicsPipelineCreateInfo {
            s_type: VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            stage_count: stages.len() as u32,
            p_stages: stages.as_ptr(),
            p_vertex_input_state: &vertex_input,
            p_input_assembly_state: &input_assembly,
            p_tessellation_state: ptr::null(),
            p_viewport_state: &viewport_state,
            p_rasterization_state: &raster,
            p_multisample_state: &multisample,
            p_depth_stencil_state: ptr::null(),
            p_color_blend_state: &color_blend,
            p_dynamic_state: ptr::null(),
            layout: resources.pipeline_layout,
            render_pass: resources.render_pass,
            subpass: 0,
            base_pipeline_handle: ptr::null_mut(),
            base_pipeline_index: -1,
        };
        check(
            unsafe {
                (f.create_graphics_pipelines)(
                    self.device,
                    ptr::null_mut(),
                    1,
                    &pipeline_info,
                    ptr::null(),
                    &mut resources.pipeline,
                )
            },
            "vkCreateGraphicsPipelines",
        )?;

        // 4. Record and submit the actual draw.
        let family = self
            .queue_family_index
            .ok_or_else(|| "graphics queue family unavailable".to_owned())?;
        let pool_info = VkCommandPoolCreateInfo {
            s_type: VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            queue_family_index: family,
        };
        check(
            unsafe {
                (f.create_command_pool)(
                    self.device,
                    &pool_info,
                    ptr::null(),
                    &mut resources.command_pool,
                )
            },
            "vkCreateCommandPool",
        )?;
        let alloc_info = VkCommandBufferAllocateInfo {
            s_type: VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
            p_next: ptr::null(),
            command_pool: resources.command_pool,
            level: VK_COMMAND_BUFFER_LEVEL_PRIMARY,
            command_buffer_count: 1,
        };
        check(
            unsafe {
                (f.allocate_command_buffers)(
                    self.device,
                    &alloc_info,
                    &mut resources.command_buffer,
                )
            },
            "vkAllocateCommandBuffers",
        )?;
        let begin_info = VkCommandBufferBeginInfo {
            s_type: VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
            p_next: ptr::null(),
            flags: VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
            p_inheritance_info: ptr::null(),
        };
        check(
            unsafe { (f.begin_command_buffer)(resources.command_buffer, &begin_info) },
            "vkBeginCommandBuffer",
        )?;

        let clear = VkClearValue {
            color: VkClearColorValue {
                float32: [0.02, 0.02, 0.025, 1.0],
            },
        };
        let pass_begin = VkRenderPassBeginInfo {
            s_type: VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
            p_next: ptr::null(),
            render_pass: resources.render_pass,
            framebuffer: resources.framebuffer,
            render_area: scissor,
            clear_value_count: 1,
            p_clear_values: &clear,
        };
        unsafe {
            (f.cmd_begin_render_pass)(
                resources.command_buffer,
                &pass_begin,
                VK_SUBPASS_CONTENTS_INLINE,
            );
            (f.cmd_bind_pipeline)(
                resources.command_buffer,
                VK_PIPELINE_BIND_POINT_GRAPHICS,
                resources.pipeline,
            );
            (f.cmd_draw)(resources.command_buffer, vertices, 1, 0, 0);
            (f.cmd_end_render_pass)(resources.command_buffer);
            let copy = VkBufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: VkImageSubresourceLayers {
                    aspect_mask: VK_IMAGE_ASPECT_COLOR_BIT,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                image_offset: VkOffset3D { x: 0, y: 0, z: 0 },
                image_extent: VkExtent3D {
                    width,
                    height,
                    depth: 1,
                },
            };
            (f.cmd_copy_image_to_buffer)(
                resources.command_buffer,
                resources.image,
                VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                resources.readback_buffer,
                1,
                &copy,
            );
        }
        check(
            unsafe { (f.end_command_buffer)(resources.command_buffer) },
            "vkEndCommandBuffer",
        )?;
        let submit = VkSubmitInfo {
            s_type: VK_STRUCTURE_TYPE_SUBMIT_INFO,
            p_next: ptr::null(),
            wait_semaphore_count: 0,
            p_wait_semaphores: ptr::null(),
            p_wait_dst_stage_mask: ptr::null(),
            command_buffer_count: 1,
            p_command_buffers: &resources.command_buffer,
            signal_semaphore_count: 0,
            p_signal_semaphores: ptr::null(),
        };
        check(
            unsafe { (f.queue_submit)(self.queue, 1, &submit, ptr::null_mut()) },
            "vkQueueSubmit",
        )?;
        check(
            unsafe { (f.queue_wait_idle)(self.queue) },
            "vkQueueWaitIdle",
        )?;

        let mut mapped = ptr::null_mut();
        check(
            unsafe {
                (f.map_memory)(
                    self.device,
                    resources.readback_memory,
                    0,
                    readback_size,
                    0,
                    &mut mapped,
                )
            },
            "vkMapMemory(readback)",
        )?;
        if mapped.is_null() {
            return Err("vkMapMemory(readback) returned a null pointer".into());
        }
        let host_len =
            usize::try_from(readback_size).map_err(|_| "readback size does not fit host usize")?;
        let pixels = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), host_len) };
        let bright_pixels = pixels.chunks_exact(4).filter(|rgba| rgba[0] > 128).count() as u64;
        unsafe { (f.unmap_memory)(self.device, resources.readback_memory) };
        if bright_pixels == 0 {
            return Err(
                "Vulkan draw completed but readback contains no rasterized bright triangle pixels"
                    .into(),
            );
        }

        Ok(OffscreenDrawReport {
            width,
            height,
            vertices,
            shader_modules: 2,
            bright_pixels,
        })
    }

    /// Executes a transfer-only plan with real Vulkan buffers and writes modified payloads back.
    ///
    /// The first native resource path deliberately uses HOST_VISIBLE | HOST_COHERENT memory. This
    /// is not the final high-performance allocator, but it proves the guest payload -> VkBuffer ->
    /// queue -> guest payload round-trip without relying on the CPU transfer emulation path.
    pub fn execute_native_transfer_plan(
        &self,
        plan: &VulkanExecutionPlan,
        provider: &mut dyn GpuResourceData,
    ) -> Result<(), String> {
        if self.device.is_null() || self.queue.is_null() || self.physical_device.is_null() {
            return Err("no graphics-capable Vulkan device is available".into());
        }
        if !plan.commands.iter().all(|command| {
            matches!(
                command,
                VulkanPlanCommand::CopyBuffer { .. }
                    | VulkanPlanCommand::FillBuffer { .. }
                    | VulkanPlanCommand::PipelineBarrier { .. }
                    | VulkanPlanCommand::CacheFlush { .. }
                    | VulkanPlanCommand::InvalidateCaches { .. }
            )
        }) {
            return Err("native transfer path received a non-transfer command".into());
        }

        let usage = crate::resource_validation::analyze(plan).map_err(|error| error.to_string())?;
        let f = VulkanFns::load(&self._library)?;
        let mut memory_properties = VkPhysicalDeviceMemoryProperties::default();
        unsafe { (f.get_memory_properties)(self.physical_device, &mut memory_properties) };

        let required_ids = usage
            .read_buffers
            .iter()
            .chain(usage.written_buffers.iter())
            .copied()
            .collect::<BTreeSet<_>>();
        let descriptions = plan
            .buffers
            .iter()
            .map(|desc| (desc.id, desc))
            .collect::<BTreeMap<_, _>>();
        let mut buffers = NativeTransferBuffers::new(self.device, self.queue, &f);

        for id in required_ids {
            let desc = descriptions
                .get(&id)
                .ok_or_else(|| format!("buffer resource {} is not declared", id.0))?;
            let allocation =
                create_host_buffer(self.device, &f, &memory_properties, desc.size.max(4))?;
            if usage.read_buffers.contains(&id) {
                let host_len = usize::try_from(desc.size)
                    .map_err(|_| format!("buffer {} is too large for host usize", id.0))?;
                let mut bytes = vec![0_u8; host_len];
                provider
                    .read(id, 0, &mut bytes)
                    .map_err(|error| error.to_string())?;
                write_mapped(self.device, &f, allocation.memory, &bytes)?;
            }
            buffers.buffers.insert(id, allocation);
        }

        buffers.create_command_pool(
            self.queue_family_index
                .ok_or_else(|| "Vulkan device has no graphics queue family".to_owned())?,
        )?;
        buffers.allocate_command_buffer()?;
        let begin = VkCommandBufferBeginInfo {
            s_type: VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
            p_next: ptr::null(),
            flags: VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
            p_inheritance_info: ptr::null(),
        };
        check(
            unsafe { (f.begin_command_buffer)(buffers.command_buffer, &begin) },
            "vkBeginCommandBuffer(native-transfer)",
        )?;

        for command in &plan.commands {
            match command {
                VulkanPlanCommand::CopyBuffer {
                    source,
                    destination,
                    source_offset,
                    destination_offset,
                    bytes,
                } => {
                    if (source != destination || source_offset != destination_offset) && *bytes != 0
                    {
                        let src = buffers.buffers.get(source).ok_or_else(|| {
                            format!("source buffer {} was not allocated", source.0)
                        })?;
                        let dst = buffers.buffers.get(destination).ok_or_else(|| {
                            format!("destination buffer {} was not allocated", destination.0)
                        })?;
                        let region = VkBufferCopy {
                            src_offset: *source_offset,
                            dst_offset: *destination_offset,
                            size: *bytes,
                        };
                        unsafe {
                            (f.cmd_copy_buffer)(
                                buffers.command_buffer,
                                src.buffer,
                                dst.buffer,
                                1,
                                &region,
                            )
                        };
                        emit_transfer_barrier(&f, buffers.command_buffer);
                    }
                }
                VulkanPlanCommand::FillBuffer {
                    destination,
                    value,
                    bytes,
                } => {
                    let dst = buffers.buffers.get(destination).ok_or_else(|| {
                        format!("destination buffer {} was not allocated", destination.0)
                    })?;
                    // Vulkan requires offset/size to be 4-byte aligned for vkCmdFillBuffer. The
                    // canonical command is u32-pattern based, so reject malformed guest ranges
                    // rather than silently changing the requested byte count.
                    if bytes % 4 != 0 {
                        return Err(format!(
                            "vkCmdFillBuffer size {} is not 4-byte aligned",
                            bytes
                        ));
                    }
                    unsafe {
                        (f.cmd_fill_buffer)(buffers.command_buffer, dst.buffer, 0, *bytes, *value)
                    };
                    emit_transfer_barrier(&f, buffers.command_buffer);
                }
                VulkanPlanCommand::PipelineBarrier { .. }
                | VulkanPlanCommand::CacheFlush { .. }
                | VulkanPlanCommand::InvalidateCaches { .. } => {
                    emit_transfer_barrier(&f, buffers.command_buffer);
                }
                _ => unreachable!("transfer command filter above rejected non-transfer commands"),
            }
        }

        check(
            unsafe { (f.end_command_buffer)(buffers.command_buffer) },
            "vkEndCommandBuffer(native-transfer)",
        )?;
        let submit = VkSubmitInfo {
            s_type: VK_STRUCTURE_TYPE_SUBMIT_INFO,
            p_next: ptr::null(),
            wait_semaphore_count: 0,
            p_wait_semaphores: ptr::null(),
            p_wait_dst_stage_mask: ptr::null(),
            command_buffer_count: 1,
            p_command_buffers: &buffers.command_buffer,
            signal_semaphore_count: 0,
            p_signal_semaphores: ptr::null(),
        };
        check(
            unsafe { (f.queue_submit)(self.queue, 1, &submit, ptr::null_mut()) },
            "vkQueueSubmit(native-transfer)",
        )?;
        check(
            unsafe { (f.queue_wait_idle)(self.queue) },
            "vkQueueWaitIdle(native-transfer)",
        )?;

        for id in &usage.written_buffers {
            let desc = descriptions
                .get(id)
                .expect("resource analysis validated descriptor");
            let allocation = buffers
                .buffers
                .get(id)
                .expect("written buffer was allocated");
            let host_len = usize::try_from(desc.size)
                .map_err(|_| format!("buffer {} is too large for host usize", id.0))?;
            let bytes = read_mapped(self.device, &f, allocation.memory, host_len)?;
            provider
                .write(*id, 0, &bytes)
                .map_err(|error| error.to_string())?;
            provider.flush(*id).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

struct VulkanFns {
    create_buffer: VkCreateBuffer,
    destroy_buffer: VkDestroyBuffer,
    get_buffer_memory_requirements: VkGetBufferMemoryRequirements,
    bind_buffer_memory: VkBindBufferMemory,
    map_memory: VkMapMemory,
    unmap_memory: VkUnmapMemory,
    create_image: VkCreateImage,
    destroy_image: VkDestroyImage,
    get_image_memory_requirements: VkGetImageMemoryRequirements,
    get_memory_properties: VkGetPhysicalDeviceMemoryProperties,
    allocate_memory: VkAllocateMemory,
    free_memory: VkFreeMemory,
    bind_image_memory: VkBindImageMemory,
    create_image_view: VkCreateImageView,
    destroy_image_view: VkDestroyImageView,
    create_render_pass: VkCreateRenderPass,
    destroy_render_pass: VkDestroyRenderPass,
    create_framebuffer: VkCreateFramebuffer,
    destroy_framebuffer: VkDestroyFramebuffer,
    create_shader_module: VkCreateShaderModule,
    destroy_shader_module: VkDestroyShaderModule,
    create_pipeline_layout: VkCreatePipelineLayout,
    destroy_pipeline_layout: VkDestroyPipelineLayout,
    create_graphics_pipelines: VkCreateGraphicsPipelines,
    destroy_pipeline: VkDestroyPipeline,
    create_command_pool: VkCreateCommandPool,
    destroy_command_pool: VkDestroyCommandPool,
    allocate_command_buffers: VkAllocateCommandBuffers,
    begin_command_buffer: VkBeginCommandBuffer,
    end_command_buffer: VkEndCommandBuffer,
    cmd_begin_render_pass: VkCmdBeginRenderPass,
    cmd_end_render_pass: VkCmdEndRenderPass,
    cmd_bind_pipeline: VkCmdBindPipeline,
    cmd_draw: VkCmdDraw,
    cmd_copy_image_to_buffer: VkCmdCopyImageToBuffer,
    cmd_copy_buffer: VkCmdCopyBuffer,
    cmd_fill_buffer: VkCmdFillBuffer,
    cmd_pipeline_barrier: VkCmdPipelineBarrier,
    queue_submit: VkQueueSubmit,
    queue_wait_idle: VkQueueWaitIdle,
}
impl VulkanFns {
    fn load(library: &Library) -> Result<Self, String> {
        Ok(Self {
            create_buffer: load(library, b"vkCreateBuffer\0")?,
            destroy_buffer: load(library, b"vkDestroyBuffer\0")?,
            get_buffer_memory_requirements: load(library, b"vkGetBufferMemoryRequirements\0")?,
            bind_buffer_memory: load(library, b"vkBindBufferMemory\0")?,
            map_memory: load(library, b"vkMapMemory\0")?,
            unmap_memory: load(library, b"vkUnmapMemory\0")?,
            create_image: load(library, b"vkCreateImage\0")?,
            destroy_image: load(library, b"vkDestroyImage\0")?,
            get_image_memory_requirements: load(library, b"vkGetImageMemoryRequirements\0")?,
            get_memory_properties: load(library, b"vkGetPhysicalDeviceMemoryProperties\0")?,
            allocate_memory: load(library, b"vkAllocateMemory\0")?,
            free_memory: load(library, b"vkFreeMemory\0")?,
            bind_image_memory: load(library, b"vkBindImageMemory\0")?,
            create_image_view: load(library, b"vkCreateImageView\0")?,
            destroy_image_view: load(library, b"vkDestroyImageView\0")?,
            create_render_pass: load(library, b"vkCreateRenderPass\0")?,
            destroy_render_pass: load(library, b"vkDestroyRenderPass\0")?,
            create_framebuffer: load(library, b"vkCreateFramebuffer\0")?,
            destroy_framebuffer: load(library, b"vkDestroyFramebuffer\0")?,
            create_shader_module: load(library, b"vkCreateShaderModule\0")?,
            destroy_shader_module: load(library, b"vkDestroyShaderModule\0")?,
            create_pipeline_layout: load(library, b"vkCreatePipelineLayout\0")?,
            destroy_pipeline_layout: load(library, b"vkDestroyPipelineLayout\0")?,
            create_graphics_pipelines: load(library, b"vkCreateGraphicsPipelines\0")?,
            destroy_pipeline: load(library, b"vkDestroyPipeline\0")?,
            create_command_pool: load(library, b"vkCreateCommandPool\0")?,
            destroy_command_pool: load(library, b"vkDestroyCommandPool\0")?,
            allocate_command_buffers: load(library, b"vkAllocateCommandBuffers\0")?,
            begin_command_buffer: load(library, b"vkBeginCommandBuffer\0")?,
            end_command_buffer: load(library, b"vkEndCommandBuffer\0")?,
            cmd_begin_render_pass: load(library, b"vkCmdBeginRenderPass\0")?,
            cmd_end_render_pass: load(library, b"vkCmdEndRenderPass\0")?,
            cmd_bind_pipeline: load(library, b"vkCmdBindPipeline\0")?,
            cmd_draw: load(library, b"vkCmdDraw\0")?,
            cmd_copy_image_to_buffer: load(library, b"vkCmdCopyImageToBuffer\0")?,
            cmd_copy_buffer: load(library, b"vkCmdCopyBuffer\0")?,
            cmd_fill_buffer: load(library, b"vkCmdFillBuffer\0")?,
            cmd_pipeline_barrier: load(library, b"vkCmdPipelineBarrier\0")?,
            queue_submit: load(library, b"vkQueueSubmit\0")?,
            queue_wait_idle: load(library, b"vkQueueWaitIdle\0")?,
        })
    }
}

struct OwnedResources<'a> {
    device: *mut c_void,
    queue: *mut c_void,
    f: &'a VulkanFns,
    image: VkImage,
    memory: VkDeviceMemory,
    readback_buffer: VkBuffer,
    readback_memory: VkDeviceMemory,
    image_view: VkImageView,
    render_pass: VkRenderPass,
    framebuffer: VkFramebuffer,
    vertex_shader: VkShaderModule,
    fragment_shader: VkShaderModule,
    pipeline_layout: VkPipelineLayout,
    pipeline: VkPipeline,
    command_pool: VkCommandPool,
    command_buffer: VkCommandBuffer,
}
impl<'a> OwnedResources<'a> {
    fn new(device: *mut c_void, queue: *mut c_void, f: &'a VulkanFns) -> Self {
        Self {
            device,
            queue,
            f,
            image: ptr::null_mut(),
            memory: ptr::null_mut(),
            readback_buffer: ptr::null_mut(),
            readback_memory: ptr::null_mut(),
            image_view: ptr::null_mut(),
            render_pass: ptr::null_mut(),
            framebuffer: ptr::null_mut(),
            vertex_shader: ptr::null_mut(),
            fragment_shader: ptr::null_mut(),
            pipeline_layout: ptr::null_mut(),
            pipeline: ptr::null_mut(),
            command_pool: ptr::null_mut(),
            command_buffer: ptr::null_mut(),
        }
    }
}
impl Drop for OwnedResources<'_> {
    fn drop(&mut self) {
        unsafe {
            // Be conservative on every teardown path: a submit can succeed and a later wait can fail.
            // Waiting again before object destruction prevents releasing resources that may still be
            // referenced by in-flight work. Errors cannot be reported from Drop, so this is best-effort.
            if !self.queue.is_null() {
                let _ = (self.f.queue_wait_idle)(self.queue);
            }
            if !self.command_pool.is_null() {
                (self.f.destroy_command_pool)(self.device, self.command_pool, ptr::null());
            }
            if !self.pipeline.is_null() {
                (self.f.destroy_pipeline)(self.device, self.pipeline, ptr::null());
            }
            if !self.pipeline_layout.is_null() {
                (self.f.destroy_pipeline_layout)(self.device, self.pipeline_layout, ptr::null());
            }
            if !self.vertex_shader.is_null() {
                (self.f.destroy_shader_module)(self.device, self.vertex_shader, ptr::null());
            }
            if !self.fragment_shader.is_null() {
                (self.f.destroy_shader_module)(self.device, self.fragment_shader, ptr::null());
            }
            if !self.framebuffer.is_null() {
                (self.f.destroy_framebuffer)(self.device, self.framebuffer, ptr::null());
            }
            if !self.render_pass.is_null() {
                (self.f.destroy_render_pass)(self.device, self.render_pass, ptr::null());
            }
            if !self.image_view.is_null() {
                (self.f.destroy_image_view)(self.device, self.image_view, ptr::null());
            }
            if !self.readback_buffer.is_null() {
                (self.f.destroy_buffer)(self.device, self.readback_buffer, ptr::null());
            }
            if !self.readback_memory.is_null() {
                (self.f.free_memory)(self.device, self.readback_memory, ptr::null());
            }
            if !self.image.is_null() {
                (self.f.destroy_image)(self.device, self.image, ptr::null());
            }
            if !self.memory.is_null() {
                (self.f.free_memory)(self.device, self.memory, ptr::null());
            }
        }
    }
}

struct NativeBufferAllocation {
    buffer: VkBuffer,
    memory: VkDeviceMemory,
}

struct NativeTransferBuffers<'a> {
    device: *mut c_void,
    queue: *mut c_void,
    f: &'a VulkanFns,
    buffers: BTreeMap<ResourceId, NativeBufferAllocation>,
    command_pool: VkCommandPool,
    command_buffer: VkCommandBuffer,
}
impl<'a> NativeTransferBuffers<'a> {
    fn new(device: *mut c_void, queue: *mut c_void, f: &'a VulkanFns) -> Self {
        Self {
            device,
            queue,
            f,
            buffers: BTreeMap::new(),
            command_pool: ptr::null_mut(),
            command_buffer: ptr::null_mut(),
        }
    }
    fn create_command_pool(&mut self, queue_family_index: u32) -> Result<(), String> {
        let info = VkCommandPoolCreateInfo {
            s_type: VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
            p_next: ptr::null(),
            flags: 0,
            queue_family_index,
        };
        check(
            unsafe {
                (self.f.create_command_pool)(
                    self.device,
                    &info,
                    ptr::null(),
                    &mut self.command_pool,
                )
            },
            "vkCreateCommandPool(native-transfer)",
        )
    }
    fn allocate_command_buffer(&mut self) -> Result<(), String> {
        let info = VkCommandBufferAllocateInfo {
            s_type: VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
            p_next: ptr::null(),
            command_pool: self.command_pool,
            level: VK_COMMAND_BUFFER_LEVEL_PRIMARY,
            command_buffer_count: 1,
        };
        check(
            unsafe {
                (self.f.allocate_command_buffers)(self.device, &info, &mut self.command_buffer)
            },
            "vkAllocateCommandBuffers(native-transfer)",
        )
    }
}
impl Drop for NativeTransferBuffers<'_> {
    fn drop(&mut self) {
        unsafe {
            if !self.queue.is_null() {
                let _ = (self.f.queue_wait_idle)(self.queue);
            }
            if !self.command_pool.is_null() {
                (self.f.destroy_command_pool)(self.device, self.command_pool, ptr::null());
            }
            for allocation in self.buffers.values() {
                if !allocation.buffer.is_null() {
                    (self.f.destroy_buffer)(self.device, allocation.buffer, ptr::null());
                }
                if !allocation.memory.is_null() {
                    (self.f.free_memory)(self.device, allocation.memory, ptr::null());
                }
            }
        }
    }
}

fn emit_transfer_barrier(f: &VulkanFns, command_buffer: VkCommandBuffer) {
    let barrier = VkMemoryBarrier {
        s_type: VK_STRUCTURE_TYPE_MEMORY_BARRIER,
        p_next: ptr::null(),
        src_access_mask: VK_ACCESS_TRANSFER_WRITE_BIT,
        dst_access_mask: VK_ACCESS_TRANSFER_READ_BIT | VK_ACCESS_TRANSFER_WRITE_BIT,
    };
    unsafe {
        (f.cmd_pipeline_barrier)(
            command_buffer,
            VK_PIPELINE_STAGE_TRANSFER_BIT,
            VK_PIPELINE_STAGE_TRANSFER_BIT,
            0,
            1,
            &barrier,
            0,
            ptr::null(),
            0,
            ptr::null(),
        )
    };
}

fn create_host_buffer(
    device: *mut c_void,
    f: &VulkanFns,
    memory_properties: &VkPhysicalDeviceMemoryProperties,
    size: u64,
) -> Result<NativeBufferAllocation, String> {
    let info = VkBufferCreateInfo {
        s_type: VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        size,
        usage: VK_BUFFER_USAGE_TRANSFER_SRC_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT,
        sharing_mode: VK_SHARING_MODE_EXCLUSIVE,
        queue_family_index_count: 0,
        p_queue_family_indices: ptr::null(),
    };
    let mut buffer = ptr::null_mut();
    check(
        unsafe { (f.create_buffer)(device, &info, ptr::null(), &mut buffer) },
        "vkCreateBuffer(native-transfer)",
    )?;
    let mut requirements = VkMemoryRequirements::default();
    unsafe { (f.get_buffer_memory_requirements)(device, buffer, &mut requirements) };
    let flags = VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;
    let memory_type_index =
        match choose_memory_type_strict(requirements.memory_type_bits, memory_properties, flags) {
            Some(index) => index,
            None => {
                unsafe { (f.destroy_buffer)(device, buffer, ptr::null()) };
                return Err(
                    "no HOST_VISIBLE|HOST_COHERENT Vulkan memory type for transfer buffer"
                        .to_owned(),
                );
            }
        };
    let allocation_info = VkMemoryAllocateInfo {
        s_type: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
        p_next: ptr::null(),
        allocation_size: requirements.size,
        memory_type_index,
    };
    let mut memory = ptr::null_mut();
    if let Err(error) = check(
        unsafe { (f.allocate_memory)(device, &allocation_info, ptr::null(), &mut memory) },
        "vkAllocateMemory(native-transfer)",
    ) {
        unsafe { (f.destroy_buffer)(device, buffer, ptr::null()) };
        return Err(error);
    }
    if let Err(error) = check(
        unsafe { (f.bind_buffer_memory)(device, buffer, memory, 0) },
        "vkBindBufferMemory(native-transfer)",
    ) {
        unsafe {
            (f.free_memory)(device, memory, ptr::null());
            (f.destroy_buffer)(device, buffer, ptr::null());
        }
        return Err(error);
    }
    Ok(NativeBufferAllocation { buffer, memory })
}

fn write_mapped(
    device: *mut c_void,
    f: &VulkanFns,
    memory: VkDeviceMemory,
    bytes: &[u8],
) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut mapped = ptr::null_mut();
    check(
        unsafe { (f.map_memory)(device, memory, 0, bytes.len() as u64, 0, &mut mapped) },
        "vkMapMemory(upload)",
    )?;
    if mapped.is_null() {
        return Err("vkMapMemory(upload) returned null".into());
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
        (f.unmap_memory)(device, memory);
    }
    Ok(())
}

fn read_mapped(
    device: *mut c_void,
    f: &VulkanFns,
    memory: VkDeviceMemory,
    len: usize,
) -> Result<Vec<u8>, String> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut mapped = ptr::null_mut();
    check(
        unsafe { (f.map_memory)(device, memory, 0, len as u64, 0, &mut mapped) },
        "vkMapMemory(download)",
    )?;
    if mapped.is_null() {
        return Err("vkMapMemory(download) returned null".into());
    }
    let mut bytes = vec![0_u8; len];
    unsafe {
        ptr::copy_nonoverlapping(mapped.cast::<u8>(), bytes.as_mut_ptr(), len);
        (f.unmap_memory)(device, memory);
    }
    Ok(bytes)
}

fn create_shader_module(
    device: *mut c_void,
    f: &VulkanFns,
    words: &[u32],
) -> Result<VkShaderModule, String> {
    if words.len() < 5 || words[0] != 0x0723_0203 {
        return Err("invalid SPIR-V module supplied to Vulkan".into());
    }
    let info = VkShaderModuleCreateInfo {
        s_type: VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
        p_next: ptr::null(),
        flags: 0,
        code_size: mem::size_of_val(words),
        p_code: words.as_ptr(),
    };
    let mut module = ptr::null_mut();
    check(
        unsafe { (f.create_shader_module)(device, &info, ptr::null(), &mut module) },
        "vkCreateShaderModule",
    )?;
    if module.is_null() {
        return Err("vkCreateShaderModule returned a null handle".into());
    }
    Ok(module)
}

fn choose_memory_type(
    bits: u32,
    properties: &VkPhysicalDeviceMemoryProperties,
    preferred_flags: u32,
) -> Option<u32> {
    let count = properties
        .memory_type_count
        .min(properties.memory_types.len() as u32);
    (0..count)
        .find(|index| {
            bits & (1u32 << index) != 0
                && properties.memory_types[*index as usize].property_flags & preferred_flags
                    == preferred_flags
        })
        .or_else(|| (0..count).find(|index| bits & (1u32 << index) != 0))
}

fn choose_memory_type_strict(
    bits: u32,
    properties: &VkPhysicalDeviceMemoryProperties,
    required_flags: u32,
) -> Option<u32> {
    let count = properties
        .memory_type_count
        .min(properties.memory_types.len() as u32);
    (0..count).find(|index| {
        bits & (1u32 << index) != 0
            && properties.memory_types[*index as usize].property_flags & required_flags
                == required_flags
    })
}

fn check(result: VkResult, function: &str) -> Result<(), String> {
    if result == VK_SUCCESS {
        Ok(())
    } else {
        Err(format!("{function} failed with VkResult {result}"))
    }
}

fn load<T: Copy>(library: &Library, name: &'static [u8]) -> Result<T, String> {
    let symbol: Symbol<'_, T> = unsafe { library.get(name) }.map_err(|error| {
        format!(
            "missing Vulkan symbol {}: {error}",
            String::from_utf8_lossy(name)
        )
    })?;
    Ok(*symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_type_prefers_requested_flags_then_falls_back() {
        let mut properties = VkPhysicalDeviceMemoryProperties {
            memory_type_count: 2,
            ..Default::default()
        };
        properties.memory_types[0] = VkMemoryType {
            property_flags: 0,
            heap_index: 0,
        };
        properties.memory_types[1] = VkMemoryType {
            property_flags: VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT,
            heap_index: 0,
        };
        assert_eq!(
            choose_memory_type(0b11, &properties, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT),
            Some(1)
        );
        assert_eq!(
            choose_memory_type(0b01, &properties, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT),
            Some(0)
        );
        assert_eq!(
            choose_memory_type(0, &properties, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT),
            None
        );
        assert_eq!(
            choose_memory_type_strict(0b01, &properties, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT),
            None
        );
        assert_eq!(
            choose_memory_type_strict(0b10, &properties, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT),
            Some(1)
        );
    }
}
