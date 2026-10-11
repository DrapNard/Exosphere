# GPU Runtime Resource Data v1

**Maintainer:** DrapNard  
**Status:** Runtime boundary frozen for implementation

Canonical GPU descriptors intentionally describe *what* a resource is without embedding potentially huge guest allocations. Runtime bytes are supplied through `GpuResourceData`.

```text
GpuModule / VulkanExecutionPlan
        │
        ├── BufferDesc / TextureDesc / ResourceId
        │
        ▼
GpuResourceData
        │
        ├── InMemoryGpuResources      (tests / standalone data)
        └── MappedGpuResourceData     (guest GPU virtual memory)
                    │
                    ▼
             GpuAddressSpace
                    │
         ResourceId + resource offset
                    │
                    ▼
              guest GPU VA
```

## Rules

- A GPU virtual address is never a resource identity.
- Aliases/remaps may expose one `ResourceId` through multiple guest VAs.
- Backends operate on `ResourceId + offset`, not raw guest pointers.
- Payload access is bounds checked.
- Writes honor GPU mapping permissions.
- Backends cannot retain raw pointers across provider calls.
- Read-only shader discovery only requires `GpuMemory`; guest-visible GPU writes require `GpuMemoryAccess`.

## Vulkan validation

Before touching raw Vulkan handles, `resource_validation` verifies:

- every referenced resource exists in the execution plan;
- indirect/vertex/index/conditional ranges stay inside declared buffers;
- transfer sizes fit source and destination descriptors;
- sampled/read resources have payload bytes available;
- write-only targets do not require meaningless initial payload data.

This moves malformed guest/capture failures out of the driver and into deterministic Exosphere errors.

## Transfer fallback

`CopyBuffer` and `FillBuffer` now have a correctness-first host fallback. Transfer-only plans execute in-order through `GpuResourceData` while native Vulkan transfer command recording is still being expanded.

The fallback is deliberately not used to reorder mixed graphics/transfer plans.
