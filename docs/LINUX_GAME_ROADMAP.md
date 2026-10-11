# Linux game execution roadmap

Target: complete the Linux roadmap, stabilize Snipperclips Plus
(`01008E20047DC000`), then validate and stabilize Super Mario Odyssey
(`0100000000010000`). Representation coverage is not execution coverage.

## Current execution gates

- Snipperclips Plus: not booted to a verified frame/menu/gameplay. Last actual
  probe: 199,328,010 instructions, PC 0x7102649744, unsupported ZCullGetCtxSize.
- Super Mario Odyssey: stabilization remains deferred until Snipperclips passes.
- GPU evidence below consists of controlled fixtures, not game execution.
- Firmware 22.1.0 NV executable partition now decrypts and verifies with the
  user's updated keys; ZCull context image size is still not established.
- Unmodified homebrew main loop: unverified; no NRO/homebrew binary was found
  among the supplied local game resources during the latest inventory.
- Historical baseline and journal entries below retain their original scope;
  passing a primitive's tests does not complete its broader milestone checklist.

## Verified baseline — 2026-10-05

- `cargo check --workspace --all-targets --offline`: passes, with warnings.
- CPU IR tests: 5 pass; GPU IR tests: 71 pass; ingest tests: 22 pass;
  runtime mapping/interpreter/execution tests: 120 pass; Horizon tests: 130 pass.
- `cargo test --workspace --offline`: passes after fixing a stale filesystem-object
  test and replacing an incorrect allocator free-byte assertion with timeline reuse checks.
- CLI `horizon-ipc-smoke` and `fs-smoke`: pass.
- Snipperclips Plus NSP partition index: parsed from the supplied 1,262,965,600-byte
  container without loading payloads. Contains ticket, certificate, program/control
  candidates and content metadata.
- Added user-provided header-key parsing and NCA2/NCA3 AES-XTS header decryption.
  `content-inspect --keys <prod.keys>` verifies filesystem header SHA-256 hashes,
  container size and section bounds. It never prints keys, rights IDs or decrypted key areas.
- Snipperclips Plus NCA headers now identify program, manual, metadata and control
  content, all with title ID `01008e20047dc000`, key generation 3. Program section 0
  is AES-CTR PartitionFs (21,970,944 bytes); section 1 is AES-CTR RomFs
  (1,239,662,592 bytes); section 2 is unencrypted PartitionFs.
- Common-ticket title-key resolution and AES-CTR range decryption now work, including
  key-area resolution for NCAs without title rights. Verified PartitionFS checks
  the master hash and every filesystem block hash before exposing files.
- Snipperclips Plus ExeFS contains `main`, `rtld`, `sdk`, `subsdk0` through `subsdk3`,
  and `main.npdm`. All seven NSO modules decompress and load successfully. Its logo
  partition and CNMT partition also pass their hierarchical SHA-256 checks.
- Crypto tests cover an independent OpenSSL-backed XTS vector, ticket generation,
  personalized-ticket rejection, corruption and truncated/bounded-region rejection.
  RomFS data reads now verify the IVFC chain on demand; whole-image verification,
  update/sparse crypto and RSA signatures remain incomplete.
  Full game execution remains unverified.
- NPDM META/ACI0/ACID parsing validates bounded sections, program ID range, priority
  and stack alignment. Capability enforcement and RSA authentication remain separate.
- Snipperclips Plus's seven modules map successfully into a fresh 39-bit address space
  in rtld/main/subsdk/sdk order, with RX/R/RW segment permissions and zeroed BSS.
  Its initial rtld PC is `0x7100000000`, stack top `0x7f00000000`, priority 44.
  The first instruction is `14000002` (branch +8). This is a diagnostic image,
  not a running game: TLS setup, full guest rtld execution and relocation
  handling remain necessary.
- Added a bounded scalar ARM64 reference interpreter and `content-inspect --cpu-steps`.
  The diagnostic creates a Horizon process/thread and passes the thread handle in x1.
  Instruction fetch enforces execute permission. Supported operations include basic
  branches, condition flags, moves, immediate/shifted arithmetic, bitfields, logical
  immediates, multiply/add/subtract, ADR, integer loads/stores and pre/post-indexed
  memory operations, extended/scaled register-offset integer transfers and conditional
  select/increment/invert/negate, conditional compares, extended-register arithmetic,
  signed/unsigned division and signed/unsigned long multiply-accumulate/subtract.
  Division tests include zero divisors and signed MIN/-1 overflow without host panics.
  RBIT, REV16/REV32/REV64, CLZ/CLS, variable shifts/rotates and SMULH/UMULH
  are covered by width-specific regression tests.
  MRS/MSR FPCR/FPSR now retain per-thread floating-point control/status for the
  baseline ARMv8.0 non-trapping profile; reserved/unsupported feature bits read as zero.
  Floating-point arithmetic coverage remains incomplete.
  SIMD/FP pair transfers support S/D/Q widths and offset/pre/post-index addressing;
  single-register transfers support B/H/S/D/Q widths with unsigned scaled offsets,
  signed unscaled offsets, pre/post-index writeback and extended/scaled register
  offsets. Narrow loads clear the remaining vector bits; memory faults leave vector
  and address registers unchanged. Tests cover every width, independent assembler
  encodings, negative SXTW offsets and legal matching address/vector register numbers.
  The structural decoder now excludes SIMD unsigned transfers from integer IR;
  a full interpreter-path regression covers the actual guest STR Q0,[X8] boundary.
  Literal transfers and structure/lane transfers remain incomplete.
  SIMD/FP registers and control/status state are retained per thread;
  SIMD DUP from a general register supports 8/16/32/64-bit elements.
  Integer modified-immediate MOVI/MVNI/ORR/BIC support shifted, shifting-ones
  and byte-mask forms, including 64-bit writes that clear the upper vector half.
  Regression encodings were independently checked with clang's AArch64 assembler.
  Scalar FMOV immediate S/D and vector FMOV immediate 2S/4S/2D now share an
  integer-only VFPExpandImm helper. All 256 constants are checked against an
  independent exact binary-rational reference, and vector tests cover every
  immediate/arrangement combination. Scalar writes clear upper vector bits;
  moves leave FPCR/FPSR/NZCV unchanged. Reserved and FP16 forms remain unsupported.
  Numeric floating-point arithmetic/conversions are still incomplete.
  SCVTF/UCVTF scalar integer forms now convert W/X sources to S/D using
  integer-only normalization and rounding. All four FPCR rounding modes are
  honored, including ties-to-even and exponent carry; inexact results accumulate
  FPSR.IXC without clearing existing flags. Scalar writes clear upper vector bits.
  Tests cover signed/unsigned limits, source-width truncation, zero registers,
  directed rounding for both signs, and 10,000 deterministic samples against an
  independent nearest-rounding oracle. FP16, fixed-point/SIMD conversions and
  floating-to-integer forms beyond FCVTZS/FCVTZU remain incomplete. The FP profile is non-trapping.
  Scalar FMUL S/D now uses an exact u128 significand product and integer-only
  rounding, covering all four FPCR modes, signed zeros, infinities, signaling/quiet
  NaN priority, default-NaN policy and FZ input/output handling. FPSR invalid,
  overflow, underflow, inexact and input-denormal flags accumulate without changing
  NZCV. Subnormal results round directly at their own quantum; tininess is detected
  before rounding in the baseline profile. Tests cover directed rounding, exponent
  limits, payload preservation and 20,000 deterministic pairs per precision against
  an independent nearest-mode oracle, plus a full interpreter-path regression.
  FP16/vector multiplication and the remaining arithmetic operations are incomplete.
  Scalar FSQRT S/D uses an exactly scaled u128 radicand, an integer floor root
  and one final rounding in the guest FPCR mode. NaN payload/default-NaN policy,
  negative-input IOC, signed zeros, FZ input IDC and sticky IXC are supported.
  Positive finite S/D roots are always finite normal results. Tests cover exact/
  irrational roots, directed rounding, subnormal inputs and 10,000 deterministic
  samples per precision, plus in-place execution through the decoder/runtime.
  FP16/vector square root and remaining FP arithmetic remain incomplete.
  Scalar FADD/FSUB S/D use integer guard/round/sticky bits with all four FPCR
  rounding modes, signed-zero cancellation, NaN priority/payload preservation,
  default NaNs, FZ and sticky exception flags. Tests cover 20,000 deterministic
  pairs per precision/operation and in-place decoder execution.
  Scalar FDIV S/D now divides normalized integer significands, retaining three
  rounding bits and a sticky rational remainder. All four rounding modes, signed
  zeros, NaNs, infinities, finite divide-by-zero, overflow, gradual underflow and
  FZ input/output policies are supported. Tests cover 20,000 deterministic pairs
  per precision, directed rounding/exception boundaries and source/destination
  aliasing through the runtime; instruction words were independently assembled.
  FP16/vector division, fused arithmetic and other scalar FP forms remain incomplete.
  Scalar register-to-register FMOV, FABS and FNEG support S/D with pure bit
  operations, preserving signaling/quiet NaN payloads and subnormals regardless
  of FPCR.DN/FZ. Scalar writes clear upper vector bits, sample the source before
  writing and leave FPCR/FPSR/NZCV unchanged. Tests cover signed zeros, infinities,
  both NaN classes, subnormals and source/destination aliasing; independent
  assembler fixtures confirm all six encodings. Reserved/FP16 precision forms
  stop without mutating context. Vector unary forms remain incomplete.
  Scalar FCMP/FCMPE S/D support register and zero operands with exact NZCV
  outcomes for less/equal/greater/unordered. Signed zeros compare equal;
  signaling NaNs raise IOC for FCMP, and any NaN raises IOC for FCMPE. FZ
  inputs become signed zeros and accumulate IDC. Tests cover operand ordering,
  infinities, NaNs, denormals and a complete decoder/interpreter-path regression
  preserving vector registers and non-NZCV PSTATE bits. FP16 and vector
  comparison forms remain incomplete; scalar conditional forms are covered below.
  FCVTZS/FCVTZU scalar S/D-to-W/X forms now truncate using integer-only
  exponent/significand processing, saturate out-of-range results and accumulate
  IOC/IXC/IDC according to the baseline non-trapping profile. NaNs produce zero;
  invalid conversion suppresses a new inexact exception. FZ inputs become zero.
  Tests cover signed/unsigned boundaries, subnormals, infinities, fractional
  negatives, 10,000 deterministic samples and the actual decoder/runtime path,
  including W zero-extension and XZR writes without changing SP or vector state.
  Other rounding modes, fixed-point/vector forms and FP16 remain incomplete.
  Baseline FMOV general-register transfers cover W/S and X/D in both directions
  plus X/vector D[1] transfers. These are bit-preserving moves, not numeric
  conversions: signaling-NaN payloads, signed zero and subnormal bits are retained,
  with unchanged FPCR/FPSR/NZCV. Low scalar writes clear upper vector bits;
  D[1] writes preserve D[0]. Tests cover XZR/WZR semantics, width mismatches and
  unsupported FP16 encodings, using independently assembled instruction words.
  INS general-register insertion covers every B/H/S/D lane in a 128-bit vector,
  preserving all other lanes. Tests exhaust the valid lane/width combinations,
  check zero-register insertion and reject reserved element-width encodings.
  Vector AND/BIC/ORR/ORN/EOR/BSL/BIT/BIF support 8B and 16B arrangements.
  All source operands, including the old destination for select/insert forms,
  are sampled before writing; 64-bit writes clear the upper vector half.
  Tests compare per-bit truth tables across every operation/width and destination
  aliasing either source, plus the actual ORR/MOV guest instruction through the
  full decoder/interpreter path. NZCV and integer registers remain unchanged.
  Baseline DMB/DSB barriers support the twelve named domain/access options;
  ISB supports SY. Guest memory accesses complete synchronously in the serial
  interpreter, and each instruction is freshly fetched after the prior step.
  Tests cover unchanged context/exclusive reservations, exact PC advancement and
  fetching updated code after ISB. Reserved options remain explicitly unsupported.
  These semantics do not implement parallel CPU or asynchronous GPU/MMIO barriers.
  Unsupported encodings remain explicit.
  LDPSW loads two 32-bit words and sign-extends each into an X register, with
  signed-offset, pre-index and post-index addressing scaled by four bytes.
  Complete-pair reads precede destination/base updates; faults, reserved forms
  and constrained-unpredictable overlapping registers leave state unchanged.
  Tests cover sign boundaries, all addressing modes, SP/XZR behavior and the
  actual guest encoding 6941a660; four fixtures were independently assembled.
- Bounded thread execution now dispatches audited SVCs and resumes at PC+4, preserving
  unaffected registers. QueryMemory writes the guest 0x28-byte MemoryInfo structure
  and reports mappings, unmapped gaps and the inaccessible 39-bit address-space tail.
  Unimplemented/unaudited calls still stop explicitly; this is not a full scheduler.
- Actual Snipperclips probe now exits rtld and reaches SDK code. FPCR initialization
  and GetInfo RandomEntropy are crossed successfully. Four seed words are lazily
  sourced through the host OS CSPRNG and retained per process; seed values are redacted
  from process Debug output. Wrong handles/indices and unsupported InfoTypes fail explicitly.
  GetInfo now exposes explicit ASLR/alias/heap/stack reservations and ProgramId,
  resolving process handles and validating subtypes. The loader creates the same
  checked 39-bit layout passed into Horizon; placement is diagnostic/deterministic,
  not randomized ASLR. Actual heap/alias allocation SVCs remain incomplete.
  GetThreadPriority resolves the main-thread handle and returns its NPDM-derived priority.
  NPDM scheduling descriptors now provide per-process core/priority masks from
  bounded ACI0 capability arrays. Duplicate descriptors, malformed ranges,
  kernel priorities and cores outside the four-core original-Switch profile are
  rejected. Requested grants must fit the ACID ceiling, and the main thread's
  core/priority must fit those grants. Missing grants remain explicitly unconfigured,
  not fabricated. GetInfo CoreMask/PriorityMask (types 0/1, subtype zero) resolves
  process handles or the current-process pseudo-handle and reports these masks.
  Tests cover bounds, duplicate grants, ACID containment, main-thread admission,
  cross-process/stale handles and the guest SVC register ABI. RSA authentication,
  other capability classes and multicore scheduler enforcement remain incomplete.
  Thread creation now uses the NPDM default guest core and checks configured
  scheduling grants before allocating handles/TLS. SetThreadCoreMask retains
  per-thread guest affinity, validates ownership, a nonempty subset of the process
  core mask and inclusion of a concrete ideal core. Process-default, don't-care
  and no-update requests are handled explicitly; malformed updates preserve prior
  affinity. GetThreadCoreMask returns the retained ideal core/mask in the audited
  guest register ABI. Tests cover special requests, creation rejection, stale/
  foreign handles, unchanged state and set/get execution through SVC instructions.
  These are guest policy semantics: physical migration, pinning and multicore
  scheduling are not implemented by the serial reference runner.
  SetThreadPriority now resolves thread handles, checks current-process ownership
  and admits only user priorities included in the NPDM mask. Successful changes
  retain the new priority without altering CPU registers beyond the SVC result.
  Cooperative ready-thread selection honors lower numeric priority first and FIFO
  ties; start/wakeup enqueue at the thread's priority, and changing a queued
  priority reorders without duplicating it. Nonqueued/waiting threads are not woken.
  Tests cover ready-order changes, invalid grants/ranges, stale/foreign handles,
  waiting-state preservation and the guest ABI. Base/effective priority coincide
  before mutex inheritance is applied; physical preemption, migration and a
  complete multi-thread runner remain incomplete.
  time:u now returns typed user/network/steady/timezone/local clock objects
  through real native move handles or the existing domain-object routing.
  Steady GetCurrentTimePoint serializes signed whole seconds and the kernel
  source identity. User/local GetCurrentTime and GetSystemClockContext read
  the injectable wall/monotonic source with checked context-offset arithmetic.
  This is explicit diagnostic host-source policy, not network synchronization,
  persistent RTC state or firmware time setup. Tests cover object identity,
  wire layouts, source consistency and actual guest TLS move-handle/read replies,
  including child lifetime after closing the root. Network reads, timezone rules,
  setters, shared-memory publication and remaining time commands stop explicitly.
  CreateTransferMemory now returns a real process handle to an existing aligned
  heap range. Reservations restrict owner access to None/Read/ReadWrite without
  copying or splitting sparse heap backing; overlap, invalid permissions/state,
  unaligned/empty/overflowing ranges and backing changes are rejected. QueryMemory
  splits reported ranges at reservation boundaries and reports effective owner
  permissions plus Locked. Shrinking away, unmapping or reprotecting reserved pages
  is prevented. Last handle closure releases the reservation and restores access;
  handle allocation/reservation failures roll back without leaving pages locked.
  Tests cover all owner permissions, cross-boundary accesses, unchanged bytes and
  accounting, resize/protect/unmap rejection, duplicate handle lifetime and guest ABI.
  Transfer mapping into another process, other service-owned references, resource-count
  limits, process-exit cleanup and non-heap transferable states remain incomplete.
  nvdrv now creates an independent typed client endpoint. Initialize validates
  two copy handles (the caller's process and its None-permission transfer memory),
  size and ownership before retaining a service-owned transfer reference. It returns
  a distinct NvError payload in the CMIF success reply. SetAruid requires a PID
  descriptor and the kernel-owned application resource identity, not guest PID
  contents. Reinitialization and malformed requests do not replace client state.
  Transfer reservations now stay locked while retained by service/domain objects,
  surviving client transfer-handle closure and shared-server endpoint clones;
  last endpoint/domain release reclaims otherwise-unreferenced reservations.
  Tests cover real TLS Initialize framing, size rejection, repeat initialization,
  reference lifetime, clone/last-close behavior and resource-identity validation.
  Device Open/Ioctl/QueryEvent, nvmap allocations, GPU virtual mappings, submission
  and synchronization remain unsupported; no host NVIDIA device is accessed.
  Threads now receive distinct zeroed 0x200-byte TLS regions, eight slots per page,
  in a checked loader reservation. TPIDRRO_EL0 reads that mapped guest address;
  TPIDR_EL0 is separate guest-writable state. QueryMemory reports ThreadLocal type 12.
  Tests cover page crossing, isolation and failed-allocation rollback. Slots are not
  reclaimed yet, and guest TLS IPC synchronization remains separate work.
  Single-register LDXR/LDAXR/STXR/STLXR and LDAR/STLR cover byte/halfword/word/doubleword
  widths in the serial reference interpreter. Per-thread exclusive reservations use
  a conservative address-space write epoch, invalidated by all writes/mapping changes
  (including same-value writes). CLREX and SVC exception entry clear the local monitor.
  This allows conservative spurious failures and is not a parallel-backend memory model;
  remaining atomic forms are incomplete.
  Paired LDXP/LDAXP/STXP/STLXP now cover W/W and X/X operands, with pair-size
  alignment checks, one checked transfer and reservations spanning both elements.
  Store status reports success once or failure after epoch/address/size mismatch;
  every completed store attempt clears the reservation. Tests cover both widths,
  acquire/release variants, SP addressing, same-value invalidation, reserved/
  overlapping operands and faults. Failed memory writes preserve the diagnostic
  CPU context, including status/reservation, for both single and paired stores.
  This serial model does not claim parallel-backend 128-bit atomic integration.
  CNTPCT_EL0 and CNTFRQ_EL0 reads now dispatch to the kernel's injectable
  monotonic time source at each instruction, sharing the source used for wait
  deadlines. The architectural frequency is 19.2 MHz, with widened integer
  nanosecond conversion and floor rounding. CPU-only interpretation stops explicitly
  at a counter boundary when no kernel source is supplied; the thread runner samples
  and resumes without treating MRS as an exception or clearing exclusive monitors.
  Tests cover sub-tick boundaries, u64-range conversion, injected clock changes,
  exact budget/PC accounting and XZR/flags/SP/exclusive preservation. Virtual counter
  offset policy, timer interrupts and GetSystemTick SVC remain incomplete.
  SignalProcessWideKey now clears the guest waiter flag for an empty wait queue.
  Known mutex and object waits do not belong to that condition queue and are not
  woken by its empty-queue signal. Unclassified waiting threads still stop explicitly:
  unsupported wait kinds remain explicit; condition queues and deadlines are now
  implemented as described in the latest process-runner baseline below.
  ConnectToNamedPort reads a bounded guest string and creates a real sm: session;
  unknown ports, unterminated names and invalid encoding are rejected without advancing PC.
  SendSyncRequest now exchanges the mapped 0x100-byte TLS command buffer with the
  existing HIPC/CMIF dispatcher. Guest buffer permissions are checked before service
  dispatch; replies are bounded and do not overwrite the other 0x100 TLS bytes.
  Tests cover real TLS exchange and malformed-message failure without guest PC/data mutation.
  Corrected response framing: CMIF starts at an absolute 16-byte-aligned message
  offset, with 16 total padding bytes split before/after the body. Exact-offset tests
  cover plain and handle-bearing responses; permissive parser-only checks missed this.
  Signed integer immediate-offset loads are now covered by width-specific tests.
  The incorrect response framing previously led the guest SDK to Break; after its
  correction, Snipperclips proceeds to service initialization.
  lm OpenLogger now returns a typed, independent ILogger session through a real move
  handle. Log accepts validated input buffers with retail discard behavior and bounded
  counters; SetDestination retains the shared mask. Newer logging commands remain explicit
  unsupported operations. No guest messages are written to host files or remote sinks.
  HIPC Release is handled separately from CMIF, closes the server object while preserving
  the client handle for CloseHandle, and rejects subsequent use. Tests verify that releasing
  the root lm session does not close the returned logger session.
  apm OpenSession now returns a typed move-handle session. Set/GetPerformanceConfiguration
  retain separate normal/boost policy tokens; CPU overclock policy is shared with the root
  manager. The diagnostic machine uses handheld mode and explicit initial configurations;
  this does not modify host clocks or implement firmware clock validation/docking events.
  Tests cover retained state, malformed arguments and the real guest TLS move-handle reply.
  appletOE now exposes OpenApplicationProxy and a typed application-proxy/controller
  hierarchy. The kernel validates the caller's process copy-handle and PID transport
  presence; guest PID contents are not used as authority. Controller operations remain
  unsupported, and application ownership/lifecycle policy is not yet complete.
  HIPC manager commands are routed separately from service methods. QueryPointerBufferSize
  reports zero because no server-process pointer buffer is reserved; AutoSelect clients
  consequently use map-alias buffers. Regression tests cover all three manager message
  types and typed proxy handles through the real TLS transport.
  ConvertCurrentObjectToDomain now transfers the root into a bounded per-session object
  table. Domain calls route by object ID, returned application/APM/logger/service objects
  receive domain IDs instead of native handles, and domain Release invalidates only the
  selected object. IDs are monotonic, not reused; session Release drops the entire table.
  Exact-offset tests verify response framing, root conversion, child IDs and release
  independence. Filesystem, file, directory and storage results now use shared
  validation for native sessions and domain children. A guest TLS regression opens
  the current-process RomFS storage in a domain and reads its actual backend size
  through the returned object ID without allocating a native child handle.
  Input-object transfer, manager copy
  operations and full capacity-failure transactionality remain incomplete.
  IWindowController GetAppletResourceUserId now returns a stable per-process resource
  identity, shared across that process's controllers and distinct between processes.
  Visibility and nonnegative GPU time-slice policies are retained in process state;
  malformed updates do not change state. Foreground-right commands use documented AM
  no-op behavior, not host focus manipulation. Window visibility/time-slice policy
  is not yet connected to rendering or scheduling, nor identity to HID/VI ownership.
  ICommonStateGetter GetEventHandle now returns a native copy-handle to the process's
  level-triggered notification event, including in domains (not a domain object ID).
  Repeated handles refer to the same event; closing a client handle preserves the
  application-owned event. The diagnostic starts in focus with a queued focus-change
  message. ReceiveMessage clears the signal when the queue drains and returns guest
  result 0x680 when empty. Focus/handheld/performance reads match the diagnostic policy.
  Tests verify event signal lifetime, queue draining, domain copy-handle framing and
  repeated handle closure. Host-driven transitions and full event waits remain incomplete.
  WaitSynchronization now dispatches through the guest register ABI, validates up to
  64 handles before selecting a signaled object, and returns the first ready index in
  w1. Event/process/thread signal states are supported; zero-timeout polls return
  timeout 0xea01. Pending waits retain resolved object identities, suspend the thread,
  and resume through cooperative polling on signal or a monotonic deadline. Tests use
  an injected clock and verify handle-close lifetime, deadlines and untouched registers/
  guest out-pointer memory. Event cancellation/reset, general event creation, host-driven
  signaling and full scheduler-loop integration remain incomplete; existing application
  events are manual-clear, and auto-clear synchronization semantics are not generalized.
  ISelfController now retains the three adjacent focus-handling bools, out-of-focus
  suspension policy, operation/performance notification flags, restart-message flag
  and screenshot permission in application state. Payloads/enums are validated before
  mutation; regression tests verify malformed updates preserve prior state.
  Host focus transitions, actual scheduling suspension and screenshot behavior remain
  unintegrated; stored policy does not imply those subsystems are complete.
  aoc:u now provides Count/ListAddOnContent against an explicit installed catalog and
  GetAddOnContentListChangedEvent as a native copy-handle. Caller queries use the kernel's
  process identity, not the guest PID word. The catalog starts empty and advertises only
  registered validated entries, with deterministic sorted pagination and bounded output.
  Registration signals the shared change event; duplicates do not add entries. Tests
  cover empty/nonempty catalogs, per-application isolation, pagination/output limits and
  real guest TLS event handles connected to WaitSynchronization.
  Automatic CNMT/entitlement population, add-on mounts, purchase services and firmware-
  profiled command availability remain incomplete; no missing DLC is fabricated.
  pctl:a CreateService/CreateServiceWithoutInitialize now return typed parental-control
  sessions with distinct initial initialization state, through native move handles or
  domain object IDs. The factory validates a reserved u64 and PID descriptor presence;
  the returned object's Initialize command retains initialization state. Tests verify
  factory differences, malformed requests, guest TLS handles and initialization dispatch.
  Permission/settings commands remain explicitly unsupported; no parental restrictions
  are silently bypassed. Profile command gating and session limits remain incomplete.
  HIPC CloneCurrentObject now returns a native move handle to an independent client
  endpoint sharing the canonical server object and domain table. Mutable session state
  is not duplicated. Release and CloseHandle remove endpoint references independently;
  the last endpoint destroys the canonical object/domain. Clone allocation reserves the
  handle before moving state, preserving the original on failure. Tests cover mutable
  object sharing, domain ID/table lifetime, last-close cleanup and allocation rollback.
  CopyFromCurrentDomain and process-exit-wide handle cleanup remain
  incomplete. Shared endpoints use kernel-owned canonical IDs, not nested alias chains.
  CloneCurrentObjectEx now accepts a u32 session-manager tag using the same
  canonical-server sharing and native move-handle lifecycle as ordinary clones.
  The serial dispatcher has one manager, so tags do not select another worker or
  duplicate object state. Missing arguments are rejected using raw-data length
  including total HIPC padding, preventing trailing padding from masquerading as
  a tag. Tests cover all three manager message types, tags 0/1/u32::MAX, malformed
  length without ID mutation, shared nvdrv state and independent/last-close behavior.
  Parallel server-worker routing remains outside this serial implementation.
  GetInfo UsedMemorySize (type 7) now sums byte-backed code/data, stack and heap
  mappings, excluding virtual reservations, TLS and non-charged shared/alias kinds.
  Checked arithmetic rejects overflow. Tests cover unmapping, process-handle/pseudo-
  handle resolution and wrong handle/subtype rejection. Unique-page charging for
  future shared/alias backing and exact kernel metadata accounting remain incomplete.
  AddressSpace now has an explicit charged-memory quota, enforced before backing
  allocation for code/data/stack/heap mappings. GetInfo TotalMemorySize reports that
  quota and rejects unconfigured address spaces. The diagnostic loader chooses 3 GiB
  independently of its 6-GiB virtual heap reservation; this is allocator policy, not
  a measured/firmware-derived application pool. Tests cover failed-map rollback,
  quota reduction rejection, unmap refunds and consistent GetInfo values. Host reserve
  allocation failures become explicit errors for dense non-heap backing.
  Heap storage now uses demand-zero 4-KiB pages: untouched reads allocate no pages,
  writes materialize only touched pages, and multi-page transfers retain permissions/
  bounds checks. Dense code backing retains its direct fetch path. Heap resizing
  preserves retained pages, discards truncated pages, zeroes regrown ranges and refunds
  charged quota on shrink/unmap. Admission checks precede mutation.
  Audited SetHeapSize enforces 2-MiB alignment, the 4-GiB size ceiling, virtual heap bounds
  and charged quota, returning fixed heap base in x1 and guest result codes on admission
  failure. Tests cover a multi-GiB empty heap, cross-page writes, shrink/regrowth, quota
  rollback and register ABI. Sparse-page host OOM recovery, backing aliases and fragmented
  heap reprotection/reference constraints remain incomplete.
  Snipperclips obtains its 0xbd000000-byte heap and crosses the previous SIMD
  encoding 6f00e400 and DMB ISH encoding d5033bbf boundaries,
  and the SIMD single-transfer boundaries 3c830ec0/3d800100. The latest probe
  crosses FMOV general-register encoding 9e670100 and INS encoding 4e181d00.
  Scalar FMOV immediate encoding 1e2c1000 and unsigned conversion UCVTF encoding
  9e230101 are also crossed. The vi:m opening boundary is crossed. A subsequent
  probe reached 145,077,356 instructions and identified an unsupported domain
  return for the current-process RomFS storage; that routing is now implemented.
  The CPU diagnostic now mounts the actual title data using a separate source
  descriptor and a read-only NCA RomFS backend. Its exposed size and offsets refer
  to the final IVFC data level, not to the entire encrypted section. Bounded reads
  decrypt only required ranges and verify full zero-padded blocks recursively
  against authenticated parent hash blocks and the FS-header master hash.
  The FS header's digest is checked by ingest; RSA trust remains unimplemented.
  Source bounds, overlapping levels, malformed metadata and unsupported patch/
  sparse/compressed/encryption profiles fail explicitly. Corruption does not expose
  a partially verified output prefix. Tests cover plaintext/CTR reads crossing
  blocks, partial final blocks, every hash level, late corruption, truncation,
  malformed layouts, read-only operations and EOF/overflow bounds. No full-image
  verification or host cache is claimed. The verified Snipperclips rerun reaches
  145,185,154 instructions (`0x7102649744`) and stopped at fsp-srv command 1005;
  the previous missing RomFS mount boundary is crossed. GetGlobalAccessLogMode
  (1005) now returns the disabled host policy as a u32, with native/domain TLS
  response tests. Access-log setters and SD output remain unsupported until real
  sinks are connected. The subsequent probe reached 147,031,046 instructions
  and stopped at LDPSW encoding 6941a660, now implemented.
  A verified rerun crossed LDPSW and reached 147,291,736 instructions,
  stopping while opening network interface service `nifm:u`. That service now
  returns typed IGeneralService objects through its legacy/modern factories,
  with native move handles or domain IDs. Modern requests require the reserved
  u64 argument and a PID descriptor; the transmitted PID is not process authority.
  Tests cover malformed requests, factory results and guest TLS native/domain
  framing, including missing-PID rejection before child allocation. Profile,
  connectivity and request commands remain explicitly unsupported, not successful
  placeholders. A verified rerun reached 147,380,329 instructions and stopped
  while opening socket service `bsd:u`. BSD endpoints now retain validated
  process-local transfer memory on RegisterClient (0), including exact size,
  locked/None owner permissions, one copy handle and PID descriptor validation.
  Version-1 buffer configuration is retained without claiming a socket backend.
  RegisterClient returns a u32 registration status, independently of the CMIF
  result. Success is zero, not a process identifier. An earlier u64/PID response
  was incorrect and caused the guest SDK to reject successful registrations.
  StartMonitoring (1) acquires the same registered resource reference on a separate
  endpoint using the authenticated caller identity. Its scalar u64 is reserved,
  not permission to select another process; unregistered callers are rejected.
  This does not imply connection
  success. Weak registry entries do not keep resources alive after last session
  closure; transfer garbage collection includes BSD references and domain objects.
  Tests cover guest TLS response layout, malformed size/PID rejection, shared
  monitor references, duplicate registration, final closure restoring owner access
  and unsupported socket operations. Socket I/O and actual network monitoring are
  not implemented. The latest verified rerun reaches 147,509,997 instructions
  (`0x7102649744`) and stopped at IApplicationFunctions command 22.
  SetTerminateResult (22) now retains the application's u32 termination result,
  independently of the successful CMIF response. It does not exit the process;
  the value survives a subsequent process exit and remains process-local.
  Tests cover arbitrary result bits, truncated payloads, native HIPC padding,
  successful guest TLS response and unchanged running state. The diagnostic
  reports nonzero termination results and, on svcBreak, emits bounded frame-pointer
  return addresses with module-relative offsets. Invalid/unreadable/cyclic chains
  stop safely and non-code return addresses are not emitted; a CLI test covers
  those cases and the sixteen-frame limit. This is not a complete symbolic unwinder.
  The latest verified rerun reaches 147,517,146 instructions at `0x710264979c`:
  the guest sets termination result `0x2a2` and invokes svcBreak (0x26) with reason
  zero. This is a guest abort requiring diagnosis, not an instruction/service that
  may be skipped to make boot appear successful. The trace includes
  `sdk+0x91ad4`, `sdk+0x91b58`, `sdk+0x913dc`, `main+0x7db830`,
  `main+0x6cac3c` and `rtld+0xdc`.
  Bounded NSO ELF64 dynamic function-symbol diagnostics now use .rodata-relative
  tables and module-relative symbol values. Undefined imports and data/TLS entries
  cannot label return PCs; malformed table/name ranges, unterminated/control names
  and overflow are covered by tests. Optional diagnostic metadata failures are
  reported without changing executable admission or applying relocations.
  The verified symbolic trace identifies the caller as
  `nn::socket::detail::InitializeCommon(nn::socket::Config const&)`, through the
  public socket Initialize function, then `nnMain`. Bounded executable caller
  windows are included for diagnosing its failed check without dumping stack data.
  Expanding the first caller's bounded code window identifies a nonzero RegisterClient
  output status check at `sdk+0x91a44`. Our former PID-valued response made that
  check fail and branch to the abort block. The response is now the correct zero
  u32 registration status; StartMonitoring resolves its caller rather than treating
  the reserved scalar as a PID. Unit/TLS lifetime tests assert the corrected wire
  value, zero reserved argument and inability to monitor another process through
  a fabricated scalar. BSD registration remains distinct from socket I/O support.
  The verified corrected rerun crosses that status check without the prior
  `0x2a2` termination result or abort trace. It reaches 147,520,968 instructions
  at `0x71026495e4` and stops at unaudited MapMemory (SVC 0x04): destination
  `0x7e8dbdd000`, source `0x20b7ffb000`, size `0x100000`. Shared backing,
  owner permissions and mapping lifetime were required before resuming.
  MapMemory/UnmapMemory now support heap-backed same-process stack aliases without
  copying bytes or double-charging memory. The original range is inaccessible and
  QueryMemory reports Locked with no owner permissions; exact-pair unmapping
  restores access while preserving writes through the alias. Backing changes and
  transfer reservations cannot invalidate live aliases. Four dedicated tests cover
  backing/lifetime, query boundaries, rejected mutations, malformed requests,
  independent aliases, heap growth and cloned address spaces. Other source states
  and partial alias unmapping remain incomplete.
  The subsequent real-game probe crosses MapMemory and reaches 147,538,676
  instructions at `0x7102649618`, stopping at unaudited CreateThread (SVC 0x08).
  CreateThread now has a separate audited guest ABI: x1 entry, x2 argument,
  x3 initial stack, w4 priority and w5 core; w0 returns the result and w1 the
  handle without dereferencing the wrapper's x0 output pointer. Admission checks
  the process core/priority grants before handle/TLS allocation, including the
  process-default core request. The child retains its argument, independent TLS,
  requested affinity and inherited FPCR/FPSR, remaining Created rather than
  running immediately. Two runtime tests exercise actual SVC instructions and
  rejected grants without creating threads. Resource-count limits, complete
  firmware-specific TLS fields and multi-thread execution remain incomplete.
  The subsequent real-game rerun crosses CreateThread and reaches 147,568,008
  instructions at `0x710264962c`, stopping at unaudited StartThread (SVC 0x09).
  StartThread now validates native thread handles, ownership and Created state
  in the guest path, returning guest errors for invalid handles and restarts.
  A serial process runner executes ready threads in bounded 100,000-instruction
  slices using Horizon priority/FIFO selection, retaining complete contexts and
  clearing local exclusives at context-switch boundaries. StartThread ends the
  caller's slice so a newly ready child can be considered immediately. Selection
  leaves other processes' ready entries untouched; faults identify the actual
  stopped thread and leave its PC available for diagnosis. The CLI probe now uses
  this runner and reports suspension when no thread is ready, not boot success.
  Three tests cover actual child execution/exit, context retention and budget
  resumption, foreign queues/faults, and invalid/repeated start requests.
  Physical multicore scheduling, timed idle waiting and
  the remaining wait/yield mechanisms are still incomplete.
  The verified process-runner probe crosses StartThread and executes child
  thread 2. It reaches 147,572,969 total instructions, stopping in that child at
  `0x71026496fc` on unaudited ArbitrateLock (SVC 0x1a). This establishes actual
  child execution, not completion of thread synchronization or game boot.
  ArbitrateLock/ArbitrateUnlock now use a process-local mutex wait graph. A
  matching guest owner/waiter word suspends the caller; a changed word returns
  without inventing ownership. Unlock commits the next waiter's tag to guest
  memory, selects by effective priority/FIFO and reparents remaining waiters.
  Priority inheritance propagates through owner chains and is recomputed from
  separate base priorities after handoff or priority changes. Failed guest writes
  preserve the wait graph and do not wake threads. Four Horizon tests cover
  handoff/FIFO, chained donation/base changes, malformed inputs/races and failed
  unlock; one actual-SVC runtime test covers suspension PC, resumption and context
  retention. General cancellation and exiting owners with waiters are not
  implemented; owner exit currently fails explicitly.
  The first real-game mutex rerun crossed ArbitrateLock but exposed an overly
  broad SignalProcessWideKey guard: it rejected unrelated mutex waits. The guard
  now distinguishes known mutex/object queues from unimplemented condition waits.
  A fifth Horizon regression verifies that signaling an empty condition queue
  clears its flag without changing a mutex word, ownership, donation or wait state.
  The corrected real-game rerun crosses mutex wait/unlock and the empty condition
  signal, reaching 147,594,167 instructions in thread 2 at `0x710264970c`.
  It stops at unaudited WaitProcessWideKeyAtomic (SVC 0x1c), with mutex address
  `0x7101c2d684`, condition key `0x7101c2d688` and infinite timeout. Atomic mutex
  release, condition waiting/signaling and reacquisition are the next boundary.
  WaitProcessWideKeyAtomic now releases the mutex and enters a process-local
  condition queue in a single serial kernel transition. Signal aligns the key,
  selects by priority/FIFO and honors count/broadcast, acquiring an available
  mutex or requeuing against its owner with priority donation. Positive deadlines
  survive requeue and cancel either wait phase with TimedOut; zero timeout still
  releases the mutex without sleeping. Key flag writes are best effort, matching
  the void signal ABI; native guest result/output registers are preserved correctly.
  Six Horizon tests use a frozen clock for deterministic queue/deadline checks,
  covering free/busy reacquisition, count/FIFO, process isolation, zero/positive
  timeout and invalid owners. One actual-SVC process-runner regression covers
  aligned-down keys, suspension PC and resumption with retained registers.
  Timed host-idle waiting, general cancellation and tick-quantized firmware timeout
  rounding remain incomplete; the diagnostic runner can return suspended.
  Two real-game reruns cross condition wait/signal and reach 149,995,518
  instructions in thread 1 at `0x7102861d7c`, stopping on unsupported ARM64
  instruction `138c2222`. No menu, frame presentation or gameplay is verified.
  EXTR now extracts a W/X-sized window from the concatenated register pair,
  including its ROR-immediate alias. Both sources are sampled before writes;
  W sources are truncated and W results zero-extended, with ZR rather than SP
  semantics and unchanged flags. All legal shifts in both widths and destination
  aliasing are checked against a bit-by-bit oracle. Two further regressions cover
  the actual guest instruction, zero sources and reserved encodings that leave
  state unchanged. Eight encodings were independently assembled with clang.
  The verified EXTR rerun crosses the prior SDK instruction and reaches
  171,876,380 total instructions in thread 1 at `0x71006ce2cc`, stopping on
  unsupported instruction `f9800261`. Crossing SDK initialization is not proof
  of reaching the game loop, a menu or frame presentation.
  PRFM immediate/register/literal and PRFUM unscaled prefetch hints now have an
  explicit no-cache policy in the serial reference backend: no data translation,
  memory access or allocation occurs, and registers/flags/exclusives remain
  unchanged. Fetch permissions still apply. Register sub-word indices and newer
  RPRFM encodings are not admitted. The unsigned hint is no longer classified as
  an integer load/store in CPU IR. Two runtime tests cover independently assembled
  addressing forms, hint/extension values, untouched state and neighboring real
  loads/stores that must still fault; one decoder test guards the IR distinction.
  Nine fixtures, including actual guest instruction `f9800261`, were independently
  assembled with clang. This is an implementation-defined hint policy, not a
  bypass of required guest reads or a host cache-performance claim.
  The verified prefetch rerun reaches 192,547,687 instructions in thread 1 at
  `0x71003752f0`, stopping on unsupported instruction `1e205500`. No game loop,
  menu or frame presentation is verified by that instruction count.
  FCCMP/FCCMPE conditional scalar S/D comparisons now reuse the existing exact
  comparison helper and guest condition evaluator. A passing condition updates
  NZCV and sticky FPSR exceptions; a failing condition installs immediate NZCV
  without evaluating FP inputs or raising NaN/denormal exceptions. Both AL and
  NV use the architectural always-true condition semantics. Registers, vectors,
  FPCR and non-NZCV state remain unchanged. Three tests cover all sixteen input
  flag patterns/conditions/fallbacks, S/D quiet/signaling comparisons, skipped
  exceptions, actual guest encoding and unsupported precisions without mutation.
  Four FCCMP/FCCMPE fixtures were independently assembled with clang.
  The verified conditional-compare rerun crosses the prior instruction and
  reaches 192,547,705 instructions in thread 1 at `0x710037532c`, stopping on
  unsupported instruction `1e22c001`. Frame/menu/gameplay milestones remain unverified.
  Scalar FCVT S-to-D and D-to-S now convert with integer significands and the
  shared final-rounding helper, honoring all four guest rounding modes. NaN
  sign/payload alignment, signaling quieting/IOC, default NaNs, signed zero,
  FZ input/output, overflow and gradual underflow follow the non-trapping baseline.
  Subnormal outputs round at their own quantum; scalar writes clear upper vector
  bits and accumulate FPSR without changing NZCV. Three arithmetic tests cover
  20,000 deterministic samples per direction, directed rounding, exact/inexact
  subnormals, overflow and NaN policy. One actual-interpreter regression covers
  both directions, preserved state and rejected half/reserved/same-precision forms.
  Four instruction encodings were independently assembled with clang. FP16 and
  vector precision conversions remain incomplete.
  The verified precision-conversion rerun crosses the prior instruction and
  reaches 193,686,947 instructions in thread 1 at `0x7102649744`. It stops while
  opening unregistered service `hid`, the next input-service integration boundary.
  No controller shared-memory publication, game menu or frame presentation is
  established by this service request.
  HID is now registered with a typed CreateAppletResource factory. The guest
  transport requires a PID descriptor, no transferred handles and a complete u64
  application resource identity matching the kernel-owned caller identity; the
  transmitted PID is not authority. Native replies return a real child session,
  and domain replies return a typed IAppletResource object. One service test and
  one real-TLS native/domain regression cover retained identity, missing/truncated
  arguments, forged identity, no allocation on rejection and native child lifetime
  after root closure. GetSharedMemoryHandle now returns a real copy handle to a
  service-owned 0x40000-byte shared object in native and domain transport. Shared
  mappings retain the same backing, enforce data permissions and reject generic
  reprotection, loader writes and unmapping; object-matched unmapping is required.
  Publication revisions invalidate the conservative exclusive-monitor epoch.
  Four storage/mapping tests and the extended guest transport regression pass.
  Guest MapSharedMemory/UnmapSharedMemory SVC integration now validates aligned
  addresses/sizes, handles, full object size, configured ASLR placement excluding
  heap/alias reservations, and object-owned guest permissions before mutation.
  HID objects permit only Read; rejected mapping/unmapping leaves storage intact.
  Both SVCs return W0 only through the audited interpreter dispatch.
  A mapping SVC regression covers errors, publication and identity-matched removal.
  Controller sampling and Linux input remain incomplete; this buffer does not yet
  publish HID samples. General owner/remote shared-memory creation is incomplete.
  The verified HID-factory rerun reaches 193,731,090 instructions in thread 1
  at `0x7102649744`, stopping at IAppletResource command 0 (GetSharedMemoryHandle).
  This verifies resource creation by the actual game, not shared-memory publication
  or usable controller input.
  After the real shared-memory handle implementation, the actual game reaches
  193,759,205 instructions and stops at MapSharedMemory (SVC 0x13), PC
  `0x71026496a4`: address `0x18c0204000`, size `0x40000`, permission Read.
  This crosses GetSharedMemoryHandle but does not yet map or sample HID data.
  The subsequent SVC mapping implementation crosses that boundary in the actual
  game: 193,769,399 instructions, PC `0x7102649744`, stopping at HID command 103
  (ActivateNpad). Shared mapping succeeds; no controller sampling,
  menu, rendered frame or gameplay is verified.
  ActivateNpad (103) now retains application-scoped activation state, with
  idempotent repeated activation. Guest requests require PID framing and the
  kernel-owned application resource identity; forged/truncated requests reject.
  Service and actual TLS regressions pass. Activation does not connect any device
  or publish samples. The command identification was corrected against libnx;
  SetSupportedNpadStyleSet is command 100, not 103.
  The actual activation rerun reaches 193,777,685 instructions in thread 1,
  PC `0x7102649744`, stopping at unsupported HID command 1. This crosses
  ActivateNpad but does not validate controller connection or input publication.
  HID command 1 is ActivateDebugPad (SwIPC interface reference). It now retains
  separate, application-scoped debug-pad activation without activating Npad or
  inventing a connected device. Repeated activation is idempotent. Truncated
  payloads reject; the guest TLS identity/PID regression now covers factory,
  debug-pad and Npad activation. Debug-pad sampling remains incomplete.
  The real debug-pad activation rerun reaches 193,815,547 instructions in
  thread 1, PC `0x7102649744`, stopping at HID command 100
  (SetSupportedNpadStyleSet). All offline workspace tests pass after this change.
  SetSupportedNpadStyleSet (100) and GetSupportedNpadStyleSet (101) now retain
  and roundtrip the full u32 application policy without inventing device connection.
  The setter's aligned u32/padding/u64-Aruid wire layout and caller identity are
  checked before state mutation. Service regression covers all truncated payloads,
  padding independence, high-bit retention and application isolation; guest TLS
  regression covers both commands' PID and authenticated identity requirements.
  Actual style-policy rerun reaches 193,921,336 instructions and a guest abort
  at PC `0x710264979c` (Break SVC 0x26), terminate result `0x00011a0b`.
  The bounded guest backtrace identifies `nn::hid::SetSupportedNpadIdType`
  and includes `nnMain+0x550`. This is evidence of entry into game initialization,
  not a main loop, menu or gameplay. The next input boundary is controller-ID
  policy (command 102), whose guest buffer/transport requires investigation.
  HID now advertises a bounded 0x400-byte host pointer-staging capacity through
  HIPC manager command 3; other endpoints retain zero. Aggregate HID input/output
  pointer sizes are checked before buffer materialization. The existing native
  transport query regression covers both policies and all three manager message
  forms. SetSupportedNpadIdType (102) validates one input u32 list and commits
  application policy only after checking its size and every controller ID.
  IDs 0..7, Other (0x10) and Handheld (0x20) are retained, with at most ten entries;
  empty lists are supported. A service regression covers invalid direction,
  truncated/oversized buffers, invalid IDs, atomic rejection and application
  isolation. Controller connection and sampling remain incomplete.
  The actual HID pointer-capacity/controller-ID rerun crosses the previous SDK
  abort and reaches 195,041,951 instructions, PC `0x710283e388`, stopping at
  unsupported ARM64 instruction `1e223c21`. No frame/menu/gameplay is verified.
  Scalar FCSEL S/D now selects raw operand bits using guest NZCV, clearing the
  upper vector bits without evaluating NaNs/denormals or modifying FP status.
  Tests cover all 16 conditions across all NZCV combinations, both widths,
  source/destination aliases and V31, plus actual guest instruction `1e223c21`
  and unchanged state on unsupported FP16/reserved precision. Four instruction
  fixtures were independently checked with clang's AArch64 assembler.
  The real FCSEL rerun crosses that instruction and reaches 198,974,842
  instructions in thread 1, PC `0x7102649744`, stopping at unsupported
  ISelfController command 40. This is the next application/display integration
  boundary; no rendered frame, menu or gameplay is verified.
  ISelfController CreateManagedDisplayLayer (40) now allocates a real display
  model layer on the enabled Default display, with its dimensions and an owning
  application resource identity. Replies carry that allocated u64 identity, not
  a fixed placeholder. Managed layers survive controller handle closure and are
  removed on owning process exit; lookup rejects other resource identities.
  Display ownership/lifetime and actual native guest TLS reply regressions pass.
  Native windows, Binder producers/buffer queues and actual rendering remain
  incomplete; layer allocation alone does not create a Linux window or frame.
  The actual managed-layer rerun crosses ISelfController 40 and reaches
  199,015,269 instructions in thread 1, PC `0x7102649744`, stopping at
  IApplicationDisplayService OpenLayer (2020). All offline workspace tests and
  all-targets checking pass. The next required integration is an owned native
  window backed by a Binder buffer-queue producer; no presentation is verified.
  A host-neutral BufferQueue now retains allocator-provided backing through
  Free/Dequeued/Queued/Acquired transitions, FIFO submission and frame-identity
  checked release. Exhaustion returns no available slot, never a fabricated
  buffer. In-use replacement, stale releases and invalid producer operations
  reject without changing ownership. Tests cover real shared-storage lifetime,
  published bytes, reuse only after release, FIFO and cancellation.
  This is an internal ownership component, not a complete Android BufferQueue
  implementation: Binder registration/parcels, fences, nvmap buffers, native-window
  serialization and VI OpenLayer integration remain required. The actual game
  stop remains OpenLayer (2020); no new game boot claim is made by these tests.
  Checked Binder parcel framing now exposes bounded, nonoverlapping payload and
  opaque object-table slices, with overflow/truncation rejection. Serialization
  uses the four-u32 Switch header. Aligned payload reads and UTF-16 strings use
  transactional cursors so malformed strings cannot partially consume requests.
  Two independent fixture/negative regression tests pass. Object interpretation,
  producer registration, native-window export and Binder transactions are not
  implemented by this parser; OpenLayer remains the actual guest boundary.
  A separate producer registry now binds distinct positive Binder identities to
  application/layer-owned queues and serializes the 0x28-byte dispdrv native-window
  payload inside a checked parcel. The exported ID resolves to that actual queue;
  no buffers are attached until storage is supplied. Managed-layer lookup validates
  ownership before creating an export; layer/process destruction releases producers.
  Registry tests cover stable per-layer identity, foreign rejection, export framing,
  initially empty queues and non-recycled identities. VI guest OpenLayer routing,
  Binder transactions, reference accounting, nvmap backing and fence integration
  remain incomplete. This is not yet a usable guest native window or rendered frame.
  VI OpenLayer (2020) now routes through authenticated caller ownership and exports
  the managed layer's registered producer parcel through a checked output buffer.
  Display name/session-open state, u64 layer/resource identities, PID framing,
  native argument length and output write permission/capacity are validated.
  A service regression verifies exported size/identity, foreign and short-buffer
  rejection, and resolution to the actual empty producer queue. Binder commands,
  VI CloseLayer, buffer allocation/fences and presentation remain incomplete.
  The actual OpenLayer rerun no longer stops at unknown command 2020, but the
  guest aborts in VI CreateLayer with terminate result `0x00000272` after
  199,150,296 instructions (Break SVC 0x26, PC `0x710264979c`). The backtrace
  includes `nnMain+0x5c4`. Native-window acceptance is therefore unproven;
  inspect the exact SDK parcel expectations before claiming this boundary passed.
  Added a bounded 16-entry IPC metadata history (service, command, result only)
  and the immediate VI caller's code to abort diagnostics. A real rerun confirms
  OpenDisplay (1010), CreateManagedDisplayLayer (40) and OpenLayer (2020) each
  returned CMIF success; no Binder request occurs before the guest-generated
  `0x272` failure. The caller checks a local native-window conversion result
  (call site `0x7102582958` targets `0x71025858c8`). This narrows
  the next action to SDK native-window decoding rather than guessing a service
  failure. Horizon's 102 tests remain passing.
  A subsequent probe confirms that `0x71025858c8` is an AArch64 import stub
  (ADRP/LDR/ADD/BR), not the decoder body. Abort diagnostics now resolve this
  exact stub sequence through the loaded import slot, requiring an executable
  destination before printing its bounded instruction window. A CLI regression
  covers successful resolution, non-executable destinations, mismatched stubs
  and invalid addresses; both CLI tests pass. This diagnostic improvement does
  not change the native-window reply or establish graphics acceptance.
  The resolved-code probe still terminates with `0x272` after 199,150,296
  instructions. Its expanded trace exposes a parcel conversion routine beginning
  at `0x7102584d8c`, which initializes a parcel, calls another import to load the
  supplied bytes and rejects a nonzero return before accessing the object.
  Follow those import targets and their length/object checks next; no parcel
  format correction has yet been verified against the game.
  The SDK parcel-loader import `0x7102585d88` resolves to `0x7102584f70`:
  it reads the four-word header and passes the object-table length divided by
  four to its internal reader. Native-window export now includes one actual
  payload-relative object offset (zero), identifying the flattened producer at
  the start of the payload. The reply is 0x3c bytes rather than 0x38, and VI
  capacity checks plus kernel write-permission admission use the same shared
  size constant. Regression checks cover the exact header and object-table bytes
  as well as the one-byte-short output rejection.
  A real Snipperclips rerun crosses the previous `0x272` conversion abort and
  reaches `IHOSBinderDriverRelay` command 1 after 199,100,150 instructions
  (PC `0x7102649744`). This proves native-window parsing progresses into Binder
  reference management; it does not prove full layer creation or presentation.
  The next boundary is Binder AdjustRefcount with authenticated producer identity
  and retained strong/weak ownership, not a success-only stub. All 102 Horizon
  tests, both CLI tests, whole-workspace tests and whole-workspace/all-targets
  type checking pass after this correction (existing warnings remain).
  Binder relay AdjustRefcount (command 1) now decodes signed producer/delta and
  weak/strong selector, authenticates the owner against the calling process and
  retains separate checked u32 counters on the actual producer. Invalid selectors,
  short arguments, buffers/input handles, foreign IDs, underflow and overflow
  reject before changing reference state. The command ABI and selector mapping
  are checked against [libnx's Binder implementation](https://github.com/switchbrew/libnx/blob/master/nx/source/display/binder.c)
  and [reference helpers](https://github.com/switchbrew/libnx/blob/master/nx/include/switch/display/binder.h).
  Removing a layer drops its service-owned retention; this owner-scoped model
  keeps the detached producer/queue while either client count is nonzero, then
  removes its non-recycled identity on the last release. Process termination
  releases its remaining producers. Cross-process Binder sharing, full Android
  weak-reference promotion/death notifications and VI CloseLayer remain incomplete.
  Registry regressions verify retained buffer bytes after detachment, independent
  counts and transactional rejection, and VI service tests cover argument failures.
  The existing native TLS/move-handle regression now exercises balanced weak and
  strong adjustments through the real relay handle and checks CMIF replies/state.
  All 104 Horizon tests and whole-workspace/all-targets type checking pass.
  A real Snipperclips rerun crosses command 1 and stops at relay GetNativeHandle
  (command 2) after 199,166,302 instructions, PC `0x7102649744`. Next implement
  the producer's actual buffer-availability event and guest copy-handle lifetime;
  no rendered frame, interactive menu or complete Binder implementation is claimed.
  Relay GetNativeHandle (command 2) now validates the producer's owner, its
  signed identity, eight-byte arguments, absent buffers/input handles and selector
  0x0f. The selector is used by [libnx native-window initialization](https://github.com/switchbrew/libnx/blob/master/nx/source/display/native_window.c),
  and [libnx Binder transport](https://github.com/switchbrew/libnx/blob/master/nx/source/display/binder.c)
  requests a copy handle. VI returns the actual queue availability event, not an
  always-signaled placeholder. The kernel materializes native copy handles even
  for domain replies; cloned handles and suspended waits retain the same shared
  event identity independently of the originating session or handle slot.
  Availability currently means connected with at least one free attached buffer:
  attach/connect/cancel/release publish it; dequeuing the last eligible buffer
  clears it. Empty queues are unsignaled, and destruction clears retained event
  state. This matches the current real-backing queue model, not a complete
  allocation/fence-aware Android dequeue policy. Guest reset/clear-event SVCs,
  Binder buffer transactions, nvmap allocation and fence integration remain
  incomplete. Queue, service and native/domain TLS regressions cover transitions,
  selector/owner rejection, copy-handle identity, close survival and wait wakeup.
  All 107 Horizon tests and whole-workspace tests pass; workspace/all-targets
  type checking and changed-file formatting checks also pass (existing warnings).
  The real Snipperclips rerun crosses GetNativeHandle and reaches relay
  TransactParcel (command 0) after 199,234,103 instructions, PC `0x7102649744`.
  Next validate parcel framing/interface/transaction identity and implement the
  actual requested producer operation. No graphical frame is verified.
  VI relay TransactParcel/TransactParcelAuto (0/3) now route bounded input/output
  buffers to a producer-owned Binder dispatcher. It validates the signed identity,
  authenticated owner, synchronous flags, parcel bounds and UTF-16 interface token
  `android.gui.IGraphicBufferProducer`. StrictMode policy is read as caller metadata,
  not an operation or identity. Native framing/input handles and reply write
  permission are checked before queue mutations. Unsupported operations stop with
  the inner transaction ID; no generic success fallback is used.
  The first real transaction was Connect (10). Its null-listener form now reads
  and retains API/application-control state, reports the real managed-layer size,
  identity transform and actual pending-frame count in QueueBufferOutput, then
  connects the actual empty queue. Parsing, capacity and reply serialization
  precede mutation; a duplicate connection returns Android BAD_VALUE (-22) without
  replacing the connection. The wire shape follows [libnx buffer producer code](https://github.com/switchbrew/libnx/blob/master/nx/source/display/buffer_producer.c),
  and duplicate semantics follow [AOSP BufferQueueProducer](https://android.googlesource.com/platform/frameworks/native/+/refs/tags/android-5.0.0_r7/libs/gui/BufferQueueProducer.cpp).
  Listener callbacks, disconnect, abandoned-layer status, Android allocation/
  dequeue-limit policies and other producer transactions remain incomplete.
  No buffer is fabricated or made available merely by connecting an empty queue.
  Two Binder regressions cover interface/owner/flag/buffer rejection, malformed or
  short Connect arguments/replies, exact metadata, retained connection, duplicate
  rejection and both relay command values at the dispatcher level. Horizon has
  109 passing tests. Whole-workspace tests also pass.
  The real Snipperclips rerun accepts Connect and stops at VI
  SetLayerScalingMode (2101) after 199,277,357 instructions, PC `0x7102649744`.
  Implement that retained layer policy next; graphics submission/presentation and
  playable menus remain unverified.
  VI SetLayerScalingMode (2101) now decodes u32 mode plus ABI padding and the
  full u64 layer identity, authenticates the process owner and retains a typed
  policy on that actual managed layer. The supported modes are None (0),
  FitToLayer (2) and PreserveAspectRatio (4), following [libnx VI definitions](https://github.com/switchbrew/libnx/blob/master/nx/include/switch/services/vi.h)
  and [its command framing](https://github.com/switchbrew/libnx/blob/master/nx/source/services/vi.c).
  Other mode values stop explicitly rather than silently selecting a transform.
  Unconfigured policy is represented separately from requested None. This is
  retained compositor policy, not applied scaling or proof of presentation.
  Short arguments, buffers/input handles, unknown/full-width foreign layer IDs
  and foreign owners reject before mutation; existing layer dimensions and buffer
  backing are not rewritten by a policy update. Service and native/domain TLS
  regressions cover the three modes, invalid inputs, exact replies and policy
  lifetime beyond session closure. All 111 Horizon tests, whole-workspace tests,
  workspace/all-targets type checking and changed-file formatting checks pass
  (existing warnings remain).
  A real Snipperclips rerun crosses SetLayerScalingMode and stops at nvdrv Open
  (command 0) after 199,319,220 instructions, PC `0x7102649744`. Next identify the
  actual requested device path and implement its typed descriptor/lifetime before
  the corresponding IOCTLs. No GPU submission, graphical frame, menu or gameplay
  is verified by reaching this driver boundary.
  nvdrv Open/Close now retain typed, monotonically allocated per-client device
  descriptors for nvmap, host control, GPU control, GPU address spaces and GPU
  channels. Paths are bounded and validated, initialization authenticates the
  owner, stale descriptors return BadParameter, and supported session clones
  share the same registry until their last endpoint closes. IOCTL calls resolve
  the actual descriptor and stop with its device path and full request word;
  no device operation is replaced by generic success. Native argument truncation
  and unexpected input handles reject before device mutations. SetAruid also
  rejects callers different from the authenticated initialization owner.
  All 113 Horizon tests pass, including descriptor isolation/nonreuse, malformed
  paths, IOCTL routing and retained device state across extended session clones.
  The real Snipperclips rerun crosses Open and reaches GPU GetCharacteristics
  (`0xc0b04705`) on `/dev/nvhost-ctrl-gpu` after 199,321,541 instructions,
  PC `0x7102649744`. Next implement its bounded ABI and an explicit emulated GPU
  capability profile, then follow the actual allocation/submission requests.
  This remains driver initialization, not evidence of a rendered frame.
  On 2026-10-06 GPU GetCharacteristics now returns the explicit original-Switch
  GM20B hardware identity through bounded IPC writeback. The 160-byte little-endian
  identity follows [libnx's guest structure](https://github.com/switchbrew/libnx/blob/master/nx/include/switch/nvidia/ioctl.h),
  not host Vulkan capabilities. Its 16-byte wrapper preserves address metadata
  without dereferencing it and reports the actual structure size. Invalid address
  metadata or insufficient requested size returns BadParameter without output;
  malformed/truncated IPC buffers reject explicitly. AutoSelect's inactive
  zero-sized descriptors are skipped while preserving original writeback indices.
  Regression coverage checks the inline/InOut and separate input/output layouts,
  inactive descriptors, exact identity offsets and rejected capacities/metadata.
  All 114 Horizon tests, whole-workspace tests and all-targets type checking
  pass after the AutoSelect adjustment; existing warnings remain.
  The real rerun accepts the characteristics response and stops at GetTpcMasks
  (`0xc0184706`) on `/dev/nvhost-ctrl-gpu` after 199,326,068 instructions,
  PC `0x7102649744`. GPU execution, frame presentation and gameplay remain unverified.
  GetTpcMasks (`0xc0184706`) now derives the active mask from the same GM20B
  topology: two TPCs in GPC 0 and no GPC 1. The 24-byte inline structure follows
  [libnx's query ABI](https://github.com/switchbrew/libnx/blob/master/nx/source/nvidia/ioctl/nvhost-ctrl-gpu.c).
  Requests from zero through eight output bytes preserve wrapper metadata and
  untouched trailing bytes; oversized requests return BadParameter without writes.
  Shared GPU-query framing validates exact input size, sufficient output capacity
  and AutoSelect descriptor selection while preserving writeback indices.
  All 115 Horizon tests, whole-workspace tests, all-targets type checking and
  changed-file formatting checks pass, with existing warnings.
  The real Snipperclips rerun accepts GetTpcMasks and stops at ZCullGetCtxSize
  (`0x80044701`) after 199,328,194 instructions, PC `0x7102649744`.
  Next implement the guest depth-culling context ABI consistently with the GPU
  runtime; this driver query alone does not validate rendering or gameplay.
  The output-only GPU query transport and ZCullGetInfo (`0x80284702`) now
  support the documented 40-byte original-Switch alignment/region structure from
  [libnx](https://github.com/switchbrew/libnx/blob/master/nx/include/switch/nvidia/ioctl.h).
  Only one active output descriptor is admitted; inactive AutoSelect entries
  preserve their original indices. Extra active inputs, incorrect direction,
  materialized output data and insufficient capacity reject before writeback.
  All 116 Horizon tests pass. The real-game frontier remains ZCullGetCtxSize:
  the researched emulator examples use a provisional one-byte size, which is
  not accepted here as evidence of the guest hardware context size. Determine
  the firmware/profile context requirement and connect later binding to actual
  GPU memory ownership before claiming that frontier crossed. ZCullGetInfo is
  unit-tested, not yet exercised by Snipperclips; no new frame/gameplay evidence.
  A read-only `firmware_nv_inspect` ingest example now locates the NV services
  program (`0100000000000019`) in the supplied firmware directory and attempts
  bounded, integrity-verified PartitionFS reads without exporting plaintext or
  logging keys/rights IDs. The actual 22.1.0 dump identifies its program NCA as
  `70f137c139489153e343bf8fe9f34fd6.nca`, with key generation 22. Header decryption
  succeeds, but executable-section decryption returns MissingKey: this generation
  requires key-area revision `15` (hex), while the supplied key-area entries reach
  revision `14`. Firmware-derived context inspection therefore requires a matching
  user-owned key dump. This is not a failure to decrypt the older Snipperclips
  game, and does not establish that every remaining roadmap task is blocked.
  Rechecking the firmware inspector still reports MissingKey. Independent work
  now implements nvmap Create (`0xc0080101`) with device-owned, nonrecycled object
  handles, the requested size and overflow-safe page-rounded size. It creates
  an unbacked object only; Alloc, GPU mapping and submission remain unsupported.
  Zero size, malformed buffers, foreign callers and exhausted handles reject
  without inserting objects. Closing the owning nvmap descriptor releases its
  objects. Native nvdrv IOCTL output mappings are checked writable before dispatch
  so allocation state cannot be committed ahead of failed output writeback.
  The new regression covers object ownership, upper size bounds, malformed
  requests, exhaustion and device closure; all 117 Horizon tests pass.
  The real-game frontier remains ZCullGetCtxSize, with no new game execution
  claim from this independent memory-object prerequisite.
  nvmap Param SIZE (`0xc00c0109`, parameter 1) now reads the actual retained
  page-rounded object size; other parameters remain explicit unsupported cases
  until backing/alignment/kind state exists. Free (`0xc0180105`) validates
  device ownership and its full output structure before removing the unbacked
  object. Its address and cache flags are zero because no backing was allocated.
  Unknown/stale/foreign objects return BadParameter without output or mutations.
  Create now rejects sizes rounding above u32::MAX: the earlier u64 rounding
  was overflow-safe internally but a 4-GiB result was not representable in the
  guest Param/Free response ABI. Tests cover this boundary and failed/free/stale
  lifetimes; all 118 Horizon tests pass. No actual backing allocation, GPU mapping
  or new game frontier is claimed by these memory-object lifecycle operations.
  DeviceMemoryLease now provides a prerequisite for nvmap Alloc: bounded access
  to live Heap/Data backing, without copying guest bytes or allocating a shadow
  GPU buffer. Device reservations prevent unmap, permission changes, destructive
  loading, overlapping transfer/stack alias operations and heap shrink until the
  last lease drops. Normal CPU writes remain visible to device reads, and device
  writes update the existing exclusive-monitor epoch. Read-only device leases
  reject writes; out-of-range and foreign/snapshot address-space access reject.
  Cloning guest memory creates a separate device identity and does not replay
  external device leases against copied backing. Two regressions cover coherent
  bytes, boundaries, lifetime and mapping protection; all 120 Horizon tests pass.
  This primitive is not yet wired to nvmap Alloc or GPU submission. The real-game
  frontier and firmware key requirement remain unchanged.
  nvmap Alloc (`0xc0200104`) is now dispatched with the authenticated caller's
  actual AddressSpace. Its 32-byte structure validates object/device ownership,
  single allocation, flags, power-of-two alignment and a nonzero aligned address.
  User-backed allocations retain DeviceMemoryLease over the page-rounded range;
  alignment, kind, cache flags and heap-mask metadata are retained separately.
  No heap is invented or copied. Read-only/RW access follows the allocation flags,
  and failed range admission leaves the object unallocated. Param ALIGNMENT/KIND
  now reads retained backing metadata and rejects unallocated objects.
  Free returns the backing address only when no device references remain, reports
  the actual uncached flag and releases the object's lease. Outstanding device
  references continue protecting the guest range after the object handle is gone.
  A regression covers rejected addresses, live CPU/GPU coherence, duplicate Alloc,
  access flags and Free with outstanding references; all 121 Horizon tests pass.
  GPU virtual mapping/submission and driver-owned allocations remain incomplete;
  Snipperclips has not exercised this path yet and its frontier is unchanged.
  A native HIPC regression now exercises nvmap Open, Create and Alloc through
  TLS descriptor parsing and kernel dispatch, rather than only direct service
  calls. A read-only response mapping rejects Alloc before object mutation or
  device pinning; restoring write permission permits the same request. An actual
  kernel session clone retains the allocated backing after the root closes, and
  closing the final endpoint releases the pin so the guest range can be unmapped.
  This verifies transport admission and canonical-server allocation lifetime;
  it does not claim game execution or GPU virtual-address submission coverage.
  NvGpuAddressSpace now maps bounded, page-aligned guest GPU virtual ranges to
  actual nvmap backing leases and buffer offsets. It enforces the declared 40-bit
  guest VA limit, rejects overlap/overflow/backing-range violations and supports
  live CPU/device reads and writes without copying resources. Separate GPU aliases
  retain the same backing; unmapping one alias cannot release memory still used
  by another. Foreign address-space snapshots reject through lease identity.
  The regression covers alias coherence, exact offsets, invalid mappings, crossing
  access bounds and last-unmap lifetime. All 123 Horizon tests pass. This is
  device-side ownership, distinct from GPU IR's stable backend-resource mapping;
  nvhost-as-gpu IOCTL reservation/initialization and MapBufferEx integration remain
  to be wired. It does not advance the unchanged real-game ZCullGetCtxSize frontier.
  Opening `/dev/nvhost-as-gpu` now creates a real per-descriptor NvGpuAddressSpace;
  Close drops that state. InitializeEx (`0x40284109`) parses the 40-byte input-only
  [libnx ABI](https://github.com/switchbrew/libnx/blob/master/nx/source/nvidia/ioctl/nvhost-as-gpu.c),
  authenticates client ownership and retains flags plus the guest big-page policy.
  The default resolves to the declared 128-KiB GM20B page size; 64/128-KiB explicit
  selections are accepted. Unsupported flags, inherited descriptors/custom ranges,
  wrong framing and duplicate initialization reject before policy mutation.
  Reserved wrapper padding is ignored according to the reference client framing.
  Tests cover default/explicit policies, failed admissions, foreign ownership,
  duplicate calls and closure. All 124 Horizon tests pass. VA reservation,
  MapBufferEx, channel binding and command execution still need integration;
  Snipperclips has not reached this new path and its actual frontier is unchanged.
  Fixed AllocSpace (`0xc0184102`, flags=1) and FreeSpace (`0xc0104103`) now
  route to initialized, descriptor-owned GPU VA reservations. They validate
  page policy, nonzero counts, alignment, checked bounds and overlap before
  insertion; FreeSpace requires an exact reservation and rejects live mappings.
  Four-KiB pages and the configured big-page size are admitted. Automatic/sparse
  reservations remain explicitly unsupported until allocator/range policy exists.
  The inline reply is prepared before mutation and output permissions remain
  checked by kernel transport. Component and service regressions cover fixed
  reservation/release, page-policy violations, overflow, duplicates, stale frees
  and mapped-range lifetime. All 125 Horizon tests pass; no changed game frontier
  or rendering validation is claimed by this driver memory-management work.
  Fixed MapBufferEx (`0xc0284106`, flags=1) now routes allocated client-owned
  nvmap backing into actual reserved GPU VA ranges, checking the full 40-byte
  structure, reservation/page policy, buffer offsets, sizes and overlap. A zero
  mapping size resolves to the actual remainder of the object; requested page
  size zero resolves to the reservation policy and is returned in the reply.
  Default/current backing kinds are accepted; kind reinterpretation and automatic,
  cacheable or remapping variants remain explicit unsupported cases. UnmapBuffer
  (`0xc0084105`) removes only an existing exact mapping and releases its lease.
  The service regression exercises Create/Alloc/Open/Initialize/Reserve/Map/Free/
  Unmap: CPU writes are visible through the mapped GPU subrange, freeing nvmap
  cannot release backing still mapped to the GPU, and final Unmap permits the
  underlying guest memory to be unmapped. Invalid oversize/duplicate/stale requests
  reject. All 126 Horizon tests pass. Channels/submission remain unwired, with
  no new Snipperclips execution frontier, graphical frame or gameplay evidence.
  BindChannel (`0x40044101`) now validates its input-only fd structure, an
  initialized GPU address space and an actual client-owned GPU channel, then
  binds the channel to the canonical mutable space. Uninitialized/wrong-type
  channels and duplicate bindings reject without replacing ownership. Multiple
  channels share live mappings, not snapshots; closing the AS descriptor prevents
  further fd operations but retains its space while channels still reference it.
  Closing the final bound channel releases a space whose fd was already closed.
  The regression verifies owner/framing rejection, duplicate binding, live shared
  mappings and backing pin lifetime across AS/channel closure. All 127 Horizon
  tests pass. Channel queue allocation, submission and completion remain unwired;
  no new actual-game progress or rendered frame is claimed.
  The runtime now depends on the existing GPU IR front end and exposes a
  per-channel GuestGpuDecoder bridge. It reads bounded, aligned pushbuffers
  through real NvGpuAddressSpace/nvmap leases, decodes little-endian words and
  retains Maxwell engine state across buffers. Full memory-range admission
  precedes state updates; malformed PFIFO packets reject through the front end.
  The new runtime regression reads live CPU-written SET_OBJECT/BEGIN commands,
  rejects invalid ranges/oversized/truncated buffers without losing state, then
  decodes a later draw using the retained engine binding. All 115 runtime tests
  pass. This produces typed/opaque traces only: channel IOCTL scheduling,
  backend execution, synchronization/completion and presentation are not yet
  connected, and no success fence or graphical frame is fabricated.
  A typed GPFIFO entry decoder now follows [NVIDIA's Maxwell B06F header](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/main/src/common/sdk/nvidia/inc/class/clb06f.h):
  full 40-bit dword-aligned addresses, 21-bit word counts and independent fetch,
  privilege, level and synchronization flags. Zero-length entries retain control
  opcode/operand metadata instead of being misread as empty command buffers;
  reserved low address bits reject. Two GPU IR regressions cover boundaries and
  lossless control handling; all 60 GPU IR tests pass.
  The runtime bridge now reads aligned descriptors from actual mapped GPU memory
  and follows supported unconditional/user/main/proceed pushbuffers through the
  existing front end. Unsupported control/fetch/privilege/subroutine/wait modes
  reject before channel-state mutation. The runtime regression now uses an actual
  descriptor and checks those rejection paths. This is bounded command ingestion,
  not queued submission, backend execution or fence completion; the real-game
  frontier remains unchanged and no image/menu/gameplay is verified.
  GPU reads/writes now resolve complete ranges across contiguous mappings rather
  than requiring one mapping per access. Every segment's lease identity, backing
  bounds and access rights are checked before copying/writing any bytes, so later
  holes, read-only segments or foreign address spaces cannot cause partial writes
  or partially altered output. A new Horizon regression covers adjacent live
  backing, read-only/hole rollback and successful cross-page writes; all 128
  Horizon tests pass. The existing runtime regression now fetches a pushbuffer
  spanning two distinct GPU mappings from its real GPFIFO descriptor. Submission,
  rendering and the unchanged Snipperclips frontier remain unverified.
  FrontEndTrace now retains a common indexed event sequence in addition to the
  per-engine diagnostic vectors. Interleaved graphics, compute, DMA, inline,
  2D and unknown-engine emissions no longer require consumers to reconstruct
  their order from separate lists. Graphics macro emissions are recorded at the
  existing invocation boundaries; cross-buffer macro lifetime and asynchronous
  completion semantics still require execution-level validation. A regression
  checks repeated, interleaved engine events and preservation of opaque writes.
  All 61 GPU IR and 115 runtime tests pass after this change. This is an execution
  prerequisite, not actual command execution or a new Snipperclips frontier.
  Canonical trace lowering now traverses that common event sequence instead of
  processing all graphics events before all DMA events. A regression verifies
  barrier/copy/draw ordering and counts still-unimplemented engine events as
  opaque coverage rather than silently implying their support. GPU IR (61) and
  runtime (115) tests pass. Macro invocation boundaries, actual DMA layout/flags,
  resource offsets and backend execution remain separate incomplete requirements;
  this ordering correction does not establish a rendered frame or game progress.
  Buffer-copy commands now carry independent source/destination offsets through
  canonical IR, Vulkan plan lowering, the host transfer executor and both native
  Vulkan recording paths. DMA address resolution preserves mapping/subrange
  offsets instead of copying from resource byte zero. Range validation checks
  both offset-plus-length bounds before execution, rejects overlapping same-buffer
  regions, and treats exact self-copies as no-ops consistently. The native rule
  follows [Vulkan's buffer-copy requirements](https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdCopyBuffer.html).
  GPU mapping admission also rejects resource-offset overflow before insertion.
  Tests cover nonzero mapping offsets, canonical-to-plan-to-host subrange copies,
  untouched surrounding bytes and rejection of overlapping/out-of-bounds/overflow
  requests without mutation. Whole-workspace tests pass. Native recording code is
  compiled, not device-executed; guest DMA flags/layout and actual game rendering
  remain incomplete, and the Snipperclips frontier is unchanged.
  DMA lowering now validates launch transfer type, pitch layouts and multi-line
  mode against [NVIDIA's B0B5 class header](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/main/src/common/sdk/nvidia/inc/class/clb0b5.h).
  Transfer NONE/reserved types, block-linear modes, remapping, physical addressing,
  semaphore/interrupt/cache effects and unknown flags are not converted into plain
  copies. Supported pitch-layout launches expand into ordered per-row copies with
  independent strides; disabling multi-line uses one row regardless of LINE_COUNT.
  All rows resolve with read/write permissions before any canonical copy is emitted,
  so a later invalid row rejects the whole launch. Expansion above 65,536 rows is
  explicitly unsupported rather than truncated. Tests cover launch bits, row offsets
  and later-row rejection; all 62 GPU IR tests and whole-workspace tests pass.
  This establishes supported command translation, not queue submission, completion,
  native-device execution or a new Snipperclips frontier. Remaining DMA modes and
  the unchanged initialization query still require implementation.
  The runtime now exposes synchronous execution of admitted non-pipelined virtual
  pitch-layout DMA launches against real NvGpuAddressSpace/nvmap leases. A shared
  copy-ranges primitive validates every source/destination row before mutation,
  detects overlapping CPU backing even through unrelated GPU aliases, stages all
  sources and writes through the coherent guest memory API. Staging is bounded to
  64 MiB and row expansion to 65,536; oversized transfers reject, not truncate.
  Pipelined launches and unimplemented side effects remain explicitly unsupported.
  A runtime regression reads and decodes an actual DMA pushbuffer from guest GPU memory,
  executes independently pitched rows (including a source spanning two mappings),
  checks CPU-visible bytes and untouched gaps, and rejects later-row faults and
  backing aliases before mutation. All 116 runtime and 128 Horizon tests pass.
  Channel submission does not call this executor yet; no fence, rendered frame,
  game-menu validation or new Snipperclips frontier is claimed.
  Macro execution failures are now explicit typed graphics events: missing uploaded
  programs, unsupported instructions, incomplete programs, step-budget exhaustion,
  emission truncation and recursive macro emissions no longer disappear silently.
  Valid emitted prefixes remain diagnostic evidence; translation counts a macro
  failure and stops before following commands rather than treating the remainder
  as a successfully translated stream. Tests cover missing/opaque/truncated/budget
  cases, a complete exit including its delay slot, and failure ordering before a
  following draw. All 64 GPU IR tests and whole-workspace tests pass. These failure
  reports do not complete MME coverage, invocation-boundary semantics or channel
  fault handling, and do not advance the real Snipperclips frontier.
  Runtime trace execution now traverses the common event order and invokes live
  synchronous DMA execution for supported launches. Unknown/opaque/graphics or
  other unimplemented events stop execution explicitly with the event position
  and count of already-applied transfers; following commands are not executed.
  Public trace validation requires every per-engine event exactly once and in
  local order, rejecting missing, duplicated and out-of-range indices before any
  mutation. The live-pushbuffer regression now exercises this ordered executor,
  verifies a successful transfer prefix followed by an unsupported draw leaves a
  later destination untouched, and checks malformed-trace rejection before the
  first write. Whole-workspace tests pass. This is not yet connected to native
  nvdrv submission/channel fault policy and does not produce a completion fence,
  frame, game menu or new Snipperclips frontier.
  Native nvdrv channel SetNvmapFd (`0x40044801`) now admits the documented
  four-byte input layout from [libnx's channel wrapper](https://github.com/switchbrew/libnx/blob/master/nx/source/nvidia/ioctl/nvchannel.c).
  The descriptor must identify a live nvmap device in the authenticated client;
  wrong kinds, malformed directions/sizes and foreign callers reject before
  binding. Repeated binding remains explicitly unsupported, not silently replaced.
  Channels retain the canonical nvmap owner: closing the map fd preserves its
  objects while a channel still references it, and closing the last reference
  releases those objects (GPU mapping leases remain independently owned).
  A regression binds two channels and verifies actual memory pins survive fd and
  first-channel closure, then release after the last channel closes. All 129
  Horizon tests pass. This establishes a submission prerequisite, not allocated
  GPFIFO queues, syncpoint/fence behavior or a new Snipperclips execution frontier.
  Rechecking local key names confirms both application key-area and master-key
  revisions still stop at hex `14`; the firmware's required revision `15` is not
  locally available even as a master-key derivation input. No key values were
  logged, and this remains a local firmware-inspection blocker, not a global impasse.
  Runtime GuestGpuChannel now provides a bounded software descriptor queue with
  whole-batch admission, stateful decoding and ordered synchronous execution.
  Pushbuffer bytes and canonical GPU mappings are read at consumption time;
  no command-byte snapshot is taken at enqueue. A decode/execution fault retains
  the head and stops further consumption/admission, preventing automatic replay
  of an already-applied transfer prefix. Tests verify a DMA-written later command,
  retained engine state, capacity/invalid-batch admission and durable fault-stop
  without repeated memory effects. All 117 runtime tests pass. The software
  capacity limit is not a claimed hardware queue size; nvdrv allocation/submission,
  guest error notification and syncpoint/fence integration remain unimplemented.
  This does not advance the Snipperclips initialization frontier or validate rendering.
  A per-channel GuestGpuExecutor now retains inline-upload progress across queued
  descriptors and writes words directly through real GPU mappings into coherent
  CPU backing. Launch flags follow [NVIDIA's A140 header](https://github.com/NVIDIA/open-gpu-doc/blob/master/classes/inline-to-memory/cla140.h).
  The implemented path is a single pitch-layout line with a nonzero dword-aligned
  length, bounded to 64 MiB, without flush/interrupt/semaphore/reduction effects.
  Multi-line, partial-dword and block-linear modes remain unsupported. Destination
  access is preflighted over the entire upload before accepting a launch. Excess
  data or replacing an unfinished upload fails explicitly; standalone execution
  reports an unfinished upload instead of silently dropping its state. Completion
  counts cover whole transfers only, not already-visible partial upload bytes.
  A live queue regression splits an upload over two descriptors and two GPU
  mappings, checks exact little-endian CPU-visible bytes and rejects excess data
  and unsupported launch flags without neighboring-memory changes. All 118 runtime
  tests pass. Native nvdrv submission/fences and the real Snipperclips initialization
  frontier are still unchanged; this does not establish a graphical frame.
  MME parameter fetching no longer substitutes zero for absent invocation words.
  Direct loads and all three fetch-and-result actions report ParameterUnderflow
  with instruction/parameter positions before that instruction changes registers,
  method state or emitted commands. Earlier valid state remains available for
  diagnostics, and the graphics front end propagates the stop as MacroFailed.
  Tests distinguish an explicitly supplied zero from absence, cover every fetch
  action and verify a later missing word preserves the preceding register value.
  All 65 GPU IR tests and whole-workspace tests pass. This is explicit incomplete
  invocation reporting, not hardware parameter-wait/resume semantics; cross-buffer
  macro lifetime, context allocation and native submission remain incomplete.
  No new Snipperclips execution or rendered frame is claimed.
  Horizon now exposes authenticated canonical channel access for runtime execution:
  process-local session handles resolve through IPC aliases, the initialized nvdrv
  owner must match the process, and live channel/nvmap/address-space bindings are
  required before borrowing mappings together with the owning process memory.
  Runtime queues provide step_bound through this access path and retain an opaque
  channel identity, refusing decoder reuse across another client/channel even when
  numeric descriptors coincide. Identity follows canonical IPC clones, not handle
  numbers or a copied mapping table. Tests cover unbound/wrong-kind/foreign-owner/
  stale-handle rejection, same-number independent clients, and access through an
  alias after original-session closure with CPU-visible memory writes. All 130
  Horizon tests pass. The driver still does not automatically allocate/enqueue or
  consume submissions through this bridge; syncpoints, error notification and
  the unchanged Snipperclips initialization frontier remain incomplete.
  A native runtime integration regression now initializes nvdrv via guest TLS
  HIPC/CMIF with real copied process/transfer handles, then uses native Open and
  IOCTL requests for address-space initialization/channel binding/SetNvmapFd,
  nvmap Create/Alloc, fixed reservation and MapBufferEx. The runtime queue reads
  and executes a DMA pushbuffer through these actual kernel-owned mappings; exact
  copied bytes are verified in the owning process memory. Binding reuse against
  a second properly configured channel rejects without consuming the queue. A
  later native UnmapBuffer causes decode failure in the original channel, proving
  the runtime observes canonical mapping changes instead of a retained snapshot.
  All 119 runtime tests pass. Descriptors are still enqueued explicitly by the test,
  not through SubmitGpfifo/KickoffPb; this does not implement guest submission,
  syncpoint/fence completion, a graphical frame or new Snipperclips game progress.
  MME method reads now use the same dword-index units as emitted methods and
  the front-end raw-register mirror; the former extra byte-address shift caused
  nonzero reads to miss both prior emissions and the actual register key. The
  regression now checks nonzero/incremented method indices, latest-write priority,
  fallback reader keys and signed register-relative addressing instead of only
  asserting that execution did not exhaust its step budget. A second raw-program
  regression reads an actual nonzero front-end register and emits BEGIN with that
  value, verifying the state-machine integration. All 66 GPU IR tests and
  whole-workspace tests pass. Native submission, full macro/context semantics and
  the unchanged real-game Snipperclips frontier remain unverified.
  Canonical lowering now shares the runtime's complete-event-order admission.
  Missing, duplicated, out-of-order and out-of-range public trace indices reject
  before emitting any canonical commands, with an explicit invalid_traces counter
  rather than an indexing panic or an apparently successful empty translation.
  Tests cover those cases and distinguish a legitimately empty trace. Existing
  interleaving fixtures now retain valid per-engine order and remove unused event
  vectors when constructing isolated launch traces. All 67 GPU IR tests and
  whole-workspace tests pass. This validation does not complete backend execution,
  guest submission or the unchanged Snipperclips initialization frontier.
  Vertex-array and indexed draw events now retain their own immutable register
  snapshot at emission time, rather than relying on the final front-end state
  after all following writes. The snapshot preserves raw register words and
  current primitive state; unknown fields and floating-point bit patterns are
  retained exactly without conflating register capture with semantic coverage.
  A regression changes shader region/selection, target address, viewport NaN
  bits and primitive state between two draw types and verifies each keeps its
  original values. All 68 GPU IR tests and whole-workspace tests pass. Canonical
  draw lowering/backend resource and pipeline binding do not consume these
  snapshots yet; draw execution still rejects in the synchronous runtime path.
  No new actual-game frontier, pixel output or graphics completion is claimed.
  Shader entry discovery and offline compilation now have per-draw snapshot
  entry points. Snapshot program-region and pipeline-slot register words feed
  the same discovery logic as typed channel state, preserving stage/slot, register
  count, binding and raw select metadata without consulting final channel state.
  Address addition is checked in both paths instead of wrapping. Tests verify
  two successive draw types select different program addresses/compile windows,
  preserve fragment metadata and reject address overflow. All 69 GPU IR tests and
  whole-workspace tests pass. Explicit shader windows are still required; these
  tests establish entry/window selection, not successful shader translation,
  executable pipeline binding or rendering. Raw enable/stage-selection semantics
  and native submission remain incomplete, with no new Snipperclips frontier.
  Runtime GuestShaderMemory now implements the compiler's GPU-memory reader
  against canonical NvGpuAddressSpace mappings and live guest nvmap backing.
  Complete-range read access is validated before allocation; reads have an explicit
  64 MiB software budget and allocation failures retain diagnostics without
  logging payload bytes. Tests verify cross-mapping reads, CPU-written byte changes,
  invalid ranges/budgets/overflow, foreign memory snapshots and later unmaps.
  All 120 runtime tests and whole-workspace tests pass. Shader windows still need
  verified sizes, and actual game shader compilation/pipeline execution remain
  unverified; this reader does not advance Snipperclips or produce a frame.
  Graphics shader compilation now validates and indexes the full explicit window
  table before reading any bytes, rejecting conflicting lengths for one address
  and missing selected entries without reading an earlier shader first. Graphics
  and compute windows reject zero/oversized lengths and address overflow; the
  64 MiB software budget is shared with the guest-memory reader. Tests use a
  read-forbidden provider to prove invalid metadata rejects before access. All
  70 GPU IR tests and whole-workspace tests pass. This admission does not discover
  actual game shader lengths or validate executable shaders/rendering, and the
  Snipperclips initialization frontier remains unchanged.
  The strict all-target Clippy gate was exercised with `-D warnings`; it is not
  yet passing. Initial blockers in GPU IR, VFS, the Linux audio boundary and plugin
  loader documentation were corrected without suppressing lints: explicit
  non-truncating offset writes, equivalent priority sorting/default implementations,
  normalized opcode spelling, direct opaque decoding and reverse-end iteration.
  Plugin loader safety obligations now state ABI/table storage requirements.
  Coverage ratios use u128 arithmetic to avoid overflowing valid large counters
  or their aggregated totals; a new regression covers empty/full/near-half ratios.
  All 71 GPU IR tests and whole-workspace tests pass. The next strict Clippy run
  reaches remaining CPU IR, ingest and Horizon issues, so the combined quality
  milestone remains unchecked. No game/frontier/rendering milestone is advanced.
  No guest networking, frame presentation or game menu is verified.
  Scalar FMUL, FCMP, FCVTZU, FSQRT, FADD/FSUB, FDIV, FNEG,
  paired exclusives, CNTPCT_EL0 and vector ORR are crossed. The previous FNEG
  boundary, GetInfo core/priority-mask reporting, SetThreadCoreMask and
  SetThreadPriority, CreateTransferMemory and the previous time:u opening boundary
  are crossed. nvdrv opening, Initialize and CloneCurrentObjectEx are also crossed.
  VI root factories and application display objects are typed, with privilege
  checks, balanced display opens/closes and resolution reads from existing display
  metadata. Binder relay/indirect and system/manager objects retain their types,
  but their commands remain unsupported; layers, native windows, vsync and actual
  frame presentation are incomplete. Service opening is not game-menu validation
  and does not prove every clock command or time initialization complete.
  Diagnostic snapshots at 3 million and 100 million steps show changing symbol/module
  search state; nearby rtld PCs did not indicate a frozen loop. The thread runner now
  borrows guest memory once per CPU slice, retaining precise SVC/error/budget boundaries.
  QueryMemory has been crossed successfully. SDK entry does not prove complete process
  initialization, arrival at the game main loop, or gameplay.
- Whole-workspace offline tests and all-targets type checking were rerun after
  the extended IPC clone changes and pass
  (type checking retains existing warnings).
  TLS, remaining SIMD/FP, remaining scalar semantics,
  scheduler integration and remaining SVC dispatch are not yet complete.
- Both native Vulkan triangle commands fail at instance creation with incompatible
  driver. This environment exposes no `/dev/dri` and only an NVIDIA ICD manifest;
  native rendering is not verified here.

## Required milestones

### Event object's stored invalid handle (2026-10-10)

A bounded event-metadata probe at NvOsEventDestroy identifies object
0x2000da6dd0 with kind 1. Its two list links both point to object+16;
object+32 contains 0x10101 and object+36 contains 1. The SDK destroy routine
receives object+16, reads its ownership flag at +18, then its handle at +20:
the latter is exactly enclosing object+36. Thus the invalid handle is already
stored in the event object before CloseHandle; it is not introduced by syscall
error handling. Self-linked list metadata alone does not validate complete
initialization, nor prove which writer supplied the handle.

Next investigation is the writer/attachment path for enclosing object+36,
compared with the actual transferred handle 0x10002c. The provisional GPU probe
still terminates with 0xe401 after 199,553,018 instructions. Its temporary event
instrumentation and GPU overrides were removed; no game rendering is verified.

### Event transfer versus failed close (2026-10-10)

A temporary identifier-only trace on the provisional GPU probe records exactly
one ReadableEvent copy, handle 0x10002c. Earlier guest CloseHandle calls use
0x100002, 0x10000a and 0x100016; the failing close uses 1, not the copied event
handle. Therefore this run does not show the kernel returning handle 1 from
QueryEvent. Corruption, event initialization/layout or a different event remain
possible; none is established by this observation.

Bounded game caller metadata shows NvOsEventDestroy selects event kind 1 and
passes its object+16 to the SDK destroy routine. Its caller loads that event
pointer from object+936 (subsdk2+0x32160). This provides a concrete next tracing
target: event creation/attachment and that object's stored handles, rather than
changing CloseHandle to accept 1. Termination remains 0xe401 at 199,553,018
instructions. All temporary trace statements and GPU overrides were removed;
there is still no verified game image or gameplay.

### Invalid event-handle caller identified (2026-10-10)

Another explicitly provisional GPU probe reproduces termination 0xe401 after
199,553,018 instructions. Symbol-only abort frames identify the failing close
as sdk InterProcessEventImplByHorizon::Close, called by
DestroyInterProcessEvent and subsdk2 NvOsEventDestroy (subsdk2+0xb178), then
subsdk2+0x3216c and +0x32268. This narrows handle-1 provenance to event teardown;
it does not yet prove QueryEvent returned that value or explain why teardown
started. Recent IPC ends with nvdrv Close followed by application termination.

An independent literal-byte regression now checks the one-copy-handle response
layout against the primary libnx hipc.h definition, without reusing our parser:
special header at byte 8, handle at byte 12, aligned CMIF at byte 16. It passes.
The temporary ZCull and VA overrides were removed again. No production GPU
policy, game frame, menu or gameplay milestone is advanced by this experiment.

### Guest CloseHandle result transport (2026-10-10)

The expanded diagnostics identify the provisional Snipperclips stop as SVC
0x16 (CloseHandle), with input handle 1, at PC 0x71026496cc after
199,481,106 instructions. Guest dispatch now returns InvalidHandle (0xe401)
for an invalid or stale handle instead of terminating host execution. It does
not accept that handle or suppress cleanup failures; direct host dispatch
retains its strict error behavior. Regression tests cover malformed handles,
slot reuse, surviving objects, missing processes and continuation through the
actual guest SVC instruction with unrelated CPU state preserved.

A subsequent provisional probe reaches 199,553,017 instructions and PC
0x710264979c: the game receives InvalidHandle, then explicitly terminates with
0xe401 through SVC 0x26. The underlying origin of handle 1 is still unresolved.
This corrects error transport, not the game's initialization failure. Candidate
ZCull=1 and provisional VA bounds were removed after both diagnostic runs;
production still has the unresolved ZCull/default-VA frontier. No game image,
menu, audio or gameplay has been verified.

Workspace offline tests, strict all-target Clippy and formatting were rechecked
successfully. Native opt-in tests remain outside this validation. Updated local
revision-15 keys also pass the firmware 22.1.0 metadata inspector; keys are no
longer the blocker. Next: identify the guest caller and handle provenance, while
keeping provisional GPU assumptions separate from verified firmware behavior.

### Real-game automatic VA reservation frontier (2026-10-10)

A temporary diagnostic probe with the same candidate ZCull=1 and provisional
40-bit VA split confirms the real game passes CNT and UADDLV. It reaches
199,413,662 instructions, PC 0x7102649744, then the previous unsupported
automatic/sparse reservation branch. Production now implements non-sparse
automatic AllocSpace (flags 0) alongside fixed requests (flags 1).
The [libnx ioctl implementation](https://github.com/switchbrew/libnx/blob/master/nx/source/nvidia/ioctl/nvhost-as-gpu.c)
defines the final eight-byte field as input alignment / output GPU offset;
the caller-facing [address-space wrapper](https://github.com/switchbrew/libnx/blob/master/nx/source/nvidia/address_space.c)
passes page-size alignment for automatic requests.

Allocation uses deterministic first-fit within an already established page-class
region, excludes reservations and standalone/nested mappings, and checks
alignment, exhaustion and arithmetic before mutation. It does not establish or
invent firmware default bounds. Sparse/unknown flags remain explicit errors.
The IPC reply preserves the header/padding and writes the selected VA to the
actual output descriptor; invalid ownership or output capacity cannot reserve VA.
Tests cover reuse, page classes, mapping overlap, invalid requests, output
descriptor selection and 256 occupancy patterns against a bitmap reference.

With this allocator, a second temporary real-game probe advances to 199,480,928
instructions and PC 0x71026496cc, stopping on invalid/stale handle 1. This is
evidence that the previous reservation rejection is passed, not proof of a game
frame or correct provisional GPU policy. Both overrides were removed and the
normal CLI rebuilt. The next diagnostic run can use the expanded CLI metadata:
stopped SVC number/arguments are now reported for kernel errors as well, and the
bounded recent IPC history is reported for all stops, not only guest aborts.
The failing handle's exact syscall/provenance is not yet identified.

All 159 Horizon tests and workspace tests pass; the 28 native opt-in tests remain
ignored. Strict workspace/all-target Clippy and formatting pass. Production's
unresolved ZCull/default-VA frontier remains distinct from these provisional
probe frontiers; no actual image/audio/menu/gameplay is claimed.


### Real Snipperclips diagnostic frontier: CNT and UADDLV (2026-10-10)

Two explicitly temporary probes used candidate ZCull size 1 and a provisional
default VA policy: small pages [0x4000000, 1<<39), big pages [1<<39, 1<<40).
These values were diagnostic hypotheses, not established firmware defaults.
They allowed the real game to pass the previous GetVARegions frontier:

- Before the CPU change: 199,393,507 instructions, PC 0x71024545f8,
  unsupported opcode 0x0e205800 (CNT V0.8B,V0.8B).
- With CNT implemented: 199,393,125 instructions, PC 0x71024545fc,
  unsupported opcode 0x2e303800 (UADDLV H0,V0.8B). Instruction totals vary
  between runs and do not indicate a regression; the second PC advances by 4.

CNT is now implemented for 8B/16B arrangements with independent byte counts,
alias-safe reads and upper-half clearing. UADDLV now supports 8B/16B, 4H/8H
and 4S, widening the scalar sum and clearing unused destination bits. Reserved
arrangements fail before mutation; signed reductions remain unsupported. LLVM's
AArch64 assembler independently verified the encodings. Tests cover all byte
inputs, aliases including V31, maximum sums, other-register/flag preservation,
and the actual observed CNT/UADDLV sequence through the normal step decoder.
UADDLV has not yet been re-probed in the real game.

Both temporary GPU changes were removed after each probe; the normal CLI was
rebuilt without overrides. Production still rejects unresolved default VA
queries and ZCullGetCtxSize. No real image/audio/menu is claimed. Runtime has
144 passing tests; all workspace tests pass (28 native opt-in tests ignored),
the inspector's 71 tests pass, and strict workspace/all-target Clippy plus
formatting pass. The next real diagnostic probe should verify UADDLV and expose
the following operation while continuing to resolve the production GPU policy.

The firmware source investigation also establishes that 0x7d3d4 calls 0x9b3a0
with X0=1 and X1=1 in two locally reaching paths from 0x7d3b0. That callee has
conditional paths and a nested call; its X1 return/clobber behavior is not yet
proven, so the later Input(1) must not be treated as a preserved source pointer.


### ZCull upstream return-path pruning (2026-10-10)

The opt-in entry probe accepts `EXOSPHERE_NV_CALL_PROBE` for an exact queried
call. Local reverse reachability now excludes paths that terminate before that
call, including direct/indirect calls followed only by RET. Reachable calls remain
unsupported; unknown indirect tail branches conservatively remain barriers.
Explicit windows are capped at 1024 bytes and 512 explored states, with separate
metadata explaining reachable call/loop barriers. External successors are outside
this local proof; the selected start is still not a proven function boundary.

Fresh firmware metadata for start 0x7d3dc and query 0x7d628 follows the earlier
conditional predecessor across a 588-byte window. One local path reaches
0x7d628 -> 0x7a1e0, with X0=Input(19), X1=Input(1), X2=Input(2), no unmodeled
operations and no reachable barriers. Starting earlier at 0x7d3c0 still encounters
calls/backward edges, so argument provenance must be traced before the newly
selected window; the source template and ZCull cached value remain unproven.
Tests cover live/dead direct and indirect calls, unknown tail branches and long
forward jumps. All 71 inspector tests, workspace strict Clippy and formatting
pass. No production response or real-game frame is admitted.


### ZCull upstream window-boundary correction (2026-10-10)

An opt-in metadata-only `EXOSPHERE_NV_ENTRY_PROBE=0x7d5f4` inspection checks
aligned raw pointer locations in rodata/data, neighboring in-text pointer
candidates, preceding transfer kind and all incoming direct edge kinds.
Neither data segment contains a raw aligned pointer to that address. Crucially,
the preceding instruction is a return, but an earlier conditional edge
0x7d3e8 -> 0x7d5f4 reaches the address from outside the 60-byte argument window.
Thus the assumed suffix entry is not an established function entry: the previous
absence of direct callers must not redirect the investigation exclusively toward
indirect tables. The next provenance search must cover this earlier conditional
predecessor and distinguish paths ending in the intervening return from paths
reaching 0x7d628. No cache value or production ZCull response follows from the
suffix's Input(1) expression. Pointer-scan tests cover absolute alignment,
partial words and overflow; transfer metadata tests distinguish returns/calls.
All 69 inspector tests, strict workspace/all-target Clippy and formatting pass.
Snipperclips' actual-game frontier remains unchanged.


### ZCull template-copy upstream path joins (2026-10-10)

The inspector can now join bounded forward paths to an exact direct call.
Unknown flags explore both conditional successors; disagreeing register values
become Unknown. Loops, intervening calls/indirect transfers, oversized windows,
and excessive path growth reject the probe. The selected window entry is an
explicit assumption, not proof of a function boundary or global dominance.
Unsigned UBFM extraction/zero-insertion is also modeled with exhaustive immediate
tests across 32/64-bit widths; signed and destination-merging forms remain excluded.

Fresh NV inspection finds two reaching paths from assumed entry 0x7a1e0 to
0x7a210 -> 0x8ba90; both preserve incoming X0/X1. Its direct caller is
0x7d628 -> 0x7a1e0. The upstream bounded window starts at 0x7d5f4 and now
resolves X0 to incoming X19 while X1 remains incoming X1, with no unmodeled
operations. No direct call or unconditional branch targets that assumed entry
in the complete text scan. This redirects the next source search toward indirect
references, without proving the source template or cached ZCull value.
All 67 inspector tests, workspace tests, strict workspace/all-target Clippy and
formatting pass; the 28 native opt-in tests remain ignored. No production ZCull
reply or verified real-game image is introduced.


### ZCull callback-object bulk-copy lead (2026-10-10)

Field-pointer metadata now labels symbolic bases as stack-relative,
constant-derived, unknown-derived or register-derived; these labels do not
establish ownership (including for frame-pointer or stack-loaded pointers).
Among register-derived X0 +0x28 candidates with a constant third argument,
the local module exposes a copy-like call at 0x8bab0 to 0xb5420, which branches
to 0xb7340. Its arguments are incoming X0+0x28, incoming X1, and length 1816.
The bounded following neighborhood includes 0x8bae4/0x8baf4 calls to callback
installer 0x84330, both receiving suffix X19+0x28. A full direct-edge scan finds
one nearby-entry caller: 0x7a210 -> 0x8ba90, with incoming X0/X1 in its bounded
snapshot. This provides a concrete template-copy/object-initialization lead;
the template source, copy semantics, saved-register lifetime, and connection to
the ZCull cache still need proof. No firmware plaintext is exported, and no
production reply is enabled. All 63 inspector tests, workspace strict Clippy,
and formatting checks pass.


### ZCull field-pointer search coverage correction (2026-10-10)

Field-address call candidates now cover X0 through X7 for direct calls and
X0 through X2 for indirect calls (the latter's existing snapshot interface).
The previous X1/X2-only scan could omit memcpy-like output destinations in X0.
Nested calls now use exact-site bounded snapshots rather than a 12-byte prefix
that could select an earlier call. Fresh metadata reports +0x28: 332 direct and
7 indirect candidates; enclosing +0x930 still has zero matches for this specific
ADD-immediate pattern. Increased candidate count is not evidence of a ZCull
writer or cached value; stack destinations and unrelated objects are included.
Tests cover every exposed argument position, reject X8 and preserve branch/call
boundaries. All 62 inspector tests pass. Production ZCull remains unsupported.


### Firmware range initializer store provenance (2026-10-10)

Candidate constructor calls with X0 subobject offsets 0xf8/0x190 and X4=1
select the shared target 0x60130 for bounded metadata inspection. Before its
first call at 0x60160, saved X23 derives from incoming X0, X21 from W2, X20
from W3, and X19 from W4, with no unmodeled operations. A separate straight-line
window shows stores at 0x60190 to X23+0x28 (X21) and X23+0x2c
(32-bit X21+X20-1), consistent with start and inclusive-end indices; 0x60188
stores X19 at +0x30. This is not interprocedural execution proof: the two
windows have intervening calls, and live object identity and the relation to
GetVARegions' indexed loads remain unverified. No production default ranges
or ZCull override are introduced. Inspector: 62 tests and strict Clippy pass.


### Firmware VA descriptor argument recovery (2026-10-10)

The bounded symbolic walker now models unsigned CLZ, LSRV, and valid UBFM
immediate-LSR aliases, preserving operand width and modulo-width variable shift
counts. Signed/arithmetic variants and other bitfield forms remain unsupported.
A fresh local firmware inspection resolves both candidate descriptor preparation
calls (0x75930 and 0x759b4) without unmodeled operations in their predecessor
windows: X0 is SP+32, X1 is 32, X2 is 766718, X3 is incoming X21, and X4 is
the 32-bit object field at +0x248 or +0x24c respectively, logically shifted right
by 10. This is local argument provenance, not a decoded format-string claim or
proof of the live object's identity/default ranges. ZCull and real-game launch
remain unresolved. All 62 inspector tests and targeted strict Clippy pass.


### Firmware VA bound arithmetic classification (2026-10-10)

The bounded metadata inspector now distinguishes CLZ, unsigned division,
variable left shift, and variable logical right shift without emitting instruction
bytes or assigning unsupported operations a symbolic value. A fresh inspection
identifies CLZ at candidate-constructor sites 0x75914 and 0x75998, followed by
bitfield operations and variable logical right shifts at 0x75928/0x7592c and
0x759ac/0x759b0. This narrows the missing arithmetic needed to trace descriptor
arguments; it does not prove constructor lifetime, default VA bounds, or a
production ZCull response. All 61 inspector tests pass, including width variants
and rejection of signed-division/arithmetic-shift classification aliases.


Candidate VA constructor 0x757f0 now has a bounded store/call metadata report,
extracted into a dedicated helper rather than another deeply nested inspector
block. Actual metadata finds 0x75868 storing 4096 (32 bits) to suffix Input19+
0x248, and 0x75870 storing incoming Input21 to +0x24c. This supports distinct
small/big-page fields in the candidate object; the complete link to the active
query object is still unproven. Calls at 0x75948 and 0x759cc reach 0x60130 with
object offsets 0xf8 and 0x190 respectively, local descriptor pointers, and
32-bit index/count expressions. These are concrete allocator-initialization
leads, not numeric default-region evidence. Several expressions near the
descriptor-building calls remain unknown due to unmodeled arithmetic/bitfield
operations. Candidate scans remain bounded to 512 bytes and do not establish
function extents or choose conditional branches. Actual firmware metadata was
rerun; 60 inspector tests and strict workspace/all-targets Clippy pass offline.
No production GPU response changed and no game frame is verified.

VA callback-table lead: a full, aligned 64-bit constant pointer store at 0x81a28
to candidate field 0x38 refers to data address 0xeea70. Its bounded raw first
slot contains text-range address 0x75630. This is a static candidate, not an
object-lifetime or applied-relocation proof connecting it to 0x61798. The
candidate target has multiple parameter-validation branches; in a 512-byte
metadata window, calls include an allocation-like call with size 592 and a
call at 0x757d4 to 0x757f0 with multiple saved-register inputs and final argument
1. That latter target is the next constructor/provenance lead; numeric default
bounds remain unverified. Candidate windows may include adjacent blocks and
are never claimed to establish a complete function boundary. Added a bounded
pointer-store search for mapped non-text tables, requiring a contained first
slot, full integer width, eight-byte alignment and non-stack destination.
Tests reject truncated/partial/unaligned/null and call-clobbered candidates.
Actual firmware metadata was rerun; 60 inspector tests and strict
workspace/all-targets Clippy pass offline. No production GPU reply changed.

InitializeEx argument provenance narrowed further: after the 0x610ec path's
0x6110c call, CBZ at 0x61110 can branch to 0x611b8; on the adjacent fallthrough
candidate a call at 0x61138 reaches 0x616e0. The direct-call inspector now
retains AAPCS64 X3..X7 as well as X0..X2. Actual metadata at 0x61138 shows X3/X4
loaded from suffix Input20+0/+8 (32 bits), and X5/X6/X7 from +16/+24/+32
(64 bits). These match the flags, big-page-size and start/end/split fields of
the libnx InitializeEx structure without changing its existing field order.
The callee has an indirect call at 0x61798: target is loaded through suffix
Input20+0x38, then offset 0; arguments0/1/2 are incoming saved-register values,
with the latter two truncated to 32 bits. Object identity and callback
installation remain unproven, and numeric default bounds are not established.
After-call stores/edges and indirect candidates are metadata leads only;
conditional paths are not executed or silently selected by the inspector.
59 inspector tests, strict workspace/all-targets Clippy and formatting pass
offline; actual firmware metadata was rerun. No production GPU reply changed.

Control-flow correction for the initialization investigation: the edge reporter
previously omitted CBZ/CBNZ and TBZ/TBNZ, although the symbolic walker stopped
at them. It now reports all four direct conditional families with signed
targets and rejects misaligned starting sites. Actual InitializeEx has a CBZ
at 0x61040 -> 0x610ec BEFORE its 0x61048 -> 0x61304 unconditional jump; the latter
is therefore not the sole entry path. This supersedes any implication that
the shared block alone described initialization. The newly visible 0x610ec
path reaches a direct call at 0x6110c to 0x5dd70, with argument0 derived from
the global pointer chain and offset 0xa60. Arguments1/2 remain suffix inputs;
this is not yet proof of the address-space constructor or default bounds.
Added exact non-link indirect-tail snapshots (distinct from BLR), retaining
targets and arguments without crossing an earlier control transfer. The
shared block has no such tail in the inspected 32-byte window. Actual firmware
reinspection completed; 58 inspector tests, strict workspace/all-targets
Clippy and formatting pass offline. No production GPU response changed.

VA-policy invariant repair: installing explicit bounds now checks retained
fixed mappings before mutating configuration. Bounds excluding a mapping or
cutting it across the small/big page split are rejected atomically; existing
leases/mappings stay usable after failure. New regression covers mappings
below the start, beyond the exclusive end and across the split, both before
initialization and during subsequent fixed mapping. 154 Horizon tests pass.
Firmware follow-up: exact InitializeEx dispatch is 0x61008 -> 0x6103c, which
branches at 0x61048 to the shared block 0x61304. The nearby 0x61064 call to
0x5c8b0 is NOT established as part of this path (it lies after that unconditional
jump); broad linear proximity must not be promoted to constructor provenance.
The shared block has a 64-bit read at 0x61310 with offset 0x50 and unknown base
in the current bounded probe; its first direct-call probe is None. Numeric
default regions therefore remain unresolved. Actual firmware metadata was
rerun, all 57 inspector tests pass, and strict workspace/all-targets Clippy and
formatting pass offline. This does not advance the normal Snipperclips ZCull
frontier or validate a game frame.

Production VA-range model added for explicitly configured InitializeEx bounds:
start/end/split are retained as two typed page-specific regions. Validation is
atomic, excludes zero/unaligned/empty/reversed/out-of-40-bit bounds, aligns the
page-class split to GM20B PDE coverage, and leaves a failed initialization
uninitialized. Reservations enforce the same region and page-size limits,
including exclusive ends; direct mappings cannot escape/cross the reported
regions. Inline GetVARegions (command 1, 0xc0404108) now serializes these exact
regions to the validated 64-byte IPC output structure, forces the required
48-byte record count, clears each record's padding and preserves the ignored
header pointer without dereferencing it. AutoSelect inactive indices remain
intact. Inherited address spaces and Ioctl3 extra-output transport remain
unsupported. Default InitializeEx bounds still have no established firmware
policy, so they remain unreported instead of silently receiving fabricated
defaults; Snipperclips's normal ZCull frontier is unchanged. This is a range
model/transport implementation, not a verified default-profile game boot.
Validation: 153 Horizon tests and strict workspace/all-targets Clippy pass
offline. New tests cover both 64/128-KiB policies, invalid initialization before
mutation, page-class boundaries, end exclusivity, truncated output, foreign
ownership, ignored pointer metadata and exact region serialization.

VA-region field construction: added typed offset-mode store snapshots that
reuse the existing integer-store decoder and support negative frame offsets
without forwarding memory or crossing calls. Real handler stores are at
0x61380 (frame -16, 32-bit load from suffix Input19+0x248), 0x6138c (frame -8,
64-bit store of a truncated 32-bit difference of the indexed fields), and
0x61390 (frame -24, unsigned product of that loaded page-size candidate and the
start-index candidate). Together with the 24-byte copy this clarifies why the
0x28/0x2c and 0x7c/0x80 fields must not themselves be assumed to be page sizes:
they participate in start/end page-index arithmetic. Bounded suffixes have
different incoming-register identities; expressions are not merged across
them as a whole-function proof. A whole-module overlapping-store scan finds
constant 0x8000000 candidates for field 0x2c at 0x365bc, 0x36778, 0x367a0 and
0x36870. Constructor/object identity remains unproven, so no numeric VA policy
or production reply is enabled. Other offsets have unrelated constant-store
candidates, reinforcing that field-offset coincidence alone is insufficient.
57 inspector tests, strict workspace/all-targets Clippy and formatting pass
offline; actual firmware metadata was re-examined. Tests include both pair
sources, negative frame offsets, call resets, malformed sites and rejection of
loads/vector stores by this integer-only snapshot API.

GetVARegions symbolic-provenance progress: added conservative CSEL alternatives,
unsigned scalar register-offset LDR with UXTX/LSL, low-half MADD/MSUB, and UMULL
(W-source truncation, X destination). No flags are evaluated, signed-index
loads and signed-widening multiplication remain unknown, and no instruction
bytes or disassembly are exported. The real handler's copy candidate at
0x61394 now has no unmodeled operations in its bounded suffix: source is
Input(29)-24 and length is 24. Indexed reads at 0x61378 and 0x6137c resolve to
Input(23) plus conditional alternatives 0x80/0x2c and 0x7c/0x28 respectively,
both 32-bit loads. Input(23) denotes the suffix's incoming register, not a
proven constructor object; condition code 1 is retained symbolically without
selecting a branch. These offsets provide concrete next constructor-search
targets but do not establish numeric VA bounds. Regression coverage includes
register preservation, unchosen alternatives, indexed scale, discarded ZR
destinations, word wraparound, UMULL truncation and unsupported signed forms.
All 56 inspector tests and strict workspace/all-targets Clippy pass offline;
the actual firmware inspector was rerun. No production GPU reply changed.

GetVARegions follow-up: confirmed the 64-byte inline ABI from libnx
`nx/source/nvidia/ioctl/nvhost-as-gpu.c`: ignored pointer header, returned byte
count, padding, two 24-byte offset/page-size/page-count records. No guest pointer
write may be substituted for IPC descriptor writeback. Beware that the current
Switchbrew AllocAsEx description orders the flags/page-size fields differently
from libnx InitializeEx; the current implementation matches libnx and was NOT
swapped on the basis of that conflicting description.
The metadata inspector now recognizes the complete 0xc0404108 dispatch chain:
site 0x610a0, equality handler 0x61324. The handler reads argument1+8 (32 bits),
has an early conditional edge at 0x6132c to 0x6140c, calls the zero-fill candidate
at 0x61344 with size 24, and calls the copy candidate at 0x61394. Two conditional
selects, two indexed memory accesses, and a multiply prevent the conservative
symbolic engine from claiming the latter call's arguments. Therefore this is
not evidence for numeric region bounds. The public older NVIDIA GK20A driver
uses a half-aperture split and a big-page-size-dependent low hole; those values
are not assumed to be the Switch firmware defaults. Production GetVARegions
remains unimplemented rather than advertising unverified page-specific bounds.
Actual firmware reinspection completed with verified segment hashes; 53
inspector tests and strict workspace/all-targets Clippy pass offline.

Controlled Snipperclips continuation experiment: temporarily replying with the
firmware-derived candidate ZCull size 1 advances from the production ZCull
frontier to nvdrv command 4 at 199,367,499 instructions, PC 0x7102649744.
This is NOT proof of the size contract; the temporary reply was removed.
Implemented the independently documented QueryEvent command for GPU-channel
debug/error events 1..=3. Replies contain the NV status and a copied readable
event; repeated queries share event identity, initial state is unsignaled,
foreign/uninitialized clients and malformed buffers are rejected, and invalid
descriptors/event IDs return BadParameter without an object. Closing a channel
drops the driver's references while copied handles retain their object. Native
HIPC now validates command 4's argument length and rejects incoming handles.
Error/debug production signaling and host-control syncpoint events are NOT
implemented by this change. Reference: libnx `nx/source/services/nv.c`,
`nvQueryEvent`, and Switchbrew NV_services QueryEvent event-ID documentation.
With QueryEvent and the same temporary ZCull experiment, actual Snipperclips
reaches 199,386,624 instructions at PC 0x7102649744 and stops on address-space
IOCTL 0xc0404108. Again the experiment was removed, so the production frontier
remains ZCullGetCtxSize. No game frame is verified. Validation: 150 Horizon
tests (including native copied-handle transport and driver ownership/lifetime
regressions), strict workspace/all-targets Clippy, and formatting pass offline.

Expanded the static I/O scan beyond the queue-reference windows: the real FECS
module yields 32 resolved read sites with register/offset propagation. No
resolved read uses FIFO offsets 0x14000/0x14100 (envytools documents those as
command argument/submission); unresolved/computed accesses remain possible,
so this is not an absence proof or a reason to rename the queue consumer.
The two previously unresolved register meanings (0x18d00 and 0x20600) remain
unknown in the inspected primary register map. Added a regression excluding
unknown indices and I/O writes from this scan; 53 inspector tests and strict
workspace/all-targets Clippy pass offline. The scan reports metadata only,
and does not expose firmware code or keys or change the runtime GPU contract.

The indexed-I/O follow-up now resolves all three addresses above: the latest
index definitions are full-word CLEAR at 0x3860 (R11), 0x3e87 (R9), and 0x44c6
(R9). CLEAR propagation follows envytools `docs/hw/falcon/arith.rst`, while
b8/b16 partial clears remain unknown. Actual reinspection gives Falcon I/O
addresses 0x2c000, 0x18d00, and 0x20600 respectively. The first maps to host
offset 0xb00: envytools `gf100_pgraph/ctxctl.xml` identifies CURRENT_CTX for
GM107+, and `ctxctl/intro.rst` calls it CHAN_CUR. This narrows that reference
to channel/context handling, not proof that it produces a host method word.
The latter two registers are not assigned meanings by this evidence. Added
typed candidate index-origin metadata and regression coverage for scaled
CLEAR, unknown/clobbered indices, partial widths and overflow. All 52 inspector
tests and strict workspace/all-targets Clippy pass. No production NV reply
changed and Snipperclips still has no verified frame.

Follow-up on the apparently unresolved I/O base constants: actual metadata
confirms bases 0x2c000 (read 0x3862), 0x18d00 (read 0x3e8d), and 0x20600
(read 0x44c8). These are not complete addresses: envytools encodes a second
register scaled by four in each memory operand (R11, R9, R9 respectively).
The inspector now resolves that indexed offset with checked multiplication
only when the prior register value is known, and reports nearby base-definition
metadata separately. All three complete addresses remain unknown in the real
module; no zero index is assumed and no production IOCTL reply is enabled.
The suspected opcode-token parsing changes were discarded because they did
not explain the observation. Validation: 52 inspector tests pass and strict
workspace/all-targets Clippy passes offline. The actual firmware inspector
was rerun; this remains investigation evidence, not a verified game frame.

Queue-reference windows now report candidate I/O reads and typed DMEM-store
metadata, with 96-byte lookbehind/160-byte lookahead bounds. Actual initialization
includes a store at 0x9b through bounded base constant 0xd90. Near 0x37da/0x37f8,
stores at 0x380a and 0x3816 use unresolved computed bases, and the nearby I/O read
at 0x3862 still has no constant address proof. This does not establish a producer
or a method source. Constant propagation now supports SETHI's documented
low-half preservation/high-half replacement (envytools local arith.rst), requiring
a known prior word and a valid unsigned 16-bit operand; actual I/O unknowns remain.
All 51 inspector tests and strict workspace Clippy pass. Window tests cover
bounds, direct/register I/O addresses and unknown bases; no raw code is exported.
Runtime ZCull and actual game-frame milestones remain unverified.

Static external-entry scanning finds no direct branch/call entering the interval
between the latest R13 definition and either index read. This does not exclude
indirect entries or establish complete dominance. A separate exact MOV-immediate
scan finds queue-address references at 0x49/0x89 (0xd8c/0xd4c), 0x37da/0x37f8
(same pair), 0x3ee4/0x3ef8 (same pair), and 0x450e (0xd8c). The 0x37da region
is a new producer-side investigation lead, not an established enqueue path.
Tests cover external-vs-local edges, definition-entry boundaries and literal
materializations vs register moves/arithmetic. All 49 inspector tests and strict
workspace Clippy pass. No supported runtime ZCull reply or game frame yet.

Index-base definition/control metadata isolates why the second FECS index read
has no straight-line constant proof. Both 0x3eea and 0x3ef2 have the same latest
R13 move candidate at 0x3ee4, but the latter is separated by a branch at 0x3eee
targeting 0x4328. This is a control-flow gap rather than evidence of a fresh
unknown address calculation; dominance/reachability must still be proven before
carrying the constant across it. Reports retain only four latest base-definition
candidates and edge addresses/kinds, never decoded instruction text. All 47
inspector tests and strict workspace Clippy pass. No runtime ZCull reply or
actual game-frame milestone is claimed.

Independent [Switchbrew interface documentation](https://switchbrew.org/wiki/NV_services#NVGPU_GPU_IOCTL_ZCULL_GET_CTX_SIZE)
confirms the four-byte output structure, but supplies no hardware return value;
it therefore does not corroborate the candidate reply of 1. FECS constant-address
analysis now follows register moves and simple ADD/SUB/AND/SHL/SHR within a shared
128-byte window and depth limit 8. Branches, unknown inputs, excessive shifts
and subword merges stop constant inference. The real index/control metadata is
unchanged under this stronger probe: 0x3eea has base 0xd8c, 0x3ef2 remains unknown,
and no direct constant-address control store is found. All 46 inspector tests
and strict workspace Clippy pass. This extends the diagnostic path only; no
runtime GetCtxSize value or actual-game milestone is admitted.

FECS index-source metadata now reports nearby DMEM reads before the index
transforms. The actual 0x3f08 candidate includes reads at 0x3ef2 (R13+0, unknown
bounded base) and 0x3eea (R13+0, bounded base constant 0xd8c). A full text scan
for direct constant-address stores to that identified control address returns
none under the conservative constant/branch rules. This does not exclude
computed-address stores or prove a queue producer/consumer relationship.
Tests cover index-source reads, exact control-store address matching, overflow
and branch boundaries. All 45 inspector tests and strict workspace Clippy pass.
The host-method link and cached ZCull reply remain unverified; no game frame.

The FECS metadata path now follows nearby definitions of the base register used
by the candidate method-word DMEM read, not only definitions of the method word
itself. At 0x3f08, the latest address arithmetic candidate is 0x3f05 using R9/R12;
bounded preceding constant analysis identifies R12=0xd4c. Latest index-transform
candidates are 0x3eff (shift operand 3) and 0x3efc (mask operand 7). These are
queue-layout leads, not proof that a host method 0x16 reaches this record. The
report retains only eight latest definition candidates per read and exports no
instruction text/bytes. Synthetic tests cover destination-vs-source matching,
base arithmetic, source constants and index-transform metadata. All 44 inspector
tests and strict workspace Clippy pass. Runtime GetCtxSize remains unsupported;
the FECS candidate reply of 1 is not promoted to a verified cached driver value.

Output-pointer candidates now include indirect BLR calls, with tests rejecting
earlier direct calls, branches and mismatched offsets. The real module scan
reports +0x28: 14 direct and 5 indirect candidates; all five indirect arguments
use stack-relative +0x28 addresses in their bounded suffixes. The corresponding
full enclosing offset +0x930 reports zero direct/indirect candidates for this
specific ADD-immediate/argument-1-or-2 pattern. Neither zero count proves absence
of initialization through other addressing or data-copy paths. All 42 inspector
tests and strict workspace Clippy pass. Cached-size provenance remains open;
there is still no supported runtime GetCtxSize or verified Snipperclips frame.

Entry memory metadata now distinguishes scalar reads/writes and retains their
entry-relative base expressions only before control flow. Narrow integer/SIMD
pair-save prologues with pre-indexed SP preserve arguments while tracking SP;
non-SP writeback is still unsupported and stored memory is not forwarded.
Actual target 0x5e760 reads a 64-bit value at incoming argument X1+0x218 within
its entry window. That does not match a simple output-only four-byte scalar
contract without additional containing-object semantics; it must not be assumed
to be the cached-size writer. Targets 0xaf950/0xaf6d0 initially read X0+4 instead.
Missing entry metadata is not evidence of no access. The next search includes
indirect calls receiving the field address, since the current output-pointer
scan admits only direct calls. All 41 inspector tests and strict workspace Clippy
pass; no runtime GPU behavior or game-frame milestone changed.

The cached-field search now includes calls receiving a +0x28 address in argument
1 or 2, since a callee can populate an output pointer without a direct store at
the caller. Fourteen actual module candidates are reported, including targets
0x5e760 at several sites and 0xaf950/0xaf6d0 with selector constants. Stack
addresses and unrelated objects remain candidates, not established ZCull writes;
direction and final field value must be proven from the callee. Tests require
the matching field argument and reject intervening branches, earlier indirect
calls and missing data. All 39 inspector tests and strict workspace Clippy pass.
The runtime driver still rejects GetCtxSize rather than supplying an unverified
value; Snipperclips and Odyssey remain unlaunched/unverified as games.

Exact-site register snapshots establish that the callback-bearing X20 in
0x65000 equals incoming argument X0 immediately before its first direct call;
the snapshot does not cross that call. Nearby-entry caller metadata identifies
0x84330 as an installer candidate called at 0x8bae4 and 0x8baf4. Exact-site
direct-call lookup now resets after the latest control-flow edge rather than
accidentally returning an earlier neighboring call. Both actual installer calls
receive incoming X19+0x28 as argument X0 in their respective straight-line
suffixes. Connecting these instances across the full object lifetime and to the
cached ZCull field remains unproven. All 38 inspector tests and strict workspace
Clippy pass, including exact-site and earlier-call rejection tests. This is
metadata-only progress; runtime GetCtxSize and actual game frames remain open.

Callback-installation candidates are now scanned once per observed slot offset,
requiring a complete integer 64-bit write and a nonnull aligned in-text constant
source. This is still not proof of a function boundary or object identity.
Actual NV metadata finds slot +0x48 written at 0x84390 with target 0x84a60,
and slot +0x50 written at 0x843a4 with target 0x84ae0; both bases are incoming
X0 within their bounded suffixes, with no unsupported operations. Their target
access/edge metadata is reported without exporting firmware code. The remaining
gap is connecting that installed object to the X20 used in 0x65000, then proving
the cached ZCull field initialization. Tests reject null, unaligned, out-of-text
and partial-word pointer candidates. All 36 inspector tests and strict workspace
Clippy pass; runtime GetCtxSize remains unsupported and no game frame is verified.

Unsigned LDUR byte/halfword/word/doubleword provenance now retains signed byte
addresses; sign-extending and writeback forms remain unsupported. Offset-mode
stores reuse the shared store decoder and preserve argument registers without
pretending to model stored memory. Indirect-call lookup now uses the same
60-byte straight-line suffix as write-origin lookup, resetting after the last
control-flow edge instead of an arbitrary 12-byte start. This matters in the
actual NV module: 0x65070 resolves to a function pointer at incoming X20+0x48,
with arguments X20, SP+4 and zero, and no unsupported operations in that suffix.
Its earlier unknowns were unscaled memory operations (not proven to be loads),
and the short window omitted the target definition. This identifies a callback
slot, not its executable target or a cached size. All 35 inspector tests and
strict workspace Clippy pass; no new runtime/game-frame milestone.

Subobject inspection now accepts a proven offset expression in either argument
0 or argument 1. This exposes additional callees 0x6a260, 0x65000 and 0x61cf0;
the earlier argument-0-only report omitted them. Bounded 1024-byte region
metadata includes indirect BLR sites as well as direct calls. The shared
symbolic walker reports an indirect target expression without treating the
register number as a code address, and reports unknown provenance when a prior
control-flow edge prevents reaching the requested call site. Tests cover loaded
targets, direct/indirect distinction, BR exclusion and bounds; all 33 inspector
tests and strict workspace Clippy pass. Actual 0x65000-region indirect targets
include fields at incoming X20+0x48 and X20+0x50; some earlier targets remain
unknown because of unscaled loads. Region bounds are not function boundaries
or proof of execution. No cached ZCull size or game frame has been established.

Register ADD/SUB with LSL and logical immediates are now represented in the
bounded provenance engine. Synthetic LLVM encodings test widths, shifts, wrap
and rejection of extended/SP or unsupported shift forms. Logical mask decoding
is exhaustively checked across all 8192 encodings at both widths against an
independent per-bit construction, including reserved encodings.
Actual NV metadata resolves two more stores: 0x6d0e8 adds the existing X23+0x28
field to ((incoming W22+3) & 0x1fc), while 0x708e8 performs the analogous update
using the X21 object and incoming W19. These are arithmetic accumulations, not
directly established GPU size replies. At 0x60190 the source/base remain
start-relative X21/X23 with no unsupported operations. This narrows the search
but still does not identify the cached ZCull object/value. All 32 inspector
tests and strict workspace Clippy pass. No runtime ioctl implementation, actual
game frame or completion milestone is claimed.
Reference: [Arm-authored AND immediate pseudocode mirror](https://www.scs.stanford.edu/~zyedidia/arm64/and_log_imm.html).

Immediate arithmetic provenance now covers SUB, 32-bit wrap/zero extension and
flag-only CMP/CMN without erasing unrelated registers. The real module's store
at 0x75b84 is now resolved as a decrement of the existing 32-bit +0x28 field,
not a discovered ZCull size reply. At 0x36bd0 the value is a truncated incoming
X8 rather than a locally established constant. Remaining unresolved origins
include register arithmetic, bitfield/logical operations and unclassified
instructions. All 28 inspector tests and strict workspace Clippy pass; this
refinement changes diagnostic evidence only, not runtime GPU behavior.

Bounded write-origin reporting now reuses the argument-provenance engine rather
than a separate interpreter. It starts after the latest control-flow edge in a
60-byte window and never carries constants across a call/branch. Integer offset
pair loads snapshot their original base before destination updates; unsupported
negative offsets remain unknown. Twenty-seven inspector tests and strict
workspace Clippy pass. Actual NV metadata proves the store at 0x6d06c copies a
32-bit field at X23 + 0x2c into X23 + 0x28, while 0x5ea44 retains unknown
start-relative X21/X20 inputs. This does not identify either object as the ZCull
subobject. Other candidate origins still encounter unsupported instructions;
no runtime ioctl value or game-frame evidence follows from these diagnostics.

The field-write scan also covers integer/SIMD STUR signed byte offsets, with
independently LLVM-assembled synthetic cases checking load/writeback rejection.
All 24 inspector tests and strict all-target workspace Clippy pass. The actual
NV scan still finds the same three enclosing-offset candidates; no extra STUR
initialization resolves the cached-value gap. A second scan through subobject
offset 0x28 retains eight complete integer 32-bit call-adjacent writes, reporting
bounded argument/access metadata for their calls. Their addresses are 0x36bd0,
0x3a178, 0x5ea44, 0x60190, 0x6d06c, 0x6d0e8, 0x708e8 and 0x75b84. These are
search leads only: intervening branches and unrelated base objects mean call
adjacency cannot establish returned-value or object provenance. Runtime behavior
is unchanged; Snipperclips still has no verified game frame.

The bounded field-write probe now reports byte intersections for integer and
SIMD unsigned stores and offset-mode pairs, including partial/wider writes.
Loads, writeback addressing, malformed ranges and incomplete words are excluded;
offset matches still do not prove object identity. Twenty-three inspector tests
pass. In the actual verified NV module, additional matches in the two candidate
initializers are stack saves, not proof of a ZCull field initialization. A full
text scan for enclosing offset 0x930 finds only three non-stack candidates:
0x244 (64-bit zero store), 0x51fe8 and 0x52ebc (byte zero stores). These remain
unrelated-object candidates; none establishes a nonzero cached size. The next
investigation must track object/pointer provenance or indirect initialization,
not infer a value from offset matches. No runtime reply or game-frame claim is
introduced by this inspection.

Bounded ARM64 argument provenance now confirms the GetCtxSize handler passes
X0 = incoming X21 and X1 = incoming X21 + 0x908 to the accessor; its returned
32-bit field is therefore at enclosing-object offset 0x930. The ioctl dispatch
probe now requires the complete MOVZ/MOVK/CMP/B.EQ chain with matching register
identity, not just a nearby branch; both actual ZCull commands satisfy it.
Subobject-reference candidates identify calls to 0x6a6b0 and 0x81250. A nested
call in the former has destination expression incoming X21 + 0xb68, value 0,
and length 48, but this alone does not establish the final cached context size.
No local immediate constant store to field 0x930 was found by the conservative
probe. Twenty-one example tests cover argument aliases, 32-bit zero extension,
unknown/branch stopping, complete dispatch matching and pair-store bounds.
The inspector's ARM64 reporting is separated from segment enumeration and the
symbolic walker lives in its own module. The runtime ioctl remains unsupported
until initialization evidence closes the cached-value gap; no new game frame.

The NV service executable now supplies a more direct ZCull investigation path.
Predefined ARM64 ioctl materializations locate ZCullGetCtxSize at 0x79814/18;
the equality branch at 0x79820 targets 0x799c8, which calls 0x64970. That callee
is exactly a 32-bit load from argument register X1 + 0x28 followed by RET.
This verifies the accessor, not its cached value or the identity of X1's object.
The corresponding GetInfo equality path is 0x795e8 -> 0x79a6c -> call 0xb55a0.
No full ioctl constants occur in the inspected non-text segments. The project
CPU decoder is reused as an ingest dev-dependency for bounded access metadata;
neither keys nor decrypted code are exported. Sixteen synthetic example tests
and strict workspace all-target Clippy pass. The next direct step is tracing
the GetCtxSize caller's X1 argument and the initialization of its field 0x28.
The Falcon queue load at 0x3f08 reads through R9 with offset zero; its address
is not treated as known across a control-flow boundary. No ZCull reply or game
frontier advancement is claimed from these metadata-only findings.

The dispatch-input mask is now identified as 0xffff0fff from a signed-format
Falcon immediate at 0x3f23, applied at 0x3f27. It preserves an input method 0x16.
The inspector resolves the latest nearby register definition rather than using
an older overwritten constant; signed-word parsing and overwritten-definition
tests pass. Eleven synthetic example tests and strict workspace all-target
Clippy pass. The remaining provenance gap is the queue-word data read at 0x3f08
and its relationship to the externally submitted FECS method. The value 1
remains a candidate result, not an enabled ZCullGetCtxSize response.

Direct inspection of the Tegra init_ctx_state source confirms that discovery
reads mailbox 0's nonzero value into zcull_ctx_image_size, rather than returning
only a success flag. Falcon's indexed I/O mapping relates 0x20000 to the host
FECS context-switch mailbox 0 window. The remaining gap is dispatch input
provenance: new metadata probes identify the compared register's nearby data
read at 0x3f08 and mask at 0x3f27, but do not establish their dominance or the
queue-word semantics. No ioctl response is enabled from this candidate alone.
Ten synthetic example tests and strict workspace all-target Clippy pass;
metadata probes never expose arbitrary register contents or firmware instructions.

The optional Falcon bridge now follows a bounded compare/branch dispatch tree
for a hypothesized method-register value 0x16, stopping before unknown operations.
Actual candidate path: 0x3f95 -> 0x3f98 -> 0x3f9c -> 0x3f9f -> 0x428b.
The candidate handler sets constants then jumps to 0x4325; a separate narrow
constant-propagation probe finds its first I/O write at Falcon address 0x20000
with value 1. This is not yet used as a ZCull size: the dispatch register origin
and mailbox/result semantics must be established before admitting the ioctl.
Nine example tests cover unknown-register stopping, branch selection and
constant I/O propagation, in addition to previous bounds checks. Strict
workspace all-target Clippy passes. No decrypted instructions/files are logged
or exported, and no new game frame or runtime progress is claimed.

An optional local Falcon decoder bridge now analyzes the verified FECS asset
through anonymous pipes, retaining only counts and dispatch candidate addresses.
No firmware plaintext or disassembly file is exported. The temporary envydis
build uses upstream envytools revision f102b82381f3f11cee113d16374c87091db039d9
at /tmp/exosphere-falcon-tools.8FwHjf/build/envydis/envydis; it is not a runtime
dependency or system installation. Set EXOSPHERE_FALCON_DECODER explicitly to
enable the inspector-only bridge. On the actual firmware, fuc5 decoding reports
5,652 address-bearing lines and zero unknown/incomplete markers. The public
Tegra method ID 0x16 has a comparison candidate at FECS 0x3f95; nearby branch
edges are 0x3f98 -> 0x3fb8 and 0x3f9f -> 0x428b. These are dispatch candidates,
not proof of the mailbox result or successful FECS execution. Seven synthetic
example tests and whole-workspace offline tests pass. The next step is tracing
the candidate handler and its inputs; ZCullGetCtxSize remains unimplemented.

The Tegra netlist header was read directly from the public source after web
fetch failures: [gr_ctx_gk20a.h](https://android.googlesource.com/kernel/tegra/+/76359c267702c0815c82c970f38f5b27031d5ba6/drivers/gpu/nvgpu/gk20a/gr_ctx_gk20a.h).
It identifies region 11 as the ZCull GPC AIV register list and regions 15..18
as scalar metadata. Actual firmware inspection finds 56 ZCull entries with
index range 0..0xdc; general buffer metadata is 4, major version 0, register
base index 0 and netlist number 0xb. None is treated as the FECS-discovered
ZCull allocation size. The inspector now reports these named metadata fields
without register values or microcode payloads. Six example tests, strict ingest
all-target Clippy and formatting pass. The real-game frontier remains open;
the next investigation is the FECS discovery calculation, not file-length guessing.

The netlist probe now rejects duplicate region IDs and overlapping nonempty
payloads while permitting empty entries. Five synthetic example tests pass,
including bounded asset descriptors and safe filename handling; strict ingest
all-target Clippy passes. The actual NV firmware still passes this stronger
inspection. Region IDs 0..3 match the FECS/GPCCS data/instruction identifiers
documented by the [Nouveau loader](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/nouveau/nvkm/engine/gr/ga102.c).
The [Tegra netlist loader](https://android.googlesource.com/kernel/tegra/+/76359c267702c0815c82c970f38f5b27031d5ba6/drivers/gpu/nvgpu/gk20a/gr_ctx_gk20a.c)
distinguishes microcode, register lists and general buffer metadata. None of
these file lengths establishes the mailbox-discovered ZCull context size.
Unknown region IDs remain unclassified rather than assigned guessed semantics.

The verified NV NSO now has metadata-only embedded-asset inspection. All three
segments pass their integrity checks. NETB_img.bin is a bounded 70,580-byte asset;
its version-0 netlist candidate has 28 bounded regions (including empty regions).
The inspector reports only sanitized relative asset names and region metadata,
never keys, instructions or decrypted payload bytes. Synthetic tests cover
filename filtering, truncated tables and out-of-bounds region rejection; the
example tests and strict ingest all-target Clippy pass. These region sizes are
not evidence for ZCullGetCtxSize: that real-game gate remains unresolved.

The user updated the local key dump. Revision-15 master/KEK/application keys
are now present; the first inspector attempt exposed an overly broad
master_kek_ prefix match that treated unused master_kek_source_* metadata as
direct 16-byte KEKs. Those metadata names are now ignored, consistently with
other unconsumed dump fields; actual KEK width, duplicate and derivation-conflict
checks remain strict. Synthetic tests cover non-key metadata and rejection of
malformed/duplicate usable KEKs. Whole-workspace offline tests, strict all-target
Clippy and formatting pass. No user key file was changed or key values logged.
A fresh real firmware_nv_inspect succeeds: NV program 0100000000000019,
generation 22, verified PartitionFS section 0, main 1,001,425 bytes and main.npdm
1,476 bytes. This supersedes the earlier missing-key gate, not the open ZCull
context-size investigation or actual Snipperclips frame requirement.

FLO is now also verified through authenticated Linux IPC: 1,152 variants cover
three source forms, eight source words, source inversion, signed/unsigned search,
bit index/shift amount and six output projections over three geometries.
All 3,456 added draws pass, raising total fixture coverage to 9,747 draws.
The immediate oracle uses the sign-extended encoded operand; high-bit projections
check the not-found sentinel. Shared unary source/encoding helpers preserve
existing POPC coverage without duplicating that transport logic. The existing
completion ordering, rejection preservation, exact background/alpha and used-
channel cleanup remain asserted. This is controlled driver-path evidence only.

Maxwell FLO now lowers its exact one-source register/constant-buffer/short-
immediate forms to executable FindLeadingWord32 IR. Signed search, inversion
before sign normalization, bit-index versus shift-amount mode, and the unchanged
0xffffffff sentinel are explicit. CC/reserved/src0 controls and RZ destinations
reject; RZ sources and predication retain canonical effects. Vertex/fragment
interfaces and certified extent discovery accept the operation. Integer-only
SPIR-V uses legal fixed shifts, prefix filling and population count, without
float reinterpretation or optional extended instructions.

Full-word evaluation of emitted SPIR-V checks 4,624 mode/input combinations
against an independent most-significant-bit search. The test integer executor
is shared with existing conversion tests rather than duplicating their harness.
Native SwiftShader additionally decodes aliased register FLO and checks seven
result-bit projections across 71 source words and eight mode combinations:
3,976 controlled renders pass. Decoder tests cover immediate/constant forms;
authenticated Linux IPC has not yet added FLO and remains at 6,291 draws.
Whole-workspace offline tests, strict all-target Clippy and formatting pass. No actual game shader/frame
or ZCull boundary crossing is claimed.
Sources:
https://raw.githubusercontent.com/chaotic-cx/mesa-mirror/main/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp
https://docs.nvidia.com/cuda/parallel-thread-execution/#integer-arithmetic-instructions-bfind

After the user's key-dump clarification, a fresh metadata-only firmware inspector
run still reports generation 22 and MissingKey for key_area_key_application_15.
Key-name-only inspection confirms master_key_14, master_kek_14 and
key_area_key_application_14 are present. This is a missing newer revision, not
evidence that the existing dumped keys are invalid; Snipperclips decryption and
GPU shader work are separate from that firmware-inspection prerequisite.

Maxwell POPC now has executable CountBits32 IR and SPIR-V OpBitCount rather than
guessed BitfieldRich register fields. Exact GM107 register, direct constant-buffer
and signed-short-immediate forms have one source, with optional bitwise inversion
before counting. Unsupported CC/sign/reserved/src0 bits and RZ destinations
reject; RZ sources and predicates retain their canonical semantics. Certified
extent discovery and vertex/fragment interfaces admit the explicit operation.

Native SwiftShader tests decode actual aliased register POPC instructions and
compare all six result bits against an independent bitwise reference: all counts
0..32, five additional noncontiguous patterns, and both inversion modes (456
controlled renders). Authenticated Linux IPC additionally verifies three source
forms, eight source words, both inversion modes and all six result bits across
three geometries: 864 new draws, 6,291 total fixture draws. Immediate expectations
use the encoded sign-extended 20-bit word, not the original register word.
All 205 GPU IR tests, whole-workspace offline tests, strict all-target Clippy,
formatting and all 27 native Vulkan/Linux tests pass. These fixtures establish
POPC execution, not a Snipperclips frame or progress beyond ZCullGetCtxSize.
Encoding source:
https://raw.githubusercontent.com/chaotic-cx/mesa-mirror/main/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

The renewed ZCull source inspection still supplies no verified GM20B context
image size: the firmware-discovered context size is distinct from region geometry.
The local key-name-only audit finds no master_key_15, master_kek_15 or
key_area_key_application_15 entry. No guessed size, firmware payload or secret
key material was introduced or printed; firmware inspection remains unavailable
through that key revision and does not by itself guarantee recovery of the size.

Authenticated Linux IPC now exercises ordinary BFE through real channel-owned
GPU mappings, encoded fragment programs, uniform discovery, Vulkan submission
and tiled guest color writeback. It adds 1,176 variants across all three control
sources, signed/unsigned extraction, seven offsets, seven widths, two source
words and low/sign-bit projections, repeated over three geometries: 3,528 new
draws, 5,427 total authenticated fixture draws. The existing completion ordering,
background/alpha checks, rejection preservation and channel teardown checks
remain enabled. Register/constant controls include ignored upper bits; short
immediates retain the same field bytes after signed encoding. Shared integer
projection encoding avoids duplicating shader construction with shift fixtures.
The selected native IPC test, whole-workspace offline tests, strict all-target
Clippy and formatting pass. This is controlled driver-path evidence, not a game
frame, and does not implement ZCullGetCtxSize. A fresh actual Snipperclips probe
stops after 199,328,010 instructions at PC 0x7102649744 on that same unsupported
GPU-control IOCTL (0x80044701); no frame/menu/gameplay is verified.

Ordinary Maxwell BFE now lowers register, signed-short-immediate and direct
constant-buffer control operands to explicit ExtractWord32 IR rather than the
old guessed BitfieldRich register fields. Packed offset/width bytes, signed
extension, zero width, out-of-word fields and ignored upper control bits have
explicit SPIR-V semantics; all executed shifts stay below 32 even on discarded
OpSelect branches. Reverse/CC, reserved controls and RZ destinations reject
instead of silently losing effects; RZ sources and predication retain their
canonical representation. Vertex/fragment interfaces and executable extent
discovery accept the certified ordinary form.

Native SwiftShader verification decodes the actual register instruction and
checks every output bit against an independent bitwise oracle across signed/
unsigned extraction, seven offsets, seven widths and two source words: 6,272
controlled renders, with aliased source/destination. Decoder tests separately
cover immediate/constant sources and rejected controls. The complete native
Vulkan/Linux suite passes 26 tests; authenticated IPC coverage remains 1,899
draws and does not yet exercise BFE. No new actual Snipperclips frame or ZCull
boundary crossing is claimed.

Encoding/layout sources: Mesa GM107 emitBFE and constant-folding OP_EXTBF,
with NVIDIA PTX bfe for the independent bit reference:
https://raw.githubusercontent.com/chaotic-cx/mesa-mirror/main/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp
https://chromium.googlesource.com/chromiumos/third_party/mesa/+/a11d1bd247df18057ecb9945e01bf39ff85d8d9d/src/gallium/drivers/nouveau/codegen/nv50_ir_peephole.cpp
https://docs.nvidia.com/cuda/parallel-thread-execution/#integer-arithmetic-instructions-bfe

User-key ingestion now accepts master_kek_<revision> and derives the exact
revision's master using the supplied master_key_source, before existing title-
KEK/key-area/header derivations. Matching supplied masters are retained;
contradictory supplied/derived masters fail integrity checks. No built-in keys,
cross-revision substitution or newer-key invention is introduced. Synthetic
inverse-encryption fixtures check downstream derivation, missing prerequisites,
revision isolation, matching/conflicting masters, duplicates and malformed KEKs.
All 27 ingest tests, whole-workspace offline tests, formatting and strict
all-target Clippy pass. The real local key file parses successfully with this
additional consistency check; its intermediate KEKs reach revision 14, not 15.
The firmware NV inspector still reports generation 22 and MissingKey for
key_area_key_application_15. Equivalent prerequisites are master_key_15 or
master_kek_15 plus master_key_source and the key-area derivation sources.
A short real Snipperclips regression executes its full 1,000,000-instruction
budget at PC 0x71000008bc without a key error; this is not a full boot-frontier
probe and does not supersede the last ZCull stop. No secrets/plaintext are logged.
Derivation reference: https://github.com/SciresM/hactool/blob/master/pki.c

Homebrew-ingest audit found that NRO segment reads were bounded by the whole
container rather than the declared codebin size, allowing trailing assets to
serve as program bytes. Segment reads now use the declared program slice and
reject overlapping nonempty segments. ASET parsing uses a checked tail slice
and u64 endpoint comparisons rather than narrowing untrusted offsets to usize.
Two regressions cover trailing-byte segment escapes, overlaps/empty segments,
valid relative assets, short headers and oversized/overflowing asset offsets.
All 26 ingest tests, whole-workspace offline tests, formatting and strict
all-target Clippy pass. This is bounded NRO parsing, not a homebrew loader ABI
or proof of an unmodified homebrew main loop. No local homebrew binary was found;
the execution milestone and the real-game ZCull frontier remain unverified/open.
Header reference: https://raw.githubusercontent.com/switchbrew/libnx/master/nx/include/switch/nro.h

Native mip sampling now includes interpolated UV gradients rather than only
constant coordinates and forced sampler LODs. A vertex buffer supplies the same
triangle positions as the reference plus UV spans giving rho=2/4 for the 2x2
texture. Vertex varyings feed fragment interpolation and implicit TEX, with
minLOD=0/maxLOD=1 and nearest mip selection. The expected level-1 RGBA8/BC1
colors survive both texel filters and live payload refresh; background remains
unchanged. Sixteen additional draws bring this native mip fixture to 80 draws.
Distinct vertex shader identities preserve the immutable shader-cache contract.
This proves controlled minification and maximum-LOD clamping, not all derivative
cases, magnification, anisotropy or authenticated IPC gradient-driven sampling.
Workspace offline tests, all 25 native Vulkan/Linux tests on SwiftShader,
formatting and strict all-target Clippy pass. IPC coverage remains 1,899 draws.
The real Snipperclips ZCull startup frontier remains unchanged; no game frame
or new game probe is claimed.

Mip sampling now has native trilinear and authenticated Linux IPC evidence.
The native RGBA8/BC1 fixture expands from 32 to 64 draws, testing nearest mip 1
and an independently computed half-LOD blend with mip 0 under nearest/linear
texel filtering, implicit/zero shader LOD and live refresh. The IPC fixture adds
16 variants across three geometries (48 draws): RGBA8/BC1 full-view tiled chains,
nearest/half-blended mip selection, implicit/zero LOD and refreshed level data.
Decoded TIC/TSC records, packed live mip payloads, Vulkan pixels, guest canonical
writeback and semaphore ordering all pass; RGB tolerance is one, alpha and
untouched background are exact. Total IPC fixture coverage is 1,899 draws,
including 240 TEX draws and 96 compressed draws. Completion lies in unused
second-mip GOB padding, not sampled logical bytes. Whole-workspace offline tests,
all 25 native Vulkan/Linux tests on SwiftShader, formatting and strict all-target
Clippy pass. Unrestricted derivative-driven LOD, partial mip views, arrays and
actual game mip textures are not established. Snipperclips's real ZCull startup
frontier remains unchanged; no new game probe/frame is claimed.

Full-view single-layer block-linear 2D mip chains now traverse TIC discovery,
live packed resource reads, prepared-resource validation and Vulkan upload.
Each mip retains explicit physical/packed offsets and independently reduced
GOB height in format block rows. Address resolution covers the full physical
chain; partial mip views, pitch mip chains, arrays, sparse/MSAA and unsupported
controls remain rejected. Image mip count comes from the validated descriptor,
not a fixed one-level default. Reads split at GOB, row and mip boundaries;
invalidation remains conservative over the whole backing allocation.
Two unit regressions verify shrinking GOB heights, exact offsets, short-map
rejection, full and arbitrary cross-mip reads, compressed blocks and live last-
mip updates. A new native test executes 32 draws across RGBA8/BC1, nearest/linear
texel filters, implicit/zero shader LOD, coordinates and refreshes, forcing mip 1
through a nearest-mip sampler LOD clamp. Exact selected-level colors/background
pass on SwiftShader. This does not yet verify trilinear blending, unrestricted
derivative-driven LOD, authenticated IPC mip draws or actual game textures.
Workspace offline tests (203 GPU IR), all 25 native Vulkan/Linux tests, strict
all-target Clippy and formatting pass. The real Snipperclips ZCull frontier
remains unchanged; no new game probe/frame is claimed.
Layout reference: https://raw.githubusercontent.com/devkitPro/deko3d/master/source/dk_image.cpp
TIC fields: https://raw.githubusercontent.com/devkitPro/deko3d/master/source/maxwell/texture_image_control_block.h

New-image initialization now rolls back cached/native image ownership after
ordinary payload or preparation failures, using the existing image-allocation
guard for ordered destruction. This applies only to newly created images;
failed refreshes preserve previously valid contents and identity. Images from
uncertain submissions remain retained on the poisoned device until teardown.
A native regression covers missing/empty/truncated initial payloads, verifies
cache absence and same-ID retry, reads exact successful pixels, then rejects a
truncated refresh and verifies the previous handle/layout/pixels are unchanged.
All 24 native Vulkan/Linux tests, workspace offline tests, formatting and strict
all-target Clippy pass. A fresh real Snipperclips probe completes 199,328,011
instructions at PC 0x7102649744, stopping on GPU-control ioctl 0x80044701
(ZCullGetCtxSize). This supersedes prior instruction counts, not the startup
frontier. The generation-22 key-name audit still reports absent entries. No
real-game image or ZCull startup fix is established.

Synchronous texture upload/readback now transfer staging ownership through
submission instead of detaching and leaking handles on error. Pre-submit
failures release staging through its guard. A failed fence wait retains the
command buffer, fence and optional staging together on the device, and poisons
future submissions because recorded image layouts may be uncertain. Device
teardown waits idle and drains retained objects before destroying its command
pool/device. Readback uses the same guard and receives ownership back only
after a successful wait, keeping host mapping separate from queue lifetime.
A native regression submits a real empty command and waits for actual completion,
then injects a failed host result to verify retained handles, a still-live fence
and rejection of new commands before teardown. This covers ownership policy,
not an actual device-loss fault or recovery without device recreation. All 23
native Vulkan/Linux tests, whole-workspace offline tests, formatting and strict
all-target Clippy pass. Snipperclips's real ZCull startup frontier is unchanged.

Texture upload now preflights every packed mip footprint and checked cumulative
offset before staging allocation or recording image transitions. Truncated
payloads no longer leave a cached layout describing an unsubmitted transition.
Unit tests cover all mip boundaries and layered offsets; the native image test
rejects a short upload, preserves image handle/layout, then refreshes successfully.
Texture staging now maps the whole allocation and flushes its whole mapped
range before unmapping, including when HOST_COHERENT is unavailable (the existing
memory selector only prefers it). The pending-buffer guard releases partial
allocation/mapping on pre-submit failures. Ownership is deliberately detached
before submission: an uncertain fence-wait failure still retains staging rather
than destroying possibly in-flight memory; deferred retirement/fault injection
remain unimplemented. Native validation uses SwiftShader and does not independently
prove the noncoherent-memory branch on hardware. All 22 native Vulkan/Linux tests,
workspace offline tests, formatting and strict all-target Clippy pass. These
changes do not resolve the real-game ZCull initialization frontier.

Vulkan image creation now validates canonical shape before cache lookup,
provider access or native allocation. Width/height/depth/layers/mips/sample
counts are no longer silently normalized from zero to one. Excess mip levels,
3D array layers, unsupported sample counts and multisample depth/mip combinations
are rejected explicitly. The mip limit uses integer bit length, including
u32::MAX without overflow. Valid dimensions are passed unchanged to creation,
views and cached image metadata. Unit cases cover boundaries; a native regression
checks nine invalid descriptors leave no cached image, then reuses the same ID
successfully and verifies a later invalid request preserves its valid descriptor.
Whole-workspace offline tests, formatting and strict all-target Clippy pass;
all 22 native Vulkan/Linux tests pass on SwiftShader, including the 1,851-draw
authenticated fixture. This is resource-validation hardening, not a Snipperclips
boot fix. No new game probe or progress past ZCullGetCtxSize is claimed.
Reference: https://docs.vulkan.org/refpages/latest/refpages/source/VkImageCreateInfo.html

ZCull startup investigation: current upstream Nouveau independently confirms
that external FECS firmware supplies the ZCull context image size through
gf100_gr_fecs_discover_zcull_image_size during context-control initialization.
There is no GM20B byte-size constant established by this source. Historical
emulator code returning 1 is a compatibility implementation, not a hardware
measurement, and is not evidence for the allocation size in this project.
Do not substitute that value for a verified profile or implemented context model.
The local key-name-only audit still finds neither master_key_15 nor
key_area_key_application_15, so the generation-22 NVN firmware inspection path
remains unavailable. No key contents or decrypted firmware are logged. This
investigation does not advance game execution or establish that firmware
inspection alone will recover the required size.
Primary source: https://raw.githubusercontent.com/torvalds/linux/master/drivers/gpu/drm/nouveau/nvkm/engine/gr/gf100.c

Constant-buffer SHL/SHR operands now have authenticated native Linux IPC
coverage, in addition to register and signed-short-immediate forms. The fixture
binds fragment c[1][0] to a live, explicitly allocated 16-byte guest uniform
block at GPU 0x200d00, separate from vertex depth and aligned to 256 bytes.
No implicit descriptor padding or alignment bypass is used. All 432 shift
variants across three geometries pass exact pixel/background/writeback and
completion checks: 1,296 shift draws, 432 newly added constant-buffer draws,
and 1,851 total fixture draws. These cover both wrap modes, all three shift
directions, boundary/oversized counts and bit-0/31 projections. Linux unit tests,
all 21 native Vulkan/Linux tests on SwiftShader, formatting and strict Linux
all-target Clippy pass. This verifies controlled guest shaders,
not actual game rendering; the last Snipperclips frontier remains ZCullGetCtxSize.

Encoded SHL/SHR now traverse the authenticated Linux IPC submission fixture:
288 register/short-immediate variants across three geometry cases add 864
native draws, bringing the fixture to 1,419 draws. Cases cover logical/arithmetic
right shifts and left shifts, wrap/clamp modes, counts 0/1/31/32/33/0xffffffff,
positive/negative source words, and projections of bits 0/31 through LOP and
U32-to-F32 conversion. Exact RGBA8 pixels, untouched background, canonical
writeback and completion ordering are checked. Constant-buffer shift operands
are semantically covered but are not claimed as authenticated native draws.
Whole-workspace offline tests (201 GPU IR), all 21 native Vulkan/Linux tests,
strict all-target workspace Clippy and formatting passed for this change.
The latest real Snipperclips probe stops after 199,328,020 instructions at
PC 0x7102649744 on unsupported GPU-control ioctl 0x80044701 (ZCullGetCtxSize).
This supersedes earlier instruction counts, not the startup frontier. No real
game frame/menu/gameplay is verified. Generation-22 key names remain absent
from the local key file; no key values are recorded here. Context size remains
unsupported pending verified hardware/profile evidence or a genuine context
model, rather than a fabricated allocation size.

Explicit ShiftWord32 IR now preserves SHL, logical SHR and arithmetic SHR with
wrap/clamp control rather than misusing generic integer modifiers. Semantic
decoding admits exact register/constant-buffer/short forms, including SHR sign
bit 48 and wrap bit 39; CC/X, bit reversal and reserved fields remain rejected.
SPIR-V masks the shift count before every shift (including discarded select
operands), then selects zero/sign-fill for nonwrap counts >=32. This avoids
undefined Vulkan shifts while retaining unsigned 32-bit count semantics.
A semantic matrix checks modes/forms and unsupported controls. A new native
test executes 144 draws: three directions, two wrap modes, counts 0/1/31/32/33/
0xffffffff, positive/negative source words, and projections of result bits 0/31
to exact black/white pixels through integer conversion. Background is preserved.
Whole-workspace offline tests (201 GPU IR), all 21 native Vulkan/Linux tests on
SwiftShader, strict workspace all-target Clippy and formatting pass. The
authenticated IPC fixture remains 555 draws; encoded IPC shift draws and actual
game shaders are not yet verified. Snipperclips's ZCull frontier is unchanged.
References: https://raw.githubusercontent.com/chaotic-cx/mesa-mirror/main/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp
https://docs.nvidia.com/cuda/parallel-thread-execution/#logic-and-shift-instructions-shr

Maxwell LOP AND/OR/XOR now use form-specific decoding and native 32-bit bitwise
SPIR-V emission, including independent source inversion (OpNot, not negation).
Register/constant-buffer/short forms require PT predicate output; full immediate
function and inversion bits are decoded separately from the operand word.
Predicate writes, CC/X, reserved fields and PASS_B remain explicitly rejected.
A semantic matrix covers four source forms, three functions and four inversion
combinations, plus side-effect rejection. Thirty-six authenticated IPC variants
(register/short/full immediate) across three geometries add 108 native draws,
with exact RGBA8 results, canonical writeback, background and semaphore checks.
The authenticated fixture totals 555 draws (185 variants); TEX counts unchanged.
Whole-workspace offline tests (200 GPU IR), all 20 native Vulkan/Linux tests on
SwiftShader, strict workspace all-target Clippy and formatting pass. No new
real-game shader, frame or progress past Snipperclips ZCullGetCtxSize is claimed.
Encoding reference: https://raw.githubusercontent.com/chaotic-cx/mesa-mirror/main/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Physical RZ (register 255) sources now become immutable ConstantBits32 zero
operands in semantic value resolution rather than undefined register reads.
Each source gets its own temporary; integer low-word admission permits RZ reads
while continuing to reject destination-RZ forms (discarded writes unimplemented).
A semantic regression checks distinct zero operands and destination rejection.
Six authenticated IPC variants across three geometry cases add 18 native draws
for IADD/IMUL with left/right/both RZ sources. Exact black/gray output, alpha,
background, canonical writeback and completion checks pass. The IPC fixture now
totals 447 draws (149 variants); TEX/compressed counts are unchanged.
Whole-workspace offline tests (199 GPU IR), all 20 native Vulkan/Linux tests on
SwiftShader, strict workspace all-target Clippy and formatting pass. This does
not establish all zero-register instruction forms or a real game frame.
Snipperclips remains at its last verified ZCullGetCtxSize startup frontier.
Encoding reference: https://raw.githubusercontent.com/chaotic-cx/mesa-mirror/main/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Low-word integer source negation now emits OpISub(0, word), preserving defined
modulo-2^32 behavior instead of using floating-point negation. Both sources are
read before any aliased destination write. Maxwell IADD ordinary forms preserve
NEG bits 49/48; IADD32I preserves the source NEG at bit 56 without confusing it
with the full immediate payload. Shared admission still rejects ABS/invert,
carry, saturation and high-word controls. Encoding regressions now cover these
NEG combinations, including constant-buffer metadata forms.
Seven new authenticated IPC variants add 21 draws for left/right/both NEG in
register/short immediate forms and left NEG with a full immediate. Exact native
RGBA8 pixels pass through canonical guest writeback and semaphore completion.
The fixture totals 429 draws (143 variants), with TEX/compressed counts unchanged.
Whole-workspace offline tests (198 GPU IR), all 20 native Vulkan/Linux tests on
SwiftShader, strict workspace all-target Clippy and formatting pass. This does
not prove new real-game progress or all integer controls. Snipperclips remains
at its last verified ZCullGetCtxSize startup frontier.
Semantics reference: https://registry.khronos.org/SPIR-V/specs/unified1/SPIRV.html

Maxwell integer arithmetic now reaches actual authenticated Linux IPC draws.
End-to-end validation exposed and fixed missing production admission in shader
extent discovery and fragment export lowering. Both reuse the strict semantic
or SPIR-V integer-control checks; unknown instructions are not broadly admitted.
Signed immediate words are normalized to ConstantBits32 before native emission.
Nine variants (IADD, unsigned IMUL, signed IMUL; register, signed short immediate
and full immediate sources) across three geometry cases add 27 draws. Overflow
inputs produce independently expected float bit patterns for exact grayscale
RGBA8 output; clipped background and semaphore checks remain unchanged.
The authenticated fixture totals 408 draws (136 variants), including the
unchanged 192 TEX/72 compressed draws. Whole-workspace offline tests, all 20
native Vulkan/Linux tests on SwiftShader, strict workspace all-target Clippy
and formatting pass. Constant-buffer arithmetic, unsupported modifiers and
actual game shaders are not established by these cases. Snipperclips remains
at its last real ZCullGetCtxSize frontier; no new game probe/frame is claimed.

Maxwell IADD/IMUL low-word admission now uses form-specific encoding masks and
controls rather than the generic integer modifier extraction. The old long
immediate lowering could interpret bits within the 32-bit operand as saturation,
carry or source modifiers. Register, constant-buffer, signed short immediate
and full immediate forms now have exact masks; supported IMUL signedness is
decoded from bits 40/41 or 54/55. Carry/high/saturation/negation, mixed signedness,
reserved bits and zero-register forms remain explicitly unsupported rather than
being advertised Ready. A regression covers eight source/opcode combinations,
including a full 0xffffffff operand, and rejects form-specific control bits.
Primary encoding reference: https://raw.githubusercontent.com/chaotic-cx/mesa-mirror/main/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp
Whole-workspace offline tests (198 GPU IR tests), strict workspace all-target
Clippy, formatting and all 20 native Vulkan/Linux tests on SwiftShader pass.
Existing canonical integer pixels remain verified; the new binary forms have
semantic regression evidence, not new authenticated IPC arithmetic draws.
Snipperclips's last real ZCullGetCtxSize startup frontier remains unchanged.

The straight-line native shader backend now emits canonical IntegerBinary
low-word Add/Mul as OpIAdd/OpIMul on 32-bit integer register words. Scalar float
sources are bitcast, not numerically converted, and both sources are captured
before destination replacement. Signed/unsigned low words share modulo-2^32
semantics; carry, saturation, high-word operations and source modifiers remain
explicitly rejected. No NoSignedWrap/NoUnsignedWrap decorations are introduced.
Reference: https://registry.khronos.org/SPIR-V/specs/unified1/SPIRV.html
An admission regression checks unsupported controls. Eight native fragment
draws verify signedness modes, add/multiply overflow and aliasing either source;
the resulting words are interpreted as output floats and compared to exact
RGBA8 pixels with background preservation. This is canonical arithmetic
execution evidence, not new verified Maxwell binary encoding coverage or a
real game shader. Whole-workspace offline tests (197 GPU IR tests), all 20
native Vulkan/Linux tests on SwiftShader, strict workspace all-target Clippy
and formatting pass. The authenticated fixture remains 381 draws; the real
Snipperclips ZCullGetCtxSize frontier remains unresolved.

Authenticated Linux IPC TEX coverage now includes BC2 and BC3 UNORM color plus
nontrivial alpha, adding 48 draws to the previous BC1 cases. The fixture totals
381 draws (127 variants across three geometry cases), including 192 TEX draws
and 72 BC1/2/3 compressed draws. BC2 alpha nibbles [0,4,8,12] become [0,4,8,8]
on refresh, giving exact center alpha 102 then 85. BC3 endpoint selectors produce
[0,128,0,128] then [0,128,0,0], giving exact center alpha 64 then 32. RGB also
changes independently. Pitch/block-linear layout and implicit/zero LOD variants
retain actual guest descriptors, queue completion and canonical writeback.
RGB allows one output-unit filtering tolerance; alpha/background are exact.
Encoding reference: https://learn.microsoft.com/en-us/windows/win32/direct3d10/d3d10-graphics-programming-guide-resources-block-compression
Whole-workspace offline tests, all 19 native Vulkan/Linux tests on SwiftShader,
strict workspace all-target Clippy and formatting pass. This does not establish
compressed sRGB, BC7, mip-chain or actual game rendering support. The last real
Snipperclips probe remains stopped at ZCullGetCtxSize; no game image is claimed.

The authenticated Linux IPC fixture now includes BC1 UNORM TEX draws through
the production descriptor pools, live guest memory adapter, native queue and
canonical color writeback. Eight new variants (pitch/block-linear, implicit/zero
LOD and refresh) across three geometry cases add 24 compressed texture draws.
The fixture totals 333 draws (111 variants), including 144 TEX draws. A partial
2x2 BC1 block with red/green endpoints uses selector positions 0/1/4/5; changing
selector 5 checks that current guest bytes reach bilinear sampling. Output RGB
permutes to [B,R,G], with at most one output unit rounding tolerance; alpha and
background stay exact. Existing completion, failed-draw and channel-close
checks remain in place. Whole-workspace offline tests, all 19 native
Vulkan/Linux tests on SwiftShader, strict workspace all-target Clippy and
formatting pass. This is controlled guest IPC evidence, not a real game frame;
Snipperclips remains at its last verified ZCullGetCtxSize startup frontier.

Fresh real Snipperclips probe after texture admission/BC1 validation completes
199,328,011 instructions and stops at PC 0x7102649744 on GPU-control ioctl
2147763969 (0x80044701, ZCullGetCtxSize). This supersedes the previous instruction
count, not the unchanged startup frontier. A key-name-only check still finds
neither master_key_15 nor key_area_key_application_15 in the local key file;
firmware investigation remains unavailable and no key contents were printed.
Native image allocation now has a scoped owner for partial image/memory/view
creation, releasing view before image before backing memory on early errors.
Successful publication explicitly transfers ownership to the device cache.
Whole-workspace offline tests, 19 explicitly native Vulkan/Linux tests,
Vulkan/Linux strict all-target Clippy and formatting pass. Driver allocation
failure injection is not covered by the native success tests. No menu, actual
game frame, audio or gameplay is verified.

BC1 UNORM sampling now has native pixel evidence, not just format metadata or
image creation evidence. The existing descriptor-backed texture sampling test
is shared with a BC1 case: a partial 2x2 view uses selectors 0/1/4/5 in a 4x4
block with red/green RGB565 endpoints. Updating selector 5 changes the expected
nearest and bilinear results independently. Thirty-two draws cover nearest vs
linear, implicit vs zero LOD, two coordinates, pitch vs Tegra block-linear
payloads, and live refresh through TextureResourceData; TIC channel permutation
and untouched background pixels are checked. SwiftShader explicitly reports
BC1 pixels verified, rather than taking the missing-feature rejection branch.
All 19 native Vulkan/Linux tests, strict all-target Vulkan/Linux Clippy and
formatting pass. This does not validate BC1 sRGB sampled pixels, BC2/3/7, or
authenticated guest IPC BC draws; the existing 309-draw IPC fixture is unchanged.
The real-game ZCullGetCtxSize frontier remains unchanged; no game frame claimed.

Native image creation now derives usage flags from optimal-tiling format
features instead of assigning attachment/storage usage to every color format.
The selected combination, extent, layers, mip count and sample count are checked
against physical image-format properties before vkCreateImage. Descriptor and
render attachment uses reject missing native usage flags. Supported BC/ASTC LDR
core features are explicitly enabled; unavailable families are rejected.
Reference: https://docs.vulkan.org/spec/latest/chapters/resources.html
Two ordinary regressions cover usage and feature admission; a new native test
checks BC1 UNORM/sRGB creation/upload when supported, or exact missing-feature
rejection otherwise. This test does not prove compressed sampled pixels.
Validation: 22 ordinary Vulkan tests and 18 explicitly native Vulkan/Linux
tests pass on SwiftShader, including the unchanged 309-draw authenticated IPC
fixture. Snipperclips's last verified startup frontier remains ZCullGetCtxSize;
no actual game image or boot advancement is claimed.

Texture staging no longer converts every provider length failure into an absent
payload. Read-only sampled inputs require a nonempty payload; only an explicitly
missing optional initial render-target payload is allowed. Other provider errors
propagate for both inputs and outputs. Host length conversion and staging
reservation are fallible instead of truncating lengths or using an infallible
allocation. Two regressions cover required/optional and error propagation policy.
Validation: 20 ordinary Vulkan tests, all 17 explicitly native Vulkan/Linux tests
on SwiftShader (including the authenticated 309-draw fixture), and strict
all-target Clippy for the Vulkan/Linux packages pass. This is upload admission
hardening, not compressed-format support or advancement past Snipperclips's
last verified ZCullGetCtxSize frontier; no game frame is claimed.

TIC sRGB views with enabled TSC conversion now select the corresponding
canonical sampled format without changing the underlying UNORM storage
footprint. RGBA8 and BC1/2/3/7 view mappings have explicit regressions; compressed
native sampling is not claimed by those metadata tests. Disabled TSC conversion
on an sRGB TIC remains explicitly unsupported pending verified Maxwell policy.
Prepared views preserve raw TSC controls, and fragment preparation/draw assembly
revalidate the complete prepared pair rather than only the pixel backing.
Standard sRGB border colors use the separate encoded TSC RGB bytes while alpha
remains the ordinary float word; arbitrary encoded borders remain unsupported.
Field reference: envytools rnndb/graph/g80_texture.xml, SRGB_BORDER_COLOR fields.

The authenticated IPC fixture now executes 309 draws (103 variants across
three geometry cases), including 24 new sRGB draws through encoded TEX. Midtone,
nonuniform texels are decoded by an independent reference before bilinear
filtering and TIC channel permutation; RGB comparisons allow one output-code
unit, while nonopaque alpha must match exactly. Both pitch/tiled storage, LOD
forms and source refresh pass with persistent native caches. All ordinary
workspace tests (196 GPU IR tests), formatting, strict workspace all-target
Clippy and all 17 native Vulkan/Linux tests pass. The real Snipperclips frontier
remains the last verified ZCullGetCtxSize stop; no new game probe/frame is claimed.

Float TIC swizzles now include explicit zero and one-float outputs, represented
by TextureSampleChannel rather than out-of-range component indices. Handle
lowering composes these constants with TEX's compact selected-channel outputs;
SPIR-V emits exact F32 constants instead of illegal vector extracts. Reserved
swizzle encoding 1 and integer-one encoding 6 remain rejected by this float-only
path. This enables ordinary R8/RG8 views with zero-filled missing channels and
constant alpha, as well as explicit constant outputs on RGBA textures.
The field encodings follow deko3d's public Maxwell image_formats.h.

The authenticated Linux fixture now executes 285 draws (95 variants across
three geometry cases), including 96 encoded TEX draws. Its added R8, RG8 and
RGBA-constant-swizzle cases cover both guest storage layouts, both supported LOD
forms and pixel refresh, checking actual sampled pixels and canonical writeback.
The implicit and zero-LOD diagnostics with constant channels both pass
spirv-val for Vulkan 1.1. All ordinary workspace tests (194 GPU IR tests), strict
workspace all-target Clippy and all 17 native Vulkan/Linux regressions pass.
Reinspection of the documented ZCull path still provides no verified firmware
context size: this sampling improvement does not resolve Snipperclips startup,
and no new game probe or frame is claimed for this step.

Linux guest draw submission now captures the fragment shader's direct TEX
selectors automatically, reads each handle/TIC/TSC through live canonical
channel memory, and prepares texture bindings above the stage's constant-buffer
slots. GuestColorRenderer owns persistent sampled-image and sampler identities
in the same checked descending namespace as color images; allocation/view
namespace collisions fail. Complete backing/view and sampler state control
identity reuse, while changed pixels retain their identities and refresh on
submission. The canonical resource provider installs checked packed aliases
for each captured texture before native execution. Single-color admission now
allows explicit read-only sampled inputs without admitting storage-image or
additional attachment writes. Physical target/input feedback overlap is
rejected until a verified feedback/coherence implementation exists.

The authenticated nvdrv IPC fixture now executes 213 draws (71 variants over
three geometry cases), including 24 encoded Maxwell TEX draws through actual
pool/constant-buffer state, live resource selection, shader preparation, draw
assembly, Vulkan sampling, tiled color writeback and completion signaling.
Those cases cover pitch/block-linear storage, implicit/zero LOD, TIC channel
mapping and source refresh under persistent native caches. Shader/pool/image
payloads occupy disjoint checked fixture ranges. All ordinary workspace tests,
strict workspace all-target Clippy, formatting and all 17 explicitly executed
native Vulkan/Linux tests pass. This establishes the supported encoded sampling
path, not arbitrary textures, a captured Snipperclips shader or a game frame.
The name-only firmware-key audit still finds neither generation-22 key entry;
no key material was printed or modified. Real-game startup remains a separate
required gate.

The fresh Snipperclips content-inspect CPU probe after this integration completes
199,328,002 instructions at PC 0x7102649744, then stops on unsupported GPU-control
IOCTL 2147763969 (0x80044701, ZCullGetCtxSize). This supersedes the previous
instruction count, not the startup frontier. No game menu/frame/audio/gameplay
is verified, and the diagnostic CLI exit code is not proof of game launch.

Resolved shader-resource reflection now derives uniform/image/sampler layouts
from actual operations, including accesses inside predicates, and merges stage
visibility without dropping uniform-size ABI or descriptor-type conflicts.
Unresolved handles cannot acquire guessed declarations. PreparedShader retains
its validated texture views; prepare_fragment_with_textures resolves the TEX
interface, prepares fragment exports/SPIR-V and emits image/sampler bindings
alongside uniforms, rechecking current pixel ownership against the draw mapping.
Prepared graphics merging validates descriptor command types and referenced
views. Uniform allocation selection still selects only real buffer allocations.

Color draw assembly now carries prepared sampled images/samplers into the GPU
module, checks remaps and identity conflicts with buffers/targets, and places
descriptor bindings outside the render pass for image transitions. Native 2D
sampling now uses prepare_fragment_with_textures for exports, reflected layout
and bindings rather than hand-declaring those resources; all 32 sampled draws
and all 17 native Vulkan/Linux tests pass. New regressions cover shared/nested
declarations, cross-stage ABI conflicts, missing resources and stale ownership.
All ordinary workspace tests (193 GPU IR tests), strict workspace all-target
Clippy and formatting pass. Renderer-owned view identity allocation, automatic
TEX selector capture and live alias installation in authenticated game draw
submission remain to be connected. No actual Snipperclips probe/frame or ZCull
startup progress is claimed.

Joint TIC/TSC preparation now produces a canonical packed float 2D image,
sampler and explicit channel mapping from the verified descriptor fields.
TextureHandleRecords exposes this preparation without confusing descriptor
allocations with image identities. Supported fixed-point LOD/filter/address,
anisotropy and standard float border settings are retained; backend device
limits remain checked at native creation. Unsupported depth/integer sampling,
sRGB views, constant swizzles, unnormalized coordinates, header LOD controls,
reduction and snapping controls fail explicitly rather than being dropped.
Field auditing uses deko3d's public Maxwell TIC/TSC definitions and
[TSC generator](https://github.com/devkitPro/deko3d/blob/master/source/maxwell/tsc_generate.cpp).

lower_texture_interface now resolves explicit handle-selector interfaces into
SampleTexture2d operations, composing TEX-selected channels with TIC swizzles.
It preserves coordinates, destination registers, LOD mode, shader provenance
and predicate structure, while rejecting missing/unused/duplicate interfaces,
descriptor collisions, invalid channels and unsupported stages. Preserving a
predicate here does not claim executable backend predication support. The 32
native sampling draws now derive canonical image/sampler parameters from raw
TIC/TSC and run this handle-lowering pass before sampling live pitched/tiled
subviews; all 17 native Vulkan/Linux tests pass. All 189 GPU IR tests and strict
workspace all-target Clippy pass. Automatic selector discovery, renderer-owned
view IDs, reflected pipeline resources and authenticated game draw wiring still
remain; this is not a real-game TEX/frame or a resolution of ZCull startup.

TextureResourceData now exposes reserved, read-only packed image IDs over live
allocation payloads without copying or retaining guest pixels. Registration
validates TIC storage and source allocation bounds, rejects allocation/view ID
collisions and preserves suballocation offsets. Arbitrary logical reads split
at pitched row boundaries or Tegra 16-byte GOB runs; source reads remain live,
so provider errors and current lease checks propagate. Writes cannot escape
through the texture alias; unrelated allocation operations and fingerprints
are forwarded. Packed view fingerprints deliberately remain uncertified rather
than incorrectly hashing padded storage. Invalidation conservatively forwards
the complete physical storage range.

The native 2D sampling fixture now obtains every image through this adapter from
poison-padded, offset allocations described by pitch/block-linear TICs. All 32
draws preserve the expected nearest/linear pixels, channel selection, both LOD
forms and source refresh. Unit tests compare arbitrary tiled slices across GOB
boundaries and verify pitched refresh, bounds and identity checks. All ordinary
workspace tests (including 186 GPU IR tests), strict workspace all-target Clippy
and all 17 explicitly executed native Vulkan/Linux tests pass. This proves
TIC-backed payload adaptation for canonical sampling, not decoded game TEX
execution: joint TIC/TSC view preparation, production identity allocation and
draw/IPC wiring are still required. No Snipperclips boot probe or game frame is
claimed; its documented ZCull frontier remains unchanged.

GM200 texture storage discovery now resolves the TIC pixel address separately
from handle/TIC/TSC allocation identities. TextureHandleRecords exposes this
through resolve_image_backing. The validated single-level, single-layer 2D path
retains the actual resource offset and distinguishes pitch storage (including
inter-row padding, excluding unused final-row padding) from block-linear GOB
storage. Compressed formats use block rows/bytes rather than pixel rows.
Resolution checks readable ownership and the complete storage span afresh;
unsupported headers, physical borders, layer offsets, mip chains, sparse/MSAA
storage, alternate tile widths/GOB-3D and invalid tile exponents fail explicitly.
The implementation follows the public deko3d GM200 TIC definition. All 184 GPU
IR tests and strict GPU IR all-target Clippy pass. This is storage preparation,
not a completed sampling view: TIC swizzles/sRGB/coordinate controls, joint TSC
validation, production provider aliases and descriptor binding remain required.
No native tests or Snipperclips boot probe were rerun for this change; no game
frame or advancement beyond ZCull is claimed.

Resolved canonical SampleTexture2d now emits separate UniformConstant image
and sampler descriptors, OpSampledImage and implicit-LOD or explicit-zero-LOD
sampling. Float/bit-register coordinates are assembled before any sampled
destination overwrites them; selected channels are extracted into their actual
destination registers. Descriptor collisions with uniforms or the other texture
descriptor type, invalid channels/destination shapes and undefined coordinates
are rejected. Fragment export lowering accepts only the resolved form; raw
SampleTexture2dHandle still requires draw-time TIC/TSC preparation. Both modes
of the texture_2d_spirv diagnostic pass spirv-val for Vulkan 1.1, following the
[SPIR-V image instruction specification](https://registry.khronos.org/SPIR-V/specs/unified1/SPIRV.html).

A native fixture now checks actual 2x2 RGBA8 sampled pixels through separate
set-1 image/sampler bindings: nearest/linear filters, two coordinates, channel
permutation, both LOD forms and two successive provider payloads (16 draws).
The refresh case first exposed stale cached images, then passes with read-only
texture input refresh through staging. GPU-written images are not overwritten
by that refresh path, and pending writeback prevents input replacement. Native
image shape/format/sample identity is checked on reuse; guest payload layout
is deliberately separate from Vulkan optimal tiling, preserving legitimate
linear host initialization followed by tiled guest payloads.

All 180 GPU IR tests, ordinary workspace tests, strict workspace all-target
Clippy and 17 explicitly executed native Vulkan/Linux regressions pass. This
establishes canonical native texture sampling, not a Maxwell-game textured
frame. Required next integration is verified TIC image-view/footprint and TSC
pairing, descriptor/resource ownership in production draw assembly, followed
by encoded guest sampling through authenticated IPC. Snipperclips ZCull boot
status is unchanged and was not reprobed in this step.

Texture opcode classification now precedes the broad high-bit load family,
which previously hid C/D texture encodings from semantic decoding. The old
generic TextureRich lowering dropped dimensions, coordinates, compact output
channels and LOD modes while claiming losslessness; it is replaced by a strict
direct TEX 2D non-array/non-shadow form, ordinary or level-zero LOD, with no
offset/live-only/derivative-all controls. SampleTexture2dHandle explicitly
retains the encoded selector (not a resource identity), both packed coordinate
registers, selected channels and consecutive destination registers. Predicates
remain attached. Unsupported texture variants stay opaque rather than acquiring
ordinary sampling semantics. Graphics extent discovery includes the certified
form but remains separate from executable shader support.

Draw-time read_texture_handle_records resolves that selector as a word offset
in the stage's captured constant buffer selected by method 0x982, reads the live
32-bit handle, splits TIC/TSC indices and reads the bounded pool records. It
retains separate handle/descriptor backing identities, never confusing them
with the image resource. Explicit unlinked TSC mode is currently required;
unknown/linked selection is rejected pending validation. This follows Mesa's
[GM107 TEX emitter](https://github.com/chaotic-cx/mesa-mirror/blob/main/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp)
and [NVC0 handle lowering](https://github.com/chaotic-cx/mesa-mirror/blob/main/src/gallium/drivers/nouveau/codegen/nv50_ir_lowering_nvc0.cpp).
Tests cover all nonempty channel masks, selector bounds, both LOD modes,
predicates, refused controls, stage isolation, handle/pool payload refresh and
live remaps. All 179 GPU IR tests and strict workspace all-target Clippy pass.
These operations are not yet admitted by executable fragment lowering: image
view/footprint preparation, canonical descriptor bindings and sampled-image
SPIR-V emission remain open. No sampled pixels or new Snipperclips boot/frame
progress is claimed by this change.
Ordinary whole-workspace tests, format checking and all 16 explicitly executed
native Vulkan/Linux regressions also pass after the texture classification change.

Canonical samplers now carry explicit filter/mipmap/address modes, exact
fixed-point LOD clamps/bias, anisotropy, optional comparison, border color and
coordinate normalization rather than an uninterpreted raw_key. Vulkan creation
uses these fields instead of the previous unconditional linear/repeat fallback.
Sampler anisotropy is enabled only when reported supported, and creation checks
device anisotropy/LOD-bias limits. Unsupported legacy clamp modes and optional
mirror-clamp-to-edge are rejected rather than approximated. Unnormalized
samplers obey creation restrictions; their image-view/shader restrictions still
need validation when texture instructions are integrated.

Non-mipmap canonical state uses Vulkan's documented nearest-mip/base-level
emulation (maxLod 0.25 when min/mag filters differ, zero when equal); nonzero
clamps remain rejected pending Maxwell policy validation. See
[Vulkan sampler specification](https://docs.vulkan.org/spec/latest/chapters/samplers.html).
Cached sampler objects retain their complete descriptor and reject identity
mutation instead of silently returning stale settings. Unit tests check exact
native creation fields, feature/limit rejection and normalization restrictions;
an explicit native regression creates distinct samplers, reuses identical state
and rejects mutation without replacing the original object. This verifies native
sampler creation, not sampled pixels. TSC-to-canonical pairing with TIC view
controls and actual texture shader/image bindings still need integration.
Ordinary whole-workspace tests, strict whole-workspace all-target Clippy,
format checking and all 16 explicitly executed native Vulkan/Linux regressions
pass. Snipperclips startup/frame status is unchanged and was not reprobed here.

The TIC/TSC audit now uses the GM200 texture-header layout (not the older
G80/GM107 TIC layout), cross-checked with
[envytools GM200 TIC](https://github.com/envytools/envytools/blob/master/rnndb/graph/gm200_texture.xml),
[envytools TSC](https://github.com/envytools/envytools/blob/master/rnndb/graph/g80_texture.xml)
and devkitPro/deko3d's Maxwell header/format definitions. TIC storage format
promotion now checks numerical component types instead of treating the size code
alone as UNORM/float. Supported integer/float and signed BC variants remain
distinct; unrepresented mixed/force-FP16 conversions stay Unknown with the full
format word. Header-dependent address masking excludes block-linear GOB depth
offset bits, buffer widths include their upper 16 bits, unrepresentable 2^32
extents stay unknown, and reserved headers no longer invent image dimensions.
This storage format does not yet apply swizzles, joint TIC/TSC sRGB controls,
pitch, sparse/GOB offsets or sampling-view policy.

TSC decoding preserves all eight distinct wrap modes, including legacy GL_CLAMP
and mirror-border variants, and distinguishes no mip filtering from nearest.
Compare controls, sRGB enable, unnormalized-coordinate override, reduction mode
and raw border colors are retained. The existing descriptor_memory read path
now rejects incomplete/overlong responses and overflowing ranges instead of
zero-padding or indexing past eight words; pool reads share that implementation.
All 176 GPU IR tests, ordinary whole-workspace tests, strict whole-workspace
all-target Clippy and 15 explicitly executed native Vulkan/Linux regressions
pass. A key-name-only check still finds neither generation-22 key entry; no key
values were printed. No new Snipperclips probe/frame or ZCull fix is claimed.
Texture shader discovery, complete sampling-view preparation and actual native
texture/sampler binding remain required before claiming textured game rendering.

Raw texture/sampler pool records can now be resolved and read using the captured
pool configuration. Resolution checks the inclusive index bound, address
overflow and read permission, resolves only the selected 32-byte record, and
reconsults the current address space rather than caching a guest VA as a resource
identity. Reads retain all eight little-endian words and reject both short and
overlong provider responses. Descriptor backing identity is kept separate from
the image referenced by the record. Tests exercise independent pools, nonzero
resource offsets, sparse mappings, live remaps, denied reads, overflow and exact
read lengths. These helpers are not yet wired into texture instruction discovery
or native image binding; TIC/TSC field auditing and actual sampling remain open.
No new game boot or frame is established by this preparation.

Texture descriptor pool methods (sampler 0x557..0x559, header
0x55d..0x55f) now capture state without generating opaque work. Draw snapshots
expose the complete raw address and inclusive maximum index independently for
each pool; incomplete configuration stays unknown, and later writes cannot
mutate an earlier snapshot. Cache invalidations remain unsupported commands,
not silently consumed state. The register layout is cross-checked against
devkitPro/deko3d source/maxwell/engine_3d.def. Focused regressions cover missing
words, inclusive index zero, full-width maxima, snapshot lifetime and cache
command preservation. All 169 GPU IR tests pass. This is pool-state preparation
only: descriptor memory resolution, verified TIC/TSC field decoding and actual
texture sampling still need production integration. No new Snipperclips frame
or ZCull startup progress is claimed.

Scalar user varyings now connect encoded vertex AST to fragment IPA for the
verified linear/default interpolation subset. Vertex outputs retain their
hardware byte-address-derived location/component; fragment inputs use explicit
NoPerspective scalar SPIR-V interfaces. Pipeline linkage rejects any fragment
component not written by the vertex shader. Position outputs remain separate;
mixed Position/user vector stores and indirect/patch accesses remain refused.
The IPA audit also corrected structural classification (a broad high-bit load
match previously hid IPA), byte-address component calculation and lossy mode
handling. Exact linear/default and flat/default metadata is retained; perspective,
SC, centroid/offset, saturation and predicate-output variants remain refused.
Flat metadata is not yet executable because provoking-vertex policy is unresolved.
The Linux fixture verifies real per-pixel Position.x gradients through attributes
0x80/0x88/0x94, with native readback matching canonical tiled writeback. Actual
authenticated IPC now runs 63 ordered variants per fixture case (189 draws),
including those gradients, payload refresh, semaphore ordering and durable failure.
Both diagnostic shader stages pass spirv-val. Whole-workspace ordinary tests,
15 explicit native regressions and strict workspace all-target Clippy pass.
This is user-varying support, not texture sampling, perspective interpolation or
a verified Snipperclips frame; ZCull startup remains unchanged.

Captured integer vertex formats now lower to explicit LoadInputInteger
operations, preserving signed/unsigned register words instead of silently
creating floating-point inputs. SPIR-V declares matching ivec4/uvec4 inputs,
extracts scalar words and bitcasts signed words without numeric conversion;
I2F remains a separate operation. Shared locations reject conflicting scalar
types, and color-pipeline assembly checks format signedness against the shader
interface before submission. Existing float-input checks remain strict.
The persistent Linux indexed fixture exercises R8/R16/R32 signed and unsigned
inputs with actual packed strides/limits and encoded ALD + I2F, all four modes,
visible/clipped/restored pixels and exact tiled writeback (72 draws). Typed
diagnostics pass spirv-val for eight sign/rounding variants. Whole-workspace
ordinary tests, 15 explicit native regressions and strict workspace Clippy pass;
the enlarged packed-input fixture also passes independently. Snipperclips still
has no verified frame and its ZCull startup remains unresolved.

Fragment export lowering now admits the same verified plain I32/U32-to-F32
subset as vertex lowering. Fragment shaders containing ConvertRich select the
integer-based SPIR-V emitter even without uniform reads, rather than the older
constant-only emitter. Unsupported controls and undefined sources remain
rejected; physical R0..R3 exports still occur at actual termination. The Linux
persistent fixture executes 48 encoded fragment conversions from uniforms or
immediates, both signs and four rounding encodings, with red 0/255/0 coverage.
Vertex depth and fragment color use distinct words of one canonical allocation
and distinct stage-group bindings; conflicting uniform payloads catch mistaken
immediate-source reuse. All covered/background pixels and canonical tiled
writeback are checked. Whole-workspace ordinary tests, 15 explicit native
regressions and strict workspace all-target Clippy pass. No real Snipperclips
frame or ZCull initialization progress is established by these synthetic draws.

The authenticated native IPC replay exposed a production discovery gap: direct
constant-buffer I2F still failed shader extent discovery, although fixed-window
standalone rendering passed. Extent discovery now admits only semantically
lossless I2F fallthrough forms, including Immediate20, without broadening unknown
control-flow admission. Regression coverage checks both extent walkers, four
rounding modes, separate immediate-sign encodings and rejected reserved fields.
Authenticated replay now submits 60 ordered variants per fixture case (180 real
IPC draws), including uniform/immediate I2F for both signs and all four rounding
encodings. It verifies native canonical pixels, payload refresh, semaphore-after-
writeback ordering, durable shader failure and exactly-once cleanup. Whole-
workspace ordinary tests, all 15 explicit native regressions and strict workspace
all-target Clippy pass.
The actual Snipperclips rerun completes 199,328,011 interpreted instructions,
then stops at PC 0x7102649744 on GPU-control ioctl 0x80044701 (ZCullGetCtxSize).
No frame is verified. A key-name-only inventory confirms master/application
entries stop at revision 14 hex, and no alternate local key file was found
beyond prod.keys/title.keys. Matching revision 15 remains required to inspect
the supplied generation-22 NV firmware; user input was requested without asking
for key values in chat. Other GPU implementation work remains possible.

I2F Immediate20 source encoding is now decoded and lowered, preserving the
separate bit-56 sign and sign-extending the 20-bit word before independently
applying I32/U32 conversion semantics. Boundary regressions cover -524288, -1,
0, 1 and 524287, both types, all rounding encodings and predicate guards. I2F
admission now checks fixed opcode bits explicitly; reserved bits 48/50 that the
previous broad high-word mask allowed are rejected for register, uniform and
immediate forms. The persistent Linux fixture adds 24 encoded immediate draws
with visible/clipped/restored coverage; non-memory forms use a conflicting
uniform payload to catch accidental buffer-source reuse. Whole-workspace tests
passed before the final mask tightening; the final GPU IR tests, all 15 native
regressions and strict workspace Clippy pass. This does not change the verified
Snipperclips ZCull startup frontier or establish a game frame.

Encoded I2F now also admits a direct constant-buffer source, retaining its actual
slot and unsigned aligned byte offset and loading the word into the conversion's
own destination. Predicate guards cover both load and conversion. Register-source
RZ is explicitly lowered to zero for the modifier-free word subset, never to an
undefined ordinary register; discard destinations and modified RZ remain refused.
Regressions cover buffer slots 0/3/31, offsets 0/28/0xfffc, guards and unsupported
fields. The persistent Linux fixture executes 24 direct c[0][0] conversions
(signed/unsigned, four rounding encodings, live zero/clipped/restored payloads)
and eight encoded RZ conversions, checking actual native pixels and tiled guest
writeback. Whole-workspace ordinary tests, all 15 explicit native regressions
and strict all-target Clippy pass. Snipperclips remains at ZCull initialization;
this extends real shader translation, not verified game rendering.

Runtime-valued plain I32/U32-to-F32 conversion is now emitted as core integer
SPIR-V operations: magnitude, highest-bit detection, truncated significand,
explicit nearest-even/directed rounding, exponent carry and sign construction.
This does not require optional floating-point rounding capabilities, silently
change a requested mode, or specialize a dynamic VertexID to a constant.
Unsupported modifiers, other source/destination types and unspecified rounding
remain rejected. Constant folding retains its fast path; unknown values now
remain real ConvertRich operations, with undefined-source rejection at emission.
An interpreter of the emitted integer instructions checks 4,184 signed/unsigned
boundary and sampled cases against an independent f64/F32-neighbor oracle,
including all evaluated shift counts. The encoded vertex_i2f_spirv diagnostic
passes spirv-val for all eight sign/rounding variants. The persistent Linux
fixture additionally executes live encoded VertexID ALD + I2F, signed and
unsigned, all four modes, with visible/clipped/restored coverage and canonical
tiled writeback matching native readback. Whole-workspace ordinary tests, all
15 explicit native regressions and strict workspace all-target Clippy pass;
Snipperclips startup is still at ZCull, without a verified game frame.

The persistent Linux native indexed fixture now executes encoded scalar ALD
InstanceID followed by encoded I2F R9,R9 from live guest shader memory. All four
rounding encodings are exercised for unsigned 0/2/0 and signed 0/-1/0: zero
renders the triangle, numeric depths 2 and -1 clip it, and restoring zero renders
again. Canonical tiled writeback equals native readback in every case. This
distinguishes numeric conversion from the preceding raw-bit fixture (unsigned
integer 2 would otherwise become a tiny positive float), while unit tests remain
the evidence for non-exact rounding boundaries. W stays 1 from the actual ALD
vertex payload; only its redundant MOV is replaced and restored. All 15 explicit
native regressions, 310 ordinary GPU IR/runtime/Linux tests and strict workspace
Clippy pass. Conversion is constant specialization, not general runtime I2F;
no new game frame or ZCull startup progress is claimed.

Vertex interface lowering now folds proven constant I32/U32-to-F32 conversions
with integer-only IEEE significand rounding for all four captured modes. This
allows an already specialized InstanceID constant to acquire numeric float
semantics instead of relying on a register-bit reinterpretation. Unsupported
modifiers and runtime-valued conversions remain rejected. Conservative ordered
constant tracking invalidates knowledge on unmodeled register effects, avoiding
stale values or forward definitions. Regressions cover ties-to-even, directed
rounding, signed minimum, unsigned maximum, overwrite and use-before-definition.
310 ordinary GPU IR/runtime/Linux tests and strict workspace all-target Clippy
pass. The generation-22 firmware key entry is still absent in the local key file;
ZCull startup and real Snipperclips rendering remain unverified.

Independent comparison with Mesa GM107 emitRND corrected the preceding I2F
rounding table: hardware encoding is nearest/minus-infinity/plus-infinity/zero
for values 0/1/2/3. The shared float-conversion table had the same zero/minus
inversion and is corrected too, with a separate F2I/F2F regression. This
supersedes the preceding claim that all four modes were already correct.
308 ordinary GPU IR/runtime/Linux tests, 15 explicit SwiftShader native
regressions and strict workspace all-target Clippy pass. These regressions
certify existing paths, not native conversion execution or a Snipperclips frame.

Register-source I2F decoding now explicitly admits only 32-bit signed/unsigned
integer to F32 conversion, preserving all four rounding modes, absolute/negate
modifiers and predicate guards. Unsupported sizes, subword selectors and
condition-code output are rejected rather than described as signed 32-bit input.
The older conversion regression now uses actual encoded word-size fields.
Two targeted regressions cover the admitted controls and rejected fields;
307 ordinary GPU IR/runtime/Linux tests and strict workspace all-target Clippy
pass. This is accurate conversion IR, not native ConvertRich execution: SPIR-V
conversion support and the unchanged Snipperclips ZCull startup remain open.

Authenticated IPC replay now also executes encoded VertexID reads with independent
shader-ID and signed fetch bases. Each captured case runs three extra variants
using index values 1/2/3, fetch base -1 and changed/restored shader ID bases; native
coverage and canonical color writeback are checked before semaphore completion.
This exercises live register capture, event-aware compilation and immutable
shader/pipeline cache identity on one actual channel, not a host-only shader
fixture. The final shader fault remains durable on this VertexID submission.
All three native Linux tests, 305 ordinary GPU IR/runtime/Linux tests and strict
workspace Clippy pass. Direct InstanceID is native-tested but its dedicated IPC
shader-read fixture remains unfinished; Snipperclips ZCull startup is unchanged.

The installed Linux shader provider now prepares from the full draw event and
admits direct scalar ALD VertexID at a[0x2fc] with explicitly captured generation,
replacement and ID-base controls. Indexed IDs use shader ID base minus fetch
element base; non-indexed IDs with zero ID base follow the captured add-FIRST bit.
Replacement, reserved/missing controls, indirect/vector reads and nonzero array
ID bases remain unsupported. IR regression checks independent signed fetch base
and shader ID base, both array modes and missing/replacement rejection. The native
encoded fixture changes actual VertexID base 0/0x40000000/0 without moving guest
vertex fetches, verifying changed/restored native pixels and canonical writeback.
Sources:
https://github.com/envytools/envytools/blob/master/rnndb/graph/gf100_3d.xml
https://github.com/devkitPro/uam/blob/master/source/nv_attributes.h
https://chromium.googlesource.com/external/gitlab.freedesktop.org/mesa/mesa/+/6f19fc972f4d3004a6719ccbcca221e33098a481/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c
This is supported draw translation, not a verified Snipperclips frame or ZCull
startup progress.

Canonical LoadVertexIndex now emits the signed SPIR-V VertexIndex input, preserves
its integer register bits and applies an explicit wrapping 32-bit bias with IAdd.
An ordinary emission regression checks the builtin decoration/integer operation
and rejects fragment use. The native shared vertex/index allocation fixture
exercises indexed base vertex -1 and bias 0/0x40000000/0, verifying real pixels
change then restore with distinct immutable shader/pipeline IDs. Guest Maxwell
VertexID ALD is still rejected pending captured generation/base/replacement policy;
this is native backend capability, not an admitted game VertexID implementation.
Snipperclips remains at ZCull initialization without a verified game frame.

Direct scalar vertex ALD at a[0x2f8] now specializes to the exact captured guest
InstanceID as integer register bits during one-instance shader preparation.
This avoids incorrectly exposing Vulkan instance zero after fetch rebasing;
immutable shader/pipeline identities include the specialized content. Missing
history, vector/indirect system reads and unsupported VertexID remain rejected.
IR regression checks bit preservation and malformed admission; the encoded native
fixture changes captured IDs 0/0x40000000/0 and verifies real coverage and canonical
writeback change then restore on one cached renderer. This bit-pattern fixture
does not claim integer-to-float instruction support or general system builtins.
Attribute mapping source:
https://github.com/devkitPro/uam/blob/master/source/nv_attributes.h
Snipperclips has no verified frame and its ZCull startup frontier is unchanged.

Authenticated Linux IPC replay now verifies ordered instance continuation on one
real channel across submissions. Each encoded fixture case runs nine variants:
ordinary, indexed, restart and instance IDs 0/1/2/2/3/0 via BEGIN reset/next/keep.
A separate scalar instance-depth stream with divisor two changes actual native
coverage; canonical color writeback is checked before ordered semaphore release.
The admission trace independently checks captured instance IDs, and the final
shader-fault submission remains durable. All three native Linux tests, 302
ordinary GPU IR/runtime/Linux tests and strict workspace Clippy pass. Shader
InstanceID builtins and general game rendering remain unfinished; this does not
advance the verified Snipperclips ZCull initialization frontier.

An encoded scalar ALD instance-depth fixture now proves selected instance fetches
change native pixels: divisor two keeps instances 0/1 visible and clips 2/3,
then returning to instance zero restores visibility. Vertex positions and instance
depths use separate real streams in one canonical allocation; every native result
is checked against tiled guest writeback. This exposed and fixed topology parsing
that incorrectly included BEGIN instance flags in primitive kind. IR regression
covers admitted continuation with valid history and rejected missing history.
Native execution is verified through explicit captured snapshots here; authenticated
ordered continuation replay and shader InstanceID still need integration. The real
Snipperclips frontier remains ZCull initialization, with no game frame claimed.

Captured instance history now selects actual per-instance fetch elements using
instance_id / divisor for each one-instance draw. Native bindings are rebased to
the selected canonical element, with checked offsets and unchanged allocation
identity/end; indexed and vertex-array preparation share this selection. Unknown
BEGIN flags/history remain rejected. Runtime regressions verify divisor-three
selection across instances 1, 2, 3 and 11, and reject instance 12 beyond the live
stream limit. All 15 native Vulkan/Linux tests and strict workspace Clippy pass.
This models fetch selection only: shader InstanceID builtins, native batching and
an authenticated continuation pixel fixture remain unfinished. No real game
frame or progress past Snipperclips ZCull startup is claimed.

Ordered BEGIN state now retains a checked instance counter in immutable draw
snapshots. Ordinary BEGIN resets it, INSTANCE_NEXT increments it and INSTANCE_CONT
preserves it across END. Unknown flags, mutually exclusive flags, missing prior
history and counter overflow remain invalid rather than becoming instance zero.
A new IR regression covers sequence, frozen snapshot and overflow behavior.
This captures history only: continuation draws are still rejected by current
draw assembly until their instance selection/native submission is connected.
Register semantics source:
https://github.com/envytools/envytools/blob/master/rnndb/graph/gf100_3d.xml
No real-game frame or ZCull advancement is claimed.

Vertex input selection now reads the explicit per-stream instance boolean at
byte address 0x1880 rather than ignoring it or inferring rate from the divisor.
Missing/reserved controls and unsupported zero instance divisors are rejected.
For the current single-instance path, nonzero divisors select element zero;
indexed and vertex-array preparations use the correct native input rate. Runtime
regressions cover missing/reserved/zero-divisor admission, and the encoded native
fixture changes the same stream from vertex to instance rate and back on one
renderer, checking actual pixel changes. All 15 native tests and strict workspace
Clippy pass. Multi-instance draws, general divisors and instance continuation are
not implemented. Register/driver sources:
https://github.com/envytools/envytools/blob/master/rnndb/graph/gf100_3d.xml
https://chromium.googlesource.com/external/gitlab.freedesktop.org/mesa/mesa/+/6f19fc972f4d3004a6719ccbcca221e33098a481/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c
Snipperclips remains at the verified ZCull startup frontier without a game frame.

Restart topology admission is now checked in resource-plan analysis before any
payload read or native driver operation, not merely at pipeline creation. One
shared IR capability predicate is used by indexed assembly, plan validation and
Vulkan creation. A new regression covers all three supported strip/fan topologies
and rejects list, loop, patch and unknown forms. All 303 ordinary GPU IR/Vulkan/
runtime tests and strict workspace Clippy pass. The real-game frontier remains
the previously measured ZCull query; this admission change does not claim a frame.

Primitive restart is now explicit immutable graphics pipeline state and reaches
native Vulkan input assembly. The indexed compiler accepts standard all-one
markers only for triangle strip/fan and line strip; custom markers and list
restart remain rejected. Native fetch validation skips markers before signed
base-vertex addition and handles all-marker draws without fake vertex reads.
Ordinary regressions cover enabled/disabled markers, all-marker input and negative
fetches; pipeline identity distinguishes restart state. Authenticated queue replay
now includes three real native two-strip restart draws with canonical writeback
and ordered semaphore release. Vulkan semantics reference:
https://docs.vulkan.org/refpages/latest/refpages/source/VkPipelineInputAssemblyStateCreateInfo.html
No Snipperclips frame or change to the verified ZCull startup frontier is claimed.

Fresh real Snipperclips validation after indexed queue integration completes
199,328,015 interpreted instructions and stops at PC 0x7102649744 on unsupported
GPU-control ioctl 2147763969 (0x80044701, ZCullGetCtxSize). This supersedes older
instruction counts, not the unchanged startup frontier. No game frame, menu,
audio or gameplay is verified. Full workspace ordinary tests and strict all-target
Clippy pass; all 15 explicit Vulkan/Linux native tests pass on SwiftShader (the
arithmetic fixture may certify capability rejection rather than execution).
A key-name-only recheck still finds neither master_key_15 nor
key_area_key_application_15 in the local prod.keys; values were not printed.

Indexed controls (signed vertex element base and restart enable/marker) are now
captured state writes, with validity checked at draw preparation. Authenticated
Linux IPC queue replay alternates three non-indexed and three indexed draws on
one real channel. Index FIRST skips a sentinel, proving its binding offset is
applied once; native pixels match canonical writeback before the following
semaphore release. Shader fault handling remains durable on the final indexed
submission. The expanded native IPC test, 301 ordinary GPU/runtime/Linux tests
and strict Linux Clippy pass. This certifies the supported synthetic queue path,
not a real game shader/frame, primitive restart or Snipperclips ZCull startup.

Mixed indexed/non-indexed rendering exposed a cached allocation usage mismatch:
the same canonical buffer initially needed vertex/uniform roles, then gained an
index role. Linux now assigns a stable native input capability superset before
first submission, preserving canonical identity, actual size and guest bindings.
The encoded native test now alternates both draw kinds on one persistent renderer
and changes/restores live indices without replacing its cache. All three native
Linux and 13 ordinary Linux tests pass; strict Linux Clippy passes. Indexed queue
admission and real game rendering remain separate unverified milestones.

The Linux renderer now accepts captured indexed draw events, reads canonical
indices, selects full backing allocations, assigns immutable shader/pipeline
identities and executes the assembled color module through the existing native
load/readback/writeback path. The standalone encoded-shader test compares indexed
and non-indexed native pixels, then changes live indices to a degenerate triangle
and restores them on the same indexed renderer. All three native Linux tests and
13 ordinary Linux tests pass on SwiftShader; strict Linux Clippy passes. Indexed
PFIFO control-word admission/authenticated queue replay still needs verification,
as do mixed usage transitions in one native cache. Restart and general game
rendering remain unsupported; Snipperclips has no verified frame or ZCull progress.

Single-color indexed module assembly now shares shader/interface, fixed-function,
attachment and full-allocation validation with non-indexed assembly. It validates
one index binding, format/alignment, exact canonical range, signed base vertex,
topology and restart-disabled state before emitting DrawIndexed. The encoded
Linux fixture checks the assembled module preserves the one shared index/vertex/
uniform allocation. Native indexed execution through the Linux renderer remains
unconnected; this module-assembly check is not evidence of an indexed game frame.

Captured indexed inputs now assemble vertex/index bindings and DrawIndexed with
the actual count and signed base vertex. FIRST stays exclusively in the prepared
index binding offset; first_index is zero, avoiding double application. Empty
draws emit no fetch commands. Primitive restart is explicitly rejected until the
graphics pipeline descriptor represents it. The canonical index regression
checks command count, binding uniqueness and restart rejection. This does not
yet connect indexed draws to color module assembly or Linux submission.

Indexed allocation selection now revalidates the captured index range and binding
against the current resource snapshot and combines index, vertex and uniform
roles into one full-size canonical allocation. It rejects mismatched index
offsets rather than accepting stale prepared bindings. The shared encoded-shader
Linux fixture verifies all three roles alias the same backing identity, including
through the authenticated channel fixture. All three native Linux tests pass on
SwiftShader. This is allocation preparation, not complete indexed draw submission;
no Snipperclips frame or progress beyond ZCull initialization is claimed.

The native vertex-attribute regression now actually places vertex payload and
indices in one full IndexVertexUniform allocation, binding two real subranges
under the same resource ID. Baseline rendering matches reference pixels;
updating only the shared allocation's index subrange produces a degenerate draw,
and restoring those indices restores the image with stable IDs and cached native
state. The selected native regression and strict Vulkan Clippy pass. This verifies
shared index/vertex consumption and refresh, not uniform consumption in that same
allocation or complete canonical guest indexed submission.

BufferUsage::IndexVertexUniform now represents one real allocation with combined
index, vertex and uniform read roles. Vulkan creation flags, input upload/refresh,
fingerprinting, binding validation and index-refresh admission recognize it without
creating separate resource IDs. Existing usage tags remain unchanged; the new
resource-table tag is distinct. The native attribute/index fixture now selects
this combined usage for its index allocation. Full canonical shared index/vertex
allocation selection and installed guest indexed draw assembly remain unfinished;
combined capability alone does not prove all roles rendered from one allocation.

Captured indexed preparation now resolves the exact real index subrange into
backend-neutral BindIndexBuffer metadata, preserving resource identity and byte
offset rather than using a guest address as an ID. The captured first index is
incorporated once in the binding offset; a future native DrawIndexed must not
double-apply it. Read permission and index-type alignment are required. Zero-count
draws produce no index range/binding and read no index memory. Canonical index
tests cover real offset/type selection and misaligned resource offsets. This is
binding preparation, not completed indexed draw assembly or Linux execution.

Index-bound preparation now rejects a primitive-restart marker that cannot fit
the selected index format before reading or allocating guest index bytes. An
unmapped-memory regression distinguishes invalid restart metadata from genuine
mapping errors; zero-count draws retain the no-index-read behavior. Both canonical
index tests and strict runtime all-target Clippy pass. Full indexed draw assembly
and Linux submission remain unfinished, and real Snipperclips ZCull startup is
unchanged.

Indexed per-vertex preparation now derives its selection from the actual
DrawIndexBuffer capture: the count, index descriptor, signed VB_ELEMENT_BASE and
explicit primitive-restart controls, without caller-invented selection values.
Missing controls and unsupported enable words reject; actual live index values
still validate negative effective vertices, mapped streams and buffer limits.
The canonical index regression covers captured preparation and these failures.
This prepares fetch metadata only: instance inputs, VertexID semantics, complete
indexed pipeline assembly and installed Linux indexed rendering remain open.
Register/count sources:
https://github.com/envytools/envytools/blob/master/rnndb/graph/gf100_3d.xml
https://github.com/chaotic-cx/mesa-mirror/blob/main/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c

Independent shader extent inspection is now capped at the next captured program
entry as well as the software budget. An unterminated shader cannot borrow an
adjacent shader's EXIT; shared code tails remain unsupported pending explicit
ownership/control-flow handling. A canonical-memory regression rejects that case
and accepts both programs once each has its own valid termination. Provider tests,
strict Linux Clippy and all three native Linux tests pass. Local key-name-only
revalidation still finds no master_key_15/key_area_key_application_15 entry for the
generation-22 NV firmware; no key values were read into tool output. Matching dump
material remains needed for that firmware inspection, not for all GPU development.

The installed direct-program provider now validates constant-group indices at
construction and admits exactly one vertex plus one fragment stage before reading
shader memory. Missing, duplicate, geometry/tessellation and extra-stage programs
reject before native device creation; malformed discovery and genuine mapping
errors retain their existing causes. An ordinary regression checks unsupported
pairs against completely unmapped memory and verifies that the supported pair
reaches the real memory error. Provider regressions and strict Linux Clippy pass.
Additional shader stages remain unsupported; this is admission validation, not
an expansion of rendering support or progress past Snipperclips ZCull startup.

Whole-workspace verification after installed Linux queue rendering and durable
fault coverage: offline workspace tests and strict workspace all-target Clippy
pass. All 12 explicit native Vulkan and all three native Linux tests pass on
SwiftShader. The arithmetic capability regression verifies rejection only because
the selected device lacks required Float32 modes; arithmetic execution remains
unverified. These regression gates do not establish real game rendering, window
presentation, audio or gameplay. Snipperclips ZCull startup remains unresolved.

The authenticated native Linux queue regression now also replaces a live shader
instruction with an unknown encoding after successful rendering. Actual IPC
submission is admitted, but kernel polling reports a durable draw fault without
modifying the previous target allocation or releasing the following semaphore.
Restoring the instruction does not automatically replay the faulted descriptor.
The channel still closes through ordinary IPC cleanup. This tests failure fidelity
for the installed draw path, not recovery policy or progress past game ZCull setup.

The authenticated native Linux fixture now replays all three vertex/uniform
scenarios through actual nvdrv queue allocation, object allocation, SubmitGpfifo
IPC and kernel GPU polling with the installed Linux renderer. Admission leaves
the guest completion semaphore zero; polling retires one descriptor, produces
the expected canonical pixels and executes the following FIFO release. Idle
polling retires nothing, and channel close cleans up the used provider once.
This exposed fixed-function and vertex-fetch register writes that were wrongly
emitted as opaque work. They now update captured state without executing work;
draw preparation still validates values. Typed viewport/scissor updates remain
intact; reserved scissor words and unknown methods remain opaque, barriers remain
real events. All 150 GPU IR, 137 runtime and ordinary Linux tests pass, alongside
all three native Linux regressions on SwiftShader. This is fixture-based full
queue integration, not a real Snipperclips frame or resolution of ZCull startup.

A new explicit native Linux regression invokes LinuxDrawRenderer with an authentic
kernel-owned channel identity and canonical process memory/address space after
real nvmap allocation, reservation and alias mapping IPC. Shared encoded fixture
draws produce expected native and canonical pixels across vertex/uniform updates,
reuse one per-channel Vulkan target, then install the used renderer in the native
driver for actual IPC channel close and once-only provider cleanup. All three
native Linux tests pass on SwiftShader. Obsolete unused host smoke shader/pipeline
scaffolding was removed from this fixture; only actual encoded programs render.
Draw events are still supplied by the test through bound kernel access, not guest
GPFIFO submissions; that full queue integration remains a separate gate, as does
real Snipperclips rendering and the unresolved ZCull startup query.

Direct BRA semantic lowering now shares its exact ordinary-encoding validation
with extent discovery. Unrepresented condition-code tests and branch modifiers
remain non-lossless instead of being discarded. Target arithmetic rejects negative,
overflowing and unaligned targets rather than clamping negatives to zero; predicate
guards remain explicit in canonical IR. A regression covers ordinary/predicated
branches and each rejection category. All 149 GPU IR tests pass. This fixes
translation fidelity but does not claim native branch execution or ZCull progress.

Direct program discovery now has an authenticated canonical-memory regression:
real HIPC/CMIF nvmap Create/Alloc and address-space reservation/mapping expose
vertex/fragment fixture programs through bound channel access. The same provider
observes an in-place EXIT replacement and discovers a larger extent at the same
GPU address. Native UnmapBuffer then causes typed shader-memory failure, proving
neither stale extent nor copied mapping reuse. All four authenticated Linux IPC
tests and strict Linux all-target Clippy pass. This verifies metadata discovery
through real driver ownership, not shader execution or any actual game frame.

DirectDrawProgramProvider replaces the linear-only provider in Linux composition.
Extent discovery now traverses certified ordinary PC-relative BRA edges and
predicate fallthrough with a visited set. Direct targets must identify aligned
instruction words inside the inspection budget, never scheduling words or header
bytes. Backedges terminate inspection without claiming shader execution terminates;
at least one reachable EXIT and a readable complete prefix are required. Calls,
indirect targets, reconvergence and uncertified branch modifiers still reject.
All 148 GPU IR tests and strict CLI/GPU all-target Clippy pass. This expands extent
discovery, not native branch execution or the unchanged Snipperclips ZCull frontier.

Fresh Snipperclips probe with the installed Linux draw provider completes
199,328,194 interpreted instructions, then stops at PC 0x7102649744 on
/dev/nvhost-ctrl-gpu query 0x80044701 (ZCullGetCtxSize). This is the latest probe
count; the unchanged startup failure occurs before any real accepted draw.
Provider installation therefore advances composition, not the game frontier.

The default Linux runtime and CLI content CPU probe now share a native-driver
factory that installs LinuxDrawRenderer with LinearDrawProgramProvider. Vulkan
creation remains lazy, and unsupported shaders/draws still produce typed failures
rather than fallback output. The current supported vertex/fragment composition
uses explicit constant groups 0/4 and host descriptor sets 0/1 with a 1 MiB shader
inspection budget and 256-channel software capacity. All 10 ordinary Linux tests
and strict CLI/Linux all-target Clippy pass. Actual accepted driver rendering,
general shader control flow, and ZCull initialization remain independent gates.

LinearDrawProgramProvider now implements the Linux program-provider interface
using captured shader entries and bounded live canonical GuestShaderMemory reads.
It discovers extents on every draw rather than caching by GPU address; stage-group
ownership remains explicit. The encoded vertex/uniform native Linux regression
now renders using these discovered windows instead of caller-supplied lengths.
Both native Linux tests pass on SwiftShader. Structural decode gaps for constant
MOV/MOV32I are admitted only when their semantic forms are certified; unknown
encodings and unanalyzed control flow still reject. This provider is not installed
for the actual game yet, and general CFG discovery and ZCull startup remain open.

Straight-line graphics shader extent discovery now reads a bounded live GPU
memory prefix, accounts for scheduler words, and stops only at a semantically
valid unpredicated EXIT. Conditional EXITs do not truncate remaining lanes;
branches, calls, reconvergence and unknown opcodes reject instead of inventing
an extent. Missing termination, malformed output and unmapped reads reject.
This is a limited building block, not general control-flow analysis or an installed
game program provider; firmware-derived ZCull size and real game rendering remain
unverified.

Authenticated Linux adapter lifecycle coverage now also closes sessions with two
still-live bound channels and exits their owner process. Both paths notify the
installed program provider exactly once for each canonical channel; subsequent
process teardown does not duplicate notifications. All three authenticated IPC
adapter regressions and strict Linux all-target Clippy pass. These channels have
not drawn, so native Vulkan cache destruction after successful rendering remains
an independent gate. Automatic program extents must account for conditional exits
and control-flow targets; mapping tails or the first decoded EXIT alone are not
accepted as verified program windows.

The Linux adapter is now installed inside NativeGpuBackend in an authenticated
IPC teardown regression. Closing either of two real nvdrv channels automatically
forwards its exact canonical identity to the program provider, invalidates that
channel's kernel binding and preserves the other channel. Closing the session
after both channels does not duplicate cleanup. Both authenticated Linux adapter
tests and strict Linux all-target Clippy pass. This verifies driver-triggered
cleanup for unused channels; successful Vulkan cache teardown after an accepted
draw and positive authenticated rendering are still unverified.

Linux adapter admission now has an ordinary regression using real HIPC/CMIF
nvdrv initialization, device opens and channel/address-space/nvmap binding.
Two independent authenticated sessions with equal descriptor numbers produce
distinct renderer-provider identities. Rejected program metadata preserves the
typed Linux error source and creates no Vulkan state; explicit repeated adapter
cleanup forwards the same identities to an idempotent provider. This covers
negative admission, not successful drawing or automatic driver-triggered teardown.
Positive authenticated rendering and the real-game program provider remain open.

Verification checkpoint after Linux renderer adapter integration: all 148 Horizon
and 137 runtime tests pass, along with the ordinary Linux suite. All 12 explicit
native Vulkan regressions and both native Linux regressions also pass on
SwiftShader. The arithmetic capability test verifies rejection only: this device
lacks the required Float32 modes, so arithmetic execution remains unverified.
These are fixture-based regression results, not evidence of game rendering.
Authenticated adapter lifecycle coverage, a real-game program-window provider,
and Snipperclips ZCull startup remain open; no real game frame is claimed.

LinuxDrawRenderer now implements the driver's NativeDrawRenderer interface,
resolving verified program metadata before creating isolated persistent Vulkan
state for each canonical channel. Channel capacity is bounded; teardown drops
native caches and forwards idempotent provider cleanup. Original Linux errors
remain runtime draw-error sources. Both existing SwiftShader Linux regressions
and strict runtime/Linux all-target Clippy pass. The adapter is not yet exercised
through authenticated native driver IPC, and no real-game program-window provider
is installed; these integration requirements remain open alongside ZCull startup.

GuestColorRenderer now owns persistent per-channel Linux Vulkan state and combines
live catalog refresh, captured program/interface preparation, real allocation
selection, immutable shader/pipeline IDs, mapped attachment identity and canonical
render/writeback. Image identities include captured target and actual backing
resource/offset, avoiding reuse across replaced allocations; buffer/image ID
collisions reject. The encoded Linux fixture now calls this renderer directly
across vertex/uniform updates and verifies one cached image plus expected pixels.
Both SwiftShader Linux tests and strict Linux all-target Clippy pass. Windows and
stage-group policy remain verified caller inputs; authenticated driver adapter
installation and actual Snipperclips startup remain unfinished.

GuestGpuResourceSnapshot now selects draw allocation descriptors from actual
vertex ranges and validated cross-stage uniform bindings. Full allocation sizes
come from the live catalog, missing/duplicate metadata rejects and combined roles
produce one VertexUniform descriptor. Existing range/uniform allocation validators
check the selected records again. The complete Linux encoded-program fixture now
uses this selector instead of host-built buffer metadata and asserts one shared
full-sized allocation. Both SwiftShader Linux tests and strict runtime/Linux
all-target Clippy pass. Verified program-window discovery and production renderer
installation remain outstanding; actual Snipperclips boot is not advanced.

Canonical buffers now support explicit VertexUniform usage for one real allocation
consumed by both vertex fetch and uniform descriptors. Preparation, native usage
flags, descriptor validation and live input refresh preserve both roles; full draw
assembly deduplicates identical allocation descriptors. The encoded Linux fixture
now aliases one backing allocation for vertices, uniforms and shader storage,
retaining nonzero offsets/full size and observed CPU updates. Both SwiftShader
Linux tests pass. Full workspace tests passed before final descriptor deduplication;
all 144 IR tests and strict workspace all-target Clippy pass afterward. Production
renderer installation and real Snipperclips ZCull startup remain unfinished.

Linux identified draw assembly now joins shader and complete pipeline identity
catalogs to the canonical guest draw path. Shader IDs use freshly compiled actual
module code rather than trusting a mutable cached PreparedShader SPIR-V field;
pipeline commands are rewritten to the selected complete-state identity. Source
prepared shaders and guest provenance remain unchanged. The complete native Linux
encoded-program fixture exercises this entry across live payload updates. Both
SwiftShader Linux regressions and strict Linux all-target Clippy pass. This still
requires verified program windows and installation of a production renderer;
Snipperclips startup remains blocked at ZCullGetCtxSize.

PipelineIdentityCatalog now assigns graphics pipeline IDs from complete canonical
descriptor state, excluding only the caller's provisional ID. Changed shader IDs
or fixed state get a fresh identity; identical states reuse IDs with checked
exhaustion. Native shader-cache regression also verifies changed code executes
under fresh shader/pipeline IDs and the original pipeline still reproduces its
pixels afterward. Pipeline identity unit test, native cache regression and strict
runtime/Vulkan all-target Clippy pass. Catalog integration into the production
Linux draw path remains required; real Snipperclips still stops at ZCull setup.

ShaderIdentityCatalog now assigns host IDs from exact compiled stage/SPIR-V content,
separate from guest program addresses. Identical code reuses IDs; changed code or
stage gets a fresh immutable identity, with checked exhaustion. The complete Linux
encoded-program fixture uses this catalog while retaining original guest program
addresses and slot provenance. Identity regression, both SwiftShader Linux tests
and strict runtime/Linux all-target Clippy pass. Pipeline identities must track
the selected shader IDs; production renderer installation and verified program
windows remain unfinished, and the actual ZCull startup frontier is unchanged.

Actual Snipperclips content-inspect probe rerun after the recent renderer/interface
changes reaches 199,328,387 interpreted instructions and stops at PC 0x7102649744
on unsupported /dev/nvhost-ctrl-gpu IOCTL 2147763969 (0x80044701,
ZCullGetCtxSize). The instruction count supersedes prior probes; the startup
frontier has not moved. Successful CLI exit means inspection completed, not game
boot. No actual frame/menu/audio/gameplay is verified. GPU draw integration work
can continue, but this driver/context requirement still blocks real startup.

Vulkan now guards immutable shader identities using the exact stage/SPIR-V record,
including preflight before resource uploads so cached graphics pipelines cannot
bypass the check. Changed code under an existing ID and conflicting duplicate
plan IDs reject instead of silently rendering cached code. Native regression
checks changed fragment constants reject without modifying pixels and the original
shader still executes afterward. All fourteen SwiftShader Vulkan/Linux regressions
and strict Vulkan all-target Clippy pass; arithmetic remains capability rejection
only. Production code changes must receive fresh shader/pipeline identities; real
game renderer installation and verified shader windows remain unfinished.

Runtime prepare_discovered_direct_draw_shaders now derives both direct constant
interfaces and vertex attributes from actual compiled programs and captured
bindings. Caller policy specifies stage groups and separate host descriptor sets,
not handcrafted slot lists or constant declaration sizes. The native Linux
encoded-program fixture uses this fully discovered interface path and still
verifies expected pixels, guest writeback and live payload updates. Both
SwiftShader Linux tests and strict runtime/Linux all-target Clippy pass. Verified
shader windows, broader shader features and installed production renderer remain
required; this does not establish a real Snipperclips frame.

Direct constant interface discovery now derives referenced slots from actual
shader constant loads and declaration sizes from valid captured group bindings.
Within an explicitly selected host descriptor set, bindings equal hardware slots;
no allocation size or byte length is invented from the largest access. Exact
lowering validates offsets, bounds and unsupported indirect loads. Regression
covers missing/wrong-group/disabled bindings, deduplication, full captured sizes,
out-of-bounds, negative/unaligned and indirect accesses. All constant-interface
tests and strict IR all-target Clippy pass. Native preparation integration and
verified program lengths remain next; no new real game draw is claimed.

Runtime prepare_direct_draw_shaders now integrates direct attribute discovery
with captured-slot program compilation and graphics preparation. Constant accesses
are resolved before attribute discovery, and the returned interface is shared by
shader lowering and native fetch assembly. The full Linux encoded-program fixture
now uses this path and checks discovered slot/location/format against expectations;
pixel/writeback and live uniform/vertex updates still pass. Both SwiftShader Linux
tests and strict runtime/Linux all-target Clippy pass. Program windows and constant
interfaces remain explicit; real-game renderer installation is still unfinished.

Post-integration audit passes the complete offline workspace test suite and strict
workspace all-target Clippy. All thirteen explicitly enabled SwiftShader Vulkan/
Linux native regressions pass; the arithmetic fixture verifies capability
rejection only, not arithmetic execution. This establishes no observed regression
across the recent driver, allocation identity, live payload and prepared-draw
changes. It is not game startup/presentation evidence: native renderer installation,
automatic interface/window discovery and Snipperclips boot remain unfinished.

Direct vertex interface discovery now derives used hardware attribute slots from
actual ALD component accesses and exact formats from the immutable draw capture.
Canonical locations equal slots by an explicit shared lowering/fetch convention.
Cross-slot loads are collected deterministically; missing formats, integer inputs,
indirect/output/per-patch accesses and unsupported shader operations reject using
the existing lowering checks. Both vertex-interface unit tests and strict IR
all-target Clippy pass. Integration into native draw preparation remains next;
verified program-window discovery and wider shader support remain unresolved.

Runtime prepare_draw_shaders now discovers the two-stage program pair from the
draw's immutable pipeline/program-region capture, compiles via canonical live
memory and resolves explicit attribute/uniform interfaces. The native Linux
complete-draw regression uses this helper and verifies retained vertex/fragment
slot provenance and zero opaque instructions, with unchanged pixel/writeback
and live-payload assertions. Both SwiftShader Linux regressions and strict
runtime/Linux all-target Clippy pass. Verified program windows and interfaces
remain caller supplied; deriving those from real game allocations and installing
the production renderer remains unfinished.

The complete native Linux draw fixture now also reads encoded Maxwell fragment
instructions from canonical GPU memory, lifts physical color-register MOVs and
prepares exports from the graphics header. Both prepared stages therefore derive
from encoded fixture programs rather than prewritten fragment IR. An unrelated
last constant cannot replace RGBA exports; covered pixels retain the expected
color across full draw assembly and canonical writeback. Both SwiftShader Linux
regressions and strict Linux all-target Clippy pass. Program windows, interfaces
and captured state remain explicitly host-supplied fixtures; actual game shader
discovery and production renderer integration are still unfinished.

Native Linux full-draw coverage now reads an encoded Maxwell vertex program from
canonical GPU memory, parses/lifts ALD, constant MOV, position AST and EXIT, then
uses prepare_vertex for explicit attribute/uniform interfaces and bindings.
The complete guest-payload draw assembler executes this translated vertex shader
with expected colors, background preservation and observed CPU vertex/uniform
updates. Both SwiftShader Linux regressions and strict Linux all-target Clippy
pass. The encoded program is a host-built fixture with an explicit verified
window; fragment code remains canonical host IR. Real game shader discovery,
fragment preparation and production renderer installation remain unfinished.

The native Linux vertex/uniform regression now exercises the complete
execute_color_vertex_array_draw_from_guest entry: prepared host shaders, explicit
captured topology/fixed/viewport/scissor/vertex state, catalog allocation metadata,
checked module assembly, live canonical payload fetch and color writeback.
Visible pixels match the explicit constant color, uncovered pixels preserve the
guest background, and uniform-only/vertex changes still suppress geometry. Both
SwiftShader Linux regressions and strict Linux all-target Clippy pass. Prepared
shaders remain canonical host fixtures, not lifted Maxwell guest programs;
production shader discovery/interface preparation and driver renderer remain open.

Linux execute_color_vertex_array_draw_from_guest now joins checked prepared-shader
draw assembly directly to live canonical payload fetch and color writeback,
without an externally supplied host payload provider. It shares the assembler's
strict supported-state/resource checks and introduces no clear or substitute
shader. Both existing SwiftShader Linux regressions and strict Linux all-target
Clippy pass. This combined entry itself still requires a complete native
prepared-shader fixture; production shader/interface discovery and renderer
installation remain unfinished, so no new game rendering is claimed.

Native Linux payload coverage now includes a real canonical uniform allocation
and explicit vertex-stage descriptor. The host input shader takes clip-space Z
from that uniform: changing only its CPU bytes from 0 to 2 suppresses visible
geometry on the same Vulkan device; a subsequent independent vertex-buffer
change remains observed. Canonical color writeback matches device pixels in all
three cases. Descriptor offset uses aligned allocation base, not an invented
device alignment. Both SwiftShader Linux tests and strict Linux all-target Clippy
pass. Actual Maxwell shader preparation and complete guest draw construction
remain required before this path can demonstrate Snipperclips rendering.

A second native Linux regression now consumes an actual nvmap-backed vertex
buffer through catalog-derived allocation metadata and the live guest resource
provider, including a nonzero allocation offset. An explicit host input shader
renders visible geometry; CPU replacement of the same vertices with clipped
geometry changes the next render to the preserved background on the same device,
proving stale cached bytes are not reused. Canonical guest pixels equal device
pixels after both draws. Both SwiftShader Linux regressions and strict Linux
all-target Clippy pass. This is host shader/input verification, not Maxwell game
shader translation; uniforms and complete production draw assembly remain open.

Linux execute_color_draw_from_guest now scopes the canonical guest resource
provider around native execution, releasing its mutable memory borrow before
color writeback. Both host-provider and guest-provider paths share identical
preflight, attachment load and readback logic. The native queue regression uses
catalog-derived mappings with this entry and preserves pixel/semaphore assertions.
SwiftShader regression and strict Linux all-target Clippy pass. Its smoke module
has no vertex/uniform payload buffers, so this verifies borrow/order integration,
not native payload consumption or production game shader execution.

GuestGpuResourceCatalog now translates current canonical nvmap mappings into IR
address-space metadata and full-sized allocation records. Weak lease identities
deduplicate aliases without confusing guest VA with native resource IDs; offsets
and read/read-write permissions are preserved. Refresh validates live ownership
and commits identity changes only on success. Regression verifies shared aliases,
nonzero offsets, cloned-memory rejection, unmap visibility and a new ID/read-only
permissions after replacement at the same address. All 135 runtime tests and strict
runtime all-target Clippy pass. Catalog integration with the Linux renderer,
prepared shader discovery and actual game draws remains unfinished.

Device leases now provide an opaque weak allocation identity for native resource
caches. Cloned nvmap leases share the token; repinning the same CPU address after
release yields a different identity. Tokens do not retain guest memory ownership,
preventing renderer caches from obstructing unmap. A regression verifies clone
identity, successful unmap with retained token and replacement-pin distinction.
Three device-memory tests and strict Horizon all-target Clippy pass. Assigning
stable native IDs from these tokens and consuming live mappings remains next.

NvGpuAddressSpace now exposes borrowed current mapping metadata for renderer
resource translation: GPU base, mapped size, actual backing offset and backing
lease. GPU addresses remain distinct from resource identities; access still
requires canonical live lease validation. Alias regression checks two mappings
of the same nonzero backing offset retain the full allocation size and disappear
from enumeration on unmap. Three address-space regressions and strict Horizon
all-target Clippy pass. Host resource identity assignment and a concrete Linux
renderer consuming this metadata remain unfinished.

The native driver integration regression now installs a renderer probe via
NativeGpuBackend::with_renderer and submits a live mapped Maxwell draw through
actual nvdrv admission and process polling. Admission invokes no renderer; polling
calls it once with the canonical channel identity and writable live memory,
retires one descriptor, and subsequent idle polling does not replay it. Descriptor
close reaches renderer cleanup. Its marker is explicitly test-only, not a pixel.
All 134 runtime tests and strict runtime all-target Clippy pass. Production Linux
Vulkan rendering/resource preparation is still not installed into this path.

NativeGpuBackend can now own an optional NativeDrawRenderer installed at
construction. The driver's normal polling path invokes it with authenticated
channel identity and live canonical memory; channel teardown forwards idempotent
renderer cleanup. Rendering cannot be replaced after descriptors are admitted,
and no renderer preserves the previous unsupported-draw behavior. All 134 runtime
tests and strict runtime all-target Clippy pass. A concrete Linux renderer with
live shader/resource preparation is not installed yet; installed-renderer driver
regression and production Vulkan implementation remain required.

NativeGpuBackend now provides poll_channel_with_draw_handler, forwarding admitted
channel work through the queue's synchronous draw path while preserving polling
budgets, waiting cursors, durable faults and retirement accounting. Transfer-only
polling delegates with the existing unsupported-draw policy. All 134 runtime tests
and strict runtime all-target Clippy pass. The nvdrv backend trait's default poll
still uses transfer-only execution; installing a production Linux renderer and
its live resource provider into that driver path remains required.

Linux now exposes execute_color_vertex_array_draw, combining the checked
single-color assembler with synchronous attachment load, Vulkan execution and
canonical guest writeback. Its borrowed request contains the captured event,
prepared shaders, actual allocation metadata and IR address-space ownership;
it introduces no shader substitute or attachment clear. Compilation errors retain
their typed source. Seven ordinary Linux tests, the existing native SwiftShader
queue regression and strict Linux all-target Clippy pass. The new combined entry
has not yet been exercised with a complete prepared-shader native fixture or
production game resource provider; those integration steps remain required.

Native Linux queue coverage now places a FIFO semaphore release after the draw.
The regression checks its memory word remains zero both before rendering and
after completed canonical color writeback inside the handler, then becomes 42
only after executor continuation. Guest pixels already equal device pixels at
handler return. A separate injected draw failure retains the descriptor without
retirement or semaphore release. The SwiftShader regression and strict Linux
all-target Clippy pass. This proves ordering for the host smoke fixture, not
production game shaders or nvdrv syncpoint/fence integration.

The native Linux draw regression now starts from a live nvmap-backed Maxwell
pushbuffer admitted to GuestGpuChannel, rather than an injected decoded trace.
It verifies decoding, ordered synchronous Vulkan rendering, canonical color
writeback and successful descriptor retirement with no transfer inflation.
Runtime execution now recognizes pure color-target/shader/program/primitive state
notifications without claiming a device operation; barriers and unknown operations
remain rejected. The SwiftShader regression, all 134 runtime tests and strict
runtime/Linux all-target Clippy pass. The rendered module still uses host smoke
shaders; real game shader/resource construction and driver routing remain open.

The native Linux color regression now routes its captured draw through
GuestGpuExecutor's ordered draw hook before real Vulkan execution and canonical
guest writeback. It verifies zero transfer-count inflation and retains the prior
pixel/background-preservation assertions. Native draw failures have a dedicated
runtime error carrying the original backend error as its source. The SwiftShader
Linux regression and strict runtime/Linux all-target Clippy pass. This still uses
a host smoke shader/module; production game shader translation, handler resource
construction and scheduler integration are not demonstrated by this fixture.

GuestGpuChannel now routes draws through the synchronous executor hook, including
the canonical kernel-bound channel entry. Descriptor retirement waits for handler
success; draw failures retain the descriptor and become durable channel faults,
and stopped channels never call the handler again. Existing default callers still
reject unsupported draws. A scheduler regression verifies successful retirement,
zero transfer inflation and no retry after failure. All 134 runtime tests and
strict all-target runtime Clippy pass. Native Vulkan handler construction and
actual game execution remain unfinished; this does not claim a Snipperclips frame.

GuestGpuExecutor now exposes an ordered, resumable synchronous draw handler for
captured vertex-array and indexed draw events. Existing transfer-only callers
still reject draws; incomplete inline uploads cannot reach the handler. The
handler contract requires rendering and guest writeback to finish before success,
so later semaphore operations cannot be justified by mere queue submission.
Regression verifies prior DMA visibility, stopping before suffix transfers on
draw failure, and resuming without replaying the prefix; draws do not inflate
transfer counts. All 133 runtime tests and strict all-target runtime Clippy pass.
No native backend is connected to the channel scheduler yet and no game frame is
claimed; native draw implementation and scheduler ownership remain next steps.

Vertex allocation assembly now has ordinary regression coverage for nonzero
resource offsets, exact-end acceptance, one-byte-short rejection, shared-range
deduplication without shrinking allocation size, duplicate metadata, overflow,
empty ranges and unreadable ownership. Fourteen pipeline regressions and strict
IR all-target Clippy pass. This strengthens the complete draw assembler's resource
boundary evidence; runtime GuestGpuExecutor still has no native draw execution
hook, so no game frame or queue completion is claimed.

Complete color vertex-array assembly now checks each actual vertex shader input
location against the prepared fetch interface and rejects integer formats in this
floating-input shader path. Supplying a different location or omitting attributes
can no longer produce a nominally complete module with missing shader inputs.
Native encoded vertex/uniform fixtures verify both mismatches reject and the
matching assembled module still reproduces reference pixels. Both SwiftShader
pixel regressions, all 141 GPU IR tests and strict IR/Vulkan all-target Clippy pass;
arithmetic only checks capability rejection. Native game queue routing and broader
draw/shader support remain unfinished; no real Snipperclips image is claimed.

Single-color vertex-array module assembly now combines prepared vertex/fragment
shaders, immutable draw capture, checked vertex/uniform allocations, exact fixed
state, mapped color-layer metadata and viewport/scissor commands into one complete
portable module. It emits no fixture clear or substitute shader, requires distinct
shader IDs and rejects unsupported output mapping, multiple colors, active depth
and unsupported sampling. Native encoded vertex/uniform fixtures execute this
assembled module and match fractional/inverted-viewport reference pixels; host-only
attachment clear is inserted explicitly by the test. Negative regressions reject
multiple/remapped targets, active depth and unsupported anti-alias control. Both
SwiftShader pixel regressions and strict IR/Vulkan all-target Clippy pass; all 141
GPU IR tests passed before these final rejection cases, and arithmetic still only
checks capability rejection. Native guest queue routing, broader draw coverage and
real Snipperclips startup/frame remain unfinished.

Captured CT_SELECT decoding now preserves physical target indices in shader-output
order plus explicit MRT enable. Missing controls, reserved bits/counts and invalid
MRT enable reject; zero color outputs remain zero. Linux's single-color draw entry
requires its caller target to agree with the single captured selection before
memory/resource work. Regression checks remapped output order and native rejection
of empty, multiple, mismatched and reserved selections, even with guest mapping
unmapped, without altering Vulkan pixels. Thirteen pipeline regressions, the
SwiftShader Linux regression and strict IR/Linux all-target Clippy pass. Multi-target
assembly, output/physical-target blend mapping and native game routing remain open.

Graphics uniform allocation assembly now validates vertex/fragment descriptor
agreement before resolving either stage's actual allocation descriptors, then
deduplicates shared allocations deterministically. Regression verifies one shared
buffer at nonzero descriptor offset, exact allocation bounds, absent/one-byte-short
and wrong-usage metadata, duplicate allocations, missing stage binding and
cross-stage resource conflict. All 140 GPU IR tests and strict IR all-target Clippy
pass. This verifies allocation/module preparation, not new native game execution;
complete draw module assembly and Snipperclips queue/startup remain unfinished.

Prepared shader uniforms now select actual allocation descriptors after validating
the shader interface/layout/bindings. Missing/duplicate allocations, non-uniform
usage and descriptor ranges beyond the allocation reject with a typed resource
diagnostic; allocation size is not inferred from a constant binding's mapped size.
Native encoded vertex-uniform fixture consumes these descriptors instead of manual
module insertion and checks absent, one-byte-short and wrong-usage allocations.
Both SwiftShader pixel regressions, all 140 GPU IR tests and strict IR/Vulkan
all-target Clippy pass; arithmetic still only checks capability rejection.
Complete draw module assembly and native game queue routing/startup remain open.

Prepared vertex-array draws now retain resolved stream ranges and select actual
allocation buffer descriptors with checked offset-plus-length, readable ownership
and vertex usage. Allocation size is not fabricated from guest VA or mapped
subranges; missing/duplicate descriptors, undersized allocations and wrong usage
reject. Native encoded vertex/uniform fixtures consume these selected descriptors
and verify missing, one-byte-short and uniform-usage allocation rejection before
rendering. Both SwiftShader pixel regressions, all 140 GPU IR tests and strict
IR/Vulkan all-target Clippy pass; arithmetic only checks capability rejection.
Uniform resource assembly, complete draw modules and native game routing/startup
remain unfinished; no Snipperclips image is claimed.

Linux's single-color guest draw entry now checks each bound pipeline before
attachment upload/execution and rejects active depth/write/stencil or multisample
state that lacks the corresponding guest attachment ownership path. A missing
bound pipeline also rejects. Native canonical-nvmap regression verifies all four
unsupported fixed-state cases preserve both Vulkan image and guest layer pixels.
The SwiftShader Linux regression and strict Linux all-target Clippy pass. Native
depth fixtures remain valid in the general Vulkan backend; guest depth/MSAA
load/writeback and full game module/queue assembly remain unfinished.

Fixed-function preparation now combines captured raster, depth/stencil, blend and
single-sample state for explicitly selected active color slots. Missing state,
duplicate/invalid slots and empty selections reject rather than assigning active
defaults or implicitly choosing target zero. Inactive slots remain placeholders
only; attachment discovery/resolution is still required by draw assembly. Native
vertex/uniform fixtures consume the combined fixed record and verify mask coverage
while testing missing state in each component and invalid target selections.
Both SwiftShader pixel regressions and strict IR/Vulkan all-target Clippy pass;
arithmetic still only checks capability rejection. Full resource/shader/draw module
assembly and native game queue routing remain unfinished; no game frame is claimed.

Single-sample preparation now requires captured disabled anti-alias/alpha/sample
shader conventions and resolves explicit color-target sample masks. Missing or
reserved controls and unsupported raster-output mask mode reject; disabled masking
does not require an irrelevant mask word. Unit coverage verifies complete capture,
zero/one/full masks and invalid mask bits. Native encoded vertex/uniform fixtures
consume the prepared state: sample bit zero suppresses all coverage, bit one restores
reference pixels. Both SwiftShader pixel regressions, twelve pipeline regressions
and strict IR/Vulkan all-target Clippy pass; arithmetic only checks capability
rejection. Multisample/alpha-coverage conventions, full fixed-state/game module
assembly and Snipperclips startup remain unfinished.

Standard blend mapping now has exhaustive OGL/D3D coverage for eleven factors
and five equations, with reserved bits and unsupported constant/dual-source
encodings rejected. Native vertex/uniform fixtures additionally consume a captured
destination-alpha color factor over a fractional-alpha background, verifying
per-channel scaled color, independent output alpha and untouched uncovered pixels.
Both SwiftShader pixel regressions, eleven pipeline regressions and strict
IR/Vulkan all-target Clippy pass; arithmetic still only checks capability rejection.
This verifies a nontrivial alpha factor, not all blend combinations. Full game
fixed-state assembly/startup remains unfinished and no game image is claimed.

Shared enabled blend preparation now decodes exact OGL/D3D equations and standard
source/destination factors for color and alpha, requiring complete captured words
and explicitly shared equation control. Constant/dual-source factors, per-target
equations and logic/shared ROP state still reject pending their interfaces.
Regression checks missing words, reserved encodings and unsupported controls.
Native vertex/uniform fixtures consume captured reverse-subtract equations with
One/One factors over a white background and verify every color/alpha component
equals destination minus source, while uncovered pixels remain white. Both
SwiftShader pixel regressions, ten pipeline regressions and strict IR/Vulkan
all-target Clippy pass; arithmetic still only checks capability rejection.
Full fixed-state/game module assembly and startup remain unfinished.

Unblended target preparation now requires explicitly disabled blend/logic/shared
ROP controls and decodes exact sparse CT_WRITE component bits. Missing/reserved
state rejects; inactive blend factors cannot fabricate enabled blending. Unit
coverage verifies all sixteen component masks. Native encoded vertex/uniform
fixtures consume masks none, R, R+B and RGBA, clear all background channels to zero
and verify every resulting channel against reference coverage, including alpha.
Both SwiftShader pixel regressions, nine pipeline regressions and strict IR/Vulkan
all-target Clippy pass; arithmetic still only checks capability rejection. Enabled
blend equations, logic/shared ROP state and full game assembly remain unfinished.
No Snipperclips startup progress or real game frame is claimed.

Captured constant depth bias now has a native depth-effect regression, not just
pipeline creation evidence. Both encoded vertex/uniform fixtures map geometry
to depth 0.25 and compare against D32 cleared to 0.5: zero bias retains reference
pixels, while a large positive captured hardware coefficient converted through
raster preparation rejects all covered fragments. Color is independently cleared
on each draw. Both SwiftShader pixel regressions, 14 ordinary Vulkan tests and
strict Vulkan all-target Clippy pass; arithmetic still checks capability rejection.
This verifies bias direction/occlusion, not exact measured units, slope/clamp
behavior or guest depth-memory transfer. Full game assembly/startup remains open.

Captured depth-write behavior now has native evidence. Both encoded vertex/uniform
fixtures render Z=0 against D32 depth cleared to 0.5, once with writes enabled and
once disabled, then issue an Equal probe without clearing depth. Probe color is
independently cleared: only the write-enabled first draw yields reference coverage;
the write-disabled case retains depth 0.5 and rejects to clear pixels. Both
SwiftShader pixel regressions and strict Vulkan all-target Clippy pass; arithmetic
still checks capability rejection only. This verifies sequential depth ownership
within the native fixture, not guest depth-memory transfer or a rendered game.
Bias displacement, stencil and full game module routing/startup remain unfinished.

Native encoded vertex/uniform fixtures now exercise captured enabled depth tests
against a real D32Float attachment cleared to 0.5: Less preserves reference color
coverage and Greater rejects the same Z=0 geometry to clear pixels. This verifies
actual depth attachment/test execution rather than disabled-state submission.
Depth image allocation no longer requests the unrelated STORAGE usage bit; color
images retain their existing storage usage. All twelve selected SwiftShader
Vulkan/Linux tests pass and strict Vulkan all-target Clippy passes. Depth writes,
bias displacement, stencil faces and full game module routing remain unfinished;
no Snipperclips image or startup advancement is claimed.

Depth-state preparation now preserves explicit depth-test/write controls and maps
both exact OGL and D3D comparison encodings. Missing/reserved controls and enabled
stencil reject pending face-state support; inactive comparison and stencil fields
cannot fabricate active tests. Recognized depth/stencil control writes remain raw
captured state. Unit regression checks all sixteen comparison encodings and invalid
words. Native vertex/uniform fixtures now consume explicitly disabled captured
depth/stencil state instead of fixture defaults. Eight pipeline regressions, both
SwiftShader pixel regressions and strict IR/Vulkan all-target Clippy pass; arithmetic
still only verifies capability rejection. Enabled depth behavior with an attachment,
stencil faces and full game module assembly/startup remain unfinished.

Ash graphics pipeline caching now retains the complete immutable descriptor per
pipeline ID and rejects descriptor changes before a cache hit can silently reuse
old fixed state or bypass bias validation. Existing attachment-format variants
remain valid for an unchanged descriptor. Native vertex/uniform regressions reuse
a cached biased pipeline identity with NaN state and verify explicit identity
rejection with unchanged pixels. All twelve selected SwiftShader Vulkan/Linux
tests pass (eleven render/transfer regressions and one arithmetic capability
rejection), and strict Vulkan all-target Clippy passes. This guards pipeline
identity; full game draw assembly and Snipperclips startup remain unfinished.

Captured raster assembly now admits explicitly enabled format-dependent depth
bias with complete finite slope/constant/clamp coefficients and exact control 1.
The constant coefficient is halved, reversing Nouveau nvc0_state.c's API-units
times-two hardware programming; slope/clamp retain their words. Absolute bias and
reserved/missing controls reject. Recognized coefficient writes remain captured
state. Unit coverage verifies scaling and missing/non-finite rejection; native
vertex/uniform fixtures consume the captured raster rather than manually assigned
factors and verify pipeline submission. All 135 GPU IR tests, both SwiftShader
pixel regressions and strict IR/Vulkan all-target Clippy pass. The format-dependent
mapping is source-based; measured depth displacement with a depth attachment is
still unverified. Full fixed-state/game draw assembly and startup remain unfinished.

Portable raster state now retains explicit IEEE-754 constant/slope/clamp depth
bias factors with exact bitwise pipeline identity. Ash supplies those factors
when bias is enabled, rejects non-finite factors, enables supported depth-bias
clamp capability and rejects unsupported nonzero clamps. Disabled bias supplies
zero factors without interpreting inactive raw words. Native vertex/uniform
fixtures verify finite biased pipeline creation/submission with unchanged color
coverage and NaN rejection without image changes. These fixtures lack a depth
attachment: actual depth displacement is not verified. All ordinary workspace
tests and strict workspace all-target Clippy pass; both SwiftShader pixel
regressions pass and arithmetic still only verifies capability rejection.
Maxwell bias-unit/depth-format conversion is still unresolved and captured raster
assembly continues to reject enabled guest bias. Full game assembly/startup remains
unfinished; no Snipperclips frame is claimed.

Supported captured raster assembly now combines face selection with explicit
SET_RASTER_ENABLE, matching XYZ/no-clamp clip control and disabled polygon offset
for the active polygon mode. Missing/reserved controls, depth-clamp conventions
and enabled bias reject instead of using implicit factors (the portable raster
record does not yet represent bias factors). Recognized raster/offset writes remain
captured state. Native vertex/uniform fixtures use the assembled raster state and
independently verify disabled rasterization clears without triangle coverage,
with culling disabled, before restoring the culling matrix. Both SwiftShader pixel
regressions, six pipeline regressions and strict IR/Vulkan all-target Clippy pass;
arithmetic still only verifies capability rejection. Bias factors, remaining fixed
state and complete game draw routing remain unfinished; no game frame is claimed.

Winding-sensitive native culling is now verified rather than inferred from
front-and-back rejection. Both encoded vertex/uniform fixtures exercise eight
captured front/back-CW/CCW combinations across ordinary and negative-height
viewports. Exactly one face selection retains each triangle, retained pixels
match the appropriate reference, rejected pixels remain clear, and negative
height reverses framebuffer winding. Both SwiftShader pixel regressions and
strict Vulkan all-target Clippy pass. The arithmetic fixture still verifies only
capability rejection. This supplies raster integration evidence, not complete
game draw assembly, Snipperclips startup advancement or a real game frame.

Captured face-state conversion now decodes explicit culling enable/face, winding
and matching front/back polygon modes from NVIDIA clb197 encodings. Missing or
reserved words and independent front/back polygon modes reject; disabled culling
does not require an irrelevant face selector. These recognized register writes
remain captured state, not opaque executable events. The returned record covers
only face/polygon selection, not invented depth-clamp/bias/discard defaults.
Native vertex/uniform fixtures consume this state in a distinct immutable pipeline
and verify front-and-back culling yields only clear pixels, then restore ordinary
coverage. Both SwiftShader pixel regressions, all 134 GPU IR tests and strict
IR/Vulkan all-target Clippy pass; arithmetic still checks capability rejection.
Other raster controls, full game draw assembly and Snipperclips startup remain
unfinished. Winding-sensitive native culling coverage is not yet verified.

Raster-area preparation now combines captured viewport and scissor conventions
with an explicit render extent. Disabled scissoring emits the whole render area;
enabled bounds intersect that area and disjoint bounds retain zero coverage.
Missing state and zero/oversized extents reject, with no implicit framebuffer size
or unsupported window-origin bypass. Regression covers partial/disjoint bounds
and disabled state. Native vertex/uniform fixtures consume the combined commands
after a smaller scissor and verify disabled state restores reference coverage.
The five pipeline regressions, both SwiftShader pixel regressions and strict
IR/Vulkan all-target Clippy pass; arithmetic only checks capability rejection.
Full guest draw module assembly and Snipperclips startup remain unfinished.

Captured Maxwell viewport conversion now resolves complete scale/offset words
and explicitly captured identity swizzle, scale-offset enable, upper-left/no-flip
origin, zero window offsets and zero-to-one XYZ clipping. X/Y transform to Vulkan
origin and signed dimensions; Z offset/scale become explicit depth endpoints.
Unsupported conventions, missing words, NaN/overflow, negative X scale and invalid
depth reject rather than substituting reset state. Recognized global viewport
register writes remain raw-captured state instead of opaque executable operations.
Native vertex/uniform fixtures now obtain their fractional, inverted viewport
from this capture and match reference pixels. Both SwiftShader pixel regressions,
all 133 GPU IR tests and strict IR/Vulkan all-target Clippy pass; arithmetic still
only verifies capability rejection. Other clip/swizzle/window-offset conventions
and complete guest rasterization/module assembly remain unfinished. No real game
frame or progress past Snipperclips ZCull initialization is claimed.

Canonical viewports now retain floating-point origin/dimensions and explicit depth
range instead of integer truncation and hard-coded [0,1]. Ash preserves negative
height and reversed depth endpoints, with finite-value, physical dimension/bounds
and supported depth-range preflight before resource preparation or submission.
Native vertex/uniform fixtures compare fractional-origin, fractional-width and
negative-height rendering against reference pixels, requiring nonempty coverage;
NaN origin, negative width and out-of-range depth reject without changing pixels.
Both SwiftShader pixel regressions pass; arithmetic only verifies capability
rejection. All ordinary workspace tests and strict workspace all-target Clippy
pass. This extends the portable representation/backend; Maxwell viewport transform,
depth convention, window offsets and full game draw assembly remain unfinished.
No new Snipperclips startup progress or game frame is claimed.

Captured scissor decoding now preserves explicit enable/disable state and decodes
the unsigned 16-bit bounds from complete captured registers. Missing enabled
bounds, reserved enable bits, reversed rectangles and invalid slots reject;
empty rectangles remain empty, not replaced with full-screen rendering. Coordinates
remain in guest scissor space: window offset and render-area assembly are still
required. NVIDIA's public clb197 header verifies register addresses and fields.
Native vertex/uniform fixtures consume a captured nonzero-origin rectangle and
check every pixel: reference pixels inside, clear pixels outside, with nonempty
colored coverage. Both SwiftShader pixel regressions pass; arithmetic still only
verifies capability rejection. All 132 GPU IR tests and strict IR/Vulkan all-target
Clippy pass. Viewport lowering, full guest draw assembly and Snipperclips startup
remain unfinished; no game frame is claimed.

Non-indexed draw preparation now consumes the immutable DrawVertexArray event,
checks each explicit shader attribute over its selected first/count range, merges
stream bindings and retains captured topology and exact draw parameters.
BEGIN instance/split flags reject even when separate topology overrides the opcode;
they cannot be discarded into a fabricated single-instance draw. Native attribute
and uniform fixtures now use this preparation for their pipeline input and draw
command, and reject a fourth vertex beyond the captured stream limit. All 131 GPU
IR tests, 14 ordinary Vulkan tests and strict IR/Vulkan all-target Clippy pass.
Two SwiftShader vertex pixel regressions pass; the arithmetic fixture separately
verifies capability rejection, not arithmetic execution. Complete shader/fixed-state
module assembly, native queue routing and real Snipperclips startup remain unfinished.

Primitive topology decoding now distinguishes BEGIN encodings from separate
SET_PRIMITIVE_TOPOLOGY state and honors explicitly captured topology control.
Missing control/state and unsupported encodings do not fabricate defaults.
Regression verifies register application and immutable draw capture, including
the distinct meanings of raw value 1. All 130 GPU IR tests and strict all-target
GPU IR Clippy pass. Guest draw assembly/queue routing and the Snipperclips ZCull
startup frontier remain unfinished; no game frame is verified.

The legacy pipeline color-target descriptor helper now uses complete captured
registers and the validated selected-layer footprint/address, replacing incorrect
block-height/depth shifts and implicit layer defaults. Its unresolved image is
one selected layer with exact canonical format; unsupported pitch layouts reject.
The raw-format mapper shares canonical lowering and preserves unknown encodings.
Regression covers selected-layer address, block height, missing capture and reserved
format bits; strict GPU IR Clippy passes. Actual guest draw module assembly/native
queue routing remains unfinished; no Snipperclips image is verified.

Native Linux draw-entry regression now verifies an incompatible transfer command
is rejected and a read-only canonical nvmap lease fails writable preflight before
attachment upload/drawing. Both device image pixels and guest pixels remain
unchanged after these failures. The SwiftShader regression and strict Linux
all-target Clippy pass. This verifies rejection ordering in the explicit module
entry point, not native Maxwell queue integration or a rendered Snipperclips frame.

Linux single-color draw execution now provides one entry point for checked module
target identity/shape, canonical writable-lease preflight, guest attachment load,
Vulkan plan execution and guest pixel writeback. Unsupported command/resource
shapes reject instead of silently bypassing this ownership path. The native
SwiftShader regression consumes this entry point and still preserves background
pixels and exact guest writeback. This accepts an already translated module;
Maxwell draw translation/native queue routing and guest fences remain unfinished.
No actual Snipperclips shader or frame is verified.

Integrated attachment-transfer verification: all ordinary workspace tests and
strict workspace all-target Clippy pass after target preparation, tiled conversion
and Linux Vulkan/nvmap bridging. A new canonical read-only lease regression proves
color reads succeed while writes reject without altering the padded layer.
All twelve selected SwiftShader Vulkan/Linux tests pass: eleven exercise rendering
or transfers; one checks rejection of unavailable exact arithmetic modes.
Game draw scheduling is still not connected to these helpers, and the last Snipperclips
startup frontier remains ZCullGetCtxSize; no actual game image is claimed.

Linux RGBA8 attachments can now initialize on a fresh Vulkan device directly
from canonical guest pixels without a dummy clear/draw, refresh existing matching
images and write completed pixels back. Native SwiftShader regression loads a
colored guest background, renders without ClearColor, verifies every uncovered
pixel is preserved and covered pixels match reference, then verifies exact guest
writeback. Short uploads reject without changing pixels; strict Vulkan/Linux
Clippy passes. This validates host-test attachment LOAD semantics and transfers,
not native guest draw routing or Snipperclips shaders/frame/menu.

Runtime can now read a selected supported color layer from canonical nvmap into
exact-format tightly packed pixels, with live read leases and a bounded tiled
payload. Regression writes/reads changed/restored pixels to prove no stale host
image is substituted. Typed transfer errors are shared with writeback; strict
runtime/Linux Clippy passes. Uploading these pixels into Vulkan for attachment
load semantics and native guest draw routing remain unfinished; no game frame
or startup advancement is claimed.

Vulkan exposes the actual eligible RGBA8 image extent, shared with readback
validation. The Linux guest writeback bridge compares it to captured target
dimensions before reading pixels. Native regression rejects a 64x16 target for
a 32x32 image despite equal byte counts, verifies guest memory is untouched, then
transfers the correct shape successfully on SwiftShader. Strict Vulkan/Linux
Clippy passes. This prevents shape confusion, not automatic guest draw execution
or a verified Snipperclips frame.

Linux now bridges completed Vulkan RGBA8 readback to canonical guest color-layer
writeback. An explicitly selected SwiftShader regression renders a host-only
triangle, writes its pixels into real nvmap-backed guest memory and deswizzles
that memory to verify exact image equality and nonblack rendered pixels. Strict
Linux-platform Clippy passes. Image identity/submission order remain caller-owned;
this bridge is not attached to native guest draw scheduling and signals no guest
fence. No actual Snipperclips shader/frame/menu is verified.

Runtime color-layer writeback now accepts exact-format completed linear pixels,
checks a 64 MiB software budget and canonical nvmap read/write leases, preserves
existing tiled padding and writes the converted selected layer through live GPU
memory. Regression verifies mapped RGBA8 pixels, untouched padding, wrong format/
length rejection, foreign leases and unmapped targets; strict runtime Clippy
passes. This helper is not yet connected to Vulkan readback/native guest draws
and signals no fence. No Snipperclips game frame or startup progress is claimed.

The 2D block-linear conversion path now supports swizzling tightly packed bytes
into an existing padded layer without overwriting padding. Dimension/footprint
and slice sizes validate before changes. Regression checks round-trip pixels,
padding preservation and short-payload rejection; all five block-layout tests and
strict GPU IR Clippy pass. This is a conversion primitive, not a guest memory
write; canonical ownership checks and rendered target writeback remain unfinished.

RGBA8 color-target format 0xd5 now lowers explicitly. Native encoded fragment
fixtures construct captured color registers, writable backing metadata and a
PreparedColorTarget, then allocate/render its TextureDesc on a fresh Vulkan device
cache. MOV and LDC32/64/128 all match reference pixels and uniform refresh on
SwiftShader. Targeted state tests and strict GPU IR/Vulkan Clippy pass. Backing
metadata in this fixture is synthetic: guest target upload/writeback, canonical
nvmap ownership and automatic guest draw execution remain unfinished.
Format reference: https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/nv50/nv50_formats.c

Single-sample selected color layers now prepare canonical TextureDesc metadata
together with their checked writable backing range and block-linear layout.
Image identity must differ from the backing buffer; unsupported sample counts
reject rather than silently becoming single-sample. Regression checks dimensions,
format, layer selection, footprint agreement and identity/sample failures; strict
GPU IR Clippy passes. This is allocation metadata, not Vulkan allocation, upload,
rendering, writeback or guest completion; full game target integration is unfinished.

Captured block-linear color layers now resolve against live canonical resource
mappings and require write permission. Typed errors distinguish missing/unsupported
descriptors, unmapped memory and read-only targets. Regression covers a nonzero
layer/resource offset and remapping from read-only resource 9 to writable resource
10, proving snapshots do not retain stale backing identities. Targeted tests and
strict GPU IR Clippy pass. This performs no memory write; Vulkan image creation,
guest target synchronization and native game draw execution remain unfinished.

Supported block-linear color layers now resolve their guest address/length with
checked array-layer placement and 40-bit extent. Nouveau programs ARRAY_PITCH as
layer_stride >> 2: decoding uses four-byte units, rejects overlapping multilayer
strides and does not require unused single-layer stride. Regression verifies a
nonzero layer, one-word-short pitch and invalid/overflowing addresses; strict GPU
IR Clippy passes. Live writable mapping and Vulkan target creation remain
unfinished; no guest memory write or actual game image is performed here.
Reference: https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/nvc0/nvc0_state_validate.c

Supported 2D block-linear color targets now produce validated single-layer
footprints: exact dimensions/format, array layer bounds and block-height fields
are checked before the existing normalizing layout constructor. Pitch/depth
layouts and reserved fields reject. Regression verifies GOB padding and negative
descriptors. Large-width ceiling division was fixed to avoid saturation-based
footprint underestimation, with boundary regression. Targeted layout/state tests
and strict GPU IR Clippy pass. Array pitch placement, writable backing and Vulkan
target creation remain unfinished; no actual game target/frame is claimed.

Raw color-target format lowering now recognizes eleven exactly represented
float/scalar formats from NVIDIA clb197.h and rejects disabled, reserved and
unrepresented formats without masking unsupported bits. Regression checks each
canonical format/storage width and high-bit contamination; strict GPU IR Clippy
passes. Packed color channel-order formats and memory footprint/live writable
mapping validation remain unfinished; this does not create a game render target.

Draw snapshots now expose complete raw color-target descriptors using the NVIDIA
clb197.h target method layout. Missing any of the nine descriptor words rejects
instead of acquiring zero defaults. Regression verifies every missing word,
target bounds and capture isolation from subsequent address changes; strict GPU
IR Clippy passes. This is raw state extraction only: layout/format footprint,
live writable backing and native render-target creation remain unfinished.
Reference: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/3d/clb197.h

Vertex input assembly now rejects unknown attribute widths and attribute offset
plus fetch width exceeding the bound resource range. Exact-end and one-byte-overrun
regressions pass. Canonical VertexFormat::byte_width is shared with Vulkan draw
resource validation, avoiding divergent format-size tables. The targeted state
regression, all fourteen ordinary Vulkan tests and strict GPU IR/Vulkan Clippy
pass. This guards prepared descriptors, not native guest rendering or a new
Snipperclips startup/frame milestone.

Checked vertex inputs now assemble deterministic pipeline attributes and actual
BindVertexBuffer commands, deduplicating shared stream bindings only when their
descriptors and resource ranges agree. Duplicate shader locations, conflicting
strides and invalid/unreadable ranges reject. Regression checks shared streams,
nonidentity locations and resource offsets. Native vertex fixtures consume this
assembly result and both rendering fixtures pass; arithmetic remains rejection-
only on SwiftShader. Strict GPU IR/Vulkan Clippy passes. Native guest draw routing
and render-target preparation remain unfinished; no game frame is verified.

Runtime indexed-input preparation now connects canonical live index reads to
shader-aware per-vertex stream bounds and returns checked input descriptors.
Regression verifies nonidentity shader location/resource offsets and CPU changes
from valid indices to a vertex-stream overrun, which returns a typed vertex error.
Targeted runtime tests and strict Clippy pass. Instance/divisor selection remains
separate; native queue drawing and render-target preparation are not implemented
by this helper, and Snipperclips still has no verified frame.

Live index bounds now accept an explicit optional restart value rather than
assuming an all-ones marker. Markers outside the encoded index width reject.
Regression verifies 8/16/32-bit little-endian reads with nonzero first index,
signed base vertex, disabled/default/custom restart values and all-restart draws;
the latter perform no vertex selection even with a negative base. Targeted runtime
tests and strict Clippy pass. Mapping these bounds to stream validation and actual
guest draw submission remains unfinished; no game rendering progress is claimed.

Live indexed vertex-bound preparation now reads canonical guest index bytes and
computes minimum/maximum effective elements using an explicit signed base vertex
and explicit primitive-restart enable. Restart markers are skipped only when
enabled; empty/all-restart ranges have no vertex fetches and negative selections
reject. Regression uses CPU-modified mapped 16-bit indices, enabled/disabled
restart, base offsets and negative results; runtime strict Clippy passes. Other
index widths and integration with stream bounds/native draws remain unverified;
no game draw or Snipperclips startup progress is claimed.

Contiguous vertex-element preparation now validates both endpoint fetches against
captured stream limits and live readable mappings before submission. Checked
first/count arithmetic rejects overflow; zero-count ranges perform no fetch.
Regression covers exact-end ranges, nonzero first, overrun and overflow. Native
vertex fixtures use the complete three-element check. This does not infer indexed
vertex selection or instance divisors; those paths and automatic guest rendering
remain unfinished, with no new Snipperclips startup or image evidence.

Shader-aware vertex input preparation now checks explicit slot/location limits
and matches the shader interface format against the captured hardware attribute
before accepting a live mapped stream. Regression covers nonidentity location,
format mismatch and invalid slot/location; all sixteen state tests pass. Native
vertex fixtures use this check and both rendering fixtures pass on SwiftShader;
the arithmetic fixture verifies unavailable-mode rejection only. Strict GPU
IR/Vulkan Clippy passes. Draw-wide fetch bounds, render targets and native guest
draw dispatch remain unfinished; no new game frame is claimed.

Two-stage discovered graphics shader preparation now has one entry point that
prepares vertex/fragment interfaces, checked live uniform bindings, SPIR-V and
merged pipeline resources while retaining source program identities. Missing,
duplicate or additional stages reject rather than silently discarding programs;
vertex-cull composition remains unsupported. Regression verifies an unordered
valid pair and invalid stage collections; strict GPU IR Clippy passes. Input and
render-target preparation and native queue draw dispatch remain unfinished, with
no new Snipperclips startup/frame evidence.

Integrated revalidation after prepared-stage merge safety: all ordinary workspace
tests and strict workspace all-target Clippy pass. A fresh local Snipperclips
probe completes 199,328,189 instructions at PC 0x7102649744 and stops on GPU-control
IOCTL 2147763969 (0x80044701, ZCullGetCtxSize). This supersedes the prior instruction
count, not the startup frontier. Native queue execution still rejects graphics
draw events; prepared shader/layout/resource code is not automatically dispatched
there. No game frame, menu, audio or gameplay is verified.

Graphics-resource merge now validates each prepared stage independently before
combining bindings: missing descriptors, stale layouts, invalid array elements,
zero/undersized ranges and overflowing offset/range pairs are rejected. One stage
cannot conceal another stage's missing binding by providing the same descriptor
key. Regression and strict GPU IR Clippy pass; all four native fragment fixtures
still render matching pixels and refresh correctly. Live mappings still belong
to preparation/submission ownership; this does not resolve game draw dispatch or
Snipperclips ZCull initialization.

Native fragment fixtures now consume merge_graphics_resources for the actual
pipeline layout and descriptor commands. MOV, LDC32, LDC64 and LDC128 all still
match reference pixels and uniform refresh/restoration on SwiftShader; strict
Vulkan Clippy passes. The paired vertex is explicitly host-only and descriptor-free;
shared bindings across two guest stages remain unit-tested, not device-verified.
Automatic game draw submission and Snipperclips ZCull startup remain unfinished.

Prepared vertex/fragment pair regression now compiles both uniform-reading
SPIR-V stages, verifies merged stage visibility and descriptor deduplication, and
rejects reversed stages or conflicting backing resources. All 125 GPU IR and 14
ordinary Vulkan tests pass, as does strict all-target GPU IR/Vulkan Clippy. All
eleven selected SwiftShader tests pass: ten execute rendering/transfers; the
arithmetic test verifies capability rejection only. The combined-resource merge
is not yet wired into native rendering or automatic guest draws; Snipperclips
still has no verified game frame or progress past ZCull initialization.

Prepared graphics resources can now merge vertex/fragment uniform layouts and
binding commands deterministically. Shared descriptor keys deduplicate only when
resource identity, offset and range all agree; conflicts return a typed error
instead of allowing a later stage to overwrite an earlier binding. A regression
covers identical bindings, each conflicting resource field and non-buffer commands.
This is preparation infrastructure, not automatic guest draw submission or a
Snipperclips frame; device integration of the combined result remains unfinished.

The native vertex-uniform fixture now consumes CompiledShader::prepare_vertex,
not independently assembled uniform commands. It verifies undersized bindings
fail with a typed error before submission, the generated layout matches the
pipeline layout and program identity survives preparation. Actual SwiftShader
pixels and CPU uniform refresh/restoration still pass; strict Vulkan Clippy passes.
This validates the unified preparation path on-device, not automatic guest draw
dispatch or progress past Snipperclips ZCull startup.

Native direct LDC64/LDC128 fragment execution is now verified on SwiftShader.
Encoded two-word/four-word loads populate the same four color registers as the
existing scalar LDC and MOV fixtures; all four fixtures match reference pixels,
observe CPU uniform changes and restore the original image without recompilation.
GPU IR/Vulkan strict all-target Clippy passes. These are synthetic encoded shaders,
not extracted game draws: dynamic LDC modes, automatic game draw integration and
the Snipperclips ZCull initialization blocker remain unfinished.

Direct default-mode LDC now lowers aligned 64/128-bit accesses into two/four
consecutive raw register-word loads. Shared validation checks destination group
alignment, register extent and byte-offset alignment; no numeric double conversion
is performed. Regression verifies individual destination/offset pairs and rejects
misaligned or R255-overlapping groups. Native wide-LDC execution remains unverified;
dynamic addressing/other modes and full game integration remain unfinished.

Fragment preparation regression explicitly checks missing constant mappings,
missing headers, wrong shader stages, absent bindings and unmapped resources before
submission. Restoring complete metadata/mappings produces a prepared module and
binding without mutating the source shader. Typed interface/binding failures remain
distinct. This verifies preparation safety while firmware-key-dependent ZCull
analysis and full game draw integration remain unfinished.

Firmware investigation dependency revalidated: the local 22.1.0 NV executable
cannot be inspected because section zero requires `key_area_key_application_15`;
the supplied master-key revisions end at `master_key_14`. Matching user-dumped
keys are needed to continue that firmware analysis. Do not paste key values into
logs/chat or fabricate a ZCull context size. This blocks this investigation only,
not further in-scope GPU/runtime implementation; Snipperclips still has no verified
menu, frame or gameplay. No user key file was modified.

Compiled fragment shader preparation now combines explicit constant mapping,
header-selected exports, checked snapshot resources, uniform layout and SPIR-V,
sharing binding preparation with vertex shaders. Native encoded MOV/LDC fragment
fixtures consume this provenance-preserving preparation result and still pass
pixel/CPU-refresh checks. This consolidates the integration path; automatic guest
draw submission and Snipperclips boot beyond ZCull remain unfinished.

Compiled vertex shader preparation now combines explicit constant and vertex
interface lowering, checked snapshot uniform bindings, generated descriptor layout
and SPIR-V emission into one provenance-preserving result. Binding failures retain
their typed cause. Regression covers missing bindings, nonidentity descriptor and
resource offsets, source address preservation and a generated shader module. Input
buffer preparation, cross-stage pipeline merging and automatic guest draw submission
remain caller responsibilities; this does not prove a rendered game frame.

Integrated post-LDC regression: all ordinary workspace tests and strict workspace
all-target Clippy pass. All nine explicitly selected SwiftShader native tests also
pass: eight execute rendering/transfers, while the arithmetic test only verifies
rejection of unavailable exact Float32 modes. This checks the combined upload,
uniform layout, vertex/fragment and LDC changes, not Snipperclips boot or gameplay.
The last actual game frontier remains the unsupported ZCullGetCtxSize ioctl.

Native scalar LDC fragment execution is now verified on SwiftShader alongside the
constant-MOV regression. Four encoded LDC instructions read byte offsets 0/4/8/12,
then pass explicit interface/export lowering, uniform binding and SPIR-V rendering.
Reference color pixels match; CPU changes and restoration alter/restore the image
without recompilation. Both instruction fixtures retain four memory loads. Wide,
dynamic and other LDC modes remain unsupported; this is not a game shader/frame.

Dedicated Maxwell LDC now supports direct scalar 32-bit constant reads with default
mode, RZ base and aligned nonnegative offsets. Unlike arithmetic constant operands,
LDC offsets are already bytes; slot/address/size fields follow Mesa GM107 emitLDC.
Shared decode/lowering validation rejects other sizes, dynamic addressing, unknown
controls, R255 destination and unsupported offsets rather than inventing semantics.
Regression covers exact operands and negative controls. Wide/dynamic LDC and native
LDC device execution remain unfinished; no game boot progress is claimed.

Native fragment uniform execution is verified on SwiftShader from encoded Maxwell
constant-buffer MOV instructions. The test uses header-selected color registers,
explicit slot mapping, group-four snapshot binding, generated pipeline layout and
runtime SPIR-V loads. Reference pixels match; changing uniform color to black
changes the image, and restoring CPU bytes restores reference pixels without
recompiling the shader. This proves synthetic fragment uniform execution, not a
Snipperclips shader capture, game frame or initialization beyond ZCull.

Fragment shaders can now emit canonical runtime uniform-word reads through the
shared straight-line SPIR-V path with explicit color location zero and Vulkan
OriginUpperLeft mode. Position exports and vertex inputs are rejected in this
fragment path; complete four-component output remains mandatory. The
`vertex_uniform_spirv --fragment` diagnostic passes Vulkan 1.3 spirv-val, and
regression checks stage separation/incomplete export rejection. Actual fragment
uniform device execution and encoded guest fragment integration remain unverified.

Uniform pipeline layout construction now merges shader-stage visibility for shared
bindings and rejects incompatible block sizes across stages. Tests verify order
independence and vertex/fragment visibility merging. The native constant-MOV
fixture derives its layout from both actual shader modules; native pixel execution
remains valid. This prepares multi-stage integration without claiming fragment
uniform execution or game draw submission, both of which remain unfinished.

Shader-aware uniform binding preparation now joins canonical shader declarations,
explicit hardware slot mapping, captured draw buffers and live address resolution.
It rejects unresolved constant operands, missing/duplicate mappings, mismatched
declared sizes and buffers smaller than the shader's uniform block. The native
constant-MOV fixture rejects an eight-byte binding for a sixteen-byte shader block,
then restores the valid buffer and renders through the new binding helper. This
closes a shader/descriptor size gap; guest draw integration remains unfinished.

Uniform pipeline declarations can now be derived from the shader's explicit
canonical uniform-word accesses, with deterministic deduplication, stage visibility
and conflicting-size rejection. The native encoded constant-MOV fixture consumes
this generated layout rather than separately hardcoding its descriptor declaration.
The hardware slot mapping remains explicit and live memory binding remains checked.
Unit and native pixel regressions pass; this is not automatic guest draw submission.

Native driver constant uploads are now verified through HIPC/CMIF kickoff,
authenticated channel ownership, GPFIFO admission and process GPU polling. The
nvmap-backed destination stays unchanged at admission and receives exact uploaded
words only after polling execution. This extends the native mapping/channel
regression beyond standalone FIFO traces; it remains a synthetic workload rather
than a booted Snipperclips frame.

Read-only nvmap backing is now covered by an encoded FIFO constant-upload
regression. The selector and upload packet decode normally, but ordered execution
fails on the first write with zero completed transfers and unchanged backing bytes.
This proves the new upload path does not bypass canonical device-lease write
permissions. All 132 runtime tests pass; Snipperclips initialization remains blocked
at the previously verified ZCull query.

Encoded constant-upload regression now covers split pushbuffer packets and FIFO
semaphore pause/resume. One word commits before an unsatisfied equality acquire;
resuming from the retained event cursor uploads only the remaining word. A CPU
marker replacing the already uploaded word survives the resume, proving no prefix
replay. A second packet without a new upload offset retains the next destination.
These checks pass with canonical nvmap memory; they do not prove game rendering.

Encoded PFIFO constant uploads now pass through engine binding, selector writes,
one-increment offset/data packets and ordered runtime writes into live nvmap
backing. Selector/binding changes emit explicit state-only events rather than
opaque events that stopped execution before the upload. Regression verifies a
third out-of-bounds word stops execution after two committed words and prevents
a later overwrite. This supersedes manually constructed upload events in the
runtime regression; native game ZCull initialization is still unresolved.

The base Maxwell constant upload method now emits an ordered trace event with a
captured destination and raw word, advancing the decode cursor for the following
word. Runtime trace execution writes the captured bytes through canonical nvmap
memory; invalid upload addresses stop the trace rather than disappearing.
Offline translation stops at these uploads until it has an equivalent backend
command. Regression checks captured address/cursor and actual ordered trace writes.
Only the base repeated upload method is handled; other upload aliases and full
guest rendering remain unfinished. Decode cursor advancement is distinct from
successful memory execution; channel execution faults must not replay the prefix.

Runtime constant-word upload execution now writes exact little-endian register
bits through canonical live GPU memory and advances the byte cursor by four only
after success. Nvmap-backed regression verifies two consecutive words, preserved
negative-zero/NaN payload bits, end-of-buffer rejection and unchanged cursor on
unmapped memory failure. Mesa's nvc0_cb_bo_push corroborates sequential word uploads.
This helper is not yet wired into FIFO method execution; no upload command stream
or Snipperclips rendering completion is claimed.

Constant-upload address preparation now validates the current complete selector
and 16-bit upload offset against the buffer size/40-bit address range. Tests prove
selector changes affect uploads without altering prior shader bindings, reject
missing words and overrun offsets, and preserve invalid high address bits. This
accessor neither writes memory nor advances the upload cursor: ordered upload
execution and its exact cursor semantics remain unfinished.

Native uniform regression now exercises malformed descriptors (empty remainder,
oversized range, address overflow, and misalignment when the device requires it).
Each rejected plan/submission leaves the previous reference image intact, and a
subsequent valid draw succeeds on the same Vulkan device. This verifies error
recovery alongside the existing encoded constant-MOV and CPU-refresh pixel checks;
the synthetic test passes on SwiftShader. Game initialization remains at ZCull.

Post-uniform-path full regression: all ordinary workspace tests, workspace
all-target strict Clippy and rustfmt checks pass. The actual local Snipperclips
NSP was reloaded with the user key provider and a one-billion-instruction probe:
it stopped after 199,328,011 instructions at PC 0x7102649744 on unsupported
GPU-control ioctl 2147763969 (0x80044701, ZCullGetCtxSize). The CLI exit code is
inspection completion, not successful game boot. This confirms the GPU/shader
changes did not bypass or resolve the initialization frontier; no menu, game
frame, gameplay or audio has been verified. Odyssey remains behind Snipperclips
stabilization. Native uniform rendering evidence remains synthetic fixture-only.

Vulkan buffer descriptors are now validated before driver updates against actual
device alignment/range limits, allocation bounds and Uniform/Storage usage.
Canonical zero range resolves to the remaining allocation but cannot produce an
empty or oversized binding. Unit tests cover overflow, mismatched usage and
distinct uniform/storage limits. All 14 ordinary Vulkan tests and seven native
tests pass on SwiftShader (six execution tests, one arithmetic capability rejection).
This closes descriptor validation gaps, not the Snipperclips boot blocker.

Full-lane Maxwell constant-buffer MOV is now decoded/lowered losslessly with
shared strict control validation. Partial masks, reserved fields and R255 stay
unsupported. The native uniform rendering fixture now starts from encoded
`MOV R9,c[3][0]` / `MOV R10,c[3][4]`, then semantic and explicit interface lowering,
SPIR-V, snapshot descriptor preparation and Vulkan rendering. CPU uniform changes
clip/restore reference pixels on SwiftShader. This supersedes the fixture's earlier
canonical-operation substitution, but does not claim LDC or actual game execution.

Corrected Maxwell arithmetic constant-buffer offset decoding: the encoded 14-bit
field is an unsigned word offset, restored to bytes by shifting left two, not a
signed byte displacement. Mesa GM107 emitCBUF uses the inverse shift. Exhaustive
regression covers all 32 slots and 16,384 word offsets; FADD lifting also checks
4/28/0x8000/0xfffc-byte accesses. All 117 GPU-IR tests pass. The encoded constant
SPIR-V diagnostic now reads offset 28 rather than zero so unit mistakes are exposed.
This correctness fix does not resolve the game's ZCull initialization blocker.

The encoded `vertex_float_spirv --constant` diagnostic now verifies an actual
Maxwell FADD constant-buffer operand survives semantic lifting, explicit constant
interface mapping and vertex-interface lowering, and produces Vulkan 1.3-valid
SPIR-V. `CompiledShader::lower_constant_interface` retains program provenance and
source lift statistics; regression checks identity and typed missing-binding
errors. This is compile-path verification only: exact arithmetic device execution
is still capability-gated, and no new Snipperclips frame or boot progress is claimed.

Native Vulkan uniform-word execution is now verified on SwiftShader. The new
ignored test replaces decoded vertex Z/W constants with canonical uniform loads,
uses a Maxwell snapshot to prepare a nonidentity slot-to-binding descriptor,
compares pixels with reference geometry, changes W to clip all vertices, restores
the buffer and repeats existing vertex/index refresh checks. It exposed stale
uniform uploads: Uniform buffers now use the checked CPU-input refresh path,
including full-provider size validation and pending-writeback exclusion. This is
synthetic canonical uniform execution, not Maxwell LDC decoding or game rendering.

Vertex SPIR-V now emits runtime uniform-word loads using explicitly mapped
descriptor sets/bindings. std140 uvec4 arrays preserve packed 32-bit register words
with 16-byte strides, followed by bitcasts when consumed as floats. Conflicting
sizes, unaligned/out-of-bounds loads and non-16-byte buffer sizes are rejected
rather than padded beyond a descriptor. The synthetic `vertex_uniform_spirv`
example passes installed spirv-val for Vulkan 1.3. This proves module validity,
not device execution or guest draw integration; game rendering is still unverified.

Explicit shader constant interfaces now lower statically addressed Maxwell
constant-buffer operands to canonical raw uniform-word loads with caller-supplied
descriptor set/binding and declared size. Payload values are not compiled into the
shader. Missing/duplicate mappings, negative or unaligned offsets, out-of-range
words and register-relative addressing are rejected. The vertex interface pass
preserves these canonical operations. SPIR-V uniform emission remains unfinished
and still rejects them rather than fabricating shader values; no game frame is
claimed by this interface regression.

Constant-buffer descriptor preparation now emits the existing canonical
`BindBufferDescriptor` command from a captured Maxwell buffer and a caller-supplied
uniform descriptor mapping. It validates descriptor type/array bounds and resolves
the full bound range against live readable mappings. Tests verify nonidentity
set/binding mapping, resource offsets and backing changes. No automatic shader
descriptor assignment or draw integration is claimed; shader constant loads still
need SPIR-V lowering and the resulting descriptors must be wired into submission.

Runtime constant-buffer payload reads now use the live mapped resource provider
after descriptor/range validation. Raw bytes are preserved without guessed scalar
conversion; allocation is bounded by the selector size. The canonical nvmap-backed
regression confirms CPU writes are visible, unmaps fail and backing remaps select
new bytes, plus missing-slot/out-of-bounds rejection. This helper is not yet wired
to shader execution or Vulkan uniform bindings and is not evidence of game rendering.

Constant-buffer read-range resolution now checks the captured binding, nonzero
read length, offset overflow and descriptor size before resolving the requested
subrange through current GPU mappings. Unreadable mappings are rejected; remapped
resources are selected live, not cached in the draw snapshot. The new regression
covers final-byte reads, missing slots/groups, overflow, unmapped memory and changed
resource identity. All 113 GPU-IR tests pass. This validates addresses only, not
payload reads, uploads or Vulkan uniform binding; Snipperclips remains unverified.

Constant-buffer binding capture: Maxwell draw snapshots now retain each of the five
graphics bind groups and its 32 selector slots independently. Size/address words are
captured at bind time rather than reconstructed from the last selector; disabled or
malformed binds replace stale slots and remain unavailable to range validation.
Tests cover selector changes, snapshot immutability, independent groups, disabled and
reserved-bit binds, invalid sizes and 40-bit address overflow. This is state capture
only: constant-buffer uploads, live memory reads and shader/backend binding remain
unfinished. It does not advance the verified Snipperclips boot frontier or prove a
game frame. Method fields follow NVIDIA's public `clb197.h`; selector/bind sequencing
is corroborated by Mesa's `nvc0_state_validate.c`.

NCA header keys can now be derived from the exact revision-00 master and supplied
header/KEK/key-generation sources, decrypting the two distinct XTS key halves.
Explicit header keys are checked against derived values when all prerequisites
exist; conflicts and duplicate source entries reject without exposing material.
Tests cover both halves, every missing prerequisite, refusal to substitute another
master revision, explicit-only fallback and conflicts. All 24 ingest tests, strict
ingest Clippy, workspace all-target checking and formatting pass. Actual firmware
inspection confirms local derived/header agreement and still stops on missing
key_area_key_application_15. A one-instruction Snipperclips probe verifies current
content loading and initial execution (budget stop at PC 0x7100000008), not a new
startup frontier: the last full probe remains at ZCull. No keys were changed/logged.
Reference:
https://github.com/SciresM/hactool/blob/master/pki.c

User key loading now derives NCA key-area keys and title KEKs only from the exact
supplied master revision and required user-supplied sources. The verified hactool
three-step AES-ECB derivation is used for application/ocean/system keys; derived
and explicitly supplied values must agree, otherwise loading rejects without
printing any material. Missing prerequisites stay absent and older masters never
manufacture later revisions. Synthetic inverse-wrapping tests cover all key-area
kinds, title KEKs, missing prerequisites and conflicts; all 23 ingest tests and
strict ingest Clippy pass. Whole-workspace tests, strict all-target Clippy and
formatting also pass. Actual local loading validates the available dump's derived
keys, but firmware NV executable inspection still requires missing
key_area_key_application_15; the file has no master_key_15 to derive it from.
No local keys were changed or printed and the ZCull investigation remains blocked
on this firmware/key compatibility requirement.
Fresh Snipperclips revalidation with the updated provider completes 199,328,010
instructions at PC 0x7102649744 and stops at the same GPU-control IOCTL
0x80044701 (ZCullGetCtxSize). This verifies existing game decryption/startup was
not regressed; no new menu, frame or gameplay is claimed.
Reference:
https://github.com/SciresM/hactool/blob/master/pki.c

Vertex arithmetic emission now supports source ABS/NEG using unsigned sign-mask
operations followed by bitcasts, applying ABS before NEG without host evaluation
or numeric conversion. This preserves NaN payload/zero bits before the arithmetic;
invert modifiers and saturation still reject. Unit tests cover both sign modifiers
on zero, negative zero and signed NaN patterns with exact opcode/mask ordering.
All 111 GPU IR tests and strict GPU IR/Vulkan Clippy pass. The encoded arithmetic
diagnostic's new --modifiers variant validates with spirv-val for Vulkan 1.3.
Native arithmetic remains unverified on the available devices due to the documented
float-control/driver limitations; no actual game shader or ZCull progress is claimed.
Reference:
https://github.com/KhronosGroup/SPIRV-Headers/blob/main/include/spirv/1.0/spirv.hpp11

Vertex SPIR-V emission now supports separate scalar FADD/FMUL with explicit
Float32 RTE, signed-zero/Inf/NaN and uniform denorm preserve/FTZ modes, plus
NoContraction decorations. Raw register sources bitcast to float; undefined/vector
sources, mixed per-operation denorm modes, saturation and source modifiers remain
unsupported. Unit regression checks arithmetic opcodes and required modes; all
110 GPU IR tests and strict GPU IR/Vulkan Clippy pass. The new
`vertex_float_spirv` diagnostic reads encoded ALD/FADD32I/FMUL32I/AST/EXIT, resolves
the interface and emits a dynamic X+1/Y*0.5 shader that passes installed spirv-val
with target Vulkan 1.3.
Native arithmetic fixture has an independent, nonidentity input/reference test,
but SwiftShader reports Float32 RTE unavailable: module creation is correctly
rejected and arithmetic pixel execution is NOT verified. Its negative gate test
passes separately from the five existing pixel/execution regressions, which still
pass. The installed NVIDIA ICD also cannot create an instance in this environment
(ERROR_INCOMPATIBLE_DRIVER). Neither unsupported device is treated as successful
arithmetic rendering. Snipperclips remains at the verified ZCull startup frontier.

Native Vulkan shader creation now queries actual physical-device float-control
properties and gates explicit SPIR-V DenormPreserve/FlushToZero, signed-zero/Inf/NaN
preservation and RTE/RTZ execution modes by their requested width. Unsupported
widths/modes, malformed instruction lengths and conflicting execution modes reject
before module creation/cache reuse; differing-width modes respect device independence
properties. Focused positive/negative property tests and strict Vulkan Clippy pass;
all five native SwiftShader pixel/execution regressions pass. This gate is a
prerequisite for faithful arithmetic emission, not yet executable game arithmetic
or progress past Snipperclips ZCull startup.
References:
https://registry.khronos.org/vulkan/specs/latest/man/html/VkPhysicalDeviceFloatControlsPropertiesKHR.html
https://raw.githubusercontent.com/KhronosGroup/SPIRV-Headers/main/include/spirv/unified1/spirv.hpp11

Ordinary FADD/FMUL register, constant-buffer and short-immediate forms now share
exact control validation in decoder and lowerer. FADD preserves independent source
negate/absolute, saturation and FTZ; FMUL preserves its combined negation correctly
without treating DNZ as a right-source negation. Nondefault rounding, scaling,
DNZ, condition-code output and unknown controls remain unsupported rather than
being discarded. Tests exhaust all 64 FADD modifier combinations and eight FMUL
flag combinations across all three source forms, plus unsupported controls.
All 109 GPU IR tests and strict GPU IR/Vulkan Clippy pass. Executable arithmetic
SPIR-V and actual game shaders remain unfinished; no new ZCull progress is claimed.
Reference:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Primary GM107 emitter validation corrects full-immediate FADD/FMUL control positions:
FADD32I preserves left/right negate/absolute and FTZ above its 32-bit operand;
FMUL32I preserves saturation/FTZ and consumes the negation already encoded in the
immediate sign. Exact layouts reject condition-code output, DNZ, unknown fields
and R255 source/destination cases instead of claiming lossless lowering. Canonical
readiness is promoted only after successful lowering. Tests cover all 32 FADD
modifier combinations, all four FMUL flag combinations and unsupported controls.
All 108 GPU IR tests and strict GPU IR/Vulkan Clippy pass. This fixes canonical
representation, not executable floating-point arithmetic or all ordinary/short
instruction controls. No real-game/ZCull progress is claimed.
Reference:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Arithmetic short immediates now decode their separate sign bit at 56 rather than
including adjacent control bit 39 in the value. Integer source tests verify signed
20-bit extremes independently of that adjacent bit. FADD/FMUL short immediates
reconstruct IEEE-754 bits by restoring the omitted low 12 bits, while their 32-bit
forms retain all bits. Dedicated float immediate operands lower to ConstantF32,
not integer constants/numeric conversions; tests preserve negative zero, NaN
payloads and infinities. All 107 GPU IR tests, strict GPU IR/Vulkan Clippy and
whole-workspace all-target checking pass. This fixes operand representation only:
rounding/modifier/control validation and executable arithmetic shader coverage
remain incomplete. No new game execution or ZCull progress is claimed.
Reference:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Verified short-immediate MOV now reconstructs its signed 20-bit integer from low
19 bits at 20..38 and the separate sign bit at 56, then preserves the resulting
32-bit register bits. Shared exact validation requires full lanes and rejects
unknown controls/R255 destinations. Edge tests cover zero, one, largest positive,
smallest negative and minus one, plus partial masks and extra fields. The encoded
fragment fixture adds an unrelated short MOV of -1 into R7 after the actual color
register definitions; native SwiftShader pixels remain equal to the reference,
showing the last constant is not mistaken for the output. All 106 GPU IR tests,
strict GPU IR/Vulkan Clippy, formatting and the native fixture pass. No new game
probe or progress beyond ZCull is claimed.
Reference:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Register MOV now shares exact decoder/lowerer validation: its lane mask resides
at bits 39..42 and must be complete for a canonical register copy. Unknown fields,
partial masks and R255 zero/discard forms stay unsupported instead of losing their
semantics. Constant-buffer/short-immediate MOV controls are not yet fully verified
and no longer report Ready or silently lower through generic operands. Positive
and negative regressions cover these cases. The encoded fragment fixture now copies
R2 into its physical alpha register R3 with a verified full-mask MOV and still
matches native SwiftShader pixels. All 105 GPU IR tests, strict GPU IR/Vulkan Clippy
and the native fixture pass. This does not advance the real game's ZCull frontier.
Reference:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Fresh real Snipperclips probe after the recent shader changes completes 199,328,021
instructions at PC 0x7102649744 and stops on GPU-control IOCTL 0x80044701,
ZCullGetCtxSize. No menu/frame/gameplay is verified and the startup frontier is
unchanged. Primary GM107 inspection also reveals EXIT's condition field must be
0xf for unconditional condition-code execution. The semantic decoder/lowerer now
share strict validation, retaining predicate guards but rejecting other conditions
or unknown modifiers. RET is no longer silently treated as shader EXIT: subroutine
return semantics remain unsupported. Encoded runtime/vertex/fragment/Vulkan fixtures
now use the verified EXIT condition. All 104 GPU IR tests, the live shader runtime
regression, strict GPU IR/runtime/Vulkan Clippy and native Vulkan pixel fixture pass.
Reference:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Implicit SPIR-V smoke fallback is removed: empty/Return-only vertex programs no
longer synthesize a triangle, and fragment definitions without explicit exports no
longer synthesize a color or default white. Diagnostic geometry requires the
explicit host-only SmokeVertexTriangle operation, which no guest frontend emits.
Smoke module fragment fixtures now write complete explicit outputs. Regressions
reject incomplete shaders even with smoke-looking variant keys. Whole-workspace
tests, all 103 GPU IR tests, strict whole-workspace Clippy and all five selected
SwiftShader Vulkan execution/readback regressions pass. Real shaders with missing
translation must fail, not produce substitute frames; Snipperclips's ZCull startup
frontier remains unchanged and was not reprobed.

Live nvmap regression now compiles an encoded fragment program across mappings,
discovers its captured draw slot 5/binding 3, resolves its header-selected color
exports and emits SPIR-V while retaining original source address/lift statistics.
Changing the CPU header to an incomplete mask fails the next interface resolution;
restoring it and changing a raw immediate changes the next SPIR-V without mutating
the original compiled snapshot. Focused runtime regression, whole-workspace tests
and strict runtime Clippy pass. The fixture is synthetic; production draw dispatch,
actual game shaders and the Snipperclips ZCull startup query remain unfinished.

Mesa's fragment lowering converts exports into final MOVs to physical GPRs;
combined with its output-slot assignment this establishes R0..R3 for a single
complete target-0 color. Restricted fragment export resolution now inserts four
explicit outputs before termination for exactly that header layout, preserving
final register values. Sparse/multiple targets, depth/sample-mask/unknown controls,
unsupported operations, missing termination and wrong stages reject. Compiled draw
shaders expose this resolution without losing source/header/lift provenance.
The native Vulkan fixture now starts its fragment stage from encoded MOV32I/EXIT
bytes plus the color header mask, parses/lifts/resolves it and matches reference,
vertex-refresh and indexed pixels on SwiftShader. All 102 GPU IR tests, strict
GPU IR/Vulkan Clippy and the native fixture pass. This is a synthetic encoded
program, not an extracted game shader; Snipperclips remains blocked at ZCull.
References:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_lowering_nvc0.cpp
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/nvc0/nvc0_program.c

Primary Mesa inspection establishes fragment header word 18 as the color-component
mask and word 19 bits 0/1 as sample-mask/depth export enables. Stage-checked header
access now exposes eight four-component target masks and retains the complete
control word. Tests cover all targets, zero masks, unknown control bits and invalid
target/stage requests; all 101 GPU IR tests and strict Clippy pass. This metadata
does not assign physical color registers: source register packing/export lowering
still requires verification before connecting actual Maxwell fragment programs.
No game execution progress is claimed.
Reference:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/nvc0/nvc0_program.c

The native vertex-input Vulkan regression now also executes the explicit constant
fragment-output compiler: raw scalar constants, moves and four ordered component
exports reproduce the independent smoke reference pixels on SwiftShader. CPU vertex
refresh, degenerate geometry and indexed negative-base-vertex checks still pass.
Removing the alpha export causes Vulkan planning to reject the incomplete shader
before execution. Strict Vulkan all-target Clippy passes. This verifies canonical
fragment output execution, not Maxwell fragment export decoding or game rendering;
the actual Snipperclips ZCull startup frontier remains unchanged/unreprobed.

Fragment compilation now accepts an explicit constant scalar output at location 0:
ConstantF32/raw ConstantBits32, register moves and ordered component StoreOutput
writes compile only when all four channels and a Return are present. Forward or
undefined sources, integer-typed constants, unsupported outputs and instructions
after Return reject. Constant folding preserves negative-zero/NaN bits and the
value at each store even after register reassignment. All 100 GPU IR tests and
strict GPU IR/Vulkan Clippy pass. Legacy smoke fixtures still use the separate
constant-vector convention; this new explicit-output path is not Maxwell fragment
export lowering or evidence of an actual game frame. ZCull remains unresolved.

The live nvmap shader compilation regression now includes encoded MOV32I and split
AST position stores across scheduler bundles. A CPU write changes the immediate
from positive to negative zero; recompilation observes the exact new register bits,
while the earlier compiled snapshot retains its original value. Both snapshots
compile to SPIR-V and retain draw source metadata. The focused runtime regression
and strict runtime all-target Clippy pass. This tests the real memory/compiler
plumbing using a synthetic program, not production game draw dispatch; Snipperclips
has not been reprobed and its ZCull startup frontier remains unresolved.

MOV32I constants now retain untyped 32-bit register contents through vertex
interface lowering. Float consumers use SPIR-V OpBitcast, not numeric conversion;
regressions preserve negative zero, NaN payloads and infinity bit patterns. The
decoder and lowerer share exact validation: partial lane masks and unknown modifier
bits cannot be reported Ready or silently replace an entire register. The native
Vulkan fixture now executes encoded full-mask MOV32I instructions for Position Z/W
alongside ALD and split AST writes, matching reference/refresh/indexed pixels on
SwiftShader. All 99 GPU IR tests and strict GPU IR/Vulkan all-target Clippy pass.
This remains a synthetic encoded shader; no new Snipperclips probe, game frame or
progress past ZCull initialization is claimed.
Encoding reference:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp

Compiled draw shaders now expose explicit vertex-interface resolution while
retaining source address, pipeline slot/binding, graphics header and source lift
statistics. Interface failures have typed error provenance. The runtime regression
reads encoded ALD/AST/EXIT through live nvmap memory across two mappings, discovers
the captured draw's vertex slot, resolves its explicit nonidentity location and
compiles SPIR-V. Changing the encoded ALD input address in actual CPU memory changes
the next compilation and rejects its missing interface slot, without changing the
previous compiled module. Focused runtime regression and strict GPU IR/runtime
Clippy pass. Automatic draw dispatch/presentation and the real game's ZCull startup
remain unfinished; the regression still uses a synthetic encoded program.

Direct AST Position writes now support one to four components at aligned offsets
0x70..0x7c. Canonical scalar builtin writes compile to SPIR-V output access chains,
preserving store order without composition temporaries. Compilation requires all
four Position components initialized; missing components and crossing the Position
boundary reject. Unit regression covers split XY/ZW stores and missing Z; the native
Vulkan fixture now encodes separate XY/ZW AST instructions across scheduler bundles
and still verifies reference/refresh/indexed pixels on SwiftShader. Focused tests
and strict GPU IR/Vulkan Clippy pass. Real game shader extraction/dispatch and ZCull
startup are still incomplete; these tests are encoded synthetic programs.

Direct Maxwell vertex interface lowering now translates rich ALD accesses through
an explicit compact-slot/location/format mapping into scalar host inputs, and full
four-component AST writes at byte 0x70 into BuiltIn Position. Unsupported integer
interfaces, indirection, patch/output reads, duplicate mappings, unknown operations
and partial position stores reject. Composition temporaries avoid user registers.
The native Vulkan vertex fixture now starts from encoded ALD/AST/EXIT instruction
bytes, parses/lifts them, resolves Maxwell descriptors and compiles the resulting
interface to SPIR-V; SwiftShader readback still matches reference/refresh/indexed
pixels. Focused interface tests and strict GPU IR/Vulkan Clippy pass. This is an
encoded synthetic shader, not an extracted Snipperclips shader or production draw
dispatch; ZCull startup and the remaining game shader coverage are unresolved.

GM107 ALD/AST now lower through the single semantic decoder into rich shader
attribute accesses, retaining byte offset, address/vertex indirection, component
count, patch mode, output reads and predicate guards. Missing indirect registers
remain absent rather than becoming guessed values. Unknown modifier bits,
AST output-read bits and register spans crossing R255 remain opaque; compute
program lifting rejects graphics attributes even when their encodings are known.
Regression coverage checks widths/modes, guards, unknown fields and stage rejection;
all 96 GPU IR tests, strict GPU IR/Vulkan Clippy and workspace all-target checking
pass. Rich IR representation is not executable interface lowering: Vulkan still
needs stage/type/location resolution and rejects these unsupported operations.
No real game frame or progress past ZCull is claimed.

Direct inspection of Mesa's GM107 emitter now establishes the attribute operand
roles: emitADDR uses indirect operand 0 for the address register and a byte offset;
ALD/AST use indirect operand 1 for the vertex register. Missing indirect operands
encode register 255. ALD's output flag denotes FILE_SHADER_OUTPUT, and patch flags
denote perPatch. Thus a static ordinary vertex input requires both indirect operands
absent, no patch flag and no output-read flag; other modes cannot be flattened into
ordinary input loads. Mesa's vertex input assignment compacts generic slots starting
at byte 0x80 while position output begins at 0x70; the compact hardware slot must
not be presumed to equal the application's original location. These primary source
checks remove the previously unresolved operand-role ambiguity and establish the
next restricted interface-lowering path. No new shader semantics or game progress
are claimed solely from this source inspection.
References:
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/codegen/nv50_ir_emit_gm107.cpp
https://raw.githubusercontent.com/notaz/mesa/master/src/gallium/drivers/nouveau/nvc0/nvc0_program.c

Queued cross-channel semaphore coverage now verifies ADD, unsigned MIN/MAX,
signed MAX, unsigned bounded INC/DEC and XOR can unblock another channel's real
equality acquire. Three failed polls preserve a non-idempotent ADD prefix exactly
once and retain the consumer descriptor without retiring it; producer execution
changes actual shared memory, then the consumer executes its suffix and retires.
An invalid signed INC faults its producer without changing memory or releasing
the consumer. This verifies queued memory synchronization, not native guest fence
threshold IOCTLs, concurrent Vulkan operations or the unresolved game ZCull query.

Synchronous FIFO semaphore execution now also supports four-byte WFI-enabled
signed/unsigned MIN/MAX, wrapping ADD and unsigned bounded INC/DEC reductions.
INC resets at/above payload; DEC resets when zero/above payload, rather than
subtracting the payload. Exact Maxwell control decoding rejects signed INC/DEC,
extra bits, disabled WFI and timestamp-sized structures. Independent arithmetic
vectors cover signed extremes, wraparound and bounded-counter edges; live-memory
runtime tests verify results, adjacent-byte preservation and read-only lease
rejection. Focused tests and strict GPU IR/runtime Clippy pass. This remains the
serialized memory executor, not concurrent Vulkan atomics or guest fence waits;
no game probe was run after this change and the ZCull frontier remains unresolved.
References:
https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/host/clb06f.h
https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/manuals/turing/tu104/dev_pbdma.ref.txt

Fresh whole-workspace validation after vertex descriptor/runtime/Vulkan integration:
workspace tests, all-target checking, formatting, strict all-target Clippy and
warning-free documentation pass. Clippy found an AllocObjCtx test-module ordering
issue; moving the existing implementation before tests fixes it without changing
transport semantics, and its focused regression passes. All five explicitly
selected SwiftShader Vulkan execution/readback regressions pass as well.
A fresh real Snipperclips probe completes 199,328,014 instructions at PC
0x7102649744 and stops on GPU control IOCTL 0x80044701 (ZCullGetCtxSize).
The small instruction-count difference is not boot progress: the exact unsupported
operation is unchanged, and no game frame/menu/audio/gameplay is verified.

A fresh local firmware NV inspection still cannot decrypt executable section 0.
The inspector now identifies the missing requirement safely and precisely:
`key_area_key_application_15` for the generation-22 NV program in this dump.
Only key names/revisions are reported, never values, rights IDs or decrypted bytes.
The actual run confirms this missing-key error and example-target strict Clippy
passes. This blocks investigating NV executable contents via the current key file,
not all GPU implementation work; no guessed ZCull size is returned to the game.

Canonical vertex-input preparation is now a reusable draw-state operation used by
the native Vulkan regression, rather than test-local descriptor assembly. It returns
the exact supported attribute format, stream binding/stride and readable resolved
resource range; shader location and input rate must be supplied explicitly by a
verified interface/instancing policy. Unit coverage verifies nonidentity locations
and explicit input rates; native SwiftShader pixel/refresh regression and strict
GPU IR/Vulkan Clippy pass. ALD/AST shader modes remain unpromoted where their exact
semantics have not been established; production game graphics remain incomplete.

The native Vulkan vertex regression now builds its input format, stream stride,
attribute offset and resource binding from synthetic Maxwell draw descriptors and
resolved GPU mappings instead of hardcoded canonical input descriptors. SwiftShader
execution/readback verifies reference pixels, degenerate-buffer refresh and restored
pixels, including the fixture's subsequent indexed path. Strict GPU Vulkan Clippy
passes. Shader operations and their location pairing remain explicitly synthetic;
this proves descriptor-to-Vulkan integration in a fixture, not actual game shader
lifting, production draw dispatch or progress past Snipperclips ZCull startup.

Runtime vertex fetch reads now combine validated draw descriptors with the live
mapped-resource provider. The nvmap-backed regression observes actual CPU writes,
rejects reads after unmap, then observes different backing after remapping while
the draw snapshot remains unchanged. Typed errors distinguish descriptor failures
from payload/lease failures. This returns raw bytes only: shader conversion,
vertex/instance selection and invoking this path from executable game draws remain
unfinished. No new Snipperclips probe, frame or gameplay was verified.

Supported vertex attributes now resolve individual element fetches through the
draw's descriptor/stream, checked stride arithmetic and current readable mapping.
Fetch widths come only from exact supported formats; offsets and final bytes must
fit the programmed stream limit. Zero stride repeats one element, without wrongly
requiring attribute offset to fit stride. Tests cover padded RG32 float elements,
the exact last element, overflow/out-of-bounds, zero stride with maximum index and
unsupported channel swaps. The caller must supply the stream element index;
vertex/instance selection, shader location mapping and executable draw integration
are still unfinished. No real game probe or image was produced by these tests.

Maxwell vertex attribute formats now promote 33 exact unpacked canonical formats:
supported 8-bit normalized/integer and 16/32-bit integer/float layouts. Exhaustive
tests cover all 512 width/type encoding pairs and reject inactive sources,
reserved bits and R/B swaps. Packed/scaled formats and unsupported component
layouts remain unpromoted, rather than acquiring a close but incorrect format.
This is descriptor translation, not a completed Maxwell shader/input interface
or real game draw. Snipperclips remains at its documented ZCull startup frontier.
Reference: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/3d/clb197.h

Draw-state vertex stream resolution now translates validated bounded descriptors
through the current GPU address space, returning stable resource IDs and resource
offsets only for readable mappings. Typed errors retain missing/invalid descriptors,
mapping failures and read denial. Regression coverage checks nonzero resource
offsets, remapping the same snapshot to a new resource, write-only rejection and
a mapping one byte shorter than the declared stream. This helper is not yet wired
into executable Maxwell draw lowering; no game image or ZCull progress is claimed.

Vertex stream limit banks are now decoded independently from immutable draw
registers. A bounded range helper uses the inclusive final byte address, rejects
missing words, disabled streams, reserved format/address bits and reversed ranges,
and retains raw invalid limits for diagnosis. Tests cover both bank boundaries,
the highest 40-bit address, one-byte ranges and snapshots surviving later writes.
This validates descriptor ranges only: live mapping/permissions, vertex formats,
instancing and executable game draws still require integration. The real Snipperclips
ZCull frontier is unchanged and was not reprobed.
References:
https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/3d/clb197.h
https://chromium.googlesource.com/external/gitlab.freedesktop.org/mesa/mesa/+/6f19fc972f4d3004a6719ccbcca221e33098a481/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c

Draw snapshots also expose both Maxwell-B vertex stream banks from their original
four register words: stride, enable, 40-bit address and raw frequency. All four
words must have been written; incomplete state does not acquire invented defaults.
Reserved format bits remain visible, and reserved upper address bits reject address
resolution rather than silently aliasing another buffer. Indexed-draw regression
coverage verifies both bank boundaries, immutable snapshots, incomplete state,
zero stride/disabled streams and out-of-range indices. Frequency is not yet mapped
to an instance divisor; stream limits, GPU resource binding and actual game draws
remain unfinished. No new Snipperclips probe was run.
Reference: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/3d/clb197.h

Draw snapshots now expose lossless Maxwell-B vertex fetch descriptors from both
attribute banks, decoded directly from each draw's immutable register mirror.
Stream, inactive source, byte offset, component width/type encodings, channel swap
and reserved bits are retained. Unwritten/out-of-range attributes remain unknown;
no reset state, Vulkan location or executable format is inferred. Regressions
cover both bank boundaries, later writes not changing earlier draws and reserved
bit preservation. Vertex stream/resource lowering and actual game shader execution
remain incomplete; the Snipperclips ZCull startup frontier has not been reprobed.
Reference: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/3d/clb197.h

The synchronous guest memory executor now supports NVB06F four-byte XOR/AND/OR
semaphore reductions with WFI enabled, for both reduction-format bit values.
Exact control decoding rejects timestamp-sized releases, disabled WFI and extra
switch/control bits. Read-modify-write is serialized by the mutable canonical
AddressSpace in the current interpreter/channel runtime; failed destination
permission checks do not modify semaphore or adjacent bytes. Tests cover all
three operations/formats and a read-only GPU lease. This does not implement
Vulkan-side concurrent atomics, arithmetic reductions or guest fence thresholds.
NVIDIA's newer manual distinguishes strict/circular GEQ acquires; no unverified
mapping of those modes onto Maxwell control 4 is introduced here.
References:
https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/host/clb06f.h
https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/manuals/turing/tu104/dev_pbdma.ref.txt

Asynchronous service/GPU polling faults now use the CPU runner's explicit stop
path instead of returning early and discarding local CPU context/progress.
The native-backend regression injects a polling error after a real ARM64 loop
executes 4,096 instructions and verifies retained PC/registers plus actual guest
memory effects. A process-level pre-scheduling fault reports a stop rather than
idle/success and leaves its ready thread queued. All 130 runtime tests and strict
Runtime Clippy pass. This fixes fault accounting/diagnostics, not rendering or
the unresolved Snipperclips ZCull startup operation; no new game probe was run.

AllocObjCtx now has strict native 16-byte transport decoding and runtime-owned
Maxwell-B graphics object allocation (class 0xb197, flags zero). Object IDs refer
to the allocated channel's existing per-channel decoder context, are never read
from untrusted output words and are not recycled. Unsupported classes/flags,
duplicate allocations and missing queues reject. Real HIPC coverage verifies
writeback, failed requests preserving identity state, close reclaiming the
one-channel backend capacity and a second channel receiving fresh syncpoint/
object IDs. Focused decoder/native tests, strict Runtime Clippy and workspace
checking pass. This is object ownership/bootstrap, not executable game graphics;
the documented Snipperclips ZCull frontier is unchanged and was not reprobed.
Reference: https://raw.githubusercontent.com/switchbrew/libnx/master/nx/source/nvidia/ioctl/nvchannel.c

The real native HIPC regression now covers Ioctl2 KickoffPb's separate 24-byte
header, entry input and header writeback descriptors, not only inline submission.
Admission leaves DMA destination bytes untouched and returns no requested fence;
the canonical GPU scheduler later executes the queued DMA/WFI. Changing source
CPU memory after admission changes actual destination bytes at execution, proving
this path does not snapshot transfer data. This covers zero-fence HW-format
KickoffPb only; increment flags, guest fence waits, actual game shaders and the
documented ZCull startup frontier remain unfinished/unchanged.

FIFO WFI (byte method 0x78, word method 0x1e) is now retained as an ordered
HostWaitForIdle event independently of bound engine state. Pending macro events
flush before the barrier. The synchronous memory executor accepts scopes 0/1
only after supported prior effects and with no incomplete split upload; unknown
prefix events, reserved scope bits and pending uploads reject. Graphics lowering
still marks this barrier opaque until Vulkan execution-order integration exists.
The real native DMA fixture now includes WFI after its transfer; focused tests
also verify malformed scopes and incomplete/unsupported prefixes cannot produce
barrier success. Native submission increment flags remain unsupported: libnx
client counter bookkeeping alone does not prove the driver's increment semantics.
Reference: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/host/clb06f.h

Fresh real Snipperclips probe with the native backend installed and GPU polling
enabled reaches 199,328,011 CPU instructions at PC 0x7102649744, then stops at
the same unsupported ZCullGetCtxSize IOCTL (0x80044701) on GPU control. This is
the current worktree frontier; earlier instruction counts are historical. No
new game frame, menu, audio or gameplay was verified by this probe.

Canonical GPU polling is now connected to thread/process execution slices and
still runs when a CPU thread is waiting. A bounded round-robin cursor grants one
descriptor per visited channel and wraps without a snapshot of open channels;
live process memory and current nvdrv address spaces are borrowed at each poll.
NativeGpuBackend is installed in the CLI's real content CPU probe and Linux
default runtime composition. Regressions verify a native admitted DMA remains
unexecuted until the waiting-thread runner poll, then modifies actual memory,
and verify fair two-channel polling with a one-channel budget. All 129 runtime
tests, 35 kernel tests, strict CLI/Linux Clippy, workspace type checking and
formatting pass. Nonzero guest submission synchronization and actual game
Maxwell rendering still remain unfinished.

NativeGpuBackend now admits typed inline SubmitGpfifo and split KickoffPb batches
with HW-format flag bit 2 and zero fence input only. Other synchronization flags
reject without queue mutation. Admission copies descriptors, not pushbuffer
contents; reply retains unrequested fence words and does not imply completion.
Bounded explicit polling executes actual live-memory command effects, preserves
waiting cursors and reports durable faults. The real native inline-IOCTL regression
rejects unsupported increment flags, verifies DMA has not run upon admission,
then polls and observes the actual destination bytes. Production scheduling,
nonzero synchronization semantics and game graphics remain incomplete.
References:
https://github.com/kfractal/nvgpu/blob/master/uapi/linux/nvgpu.h
https://android.googlesource.com/kernel/tegra.git/+/android-tegra-3.10/drivers/gpu/nvgpu/gk20a/channel_gk20a.c

Native GPU backend teardown now covers the final nvdrv session endpoint and
authenticated process exit, not only explicit descriptor Close. Closing one
clone preserves the canonical server and its backend channels; last-endpoint
release notifies the backend before destroying that server. Process exit releases
owned channels and repeated exit is idempotent. Backend close callbacks are
explicitly idempotent for later handle cleanup. Native HIPC clone/close and
process-exit regressions pass; exited processes cannot enter the GPU IOCTL
backend. Native submission execution and production installation remain open.

NativeGpuBackend now implements AllocGpfifoEx2 with an actual bounded runtime
queue and an owned initial syncpoint, for flags=1 and zero unknown parameters.
Duplicate allocation and software capacity errors reject; unimplemented native
submission still rejects rather than returning a fence success. Canonical nvdrv
Close now notifies the backend after descriptor removal, releasing its queue
and syncpoint state. The real HIPC regression checks allocated writeback overwrites
untrusted input fence words with owned initial state, and checks close cleanup.
This backend is not installed in the production runtime yet: final-session and
process-exit teardown, submission semantics and execution scheduling remain to
be connected before claiming actual game GPU integration.

Horizon now provides a host-neutral NvGpuIoctlBackend installation boundary
for native AllocGpfifoEx2, inline SubmitGpfifo and Ioctl2 KickoffPb requests.
Dispatch authenticates the live client/channel and borrows canonical process
memory and current GPU mappings; other device commands retain nvdrv dispatch.
Native HIPC argument padding is removed before typed transport decoding. A
backend cannot be replaced after installation, avoiding abandonment of pending
execution state. The real HIPC regression installs a rejection-only probe,
verifies live mapped reads and observes its NvError response; it does not claim
queue allocation or game submission success. The production runtime backend
still needs implementation/installation and channel-close cleanup.

The native-driver mapping regression now exercises synchronized host admission
through a real mapped semaphore wait: completed syncpoint state remains unchanged
while blocked, advances only after live CPU memory satisfies the wait and the
descriptor executes, and remains unchanged on a subsequent unmapped-semaphore
execution fault. Guest native SubmitGpfifo/KickoffPb transport and guest fence
wait events are still not connected to this runtime boundary. A fresh metadata-only
inspection of the local firmware NV-services program still returns MissingKey
for key generation 22; no keys or decrypted firmware contents are logged.

Host-side synchronized channel admission now reserves an owned syncpoint ticket,
rolls it back on admission failure, and records the complete batch's monotonic
descriptor boundary. Explicit retirement authenticates the live channel and
completes only fully retired boundaries; pending split inline uploads defer
completion. A native-driver mapping regression checks failed-admission rollback
and no early completion across a DMA-plus-no-op batch. This is runtime plumbing,
not interpretation of guest submission flags or a native IOCTL success reply;
Snipperclips rendering remains unverified; no new probe beyond the documented
ZCull boot frontier was performed for this change.

Runtime GPU channels now expose monotonic admitted/retired descriptor counts
for associating later synchronization reservations with submission boundaries.
Only fully executed entries retire; semaphore waits and durable faults leave
the head unretired. Checked lifetime admission rejects sequence exhaustion
without queue mutation. These software positions are not guest fence values;
native submission/reply and syncpoint retirement integration remain unfinished.

Syncpoint reservations now support cancellation before GPU queue admission.
Only the newest pending reservation can be removed; completed counters remain
unchanged and serial identities are never reused. Regression coverage checks
stale tickets, cancellation order, foreign ownership and rollback across public
counter wrapping. This prepares failed-submission rollback, but is not native
submission integration or evidence of a new Snipperclips boot milestone.

A software NvGpuSyncpoints manager now owns syncpoints by canonical channel
identity and separates reserved fence values from explicitly completed values.
Opaque reservation tokens include manager identity and serial ordering; foreign
channels/managers, out-of-order and duplicate completion reject without advancing
the counter. Released IDs are never recycled. Software capacity/pending budgets
and checked serial exhaustion bound state; wrapping public counters do not imply
implemented guest wait-threshold semantics. Ownership/retirement tests and strict
Horizon Clippy pass. Native allocation/submission IOCTLs still need integration
with actual command execution before any fence success is returned to the game.

AllocGpfifoEx2 now has a strict 32-byte wire/IPC decoder for IOCTL 0xc020481a,
with caller-bounded queue capacity, retained flags and four uninterpreted input
parameters. Output fence words are not mistaken for input synchronization.
AutoSelect inactive slots preserve the correct writeback index; invalid buffer
directions/capacities and truncated payloads reject without state mutation.
This is allocation transport decoding, not an implemented allocation reply:
canonical syncpoint ownership, channel queue creation and native submission
still need integration before reporting an allocated/completed fence.
Reference: https://raw.githubusercontent.com/switchbrew/libnx/master/nx/source/nvidia/ioctl/nvchannel.c

Vulkan lowering now validates shader stage identity for every graphics/compute
pipeline reference instead of checking only whether its numeric ID exists.
Vertex/fragment mismatches preserve expected and actual stages in a typed error;
duplicate shader IDs reject before compilation can populate an ambiguous cache.
Focused tests and strict Vulkan Clippy pass. Attribute address mapping to actual
Maxwell interfaces, native submission and Snipperclips rendering remain unfinished.

Maxwell structural decoding now distinguishes attribute load/store opcodes and
retains their data/address/vertex registers, 10-bit offset, 1..4 component width
and patch/output flags alongside the complete raw instruction. Store family
classification precedes the broader load mask. Tests cover both directions and
all widths and verify these accesses remain opaque in conservative semantic
lifting: no unverified mapping to Vulkan input locations is invented. Attribute
address/interface typing and execution remain to be connected.
Reference: https://raw.githubusercontent.com/envytools/envytools/master/envydis/gm107.c

Straight-line vertex SPIR-V now supports scalar LoadInput components, F32
constants, typed Move and ComposeVec4 before explicit Position output. Constants
are declared globally but assigned to registers in program order; undefined
forward uses, invalid components and scalar/vector type mismatches reject. The
native vertex regression now reconstructs XY from actual attributes and ZW from
constants, copies the vector and verifies direct/indexed reference pixels and
automatic vertex refresh. This expands canonical shader execution, not Maxwell
attribute decoding or actual game shader compatibility.

Index and indirect payload inspection now each enforce a cumulative 64 MiB
read budget per validation phase, not merely a per-allocation limit. This prevents
many individually admissible padded draws from causing unbounded aggregate
inspection work. Checked charging rejects over-budget/overflow without changing
the counter; six validation tests and strict Vulkan Clippy pass. Native nvdrv
channel allocation/submission still requires a real syncpoint lifecycle; no
completion fence or new Snipperclips boot progress is synthesized here.

CPU-provided indirect draw payloads are now inspected before Vulkan submission.
A bounded validation-only expansion preserves command order, decodes indexed/
nonindexed arguments and reuses direct vertex/index fetch checks; the original
indirect commands remain the submitted plan. Nonzero firstInstance rejects since
the device feature is not enabled. GPU-written argument buffers reject pending
execution-time inspection; software budgets bound commands and read bytes. Tests
exercise valid indexed arguments, index-fetch overflow and unsupported first
instance. All five explicitly selected SwiftShader regressions and strict Vulkan
Clippy pass. Native guest submission/game frames remain incomplete.

Fresh Snipperclips CPU probe after the latest graphics changes completes
199,328,026 instructions at PC 0x7102649744 and stops on unsupported GPU-control
IOCTL 2147763969 (0x80044701, ZCullGetCtxSize). This supersedes earlier probe
instruction counts for the current worktree; the operation/frontier is unchanged.
The CLI inspection's successful exit is not evidence of a successful game boot:
no game frame, interactive menu, audio or gameplay has been verified.

Whole-workspace verification after canonical provider integration, vertex-input
SPIR-V emission and draw fetch validation passes: offline workspace tests,
all-target Clippy with warnings denied, formatting check and workspace rustdoc
with warnings denied. All five ignored Vulkan tests were additionally selected
explicitly on SwiftShader and pass (uploads, indexed, indirect, vertex attributes
and presentation blit). These are synthetic renderer regressions, not a native
Linux window session or Snipperclips gameplay verification.

Indexed draw payload validation now decodes bounded CPU-provided U8/U16/U32
indices, applies signed base vertex and validates actual vertex/instance attribute
fetch ends. Negative effective indices and out-of-bounds vertex fetches reject.
Index buffers written by the same GPU plan explicitly reject pending execution-
time inspection rather than certifying stale CPU bytes. Tests use nonzero index
binding/first-index offsets and negative base vertex; native vertex-input coverage
also compares indexed pixels and rejects an index selecting a fourth vertex
without modifying the previous target. Indirect fetch inspection and actual
guest draw submission remain incomplete; no new Snipperclips frontier is claimed.

Direct nonindexed Vulkan draw validation now retains the graphics pipeline and
vertex bindings, requires vertex buffer usage and computes each attribute's
final fetch from first_vertex/count, input rate, stride, binding offset, attribute
offset and format byte width. Overflow, missing binding and out-of-bounds fetch
reject before driver preparation. Tests cover exact-end/one-byte-short ranges,
per-instance indexing, zero stride and zero-count no-fetch behavior. Indexed
and indirect vertex fetch validation still need index/argument payload inspection;
native guest draw submission and Snipperclips rendering remain incomplete.

SPIR-V vertex compilation now supports explicit LoadInputVec4 attributes and
StoreBuiltin Position output in a straight-line, terminated program. Undefined
registers, unsupported builtins/operations, missing output/return and non-Nop
suffixes after return reject rather than falling back to the smoke triangle.
The native SwiftShader regression supplies actual Rgba32Float vertex attributes
at location 2/binding 1, with 20-byte stride, 4-byte attribute offset and 8-byte
buffer binding offset. Pixels match the reference triangle; CPU changes to
degenerate vertices suppress it and restoring bytes restores pixels automatically.
This validates canonical vertex input, not Maxwell attribute lifting, native
guest draw submission or Snipperclips rendering.
Reference: https://registry.khronos.org/SPIR-V/specs/unified1/SPIRV.html

Native SwiftShader draw-input upload coverage now inspects actual host-visible
Vulkan allocation bytes for vertex, index and indirect buffers after repeated
provider changes and restoration under stable resource identities. Inspection
invalidates the mapped allocation and unmaps before reporting failure. An
undersized replacement rejects without changing the previously certified cache
identity. Vertex allocation coherence is verified, but a shader fetching those
vertices and actual game rendering remain unverified.

Vulkan cached draw-input coherence now includes vertex and indirect buffers,
alongside indices. Certified content changes trigger bounded full refresh after
retiring device work; pending GPU writeback still prohibits overwriting its
buffer. Initial uploads require complete input payloads. Native SwiftShader
indirect regressions mutate arguments to suppress draws, then restore the same
resource identity and verify reference pixels without explicit refresh calls.
Vertex refresh shares the implementation but lacks a dedicated pixel regression;
native guest submission and actual Snipperclips frames remain incomplete.

Mapped resource resolution now searches all whole-range aliases for the required
read/write permission instead of rejecting at the first denied alias. Coverage
checks nonzero resource offsets, permitted read/write/fingerprinting, untouched
denied backing, resource holes and denial after removing the permitted alias.
Permission checks remain per GPU mapping; this fixes provider admission without
claiming actual guest rendering or advancement beyond ZCullGetCtxSize.

The native driver regression now closes its GPU channel through HIPC/CMIF and
verifies that with_channel_resources rejects the closed descriptor before calling
backend code, despite the retained resource mapping. This validates revocation
at the authenticated integration boundary, not completion of native submission
or a game graphics frame.

with_channel_resources now authenticates the kernel's live nvdrv channel before
borrowing canonical process memory and constructing a mapped backend provider.
The native IPC mapping/DMA regression reads the destination through that provider
and observes its fingerprint change only after queued DMA execution. The provider
is scoped to the kernel borrow and does not snapshot mapping ownership. This is
an integration boundary; automatic guest draw/backend dispatch remains unfinished.
The NV firmware inspection still fails with MissingKey using the supplied key
file, so no verified ZCull context-size profile or new game frontier is claimed.

Mapped resource access now preserves typed GPU memory failures and their error
source instead of disguising inaccessible canonical mappings as resource bounds
errors. Incorrect memory read lengths have a separate UnexpectedReadSize error.
The canonical unmap regression checks the resource identity, failing GPU address,
read size and retained error source. Resource bounds remain validated separately;
this improves integration diagnostics without advancing the game boot frontier.

Canonical mapped-resource fingerprint coverage now verifies direct CPU writes,
unchanged identity after writes outside the observed range, rejection after GPU
unmap, and changed backing after remapping the same GPU address and ResourceId.
Restoring identical bytes in the new backing restores the content identity.
This guards stale-cache detection without inventing allocation generation IDs;
it does not yet prove guest draw submission or game rendering.

The runtime now provides GuestGpuResourceMemory, a bounded read/write adapter
from mapped resource providers to canonical live nvmap mappings. A regression
reads and writes across two mappings, observes updated content fingerprints and
verifies the actual CPU backing bytes. Foreign process memory rejects writes;
unmapping the second segment rejects a cross-segment write without modifying
the first segment. Write failures retain a distinct GPU memory error category.
This connects provider memory access, not guest draw submission or game frames;
the Snipperclips initialization frontier remains ZCullGetCtxSize.

Mapped resource providers now compute exact-range BLAKE3 fingerprints through the
live memory abstraction after resource-offset/permission validation, with a 64 MiB
hash budget. Unreadable or incorrectly sized memory responses reject instead of
certifying stale data; ordinary mapped reads now reject size mismatch rather than
panicking in copy_from_slice. Coverage observes mapped writes changing the hash
and rejects resource holes/out-of-bounds ranges. Native canonical nvmap provider
integration and automatic game-index refresh still remain to be connected.

Cached Vulkan index preparation now compares certified exact-range BLAKE3 provider
fingerprints and automatically refreshes changed content before the next draw.
The cache records hashes of bytes actually uploaded, not a separately observed
provider hash; GPU-written buffers invalidate this identity and pending writeback
still prevents overwrite. Providers returning None retain explicit refresh policy.
Native indexed regression no longer manually refreshes changed/restored indices:
submission alone observes both pixel changes. This is connected to in-memory
providers; canonical guest providers still need fingerprint/dirty tracking support.

Resource providers now expose an optional exact-range content fingerprint, with
None explicitly meaning no certified identity. The in-memory provider uses BLAKE3
on validated ranges: identical bytes across provider replacement retain identity,
in-range changes invalidate it and outside-range changes do not. Bounds/overflow
and missing resources reject. Existing providers default to None; backend dirty
tracking and automatic refresh have not yet been connected to this contract.

Native indirect drawing now has a SwiftShader pixel regression for indexed and
nonindexed parameters, nonzero argument-buffer offset, packed zero canonical
stride and padded stride. Multi-draw cases put a no-op first and the actual
triangle second so incorrect stride cannot pass by repeating the same triangle;
unsupported multi-draw capability instead checks rejection. Indexed arguments
exercise first index and negative base vertex. This uses synthetic canonical
shaders/resources, not guest Maxwell submission or game rendering.

Indirect draw admission now checks indirect usage/offset alignment, multi-draw
stride size/alignment, and an index binding for indexed calls. Zero canonical
stride is recorded as the packed Vulkan command size (16/20 bytes), matching
range analysis instead of sending zero for multi-draw. Logical-device creation
enables multiDrawIndirect only if supported; count capability/physical limit is
checked before resource preparation. Tests exercise valid packed/padded strides
and undersized/misaligned rejection. Indirect argument payload execution and
guest rendering remain unverified.
Reference: https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdDrawIndirect.html

Vulkan plan analysis now retains index binding and validates direct indexed draw
ranges using binding offset, first_index, count and 1/2/4-byte width before driver
resource preparation. Missing bindings, unknown types, wrong index usage and
misaligned/empty binding ranges reject. Zero-count/zero-instance draws perform no
fetch-range check but still require a valid binding. Tests cover all widths,
exact-end acceptance and one-byte-short/missing-buffer rejection. Indirect indexed
draw fetch ranges still require runtime argument inspection.

Native buffer creation now retains a pending-allocation RAII owner from buffer
creation through memory-type selection, allocation, binding, mapping and upload.
Any returned error destroys the partial buffer before freeing memory, unmapping
if necessary. Ownership transfers to the cache only after complete initialization.
Strict Clippy and the native indexed-render/refresh regression pass; no injected
driver-allocation failure or native game rendering is claimed.

Initial Vulkan index-buffer upload now requires complete provider bytes and an
explicit 64 MiB software budget rather than hiding missing resources or creating
partially initialized index storage. Provider reads precede native allocation.
Host uploads flush the whole mapped allocation and unmap before propagating flush
errors, covering noncoherent memory types as well as coherent ones. The native
indexed regression rejects missing/short index sources without cache insertion
and still verifies rendered pixels/refresh. Noncoherent hardware is not available
for direct validation, and native allocation-failure cleanup remains under audit.

Vulkan exposes an explicit cached-index refresh from the resource provider,
bounded to 64 MiB and disallowed for pending-writeback resources. It reads the
complete source before mutation, retires all device work before host writes,
flushes the whole mapped allocation (coherent memory is only preferred), and
unmaps even if flushing fails. The native indexed regression changes indices to
a degenerate triangle and verifies only clear pixels, then restores indices and
recovers the reference image. Automatic guest dirty tracking/refresh scheduling
is still unimplemented; this is a serialized explicit backend operation.

Vulkan buffer-cache reuse now checks the retained size and usage contract for an
existing resource identity. Changed descriptors reject explicitly rather than
binding an old allocation as a differently sized/typed buffer. Native indexed
regression verifies both size/usage rejection and successful repeated rendering
with the original plan afterward, confirming rejection preserves its allocation.
Dynamic resizing and provider-content refresh still require separate integration;
this check does not make cached guest buffers coherent by itself.

Native indexed-draw regression passes on the explicitly selected SwiftShader
Vulkan device: U16/U32 draws match every reference triangle pixel across repeated
execution, including nonzero binding offset, first_index and negative base vertex.
U8 either follows the same pixel check when enabled or verifies explicit feature
rejection. Each format uses a distinct resource identity/size. The first fixture
incorrectly reused one resource ID for different byte widths; separating those
identities corrected the test. This validates backend indexed drawing, not Maxwell
pipeline binding, native nvdrv submission or Snipperclips rendering.

Vulkan uint8 index binding now requires actual feature enablement. Device creation
queries VK_EXT_index_type_uint8 only when advertised, enables its feature through
a fresh pNext struct when supported, and retains that capability for binding.
Unsupported devices reject U8 instead of recording an invalid UINT8 command.
U16/U32 paths are unchanged; a KHR-only device currently conservatively rejects
U8. This is backend capability admission, not a validated guest indexed draw.
Reference: https://docs.vulkan.org/refpages/latest/refpages/source/VkPhysicalDeviceIndexTypeUint8Features.html

Runtime index reads now use canonical live GPU mappings after checking exact
index format, first/count range, the inclusive programmed limit and a 64 MiB
software allocation budget. The entire range is validated before allocation;
zero-count draws fetch nothing. Tests verify CPU updates, exact-limit acceptance,
one-byte-short limit rejection, foreign memory and mapping removal. Bytes remain
encoded as guest indices; Vulkan binding/restart/topology and actual drawing are
not implemented by this reader.
Inclusive-limit reference: https://chromium.googlesource.com/external/gitlab.freedesktop.org/mesa/mesa/+/6f19fc972f4d3004a6719ccbcca221e33098a481/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c

Index-buffer state exposes exact 1/2/4-byte width decoding and a checked
nonempty-draw read range using first-index offset and count. Reserved size/address
bits and spans beyond the 40-bit GPU VA space reject; zero-count draws yield no
fetch range. Tests cover all widths and the last valid four-byte address versus
crossing/starting beyond the VA boundary. The helper intentionally does not admit
the programmed limit or prove mappings/read permissions; resource binding remains
required before execution. No native indexed draw is yet supported.

Index-buffer state now matches NVB197: C/D retain the limit address, E retains
the index-size word (0/1/2 = 1/2/4-byte indices), and F retains the first index.
The previous boolean-enable/format interpretation of E/F was incorrect. Draw
snapshots now include this typed state alongside exact raw registers. Regression
checks a one-byte indexed draw with nonzero first index, 40-bit addresses and
immutable captured state after later writes; unsupported size bits stay raw.
Native index-buffer binding and draw execution remain unimplemented.
Reference: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/3d/clb197.h

Trace lowering no longer silently ignores indexed draws or graphics syncpoint
increments. These unimplemented execution events count as opaque and stop lowering
at the already-translated prefix, preventing a dependent suffix from appearing
fully executable after omitted work. Regression covers both events between an
admitted barrier and a subsequent draw. Indexed resource binding and actual
syncpoint completion still require implementation; no game rendering is claimed.

GPU compiler errors now implement Display and the standard Error cause chain:
memory/decode failures retain their concrete source, and missing/conflicting or
incorrectly sized windows report their address and relevant sizes. This permits
runtime callers to propagate compiler failures without replacing them with generic
strings. Focused tests cover typed source recovery and bounded diagnostics; shader
execution integration and the real-game frontier remain unchanged.

Whole-workspace revalidation after the FIFO/frontend/shader changes passes offline
tests, all-target strict Clippy, formatting and rustdoc with warnings denied.
The fresh Snipperclips probe executes 199,328,379 instructions and stops at
PC 0x7102649744 on GPU-control IOCTL 2147763969 (0x80044701, ZCullGetCtxSize).
This supersedes older instruction-count observations, but the actual frontier is
unchanged: no game frame, interactive menu, audio or gameplay is validated.

Live guest shader-memory regression now compiles a captured fragment draw whose
header crosses two canonical GPU mappings. Changing the later channel program
region does not redirect the earlier draw; repeated compilation observes CPU
header writes and retains binding/header metadata. Cloned foreign memory and a
removed second mapping reject compilation. The fixture uses a synthetic header,
not an executable game shader; no Vulkan draw or game-rendering milestone is
claimed.

Graphics/compute compilation now checks that memory providers return exactly the
admitted shader window length before parsing. A word-aligned short response could
previously parse successfully while dropping code; oversized responses could
extend compilation beyond the selected window. Regression covers short, empty
and oversized provider results for both program kinds, with explicit size errors.
This is compiler-boundary validation, not actual game-shader execution.

Compiled graphics shaders now retain their discovered slot, raw selection,
binding/register-count metadata and complete graphics header. Previously only
address, canonical stage/module and lift statistics survived compilation, losing
the distinction between vertex-cull and vertex programs sharing an address/stage
and discarding header inputs needed for interface construction. Regression checks
two enabled vertex programs at the same address with different slot metadata;
compute compilation carries no fabricated graphics source/header. This does not
implement combined vertex execution or Vulkan interface/resource binding.

Graphics shader discovery now decodes NVB197_SET_PIPELINE_SHADER enable bit 0
and type bits 7:4 instead of inferring activation from any nonzero retained
register and stage from slot index. Disabled stale programs do not participate
in reads or address arithmetic; enabled unknown types/reserved select bits fail
explicitly. Coverage exercises all documented types, disabled overflowing stale
offsets and malformed enabled controls. Vertex-cull/vertex types still share the
canonical vertex-stage representation; discovery is not dual-program execution.
Reference: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/3d/clb197.h

Pending graphics macro emissions/failures now flush before a command routed to
another engine, or before SetObject changes a binding. Previously such effects
could be appended at pushbuffer end after the following engine's events. The
regression covers both an already-bound foreign engine and same-subchannel
rebinding, checking exact event order and complete trace coverage. This retains
the existing invocation-boundary model; cross-descriptor macro lifetime and native
GPU submission integration remain incomplete. No new game frame is verified.

GPFIFO control NOP entries are now admitted and consumed without fetching guest
memory. Only opcode zero with an entirely zero high word is supported; low-word
operand bits are not treated as an address. Privileged, subroutine, wait,
illegal/CRC/unknown and reserved high controls remain rejected. Queue coverage
verifies unmapped operands and atomic rejection of mixed NOP/unsupported batches.
Firmware NV inspection was rerun: generation 22 still fails with MissingKey.
No key values were printed and the Snipperclips ZCull frontier is unchanged.

Common FIFO NOP (NVB06F byte offset 0x08, method index 2) now bypasses engine
routing and ignores its handle payload. It produces no execution event regardless
of bound class, avoiding a false unsupported graphics/opaque operation. Coverage
checks all eight subchannels, unbound/graphics/unknown class bindings and zero,
maximum and arbitrary payloads. Counter acquire GEQ remains unsupported: class
headers alone do not establish its wraparound comparison semantics.
Register reference: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/host/clb06f.h

FIFO ACQ_AND (exact control 8, switch disabled) now waits until the live
four-byte semaphore value shares any set bit with its payload mask. It uses
the same retained execution cursor as equality acquire, without replaying the
applied prefix. Regression coverage includes zero, a nonzero disjoint mask,
and a partial matching mask that is not equal to the payload. GEQ, reduction,
other control modes, native fences and interrupt wakeups remain unsupported.
Semantic reference: https://github.com/NVIDIA/open-gpu-doc/blob/master/manuals/turing/tu104/dev_pbdma.ref.txt

Semaphore-resume regression now covers GPU mapping replacement during suspension:
the retained acquire observes the replacement backing rather than the old lease.
Removing the mapping instead produces a durable execution fault, retaining the
head descriptor; restoring the mapping does not silently restart the faulted
channel. All nine runtime GPU tests pass. This strengthens software scheduling
coverage, not native GPU submission integration or a new game boot milestone.

FIFO semaphore equality acquire is now modeled against live mapped GPU memory.
An unmet acquire suspends the software channel without faulting or popping its
head descriptor. The channel retains the decoded trace and first unexecuted event;
retry rereads the semaphore value and resumes only that suffix, never replaying
an already-applied release/DMA/inline prefix. Memory-access failures remain real
faults, and other acquire controls plus incomplete prior uploads are unsupported.
The new regression releases one value, waits on another, mutates the first from
the CPU, confirms repeated waits do not overwrite it, then satisfies the wait and
observes the final release. All eight runtime GPU tests and strict Clippy pass.
This is explicit caller-polled software scheduling, not native nvdrv fence wait,
interrupt wakeup, graphical completion or new Snipperclips boot progress.

Common NVB06F FIFO semaphore methods (word indices 4..7) now retain upper/lower
address and payload registers across pushbuffers and emit ordered semaphore
operations independent of bound engine class. Unsupported control/address bits
are preserved, not masked into successful operations. The synchronous runtime
admits only RELEASE with WFI enabled and a four-byte payload, after prior supported
transfers and with no incomplete inline upload, writing through live canonical
GPU/nvmap memory. Other operations/sizes/WFI modes fail explicitly. The new
regression checks split-buffer state, real four-byte publication and untouched
surrounding bytes, plus unsupported modes without writes. Existing GPU IR/runtime
tests and strict Clippy pass. This does not implement semaphore acquire scheduling,
native nvdrv submission fences, graphics completion or a new Snipperclips frontier.
Primary register source: https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/host/clb06f.h

Synchronization-model audit removed implicit zero observations: waits on absent
semaphore addresses or unknown syncpoint IDs no longer succeed for zero payloads
or thresholds. Syncpoint lookup returns an optional value, distinguishing missing
state from a real counter value. Tests cover all absent semaphore comparisons,
unknown syncpoint waits, explicit zero signals and distinct IDs. This model is
still not wired to native nvdrv submission or real guest memory semaphores;
wraparound threshold semantics, fence allocation and completion integration remain
open. These changes do not advance Snipperclips beyond ZCullGetCtxSize.

Native HID identity/handle regression now includes style-event command 106:
missing PID and foreign resource identities are rejected without allocating any
handle; authorized replies carry exactly one copied readable-event handle, not a
move handle. The returned event then observes the kernel's successful primary
connection publication, remains clear after unchanged publication, and signals
again after disconnection. This validates native transport plus publication-event
integration, not guest wait-instruction execution, initial subscription semantics
or Snipperclips controller behavior. The focused test and strict Clippy pass.

HID command 106 now returns a retained manual-clear readable style event scoped
to resource identity and Npad ID. Native transport applies the existing caller
PID/resource checks with the command's aligned identity offset. The ignored client
pointer is never dereferenced. After successful FullKey storage publication, a
connected/disconnected style transition signals player zero's subscribed event;
unchanged style/button-only updates do not re-signal it. Tests cover retained
aliases, foreign-resource isolation, clear/re-signal behavior and truncated input.
Initial subscription notification and other style transitions remain unverified;
no physical device or interactive game-input milestone is claimed.
Command layout: https://raw.githubusercontent.com/switchbrew/libnx/master/nx/source/services/hid.c

Snipperclips revalidation after bounded execution/HID changes completes
199,328,011 instructions at PC 0x7102649744 and stops on GPU-control IOCTL
2147763969 (0x80044701, ZCullGetCtxSize). The current instruction count supersedes
earlier counts but the unsupported operation and real-game frontier are unchanged.
No menu, frame or interactive input is verified; the probe exits successfully as
an inspection command, not as evidence that the game booted successfully.

Process-runner polls now service primary HID before selecting a ready thread, so
an empty ready queue still updates input when the caller polls an idle process.
The interpreted HID-reader regression advances its injected clock by 8 ms,
changes host controls, runs an idle process poll with zero completed instructions,
then reads the newly published sample through another ARM64 LDR without remapping.
All 24 execution tests and strict runtime Clippy pass. This does not create an
independent idle-time event loop or prove physical keyboard/game integration.

Instruction-level HID visibility is now verified by a runtime regression. With
the 22.1.0 profile and an injected clock, interpreted SendSyncRequest instructions
create the native HID applet resource and obtain its retained shared-memory handle;
interpreted MapSharedMemory maps that handle read-only. The runner automatically
publishes a configured host Npad sample, and an interpreted ARM64 LDR reads its
button mask from the mapped FullKey ring. Policy setup still uses host registered
service dispatch, and this fixture is not unmodified homebrew or game execution.
The focused test and strict runtime all-target Clippy pass; Snipperclips input,
style-event notification, full metadata and controller multiplayer remain open.

Primary HID sampling is now polled by the bounded thread runner, including its
initial poll for waiting threads and between at most 4096 interpreted instructions.
It uses the injected kernel monotonic time and an explicit emulator policy of
one current-state sample per 8 ms; this is not a claimed measured firmware rate.
Long gaps coalesce to one current sample rather than inventing historical input.
Missing application buffers and other layout profiles remain untouched. The
injected-clock regression verifies initial publication, interval boundaries,
unchanged-state sampling and long-gap coalescing. All 23 existing execution tests
pass after slice bounding. Full scheduler idle-time polling and instruction-level
HID-reader validation remain to be verified; no interactive game milestone is met.

FullKey style and ring publication now commit in a single shared-memory revision.
The shared-storage batch validates all disjoint ranges before taking its write
lock; a valid style field followed by an out-of-range ring cannot partially
connect a controller. Tests verify this rollback boundary and overlapping-field
rejection. The kernel regression now maps the retained HID buffer read-only into
canonical guest memory, reads the published sample through that mapping, rejects
guest writes and observes one revision per tick. This proves canonical mapped
storage visibility, not execution of guest HID-reading instructions or automatic
runtime sampling; interactive game input remains unverified.

The 22.1.0 kernel now exposes caller-scheduled primary Npad publication into its
retained application-owned HID storage at Npad[0] base 0x9a00 and FullKeyLifo
offset 0x28. It does not allocate storage before the resource is requested. The
shared service policy gates active FullKey/player-zero controls; rejected policy
publishes a disconnected sample to clear stale controls while retaining sample
continuity. A kernel regression reads a retained alias at the wire offsets,
verifies controls/style and subsequent disconnection after removing player zero,
and verifies another application's missing buffer is not allocated. This is a
host-driven kernel regression, not a guest instruction-level input test. Automatic
sampling cadence, style notifications/metadata and other styles remain incomplete.
Wire layout source: https://switchbrew.org/w/index.php?mobileaction=toggle_view_desktop&title=HID_services

Kernel publication policy now observes the exact shared HID service state modified
by registered IPC dispatch, rather than a detached configuration copy. The primary
FullKey path requires active Npad, an explicitly accepted FullKey style and player
ID zero; lookup uses the process's canonical resource identity. The registry
regression checks activation alone and style alone do not admit publication,
accepted IDs enable it only for that resource, malformed replacement leaves prior
policy unchanged, and removing FullKey style disables it. This conservative path
does not implement implicit/default policy, other controller styles or publication
timing/firmware offsets; no guest input or boot milestone is claimed.

Npad common-ring publication now retains 17 atomic-storage slots, independent
per-tick sampling numbers, a bounded count and a wrapped tail. Unchanged controls
still receive consecutive samples, as required by readers that check sample
continuity. The complete ring is written through the existing shared-storage
lock and counters commit only after successful publication. Tests verify 40
ticks across wraparound through a retained shared-memory alias, matching outer
and inner sequence numbers, and unchanged history/revision after invalid writes.
The caller-supplied aligned ring offset is not a claimed firmware-layout mapping;
application policy, verified Npad offsets and timed kernel publication remain to
be wired. No game input or new Snipperclips frontier is verified.
Reader/format references: https://raw.githubusercontent.com/switchbrew/libnx/master/nx/source/services/hid.c
and https://raw.githubusercontent.com/switchbrew/libnx/master/nx/include/switch/services/hid.h

Horizon Npad common-state encoding now serializes the documented 0x28-byte
little-endian sample explicitly: sampling number, button mask, four signed stick
axes, connection attributes and zero alignment padding. Encoding bounds axes
even when a host constructs a sample directly; disconnected samples suppress
all controls. Tests verify individual field offsets, signed values, padding and
disconnect behavior. This is the sample codec only: ring sequence/header writes,
supported-style/ID policy, timed publication and canonical guest mapping updates
still need integration before controls can be read by a running game.
Format reference: https://raw.githubusercontent.com/switchbrew/libnx/master/nx/include/switch/services/hid.h

Linux keyboard-to-controller normalization is implemented for one virtual Npad:
physical WASD keys drive the left stick, arrows drive D-pad, L/K/I/J map A/B/X/Y,
Q/E and Z/C map shoulders/triggers, Enter/Backspace map Plus/Minus. Held keys are
tracked independently, opposite directions cancel, duplicate repeats do not
advance the sample, and focus loss/close clears controls. Horizon's host-neutral
Npad sample bounds axes and clears disconnected state. The window diagnostic
feeds this state without logging keys. Tests cover repeat/focus/opposing controls;
this does not yet serialize Npad rings into guest HID shared memory, attach physical
gamepads, implement multiplayer, or prove input works in Snipperclips.
Button reference: https://raw.githubusercontent.com/switchbrew/libnx/master/nx/include/switch/services/hid.h

Native presentation-blit validation now executes on explicitly selected SwiftShader
without a window. The ignored-by-default device regression renders the synthetic
256x256 triangle, transfers it to both 256x256 and 128x128 RGBA8 targets twice,
and compares every destination pixel with the source/nearest-sample reference.
It also verifies unchanged source pixels and restored source layout after each
transfer. The explicit offline test passes (one native test); this proves the
GPU transfer and 2:1 nearest downscaling, not BGRA conversion, swapchain execution,
displayed frames or guest-game rendering.
Command: `VK_LOADER_LAYERS_DISABLE='~implicit~' VK_DRIVER_FILES=/opt/LM-Studio/vk_swiftshader_icd.json cargo test -p exosphere-gpu-vulkan --offline native_present_blit -- --ignored --nocapture`.

Linux presentation can now transfer an actual owned backend image to the acquired
swapchain image without host readback. The backend records a nearest-filter blit,
checks RGBA8 source shape/state and RGBA8/BGRA8 destination formats plus physical
BLIT_SRC/BLIT_DST support, and restores source layout. Its unsafe external-image
boundary documents ownership, command recording and completion requirements;
the Linux wrapper establishes acquisition, target barriers and synchronous
submission/retirement on the same device. A narrow execute-plan wrapper permits
rendering without exposing replaceable mutable backend ownership. The native
diagnostic now renders a synthetic triangle and requests transfer/presentation.
This path is compile-checked, not display-verified or executed by guest rendering;
the native display/game milestone remains open.

Linux diagnostic clear/presentation is implemented: discard the acquired image,
transition to transfer destination, clear a finite RGBA color, transition to
presentation layout, submit with a binary completion semaphore, and queue present
with an EXT maintenance1 retirement fence. The synchronous path waits for both
presentation-reference retirement and command completion before reusing resources.
Out-of-date/surface-lost rejection still requires retirement; allocation failures
that did not enqueue presentation must not wait on an unsignaled present fence.
Unexpected failures prohibit reuse until recreation. The native smoke example now
requests diagnostic presentation on redraw. This remains unverified on a display:
no X11/Wayland server is configured and Xvfb/Xorg/Weston/Cage/Sway were not found
on PATH. Actual guest-frame transfer, pixel validation on a display, HID and game
boot remain open; the Linux presentation milestone is not checked off.
References: https://docs.vulkan.org/refpages/latest/refpages/source/vkQueuePresentKHR.html
and https://docs.vulkan.org/refpages/latest/refpages/source/VkSwapchainPresentFenceInfoKHR.html

Linux surface creation now explicitly requires EXT surface/swapchain maintenance1
and its surface-capabilities2 dependency. Backend creation queries and enables
the requested swapchain-maintenance feature, rejecting unsupported devices;
the headless backend does not request or enable it. This is a current platform
compatibility restriction, not support for every Linux Vulkan driver. Acquired
but unsubmitted images can now be released using ReleaseSwapchainImagesEXT,
clearing ownership only on success. The diagnostic exercises acquire/release
without rendering. This prepares explicit present-fence retirement but does not
yet implement submission or presentation, and native display validation is open.
Reference: https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_swapchain_maintenance1.html

Linux swapchain acquisition now polls with zero timeout, distinguishing not-ready,
out-of-date, suspended and acquired results. A successful image index is exposed
only after its acquisition fence completes. Only one image can be held, and
native acquisition/fence errors prohibit retry until recreation. The lifecycle
example exercises acquisition on redraw and explicitly holds the image until
resize/close; no transfer or presentation is implied. Native execution remains
unverified without a display. Presentation needs per-image synchronization and
safe retirement; a submission fence or device-idle wait alone does not prove
completion of presentation-engine semaphore use.
Reference: https://docs.vulkan.org/guide/latest/swapchain_semaphore_reuse.html

Linux swapchain lifecycle now includes synchronous replacement: wait for device
idle, release the old allocation, then requery the surface and allocate the new
chain. Minimized windows suspend allocation; replacement failures leave no stale
images exposed, and later resize events can retry. The `swapchain_smoke` example
wires native window resize events to this path. Platform unit tests and strict
all-target Clippy pass, including example compilation. Actual native resize,
acquisition and presentation remain unverified without a configured display;
no game frame or presentation milestone is claimed.

One-shot Vulkan command allocation now frees its command buffer if recording
initialization, recording completion or fence creation fails before submission.
The synthetic SwiftShader triangle/readback regression still verifies 64,262
pixels at 256x256 with identical repeated readback. These error-path ownership
fixes do not provide swapchain presentation or advance the game's boot frontier.

Linux presentation preparation: window callbacks and native Wayland/Xlib/Xcb
surface ownership are implemented, along with capability-checked swapchain
allocation. Extent selection now rejects minimized windows, malformed bounds,
mixed undefined-extent sentinels and fixed extents outside advertised limits;
unit tests cover these cases and per-dimension clamping. This does not implement
image acquisition, frame transfer or presentation synchronization. Resize-driven
replacement was added subsequently as described above. No displayed frame is verified: the current environment
has no configured X11/Wayland display. The Linux presentation milestone stays open.

Vulkan execution/readback milestone verified on the accessible SwiftShader CPU
device: the ash triangle smoke clears its RGBA8 target, renders, copies image
bytes to coherent staging memory and checks all pixels outside a narrow diagonal
edge band against the analytic triangle/background reference. At 256x256,
64,262 pixels pass; at 127x93, 11,374 pixels pass. A second readback is byte-for-byte
identical in both runs. Commands use `VK_DRIVER_FILES=/opt/LM-Studio/vk_swiftshader_icd.json`
and `VK_LOADER_LAYERS_DISABLE='~implicit~'`. This proves synthetic software-device
execution/readback, not physical GPU execution, game shaders or window presentation.

The ash triangle now executes successfully at 256x256 on SwiftShader using
`VK_DRIVER_FILES=/opt/LM-Studio/vk_swiftshader_icd.json` and
`VK_LOADER_LAYERS_DISABLE='~implicit~'`. Backend creation hung because reused
feature-query structs retained pNext links; rebuilding the device feature chain
with cleared links fixes the cycle. Unsupported optional feature structs are no
longer passed to device creation. Required core features are queried/enabled.
This verifies execution completion of a synthetic dynamic-rendering triangle,
not pixel correctness/readback or actual game shader execution. The combined
Vulkan execution/readback milestone remains open until image bytes are checked.

Software Vulkan investigation found SwiftShader bundled under
`/opt/LM-Studio`, outside the previously searched standard library directories.
With `VK_DRIVER_FILES=/opt/LM-Studio/vk_swiftshader_icd.json`, `vulkan-smoke`
passes instance/device creation, reports one physical device and submits a
command. The ash triangle test still times out after 45 seconds; rendered
pixels/readback and the Vulkan execution/readback milestone remain unverified.
The backend now queries and explicitly enables required timeline semaphore,
dynamic rendering and synchronization2 features instead of implicitly assuming
core-version availability enables them. No host driver configuration changed.

Native rendering revalidation on 2026-10-06: `vulkan-ash-triangle-smoke` still
fails at `vkCreateInstance` with `ERROR_INCOMPATIBLE_DRIVER`. `/dev/dri` is
absent, the installed ICD directory contains only the NVIDIA manifest, and no
Lavapipe/SwiftShader library was found under `/usr/lib` or `/usr/local/lib`.
This is an environment-specific validation blocker, not evidence of a working
software renderer or a completed Vulkan milestone. No driver installation or
host configuration change was performed.

Actual Snipperclips revalidation on 2026-10-06: the current CLI probe with a
1,000,000,000-instruction budget stops after 199,328,002 completed instructions
at PC `0x7102649744`, unknown GPU-control IOCTL `2147763969` (`0x80044701`,
ZCullGetCtxSize). This supersedes the older instruction-count observation for
the current build but does not change the unsupported operation or execution
frontier. Whole-workspace offline tests pass; no image or menu is verified.

ZCull context-size investigation: the NVIDIA Tegra driver queries the FECS
firmware with `discover_zcull_image_size`, stores the returned size in
`zcull_ctxsw_image_size`, and exposes that stored value through the context-size
query. Region/alignment fields from ZCullGetInfo therefore do not independently
prove the required context-switch allocation size. The guest query remains
unsupported until a verified matching profile/firmware value or implemented
context model supplies it; no arbitrary size is introduced to bypass game boot.
Primary source: https://android.googlesource.com/kernel/tegra/+/76359c267702c0815c82c970f38f5b27031d5ba6/drivers/gpu/nvgpu/gk20a/gr_gk20a.c

Submission wire decoding now supports the fixed 24-byte header plus inline
SubmitGpfifo entries or separate KickoffPb entries, borrowing payload bytes with
exact count/length and caller-supplied budget validation. Flags and input fence
metadata are preserved; the ignored client pointer is never dereferenced.
The runtime admission boundary accepts decoded batches only without flags or
nonzero fence metadata, rejecting unsupported synchronization without queue
mutation. Native mapping tests cover both layouts, actual DMA copying and
observing later unmaps. All 131 Horizon tests and 120 runtime tests pass.

Bound queue admission now authenticates the canonical native channel before
accepting descriptors, retaining its identity only after successful admission.
The native mapping regression verifies foreign-channel rejection without queue
mutation and capacity rejection without claiming ownership; another authenticated
channel can then admit and execute the first DMA batch. Runtime tests and strict
all-target Clippy pass. This is a host runtime admission boundary, not implemented
guest SubmitGpfifo/KickoffPb transport, fence completion or game rendering.

Quality gate verified on 2026-10-06 after explicit lint fixes and workspace formatting:
`cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --offline -- -D warnings`, and
`RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --offline` all pass.
This verifies development quality checks, not native GPU execution or game boot.
The last verified Snipperclips frontier remains ZCullGetCtxSize at 199,328,194
instructions; no frame, interactive menu or gameplay is verified.

- [x] Restore whole-workspace type checking and CLI compilation.
- [x] Pass whole-workspace tests, formatting, Clippy and documentation gates.
- [ ] Implement user-key crypto provider, ticket/title-key resolution, NCA integrity
      verification and bounded section readers; never include keys in artifacts/logs.
- [ ] Read CNMT/control/NPDM metadata, select program content, load/decompress ExeFS
      modules, apply relocations, map RomFS and initialize process/stack/TLS.
- [ ] Implement ARM64 baseline execution and native execution integration, with
      exact guest memory, flags, SIMD/FP, exceptions and SVC boundaries.
- [ ] Execute an unmodified homebrew through its main loop.
- [ ] Complete required Horizon SVC, IPC domains/TIPC, firmware-profiled services,
      file/save handling, HID shared memory, VI/Binder and audio.
- [ ] Wire nvdrv/nvmap GPU address mappings, command submission, resource state,
      MME invocation and guest synchronization to the runtime.
- [ ] Compile actual lifted Maxwell shaders to executable SPIR-V, including required
      memory, texture, predicate, control-flow and reconvergence operations.
- [x] Validate Vulkan execution/readback on an accessible physical/software device.
- [ ] Implement Linux surface/swapchain, frame presentation and window/input handling.
- [ ] Boot Snipperclips Plus to interactive menus and gameplay with audio/input.
- [ ] Validate repeated launches, saves/reloads, multiplayer input, scene transitions
      and sustained play; record errors, frame correctness and performance.
- [ ] Complete package reader/writer/mmap, dependency/conflict/rollback behavior,
      resource translation/hot reload, media decoding and Linux backend integration.
- [ ] Optimize only against measured workloads; validate cache persistence,
      staging/allocation lifetimes, compilation variants and profile-guided paths.
- [ ] After Snipperclips stabilization, boot and stabilize Super Mario Odyssey using
      the same regression gates; preserve Snipperclips compatibility.

No game launch or stability milestone is currently verified. Existing GPU closure
documents describe canonical representation or backend primitives, not these gates.

Header format reference: https://switchbrew.org/wiki/NCA
Thread SVC implementation reference: https://github.com/Atmosphere-NX/Atmosphere/blob/master/libraries/libmesosphere/source/svc/kern_svc_thread.cpp
Guest register wrapper reference: https://github.com/switchbrew/libnx/blob/master/nx/source/kernel/svc.s
Condition implementation reference: https://github.com/Atmosphere-NX/Atmosphere/blob/master/libraries/libmesosphere/source/kern_k_condition_variable.cpp
HID client framing reference: https://github.com/switchbrew/libnx/blob/master/nx/source/services/hid.c
Process metadata reference: https://switchbrew.org/wiki/NPDM
Performance service references: https://switchbrew.org/wiki/AM_services#apm
and https://switchbrew.org/wiki/APM_services
Application manager reference: https://switchbrew.org/wiki/AM_services#appletOE
HIPC control reference: https://switchbrew.org/wiki/IPC_Marshalling#InvokeManagerMethod
Add-on discovery references: https://switchbrew.org/wiki/NS_services#aoc:u
and https://reswitched.github.io/SwIPC/ifaces.html#nn::aocsrv::detail::IAddOnContentManager
Parental-control references: https://switchbrew.org/wiki/Parental_Control_services
and https://github.com/switchbrew/libnx/blob/master/nx/source/services/pctl.c
