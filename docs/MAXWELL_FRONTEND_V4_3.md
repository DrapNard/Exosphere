# Maxwell frontend v4.3 — SSA semantic bridge

This pass turns the v4.2 physical-register CFG into a source-resolved SSA representation.

## Implemented
- Stable SSA definition ids for physical GPR writes.
- Fixed-point reaching-definition propagation across CFG backedges.
- Stable phi ids per `(block, register)` to converge loops.
- SSA source operands recorded for every lowered instruction.
- Canonical Shader IR emission for safe register forms of MOV, IADD, IMUL, FADD, FMUL, SHL, SHR and LOP.
- Canonical labels, phi nodes, unconditional branches, EXIT/RET and KILL.
- Guarded branches remain opaque until predicate definitions are decoded; this prevents a false conditional value.
- MME active invocation execution/replay API with real PGRAPH method reads and bounded method emissions.

## Deliberately still opaque
Immediate/constant-buffer instruction forms, predicate-producing instructions, indirect control flow, memory addressing, atomics and texture operands are not guessed. Their opcode families remain classified while their canonical semantics stay opaque until their selector/operand bits are validated.

## Next compatibility tranche
1. Predicate-producing opcodes and predicate SSA.
2. Immediate and constant-buffer operand selectors.
3. SIMT reconvergence (SSY/SYNC/PBK/BRK/CONT).
4. LD/ST/ATOM addressing and widths.
5. TEX/TLD/TLD4/TXD/TXQ operands plus TIC/TSC semantics.
