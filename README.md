<h1 align="center"><a href="https://github.com/DrapNard/Exosphere">Exosphere</a></h1>

<p align="center">
  An experimental Nintendo Switch emulator written in Rust.<br>
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

Exosphere brings together ARM64 execution, Horizon system services and Tegra/Maxwell graphics through a Vulkan backend. Its architecture explores offline analysis and AOT/JIT compilation alongside a reference interpreter.

The project is in active development, **not yet a playable emulator**. Linux is the current implementation and validation target; Windows and macOS support are planned, not currently verified.

## Current focus

The first milestone is to boot and stabilize **Snipperclips Plus**, followed by **Super Mario Odyssey**.

Content loading and bounded ARM64 execution are working for development diagnostics. System services and GPU integration are still being expanded. Controlled Vulkan tests do **not** establish commercial-game compatibility: no Snipperclips frame, menu or gameplay has been verified yet.

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
