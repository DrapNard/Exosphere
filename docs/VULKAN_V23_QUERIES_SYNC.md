# Vulkan v2.3 — Queries and submission synchronization

**Project:** Exosphere  
**Maintainer:** DrapNard

This pass is based on the recovered v2.2 archive supplied by DrapNard.

## Native queries

The ash Vulkan 1.3 backend now owns query pools keyed by canonical `ResourceId` and query kind.
Supported paths:

- occlusion queries
- timestamps through `vkCmdWriteTimestamp2`
- pipeline-statistics queries
- query reset
- 64-bit result copy into guest-backed Vulkan buffers
- normal Exosphere buffer writeback then exposes copied results to guest memory

Query pools grow conservatively when a larger index is observed. Reusing one canonical pool with an incompatible query kind is rejected rather than silently reinterpreted.

Transform-feedback queries remain tied to the optional transform-feedback extension and therefore remain explicitly unsupported until that extension path is implemented.

## Submission synchronization

The backend no longer calls `vkQueueWaitIdle` after every normal execution plan. Each submission receives a Vulkan fence and waits on that fence only.

This remains synchronous at the public execution boundary, but it removes the global queue-idle primitive and creates the correct stepping stone for reusable frame contexts and multiple in-flight frames.

## Next

1. reusable 2–3 frame contexts and deferred destruction
2. timeline-semaphore mapping for guest semaphores/syncpoints
3. Linux surface + Wayland/XCB swapchain
4. conditional rendering / transform feedback capability paths
5. production allocators and persistent caches
