<h1 align="center"><a href="https://github.com/DrapNard/Exosphere">Exosphere</a></h1>

<p align="center">
  A Nintendo Switch compatibility runtime bridging static recompilation and emulation.<br>
  Built with Linux today. Windows and macOS are planned.
</p>

<p align="center">
  <a href="https://github.com/DrapNard/Exosphere/actions/workflows/linux.yml"><img src="https://github.com/DrapNard/Exosphere/actions/workflows/linux.yml/badge.svg" alt="Linux CI"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/language-Rust-orange?logo=rust" alt="Written in Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue" alt="GPL-3.0-or-later"></a>
  <img src="https://img.shields.io/badge/status-experimental-yellow" alt="Experimental">
</p>

<p align="center">
  <a href="docs/LINUX_GAME_ROADMAP.md">Linux roadmap</a> ·
  <a href="spec/exosphere-docs-v0.1/docs/ARCHITECTURE.md">Architecture</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

## About

Exosphere is an experimental Rust project that sits between a static recompiler and an emulator. Its goal is to move as much work as possible into offline analysis and ahead-of-time compilation, while providing the runtime compatibility needed by the original game code.

Recompilation handles code that can be translated with established semantics. Runtime services and emulation handle the surrounding system behavior, with JIT/interpreter paths available where static compilation is insufficient. This is a hybrid architecture, not a claim that whole games already run as standalone native binaries.

**Exosphere is not yet ready to play games.** Linux is the current implementation and validation target; Windows and macOS support are planned, not currently verified.

## How it fits together

- **Analyze and recompile:** load game content, lift ARM64 code into intermediate representations and compile supported paths through LLVM/Cranelift.
- **Provide compatibility:** implement Horizon services, memory, files and synchronization around the game code.
- **Translate graphics:** process Tegra/Maxwell commands and shaders for a host Vulkan backend.
- **Retain runtime fallbacks:** use dynamic execution or interpretation where needed, keeping unsupported behavior explicit rather than guessing it.

The offline pipeline and runtime are complementary parts of the design. Complete game recompilation and end-to-end compatibility remain development goals.

## Current focus

The first milestone is to boot and stabilize **Snipperclips Plus**, followed by **Super Mario Odyssey**.

Content loading and bounded ARM64 execution through the reference interpreter are working for development diagnostics. Compiler paths cover supported subsets; full-game AOT execution is not verified. System services and GPU integration are still being expanded. Controlled Vulkan tests do **not** establish commercial-game compatibility: no Snipperclips frame, menu or gameplay has been verified yet.

See the [Linux roadmap](docs/LINUX_GAME_ROADMAP.md) for detailed progress and remaining work.

## Getting started

On Linux, install a recent stable Rust toolchain and a system compiler/linker. Native graphics tests also require a compatible Vulkan driver.

```bash
git clone https://github.com/DrapNard/Exosphere.git
cd Exosphere
cargo build -p exosphere-cli
cargo run -p exosphere-cli -- doctor
cargo run -p exosphere-cli -- --help
```

The CLI currently provides inspection and development tools. The graphical interface is an early shell, not a finished game launcher.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Keep code clear, document non-obvious decisions and add regression tests. Native opt-in tests and real-game validation remain separate from the unit-test suite.

## Content policy

No games, firmware, decryption keys or proprietary Nintendo SDK content are distributed with Exosphere. Supply your own local resources in accordance with applicable law.

`LocalDevStorage/` is excluded from Git and must remain private. Do not upload dumps or keys in commits, issues or diagnostic logs. See [source policy](legal/SOURCES.md).

---

Maintained by [DrapNard](https://github.com/DrapNard). Licensed under [GPL-3.0-or-later](LICENSE).
