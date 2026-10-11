# Vulkan descriptors, pipelines and dynamic rendering — v2.2

**Maintainer:** DrapNard

This milestone closes the main execution gap between Exosphere's canonical GPU IR and the `ash` Vulkan backend.

## Canonical IR additions

Bindings can now be explicit instead of relying only on legacy Maxwell texture/sampler slots:

```text
BindBufferDescriptor(set,binding,array,resource,offset,range)
BindImageDescriptor(set,binding,array,texture)
BindSamplerDescriptor(set,binding,array,sampler)
PushConstants(stages,offset,data)
```

Legacy slot commands remain available for Maxwell translation. They are progressively resolved to explicit descriptor bindings once shader/resource layout information is known.

## Vulkan backend

The `ash` backend now contains:

- descriptor-set layout creation from `PipelineLayoutDesc`;
- descriptor-pool allocation and exhaustion recovery;
- descriptor writes for uniform/storage buffers, sampled/storage images, samplers and combined image samplers;
- push-constant validation and command emission;
- SPIR-V shader-module caching;
- graphics-pipeline creation using canonical vertex/fixed-function state;
- compute-pipeline creation;
- attachment-format-aware graphics pipeline keys;
- Vulkan 1.3 dynamic rendering;
- descriptor binding before graphics/compute execution.

## Smoke test

```bash
cargo run -p exosphere-cli -- vulkan-ash-triangle-smoke --width 256 --height 256
```

The smoke uses a canonical `TextureDesc` render target and executes through the `ash` backend rather than the older raw Vulkan smoke path.

## Remaining work

This milestone does **not** claim Vulkan productionization is finished. Remaining work is tracked in `VULKAN_FEATURE_ROADMAP.md`, especially query pools, swapchain/presentation, precise timeline synchronization, persistent allocators/caches and real-game workload validation.
