# Formal Schemas v0.1

**Author / maintainer:** DrapNard  
**Status:** Design freeze for prototype implementation  
**Project license:** GPL-3.0-or-later

## 1. CPU IR

The CPU pipeline uses two internal levels.

```text
Switch ARM64
    ↓
Decoded CPU IR
    ↓
Semantic SSA-oriented CPU IR
    ↓
├── LLVM IR → AOT
└── Cranelift IR → JIT fallback
```

The canonical optimization IR is intentionally close to SSA because x86-64 is the first priority host target. The decoded layer preserves exact ARM64 semantics, source addresses, relocations, and debug information. The semantic layer is designed for SSA-based optimization, CFG analysis, constant propagation, dead-code elimination, interprocedural analysis, and lowering to LLVM or Cranelift. LLVM IR and Cranelift IR are backend representations, never the canonical serialized game representation.

## 2. GPU IR

GPU IR is multi-stage.

```text
Switch GPU / NVN / host1x
           ↓
      Decode GPU IR
           ↓
    Semantic GPU IR
      ├── Shader IR
      ├── Pipeline IR
      ├── Resource IR
      ├── Graphics IR
      ├── Compute IR
      ├── Transfer IR
      ├── 2D IR
      ├── Sync IR
      └── MME IR
           ↓
      Backend IR
      ├── SPIR-V
      ├── Metal/MSL path
      └── GLSL compatibility path
```

Switch-specific state must not be discarded until it is proven unnecessary for all supported backends.

## 3. IDs and hashes

Two identities are used deliberately.

### ResourceId

```text
ResourceId =
    BLAKE3(
        normalized_filename
        || content
        || resource_schema_version
    )
```

Renaming a resource changes its `ResourceId`. This is useful for modding, symbolic references, layer conflict detection, and debug tooling.

### ObjectId

The content-addressed store uses a filename-independent identity so identical data still deduplicates:

```text
ObjectId =
    BLAKE3(
        content
        || object_schema_version
    )
```

### RuntimeId

```text
RuntimeId = u64
```

Runtime IDs are compact, temporary handles and are not stable across launches.

## 4. Serialization

v0.1 canonical binary schema:

```text
FlatBuffers
```

A future hybrid may add CBOR for flexible metadata, sparse extension fields, debug annotations, and experimental data.

```text
Package
├── FlatBuffers core schema
└── CBOR extension/debug metadata
```

## 5. Package format

The package format is custom, mmap-friendly, immutable, and section-based.

```text
PackageHeader
├── magic
├── format_version
├── package_type
├── flags
├── build_mode
├── section_count
├── manifest_offset
├── object_table_offset
├── dependency_table_offset
├── source_provenance_offset
└── header_checksum
```

```text
Section
├── type
├── flags
├── compression
├── offset
├── compressed_size
├── raw_size
├── alignment
├── schema_version
└── BLAKE3
```

Initial section types:

```text
CPU_CODE
CPU_IR
GPU_IR
SHADER_IR
SPIRV
MEDIA_IR
RESOURCE_INDEX
RESOURCE_DATA
SYMBOLS
RELOCATIONS
DEPENDENCIES
PROVENANCE
DEBUG
```

Sections are independently compressible.

## 6. Compression

Default compression is Zstandard.

```text
runtime package data → low/medium zstd
cold archive/cache   → high zstd
mmap native code     → none
already-compressed resources → none when recompression is useless
```

## 7. Resource IR

```text
Resource
├── ResourceId
├── ObjectId
├── kind
├── normalized_path
├── original_format
├── source_layer
├── dependencies[]
├── variants[]
├── original_representation?
└── typed_payload | opaque_payload
```

Initial kinds:

```text
Texture
Audio
Model
Animation
StructuredData
Localization
ScriptData
Archive
Opaque
```

Original preservation is configurable:

```text
PreserveAlways
PreserveIfNeeded
DiscardAfterVerifiedConversion
```

Defaults: development/debug builds use `PreserveAlways`; normal builds use `PreserveIfNeeded`; explicitly optimized standalone builds may use `DiscardAfterVerifiedConversion`.

## 8. Media IR

Media IR uses job-style operations:

```text
DecodeVideoJob
EncodeVideoJob
DecodeImageJob
CompositeJob
ScaleJob
CropJob
ColorConvertJob
BlitJob
PresentSurfaceJob
```

Only project-owned handles and structures are canonical. FFmpeg, Vulkan Video, VideoToolbox, D3D Video and similar APIs remain backend details.

## 9. Runtime ABI

The stable binary ABI is C. Rules: `extern "C"`, explicit integer widths, versioned structures, every extensible public structure contains `abi_version` and `struct_size`, no Rust-native ABI types, no STL containers, explicit ownership/allocator rules, and ABI-stable result/error types. Rust and C++ SDKs are wrappers over this ABI.

## 10. Version matrix

Versions are tracked independently.

| Component | Version | Debug version | Runtime compatibility role |
|---|---:|---:|---|
| Package format | 1 | 1-dbg | Required |
| CPU Decoded IR | 1 | 1-dbg | If embedded |
| CPU Semantic IR | 1 | 1-dbg | If embedded |
| GPU Decode IR | 1 | 1-dbg | If embedded |
| GPU Semantic IR | 1 | 1-dbg | If embedded |
| Shader IR | 1 | 1-dbg | If embedded |
| MME IR | 1 | 1-dbg | If embedded |
| Media IR | 1 | 1-dbg | If embedded |
| Resource IR | 1 | 1-dbg | Required |
| Symbol DB | 1 | 1-dbg | Mod/debug features |
| Runtime ABI | 1 | 1-dbg | Required |
| Mod ABI | 1 | 1-dbg | Native mods |
| Horizon profile DB | 1 | 1-dbg | Required |
| Provenance schema | 1 | 1-dbg | Tooling only |

Debug variants may add source addresses, original encodings, names, analysis traces, validation data, backend diagnostics, and provenance links.

## 11. Primitive encoding rules

```text
endianness      = little-endian
integer widths  = explicit
offsets         = u64
sizes           = u64
alignment       = power of two
strings         = UTF-8 unless source metadata says otherwise
hash            = fixed-size BLAKE3 digest
```

## 12. Symbol and Language Entity Database

The Symbol DB is generalized to represent language-level entities, not only functions.

```text
LanguageEntity
├── id
├── name?
├── mangled_name?
├── module
├── namespace?
├── original_address?
├── size?
├── fingerprint
├── visibility
├── linkage
├── attributes[]
├── source_type
└── debug_metadata?
```

Entity kinds:

```text
Function
Method
Constructor
Destructor
Class
Struct
Union
Enum
EnumValue
Interface
Trait
Namespace
Module
GlobalVariable
StaticVariable
LocalVariable
Field
Property
Constant
TypeAlias
GenericType
Template
Parameter
ReturnType
Callback
VTable
RTTI
Unknown
```

Functions and methods additionally record parameters, parameter types, return type/result, calling convention, variadic state, exception behavior when known, original address, fingerprint, and references. Class-like entities may record fields, methods, base types, interfaces, vtables, alignment, size, and layout metadata. Variables may record type, storage, mutability, address, initial value, and references. Partial information is valid; unknown information must remain explicitly unknown instead of being guessed.

## 13. Provenance records

Machine-readable provenance uses `legal/SOURCES.toml`. Each record includes project, repository, commit, upstream path, local path, upstream license, copyright, mode, modifications, and notes.

Modes:

```text
copied
adapted
ported
reference-only
clean-room-reimplementation
generated
```

## 14. Human-readable provenance

A companion `legal/SOURCES.md` explains what each upstream is used for, why it was imported or referenced, what changed, licensing constraints, and whether the local implementation is copied, adapted, ported, or independently reimplemented.

## 15. Source file provenance header

Imported, adapted, or ported files carry a compact provenance header.

```text
// SPDX-License-Identifier: MIT
//
// Upstream-Project: ExampleProject
// Upstream-Repository: https://example.invalid/repo
// Upstream-Commit: abcdef123456
// Upstream-Path: src/example.cpp
// Import-Mode: adapted
//
// Local modifications are documented in legal/SOURCES.toml.
```

Original project files use:

```text
// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard
```

Detailed explanations remain in `SOURCES.toml` and `SOURCES.md`.

## 16. Upstream policy

Direct import is allowed only after license/provenance review. Typical compatible source families include MIT, BSD, ISC, zlib, Apache-2.0, and GPL code compatible with the project's GPL-3.0-or-later licensing. Reference-only treatment is required for incompatible licenses, proprietary SDK material, leaked/confidential source, uncertain provenance, or code with unclear redistribution rights.

## 17. Upstream repository layout

```text
third_party/
├── imported/
├── patched/
└── generated/

legal/
├── SOURCES.toml
├── SOURCES.md
├── NOTICE
└── licenses/

research/
├── documentation/
├── behavior-notes/
├── compatibility-tests/
└── comparisons/
```
