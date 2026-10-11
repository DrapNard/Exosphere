# Exosphere

**Author / maintainer:** DrapNard  
**License:** GPL-3.0-or-later  
**Current implementation target:** Linux-first; Phases 1–3 baseline established, Phase 4 Horizon/VFS and Phase 5 GPU/Vulkan actively expanding.

Exosphere is an experimental Switch compatibility/compiler/runtime project that moves as much work
as possible into offline analysis/AOT compilation while retaining conservative runtime fallbacks.

## Current status

- **Phase 1:** workspace/tooling, ABI policy, rustfmt/clippy/CI, CAS+SQLite, CLI/UI and platform/plugin boundaries.
- **Phase 2:** NRO/NSO/PFS0/ExeFS/HFS0/XCI/RomFS plus plaintext NCA model and a user-key-backed crypto-provider boundary.
- **Phase 3:** decoded AArch64 IR → CFG → SSA-oriented semantic IR → LLVM AOT output / Cranelift JIT for the safe subset.

Unsupported ARM64 semantics are never guessed. They become explicit fallback operations, allowing
coverage to grow without changing the canonical IR or miscompiling games.

## Content policy

Exosphere does not ship console keys, games, DLC, firmware, or proprietary Nintendo SDK content.
Encrypted NCA handling is exposed through a provider interface whose key material is supplied by the
user/environment and is not persisted into Exosphere packages.

## Development checks

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

The C/C++ ABI header also has a compiler smoke test:

```bash
c++ -std=c++20 -Wall -Wextra -Werror -Isdk/include -fsyntax-only sdk/tests/header_smoke.cpp
```

## Inspect content

```bash
cargo run -p exosphere-cli -- inspect homebrew.nro --decode 32
cargo run -p exosphere-cli -- inspect main.nso
cargo run -p exosphere-cli -- inspect root.hfs0
cargo run -p exosphere-cli -- inspect game.xci
cargo run -p exosphere-cli -- inspect extracted.romfs
cargo run -p exosphere-cli -- inspect decrypted.nca
```

## CPU pipeline

```bash
# Lift an NRO/NSO and show CFG/SSA lowering statistics.
cargo run -p exosphere-cli -- cpu-ir main.nso

# Also emit textual LLVM IR suitable for LLVM/clang tooling.
cargo run -p exosphere-cli -- cpu-ir main.nso --llvm > main.ll
```

## Why the CPU fallback exists

AOT/JIT is used only where Exosphere knows the instruction semantics. An opaque instruction causes
that path to require the baseline fallback instead of silently emitting incorrect host code. This is
a compatibility feature, not a temporary error path.

## Current implementation focus

Phase 4 now includes HIPC/CMIF, VFS/save scaffolding, HID/time/display and firmware-profile boundaries. Phase 5 now emits SPIR-V for the portable shader subset and lowers it into Vulkan execution plans. See `IMPLEMENTATION_STATUS.md` for exact remaining work.

## Current implementation snapshot

The current Linux-oriented Phase 4/5 baseline additionally includes:

- HIPC buffer descriptors materialized through guest memory permissions;
- `fsp-srv` file/directory operations using real IPC buffers;
- PFIFO/subchannel frontend for Maxwell command streams;
- structural Maxwell shader parsing with opaque semantic fallback;
- raw Vulkan instance/device/graphics-queue creation;
- command-pool allocation and a real empty command-buffer queue submission smoke test.


- Phases 1–3: compiler/ingest/CPU baseline established.
- Phase 4: Horizon kernel baseline implemented (memory, handles, processes, threads, typed IPC, initial SVC dispatch).
- Phase 5: canonical GPU IR now lowers a portable triangle to a deterministic Vulkan execution plan.
- Next: full HIPC domains/descriptors + fsp-srv objects, Maxwell shader translation/MME, then raw Vulkan device/swapchain submission.


## Horizon/VFS/GPU smoke tests

```bash
cargo run -p exosphere-cli -- horizon-smoke
cargo run -p exosphere-cli -- horizon-ipc-smoke
cargo run -p exosphere-cli -- fs-smoke
cargo run -p exosphere-cli -- gpu-triangle --width 1280 --height 720
```

## Current development smoke commands

In addition to the previous compiler/Horizon tests, the current tree includes:

```bash
cargo run -p exosphere-cli -- vulkan-smoke
cargo run -p exosphere-cli -- maxwell-shader-inspect shader.bin --stage vertex
cargo run -p exosphere-cli -- save-journal-smoke ./tmp-save
```

`vulkan-smoke` exercises the real Linux Vulkan loader and creates a headless Vulkan instance. `save-journal-smoke` exercises Exosphere's persistent write-ahead save journal. `maxwell-shader-inspect` structurally parses Tegra X1 shader programs while preserving still-unknown ISA instructions as opaque IR.

### Real Vulkan raster smoke

The Linux backend can now execute Exosphere's triangle `VulkanExecutionPlan` through a real offscreen Vulkan render target and verify the resulting pixels by copying the RGBA8 image back to host-visible memory:

```bash
cargo run -p exosphere-cli -- vulkan-triangle-smoke --width 256 --height 256
```

This remains deliberately offscreen: swapchain/window presentation belongs to the platform/display layer, while this command validates Shader IR → SPIR-V → VkPipeline → vkCmdDraw → image readback independently.

### Maxwell frontend inspection

Exosphere can now process a raw PFIFO capture through subchannel routing and the stateful MAXWELL_B frontend:

```bash
cargo run -p exosphere-cli -- maxwell-frontend-inspect pushbuffer.bin
```

The report includes the current program-region registers, MME uploads, macro parameters, understood graphics events and opaque writes still requiring implementation. Unsupported register semantics are retained rather than guessed.

### Stateful Maxwell frontend

The GPU frontend now consumes a raw pushbuffer through PFIFO decoding, subchannel/class tracking and stateful engine models. MAXWELL_B program-region/MME/macro state and MAXWELL_DMA_COPY_A transfer state are retained while unsupported methods remain visible in the trace.

```bash
cargo run -p exosphere-cli -- maxwell-frontend-inspect pushbuffer.bin
```

This is the intended bridge from captured Switch command streams toward shader discovery and canonical GPU IR.

## GPU v0.1 one-shot baseline

The GPU stack now has an end-to-end structural path across all five exposed Tegra/Maxwell engine classes:
graphics, compute, inline-to-memory, 2D and DMA. QMD parsing, shader discovery, MME program extraction,
dependency graphing and portable cache keys are also part of the canonical GPU layer.

See [`docs/GPU_ONE_SHOT.md`](docs/GPU_ONE_SHOT.md) for the exact distinction between **GPU-stack complete**
and **full semantic/game compatibility**.

## Latest GPU continuation

The GPU stack now includes a guest GPU virtual-address space, stable BLAKE3-backed resource interning, lossless texture/sampler descriptors, common compressed texture footprint metadata, and backend-neutral resource hazard analysis. DMA copies no longer treat raw guest addresses as resource IDs. See `docs/GPU_CONTINUATION.md`.

## Latest GPU continuation

See [`docs/GPU_V03_NOTES.md`](docs/GPU_V03_NOTES.md) for block-linear addressing, TIC/TSC descriptor policy, MME semantic execution, and synchronization/resource-identity work.

## GPU feature-complete v1

Canonical GPU IR/backends now cover the full planned command/state surface. See `docs/GPU_FEATURE_COMPLETE_V1.md`. Remaining GPU work is semantic coverage of opaque Maxwell/MME behavior and backend-native implementation depth, not another architecture redesign.

### GPU coverage/tooling helpers

```bash
cargo run -p exosphere-cli -- gpu-capabilities
cargo run -p exosphere-cli -- envydis-oracle shader.bin --program /usr/bin/envydis
```

`envydis-oracle` is optional development tooling only; the runtime never depends on envytools.


### Vulkan frame runtime

The ash Vulkan 1.3 backend now uses a reusable three-slot frame ring with per-frame command pools, command buffers, fences, transient descriptor lifetime tracking, and an internal timeline semaphore. Guest writeback remains synchronous until the execution API grows explicit completion tokens. See `docs/VULKAN_FEATURE_ROADMAP.md`.

## GPU runtime resources

GPU descriptors remain immutable/serializable while live bytes are supplied through `GpuResourceData`. `MappedGpuResourceData` resolves stable ResourceIds through the guest GPU virtual address space, preserving aliases/remaps and enforcing mapping permissions. See `docs/GPU_RUNTIME_RESOURCES.md`.

Smoke test:

```bash
cargo run -p exosphere-cli -- gpu-transfer-smoke
```

- [Native Vulkan transfers](docs/GPU_NATIVE_TRANSFERS.md)

## Vulkan one-shot v2

- Added a general Vulkan 1.3 backend based on `ash` alongside the raw smoke backend.
- Canonical GPU IR now includes vertex input layout and descriptor/push-constant layout.
- Native core paths include buffer allocation/upload/writeback, vertex/index binding, direct/indirect draw and dispatch, buffer copy/fill, dynamic viewport/scissor, and synchronization2 barriers.
- Image/sampler allocation and the complete Vulkan format map are present; block-linear guest textures are explicitly rejected until converted to a host-linear representation.
- Remaining extension-heavy/image-descriptor paths return explicit `UnsupportedFeature` rather than being silently ignored.

See `docs/VULKAN_ONE_SHOT_V2.md` and `docs/VULKAN_BACKEND_V2_MATRIX.md`.

### Vulkan core status (v3.0)

The ash Vulkan 1.3 backend now covers the Phase-5 execution core, including async timeline tokens,
dynamic rendering, graphics/compute pipelines, descriptors, queries, MSAA image descriptors,
Tegra block-linear upload conversion, guest sync bridging for the current single queue, and optional
conditional-rendering/transform-feedback extensions. See `docs/VULKAN_CORE_FINAL.md` and
`docs/VULKAN_FEATURE_ROADMAP.md` for the remaining performance/platform/frontend boundaries.

### Maxwell frontend coverage

The post-Vulkan frontend now distinguishes opcode recognition from semantic shader lifting. `maxwell_opcode_coverage()` reports documented SM5x instruction families while unsafe/unfinished operand semantics remain `OpaqueMaxwell`. The MME path also contains a native decoder/interpreter for the documented macro arithmetic/branch/method-emission core. See `docs/MAXWELL_FRONTEND_ROADMAP.md`.

### Maxwell semantic coverage
The Maxwell frontend includes a coverage-driven SM53 semantic decoder. It reports whether an
instruction is ready to lower, decoded but blocked by canonical-IR expressiveness, or still an
unknown encoding. See `docs/SM53_SEMANTIC_COVERAGE.md`.
