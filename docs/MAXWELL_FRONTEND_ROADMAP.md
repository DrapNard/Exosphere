# Maxwell frontend roadmap — v4.1

## Command/state frontend
- [x] PFIFO/GPFIFO parsing and subchannel routing
- [x] MAXWELL_B state mirror with lossless unknown methods
- [x] compute/DMA/2D/inline engine routing
- [x] MME upload/start-address tracking
- [ ] automatic macro replay into MAXWELL_B method stream

## SM53 structural decoder
- [x] graphics header and compute entry handling
- [x] 32-byte bundles / scheduler-word separation
- [x] scheduler control decode: stall, yield, dependency barriers, wait mask, reuse mask
- [x] common GPR fields (R0/R8/R20/R39 positions)
- [x] predicate guard P0..P7 and negation
- [x] signed PC-relative BTARG decode (PC + 8)
- [x] broad opcode-family classification
- [x] exact opcode identity for common control/ALU/global-memory/texture instructions
- [ ] instruction-form-specific immediate / constant-buffer decode
- [ ] modifier decode per opcode (neg/abs/sat/ftz/cc/type)

## SM53 semantic lifting
- [x] safe zero padding NOP
- [ ] physical-register dataflow -> SSA
- [ ] predicate dataflow and guarded operations
- [ ] integer ALU
- [ ] FP ALU
- [ ] conversions/comparisons
- [ ] CFG reconstruction (BRA/JMP/CAL/RET/SSY/SYNC/PBK/PCNT)
- [ ] local/shared/global/constant memory
- [ ] atomics/reductions
- [ ] textures / interpolation
- [ ] barriers and special registers

## MME
- [x] add/adc/sub/sbb/xor/or/and/andn/nand
- [x] immediate add and parameter fetch
- [x] extrinsrt
- [x] both extrshl forms
- [x] method reads through host callback
- [x] method address/increment and method emission
- [x] branch zero/non-zero/unconditional decode
- [ ] exact delay-slot/annul execution
- [ ] automatic macro invocation/replay in MaxwellFrontEnd

## TIC/TSC
- [x] raw descriptor reads and handle split
- [x] initial sampler decode
- [ ] complete TIC format/dimension/address decode
- [ ] exact mip/layer/block-linear metadata
- [ ] complete TSC filtering/wrap/LOD/anisotropy/border decode

## Exit criterion
The Maxwell frontend is considered compatibility-complete when real SM53 programs can be lifted without opaque instructions for the supported Switch workload corpus, MME macro launches replay automatically into MAXWELL_B state, and TIC/TSC descriptors produce complete canonical resources without guessed fields.

## v4.3 semantic bridge
- [x] source-resolved SSA instructions
- [x] fixed-point loop/backedge reaching definitions
- [x] stable phi nodes
- [x] safe register-form ALU -> canonical Shader IR
- [x] canonical labels / unconditional branches / exit / kill
- [x] bounded MME invocation execution and method replay API
- [ ] predicate-producing opcodes + predicate SSA
- [ ] immediate / constant-buffer selectors
- [ ] SIMT reconvergence stack
- [ ] memory / atomics semantic operands
- [ ] texture semantic operands + complete TIC/TSC

## v4.5 — coverage-driven long-tail pass

### SM53 semantic decoder
- [x] distinguish decoded/IR-gap/unknown encodings
- [x] typed immediate/register/cbuf/memory/predicate/texture/special-register operands
- [x] MOV32I / IADD32I / IMUL32I structural semantics
- [x] FADD32I / FMUL32I / LOP32I structural decode with modifier fallback
- [x] XMAD / IMAD / FFMA recognition
- [x] BFE / BFI / SHF / PRMT / POPC / FLO recognition
- [x] I2I / I2F / F2I / F2F recognition
- [x] ISETP / FSETP recognition
- [x] global/local/shared memory operand shapes
- [x] atomic operand shape baseline
- [x] TEX/TLD/TLD4/TXD/TXQ handle/register baseline
- [x] special-register operand baseline
- [x] per-opcode / operand-form / fallback-reason coverage report
- [x] fix unreachable interpolation classifier path
- [x] classify SHFL separately from texture/interpolation
- [ ] preserve every ALU modifier in canonical Shader IR
- [ ] generalized atomic canonical operation (min/max/cas/exchange/etc.)
- [ ] canonical memory access width/sign/cache descriptor
- [ ] richer texture operation descriptor for bindless/LOD/offset modes
- [ ] interpolation and subgroup canonical operations
- [ ] capture corpus regression suite

### Definition of semantic closure
The SM53 frontend is semantically closed for a workload when `UnknownEncoding == 0` and
`NeedsCanonicalIr == 0` for its shader corpus.  This replaces approximate global percentages with
a reproducible, per-workload compatibility criterion.

## v4.7 unified semantic compiler
- [x] one semantic decoder shared by coverage and compilation
- [x] post-lowering coverage (`Ready` means lossless canonical emission)
- [x] common GPR / immediate / constant-buffer ALU forms
- [x] direct + indirect control-flow preservation
- [x] CAL/RET preserved separately from EXIT
- [x] reconvergence metadata operations
- [x] BAR / MEMBAR / DEPBAR canonical operations
- [x] global/local/shared rich memory lowering
- [x] common atomic lowering
- [x] TEX/TLD/TLD4/TXD/TXQ rich lowering baseline
- [x] IPA / SHFL / SYS canonical lowering
- [x] synthetic decoder-to-lowerer invariants
- [ ] exact predicate destination/combine SSA for ISETP/FSETP
- [ ] lossless modifier decode for XMAD/IMAD/FFMA/conversions
- [ ] surface/image long tail
- [ ] real captured shader differential corpus
- [ ] SPIR-V structurisation for irreducible/indirect flow

## v4.8 semantic closure

- [x] Preserve instruction predicate guards in canonical IR (`Predicated`).
- [x] Canonical predicate-set representation for ISETP/FSETP register forms.
- [x] Rich ternary representation for common IMAD/XMAD/FFMA register forms.
- [x] Correct XMAD GM107 opcode family (0x5b00 family; do not alias DFMA 0x5b70).
- [x] Fix legacy CFG borrow hazard and remove unreachable blocks.
- [x] Fixed-point reaching-definition SSA convergence.
- [ ] Predicate value SSA / predicate phi integration into canonical value numbering.
- [ ] BFE/BFI/SHF/PRMT/POPC/FLO and uncommon conversion modifiers.
- [ ] CAS/safeadd/u128 and surface/image long-tail.
- [ ] Real Switch shader corpus + differential validation.

## v4.9 — SM53 / Maxwell spec closure

The frontend specification target is now considered **closed at the representation layer**.
All currently validated SM53 families have a lossless canonical representation; unsupported host
execution is reported as a backend capability problem rather than an ISA-decoder problem.

Closed in this pass: predicate-safe lowering, BFE/BFI/SHF/PRMT/POPC/FLO/LOP3, rich conversions,
FP64 arithmetic/predicates, ATOM/CAS/RED, surface load/store/reduce/atomics, VOTE, richer ALU
modifier retention, reconvergence-kind preservation, and MME branch/exit delay-slot semantics.

SM60-only half2 forms are excluded from the Switch SM53 target. Remaining work is validation on a
real shader corpus, SPIR-V structurisation/capability lowering, and compatibility fixes discovered by
that validation; those do not reopen the Maxwell specification phase unless an actual unknown SM53
encoding is observed.
