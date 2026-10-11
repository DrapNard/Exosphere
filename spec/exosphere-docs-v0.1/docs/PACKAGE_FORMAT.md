# Package Format

## 1. Goals

The package format is custom and optimized for runtime use.

Requirements:

- immutable,
- mmap-friendly,
- independently removable,
- hash-verifiable,
- layerable,
- suitable for large assets,
- compatible with CAS references,
- fast startup,
- deterministic.

---

## 2. Package families

```text
*.base.pkg
*.update.pkg
*.dlc.pkg
*.mod.pkg
*.native-mod.pkg
*.resource.pkg
```

The exact extension names are provisional.

---

## 3. Conceptual layout

```text
HEADER
MANIFEST
OBJECT TABLE
DEPENDENCY TABLE
CPU SECTION
GPU SECTION
MEDIA SECTION
RESOURCE INDEX
SYMBOL TABLE
CAS REFERENCES
OPTIONAL EMBEDDED OBJECTS
SIGNATURE / INTEGRITY DATA
```

Sections should be aligned for direct mapping where practical.

---

## 4. Manifest

TOML is used for human-readable manifests.

Example:

```toml
format_version = 1
package_type = "mod"
id = "example.mod.60fps"
version = "1.0.0"

[game]
title_id = "example"
requires_base = ">=1.0.0"
requires_update = ">=1.2.0"

[runtime]
requires = ">=0.1"

[priority]
value = 200
```

Binary manifests may be generated internally for fast runtime loading, but TOML remains the authoring format.

---

## 5. Immutable layering

Packages never modify parent packages.

```text
base
 ├── update
 │    ├── dlc
 │    └── mod
 └── alternate mod
```

The resolver determines the effective object for each logical path/symbol/pipeline.

---

## 6. Content addressing

Objects are identified by BLAKE3.

```text
logical resource
      ↓
object reference
      ↓
BLAKE3 hash
      ↓
CAS object
```

Identical objects across base/update/DLC/mods are stored once.

---

## 7. Runtime mapping

Large native sections should support direct mapping where safe.

```text
open package
 ↓
verify metadata
 ↓
mmap section
 ↓
apply runtime relocations
 ↓
execute/use
```

Compression is allowed on data that does not benefit from direct mapping.
