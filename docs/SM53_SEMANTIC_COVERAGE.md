# SM53 semantic coverage

Exosphere now tracks three different states instead of treating every non-lifted Maxwell instruction as the same opaque case:

- **Ready** — the encoding and operand form are validated and can be lowered without discarding semantics.
- **NeedsCanonicalIr** — the Maxwell encoding is understood, but Exosphere's backend-neutral Shader IR cannot yet preserve every modifier or operation.
- **UnknownEncoding** — no validated semantic decoder exists yet.

`maxwell_semantic_coverage()` reports opcode histograms, operand-form histograms and fallback reasons. This makes future work capture-driven: a game shader corpus can identify whether compatibility is blocked by missing ISA knowledge or by a canonical-IR/backend feature.

## v4.5 long-tail baseline

Decoded structurally in this pass: MOV32I, IADD32I, IMUL32I, FADD32I, FMUL32I, LOP32I, XMAD, IMAD, FFMA, BFE, BFI, SHF, PRMT, POPC, FLO, I2I, I2F, F2I, F2F, ISETP, FSETP, common global/local/shared memory forms, atomics, texture families and special-register reads.

The decoder is deliberately conservative: modifiers that cannot yet be represented losslessly remain `NeedsCanonicalIr`; they are not silently ignored.
