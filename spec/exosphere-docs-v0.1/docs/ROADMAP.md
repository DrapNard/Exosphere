# Roadmap

## Phase 0 — Specification

- [x] High-level architecture
- [x] CPU strategy
- [x] GPU strategy
- [x] Media abstraction
- [x] Horizon strategy
- [x] resource strategy
- [x] package/layer model
- [x] modding model
- [x] license direction
- [x] formal schema definitions — v0.1 frozen with DrapNard
- [x] upstream provenance policy/schema — v0.1 frozen; per-project audits continue as code is imported

---

## Phase 1 — Repository bootstrap ✅ baseline complete

- [ ] Rust workspace
- [ ] C/C++ integration layer
- [ ] core logging/error model
- [ ] BLAKE3 CAS
- [ ] SQLite index
- [ ] TOML manifests
- [ ] CLI
- [ ] minimal iced frontend shell

---

## Phase 2 — Ingest ✅ structural baseline complete

- [ ] NRO parser
- [ ] NSO parser
- [ ] ExeFS
- [ ] RomFS
- [ ] NCA
- [ ] NSP/PFS
- [ ] XCI
- [ ] update layering
- [ ] DLC layering
- [ ] key-provider interface
- [ ] integrity verification

---

## Phase 3 — CPU prototype ✅ baseline pipeline complete

- [ ] ARM64 decoder
- [ ] CPU IR v1
- [ ] CFG reconstruction
- [ ] LLVM lowering
- [ ] Cranelift lowering
- [ ] native function cache
- [ ] relocation model
- [ ] dynamic fallback
- [ ] profile collection

Target:

```text
NRO → CPU IR → native host code
```

---

## Phase 4 — Minimal Horizon runtime

- [ ] process/thread model
- [ ] virtual memory
- [ ] handles
- [ ] SVC
- [ ] basic IPC
- [ ] FS
- [ ] HID
- [ ] time
- [ ] minimal display path

Target:

```text
simple homebrew reaches its own main loop
```

---

## Phase 5 — GPU prototype

- [ ] GPU IR v1
- [ ] basic Maxwell decode
- [ ] shader IR
- [ ] SPIR-V output
- [ ] Vulkan backend
- [ ] resource bindings
- [ ] synchronization
- [ ] basic MME fallback

Target:

```text
unmodified Switch homebrew → native runtime → Vulkan triangle
```

---

## Phase 6 — Package system

- [ ] package format v1
- [ ] mmap loader
- [ ] CAS integration
- [ ] base package
- [ ] update package
- [ ] DLC package
- [ ] mod package
- [ ] dependency resolver
- [ ] conflict detection
- [ ] rollback

---

## Phase 7 — Resource system

- [ ] Resource IR v1
- [ ] VFS
- [ ] OpaqueResource
- [ ] resource packages
- [ ] development directory overlay
- [ ] file watching
- [ ] hot reload prototype

---

## Phase 8 — Media

- [ ] Media IR v1
- [ ] FFmpeg backend
- [ ] decode path
- [ ] image path
- [ ] VIC-style conversion/composition
- [ ] platform hardware acceleration
- [ ] software fallback

---

## Phase 9 — Advanced GPU

- [ ] compute
- [ ] copy/DMA
- [ ] 2D
- [ ] complete MME handling
- [ ] pipeline graph
- [ ] variant generation
- [ ] variant stripping
- [ ] portable shader cache
- [ ] local PSO cache
- [ ] PGO

---

## Phase 10 — Platforms

### Linux x86-64
- [ ] Vulkan
- [ ] KVM backend
- [ ] Linux media backend
- [ ] Linux audio/input

### macOS ARM64
- [ ] ARM64 host path
- [ ] Metal
- [ ] Apple virtualization backend
- [ ] VideoToolbox
- [ ] CoreAudio/input

### Windows
- [ ] Vulkan
- [ ] Windows virtualization backend
- [ ] Windows media APIs
- [ ] WASAPI/input

---

## Long-term goals

- commercial title compatibility,
- standalone per-game runtime profiles,
- native mod SDK,
- shared universal shader cache,
- profile-guided rebuilding,
- reproducible builds,
- automated compatibility testing,
- additional host backends without modifying core IRs.
