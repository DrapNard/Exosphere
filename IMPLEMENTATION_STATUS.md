# Exosphere implementation status

**Maintainer:** DrapNard  
**Snapshot:** full Exosphere rename + expanded Horizon Phase 4 + Shader IR → SPIR-V Phase 5

## Project identity

The old `snr-*` bootstrap names are retired. Public/project-facing identifiers now use Exosphere:

- crates: `exosphere-*`
- CLI/UI: `exosphere-cli`, `exosphere-ui`
- Rust imports: `exosphere_*`
- C ABI: `Exosphere*` / `EXOSPHERE_*`
- device-plugin entrypoint: `exosphere_device_plugin_query`
- SDK header: `sdk/include/exosphere_device_plugin.h`

## Completed baselines

### Phase 1 — repository/tooling
- Rust workspace split by subsystem
- `rustfmt.toml`, clippy and CI structure
- stable C ABI + hypervisor device-plugin ABI
- Doxygen C/C++ comments and Rustdoc boundaries
- CLI + minimal iced frontend
- CAS/manifest/package/resolver boundaries

### Phase 2 — ingest
- NRO/NRO assets
- NSO parsing, LZ4 decompression and hash verification
- PFS0 / ExeFS
- HFS0 / XCI
- RomFS
- NCA structural parsing + user-supplied crypto-provider boundary

### Phase 3 — CPU
- ARM64 decoded IR
- CFG + SSA-oriented semantic IR
- conservative opaque/fallback instructions
- LLVM textual AOT lowering boundary
- Cranelift JIT for the currently safe semantic subset
- BLAKE3 CPU cache keys

## Phase 4 — Horizon expanded baseline

Implemented:
- page-aligned address space with permissions and overlap validation
- loader-only RX initialization path
- generation-checked handle table
- process/thread model and AArch64 user context
- deterministic scheduler queue and kernel event primitive
- service registry + `sm:` initialization path
- HIPC fixed-header/special-header parsing
- CMIF `SFCI` request / `SFCO` response parsing and encoding
- process-local service sessions
- thread IPC buffer + sync-request dispatch boundary
- SVC decode with CreateThread/StartThread/SleepThread/SendSyncRequest smoke path
- host-neutral HID state for keyboard/mouse plugin events
- injectable monotonic/wall-clock time source
- display/layer manager independent from Vulkan
- versioned Horizon profile database model

### Filesystem/VFS
- normalized guest paths and mount isolation
- read/write mount policy
- overlay FS suitable for base/update/DLC/mod layering
- deterministic in-memory backend
- sandboxed host-directory backend for saves/cache/SD/dev mounts
- symlink escape rejection in host backend
- save-data identity metadata
- staged transaction journal with commit/rollback API
- Horizon FS facade

Still required for full Phase 4 compatibility:
- complete HIPC descriptor semantics and domains
- TIPC service-manager path
- full wait/arbitration SVC semantics
- exact `fsp-srv` object/command implementation
- persistent crash-safe save journal
- exact HID shared-memory serialization
- audio service stack
- full VI/Binder presentation services
- generated per-firmware service command tables

## Phase 5 — GPU/Vulkan expanded baseline

Implemented:
- split canonical GPU IR for shaders/resources/commands/module
- conservative Maxwell method decoder with opaque fallback
- graphics pipeline descriptors and render/draw semantics
- Shader IR → actual SPIR-V emission for portable Vertex/Fragment smoke shaders
- deterministic Vulkan execution plan now containing host-ready SPIR-V modules
- portable triangle smoke-test module
- dynamic Vulkan loader probe on Linux
- raw Vulkan-device trait isolated from canonical IR
- opaque Maxwell shaders/commands fail safely instead of being guessed

Still required before a real rendered game frame:
- broad Maxwell command/register coverage
- Maxwell shader ISA translator into Shader IR
- MME decoder/interpreter/AOT path
- swapchain/presentation
- guest resource/descriptor binding
- production synchronization/barrier scheduling

## CLI smoke tests

```bash
cargo run -p exosphere-cli -- horizon-smoke
cargo run -p exosphere-cli -- horizon-ipc-smoke
cargo run -p exosphere-cli -- fs-smoke
cargo run -p exosphere-cli -- gpu-triangle --width 1280 --height 720
```

These are deliberately synthetic. They validate the internal contracts independently before a game boot path wires every subsystem together.

## Latest Phase 4/5 continuation

### Horizon / filesystem
- persistent save-data journal format (`EXOJNL01`) with durable write-before-apply ordering
- crash recovery replay through `PersistentSaveJournal::recover`
- host-directory flush now syncs files/directories before journal removal
- read-only RomFS-to-VFS adapter mounts original RomFS images without extracting to host folders
- initial `fsp-srv` command surface with real command IDs for SetCurrentProcess, SD/host/data/save mounts and ExFAT query
- `sm:` GetServiceHandle path now returns a real process-local move handle rather than synthetic payload bytes
- HIPC response builder supports copy/move native handles via the special header

### GPU / Vulkan
- structural Maxwell graphics/compute shader parser
- 0x50 graphics program header retained verbatim
- scheduler/control words separated from executable 64-bit instruction words
- unknown Maxwell instructions lower to canonical `OpaqueMaxwell` operations, never guessed semantics
- raw Linux Vulkan smoke path now loads the Vulkan loader, creates/destroys a real VkInstance and enumerates physical-device count

### New CLI validation commands

```bash
cargo run -p exosphere-cli -- vulkan-smoke
cargo run -p exosphere-cli -- maxwell-shader-inspect shader.bin --stage vertex
cargo run -p exosphere-cli -- save-journal-smoke ./tmp-save
```

The raw Vulkan path now reaches logical-device/queue creation and an empty command-buffer submission. Surfaces/swapchains, resource binding, pipelines, and rendered draws remain behind the existing `VulkanDevice` boundary.

## 2026-10-02 — Phase 4/5 continuation: FS objects + PFIFO + Vulkan device

### Horizon / filesystem

- `fsp-srv` can now return typed filesystem objects rather than placeholder service names.
- Added `FsSessionObject::{FileSystem,File,Directory}`.
- Added canonical command adapters for `IFileSystem`, `IFile`, and `IDirectory`.
- File open modes enforce read/write permissions and mount writability.
- Directory reads keep a per-session cursor and emit Horizon-shaped 0x310-byte directory entries.
- VFS backends gained `set_file_size`, `free_space`, and `total_space` extension points.
- Kernel sessions now distinguish named services from filesystem object sessions and materialize move handles centrally.
- HIPC/CMIF request decoding is split from service dispatch so object sessions share the same transport path.

### Maxwell / GPU frontend

- Added PFIFO push-buffer packet decoder.
- Supports INC_METHOD, NON_INC_METHOD, IMMD_DATA_METHOD, ONE_INC, END_PB_SEGMENT and conservative tertiary mode 0.
- Added subchannel `SetObject` tracking for MAXWELL_B, MAXWELL_COMPUTE_B, FERMI_TWOD_A, KEPLER_INLINE_TO_MEMORY_B and MAXWELL_DMA_COPY_A.
- Added `exosphere-cli pfifo-inspect` to inspect raw command streams.

### Vulkan Linux

- Corrected the raw `VkApplicationInfo` ABI layout.
- Raw smoke path now enumerates physical devices, finds a graphics-capable queue family, creates a logical device and obtains a `VkQueue`.
- Destruction order is device-idle -> device -> instance.
- Swapchain/presentation is intentionally still outside canonical GPU IR.

### Remaining Phase 4/5 blockers

- HIPC domain commands and remaining receive-list/pointer-buffer marshalling edge cases.
- Full save-data attribute/space resolution and recursive FS operations.
- Maxwell engine method semantics beyond class routing.
- Maxwell shader ISA semantic lifting.
- Vulkan pipeline creation, images/resources, synchronization beyond queue-idle smoke, presentation/swapchain.
## 2026-10-02 — HIPC buffers + Vulkan queue submission

### Horizon IPC / fsp-srv

- HIPC now decodes pointer (X), map-alias send/receive/exchange (A/B/W), and explicit receive-list (C) descriptors.
- Descriptor guest addresses and capacities are materialized only after `AddressSpace` permission/bounds validation.
- Input/InOut descriptor contents are copied from guest memory into `ServiceRequest.buffers`.
- Service output buffer writes are validated and copied back into guest memory after dispatch.
- `IFileSystem`, `IFile`, and `IDirectory` prefer real HIPC buffers for paths and data.
- `IFile::Read` / `IFile::Write` use the Horizon raw argument layout (option/padding/offset/size).
- Directory reads write Horizon-shaped entries through output buffers.
- Direct `IpcValue` payloads remain only as a compatibility/test fallback while the remaining IPC surface is filled in.
- HIPC domain commands and special receive-list/pointer-buffer marshalling remain incomplete and are intentionally not guessed.

### Vulkan Linux

- Raw Vulkan smoke path now creates a command pool for the selected graphics queue family.
- Allocates and records a primary one-time command buffer.
- Submits it through `vkQueueSubmit` and waits with `vkQueueWaitIdle`.
- Command buffers/pool are freed/destroyed in all normal result paths.
- `exosphere-cli vulkan-smoke` now validates a real queue submission, not only instance/device creation.

### Next blockers

- HIPC domains and remaining buffer-marshalling edge cases.
- `IStorage` object/session implementation and remaining save-data resolution.
- Maxwell engine state accumulation and shader upload discovery.
- Maxwell ISA semantic lifting into Shader IR.
- Vulkan shader modules, descriptor/resource binding, pipeline/render target and first real draw submission.



## 2026-10-02 — Real offscreen raster + IStorage baseline

### Vulkan graphics execution

- Portable smoke vertex shader now derives three non-degenerate clip-space positions from the SPIR-V `VertexIndex` built-in.
- Added a raw Linux Vulkan offscreen path that creates an RGBA8 image, device memory, image view, render pass, framebuffer, shader modules, pipeline layout and graphics pipeline.
- Records a real render pass and `vkCmdDraw`, submits it to the selected graphics queue and waits for completion.
- Offscreen images are created with transfer-source usage and finish the render pass in transfer-source layout.
- Added a host-visible/coherent readback buffer and `vkCmdCopyImageToBuffer`.
- The smoke maps the readback memory and verifies that rasterized bright triangle pixels differ from the dark clear color. A submitted-but-empty/degenerate draw therefore fails validation.
- `RawLinuxVulkanDevice` executes the same `VulkanExecutionPlan` produced by the backend compiler rather than bypassing canonical GPU IR.
- Resource teardown performs a conservative queue-idle wait before destroying objects, including error paths after submission.

Still intentionally outside this smoke path:

- swapchain/window-system presentation,
- guest texture/buffer descriptor binding,
- broad Maxwell render-state translation,
- Maxwell ISA semantic lifting beyond opaque fallback,
- production synchronization/barrier scheduling.

### Horizon IStorage

- `HorizonFs` now keeps raw `VirtualStorage` mounts separately from path-based filesystems.
- Added kernel-managed `IStorage` sessions and returned-object materialization.
- Implemented Read, Write, Flush, SetSize and GetSize adapters over normalized HIPC buffers.
- `fsp-srv` exposes `OpenDataStorageByCurrentProcess` through the storage-object path.
- Added a bounded read/write `MemoryStorage` backend for tests and loader wiring.

### New smoke command

```bash
cargo run -p exosphere-cli -- vulkan-triangle-smoke --width 256 --height 256
```

Success now means a graphics pipeline was created, a draw was submitted, the color target was copied back, and non-clear triangle pixels were observed.

## 2026-10-02 — Maxwell stateful frontend

Phase 5 now includes a stateful Maxwell command-stream frontend rather than treating PFIFO writes as isolated calls.

Implemented:

- PFIFO -> subchannel/class routing -> MAXWELL_B state
- corrected MAXWELL_B `0x585 = End`, `0x586 = Begin` (the old provisional draw mapping was removed)
- program-region A/B tracking
- MME instruction RAM upload/pointer tracking
- MME start-address RAM tracking
- MME shadow-control tracking
- macro-call/parameter collection for the `0xE00+` method range
- lossless opaque engine-write trace for unsupported engines
- GPU-memory abstraction for shader discovery without coupling the frontend to nvmap/runtime memory
- shader location objects relative to the programmed region
- semantic Maxwell shader lift coverage: known NOP vs opaque instructions
- `maxwell-frontend-inspect` CLI command for real pushbuffer captures

The project deliberately does **not** guess the A/B program-region address ordering in canonical state. A verified backend adapter may opt into a high/low interpretation when appropriate.

## 2026-10-02 — Maxwell frontend continuation

Additional Phase 5 work:

- `MaxwellFrontEnd` now joins PFIFO decoding, subchannel class binding, MAXWELL_B state and DMA state.
- corrected the old provisional `0x585` draw mapping: MAXWELL_B `0x585` is `End`, `0x586` is `Begin`.
- program-region A/B state is preserved as raw address halves; the canonical frontend does not silently guess their ordering.
- MME instruction RAM and MME start-address RAM uploads are tracked with auto-increment pointers.
- macro methods at `0xE00 + n*2` collect the first parameter and subsequent FIFO parameters.
- `GpuMemory` and `ShaderLocation` provide a backend-neutral boundary for reading shader code from guest GPU memory.
- semantic shader lifting now reports exact coverage (`semantic` vs `opaque`) and only claims all-zero Maxwell instructions as NOP at this stage.
- `MAXWELL_DMA_COPY_A` tracks input/output addresses, pitches, line length/count and emits a complete launch snapshot.
- `maxwell-frontend-inspect` CLI exposes state, MME/macro activity, DMA launches and unsupported engine writes from raw PFIFO captures.

This keeps the next compiler stage honest: unknown register and ISA semantics stay explicit and measurable rather than being translated approximately.

## 2026-10-02 — GPU one-shot structural completion

Exosphere now has a v0.1 structural path for every major Switch GPU class used by the runtime:

- PFIFO packet decode + subchannel SetObject routing
- MAXWELL_B graphics state including color targets, viewports, scissors, pipeline shader slots,
  vertex-array/index-buffer state, Begin/End, barriers, syncpoint events and MME upload/calls
- MAXWELL_COMPUTE_B state + QMD submission boundary
- partial QMD 01.07 parser for program address, grid/block dimensions, shared memory, register count and constant-buffer descriptors
- KEPLER_INLINE_TO_MEMORY_B state and inline data collection
- FERMI_TWOD_A source/destination/clip/operation state with a lossless raw register map
- MAXWELL_DMA_COPY_A address/pitch/shape/remap/block state
- pipeline shader discovery relative to Program Region
- explicit shader windows + GPU-memory compiler boundary
- MME program extraction boundary
- dependency-graph construction and portable BLAKE3 GPU cache keys
- canonical graphics/compute pipeline/resource/command models expanded for indexed draws, viewport/scissor,
  compute pipelines, fill/copy and buffer binding
- Vulkan plans carry these normalized operations without leaking Vulkan handles into canonical IR

Important: this is intentionally called **structural completion**, not 100% semantic compatibility.
The remaining compatibility work is dominated by SM53 instruction semantics, MME semantics, exact image
format/tiling/compression behavior, descriptor binding and rare engine registers. Opaque fallbacks remain explicit.

See `docs/GPU_ONE_SHOT.md` for the coverage matrix.

## GPU resource/address continuation

- [x] guest GPU virtual address mappings → stable ResourceId + offset
- [x] overlap/unmapped-range validation
- [x] BLAKE3 resource interning independent of GPU VA
- [x] canonical texture/sampler raw descriptor containers
- [x] common BC/ASTC/uncompressed texture footprint metadata
- [x] RAW/WAR/WAW hazard analysis
- [x] DMA lowering requires mapped resources (no address-as-ID shortcut)
- [ ] exact Tegra block-linear byte swizzle for all layouts
- [ ] full TIC/TSC bitfield promotion
- [ ] complete MME / SM53 semantics

## 2026-10-02 — GPU resources/synchronization continuation v0.3

### Block-linear memory

- Added a byte-coordinate Tegra X1 block-linear layout helper.
- GOB geometry is explicit: 64x8 bytes / 512 bytes.
- Block height/depth are represented as log2 GOB counts.
- Storage-size padding and byte-offset calculation are isolated from texture pixel formats.
- Compressed formats therefore reuse the exact same tiled addressing path after pixel/block -> byte conversion.

### Texture formats / descriptors

- Expanded canonical texture formats with BC2/BC4/BC6H/BC7 and common ASTC block sizes.
- `TextureLayout::BlockLinear` now carries `block_height_log2`, `block_depth_log2`, and the raw Maxwell layout word.
- TIC/TSC descriptor records are readable directly through `GpuMemory` at their 0x20-byte table stride.
- Texture handle splitting is explicit: lower 20 bits TIC, upper 12 bits TSC.
- Descriptor bitfields that are not capture-validated remain raw/unknown. Exosphere does not guess TIC/TSC fields.

### MME

- Added a semantic MME interpreter boundary with registers, parameters, ALU, branches and method emissions.
- Raw MME words remain opaque in the native decoder until validated against public tooling/captures.
- The semantic interpreter is ready for a future native decoder, envydis-assisted validation, or AOT/JIT backend.

### Synchronization

- Added canonical semaphore/syncpoint operations and an in-memory timeline model.
- Supports signal/wait semantics plus syncpoint increment/threshold wait.

### Resource correctness

- Removed the final helper that treated a color-target GPU address as a `ResourceId`.
- Color targets now produce `UnresolvedTextureDesc`; callers must resolve guest addresses through `GpuAddressSpace` and then intern resources through `ResourceTable`.

### New CLI tools

```bash
cargo run -p exosphere-cli -- block-linear-smoke
cargo run -p exosphere-cli -- descriptor-inspect descriptor.bin --kind tic
cargo run -p exosphere-cli -- descriptor-inspect descriptor.bin --kind tsc
cargo run -p exosphere-cli -- mme-smoke
```

Validation for v0.3 snapshot: 26 Cargo manifests parsed; 116 Rust source files passed coarse truncation/delimiter checks; C++ public plugin header compiles through a normal translation unit with `clang++ -Wall -Wextra -Werror`. Rust compilation still requires a host with cargo/rustc.

## GPU feature-complete v1

Canonical GPU IR/backends now cover the full planned command/state surface. See `docs/GPU_FEATURE_COMPLETE_V1.md`. Remaining GPU work is semantic coverage of opaque Maxwell/MME behavior and backend-native implementation depth, not another architecture redesign.


## 2026-10-03 — GPU runtime resource bridge

- Added backend-neutral `GpuResourceData` payload access.
- Added deterministic `InMemoryGpuResources` for tests/tooling.
- Added `GpuMemoryAccess` and `MappedGpuResourceData` to resolve ResourceId-relative accesses through `GpuAddressSpace` aliases/remaps into guest GPU virtual memory.
- Added pre-driver Vulkan resource validation for missing resources, out-of-bounds ranges, indirect command sizes and sampled-resource payload availability.
- Added resource-aware `VulkanDevice::execute_plan_with_resources`.
- Added correctness-first CPU fallback for transfer-only `CopyBuffer` and `FillBuffer` plans.
- Added `gpu-transfer-smoke` CLI command.

This closes the architectural gap between canonical resource descriptors and the actual bytes consumed or produced by host GPU backends.

## GPU native transfer update

- [x] Guest payload → host-visible Vulkan buffer upload
- [x] `vkCmdCopyBuffer`
- [x] `vkCmdFillBuffer`
- [x] conservative transfer memory barriers
- [x] queue submission/wait
- [x] Vulkan buffer → guest payload writeback
- [x] CPU correctness fallback when native allocation/shape is unavailable
- [ ] device-local staging allocator
- [ ] native vertex/index binding
- [ ] native texture/image upload

## Vulkan one-shot v2

- Added a general Vulkan 1.3 backend based on `ash` alongside the raw smoke backend.
- Canonical GPU IR now includes vertex input layout and descriptor/push-constant layout.
- Native core paths include buffer allocation/upload/writeback, vertex/index binding, direct/indirect draw and dispatch, buffer copy/fill, dynamic viewport/scissor, and synchronization2 barriers.
- Image/sampler allocation and the complete Vulkan format map are present; block-linear guest textures are explicitly rejected until converted to a host-linear representation.
- Remaining extension-heavy/image-descriptor paths return explicit `UnsupportedFeature` rather than being silently ignored.

See `docs/VULKAN_ONE_SHOT_V2.md` and `docs/VULKAN_BACKEND_V2_MATRIX.md`.

## Vulkan images v2.1

- Native linear guest texture upload through a staging buffer.
- Per-image Vulkan layout tracking.
- Native image transitions through synchronization2.
- Native `CopyImage`, `Blit`, `Resolve`, color clear and depth/stencil clear command emission.
- Vulkan feature roadmap is maintained in `docs/VULKAN_FEATURE_ROADMAP.md`.
- Dynamic rendering, descriptor population, pipeline construction and presentation remain the next Vulkan closure milestone.

## Vulkan v2.2 — descriptors, pipelines, dynamic rendering

- Added explicit canonical descriptor binding commands `(set,binding,array_element)` for buffers, images and samplers.
- Added canonical push-constant command while preserving legacy Maxwell slot bindings.
- Ash backend now creates descriptor pools/sets and performs buffer/image/sampler/combined writes.
- Added cached SPIR-V shader modules and real graphics/compute pipeline creation.
- Graphics pipelines are specialized against current dynamic-rendering attachment formats.
- Added Vulkan 1.3 dynamic rendering with multiple color slots and depth/stencil attachments.
- Draw/indexed/indirect and compute dispatch now bind real pipelines and descriptors before execution.
- Added `vulkan-ash-triangle-smoke` CLI path using an explicit canonical render target.

## Vulkan descriptors/pipelines v2.2

- explicit backend-neutral descriptor bindings are in the GPU IR;
- descriptor-set layouts/pools/sets are created by the ash backend;
- uniform/storage/image/sampler/combined descriptors are written and bound;
- push constants are validated against the canonical pipeline layout and emitted;
- graphics and compute pipelines are created/cached from canonical descriptors;
- dynamic rendering is connected to canonical render targets;
- `vulkan-ash-triangle-smoke` exercises the new ash rendering path.

The core headless Vulkan render/compute path is now structurally closed. Remaining Vulkan work is concentrated on queries, presentation, exact timeline synchronization and production/performance systems.

## Vulkan v2.3 queries/sync

- Native Vulkan query pools: occlusion, timestamp and pipeline statistics.
- Native 64-bit query result copy into guest-backed buffers.
- Normal buffer writeback exposes query results to guest memory.
- Queue-wide `vkQueueWaitIdle` removed from normal ash execution; per-submit fences are used instead.
- Reusable frame contexts and timeline guest synchronization remain the next synchronization milestone.

## 2026-10-03 — Vulkan frame runtime v2.4

- Replaced per-plan command-buffer/fence allocation on the main ash execution path with a reusable 3-slot frame-context ring.
- Each frame owns a command pool, primary command buffer, reusable fence and transient descriptor pools.
- Added an internal Vulkan 1.3 timeline semaphore; every main submission signals a monotonically increasing value through `vkQueueSubmit2`.
- Descriptor pools now survive until the owning frame slot is known complete, preparing deferred destruction correctly.
- Guest resource writeback remains synchronous by design. True multiple-frames-in-flight requires an execution/completion-token API so guest memory lifetimes remain explicit.


## 2026-10-03 — Vulkan production-core mega pass v2.5

- Added Vulkan timeline completion polling and targeted waits to the ash backend.
- Added a real `VkPipelineCache` used by graphics and compute pipeline creation.
- Added pipeline-cache serialization so the platform/runtime layer can persist driver cache bytes without coupling GPU code to filesystem policy.
- Image layout transitions now cover every declared mip level and array layer instead of only mip 0/layer 0.
- Guest semaphore/syncpoint commands are no longer silently ignored: until the guest-object bridge is implemented they fail explicitly, preserving correctness.
- Wayland/X11 remain intentionally outside Phase 5; Linux owns window/surface creation in Phase 10.

## 2026-10-03 — Vulkan async core v2.6
- Added backend-neutral `ExecutionToken` submission/completion API.
- Ash Vulkan submissions can return before GPU completion; timeline polling is exposed.
- Guest buffer writeback is deferred until `complete_execution`, enabling real frame overlap for work whose guest results are not immediately consumed.
- Synchronous `execute_with_resources` remains as a compatibility wrapper.
- Added pipeline-cache import as well as export.
- Added a timeline-aware aligned range suballocator for device-local heaps and staging/readback rings.
- Wayland/X11 remain explicitly outside Phase 5 and belong to the Linux platform phase.

## 2026-10-03 — Vulkan core closure v3.0

- Added Tegra X1 block-linear deswizzle to the Vulkan texture upload path using the canonical `BlockLinearLayout`.
- Linear texture uploads now build per-mip `VkBufferImageCopy` regions and cover all array layers.
- Added `TextureDesc::samples`; Vulkan images can now be created with real MSAA sample counts and pipelines validate attachment sample compatibility.
- Negotiates supported Vulkan core raster features instead of hard-rejecting depth clamp/non-solid/logic-op/alpha-to-one.
- Added optional `VK_EXT_conditional_rendering` and `VK_EXT_transform_feedback` negotiation and native command recording.
- Transform-feedback queries use indexed EXT query commands when available.
- Added a monotonic guest semaphore/syncpoint bridge suitable for the current single ordered Vulkan queue; unsatisfied waits fail explicitly.
- SyncScope now maps to narrower synchronization2 stage/access masks instead of always using an all-commands barrier.
- Remaining Vulkan work is classified as performance/policy, platform presentation, or Maxwell-frontend metadata rather than missing core execution primitives.

## 2026-10-03 — Maxwell frontend / SM53 + MME pass

- Added a conservative SM5x opcode-family classifier derived from public Maxwell opcode tables.
- Added per-family opcode coverage accounting and separated `classified` coverage from truly semantic lifting.
- Kept non-zero instructions opaque until operand/predicate semantics are validated, preventing false shader compilation.
- Replaced the all-opaque MME raw decoder with a documented macro-ISA decoder for arithmetic/logic, immediate parameter operations, branches, exit/NOP and result actions.
- Extended the MME interpreter with carry state, current method address/increment, parameter fetch and PGRAPH method emission.
- Added `docs/MAXWELL_FRONTEND_ROADMAP.md` as the frontend completion checklist.

## 2026-10-03 — Maxwell frontend v4.1 operand/control decode

- Decodes GM107 common GPR positions, predicate guards and signed PC-relative branch targets from the documented envytools encoding.
- Decodes all three scheduler entries in each 64-bit Maxwell scheduler word (stall/yield/read+write dependency barriers/wait/reuse).
- Adds exact opcode identities for common control-flow, ALU, global-memory and texture forms while retaining broad family classification.
- Extends the MME interpreter with extract/insert, extract-shift forms and PGRAPH method reads via a caller-provided method-reader callback.
- Semantic SM53 lifting remains intentionally conservative until physical-register/predicate dataflow is converted to canonical SSA.

## 2026-10-03 — Maxwell frontend v4.2
- Added physical-register Maxwell IR instead of mapping hardware register numbers directly to canonical SSA IDs.
- Added basic-block/CFG construction from decoded SM53 branch targets and fallthroughs.
- Added conservative register versioning and phi-node planning at CFG merges.
- Register-form MOV, integer/float add/mul, shifts and logic now have a typed intermediate representation; uncertain operand forms remain opaque.

### Maxwell frontend v4.3
Added source-resolved, loop-aware SSA and a conservative canonical Shader IR bridge for validated register-form ALU/control-flow instructions. MME invocations can now be executed against live mirrored PGRAPH state and replay bounded method emissions back through MAXWELL_B. Predicate/immediate/CB/memory/texture semantics remain opaque where operand bits are not yet validated.

## Maxwell semantic coverage v4.5

- Added coverage-driven GM107 semantic decoder (`maxwell_semantics`).
- Separates `Ready`, `NeedsCanonicalIr`, and `UnknownEncoding`.
- Added typed register/immediate/constant-buffer/memory/predicate/texture/special-register operand forms.
- Added long-tail structural decoding for 32-bit immediate ALU, XMAD/IMAD/FFMA, bitfield/permutation/shift, conversion, predicate, memory, atomic, texture and special-register families.
- Added per-opcode, per-operand-form and per-fallback-reason coverage reporting.

## Maxwell near-completion pass (v4.6)
- Canonical shader IR now preserves generalized atomics, rich memory accesses, FMA/IMAD/XMAD-style ternaries, bitfield/permutation operations, source modifiers/float controls, interpolation, subgroup shuffle and rich texture payloads.
- Corrected GM107 LD/ST class mapping for global/local/shared forms from the public instruction table.
- Added canonical semantic coverage: decoded long-tail operations no longer remain `NeedsCanonicalIr` merely because the old IR was too small.
- Remaining Maxwell work is deliberately corpus-driven: dynamic control flow/reconvergence validation, bindless/indirect texture forms, surface/image long tail, and unobserved rare opcodes.
