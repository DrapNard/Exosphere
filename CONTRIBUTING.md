# Contributing

## Code formatting

Rust code must pass:

```bash
cargo fmt --all -- --check
```

Public Rust APIs use `///` documentation. Crates/modules use `//!` documentation. Comments should
explain ownership, safety, ABI lifetime, semantic assumptions, and non-obvious hardware behavior —
not restate obvious code.

Public C/C++ ABI declarations use Doxygen-compatible comments (`/** ... */`, `@param`, `@return`)
so IDEs can expose documentation in completion/tooltips.

## File boundaries

Do not place unrelated subsystems into one file. Prefer one concern per module: ABI types, loader,
KVM backend, device ABI, package parser, resolver, etc.

## ABI changes

Never reorder or resize published ABI fields in-place. Append fields, bump ABI/schema versions where
required, and document migration/compatibility in the version matrix.
