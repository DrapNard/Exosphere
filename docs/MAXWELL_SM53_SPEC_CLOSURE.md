# Maxwell / SM53 specification closure

This document defines what “Maxwell spec complete” means for Exosphere.

The frontend is considered specification-complete when every **SM53/GM20B-relevant** instruction
form that Exosphere recognizes is represented losslessly in canonical IR, including modifiers and
control metadata. This is intentionally separate from backend executability: SPIR-V/Vulkan may
still reject an operation that the Maxwell frontend represents exactly.

## Closed semantic families

- register, immediate and constant-buffer operands
- integer and FP32 ALU, LOP and LOP3 metadata
- IMAD, XMAD and FFMA controls
- BFE/BFI/SHF/PRMT/POPC/FLO rich bitfield representation
- I2I/I2F/F2I/F2F rich conversion representation, retaining raw modifier bits
- ISETP/FSETP and guarded instructions
- FP64 DADD/DMUL/DFMA/DSETP representation
- direct and indirect control-flow preservation
- SSY/PBK/PCNT/SYNC reconvergence metadata
- global/local/shared loads and stores
- ATOM/ATOMS/CAS/RED representation
- BAR/MEMBAR/DEPBAR
- SULD/SUST/SURED/SUATOM surface/image representation
- TEX/TLD/TLD4/TXD/TXQ rich texture representation
- IPA interpolation, SHFL and VOTE subgroup representation
- special/system register reads

## Deliberate exclusions

`HADD2`, `HMUL2`, `HFMA2` and other entries tagged SM60 by the public ISA table are Pascal-era
forms and are not part of the Switch SM53 closure target.

## Remaining validation, not specification work

1. Compile and run the Rust workspace with a real Rust toolchain.
2. Differentially compare disassembly/semantic fields against `nvdisasm -b SM53`/envytools where
   legally and practically available.
3. Feed real Switch shader captures through `lowering_coverage`.
4. Implement backend lowering/capability checks for rich surface, subgroup, indirect-flow and
   reconvergence operations.
5. Treat any real shader that reaches `UnknownEncoding` as a new compatibility bug and add it to
   the regression corpus before extending the decoder.

The canonical coverage invariant is: an instruction is `Ready` only when `lower_semantic()` returns
`lossless=true`. Recognition alone never counts as semantic support.

## MME closure

The macro interpreter now models the documented one-instruction delay for normal branches and the
`exit` modifier. A taken `annul` branch skips the delay slot, while `exit+branch` exits only on the
not-taken path. A branch encountered in a delay slot is rejected. Method reads first consult methods
emitted earlier by the same macro invocation, so read-after-write macro behavior is preserved before
falling back to the PGRAPH snapshot supplied by the caller.
