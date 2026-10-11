# Upstream Sources

## Switchbrew — reference-only

Used for public Nintendo Switch / Tegra X1 GPU class, method, shader-layout and command-stream documentation.
Exosphere re-expresses required structures in original Rust code.

## deko3d — reference-only (zlib)

Used as a public reference for Tegra X1 / NVN reverse-engineering results, including QMD layout and low-level
engine concepts. The current snapshot does not copy deko3d source files verbatim.

## envytools — reference-only (MIT)

Used to validate Maxwell/GM107 ISA terminology and decoder coverage strategy. Exosphere currently keeps unknown
shader instructions opaque rather than importing the envydis decoder tables.

## ash

- Repository: https://github.com/ash-rs/ash
- Version: 0.38.0+1.3.281
- License: MIT OR Apache-2.0
- Mode: dependency
- Use: Vulkan 1.3 bindings for Exosphere's general Vulkan backend.
- Architecture: Vulkan types remain confined to `exosphere-gpu-vulkan`; canonical GPU IR stays backend-neutral.

## Eden Emulator Maxwell ISA documentation — reference (GPL-3.0-or-later / inherited notices)
- Repository: https://github.com/eden-emulator/mirror
- Reference: `docs/NvidiaGpu.md`
- Use in Exosphere: opcode-family validation/reference. Exosphere's Rust decoder is independently structured and keeps unverified operand semantics opaque.
