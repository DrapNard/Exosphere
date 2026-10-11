# Native Vulkan transfers

**Project:** Exosphere  
**Maintainer:** DrapNard

Exosphere's Vulkan backend now has a native buffer-transfer round trip.

```text
GpuResourceData
      ↓ upload
HOST_VISIBLE | HOST_COHERENT VkBuffer
      ↓
vkCmdCopyBuffer / vkCmdFillBuffer
      ↓
transfer memory barriers
      ↓
vkQueueSubmit / vkQueueWaitIdle
      ↓ readback
GpuResourceData
```

## Current behavior

- `CopyBuffer`: native Vulkan.
- `FillBuffer`: native Vulkan when the byte count satisfies Vulkan's 4-byte fill requirement.
- transfer barriers/cache ordering: mapped to a conservative transfer-stage `VkMemoryBarrier`.
- writeback: modified Vulkan buffer contents are copied back to the guest resource provider.
- fallback: the existing synchronous CPU transfer implementation remains available if the host
  cannot expose suitable coherent memory or the native path rejects the transfer shape.

The first native implementation deliberately uses host-visible coherent allocations. It prioritizes
correctness and proves the full guest→Vulkan→guest path. A later allocator can replace this with
staging buffers plus device-local allocations without changing canonical GPU IR or `GpuResourceData`.

## Why this matters

Before this change, Exosphere could submit real graphics work but transfer-only plans were still
executed by the CPU. DMA/copy traffic is pervasive in Switch games, so a native transfer path is a
required foundation for texture uploads, vertex/index resources, indirect command buffers and
resource streaming.
