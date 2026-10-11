# Upstream Provenance Policy v0.1

**Author / maintainer:** DrapNard  
**Project license:** GPL-3.0-or-later

## Purpose

The project intentionally reuses compatible open-source work and public research where doing so avoids unnecessary duplication.

Every non-original component must answer:

1. Where did it come from?
2. Which exact revision did it come from?
3. Under which license?
4. Was it copied, adapted, ported, generated, or only referenced?
5. What changed locally?

## Records

- `legal/SOURCES.toml` — machine-readable authoritative provenance index.
- `legal/SOURCES.md` — human-readable explanation.
- file-local provenance header — durable origin metadata attached to imported/adapted/ported source.

## Import modes

- `copied`
- `adapted`
- `ported`
- `reference-only`
- `clean-room-reimplementation`
- `generated`

## Architectural independence

Imported code must be wrapped behind project-owned interfaces whenever practical.

```text
our interface
    ↑
adapter
    ↑
imported component
```

Avoid spreading an upstream emulator's internal object model through the core architecture.

## Merge requirements

No upstream-derived code should be merged without provenance entry, license confirmation, preserved notices, selected import mode, local wrapper/ownership decision, and architecture impact review.
