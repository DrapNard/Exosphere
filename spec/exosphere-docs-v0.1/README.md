# Exosphere — Project Documentation v0.1

> Working title. This project is an experimental open-source compatibility, compilation, and runtime layer for Nintendo Switch software.


## Project identity

- **Author / maintainer:** DrapNard
- **Documentation version:** v0.1
- **Architecture status:** frozen for prototype implementation

## Status

**Architecture freeze: v0.1**

The project is currently in the specification and prototyping phase.

The goal is not to build a traditional emulator around permanent runtime translation. Instead, the project aims to move as much work as possible to an offline compilation phase, then run the resulting game through a lightweight Horizon-compatible runtime with dynamic fallbacks where static compilation cannot safely resolve behavior.

## Core idea

```text
Switch content
    ↓
Ingest / verify / decrypt / extract
    ↓
Universal Game IR
    ├── CPU IR
    ├── GPU IR
    ├── Media IR
    └── Resource IR
    ↓
Immutable compiled packages
    ├── Base
    ├── Updates
    ├── DLC
    ├── Switch mods
    └── Native mods
    ↓
Resolver
    ↓
Host-specific code and assets
    ├── x86-64 / ARM64
    ├── Vulkan / Metal / OpenGL compatibility
    ├── native media backends
    └── Horizon compatibility runtime
    ↓
Play
```

## Main principles

- Rust + C++ core.
- GPL-3.0-or-later.
- From-scratch architecture with selective code reuse where licenses allow it.
- Offline compilation first, dynamic translation as fallback.
- Internal semantic IRs owned by the project.
- LLVM for AOT compilation.
- Cranelift for fast JIT fallback.
- Universal GPU IR with SPIR-V as an important backend/intermediate format.
- Vulkan primary renderer.
- Metal native backend.
- OpenGL compatibility backend.
- Universal Media IR with FFmpeg and native platform backends.
- Immutable layered packages.
- BLAKE3 content-addressed storage.
- TOML manifests.
- Stable C ABI for native extensions; ergonomic Rust/C++ SDKs on top.
- Minimal iced-based GUI.
- Multi-hypervisor abstraction.
- Modding is a first-class design requirement.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [IR Design](docs/IR_DESIGN.md)
- [Formal Schemas](docs/FORMAL_SCHEMAS.md)
- [Package Format](docs/PACKAGE_FORMAT.md)
- [Resources](docs/RESOURCES.md)
- [Modding](docs/MODDING.md)
- [Legal and Upstreams](docs/LEGAL_UPSTREAMS.md)
- [Upstream Policy](docs/UPSTREAM_POLICY.md)
- [Roadmap](docs/ROADMAP.md)

## Non-goals for v0.1

- Perfect commercial game compatibility.
- Reimplementing every Horizon service before the first prototype.
- Eliminating every runtime fallback.
- Building a feature-rich launcher.
- Binding the project permanently to one emulator, one rendering API, or one host OS.

## First milestone

The first real target is deliberately small:

```text
simple homebrew/NRO
    ↓
ingest
    ↓
CPU IR
    ↓
AOT host code
    ↓
minimal Horizon runtime
    ↓
GPU IR
    ↓
Vulkan triangle
```

If that works without relying on a traditional full CPU emulation path, the central architecture is validated.
