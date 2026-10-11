# Maxwell semantic closure v4.8

This pass is correctness-first. It does not claim full SM53 compatibility.

## Closed in this pass

- Predicate guards are preserved canonically with `ShaderOperation::Predicated`; guarded ALU, memory, texture, atomic and control operations can no longer silently become unconditional.
- `ISETP` / `FSETP` register forms lower to an explicit `PredicateSet` operation carrying both predicate destinations, compare domain/op, boolean combine mode and input predicate.
- Register-form `IMAD`, `XMAD` and `FFMA` lower to `TernaryRich`, preserving the high-value modifier bits instead of collapsing to plain multiply/add.
- The XMAD opcode family is corrected to GM107 `0x5b00...`; the previous `0x5b70` match collided with DFMA.
- Legacy register CFG no longer moves `RegOp` out of a shared borrow.
- CFG construction removes unreachable blocks before SSA.
- Reaching-definition SSA iterates to a fixed point instead of treating `4 * block_count` as normal convergence.
- Invalid semantic-lowering branches now return the closure's fallback type instead of attempting to return `CanonicalLowering` from an `Option<bool>` closure.

## Still conservative

- Predicate *value* SSA across merge points is not yet used by canonical lowering; physical predicate identity is preserved losslessly first.
- BFE/BFI/SHF/PRMT/POPC/FLO and uncommon conversion modifiers remain fallback.
- CAS/safeadd/u128 atomic long-tail remains fallback where exact tuple/type semantics are not represented.
- Surface/image instructions and uncommon half/double operations remain fallback.
- Indirect/irreducible control flow is preserved but still requires SPIR-V structuring policy.
- Real Switch shader corpus and differential testing are still required before calling the frontend compatibility-complete.

Public encoding reference: envytools `envydis/gm107.c` (reference-only; implementation is independent).
