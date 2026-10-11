# Modding

## 1. First-class requirement

Modding is not an afterthought.

The project supports two major mod families:

1. Switch-native mods
2. Native runtime mods

---

## 2. Switch-native mods

Existing RomFS/ExeFS-style modifications are imported into the package system.

```text
Switch mod
 ↓
analyze
 ↓
Portable Mod IR
 ↓
compile
 ↓
.mod.pkg
```

The mod remains independently removable.

---

## 3. Native mods

Native mods target stable project APIs rather than Switch memory addresses whenever possible.

Potential capabilities:

- resource override,
- symbol hook,
- function hook,
- GPU IR modification,
- shader replacement,
- pipeline override,
- runtime service extension,
- debugging/instrumentation.

---

## 4. ABI

The stable binary ABI is C.

```text
Runtime
  ↕ stable C ABI
Rust SDK wrapper
C++ SDK wrapper
```

Rust's unstable native ABI is not exposed as the long-term binary contract.

---

## 5. Symbolic hooks

Preferred:

```text
module = "main"
symbol = "Player::Update"
fingerprint = "..."
```

Avoid when possible:

```text
absolute_address = 0x7100123456
```

Symbol/fingerprint resolution improves resilience across game updates.

---

## 6. Conflict detection

The resolver should detect conflicts such as:

- two mods replacing one resource,
- two mods replacing the same shader,
- incompatible hooks,
- unmet update dependencies,
- incompatible runtime requirements.

The user should see conflicts before launching where possible.
