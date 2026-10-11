# GPU continuation — resource identity and hazards

**Project:** Exosphere  
**Maintainer:** DrapNard

This pass closes the conceptual gap between raw Maxwell GPU virtual addresses and stable canonical resources.

## Added

### GPU virtual address space

`GpuAddressSpace` maps guest GPU VA ranges to stable `ResourceId` objects plus a resource-relative offset and permissions.

```text
GPU VA 0x....
   ↓
GpuAddressSpace
   ↓
ResourceId + offset + permissions
```

Guest addresses are never canonical resource identities.

### Stable resource table

`ResourceTable` interns buffers and textures with BLAKE3-derived IDs from semantic metadata and a source key. This keeps identities stable across remaps.

### Texture layout metadata

The canonical resource model now covers common uncompressed and block-compressed formats used by the future texture compiler:

- RGBA8/BGRA8
- RGBA16F
- R32F
- D16/D24S8/D32F
- BC1
- BC3
- BC5
- ASTC 4x4

`linear_footprint()` computes safe block/byte footprints. Exact Tegra block-linear byte swizzling remains deliberately separate until every layout bit is validated.

### Lossless descriptor records

Texture/sampler descriptor records keep all raw Maxwell words while exposing promoted fields only when their semantics are known.

### Resource hazards

The canonical GPU layer can now identify:

- RAW — read after write
- WAR — write after read
- WAW — write after write

This produces backend-neutral synchronization requirements before Vulkan/Metal barrier selection.

### DMA correctness fix

The old prototype used a raw GPU address as `ResourceId` when lowering DMA copies. That shortcut has been removed.

DMA lowering now requires `GpuAddressSpace` resolution. Unmapped ranges remain unresolved/opaque instead of creating fake resources.

## CLI smoke tests

```bash
cargo run -p exosphere-cli -- gpu-address-smoke
cargo run -p exosphere-cli -- texture-footprint --width 1920 --height 1080 --format rgba8
cargo run -p exosphere-cli -- texture-footprint --width 1920 --height 1080 --format bc1
```

## Still intentionally not guessed

- exact Tegra block-linear swizzle/address equation for every surface mode
- full TIC/TSC bitfield promotion
- compression tags / Z-cull storage
- every Maxwell texture format
- complete MME and SM53 semantic execution

Raw state remains preserved so these can be added without format changes.
