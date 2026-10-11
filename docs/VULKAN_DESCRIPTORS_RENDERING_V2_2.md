# Vulkan descriptors + dynamic rendering v2.2

This milestone removes the final architectural blocker between Exosphere canonical GPU IR and a general Vulkan graphics/compute draw.

## Canonical IR additions

Explicit descriptor commands now coexist with legacy Maxwell slot commands:

- `BindBufferDescriptor { set, binding, array_element, ... }`
- `BindImageDescriptor { set, binding, array_element, ... }`
- `BindSamplerDescriptor { set, binding, array_element, ... }`
- `PushConstants { stages, offset, data }`

The legacy `BindTexture { slot }` and `BindSampler { slot }` remain valid frontend inputs so Maxwell state capture does not depend on Vulkan concepts. Translation/reflection can promote them to explicit bindings when shader layout information is known.

## Vulkan execution

The ash backend now owns:

- descriptor-set-layout creation;
- growable descriptor pools;
- per-state descriptor-set allocation;
- buffer/image/sampler/combined-image-sampler writes;
- graphics and compute pipeline-layout creation;
- SPIR-V shader-module caching;
- graphics pipeline creation from canonical vertex/fixed-function state;
- compute pipeline creation;
- attachment-format-specialized dynamic-rendering pipelines;
- multiple color attachments plus depth/stencil;
- `vkCmdBeginRendering` / `vkCmdEndRendering`;
- descriptor binding before draw/dispatch;
- canonical push constants.

## Correctness policy

Exosphere still rejects unsupported or under-specified behavior instead of silently guessing. Examples include unsupported host vertex formats, missing descriptors, unsupported sample counts, and unsupported optional Vulkan features.
