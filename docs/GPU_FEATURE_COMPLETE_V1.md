# GPU Feature-Complete v1 Surface

**Project:** Exosphere  
**Maintainer:** DrapNard

This milestone means the canonical GPU architecture can represent the complete families of work a Switch title can require without another redesign. It does **not** claim every undocumented Maxwell register or SM53/MME opcode is semantically decoded.

## Canonical surface

- Graphics, compute, transfer and 2D queues
- Graphics/compute pipelines
- Full fixed-function state container: raster/cull/front-face/polygon, depth/stencil, per-target blending, logic ops, multisampling
- Vertex/index buffers
- Textures, samplers and render/depth targets
- Direct and indirect draws
- Direct and indirect dispatch
- Buffer/image copy, blit, resolve and fill
- Color/depth/stencil clear
- Queries, timestamps and query copies
- Conditional rendering
- Transform feedback
- Cache control, barriers, semaphores and syncpoints
- GPU virtual address resolution, aliases/subranges and hazards
- Linear/block-linear textures and BC/ASTC format families
- Graphics/compute/2D/DMA/inline-to-memory engine frontends
- MME state/interpreter boundary
- Shader IR capable of ALU, memory, textures, control flow, discard and barriers
- Portable BLAKE3 cache keys and dependency graph
- Vulkan execution plan carrying the full canonical command surface

## Lossless Maxwell state

`Maxwell3dState::raw_registers` mirrors every method write. A register may therefore be promoted later from raw to typed semantics without recapturing the command stream.

## Semantic coverage rule

Unknown register/opcode -> preserved opaque data. Never fabricate a portable semantic.

## SM53 oracle tooling

`envydis_oracle` can optionally call a locally installed envytools `envydis -m gm107` during development. This is a tooling/oracle path only; the Exosphere runtime remains independent. The intended workflow is:

1. disassemble unknown shader with the oracle,
2. compare with Exosphere's decoder,
3. implement and test semantic lifting,
4. cache the native Exosphere result,
5. remove reliance on the oracle for that opcode family.

## Backend capability contract

Backends expose whether a feature is `Native`, `Emulated`, or `Unsupported`. The canonical IR is therefore not constrained to the least capable backend, and OpenGL compatibility or early Vulkan backends can reject a feature explicitly instead of silently misrendering it.

## What still prevents the phrase “100% hardware compatibility”

- undocumented/partially documented SM53 instruction encodings,
- complete raw MME instruction decoder,
- obscure Maxwell registers and compression/ZCull behavior,
- exact game-specific corner cases discovered only from real workloads.

These are coverage tasks, not architectural gaps.


## Raw Linux Vulkan implementation status

The raw backend intentionally validates each plan before execution. Unsupported features produce a typed `UnsupportedFeature` error; they are never silently skipped. The current raw offscreen executor is a correctness smoke backend, while the canonical plan already transports the full v1 surface.
