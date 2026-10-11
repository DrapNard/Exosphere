# Architecture

## 1. Overview

The project is split into four major phases:

1. **Ingest**
2. **Analysis / normalization**
3. **Compilation / packaging**
4. **Runtime**

```text
                  SWITCH CONTENT
                        │
                        ▼
            ┌─────────────────────┐
            │        INGEST       │
            │ verify/decrypt/load │
            └──────────┬──────────┘
                       ▼
             ┌───────────────────┐
             │ UNIVERSAL GAME IR │
             └─────────┬─────────┘
                       │
       ┌───────────────┼────────────────┐
       ▼               ▼                ▼
     CPU IR          GPU IR         RESOURCE IR
       │               │                │
       ▼               ▼                ▼
 LLVM/Cranelift   host GPU code       CAS
       │               │                │
       └───────────────┼────────────────┘
                       ▼
                IMMUTABLE PACKAGES
                       │
        ┌──────────────┼──────────────┐
        ▼              ▼              ▼
      Base           Update           DLC
        │              │              │
        └──────────┬───┴───────┬─────┘
                   │           │
              Switch Mods   Native Mods
                   │           │
                   └─────┬─────┘
                         ▼
                      RESOLVER
                         ▼
                  EFFECTIVE GAME
                         ▼
             HORIZON-COMPAT RUNTIME
                         ▼
                      HOST OS
```

---

## 2. Ingest layer

The ingest subsystem accepts user-provided game content and transforms it into a normalized representation.

Supported input families are expected to include:

- XCI
- NSP / PFS containers
- NCA
- NSO
- NRO
- ExeFS
- RomFS
- update content
- DLC
- Switch-native mods

Responsibilities:

```text
input
 ↓
identify
 ↓
verify integrity
 ↓
read metadata
 ↓
decrypt using user-provided keys
 ↓
extract / mount
 ↓
normalize
```

The project must not ship console keys, copyrighted firmware, games, DLC, or other proprietary content.

---

## 3. Universal Game IR

The Universal Game IR is the central boundary of the project.

It must not be tied directly to:

- Vulkan
- Metal
- OpenGL
- LLVM
- Cranelift
- FFmpeg
- a specific host operating system

Instead it represents Switch-visible semantics in a form that can be lowered to multiple backends.

The Game IR consists of:

```text
Game IR
├── CPU IR
├── GPU IR
├── Media IR
├── Resource IR
├── Horizon ABI metadata
├── symbol/fingerprint database
├── dependency graph
└── package metadata
```

---

## 4. CPU subsystem

### 4.1 Goals

The CPU pipeline is AOT-first.

```text
Switch ARM64
    ↓
semantic CPU IR
    ├── LLVM lowering → optimized AOT
    └── Cranelift lowering → JIT fallback
```

The semantic CPU IR is owned by the project.

LLVM IR and Cranelift IR are backend representations, not canonical game representations.

### 4.2 x86-64

For x86-64 hosts:

```text
ARM64
 ↓
CPU IR
 ↓
LLVM
 ↓
x86-64 native code
```

Unknown or dynamically generated code falls back to:

```text
ARM64
 ↓
CPU IR
 ↓
Cranelift
 ↓
JIT code
```

Successfully discovered dynamic code may later be persisted and folded into future optimized builds.

### 4.3 ARM64 hosts

On ARM64 hosts such as Apple Silicon, the runtime may use a more direct ARM64 path where safe.

Potential strategies:

- direct semantic preservation where architecture behavior matches,
- relocation and patching,
- runtime trampolines,
- LLVM lowering where transformation is required.

The implementation must prioritize correctness over assuming Switch ARM64 is directly executable unchanged.

---

## 5. Horizon compatibility layer

The project does not aim to boot a full unmodified Horizon OS as its primary architecture.

Instead it implements a Horizon-compatible execution environment.

Major responsibilities include:

```text
Horizon Runtime
├── SVC
├── IPC
├── handles
├── processes
├── threads
├── virtual memory
├── synchronization
├── filesystem
├── HID
├── audio
├── networking
├── time
├── display services
└── firmware-versioned service behavior
```

The design is hybrid:

- native reimplementation where practical,
- firmware-derived data/components where legally and technically appropriate,
- dynamic fallback where needed.

Horizon behavior must be versioned rather than encoded as one giant global implementation.

---

## 6. Hypervisor abstraction

The hypervisor is an isolation and virtualization facility, not the CPU translation solution.

The public runtime API must not depend directly on one host virtualization framework.

```text
VirtualMachineBackend
├── map_memory()
├── protect_memory()
├── create_vcpu()
├── interrupt()
├── sync()
└── destroy()
```

Possible platform backends:

```text
Linux   → KVM
Windows → WHPX / Hyper-V compatible backend
macOS   → Apple virtualization facilities
```

This layer should remain optional where a host/runtime configuration does not require full VM facilities.

---

## 7. GPU subsystem

The GPU path is split into semantic layers.

```text
Switch GPU / NVN / host1x
            ↓
        GPU Decoder
            ↓
       Universal GPU IR
        ├── Graphics IR
        ├── Compute IR
        ├── Transfer IR
        ├── 2D IR
        ├── MME IR
        ├── Sync IR
        ├── Resource IR
        └── Shader IR
            ↓
         Optimizer
            ↓
       Host backends
        ├── Vulkan
        ├── Metal
        └── OpenGL compatibility
```

### 7.1 Shader pipeline

Shaders are compiled offline whenever possible.

```text
Maxwell shader
    ↓
Shader IR
    ↓
variant analysis
    ↓
variant stripping
    ↓
backend lowering
    ├── SPIR-V / Vulkan
    ├── Metal path
    └── GLSL / OpenGL compatibility
```

The runtime retains a fallback path for dynamically discovered variants.

### 7.2 Shader cache

The shareable cache must target stable project-owned or standardized representations, not driver-specific pipeline blobs.

Possible cache levels:

```text
Universal Shader IR
        +
Canonical SPIR-V
        +
local driver pipeline cache
```

Only the first two are intended to be portable.

### 7.3 GPU graph

Offline analysis may reconstruct stable relationships between:

- render passes,
- shaders,
- buffers,
- textures,
- pipeline state,
- barriers,
- resource lifetimes,
- dependencies.

This graph can be optimized only where semantics are proven safe.

### 7.4 MME

Maxwell macro microcode is treated as a first-class subsystem.

```text
MME
├── decoder
├── MME IR
├── interpreter fallback
└── optimizer / compiled path
```

---

## 8. Tegra host / engine layer

The hardware compatibility layer must account for more than 3D rendering.

```text
Tegra Host Layer
├── GPU / host1x
│   ├── 3D
│   ├── Compute
│   ├── DMA / Copy
│   ├── 2D
│   ├── MME
│   └── Syncpoints / Fences
│
├── Media
│   ├── NVDEC
│   ├── NVENC / MSENC
│   ├── VIC
│   ├── NVJPG
│   ├── ISP
│   └── VI
│
├── Audio
│   └── APE-related behavior
│
├── Security
│   ├── TSEC
│   └── Security Engine
│
└── Interfaces
    ├── nvdrv
    ├── host1x
    └── memory mapping
```

---

## 9. Media subsystem

Media operations are normalized into a project-owned Media IR.

```text
Switch media engines
       ↓
     Media IR
       ├── DecodeVideo
       ├── EncodeVideo
       ├── DecodeImage
       ├── Scale
       ├── Crop
       ├── ColorspaceConvert
       ├── Composite
       └── Blit
       ↓
  MediaBackend
       ├── FFmpeg
       ├── Vulkan Video
       ├── VideoToolbox
       ├── Windows video APIs
       └── software fallback
```

FFmpeg is a backend, not the canonical API.

No `AVFrame`, `AVCodecContext`, or FFmpeg-specific object is allowed inside the canonical Game/Media IR.

---

## 10. Resources

Resources are represented separately from executable code.

```text
RomFS
 ↓
Resource Analyzer
 ↓
Resource IR
├── Texture
├── Audio
├── Model
├── Animation
├── StructuredData
├── Localization
├── Script/Data
└── OpaqueResource
```

Unknown formats must remain preservable byte-for-byte through `OpaqueResource`.

Resources are stored through the content-addressed store and grouped into runtime-friendly resource packages.

---

## 11. Content-addressed storage

BLAKE3 is the default object identity mechanism.

```text
object bytes
   ↓
BLAKE3
   ↓
object id
```

Benefits:

- deduplication,
- immutable content,
- reproducible package composition,
- shared data between base/update/DLC/mod layers,
- safe rollback,
- cache reuse.

SQLite may be used for indexes, references, metadata, dependency graphs, and build state.

---

## 12. Package resolver

The game is never destructively patched.

Every layer is independently compiled and removable.

```text
BASE
 ↓
UPDATE
 ↓
DLC
 ↓
SWITCH MODS
 ↓
NATIVE MODS
 ↓
RUNTIME PATCHES
```

The resolver creates the effective game view.

Conflicts must be detectable before runtime where possible.

---

## 13. Standalone runtime

A compiled game may pin a specific runtime/profile.

```text
Game Package
├── runtime version
├── Horizon profile
├── compiler version
├── GPU compatibility data
└── package dependencies
```

A standalone distribution can therefore preserve the compatibility environment known to work for that specific title without mutating the original source content.

---

## 14. Platform architecture

The main project is a platform-independent library.

```text
Core
├── Compiler
├── Runtime
├── Resolver
├── CAS
├── IR
└── APIs
     │
     ├── Linux backend
     ├── Windows backend
     └── macOS backend
```

OS-specific behavior must be isolated behind backend traits/interfaces.

---

## 15. Frontend

The initial GUI is intentionally minimal and built with iced.

Primary operations:

- import game,
- compile,
- manage updates/DLC,
- enable/disable mods,
- view compatibility/runtime profile,
- launch,
- inspect build errors,
- manage caches.

Advanced tooling should remain accessible through a CLI.

---

## 16. Design rule

Every optimization is optional.

Correctness must always have a fallback path.

```text
AOT failed?
→ JIT

shader variant unknown?
→ runtime compile

resource unknown?
→ opaque storage

GPU optimization unsafe?
→ conservative translation

media hardware unavailable?
→ software fallback
```

This rule is central to making the project incrementally implementable.
