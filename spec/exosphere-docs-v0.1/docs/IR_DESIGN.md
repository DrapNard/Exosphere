# IR Design

## 1. Philosophy

The project owns its canonical IRs.

External representations such as LLVM IR, Cranelift IR, SPIR-V, MSL, GLSL, FFmpeg objects, Vulkan structures, and Metal structures are backend implementation details.

Canonical IRs must be:

- serializable,
- versioned,
- deterministic,
- backend-neutral,
- suitable for hashing,
- suitable for caching,
- safe to extend,
- capable of representing unknown/opaque behavior.

---

## 2. CPU IR

The CPU IR models Switch-visible CPU semantics.

Expected categories:

```text
CpuIr
├── Module
├── Function
├── Block
├── Instruction
├── RegisterState
├── MemoryOperation
├── Branch
├── Call
├── SyscallBoundary
├── Relocation
├── Symbol
└── Unknown/DynamicEdge
```

The IR must preserve enough information to:

- lower to LLVM,
- lower to Cranelift,
- rebuild control-flow graphs,
- retain source addresses,
- support symbolic/fingerprint-based mod hooks,
- support profile-guided recompilation.

---

## 3. GPU IR

GPU IR is split rather than represented as one giant command list.

```text
GpuIr
├── ShaderIr
├── GraphicsIr
├── ComputeIr
├── TransferIr
├── TwoDIr
├── MmeIr
├── SyncIr
├── ResourceIr
└── GraphIr
```

### Shader IR

Must represent:

- stages,
- inputs/outputs,
- registers,
- predicates,
- memory accesses,
- texture operations,
- control flow,
- specialization dimensions,
- backend constraints.

### Graph IR

Represents dependencies between:

- passes,
- pipelines,
- resources,
- barriers,
- surfaces,
- copy operations,
- synchronization points.

The graph is an optimization aid, not a license to reorder commands without proof of equivalent behavior.

---

## 4. Media IR

Media IR describes semantics rather than a specific implementation.

Example conceptual operations:

```text
DecodeVideo
EncodeVideo
DecodeImage
Scale
Crop
ConvertColorspace
Composite
Blit
PresentSurface
```

A job references project-owned resource and surface IDs.

---

## 5. Resource IR

```text
Resource
├── Texture
├── Audio
├── Model
├── Animation
├── StructuredData
├── Localization
├── ScriptData
└── Opaque
```

Every resource carries:

- identity/hash,
- origin,
- dependencies,
- original encoding metadata,
- optional host variants.

Unknown formats remain valid through `Opaque`.

---

## 6. Versioning

Each canonical IR has an explicit schema version.

Example:

```toml
[cpu_ir]
version = 1

[gpu_ir]
version = 1

[media_ir]
version = 1

[resource_ir]
version = 1
```

Breaking changes require explicit migration or recompilation.
