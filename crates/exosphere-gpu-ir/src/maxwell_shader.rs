// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Structural and conservative semantic Maxwell SM5x shader parser.
//!
//! Opcode-family recognition follows the public Maxwell opcode tables used by the Switch emulator
//! ecosystem.  Recognition is deliberately separated from operand lifting: an instruction may be
//! classified without being promoted to canonical semantics until every operand/predicate bit is
//! validated.  This makes coverage measurable without silently miscompiling guest shaders.

use crate::{ShaderModule, ShaderOperation, ShaderStage};
use std::{collections::BTreeMap, fmt};

pub const GRAPHICS_HEADER_SIZE: usize = 0x50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaxwellShaderHeader {
    pub raw: [u8; GRAPHICS_HEADER_SIZE],
}

/// Fragment export metadata; this does not assign physical source registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxwellFragmentOutputs {
    pub color_components: u32,
    /// Retained intact so unsupported flags are never discarded by consumers.
    pub control: u32,
}

impl MaxwellFragmentOutputs {
    pub fn color_mask(self, target: usize) -> Option<u8> {
        (target < 8).then(|| ((self.color_components >> (target * 4)) & 15) as u8)
    }

    pub fn writes_sample_mask(self) -> bool {
        self.control & 1 != 0
    }

    pub fn writes_depth(self) -> bool {
        self.control & 2 != 0
    }
}

impl MaxwellShaderHeader {
    /// Interprets fragment-only fields. Callers must establish the shader stage;
    /// identical header words have different meanings in other graphics stages.
    pub fn fragment_outputs(&self, stage: ShaderStage) -> Option<MaxwellFragmentOutputs> {
        if stage != ShaderStage::Fragment {
            return None;
        }
        Some(MaxwellFragmentOutputs {
            color_components: u32::from_le_bytes(self.raw[72..76].try_into().unwrap()),
            control: u32::from_le_bytes(self.raw[76..80].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxwellInstruction {
    pub offset: u32,
    pub raw: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MaxwellOpcode {
    Nop,
    Exit,
    Kill,
    Bra,
    Jmp,
    Brx,
    Call,
    Return,
    Mov,
    FAdd,
    FMul,
    IAdd,
    IMul,
    ShiftLeft,
    ShiftRight,
    Logic,
    LoadGlobal,
    StoreGlobal,
    LoadAttribute,
    StoreAttribute,
    Tex,
    Tld,
    Tld4,
    Txd,
    Txq,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MaxwellOpcodeFamily {
    Nop,
    Exit,
    Kill,
    Branch,
    CallReturn,
    Synchronize,
    FloatAlu,
    DoubleAlu,
    HalfAlu,
    IntegerAlu,
    Logic,
    CompareConvert,
    Load,
    Store,
    Atomic,
    Texture,
    Interpolation,
    SpecialRegister,
    Barrier,
    Surface,
    Vector,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxwellPredicateGuard {
    pub index: u8,
    pub negated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxwellCommonOperands {
    pub dst: u8,
    pub src_a: u8,
    pub src_b: u8,
    pub src_c: u8,
    pub predicate: Option<MaxwellPredicateGuard>,
}

/// Structural attribute-memory operands, not a certified Vulkan interface.
/// Raw instruction bits remain in MaxwellDecodedInstruction for unsupported modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxwellAttributeAccess {
    pub store: bool,
    pub register: u8,
    pub address_register: u8,
    pub address_offset: u16,
    pub vertex_register: u8,
    pub components: u8,
    pub patch_flag: bool,
    pub output_flag: bool,
}

pub fn decode_attribute_access(raw: u64) -> Option<MaxwellAttributeAccess> {
    // GM107 attribute LD/ST and AMEM fields: envytools/envydis/gm107.c.
    let opcode = (raw >> 48) as u16 & 0xfff8;
    let store = match opcode {
        0xefd8 => false,
        0xeff0 => true,
        _ => return None,
    };
    Some(MaxwellAttributeAccess {
        store,
        register: field(raw, 0, 8) as u8,
        address_register: field(raw, 8, 8) as u8,
        address_offset: field(raw, 20, 10) as u16,
        vertex_register: field(raw, 39, 8) as u8,
        components: field(raw, 47, 2) as u8 + 1,
        patch_flag: field(raw, 31, 1) != 0,
        output_flag: field(raw, 32, 1) != 0,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxwellScheduleEntry {
    pub stall: u8,
    pub yield_hint: bool,
    pub write_barrier: Option<u8>,
    pub read_barrier: Option<u8>,
    pub wait_mask: u8,
    pub reuse_mask: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxwellDecodedInstruction {
    pub instruction: MaxwellInstruction,
    pub family: MaxwellOpcodeFamily,
    /// High sixteen instruction bits.  Maxwell's published opcode tables are expressed in this
    /// form, with low bits of this field sometimes marked don't-care.
    pub opcode16: u16,
    pub opcode: MaxwellOpcode,
    pub common: MaxwellCommonOperands,
    /// PC-relative target for control-flow encodings using GM107 BTARG (signed 24-bit @ bit 20, PC+8).
    pub branch_target: Option<u32>,
    pub attribute_access: Option<MaxwellAttributeAccess>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaxwellBundle {
    pub scheduler_offset: u32,
    pub scheduler: u64,
    pub instructions: Vec<MaxwellInstruction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaxwellProgram {
    pub stage: ShaderStage,
    pub header: Option<MaxwellShaderHeader>,
    pub bundles: Vec<MaxwellBundle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaxwellShaderError {
    TooSmall,
    MisalignedCode,
}
impl fmt::Display for MaxwellShaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall => write!(f, "Maxwell shader is smaller than its graphics header"),
            Self::MisalignedCode => write!(f, "Maxwell shader code is not aligned to 64-bit words"),
        }
    }
}
impl std::error::Error for MaxwellShaderError {}

pub fn parse_graphics(
    stage: ShaderStage,
    bytes: &[u8],
) -> Result<MaxwellProgram, MaxwellShaderError> {
    if bytes.len() < GRAPHICS_HEADER_SIZE {
        return Err(MaxwellShaderError::TooSmall);
    }
    let header = MaxwellShaderHeader {
        raw: bytes[..GRAPHICS_HEADER_SIZE]
            .try_into()
            .expect("checked header size"),
    };
    parse_code(
        stage,
        Some(header),
        &bytes[GRAPHICS_HEADER_SIZE..],
        GRAPHICS_HEADER_SIZE as u32,
    )
}

pub fn parse_compute(bytes: &[u8]) -> Result<MaxwellProgram, MaxwellShaderError> {
    parse_code(ShaderStage::Compute, None, bytes, 0)
}

fn parse_code(
    stage: ShaderStage,
    header: Option<MaxwellShaderHeader>,
    code: &[u8],
    base: u32,
) -> Result<MaxwellProgram, MaxwellShaderError> {
    if code.len() % 8 != 0 {
        return Err(MaxwellShaderError::MisalignedCode);
    }
    let words: Vec<u64> = code
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("word")))
        .collect();
    let mut bundles = Vec::new();
    for (bundle_index, group) in words.chunks(4).enumerate() {
        if group.is_empty() {
            continue;
        }
        let scheduler_offset = base + (bundle_index as u32) * 32;
        let instructions = group
            .iter()
            .enumerate()
            .skip(1)
            .map(|(index, raw)| MaxwellInstruction {
                offset: scheduler_offset + (index as u32) * 8,
                raw: *raw,
            })
            .collect();
        bundles.push(MaxwellBundle {
            scheduler_offset,
            scheduler: group[0],
            instructions,
        });
    }
    Ok(MaxwellProgram {
        stage,
        header,
        bundles,
    })
}

fn masked(op: u16, value: u16, mask: u16) -> bool {
    op & mask == value & mask
}
fn field(raw: u64, shift: u32, width: u32) -> u64 {
    (raw >> shift) & ((1u64 << width) - 1)
}
fn sign_extend(value: u64, width: u32) -> i64 {
    ((value << (64 - width)) as i64) >> (64 - width)
}

pub fn decode_predicate_guard(raw: u64) -> Option<MaxwellPredicateGuard> {
    let index = field(raw, 16, 3) as u8;
    let negated = field(raw, 19, 1) != 0;
    if index == 7 && !negated {
        None
    } else {
        Some(MaxwellPredicateGuard { index, negated })
    }
}

pub fn decode_common_operands(raw: u64) -> MaxwellCommonOperands {
    MaxwellCommonOperands {
        dst: field(raw, 0, 8) as u8,
        src_a: field(raw, 8, 8) as u8,
        src_b: field(raw, 20, 8) as u8,
        src_c: field(raw, 39, 8) as u8,
        predicate: decode_predicate_guard(raw),
    }
}

pub fn decode_scheduler_word(raw: u64) -> [MaxwellScheduleEntry; 3] {
    core::array::from_fn(|n| {
        let b = (n * 21) as u32;
        let wr = field(raw, b + 5, 3) as u8;
        let rd = field(raw, b + 8, 3) as u8;
        MaxwellScheduleEntry {
            stall: field(raw, b, 4) as u8,
            yield_hint: field(raw, b + 4, 1) != 0,
            write_barrier: (wr != 7).then_some(wr),
            read_barrier: (rd != 7).then_some(rd),
            wait_mask: field(raw, b + 11, 6) as u8,
            reuse_mask: field(raw, b + 17, 4) as u8,
        }
    })
}

pub fn decode_branch_target(instruction: MaxwellInstruction) -> u32 {
    let rel = sign_extend(field(instruction.raw, 20, 24), 24);
    ((instruction.offset as i64) + 8 + rel) as u32
}

pub fn decode_opcode(raw: u64) -> MaxwellOpcode {
    let op = (raw >> 48) as u16;
    if raw == 0 || masked(op, 0x50b0, 0xfff8) {
        MaxwellOpcode::Nop
    } else if masked(op, 0xe300, 0xfff0) {
        MaxwellOpcode::Exit
    } else if masked(op, 0xe330, 0xfff0) {
        MaxwellOpcode::Kill
    } else if masked(op, 0xe240, 0xfff0) {
        MaxwellOpcode::Bra
    } else if masked(op, 0xe210, 0xfff0) {
        MaxwellOpcode::Jmp
    } else if masked(op, 0xe250, 0xfff0) {
        MaxwellOpcode::Brx
    } else if masked(op, 0xe260, 0xfff0) || masked(op, 0xe220, 0xfff0) {
        MaxwellOpcode::Call
    } else if masked(op, 0xe320, 0xfff0) || masked(op, 0xe270, 0xfff0) {
        MaxwellOpcode::Return
    } else if masked(op, 0x5c98, 0xfff8) {
        MaxwellOpcode::Mov
    } else if masked(op, 0x5c58, 0xfff8) {
        MaxwellOpcode::FAdd
    } else if masked(op, 0x5c68, 0xfff8) {
        MaxwellOpcode::FMul
    } else if masked(op, 0x5c10, 0xfff8) {
        MaxwellOpcode::IAdd
    } else if masked(op, 0x5c38, 0xfff8) {
        MaxwellOpcode::IMul
    } else if masked(op, 0x5c48, 0xfff8) {
        MaxwellOpcode::ShiftLeft
    } else if masked(op, 0x5c28, 0xfff8) {
        MaxwellOpcode::ShiftRight
    } else if masked(op, 0x5c40, 0xfff8) {
        MaxwellOpcode::Logic
    } else if masked(op, 0xefd8, 0xfff8) {
        MaxwellOpcode::LoadAttribute
    } else if masked(op, 0xeff0, 0xfff8) {
        MaxwellOpcode::StoreAttribute
    } else if raw >> 61 == 0b100 {
        MaxwellOpcode::LoadGlobal
    } else if raw >> 61 == 0b101 {
        MaxwellOpcode::StoreGlobal
    } else if masked(op, 0xc038, 0xfc38) || masked(op, 0xdeb8, 0xfff8) || masked(op, 0xd000, 0xf600)
    {
        MaxwellOpcode::Tex
    } else if masked(op, 0xdc38, 0xff38) || masked(op, 0xdd38, 0xff38) || masked(op, 0xd200, 0xf600)
    {
        MaxwellOpcode::Tld
    } else if masked(op, 0xc838, 0xfc38) || masked(op, 0xdef8, 0xfff8) {
        MaxwellOpcode::Tld4
    } else if masked(op, 0xde38, 0xfffc) || masked(op, 0xde78, 0xfffc) {
        MaxwellOpcode::Txd
    } else if masked(op, 0xdf48, 0xfff8) || masked(op, 0xdf50, 0xfff8) {
        MaxwellOpcode::Txq
    } else {
        MaxwellOpcode::Unknown
    }
}

/// Classify an SM5x instruction by its documented opcode family.
///
/// This does not decode operands and therefore is safe to use for coverage/dispatch even when a
/// family has not yet acquired a canonical semantic lifter.
pub fn classify_instruction(instruction: MaxwellInstruction) -> MaxwellDecodedInstruction {
    let op = (instruction.raw >> 48) as u16;
    let family = if instruction.raw == 0 || masked(op, 0x50b0, 0xfff8) {
        MaxwellOpcodeFamily::Nop
    } else if masked(op, 0xe300, 0xfff0) {
        MaxwellOpcodeFamily::Exit
    } else if masked(op, 0xe330, 0xfff0) {
        MaxwellOpcodeFamily::Kill
    } else if masked(op, 0xe240, 0xfff0)
        || masked(op, 0xe250, 0xfff0)
        || masked(op, 0xe210, 0xfff0)
        || masked(op, 0xe200, 0xfff0)
    {
        MaxwellOpcodeFamily::Branch
    } else if masked(op, 0xe220, 0xfff0)
        || masked(op, 0xe260, 0xfff0)
        || masked(op, 0xe270, 0xfff0)
        || masked(op, 0xe310, 0xfff0)
        || masked(op, 0xe320, 0xfff0)
    {
        MaxwellOpcodeFamily::CallReturn
    } else if masked(op, 0xe350, 0xfff0)
        || masked(op, 0xe340, 0xfff0)
        || masked(op, 0xe2a0, 0xfff0)
        || masked(op, 0xe2b0, 0xfff0)
    {
        MaxwellOpcodeFamily::Synchronize
    } else if masked(op, 0xf0a8, 0xfff8) || masked(op, 0xf0f0, 0xfff8) || masked(op, 0xef98, 0xfff8)
    {
        MaxwellOpcodeFamily::Barrier
    } else if masked(op, 0x5c58, 0xfff8)
        || masked(op, 0x4c58, 0xfff8)
        || masked(op, 0x5c68, 0xfff8)
        || masked(op, 0x4c68, 0xfff8)
        || masked(op, 0x5b80, 0xff00)
        || masked(op, 0x5bb0, 0xfff0)
        || masked(op, 0x5b90, 0xff00)
    {
        MaxwellOpcodeFamily::FloatAlu
    } else if masked(op, 0x5c70, 0xfff8)
        || masked(op, 0x4c70, 0xfff8)
        || masked(op, 0x5b70, 0xfff0)
        || masked(op, 0x5c80, 0xfff8)
        || masked(op, 0x4c80, 0xfff8)
    {
        MaxwellOpcodeFamily::DoubleAlu
    } else if masked(op, 0x5d00, 0xfff0)
        || masked(op, 0x5d10, 0xfff0)
        || masked(op, 0x5d20, 0xfff0)
        || masked(op, 0x7800, 0xf800)
        || masked(op, 0x6800, 0xf800)
    {
        MaxwellOpcodeFamily::HalfAlu
    } else if masked(op, 0x5c10, 0xfff0)
        || masked(op, 0x4c10, 0xfff0)
        || masked(op, 0x5c20, 0xfff8)
        || masked(op, 0x4c20, 0xfff8)
        || masked(op, 0x5c38, 0xfff8)
        || masked(op, 0x4c38, 0xfff8)
        || masked(op, 0x5a00, 0xfe00)
        || masked(op, 0x5200, 0xfe00)
    {
        MaxwellOpcodeFamily::IntegerAlu
    } else if masked(op, 0x5c40, 0xfff8)
        || masked(op, 0x4c40, 0xfff8)
        || masked(op, 0x5be0, 0xfff8)
        || masked(op, 0x5bf0, 0xfff8)
    {
        MaxwellOpcodeFamily::Logic
    } else if masked(op, 0x5800, 0xff00)
        || masked(op, 0x5b50, 0xfff0)
        || masked(op, 0x5b60, 0xfff0)
        || masked(op, 0x5ca8, 0xfff8)
        || masked(op, 0x5cb0, 0xfff8)
        || masked(op, 0x5cb8, 0xfff8)
        || masked(op, 0x5ce0, 0xfff8)
    {
        MaxwellOpcodeFamily::CompareConvert
    } else if masked(op, 0xeff0, 0xfff8) {
        MaxwellOpcodeFamily::Store
    } else if masked(op, 0xe000, 0xff00) {
        // IPA also has its high bit set; classify it before broad load forms.
        MaxwellOpcodeFamily::Interpolation
    } else if masked(op, 0xd000, 0xf000) || masked(op, 0xc000, 0xf000) {
        // Texture instructions also have the high load bit set. Keep their
        // family visible so the semantic decoder can validate exact forms.
        MaxwellOpcodeFamily::Texture
    } else if masked(op, 0x8000, 0x8000)
        || masked(op, 0xeed0, 0xfff8)
        || masked(op, 0xef90, 0xfff8)
        || masked(op, 0xef40, 0xfff8)
        || masked(op, 0xef48, 0xfff8)
    {
        MaxwellOpcodeFamily::Load
    } else if masked(op, 0xa000, 0xe000) || masked(op, 0xef50, 0xfff0) {
        MaxwellOpcodeFamily::Store
    } else if masked(op, 0xed00, 0xff00) || masked(op, 0xee00, 0xfe00) || masked(op, 0xebf8, 0xfff8)
    {
        MaxwellOpcodeFamily::Atomic
    } else if masked(op, 0xef10, 0xfff8) {
        MaxwellOpcodeFamily::Vector
    } else if masked(op, 0x50c8, 0xfff8) || masked(op, 0x50d0, 0xfff8) {
        MaxwellOpcodeFamily::SpecialRegister
    } else if masked(op, 0xef00, 0xff00) || masked(op, 0xeb00, 0xff00) {
        MaxwellOpcodeFamily::Surface
    } else if masked(op, 0x5000, 0xf000) {
        MaxwellOpcodeFamily::Vector
    } else {
        MaxwellOpcodeFamily::Unknown
    };
    let branch_target = matches!(
        family,
        MaxwellOpcodeFamily::Branch | MaxwellOpcodeFamily::CallReturn
    )
    .then(|| decode_branch_target(instruction));
    MaxwellDecodedInstruction {
        instruction,
        family,
        opcode16: op,
        opcode: decode_opcode(instruction.raw),
        common: decode_common_operands(instruction.raw),
        branch_target,
        attribute_access: decode_attribute_access(instruction.raw),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpcodeCoverage {
    pub total: usize,
    pub families: BTreeMap<MaxwellOpcodeFamily, usize>,
}
impl OpcodeCoverage {
    pub fn recognized(&self) -> usize {
        self.total.saturating_sub(
            *self
                .families
                .get(&MaxwellOpcodeFamily::Unknown)
                .unwrap_or(&0),
        )
    }
}

pub fn opcode_coverage(program: &MaxwellProgram) -> OpcodeCoverage {
    let mut out = OpcodeCoverage::default();
    for bundle in &program.bundles {
        for &instruction in &bundle.instructions {
            out.total += 1;
            *out.families
                .entry(classify_instruction(instruction).family)
                .or_default() += 1;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiftStats {
    pub total: usize,
    pub semantic: usize,
    pub classified: usize,
    pub opaque: usize,
}

/// Semantic lifting stays conservative.  Opcode recognition is counted separately from promotion.
pub fn lift_semantic(id: u64, program: &MaxwellProgram) -> (ShaderModule, LiftStats) {
    // One semantic source of truth: coverage and compilation both consume decode_semantic() and
    // lower_semantic(). CFG/SSA remains available for future structurisation, but no parallel
    // opcode decoder is allowed to claim semantics independently.
    let mut operations = Vec::new();
    let mut stats = LiftStats::default();
    for instruction in program
        .bundles
        .iter()
        .flat_map(|b| b.instructions.iter().copied())
    {
        stats.total += 1;
        let decoded = crate::maxwell_semantics::decode_semantic(instruction);
        if decoded.opcode.is_some() {
            stats.classified += 1;
        }
        let lowered = crate::maxwell_semantics::lower_semantic(&decoded);
        // Compute has no graphics attribute interface. Keep invalid cross-stage
        // instructions opaque even when their operand encoding is understood.
        let invalid_attribute_stage = program.stage == ShaderStage::Compute
            && matches!(
                decoded.opcode,
                Some(
                    crate::maxwell_semantics::MaxwellSemanticOpcode::LoadAttribute
                        | crate::maxwell_semantics::MaxwellSemanticOpcode::StoreAttribute
                )
            );
        if lowered.lossless && !invalid_attribute_stage {
            stats.semantic += 1;
            operations.extend(lowered.operations);
        } else {
            stats.opaque += 1;
            operations.push(ShaderOperation::OpaqueMaxwell {
                raw: instruction.raw,
            });
        }
    }
    if !matches!(
        operations.last(),
        Some(ShaderOperation::Return | ShaderOperation::Kill)
    ) {
        operations.push(ShaderOperation::Return);
    }
    (
        ShaderModule {
            id,
            stage: program.stage,
            variant_key: Vec::new(),
            operations,
        },
        stats,
    )
}

pub fn lift_opaque(id: u64, program: &MaxwellProgram) -> ShaderModule {
    let mut module = lift_semantic(id, program).0;
    for operation in &mut module.operations {
        if matches!(operation, ShaderOperation::Nop) {
            *operation = ShaderOperation::OpaqueMaxwell { raw: 0 };
        }
    }
    module
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_output_metadata_preserves_masks_and_control() {
        let mut header = MaxwellShaderHeader {
            raw: [0; GRAPHICS_HEADER_SIZE],
        };
        header.raw[72..76].copy_from_slice(&0x87654321u32.to_le_bytes());
        for control in [0u32, 1, 2, 3, 0x80000003] {
            header.raw[76..80].copy_from_slice(&control.to_le_bytes());
            let outputs = header.fragment_outputs(ShaderStage::Fragment).unwrap();
            assert_eq!(outputs.control, control);
            assert_eq!(outputs.writes_sample_mask(), control & 1 != 0);
            assert_eq!(outputs.writes_depth(), control & 2 != 0);
            for target in 0..8 {
                assert_eq!(outputs.color_mask(target), Some(target as u8 + 1));
            }
            assert_eq!(outputs.color_mask(8), None);
            assert_eq!(outputs.color_mask(usize::MAX), None);
        }
        for stage in [
            ShaderStage::Vertex,
            ShaderStage::Geometry,
            ShaderStage::Compute,
        ] {
            assert_eq!(header.fragment_outputs(stage), None);
        }
        header.raw[72..76].fill(0);
        assert_eq!(
            header
                .fragment_outputs(ShaderStage::Fragment)
                .unwrap()
                .color_mask(0),
            Some(0)
        );
    }
    #[test]
    fn attribute_operands_retain_width_address_modes_and_raw_encoding() {
        for (base, store) in [
            (0xefd8_0000_0000_0000u64, false),
            (0xeff0_0000_0000_0000, true),
        ] {
            for width in 0..4u64 {
                let raw = base
                    | (width << 47)
                    | (17 << 39)
                    | (1 << 32)
                    | (1 << 31)
                    | (0x3fcu64 << 20)
                    | (7 << 16)
                    | (255 << 8)
                    | 9;
                let decoded = classify_instruction(MaxwellInstruction { offset: 0x58, raw });
                assert_eq!(decoded.instruction.raw, raw);
                assert_eq!(
                    decoded.attribute_access,
                    Some(MaxwellAttributeAccess {
                        store,
                        register: 9,
                        address_register: 255,
                        address_offset: 0x3fc,
                        vertex_register: 17,
                        components: width as u8 + 1,
                        patch_flag: true,
                        output_flag: true,
                    })
                );
                assert_eq!(
                    decoded.family,
                    if store {
                        MaxwellOpcodeFamily::Store
                    } else {
                        MaxwellOpcodeFamily::Load
                    }
                );
                let bytes = [0u64, raw]
                    .into_iter()
                    .flat_map(u64::to_le_bytes)
                    .collect::<Vec<_>>();
                let program = parse_compute(&bytes).unwrap();
                let (_, stats) = lift_semantic(1, &program);
                assert_eq!(stats.opaque, 1);
                assert_eq!(stats.semantic, 0);
            }
        }
        assert!(decode_attribute_access(0xeed0_0000_0000_0000).is_none());
    }
    #[test]
    fn separates_scheduler_words() {
        let mut bytes = vec![0u8; GRAPHICS_HEADER_SIZE];
        for value in 1u64..=8 {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let p = parse_graphics(ShaderStage::Vertex, &bytes).unwrap();
        assert_eq!(p.bundles.len(), 2);
        assert_eq!(p.bundles[0].scheduler, 1);
    }
    #[test]
    fn semantic_lift_only_claims_zero_padding() {
        let mut bytes = vec![0u8; GRAPHICS_HEADER_SIZE];
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&0x1234u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        let p = parse_graphics(ShaderStage::Vertex, &bytes).unwrap();
        let (_, s) = lift_semantic(1, &p);
        assert_eq!((s.total, s.semantic, s.opaque), (3, 2, 1));
    }
    #[test]
    fn recognizes_documented_control_flow_families() {
        let bra = MaxwellInstruction {
            offset: 0,
            raw: (0xe240u64) << 48,
        };
        let exit = MaxwellInstruction {
            offset: 8,
            raw: (0xe300u64) << 48,
        };
        assert_eq!(
            classify_instruction(bra).family,
            MaxwellOpcodeFamily::Branch
        );
        assert_eq!(classify_instruction(exit).family, MaxwellOpcodeFamily::Exit);
    }
    #[test]
    fn decodes_exact_common_opcodes() {
        assert_eq!(decode_opcode((0x5c58u64) << 48), MaxwellOpcode::FAdd);
        assert_eq!(decode_opcode((0x5c10u64) << 48), MaxwellOpcode::IAdd);
        assert_eq!(decode_opcode((0xe240u64) << 48), MaxwellOpcode::Bra);
    }
    #[test]
    fn decodes_common_register_fields() {
        let raw = 3u64 | (4u64 << 8) | (5u64 << 20) | (6u64 << 39) | (7u64 << 16);
        let o = decode_common_operands(raw);
        assert_eq!((o.dst, o.src_a, o.src_b, o.src_c), (3, 4, 5, 6));
        assert_eq!(o.predicate, None);
    }
    #[test]
    fn decodes_predicate_guard() {
        let g = decode_predicate_guard((2u64 << 16) | (1u64 << 19)).unwrap();
        assert_eq!(
            g,
            MaxwellPredicateGuard {
                index: 2,
                negated: true
            }
        );
    }
    #[test]
    fn decodes_scheduler_fields() {
        let raw = 3u64 | (1u64 << 4) | (2u64 << 5) | (4u64 << 8) | (0x15u64 << 11) | (9u64 << 17);
        let s = decode_scheduler_word(raw)[0];
        assert_eq!(s.stall, 3);
        assert!(s.yield_hint);
        assert_eq!(s.write_barrier, Some(2));
        assert_eq!(s.read_barrier, Some(4));
        assert_eq!(s.wait_mask, 0x15);
        assert_eq!(s.reuse_mask, 9);
    }
    #[test]
    fn branch_target_is_signed_pc_relative() {
        let i = MaxwellInstruction {
            offset: 0x100,
            raw: (4u64 << 20),
        };
        assert_eq!(decode_branch_target(i), 0x10c);
    }
}
