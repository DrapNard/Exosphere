// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Backend-neutral shader representation.
//!
//! This IR is intentionally richer than the current SM53 decoder. Unknown Maxwell instructions may
//! remain opaque, but they no longer force a future redesign of the canonical representation.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShaderStage {
    Vertex,
    TessControl,
    TessEvaluation,
    Geometry,
    Fragment,
    Compute,
}

/// One output of a floating-point sampled-image view, after TIC swizzling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSampleChannel {
    /// Storage RGBA component, restricted to 0..4.
    Component(u8),
    Zero,
    One,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F16,
    F32,
    F64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Abs,
    Not,
    BitNot,
    Sqrt,
    Reciprocal,
    Rsqrt,
    Floor,
    Ceil,
    Fract,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    And,
    Or,
    Xor,
    ShiftLeft,
    ShiftRightLogical,
    ShiftRightArithmetic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordShiftKind {
    Left,
    RightLogical,
    RightArithmetic,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySpace {
    Uniform,
    Storage,
    Shared,
    Local,
    Global,
}

/// Shader attribute byte addressing, distinct from descriptor locations.
/// Optional registers retain address/vertex indirection rather than flattening it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderAttributeAddress {
    pub byte_offset: u16,
    pub address_register: Option<u32>,
    pub vertex_register: Option<u32>,
    pub per_patch: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureOp {
    Sample,
    SampleLod,
    SampleGrad,
    Fetch,
    Gather,
    QuerySize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicOp {
    Add,
    Min,
    Max,
    Inc,
    Dec,
    And,
    Or,
    Xor,
    Exchange,
    CompareExchange,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAccessWidth {
    U8,
    I8,
    U16,
    I16,
    B32,
    B64,
    B128,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpolationMode {
    /// Linear interpolation at the default sample position, without perspective correction.
    Pass,
    Multiply,
    Constant,
    Sc,
    Centroid,
    Offset,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubgroupOp {
    ShuffleIndex,
    ShuffleUp,
    ShuffleDown,
    ShuffleButterfly,
    VoteAll,
    VoteAny,
    VoteEqual,
    Ballot,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitfieldOp {
    ExtractSigned,
    ExtractUnsigned,
    Insert,
    FunnelShiftLeft,
    FunnelShiftRight,
    Permute,
    PopCount,
    FindLeading,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TernaryOp {
    Fma,
    IMad,
    Xmad,
    Lop3,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateCombine {
    And,
    Or,
    Xor,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateDomain {
    Integer,
    Float,
    Double,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TernaryControl {
    pub negate_b: bool,
    pub negate_c: bool,
    pub saturate: bool,
    pub high: bool,
    pub carry_in: bool,
    pub carry_out: bool,
    pub merge: bool,
    pub psl: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueModifier {
    pub negate: bool,
    pub absolute: bool,
    pub invert: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloatControl {
    pub saturate: bool,
    pub flush_to_zero: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegerControl {
    pub saturate: bool,
    pub carry_in: bool,
    pub carry_out: bool,
    pub high: bool,
    pub signed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierScope {
    Subgroup,
    Workgroup,
    Device,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingMode {
    NearestEven,
    TowardZero,
    TowardPositive,
    TowardNegative,
    None,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionControl {
    pub src: ScalarType,
    pub dst: ScalarType,
    pub rounding: RoundingMode,
    pub saturate: bool,
    pub flush_to_zero: bool,
    pub absolute: bool,
    pub negate: bool,
    pub raw_flags: u32,
}

impl ConversionControl {
    /// Exact word-to-float subset shared by constant and runtime lowering.
    pub(crate) fn is_plain_word_to_f32(self) -> bool {
        matches!(self.src, ScalarType::I32 | ScalarType::U32)
            && self.dst == ScalarType::F32
            && self.rounding != RoundingMode::None
            && !self.absolute
            && !self.negate
            && !self.saturate
            && !self.flush_to_zero
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceOp {
    Load,
    Store,
    Reduce,
    Atomic,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceControl {
    pub bound: bool,
    pub handle: u16,
    pub dimension: u8,
    pub component_mask: u8,
    pub cache: u8,
    pub format: u8,
    pub raw_flags: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconvergenceKind {
    Ssy,
    Pbk,
    Pcnt,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShaderOperation {
    Nop,
    /// Explicit host-only diagnostic geometry. Guest instruction lifting must
    /// never emit this operation as a substitute for an untranslated shader.
    SmokeVertexTriangle,
    ConstantI32 {
        dst: u32,
        value: i32,
    },
    ConstantU32 {
        dst: u32,
        value: u32,
    },
    /// Untyped register bits (e.g. Maxwell MOV32I), not a numeric conversion.
    ConstantBits32 {
        dst: u32,
        value: u32,
    },
    ConstantF32 {
        dst: u32,
        value: f32,
    },
    ConstantVec4 {
        dst: u32,
        value: [f32; 4],
    },
    Move {
        dst: u32,
        src: u32,
    },
    Unary {
        dst: u32,
        op: UnaryOp,
        src: u32,
    },
    Binary {
        dst: u32,
        op: BinaryOp,
        left: u32,
        right: u32,
    },
    Compare {
        dst: u32,
        op: CompareOp,
        left: u32,
        right: u32,
    },
    Convert {
        dst: u32,
        src: u32,
        from: ScalarType,
        to: ScalarType,
    },
    ConvertRich {
        dst: u32,
        src: u32,
        control: ConversionControl,
    },
    Select {
        dst: u32,
        condition: u32,
        on_true: u32,
        on_false: u32,
    },
    LoadInput {
        dst: u32,
        location: u32,
        component: u8,
    },
    /// Integer vertex input, extended by fetch to a 32-bit register word.
    /// Signedness is part of the host interface, not an implicit float cast.
    LoadInputInteger {
        dst: u32,
        location: u32,
        component: u8,
        signed: bool,
    },
    /// Consecutive 32-bit registers; stage interface resolution is still required.
    LoadAttributeRich {
        dst: u32,
        address: ShaderAttributeAddress,
        components: u8,
        output: bool,
    },
    StoreAttributeRich {
        src: u32,
        address: ShaderAttributeAddress,
        components: u8,
    },
    /// One floating-point component of a user vertex output.
    StoreVarying {
        location: u32,
        component: u8,
        src: u32,
    },
    /// Load one four-component floating-point input attribute.
    LoadInputVec4 {
        dst: u32,
        location: u32,
    },
    /// Construct a vec4 from four floating-point scalar registers, in XYZW order.
    ComposeVec4 {
        dst: u32,
        components: [u32; 4],
    },
    StoreOutput {
        location: u32,
        component: u8,
        src: u32,
    },
    LoadBuiltin {
        dst: u32,
        builtin: u32,
    },
    /// Native indexed/array vertex number, with wrapping 32-bit register bias.
    /// Guest VertexID policy must be validated before selecting this operation.
    LoadVertexIndex {
        dst: u32,
        bias: u32,
    },
    StoreBuiltin {
        builtin: u32,
        src: u32,
    },
    StoreBuiltinComponent {
        builtin: u32,
        component: u8,
        src: u32,
    },
    LoadConstantBuffer {
        dst: u32,
        slot: u8,
        offset: i32,
        address: Option<u32>,
    },
    /// Canonical uniform word load with an explicit host descriptor mapping.
    /// The value remains raw register bits, not a numerically converted integer.
    LoadUniformWord {
        dst: u32,
        set: u32,
        binding: u32,
        byte_offset: u32,
        byte_size: u32,
    },
    LoadMemory {
        dst: u32,
        space: MemorySpace,
        binding: u32,
        offset: u32,
    },
    StoreMemory {
        space: MemorySpace,
        binding: u32,
        offset: u32,
        src: u32,
    },
    AtomicAdd {
        dst: u32,
        space: MemorySpace,
        binding: u32,
        offset: u32,
        value: u32,
    },
    LoadMemoryRich {
        dst: u32,
        space: MemorySpace,
        address: u32,
        offset: i32,
        width: MemoryAccessWidth,
    },
    StoreMemoryRich {
        space: MemorySpace,
        address: u32,
        offset: i32,
        width: MemoryAccessWidth,
        src: u32,
    },
    Atomic {
        dst: u32,
        op: AtomicOp,
        space: MemorySpace,
        address: u32,
        offset: i32,
        value: u32,
        compare: Option<u32>,
    },
    Ternary {
        dst: u32,
        op: TernaryOp,
        a: u32,
        b: u32,
        c: u32,
    },
    TernaryRich {
        dst: u32,
        op: TernaryOp,
        a: u32,
        b: u32,
        c: u32,
        control: TernaryControl,
    },
    PredicateSet {
        dst: u8,
        dst_secondary: u8,
        domain: PredicateDomain,
        compare: CompareOp,
        combine: PredicateCombine,
        left: u32,
        right: u32,
        input_predicate: u8,
        input_negated: bool,
    },
    PredicatePhi {
        dst: u32,
        predicate: u8,
        incoming: Vec<(u32, u32)>,
    },
    Predicated {
        predicate: u8,
        negated: bool,
        operations: Vec<ShaderOperation>,
    },
    Bitfield {
        dst: u32,
        op: BitfieldOp,
        a: u32,
        b: u32,
        c: Option<u32>,
    },
    BitfieldRich {
        dst: u32,
        op: BitfieldOp,
        a: u32,
        b: Option<u32>,
        c: Option<u32>,
        raw_flags: u32,
    },
    ModifiedUnary {
        dst: u32,
        op: UnaryOp,
        src: u32,
        modifier: ValueModifier,
        float: FloatControl,
    },
    ModifiedBinary {
        dst: u32,
        op: BinaryOp,
        left: u32,
        right: u32,
        left_modifier: ValueModifier,
        right_modifier: ValueModifier,
        float: FloatControl,
    },
    /// Shift a physical word; wrap masks the amount to five bits, otherwise
    /// amounts >= 32 produce zero (or the sign word for arithmetic right).
    ShiftWord32 {
        dst: u32,
        value: u32,
        amount: u32,
        kind: WordShiftKind,
        wrap: bool,
    },
    /// Extract a word field: control[7:0] is the offset, control[15:8] the
    /// width. Zero width yields zero; signed fields extend the source sign
    /// when their range reaches beyond bit 31, including offsets >= 32.
    ExtractWord32 {
        dst: u32,
        value: u32,
        control: u32,
        signed: bool,
    },
    /// Count set bits in the physical word, optionally complementing it first.
    CountBits32 {
        dst: u32,
        value: u32,
        invert: bool,
    },
    /// Find the most significant non-sign bit after optional inversion.
    /// Signed negative words search for zero bits. No matching bit yields
    /// u32::MAX; otherwise return its index or the left shift to bit 31.
    FindLeadingWord32 {
        dst: u32,
        value: u32,
        signed: bool,
        invert: bool,
        shift_amount: bool,
    },
    IntegerBinary {
        dst: u32,
        op: BinaryOp,
        left: u32,
        right: u32,
        left_modifier: ValueModifier,
        right_modifier: ValueModifier,
        control: IntegerControl,
    },
    /// Direct Maxwell TEX, 2D non-array/non-shadow, ordinary or level-zero LOD.
    /// The encoded handle selector is not a resource ID or descriptor index;
    /// draw-time descriptor preparation must resolve it before SPIR-V emission.
    /// Selected source channels are packed consecutively into the dst registers.
    SampleTexture2dHandle {
        dst: Vec<u32>,
        handle_selector: u16,
        coords: [u32; 2],
        components: Vec<u8>,
        level_zero: bool,
    },
    /// Resolved floating-point 2D sampling with separate image/sampler
    /// descriptors. Each descriptor address is [set, binding]; view semantics
    /// and normalized-coordinate policy must be validated by draw preparation.
    SampleTexture2d {
        dst: Vec<u32>,
        image: [u32; 2],
        sampler: [u32; 2],
        coords: [u32; 2],
        components: Vec<TextureSampleChannel>,
        level_zero: bool,
    },
    Texture {
        dst: u32,
        op: TextureOp,
        texture: u32,
        sampler: u32,
        coords: Vec<u32>,
        extra: Vec<u32>,
    },
    TextureRich {
        dst: Vec<u32>,
        op: TextureOp,
        texture: u32,
        sampler: u32,
        coords: Vec<u32>,
        lod: Option<u32>,
        gradients: Vec<u32>,
        offsets: Vec<i32>,
        component_mask: u8,
        arrayed: bool,
        bindless: bool,
    },
    Interpolate {
        dst: u32,
        /// Aligned hardware attribute byte address, not a scalar or slot index.
        attribute: u32,
        component: u8,
        mode: InterpolationMode,
        offset: Option<u32>,
    },
    Subgroup {
        dst: u32,
        predicate: Option<u32>,
        op: SubgroupOp,
        value: u32,
        lane: u32,
        clamp: u32,
    },
    Surface {
        dst: Option<u32>,
        op: SurfaceOp,
        control: SurfaceControl,
        address: Vec<u32>,
        values: Vec<u32>,
        atomic: Option<AtomicOp>,
    },
    Branch {
        target: u32,
    },
    BranchConditional {
        condition: u32,
        on_true: u32,
        on_false: u32,
    },
    BranchIndirect {
        base: u32,
        offset: i32,
    },
    Call {
        target: Option<u32>,
        indirect: Option<(u32, i32)>,
    },
    Label {
        id: u32,
    },
    Phi {
        dst: u32,
        incoming: Vec<(u32, u32)>,
    },
    ReconvergencePush {
        target: u32,
    },
    ReconvergencePushRich {
        kind: ReconvergenceKind,
        target: u32,
    },
    ReconvergencePop,
    Break,
    Continue,
    DependencyBarrier {
        barrier: u8,
        wait_mask: u8,
    },
    Kill,
    Barrier {
        scope: BarrierScope,
    },
    MemoryBarrier {
        scope: BarrierScope,
    },
    Return,
    ReturnValue {
        src: u32,
    },
    /// Preserves Maxwell code until the shader translator understands it safely.
    OpaqueMaxwell {
        raw: u64,
    },
}

#[derive(Debug, Clone)]
pub struct ShaderModule {
    pub id: u64,
    pub stage: ShaderStage,
    pub variant_key: Vec<u8>,
    pub operations: Vec<ShaderOperation>,
}
