# Vulkan One-Shot v2

**Author / maintainer:** DrapNard  
**Project:** Exosphere

## Purpose

This pass turns Vulkan from a collection of smoke paths into a general Vulkan 1.3 backend boundary.
The old raw loader is retained as a very small independent smoke/oracle path; normal runtime work
moves to the `ash` backend.

## Canonical IR fixes discovered during implementation

Two pieces of information were missing from the canonical GPU IR and were required before a Vulkan
backend could be correct rather than heuristic:

1. **Vertex input layout**
   - binding
   - stride
   - vertex/instance rate
   - attribute location
   - attribute format
   - attribute offset
2. **Pipeline resource layout**
   - descriptor set
   - binding
   - descriptor type
   - descriptor count
   - shader stage visibility
   - push constant ranges

Both are now part of `GraphicsPipelineDesc`; descriptor/push-constant layout is also available to
compute pipelines.

## Vulkan 1.3 backend

`AshVulkanDevice` owns:

```text
VkInstance
  ↓
VkPhysicalDevice
  ↓
VkDevice
  ↓
graphics+compute queue
  ↓
command pool
  ↓
resource / pipeline caches
```

It requests Vulkan 1.3 dynamic rendering and synchronization2.

## Native core paths

Implemented in the v2 execution core:

```text
Buffers
├── vertex
├── index
├── uniform/storage allocation class
├── indirect
├── transfer source/destination
└── host-visible guest upload/writeback

Images
├── color
├── depth/stencil
├── sampled/storage capable allocation
├── BCn formats
└── ASTC formats where supported by the host

Commands
├── bind vertex buffer
├── bind index buffer
├── direct draw
├── indexed draw
├── indirect draw
├── direct dispatch
├── indirect dispatch
├── copy buffer
├── fill buffer
├── dynamic viewport/scissor
└── synchronization2 memory barriers
```

## Explicit remaining backend work

The canonical plan already transports these features, but they remain separate executors rather
than silently ignored:

- image upload / block-linear conversion path
- sampled/storage descriptor set population
- render-target dynamic rendering execution
- graphics/compute pipeline cache creation from every fixed-function state combination
- native image copy/blit/resolve/clear
- query pools/timestamps
- conditional rendering extension
- transform feedback extension
- platform swapchain/surface integration

Every unsupported path returns `VulkanError::UnsupportedFeature` rather than being dropped.

## Backend policy

```text
canonical GPU IR
      ↓
VulkanExecutionPlan
      ↓
plan validation
      ↓
AshVulkanDevice
      ├── Native
      ├── Explicit emulation/fallback
      └── Explicit UnsupportedFeature
```

A backend capability is never marked Native merely because the IR can represent it.
