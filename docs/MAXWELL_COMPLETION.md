# Maxwell frontend completion pass

This pass changes the Maxwell frontend from architecture-complete to compatibility-driven.

## Complete/common path
- PFIFO/subchannel/MAXWELL_B routing and MME replay plumbing.
- SM53 bundles, scheduler controls, predicate guards, branch targets and common opcode identity.
- Physical-register CFG/SSA foundation.
- Register/immediate/constant-buffer operand forms.
- Rich canonical IR for FMA/IMAD/XMAD, bitfields/permutation, conversions/modifiers, memory widths, generalized atomics, interpolation, subgroup shuffle and rich texture operations.
- TIC/TSC semantic descriptors and block-linear metadata.
- Coverage reports split `Ready`, `NeedsCanonicalIr`, and `UnknownEncoding`.

## Intentionally remaining long tail
- Dynamic BRX/JMX/CALL target recovery and pathological irreducible SIMT flow.
- Exact reconvergence-stack validation on captured shaders.
- Bindless/indirect texture forms whose handles are computed dynamically.
- Surface/image forms not representable by the currently exposed backend resource model.
- Rare SM53 opcodes not observed/validated against a real Switch shader corpus.

## Exit criterion
A title/corpus is Maxwell-complete when canonical semantic coverage reports zero `NeedsCanonicalIr` and zero `UnknownEncoding` for all discovered shaders, and differential shader tests agree with a reference disassembler/interpreter for the exercised forms.
