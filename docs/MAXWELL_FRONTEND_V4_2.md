# Maxwell Frontend v4.2 — Register IR / CFG / SSA foundation

This pass introduces a physical-register Maxwell IR between SM53 decoding and canonical Shader IR.
It builds basic blocks from decoded branch targets, explicit CFG successor edges, versioned physical-register writes, and conservative phi planning at merge points. This avoids the incorrect shortcut of treating Maxwell register numbers as canonical SSA value IDs.

Safe register-form MOV/IADD/IMUL/FADD/FMUL/SHL/SHR/LOP instructions are represented in this layer; unsupported operand forms remain opaque until their immediate/constant-buffer selectors are validated.

## Next semantic frontier

- exact immediate and constant-buffer operand forms
- predicate-definition instructions and predicate SSA
- canonical ShaderOperation emission from the SSA plan
- global/shared/local memory operands and atomics
- TEX/TLD/TLD4/TXD/TXQ operand layouts and TIC/TSC semantic descriptors
- complete MME macro replay into Maxwell3dState
