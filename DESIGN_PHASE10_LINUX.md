# Phase 10 Linux bootstrap

This bootstrap intentionally creates the platform boundaries required by Phase 10 before the full
CPU/GPU/Horizon implementations exist.

## Implemented as architecture/API

- project-wide GPL-3.0-or-later + DrapNard identity
- Rust workspace + C ABI boundary crate
- `rustfmt.toml`
- package/CAS/manifest skeletons
- versioned runtime ABI
- hypervisor backend trait
- dynamic hypervisor backend ABI
- dynamic **device plugin ABI**
- KVM backend crate skeleton
- keyboard/mouse device plugin example
- Linux platform composition crate
- CLI launcher skeleton
- minimal iced UI shell
- IDE-friendly Rustdoc/Doxygen-style public docs

## Device plugin model

Device plugins are not hard-coded into KVM. They are loaded behind a versioned ABI and expose one
or more logical devices to the guest/runtime.

Examples:

```text
Hypervisor Device Plugin
├── keyboard
├── mouse
├── gamepad
├── storage
├── network
├── sensor
├── display adjunct
└── vendor/custom
```

A plugin can receive runtime events and produce guest-facing events. Platform-specific capture
(evdev/libinput on Linux, for example) remains inside the plugin/backend implementation.

## Why plugins belong beside the hypervisor

The hypervisor owns mappings, interrupts and guest-visible devices. Input capture itself belongs to
the platform plugin, but injection into the virtual machine is mediated by the hypervisor/device ABI.
This keeps KVM, WHPX and Apple backends interchangeable.


## Additional Phase-10 boundaries created

```text
canonical CPU IR      → LLVM/Cranelift backends (to implement)
canonical GPU IR      → exosphere-gpu-vulkan
canonical Media IR    → exosphere-media-linux
resource IR           → package/CAS/resolver
Horizon service ABI   → firmware-profiled service implementations
Linux audio           → exosphere-audio-linux (PipeWire target)
Linux input           → dynamic device plugin (evdev/libinput target)
```

This is intentionally a **Phase-10 architecture bootstrap**, not a claim of Switch game
compatibility. The expensive implementations (ARM64 lifting, Horizon services, Maxwell lowering,
Vulkan submission, FFmpeg/native media wiring) now have stable places to land without changing the
public architecture.

## Current implementation progress after bootstrap

The first real ingest/analyzer work is now present:

- NRO0 structural parser with segment and homebrew asset validation.
- NSO0 structural parser with compression/hash flags and modern execute-only/ZBIC flag exposure.
- PFS0 parser suitable as the starting point for ExeFS/NSP-style partition filesystems.
- Bounds-checked little-endian binary reader shared by ingest parsers.
- CLI `inspect` command for NRO/NSO/PFS0.
- Minimal conservative AArch64 decoder for bootstrap analysis (`nop`, `ret`, immediate branches, conditional branches, `svc`).
- Unknown AArch64 instructions remain explicit and are never guessed.

Example:

```bash
cargo run -p exosphere-cli -- inspect homebrew.nro --decode 32
```
