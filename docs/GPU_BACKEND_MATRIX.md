# GPU Backend Matrix v1

**Maintainer:** DrapNard

| Capability | Canonical IR | Vulkan Plan | Raw Linux Vulkan v0.1 |
|---|---:|---:|---:|
| Graphics pipelines | Yes | Yes | Native smoke subset |
| Fixed-function state | Yes | Yes, in pipeline descriptors | Smoke defaults only |
| Vertex/index buffers | Yes | Yes | Payload bridge + validation; raw binding not yet native |
| Direct draw | Yes | Yes | Native |
| Indexed draw | Yes | Yes | Not yet native |
| Indirect draw | Yes | Yes | Not yet native |
| Compute | Yes | Yes | Not yet native |
| Textures/samplers | Yes | Yes | Not yet native |
| Color/depth targets | Yes | Yes | Internal offscreen color only |
| Buffer/image transfer | Yes | Yes | Buffer copy/fill CPU-emulated; image transfer still backend work |
| Blit/resolve | Yes | Yes | Not yet native |
| Queries/timestamps | Yes | Yes | Not yet native |
| Conditional rendering | Yes | Yes | Native when VK_EXT_conditional_rendering is exposed |
| Transform feedback | Yes | Yes | Native when VK_EXT_transform_feedback is exposed |
| Barriers/cache control | Yes | Yes | Partial/emulated |
| Guest semaphores/syncpoints | Yes | Yes | Runtime emulation path required |
| BC/ASTC resource formats | Yes | Yes via descriptors | Device capability work required |
| Block-linear layout | Yes | N/A after conversion | Conversion path required |
| Runtime resource payloads | Yes (`GpuResourceData`) | Yes | Guest-VA mapped + bounds checked |

The raw backend refuses unsupported plan operations with `VulkanError::UnsupportedFeature`. It never silently ignores them.

## Maxwell semantic coverage

Architectural completeness and reverse-engineering coverage are separate metrics. `SemanticCoverage` tracks Maxwell registers, SM53 instructions, MME instructions, TIC and TSC semantics independently.

## Runtime resource bridge

See `GPU_RUNTIME_RESOURCES.md`. Vulkan plans now validate ResourceId references/ranges before raw driver execution. Transfer-only buffer copy/fill plans have a correctness-first host emulation path.

## Runtime resource execution update

| Feature | Vulkan status |
|---|---|
| Native buffer transfer round trip | **Native** |
| `vkCmdCopyBuffer` | **Native** |
| `vkCmdFillBuffer` | **Native** with aligned range; correctness fallback otherwise |
| Transfer barriers | **Native conservative mapping** |
| Guest resource writeback | **Native readback + provider flush** |
| Device-local staging allocator | Planned optimization |
| Vertex/index upload and binding | Next |
| Texture/image upload | Next |
