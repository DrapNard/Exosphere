# Legal and Upstream Policy

## 1. Project license

The project's original code is intended to use:

```text
GPL-3.0-or-later
```

This does not remove the need for upstream license review.

---

## 2. Rules for imported code

For every imported or adapted component, record:

- upstream project,
- upstream repository,
- exact commit,
- original path,
- original license,
- copyright notices,
- local modifications.

Suggested metadata:

```toml
[[source]]
project = "example"
commit = "..."
path = "..."
license = "MIT"
local_path = "third_party/example/..."
```

---

## 3. SPDX

New source files should include an SPDX identifier where applicable.

Example:

```text
SPDX-License-Identifier: GPL-3.0-or-later
```

Imported source keeps the appropriate original SPDX/license information.

---

## 4. GPL compatibility warning

GPL licensing does not mean all GPL code can automatically be combined.

In particular:

- GPL-2.0-only requires special compatibility review.
- GPL-2.0-or-later may usually be used under a later GPL version when its license terms permit.
- GPL-3.0 / GPL-3.0-or-later code still requires preserving notices and source obligations.
- permissive code keeps its original notices.

---

## 5. Clean separation

Recommended repository layout:

```text
src/            # original project code
third_party/    # imported compatible code
references/     # documentation/research notes, no copied incompatible code
legal/          # notices and provenance records
```

---

## 6. Proprietary content

The project must not distribute:

- commercial games,
- DLC,
- console keys,
- copyrighted firmware,
- proprietary Nintendo SDK files.

Game ingest/decryption operates on user-provided content and credentials/keys where legally available to the user.

This document is an engineering policy, not legal advice.
