# Resource System

## 1. Goals

The resource system must optimize runtime performance without making modding painful.

Therefore:

- physical storage uses packages + CAS,
- logical access uses a virtual filesystem,
- mod development may use raw directories,
- runtime packages remain immutable.

---

## 2. Resource flow

```text
RomFS
 ↓
Resource Analyzer
 ↓
Resource IR
 ↓
CAS
 ↓
Resource Packages
 ↓
Virtual Resource FS
```

---

## 3. Physical vs logical layout

Physical:

```text
game/
├── base.pkg
├── resources/
│   ├── boot.rpkg
│   ├── common.rpkg
│   ├── world-01.rpkg
│   └── audio.rpkg
└── cas/
```

Logical:

```text
Game/
├── textures/
├── audio/
├── models/
├── animations/
├── maps/
└── data/
```

The logical hierarchy may be generated even when the source game uses opaque proprietary archives.

---

## 4. Resource grouping

Packages may be grouped by:

- streaming locality,
- dependency graph,
- scene/region,
- boot requirements,
- content type,
- DLC/update ownership.

Grouping by runtime locality is preferred over blindly grouping only by file extension.

---

## 5. Host variants

Resources may keep multiple representations.

Example:

```text
Texture
├── original
├── universal metadata
├── Vulkan-targeted representation
└── Metal-targeted representation
```

Host variants are derived artifacts and can always be regenerated.

---

## 6. Mod development

Developer mode may mount a raw directory above packaged layers.

```text
priority:
1. ./dev-mod/
2. installed mods
3. DLC
4. update
5. base
```

A file watcher can trigger:

- re-import,
- resource compilation,
- hot reload where supported.

Final distribution converts the directory into an immutable mod package.
