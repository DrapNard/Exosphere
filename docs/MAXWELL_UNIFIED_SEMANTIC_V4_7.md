# Maxwell unified semantic pipeline — v4.7

v4.7 removes the historical split between coverage decoding and compilation. `decode_semantic()` is now the single SM53 semantic source of truth and `lower_semantic()` is used by both coverage validation and `lift_semantic()`.

## Pipeline

SM53 bundle -> decode_semantic -> lossless canonical lowering -> ShaderOperation -> SPIR-V/backend

CFG/SSA remains a control-flow/value analysis facility; it no longer owns a competing opcode-semantic table.

## Newly connected canonical paths

- register, 20-bit immediate, 32-bit immediate and constant-buffer common ALU forms
- direct and indirect control-flow representation
- CAL/RET distinction
- SSY/PBK/PCNT/SYNC reconvergence metadata
- BAR/MEMBAR/DEPBAR
- global/local/shared rich memory operations
- generalized atomics where operation is validated
- TEX/TLD/TLD4/TXD/TXQ rich texture operations
- IPA interpolation
- SHFL subgroup operation
- SYS special-register reads

## Coverage invariant

`lowering_coverage()` calls the same decoder and lowerer as compilation. A decoded instruction is counted Ready only when lowering reports `lossless=true`. Synthetic tests enforce that a Ready fixture cannot become `OpaqueMaxwell`.

## Deliberately conservative remainder

The following remain explicit blockers rather than guessed semantics: modifier-heavy XMAD/IMAD/FFMA and conversion variants not fully decoded, exact ISETP/FSETP predicate destinations/combiner SSA, CAS/safeadd/u128 atomic corner cases, packed bindless texture offset payloads, surface/image operations, half/double long tail, and irreducible dynamic control flow during SPIR-V structurisation.
