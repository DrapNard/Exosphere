# Vulkan feature roadmap — core closure v3.0

Legend: `DONE` = implemented core path, `OPTIONAL` = negotiated host extension, `PERF` = production optimization,
`PLATFORM` = intentionally owned by another phase, `FRONTEND` = needs Maxwell metadata rather than more Vulkan API.

| Area | State | Remaining boundary |
|---|---|---|
| Vulkan 1.3 device | DONE | multi-queue is optional optimization |
| Dynamic rendering | DONE | platform presentation separate |
| Buffers / transfers | DONE | device-local suballocation/rings = PERF |
| Images / textures | DONE | block-linear multi-mip offsets = FRONTEND |
| Mips / arrays / layers | DONE | block-linear multi-mip offsets = FRONTEND |
| MSAA attachments / resolve | DONE | host format/sample support still validated |
| Descriptors / push constants | DONE | descriptor indexing/cache = PERF |
| Graphics pipelines | DONE | async compilation/prewarm = PERF |
| Compute pipelines | DONE | workload validation continues |
| Queries / timestamps | DONE | XFB query depends on optional extension |
| Conditional rendering | OPTIONAL | native when VK_EXT_conditional_rendering exists |
| Transform feedback | OPTIONAL | native when VK_EXT_transform_feedback exists |
| Synchronization2 | DONE | multi-queue bridge = future extension |
| Guest semaphore/syncpoint | DONE (single queue) | timeline-backed cross-queue bridge later |
| Async execution | DONE | ExecutionToken + deferred buffer writeback |
| Pipeline cache | DONE | filesystem policy remains runtime-owned |
| Range suballocator | DONE foundation | Vulkan heap integration = PERF |
| Swapchain | PLATFORM | needs external VkSurfaceKHR / Phase 10 |
| Wayland / X11 | PLATFORM | Phase 10 Linux |

## Recommended next phase

Return to Maxwell: increase SM53 semantic coverage, execute more MME macros, promote TIC/TSC fields, and use
those decoded fields to remove the remaining frontend assumptions (especially block-linear mip offsets).
