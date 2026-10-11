# Vulkan core closure (v3.0)

Phase 5 now treats the ash Vulkan 1.3 backend as the general execution backend. Platform window
systems remain outside this crate: Linux supplies a surface later in Phase 10.

## Native core

- Vulkan 1.3 device/queue/command recording, dynamic rendering and synchronization2.
- Graphics/compute pipelines, explicit descriptor layouts/sets, push constants and pipeline cache import/export.
- Vertex/index/direct/indirect draw and direct/indirect dispatch.
- Buffers, textures, samplers, BCn/ASTC formats, transfer/copy/fill/blit/resolve/clear.
- Texture arrays, mip uploads for tightly-packed linear payloads, Tegra X1 block-linear deswizzle for
  single-mip payloads, and full mip/layer image layout transitions.
- Multisampled image descriptors (`TextureDesc::samples`) and attachment/pipeline sample validation.
- Occlusion, timestamp, pipeline-statistics and transform-feedback queries (when the extension exists).
- Three frame contexts, internal timeline semaphore, asynchronous `ExecutionToken`, deferred guest buffer writeback.
- Monotonic guest semaphore/syncpoint bridge for the current single-queue model.
- Optional `VK_EXT_conditional_rendering` and `VK_EXT_transform_feedback`, negotiated per physical device.
- Driver pipeline-cache import/export and a timeline-aware range allocator foundation.

## Deliberate boundaries

These are not blockers for calling the Vulkan *core* complete:

- Wayland/X11 surface creation is Phase 10 Linux.
- Swapchain creation requires a platform-provided `VkSurfaceKHR` before final queue/present-family selection.
- Device-local heap suballocation, persistent upload/readback rings, descriptor indexing and async PSO
  compilation are production/performance work. The correctness path remains available without them.
- Multi-queue guest synchronization will need the guest sync bridge to become timeline-backed across queues;
  the current monotonic bridge is correct for the backend's single ordered queue.
- Block-linear multi-mip payloads still require explicit per-mip guest offsets from the Maxwell resource
  frontend. The backend refuses to guess these offsets.
- Unsupported host texture formats require a conversion policy; this belongs to the resource translation layer.

## Next compatibility bottleneck

The next major compatibility limiter is no longer Vulkan API coverage. Work should return to Maxwell shader
SM53 decoding, MME execution, TIC/TSC promotion and frontend state translation.
