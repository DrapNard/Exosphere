// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Coverage-driven GM107/SM53 semantic decoding.
//!
//! This layer deliberately sits beside the CFG/SSA lifter: it decodes operand *forms* and
//! long-tail opcodes without pretending the canonical Shader IR can already express every
//! modifier.  A decoded instruction can therefore be `Ready`, `NeedsCanonicalIr`, or
//! `UnknownEncoding`, which is considerably more useful than a single opaque counter.

use crate::maxwell_shader::{
    MaxwellInstruction, MaxwellOpcodeFamily, MaxwellPredicateGuard, MaxwellProgram,
    classify_instruction,
};
use std::collections::BTreeMap;

fn bits(raw: u64, shift: u32, width: u32) -> u64 {
    if width == 64 {
        raw
    } else {
        (raw >> shift) & ((1u64 << width) - 1)
    }
}
fn sext(value: u64, width: u32) -> i64 {
    ((value << (64 - width)) as i64) >> (64 - width)
}

fn valid_mov32i_encoding(raw: u64) -> bool {
    // Only a full lane mask is equivalent to replacing a canonical register.
    // Destination, predicate and immediate fields are otherwise unrestricted.
    let variable = 0xffu64 | (0xff << 12) | (0xffffffff << 20);
    raw & !variable == 0x0100000000000000 && bits(raw, 12, 4) == 0xf
}

fn valid_exit_encoding(raw: u64) -> bool {
    // Predicate guards are represented separately. Condition-code tests need
    // condition state not yet present in the canonical termination operation.
    raw & !(15 << 16) == 0xe30000000000000f
}

pub(crate) fn valid_direct_branch_encoding(raw: u64) -> bool {
    // Only ordinary PC-relative BRA with the always condition is represented.
    // Other condition-code tests and control modifiers need additional IR state.
    let variable = (0xffffffu64 << 20) | (15 << 16);
    raw & !variable == 0xe24000000000000f
}

fn valid_register_mov_encoding(raw: u64) -> bool {
    // Ordinary GPR MOV has its lane mask at bits 39..42, unlike MOV32I.
    // R255 requires explicit zero/discard semantics rather than a normal IR id.
    let variable = 0xffu64 | (15 << 16) | (0xff << 20) | (15 << 39);
    raw & !variable == 0x5c98000000000000
        && bits(raw, 39, 4) == 15
        && bits(raw, 0, 8) != 255
        && bits(raw, 20, 8) != 255
}

fn valid_short_mov_encoding(raw: u64) -> bool {
    let variable = 0xffu64 | (15 << 16) | (0x7ffff << 20) | (15 << 39) | (1 << 56);
    raw & !variable == 0x3898000000000000 && bits(raw, 39, 4) == 15 && bits(raw, 0, 8) != 255
}

fn valid_constant_mov_encoding(raw: u64) -> bool {
    let variable = 0xffu64 | (15 << 16) | (0x7ffff << 20) | (15 << 39);
    raw & !variable == 0x4c98000000000000 && bits(raw, 39, 4) == 15 && bits(raw, 0, 8) != 255
}

fn direct_ldc_words(raw: u64) -> Option<u32> {
    let words = match bits(raw, 48, 3) {
        4 => 1,
        5 => 2,
        6 => 4,
        _ => return None,
    };
    let variable = 0xffu64 | (15 << 16) | (0xffff << 20) | (31 << 36) | (7 << 48);
    // Default mode, RZ base and aligned nonnegative byte offsets. Wide loads
    // occupy consecutive raw registers; never reinterpret them as host doubles.
    let dst = bits(raw, 0, 8) as u32;
    let offset = bits(raw, 20, 16) as u32;
    (raw & !variable == (0xef90000000000000 | (255 << 8))
        && dst % words == 0
        && dst + words <= 255
        && offset & 0x8000 == 0
        && offset % (words * 4) == 0)
        .then_some(words)
}

fn short_mov_bits(raw: u64) -> u32 {
    // The sign bit is separated from the low 19 immediate bits by the opcode.
    sext(short_immediate20(raw), 20) as u32
}

fn short_immediate20(raw: u64) -> u64 {
    bits(raw, 20, 19) | (bits(raw, 56, 1) << 19)
}

fn ordinary_float_controls(
    raw: u64,
    opcode: MaxwellSemanticOpcode,
) -> Option<(
    crate::shader::ValueModifier,
    crate::shader::ValueModifier,
    crate::shader::FloatControl,
)> {
    use crate::shader::{FloatControl, ValueModifier};
    let plain = ValueModifier {
        negate: false,
        absolute: false,
        invert: false,
    };
    let encoding = (raw >> 48) as u16 & 0xfef8;
    let (base, source_bits, register_source) = match (opcode, encoding) {
        (MaxwellSemanticOpcode::FAdd, 0x5c58) => (0x5c58000000000000, 0xff << 20, true),
        (MaxwellSemanticOpcode::FAdd, 0x4c58) => (0x4c58000000000000, 0x7ffff << 20, false),
        (MaxwellSemanticOpcode::FAdd, 0x3858) => {
            (0x3858000000000000, (0x7ffff << 20) | (1 << 56), false)
        }
        (MaxwellSemanticOpcode::FMul, 0x5c68) => (0x5c68000000000000, 0xff << 20, true),
        (MaxwellSemanticOpcode::FMul, 0x4c68) => (0x4c68000000000000, 0x7ffff << 20, false),
        (MaxwellSemanticOpcode::FMul, 0x3868) => {
            (0x3868000000000000, (0x7ffff << 20) | (1 << 56), false)
        }
        _ => return None,
    };
    if bits(raw, 0, 8) == 255
        || bits(raw, 8, 8) == 255
        || (register_source && bits(raw, 20, 8) == 255)
    {
        return None;
    }
    let operands = 0xfffffu64 | source_bits;
    let modifiers = if opcode == MaxwellSemanticOpcode::FAdd {
        (1 << 44) | (1 << 45) | (1 << 46) | (1 << 48) | (1 << 49) | (1 << 50)
    } else {
        // Only default rounding/scaling and FTZ are representable here. DNZ,
        // nondefault rounding, postmultiply scaling and condition output reject.
        (1 << 44) | (1 << 48) | (1 << 50)
    };
    if raw & !(operands | modifiers) != base {
        return None;
    }
    Some((
        ValueModifier {
            negate: bits(raw, 48, 1) != 0,
            absolute: opcode == MaxwellSemanticOpcode::FAdd && bits(raw, 46, 1) != 0,
            ..plain
        },
        ValueModifier {
            negate: opcode == MaxwellSemanticOpcode::FAdd && bits(raw, 45, 1) != 0,
            absolute: opcode == MaxwellSemanticOpcode::FAdd && bits(raw, 49, 1) != 0,
            ..plain
        },
        FloatControl {
            saturate: bits(raw, 50, 1) != 0,
            flush_to_zero: bits(raw, 44, 1) != 0,
        },
    ))
}

fn float32_immediate_controls(
    raw: u64,
    opcode: MaxwellSemanticOpcode,
) -> Option<(
    crate::shader::ValueModifier,
    crate::shader::ValueModifier,
    crate::shader::FloatControl,
)> {
    use crate::shader::{FloatControl, ValueModifier};
    let plain = ValueModifier {
        negate: false,
        absolute: false,
        invert: false,
    };
    let operands = 0xffffu64 | (15 << 16) | (0xffffffff << 20);
    if bits(raw, 0, 8) == 255 || bits(raw, 8, 8) == 255 {
        return None;
    }
    match opcode {
        MaxwellSemanticOpcode::FAdd32I => {
            // Full immediates move the modifiers above bit 51. Bit 52 writes
            // condition codes, which this IR operation cannot represent.
            if raw & !(operands | (31 << 53)) != 0x0800000000000000 {
                return None;
            }
            Some((
                ValueModifier {
                    negate: bits(raw, 56, 1) != 0,
                    absolute: bits(raw, 54, 1) != 0,
                    ..plain
                },
                ValueModifier {
                    negate: bits(raw, 53, 1) != 0,
                    absolute: bits(raw, 57, 1) != 0,
                    ..plain
                },
                FloatControl {
                    saturate: false,
                    flush_to_zero: bits(raw, 55, 1) != 0,
                },
            ))
        }
        MaxwellSemanticOpcode::FMul32I => {
            // The emitter incorporates the combined negation into the immediate
            // sign bit. DNZ (54) and condition-code output (52) stay unsupported.
            if raw & !(operands | (1 << 53) | (1 << 55)) != 0x1e00000000000000 {
                return None;
            }
            Some((
                plain,
                plain,
                FloatControl {
                    saturate: bits(raw, 55, 1) != 0,
                    flush_to_zero: bits(raw, 53, 1) != 0,
                },
            ))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MaxwellOperandForm {
    Register,
    Immediate20,
    Immediate32,
    ConstantBuffer,
    Memory,
    Predicate,
    TextureHandle,
    SpecialRegister,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxwellOperand {
    Register(u8),
    ImmediateI32(i32),
    ImmediateU32(u32),
    /// Exact IEEE-754 bits, not a numeric conversion from an integer operand.
    ImmediateF32Bits20(u32),
    ImmediateF32Bits32(u32),
    ConstantBuffer {
        slot: u8,
        offset: i32,
        address: Option<u8>,
    },
    Memory {
        base: u8,
        offset: i32,
    },
    Predicate(MaxwellPredicateGuard),
    TextureHandle(u16),
    SpecialRegister(u8),
}
impl MaxwellOperand {
    pub fn form(self) -> MaxwellOperandForm {
        match self {
            Self::Register(_) => MaxwellOperandForm::Register,
            Self::ImmediateI32(_) => MaxwellOperandForm::Immediate20,
            Self::ImmediateU32(_) => MaxwellOperandForm::Immediate32,
            Self::ImmediateF32Bits20(_) => MaxwellOperandForm::Immediate20,
            Self::ImmediateF32Bits32(_) => MaxwellOperandForm::Immediate32,
            Self::ConstantBuffer { .. } => MaxwellOperandForm::ConstantBuffer,
            Self::Memory { .. } => MaxwellOperandForm::Memory,
            Self::Predicate(_) => MaxwellOperandForm::Predicate,
            Self::TextureHandle(_) => MaxwellOperandForm::TextureHandle,
            Self::SpecialRegister(_) => MaxwellOperandForm::SpecialRegister,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MaxwellSemanticOpcode {
    Nop,
    Exit,
    Kill,
    Mov,
    IAdd,
    IMul,
    FAdd,
    FMul,
    Shl,
    Shr,
    Lop,
    Bra,
    Brx,
    Jmp,
    Jmx,
    Call,
    Return,
    Ssy,
    Pbk,
    Pcnt,
    Sync,
    Brk,
    Cont,
    Bar,
    MemBar,
    DepBar,
    Mov32I,
    IAdd32I,
    IMul32I,
    FAdd32I,
    FMul32I,
    Lop32I,
    Xmad,
    Imad,
    Ffma,
    Bfe,
    Bfi,
    Shf,
    Prmt,
    Popc,
    Flo,
    Lop3,
    I2I,
    I2F,
    F2I,
    F2F,
    ISetP,
    FSetP,
    LoadGlobal,
    LoadConstant,
    StoreGlobal,
    LoadAttribute,
    StoreAttribute,
    LoadLocal,
    StoreLocal,
    LoadShared,
    StoreShared,
    Atomic,
    Red,
    SurfaceLoad,
    SurfaceStore,
    SurfaceReduce,
    SurfaceAtomic,
    Tex,
    Tld,
    Tld4,
    Txd,
    Txq,
    Interpolate,
    Shuffle,
    Vote,
    SpecialRegister,
    DAdd,
    DMul,
    DFma,
    DSetP,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticDisposition {
    Ready,
    NeedsCanonicalIr,
    UnknownEncoding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaxwellSemanticInstruction {
    pub instruction: MaxwellInstruction,
    pub opcode: Option<MaxwellSemanticOpcode>,
    pub disposition: SemanticDisposition,
    pub operands: Vec<MaxwellOperand>,
    pub predicate: Option<MaxwellPredicateGuard>,
    pub reason: Option<&'static str>,
}

fn reg(raw: u64, shift: u32) -> MaxwellOperand {
    MaxwellOperand::Register(bits(raw, shift, 8) as u8)
}
fn imm32s(raw: u64) -> MaxwellOperand {
    MaxwellOperand::ImmediateI32(bits(raw, 20, 32) as u32 as i32)
}
fn imm32u(raw: u64) -> MaxwellOperand {
    MaxwellOperand::ImmediateU32(bits(raw, 20, 32) as u32)
}
fn cbuf(raw: u64) -> MaxwellOperand {
    // GM107 arithmetic operands encode a word offset, not a signed byte
    // displacement. Mesa emits the byte offset shifted right by two; bits
    // 34..38 independently select the buffer. Restore the unsigned byte offset.
    MaxwellOperand::ConstantBuffer {
        slot: bits(raw, 34, 5) as u8,
        offset: (bits(raw, 20, 14) << 2) as i32,
        address: None,
    }
}
fn memory(raw: u64, offset_shift: u32, offset_width: u32) -> MaxwellOperand {
    MaxwellOperand::Memory {
        base: bits(raw, 8, 8) as u8,
        offset: sext(bits(raw, offset_shift, offset_width), offset_width) as i32,
    }
}
/// Mesa GM107 emitIADD/emitIMUL use different controls for long immediates.
/// Admit only exact low-word forms; immediate payload bits are never controls.
fn low_word_integer_control(
    raw: u64,
    op: MaxwellSemanticOpcode,
) -> Option<crate::shader::IntegerControl> {
    use MaxwellSemanticOpcode as M;
    let mul = matches!(op, M::IMul | M::IMul32I);
    let long = matches!(op, M::IAdd32I | M::IMul32I);
    let mut variable = 0xffff | (15 << 16);
    let fixed = if long {
        variable |= 0xffff_ffff << 20;
        if mul {
            0x1f00000000000000
        } else {
            0x1c00000000000000
        }
    } else {
        let suffix = if mul {
            0x0038000000000000
        } else {
            0x0010000000000000
        };
        match (raw >> 56) & 0xfe {
            0x5c => {
                variable |= 255 << 20;
                0x5c00000000000000 | suffix
            }
            0x4c => {
                variable |= 0x7ffff << 20;
                0x4c00000000000000 | suffix
            }
            0x38 => {
                variable |= (0x7ffff << 20) | (1 << 56);
                0x3800000000000000 | suffix
            }
            _ => return None,
        }
    };
    let sign_bit = if long { 54 } else { 40 };
    let signed = mul && bits(raw, sign_bit, 2) == 3;
    if mul {
        // Distinct source/result signedness needs a richer control model.
        if !matches!(bits(raw, sign_bit, 2), 0 | 3) {
            return None;
        }
        variable |= 3 << sign_bit;
    } else {
        variable |= if long { 1 << 56 } else { 3 << 48 };
    }
    if raw & !variable != fixed || bits(raw, 0, 8) == 255 {
        return None;
    }
    Some(crate::shader::IntegerControl {
        saturate: false,
        carry_in: false,
        carry_out: false,
        high: false,
        signed,
    })
}

fn logic_controls(
    raw: u64,
    op: MaxwellSemanticOpcode,
) -> Option<(crate::shader::BinaryOp, [crate::shader::ValueModifier; 2])> {
    let long = op == MaxwellSemanticOpcode::Lop32I;
    let mut variable = 0xffff | (15 << 16);
    let fixed = if long {
        variable |= (0xffff_ffff << 20) | (15 << 53);
        0x0400000000000000
    } else {
        variable |= 15 << 39;
        let prefix = match (raw >> 56) & 0xfe {
            0x5c => {
                variable |= 255 << 20;
                0x5c40000000000000
            }
            0x4c => {
                variable |= 0x7ffff << 20;
                0x4c40000000000000
            }
            0x38 => {
                variable |= (0x7ffff << 20) | (1 << 56);
                0x3840000000000000
            }
            _ => return None,
        };
        // PT destination means no predicate output; CC/X remain unsupported.
        prefix | (7 << 48)
    };
    if raw & !variable != fixed || bits(raw, 0, 8) == 255 {
        return None;
    }
    let function = bits(raw, if long { 53 } else { 41 }, 2);
    let op = match function {
        0 => crate::shader::BinaryOp::And,
        1 => crate::shader::BinaryOp::Or,
        2 => crate::shader::BinaryOp::Xor,
        _ => return None, // PASS_B needs its own canonical operation.
    };
    let inv_bit = if long { 55 } else { 39 };
    Some((
        op,
        [0, 1].map(|index| crate::shader::ValueModifier {
            negate: false,
            absolute: false,
            invert: bits(raw, inv_bit + index, 1) != 0,
        }),
    ))
}

fn flo_controls(raw: u64) -> Option<(bool, bool, bool)> {
    // GM107 emitFLO: signed bit 48, shift-amount bit 41, source inversion 40.
    // CC output and src0 are intentionally absent from the admitted mask.
    let mut variable = 255 | (15 << 16) | (1 << 40) | (1 << 41) | (1 << 48);
    let fixed = match (raw >> 56) & 0xfe {
        0x5c => {
            variable |= 255 << 20;
            0x5c30000000000000
        }
        0x4c => {
            variable |= 0x7ffff << 20;
            0x4c30000000000000
        }
        0x38 => {
            variable |= (0x7ffff << 20) | (1 << 56);
            0x3830000000000000
        }
        _ => return None,
    };
    (raw & !variable == fixed && bits(raw, 0, 8) != 255).then_some((
        bits(raw, 48, 1) != 0,
        bits(raw, 40, 1) != 0,
        bits(raw, 41, 1) != 0,
    ))
}

fn popc_controls(raw: u64) -> Option<bool> {
    // Mesa emitPOPC has no src0 field, CC output or integer sign modifier.
    let mut variable = 255 | (15 << 16) | (1 << 40);
    let fixed = match (raw >> 56) & 0xfe {
        0x5c => {
            variable |= 255 << 20;
            0x5c08000000000000
        }
        0x4c => {
            variable |= 0x7ffff << 20;
            0x4c08000000000000
        }
        0x38 => {
            variable |= (0x7ffff << 20) | (1 << 56);
            0x3808000000000000
        }
        _ => return None,
    };
    (raw & !variable == fixed && bits(raw, 0, 8) != 255).then_some(bits(raw, 40, 1) != 0)
}

fn bfe_controls(raw: u64) -> Option<bool> {
    // Mesa GM107 emitBFE: ordinary extraction only; reverse/CC must not be
    // discarded. Source-form masks also exclude reserved register bits.
    let mut variable = 0xffff | (15 << 16) | (1 << 48);
    let fixed = match (raw >> 56) & 0xfe {
        0x5c => {
            variable |= 255 << 20;
            0x5c00000000000000
        }
        0x4c => {
            variable |= 0x7ffff << 20;
            0x4c00000000000000
        }
        0x38 => {
            variable |= (0x7ffff << 20) | (1 << 56);
            0x3800000000000000
        }
        _ => return None,
    };
    (raw & !variable == fixed && bits(raw, 0, 8) != 255).then_some(bits(raw, 48, 1) != 0)
}

fn shift_controls(
    raw: u64,
    op: MaxwellSemanticOpcode,
) -> Option<(crate::shader::WordShiftKind, bool)> {
    use crate::shader::WordShiftKind as K;
    let right = op == MaxwellSemanticOpcode::Shr;
    let suffix = if right {
        0x0028000000000000
    } else {
        0x0048000000000000
    };
    let mut variable = 0xffff | (15 << 16) | (1 << 39);
    if right {
        variable |= 1 << 48;
    }
    let fixed = match (raw >> 56) & 0xfe {
        0x5c => {
            variable |= 255 << 20;
            0x5c00000000000000 | suffix
        }
        0x4c => {
            variable |= 0x7ffff << 20;
            0x4c00000000000000 | suffix
        }
        0x38 => {
            variable |= (0x7ffff << 20) | (1 << 56);
            0x3800000000000000 | suffix
        }
        _ => return None,
    };
    if raw & !variable != fixed || bits(raw, 0, 8) == 255 {
        return None;
    }
    let kind = if !right {
        K::Left
    } else if bits(raw, 48, 1) != 0 {
        K::RightArithmetic
    } else {
        K::RightLogical
    };
    Some((kind, bits(raw, 39, 1) != 0))
}

fn sem(
    i: MaxwellInstruction,
    op: MaxwellSemanticOpcode,
    d: SemanticDisposition,
    operands: Vec<MaxwellOperand>,
    reason: Option<&'static str>,
) -> MaxwellSemanticInstruction {
    let unsupported_integer = matches!(
        op,
        MaxwellSemanticOpcode::IAdd
            | MaxwellSemanticOpcode::IAdd32I
            | MaxwellSemanticOpcode::IMul
            | MaxwellSemanticOpcode::IMul32I
    ) && low_word_integer_control(i.raw, op).is_none()
        || (matches!(
            op,
            MaxwellSemanticOpcode::Lop | MaxwellSemanticOpcode::Lop32I
        ) && logic_controls(i.raw, op).is_none())
        || (matches!(op, MaxwellSemanticOpcode::Shl | MaxwellSemanticOpcode::Shr)
            && shift_controls(i.raw, op).is_none())
        || (op == MaxwellSemanticOpcode::Bfe && bfe_controls(i.raw).is_none())
        || (op == MaxwellSemanticOpcode::Popc && popc_controls(i.raw).is_none())
        || (op == MaxwellSemanticOpcode::Flo && flo_controls(i.raw).is_none());
    let predicate = classify_instruction(i).common.predicate;
    MaxwellSemanticInstruction {
        instruction: i,
        opcode: Some(op),
        disposition: if unsupported_integer {
            SemanticDisposition::NeedsCanonicalIr
        } else {
            d
        },
        operands,
        predicate,
        reason: if unsupported_integer {
            Some("unsupported integer arithmetic encoding/control")
        } else {
            reason
        },
    }
}

/// Decode high-value GM107 operand forms and long-tail operations. Masks mirror the public GM107
/// instruction table; unsupported modifiers remain visible through `NeedsCanonicalIr`.
pub fn decode_semantic(i: MaxwellInstruction) -> MaxwellSemanticInstruction {
    let raw = i.raw;
    let op = (raw >> 48) as u16;
    if op & 0xfff8 == 0xef90 {
        let valid = direct_ldc_words(raw).is_some();
        return sem(
            i,
            MaxwellSemanticOpcode::LoadConstant,
            if valid {
                SemanticDisposition::Ready
            } else {
                SemanticDisposition::NeedsCanonicalIr
            },
            vec![
                reg(raw, 0),
                MaxwellOperand::ConstantBuffer {
                    slot: bits(raw, 36, 5) as u8,
                    offset: bits(raw, 20, 16) as i32,
                    address: (bits(raw, 8, 8) != 255).then_some(bits(raw, 8, 8) as u8),
                },
            ],
            (!valid).then_some("unsupported LDC size, mode, address or offset"),
        );
    }
    if let Some(access) = crate::maxwell_shader::decode_attribute_access(raw) {
        return sem(
            i,
            if access.store {
                MaxwellSemanticOpcode::StoreAttribute
            } else {
                MaxwellSemanticOpcode::LoadAttribute
            },
            SemanticDisposition::NeedsCanonicalIr,
            vec![
                MaxwellOperand::Register(access.register),
                MaxwellOperand::Memory {
                    base: access.address_register,
                    offset: i32::from(access.address_offset),
                },
                MaxwellOperand::Register(access.vertex_register),
                MaxwellOperand::ImmediateU32(u32::from(access.components)),
            ],
            Some("attribute interface addressing requires rich lowering"),
        );
    }
    // Core control and register forms. These patterns are the source of truth shared by coverage
    // and canonical lowering.
    if raw == 0 || op & 0xfff8 == 0x50b0 {
        return sem(
            i,
            MaxwellSemanticOpcode::Nop,
            SemanticDisposition::Ready,
            vec![],
            None,
        );
    }
    if op & 0xfff0 == 0xe300 {
        return sem(
            i,
            MaxwellSemanticOpcode::Exit,
            if valid_exit_encoding(raw) {
                SemanticDisposition::Ready
            } else {
                SemanticDisposition::NeedsCanonicalIr
            },
            vec![],
            (!valid_exit_encoding(raw)).then_some("unsupported EXIT condition or modifier"),
        );
    }
    if op & 0xfff0 == 0xe330 {
        return sem(
            i,
            MaxwellSemanticOpcode::Kill,
            SemanticDisposition::Ready,
            vec![],
            None,
        );
    }
    if op & 0xfff0 == 0xe320 {
        return sem(
            i,
            MaxwellSemanticOpcode::Return,
            SemanticDisposition::NeedsCanonicalIr,
            vec![],
            Some("subroutine return must not be lowered as shader termination"),
        );
    }
    if op & 0xfff0 == 0xe240 {
        return sem(
            i,
            MaxwellSemanticOpcode::Bra,
            if valid_direct_branch_encoding(raw) {
                SemanticDisposition::Ready
            } else {
                SemanticDisposition::NeedsCanonicalIr
            },
            vec![MaxwellOperand::ImmediateI32(
                sext(bits(raw, 20, 24), 24) as i32
            )],
            (!valid_direct_branch_encoding(raw)).then_some("unsupported BRA condition or modifier"),
        );
    }
    if op & 0xfff0 == 0xe250 {
        return sem(
            i,
            MaxwellSemanticOpcode::Brx,
            SemanticDisposition::Ready,
            vec![
                reg(raw, 8),
                MaxwellOperand::ImmediateI32(sext(bits(raw, 20, 24), 24) as i32),
            ],
            None,
        );
    }
    if op & 0xfff0 == 0xe210 {
        return sem(
            i,
            MaxwellSemanticOpcode::Jmp,
            SemanticDisposition::Ready,
            vec![imm32u(raw)],
            None,
        );
    }
    if op & 0xfff0 == 0xe200 {
        return sem(
            i,
            MaxwellSemanticOpcode::Jmx,
            SemanticDisposition::Ready,
            vec![reg(raw, 8), imm32s(raw)],
            None,
        );
    }
    if op & 0xfff0 == 0xe220 || op & 0xfff0 == 0xe260 {
        return sem(
            i,
            MaxwellSemanticOpcode::Call,
            SemanticDisposition::Ready,
            vec![imm32u(raw)],
            None,
        );
    }
    if op & 0xfff0 == 0xe290 {
        return sem(
            i,
            MaxwellSemanticOpcode::Ssy,
            SemanticDisposition::Ready,
            vec![MaxwellOperand::ImmediateI32(
                sext(bits(raw, 20, 24), 24) as i32
            )],
            None,
        );
    }
    if op & 0xfff0 == 0xe2a0 {
        return sem(
            i,
            MaxwellSemanticOpcode::Pbk,
            SemanticDisposition::Ready,
            vec![MaxwellOperand::ImmediateI32(
                sext(bits(raw, 20, 24), 24) as i32
            )],
            None,
        );
    }
    if op & 0xfff0 == 0xe2b0 {
        return sem(
            i,
            MaxwellSemanticOpcode::Pcnt,
            SemanticDisposition::Ready,
            vec![MaxwellOperand::ImmediateI32(
                sext(bits(raw, 20, 24), 24) as i32
            )],
            None,
        );
    }
    if op & 0xfff8 == 0xf0f8 {
        return sem(
            i,
            MaxwellSemanticOpcode::Sync,
            SemanticDisposition::Ready,
            vec![],
            None,
        );
    }
    if op & 0xfff0 == 0xe340 {
        return sem(
            i,
            MaxwellSemanticOpcode::Brk,
            SemanticDisposition::Ready,
            vec![],
            None,
        );
    }
    if op & 0xfff0 == 0xe350 {
        return sem(
            i,
            MaxwellSemanticOpcode::Cont,
            SemanticDisposition::Ready,
            vec![],
            None,
        );
    }
    if op & 0xfff8 == 0xf0a8 {
        return sem(
            i,
            MaxwellSemanticOpcode::Bar,
            SemanticDisposition::Ready,
            vec![],
            None,
        );
    }
    if op & 0xfff8 == 0xef98 {
        return sem(
            i,
            MaxwellSemanticOpcode::MemBar,
            SemanticDisposition::Ready,
            vec![],
            None,
        );
    }
    if op & 0xfff8 == 0xf0f0 {
        return sem(
            i,
            MaxwellSemanticOpcode::DepBar,
            SemanticDisposition::Ready,
            vec![],
            None,
        );
    }
    if op & 0xfff8 == 0xf0c8 {
        return sem(
            i,
            MaxwellSemanticOpcode::SpecialRegister,
            SemanticDisposition::Ready,
            vec![
                reg(raw, 0),
                MaxwellOperand::SpecialRegister(bits(raw, 20, 8) as u8),
            ],
            None,
        );
    }
    let core = match op & 0xfff8 {
        0x5c30 => Some(MaxwellSemanticOpcode::Flo),
        0x5c08 => Some(MaxwellSemanticOpcode::Popc),
        0x5c00 => Some(MaxwellSemanticOpcode::Bfe),
        0x5c98 => Some(MaxwellSemanticOpcode::Mov),
        0x5c10 => Some(MaxwellSemanticOpcode::IAdd),
        0x5c38 => Some(MaxwellSemanticOpcode::IMul),
        0x5c58 => Some(MaxwellSemanticOpcode::FAdd),
        0x5c68 => Some(MaxwellSemanticOpcode::FMul),
        0x5c48 => Some(MaxwellSemanticOpcode::Shl),
        0x5c28 => Some(MaxwellSemanticOpcode::Shr),
        0x5c40 => Some(MaxwellSemanticOpcode::Lop),
        _ => None,
    };
    if let Some(code) = core {
        let operands = if matches!(
            code,
            MaxwellSemanticOpcode::Mov | MaxwellSemanticOpcode::Popc | MaxwellSemanticOpcode::Flo
        ) {
            vec![reg(raw, 0), reg(raw, 20)]
        } else {
            vec![reg(raw, 0), reg(raw, 8), reg(raw, 20)]
        };
        let supported = match code {
            MaxwellSemanticOpcode::Mov => valid_register_mov_encoding(raw),
            MaxwellSemanticOpcode::FAdd | MaxwellSemanticOpcode::FMul => {
                ordinary_float_controls(raw, code).is_some()
            }
            _ => true,
        };
        return sem(
            i,
            code,
            if supported {
                SemanticDisposition::Ready
            } else {
                SemanticDisposition::NeedsCanonicalIr
            },
            operands,
            (!supported).then_some("unsupported instruction lanes, modifiers or zero register"),
        );
    }
    // Common constant-buffer and 20-bit immediate forms.
    for (pat, code) in [
        (0x4c30, MaxwellSemanticOpcode::Flo),
        (0x4c08, MaxwellSemanticOpcode::Popc),
        (0x4c00, MaxwellSemanticOpcode::Bfe),
        (0x4c98, MaxwellSemanticOpcode::Mov),
        (0x4c10, MaxwellSemanticOpcode::IAdd),
        (0x4c38, MaxwellSemanticOpcode::IMul),
        (0x4c58, MaxwellSemanticOpcode::FAdd),
        (0x4c68, MaxwellSemanticOpcode::FMul),
        (0x4c40, MaxwellSemanticOpcode::Lop),
        (0x4c48, MaxwellSemanticOpcode::Shl),
        (0x4c28, MaxwellSemanticOpcode::Shr),
    ] {
        if op & 0xfff8 == pat {
            let mut v = vec![reg(raw, 0)];
            if !matches!(
                code,
                MaxwellSemanticOpcode::Mov
                    | MaxwellSemanticOpcode::Popc
                    | MaxwellSemanticOpcode::Flo
            ) {
                v.push(reg(raw, 8));
            }
            v.push(cbuf(raw));
            let supported = if code == MaxwellSemanticOpcode::Mov {
                valid_constant_mov_encoding(raw)
            } else {
                !matches!(
                    code,
                    MaxwellSemanticOpcode::FAdd | MaxwellSemanticOpcode::FMul
                ) || ordinary_float_controls(raw, code).is_some()
            };
            return sem(
                i,
                code,
                if !supported {
                    SemanticDisposition::NeedsCanonicalIr
                } else {
                    SemanticDisposition::Ready
                },
                v,
                (!supported).then_some("unsupported constant-buffer instruction controls"),
            );
        }
    }
    for (pat, code) in [
        (0x3830, MaxwellSemanticOpcode::Flo),
        (0x3808, MaxwellSemanticOpcode::Popc),
        (0x3800, MaxwellSemanticOpcode::Bfe),
        (0x3898, MaxwellSemanticOpcode::Mov),
        (0x3810, MaxwellSemanticOpcode::IAdd),
        (0x3838, MaxwellSemanticOpcode::IMul),
        (0x3858, MaxwellSemanticOpcode::FAdd),
        (0x3868, MaxwellSemanticOpcode::FMul),
        (0x3840, MaxwellSemanticOpcode::Lop),
        (0x3848, MaxwellSemanticOpcode::Shl),
        (0x3828, MaxwellSemanticOpcode::Shr),
    ] {
        if op & 0xfef8 == pat {
            let mut v = vec![reg(raw, 0)];
            if !matches!(
                code,
                MaxwellSemanticOpcode::Mov
                    | MaxwellSemanticOpcode::Popc
                    | MaxwellSemanticOpcode::Flo
            ) {
                v.push(reg(raw, 8));
            }
            if code == MaxwellSemanticOpcode::Mov {
                v.push(MaxwellOperand::ImmediateU32(short_mov_bits(raw)));
            } else if matches!(
                code,
                MaxwellSemanticOpcode::FAdd | MaxwellSemanticOpcode::FMul
            ) {
                v.push(MaxwellOperand::ImmediateF32Bits20(
                    (short_immediate20(raw) << 12) as u32,
                ));
            } else {
                v.push(MaxwellOperand::ImmediateI32(
                    sext(short_immediate20(raw), 20) as i32,
                ));
            }
            return sem(
                i,
                code,
                if (code == MaxwellSemanticOpcode::Mov && !valid_short_mov_encoding(raw))
                    || (matches!(
                        code,
                        MaxwellSemanticOpcode::FAdd | MaxwellSemanticOpcode::FMul
                    ) && ordinary_float_controls(raw, code).is_none())
                {
                    SemanticDisposition::NeedsCanonicalIr
                } else {
                    SemanticDisposition::Ready
                },
                v,
                ((code == MaxwellSemanticOpcode::Mov && !valid_short_mov_encoding(raw))
                    || (matches!(
                        code,
                        MaxwellSemanticOpcode::FAdd | MaxwellSemanticOpcode::FMul
                    ) && ordinary_float_controls(raw, code).is_none()))
                .then_some("unsupported short-immediate instruction controls"),
            );
        }
    }
    // 32-bit immediate family: exact high-byte encodings from gm107.c.
    if raw & 0xfff0_0000_0000_0000 == 0x0100_0000_0000_0000 {
        return sem(
            i,
            MaxwellSemanticOpcode::Mov32I,
            if valid_mov32i_encoding(raw) {
                SemanticDisposition::Ready
            } else {
                SemanticDisposition::NeedsCanonicalIr
            },
            vec![reg(raw, 0), imm32u(raw)],
            (!valid_mov32i_encoding(raw))
                .then_some("partial lane mask or unsupported MOV32I modifier"),
        );
    }
    if raw & 0xfe80_0000_0000_0000 == 0x1c00_0000_0000_0000 {
        return sem(
            i,
            MaxwellSemanticOpcode::IAdd32I,
            SemanticDisposition::Ready,
            vec![reg(raw, 0), reg(raw, 8), imm32s(raw)],
            None,
        );
    }
    if raw & 0xff00_0000_0000_0000 == 0x1f00_0000_0000_0000 {
        return sem(
            i,
            MaxwellSemanticOpcode::IMul32I,
            SemanticDisposition::Ready,
            vec![reg(raw, 0), reg(raw, 8), imm32s(raw)],
            None,
        );
    }
    if raw & 0xfc00_0000_0000_0000 == 0x0800_0000_0000_0000 {
        return sem(
            i,
            MaxwellSemanticOpcode::FAdd32I,
            SemanticDisposition::NeedsCanonicalIr,
            vec![
                reg(raw, 0),
                reg(raw, 8),
                MaxwellOperand::ImmediateF32Bits32(bits(raw, 20, 32) as u32),
            ],
            Some("float source modifiers/FTZ must be preserved"),
        );
    }
    if raw & 0xff00_0000_0000_0000 == 0x1e00_0000_0000_0000 {
        return sem(
            i,
            MaxwellSemanticOpcode::FMul32I,
            SemanticDisposition::NeedsCanonicalIr,
            vec![
                reg(raw, 0),
                reg(raw, 8),
                MaxwellOperand::ImmediateF32Bits32(bits(raw, 20, 32) as u32),
            ],
            Some("float saturation/rounding modifiers must be preserved"),
        );
    }
    if raw & 0xfc00_0000_0000_0000 == 0x0400_0000_0000_0000 {
        return sem(
            i,
            MaxwellSemanticOpcode::Lop32I,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), reg(raw, 8), imm32u(raw)],
            Some("LOP function/invert modifiers need canonical representation"),
        );
    }

    // Common register/cbuf long-tail families. These are decoded structurally first; exact
    // operation modifiers stay explicit instead of being silently discarded.
    let hi = op & 0xfff8;
    if hi == 0x4cb8 {
        return sem(
            i,
            MaxwellSemanticOpcode::I2F,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), cbuf(raw)],
            Some("integer-to-float constant-buffer source"),
        );
    }
    if hi & 0xfef8 == 0x38b8 {
        let immediate = sext(short_immediate20(raw), 20) as i32;
        return sem(
            i,
            MaxwellSemanticOpcode::I2F,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), MaxwellOperand::ImmediateI32(immediate)],
            Some("integer-to-float signed immediate encoding"),
        );
    }
    let (long, reason) = match hi {
        0x5b00 | 0x5b08 | 0x5b10 | 0x5b18 | 0x5b20 | 0x5b28 | 0x5b30 | 0x5b38 => (
            Some(MaxwellSemanticOpcode::Xmad),
            "XMAD mode/merge modifiers",
        ),
        0x5a00 | 0x5a08 | 0x5a10 | 0x5a18 => (
            Some(MaxwellSemanticOpcode::Imad),
            "IMAD wide/hi/carry modifiers",
        ),
        0x5980 | 0x5988 | 0x59a0 | 0x59a8 => (
            Some(MaxwellSemanticOpcode::Ffma),
            "FFMA rounding/negate/saturate modifiers",
        ),
        0x5c00 => (
            Some(MaxwellSemanticOpcode::Bfe),
            "BFE signed/position/width semantics",
        ),
        0x5be0 => (
            Some(MaxwellSemanticOpcode::Lop3),
            "LOP3 truth-table semantics",
        ),
        0x5bf0 => (
            Some(MaxwellSemanticOpcode::Bfi),
            "BFI position/width semantics",
        ),
        0x5bf8 => (
            Some(MaxwellSemanticOpcode::Shf),
            "funnel-shift direction/type semantics",
        ),
        0x5bc0 | 0x5bc8 => (
            Some(MaxwellSemanticOpcode::Prmt),
            "PRMT selector mode semantics",
        ),
        0x5c08 => (Some(MaxwellSemanticOpcode::Popc), "POPC invert modifier"),
        0x5c30 => (
            Some(MaxwellSemanticOpcode::Flo),
            "FLO signed/shift modifiers",
        ),
        0x5ce0 => (
            Some(MaxwellSemanticOpcode::I2I),
            "integer conversion size/sign/saturation",
        ),
        0x5cb8 => (
            Some(MaxwellSemanticOpcode::I2F),
            "integer-to-float rounding/type",
        ),
        0x5cb0 => (
            Some(MaxwellSemanticOpcode::F2I),
            "float-to-integer rounding/type",
        ),
        0x5ca8 => (
            Some(MaxwellSemanticOpcode::F2F),
            "float conversion rounding/type",
        ),
        0x5b60 => (
            Some(MaxwellSemanticOpcode::ISetP),
            "predicate compare/combine semantics",
        ),
        0x5bb0 => (
            Some(MaxwellSemanticOpcode::FSetP),
            "floating predicate compare/combine semantics",
        ),
        0x5c70 => (Some(MaxwellSemanticOpcode::DAdd), "double add modifiers"),
        0x5c80 => (
            Some(MaxwellSemanticOpcode::DMul),
            "double multiply modifiers",
        ),
        0x5b70 => (
            Some(MaxwellSemanticOpcode::DFma),
            "double fused multiply-add modifiers",
        ),
        0x5b80 => (
            Some(MaxwellSemanticOpcode::DSetP),
            "double predicate compare/combine semantics",
        ),
        _ => (None, ""),
    };
    if let Some(code) = long {
        return sem(
            i,
            code,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), reg(raw, 8), reg(raw, 20), reg(raw, 39)],
            Some(reason),
        );
    }

    // Memory classes. Offsets follow the GM107 LD/ST operand fields; the semantic layer records
    // address shape even where cache/type flags still need lowering policy.
    if op & 0xfff8 == 0xeed0 {
        return sem(
            i,
            MaxwellSemanticOpcode::LoadGlobal,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), memory(raw, 20, 24)],
            Some("global load width/cache policy pending canonical memory access descriptor"),
        );
    }
    if op & 0xfff8 == 0xeed8 {
        return sem(
            i,
            MaxwellSemanticOpcode::StoreGlobal,
            SemanticDisposition::NeedsCanonicalIr,
            vec![memory(raw, 20, 24), reg(raw, 0)],
            Some("global store width/cache policy pending canonical memory access descriptor"),
        );
    }
    if op & 0xfff8 == 0xef40 {
        return sem(
            i,
            MaxwellSemanticOpcode::LoadLocal,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), memory(raw, 20, 24)],
            Some("local memory access width pending"),
        );
    }
    if op & 0xfff8 == 0xef50 {
        return sem(
            i,
            MaxwellSemanticOpcode::StoreLocal,
            SemanticDisposition::NeedsCanonicalIr,
            vec![memory(raw, 20, 24), reg(raw, 0)],
            Some("local memory access width pending"),
        );
    }
    if op & 0xfff8 == 0xef48 {
        return sem(
            i,
            MaxwellSemanticOpcode::LoadShared,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), memory(raw, 20, 24)],
            Some("shared memory access width pending"),
        );
    }
    if op & 0xfff8 == 0xef58 {
        return sem(
            i,
            MaxwellSemanticOpcode::StoreShared,
            SemanticDisposition::NeedsCanonicalIr,
            vec![memory(raw, 20, 24), reg(raw, 0)],
            Some("shared memory access width pending"),
        );
    }
    if op & 0xfff8 == 0xebf8 {
        return sem(
            i,
            MaxwellSemanticOpcode::Red,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), memory(raw, 20, 20), reg(raw, 0), reg(raw, 39)],
            Some("global reduction op/type lowering"),
        );
    }
    if op & 0xff00 == 0xed00
        || op & 0xfff0 == 0xeef0
        || op & 0xff80 == 0xee00
        || op & 0xff00 == 0xec00
    {
        return sem(
            i,
            MaxwellSemanticOpcode::Atomic,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), memory(raw, 20, 20), reg(raw, 20), reg(raw, 39)],
            Some("atomic op/type/CAS tuple lowering"),
        );
    }

    if op & 0xfff8 == 0xef10 {
        return sem(
            i,
            MaxwellSemanticOpcode::Shuffle,
            SemanticDisposition::Ready,
            vec![reg(raw, 0), reg(raw, 8), reg(raw, 20), reg(raw, 39)],
            None,
        );
    }
    if op & 0xfff8 == 0x50d8 || op & 0xfff8 == 0x50e0 {
        return sem(
            i,
            MaxwellSemanticOpcode::Vote,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), reg(raw, 20)],
            Some("vote mode/predicate output lowering"),
        );
    }
    // Maxwell surface/image family. Keep the complete raw flag payload in SurfaceControl during
    // lowering, so unknown cache/format combinations remain lossless rather than opaque.
    if op & 0xffe0 == 0xeb00 {
        return sem(
            i,
            MaxwellSemanticOpcode::SurfaceLoad,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), reg(raw, 8), reg(raw, 20), reg(raw, 39)],
            Some("surface load descriptor lowering"),
        );
    }
    if op & 0xffe0 == 0xeb20 {
        return sem(
            i,
            MaxwellSemanticOpcode::SurfaceStore,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), reg(raw, 8), reg(raw, 20), reg(raw, 39)],
            Some("surface store descriptor lowering"),
        );
    }
    if op & 0xffe0 == 0xeb40 {
        return sem(
            i,
            MaxwellSemanticOpcode::SurfaceReduce,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), reg(raw, 8), reg(raw, 20), reg(raw, 39)],
            Some("surface reduction descriptor lowering"),
        );
    }
    if op & 0xffc0 == 0xea00 || op & 0xffc0 == 0xea80 {
        return sem(
            i,
            MaxwellSemanticOpcode::SurfaceAtomic,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), reg(raw, 8), reg(raw, 20), reg(raw, 39)],
            Some("surface atomic descriptor lowering"),
        );
    }

    // Texture handle and principal register tuple. Complex bindless/LOD/offset modes are retained
    // as NeedsCanonicalIr until the canonical Texture operation can encode them losslessly.
    let family = classify_instruction(i).family;
    if family == MaxwellOpcodeFamily::Texture {
        let code = if op & 0xfff8 == 0xdf48 || op & 0xfff8 == 0xdf50 {
            MaxwellSemanticOpcode::Txq
        } else if op & 0xfffc == 0xde38 || op & 0xfffc == 0xde78 {
            MaxwellSemanticOpcode::Txd
        } else if op & 0xfc38 == 0xc838 || op & 0xfff8 == 0xdef8 {
            MaxwellSemanticOpcode::Tld4
        } else if op & 0xff38 == 0xdc38 || op & 0xff38 == 0xdd38 || (op & 0xf600) == 0xd200 {
            MaxwellSemanticOpcode::Tld
        } else {
            MaxwellSemanticOpcode::Tex
        };
        return sem(
            i,
            code,
            SemanticDisposition::NeedsCanonicalIr,
            vec![
                reg(raw, 0),
                reg(raw, 8),
                MaxwellOperand::TextureHandle(bits(raw, 36, 13) as u16),
            ],
            Some(
                "texture dimension/LOD/offset/bindless modifiers require richer canonical texture descriptor",
            ),
        );
    }
    if family == MaxwellOpcodeFamily::Interpolation {
        return sem(
            i,
            MaxwellSemanticOpcode::Interpolate,
            SemanticDisposition::NeedsCanonicalIr,
            vec![reg(raw, 0), reg(raw, 8)],
            Some("interpolation mode/sample/offset semantics pending"),
        );
    }
    if family == MaxwellOpcodeFamily::SpecialRegister {
        return sem(
            i,
            MaxwellSemanticOpcode::SpecialRegister,
            SemanticDisposition::NeedsCanonicalIr,
            vec![
                reg(raw, 0),
                MaxwellOperand::SpecialRegister(bits(raw, 20, 8) as u8),
            ],
            Some("special-register id must map to canonical builtin"),
        );
    }

    MaxwellSemanticInstruction {
        instruction: i,
        opcode: None,
        disposition: SemanticDisposition::UnknownEncoding,
        operands: Vec::new(),
        predicate: classify_instruction(i).common.predicate,
        reason: Some("no validated semantic form"),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SemanticCoverageReport {
    pub total: usize,
    pub ready: usize,
    pub needs_canonical_ir: usize,
    pub unknown_encoding: usize,
    pub opcodes: BTreeMap<MaxwellSemanticOpcode, usize>,
    pub operand_forms: BTreeMap<MaxwellOperandForm, usize>,
    pub fallback_reasons: BTreeMap<&'static str, usize>,
}
impl SemanticCoverageReport {
    pub fn decoded(&self) -> usize {
        self.ready + self.needs_canonical_ir
    }
    pub fn decoded_percent(&self) -> f32 {
        if self.total == 0 {
            100.0
        } else {
            100.0 * self.decoded() as f32 / self.total as f32
        }
    }
}

pub fn semantic_coverage(program: &MaxwellProgram) -> SemanticCoverageReport {
    let mut r = SemanticCoverageReport::default();
    for i in program
        .bundles
        .iter()
        .flat_map(|b| b.instructions.iter().copied())
    {
        r.total += 1;
        let d = decode_semantic(i);
        match d.disposition {
            SemanticDisposition::Ready => r.ready += 1,
            SemanticDisposition::NeedsCanonicalIr => r.needs_canonical_ir += 1,
            SemanticDisposition::UnknownEncoding => r.unknown_encoding += 1,
        }
        if let Some(op) = d.opcode {
            *r.opcodes.entry(op).or_default() += 1
        }
        for operand in d.operands {
            *r.operand_forms.entry(operand.form()).or_default() += 1
        }
        if let Some(reason) = d.reason {
            *r.fallback_reasons.entry(reason).or_default() += 1
        }
    }
    r
}

#[cfg(test)]
mod tests {
    #[test]
    fn wide_ldc_preserves_consecutive_words_and_rejects_misalignment() {
        use super::*;
        for (size, words) in [(5u64, 2u32), (6, 4)] {
            let raw = 0xef90000000000000
                | (size << 48)
                | (255 << 8)
                | (7 << 16)
                | (3 << 36)
                | (32 << 20)
                | 8;
            let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
            assert_eq!(decoded.disposition, SemanticDisposition::Ready);
            let lowered = lower_semantic(&decoded);
            assert!(lowered.lossless);
            assert_eq!(lowered.operations.len(), words as usize);
            for (word, operation) in lowered.operations.iter().enumerate() {
                assert!(
                    matches!(operation, crate::ShaderOperation::LoadConstantBuffer {
                    dst, slot: 3, offset, address: None,
                } if *dst == 8 + word as u32 && *offset == 32 + word as i32 * 4)
                );
            }
            for invalid in [raw | 1, raw | (4 << 20), (raw & !255) | 254] {
                assert!(direct_ldc_words(invalid).is_none());
                assert!(
                    !lower_semantic(&decode_semantic(MaxwellInstruction {
                        offset: 8,
                        raw: invalid
                    }))
                    .lossless
                );
            }
        }
    }
    #[test]
    fn ldc_scalar_direct_reads_keep_byte_offsets_and_reject_other_modes() {
        use super::*;
        let raw =
            0xef90000000000000 | (4 << 48) | (255 << 8) | (7 << 16) | (3 << 36) | (28 << 20) | 9;
        let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
        assert_eq!(decoded.disposition, SemanticDisposition::Ready);
        let lowered = lower_semantic(&decoded);
        assert!(lowered.lossless);
        assert!(matches!(
            lowered.operations.as_slice(),
            [crate::ShaderOperation::LoadConstantBuffer {
                dst: 9,
                slot: 3,
                offset: 28,
                address: None
            }]
        ));
        for invalid in [
            raw | (1 << 44),
            raw | (1 << 47),
            raw | (1 << 20),
            raw | (1 << 35),
            (raw & !(255 << 8)) | (1 << 8),
            (raw & !(7 << 48)) | (5 << 48),
            (raw & !255) | 255,
        ] {
            let decoded = decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: invalid,
            });
            assert_ne!(decoded.disposition, SemanticDisposition::Ready);
            assert!(!lower_semantic(&decoded).lossless);
        }
    }
    #[test]
    fn constant_mov_preserves_memory_operands_and_rejects_partial_lanes() {
        use super::*;
        let raw = 0x4c98000000000000 | (31 << 34) | (7 << 20) | (15 << 39) | (7 << 16) | 9;
        let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
        assert_eq!(decoded.disposition, SemanticDisposition::Ready);
        let lowered = lower_semantic(&decoded);
        assert!(lowered.lossless);
        assert!(lowered.operations.iter().any(|op| matches!(
            op,
            crate::ShaderOperation::LoadConstantBuffer {
                slot: 31,
                offset: 28,
                address: None,
                ..
            }
        )));
        assert!(matches!(
            lowered.operations.last(),
            Some(crate::ShaderOperation::Move { dst: 9, .. })
        ));
        for mask in 0..15 {
            let invalid = decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: (raw & !(15 << 39)) | (mask << 39),
            });
            assert_ne!(invalid.disposition, SemanticDisposition::Ready);
            assert!(!lower_semantic(&invalid).lossless);
        }
        for reserved in [1 << 8, 1 << 12, 1 << 43, 1 << 47] {
            assert!(!valid_constant_mov_encoding(raw | reserved));
        }
        assert!(!valid_constant_mov_encoding((raw & !255) | 255));
    }
    #[test]
    fn constant_arithmetic_offsets_are_unsigned_words_independent_of_slot() {
        use super::*;
        for slot in 0..32u64 {
            for word in 0..0x4000u64 {
                let raw =
                    0x4c58000000000000 | (slot << 34) | (word << 20) | (7 << 16) | (1 << 8) | 2;
                assert_eq!(
                    cbuf(raw),
                    MaxwellOperand::ConstantBuffer {
                        slot: slot as u8,
                        offset: (word * 4) as i32,
                        address: None,
                    }
                );
            }
        }
        for offset in [4u64, 28, 0x8000, 0xfffc] {
            let raw =
                0x4c58000000000000 | (3 << 34) | ((offset / 4) << 20) | (7 << 16) | (1 << 8) | 2;
            let semantic =
                decode_semantic(crate::maxwell_shader::MaxwellInstruction { offset: 8, raw });
            let lowered = lower_semantic(&semantic);
            assert!(lowered.lossless);
            assert!(lowered.operations.iter().any(|operation| matches!(operation,
                crate::ShaderOperation::LoadConstantBuffer { slot: 3, offset: decoded, .. } if *decoded == offset as i32)));
        }
    }
    use super::*;
    use crate::ShaderStage;
    use crate::maxwell_shader::MaxwellBundle;
    #[test]
    fn decodes_mov32i() {
        let i = MaxwellInstruction {
            offset: 8,
            raw: 0x0100_0001_2345_f002,
        };
        let d = decode_semantic(i);
        assert_eq!(d.opcode, Some(MaxwellSemanticOpcode::Mov32I));
        assert_eq!(d.disposition, SemanticDisposition::Ready);
    }
    #[test]
    fn short_mov_immediate_uses_separate_sign_bit() {
        for (immediate, expected) in [
            (0u64, 0u32),
            (1, 1),
            (0x7ffff, 0x7ffff),
            (0x80000, 0xfff80000),
            (0xfffff, 0xffffffff),
        ] {
            let raw = 0x3898000000000000
                | (15 << 39)
                | (7 << 16)
                | 2
                | ((immediate & 0x7ffff) << 20)
                | ((immediate >> 19) << 56);
            let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
            assert_eq!(decoded.disposition, SemanticDisposition::Ready);
            assert_eq!(decoded.operands[1], MaxwellOperand::ImmediateU32(expected));
            assert_eq!(
                lower_semantic(&decoded).operations,
                vec![crate::ShaderOperation::ConstantBits32 {
                    dst: 2,
                    value: expected
                }]
            );
            for mask in 0..15 {
                let invalid = decode_semantic(MaxwellInstruction {
                    offset: 8,
                    raw: (raw & !(15 << 39)) | (mask << 39),
                });
                assert_ne!(invalid.disposition, SemanticDisposition::Ready);
                assert!(!lower_semantic(&invalid).lossless);
            }
            for bit in [8, 12, 43] {
                let invalid = decode_semantic(MaxwellInstruction {
                    offset: 8,
                    raw: raw | (1 << bit),
                });
                assert!(!lower_semantic(&invalid).lossless);
            }
        }
    }
    #[test]
    fn arithmetic_immediates_preserve_integer_sign_and_float_bits() {
        for opcode in [
            0x3810000000000000u64,
            0x3838000000000000,
            0x3840000000000000,
        ] {
            for (immediate, expected) in [
                (0u64, 0i32),
                (0x7ffff, 0x7ffff),
                (0x80000, -524288),
                (0xfffff, -1),
            ] {
                let raw =
                    opcode | (7 << 16) | ((immediate & 0x7ffff) << 20) | ((immediate >> 19) << 56);
                for adjacent in [0, 1 << 39] {
                    let decoded = decode_semantic(MaxwellInstruction {
                        offset: 8,
                        raw: raw | adjacent,
                    });
                    assert_eq!(decoded.operands[2], MaxwellOperand::ImmediateI32(expected));
                }
            }
        }
        for (opcode, short) in [
            (0x3858000000000000u64, true),
            (0x3868000000000000, true),
            (0x0800000000000000, false),
            (0x1e00000000000000, false),
        ] {
            for bits in [0u32, 0x80000000, 0x3f800000, 0x7fa12000, 0xff800000] {
                let encoded = if short {
                    (u64::from(bits >> 12) & 0x7ffff) << 20 | (u64::from(bits >> 31) << 56)
                } else {
                    u64::from(bits) << 20
                };
                let decoded = decode_semantic(MaxwellInstruction {
                    offset: 8,
                    raw: opcode | (7 << 16) | encoded,
                });
                assert_eq!(
                    decoded.operands[2],
                    if short {
                        MaxwellOperand::ImmediateF32Bits20(bits)
                    } else {
                        MaxwellOperand::ImmediateF32Bits32(bits)
                    }
                );
                let lowered = lower_semantic(&decoded);
                assert!(
                    lowered
                        .operations
                        .iter()
                        .any(|operation| matches!(operation,
                    crate::ShaderOperation::ConstantF32 { value, .. } if value.to_bits() == bits))
                );
                assert!(!lowered.operations.iter().any(|operation| matches!(
                    operation,
                    crate::ShaderOperation::ConstantI32 { .. }
                        | crate::ShaderOperation::ConstantU32 { .. }
                )));
            }
        }
    }

    #[test]
    fn integer_arithmetic_uses_form_specific_controls_not_immediate_payload_bits() {
        use crate::ShaderOperation as S;
        for (opcode, long, mul) in [
            (0x5c10000000000000u64, false, false),
            (0x4c10000000000000, false, false),
            (0x3810000000000000, false, false),
            (0x1c00000000000000, true, false),
            (0x5c38000000000000, false, true),
            (0x4c38000000000000, false, true),
            (0x3838000000000000, false, true),
            (0x1f00000000000000, true, true),
        ] {
            let payload = if long { 0xffff_ffffu64 } else { 2 };
            let raw = opcode | 1 | (3 << 8) | (7 << 16) | (payload << 20);
            let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
            assert_eq!(decoded.disposition, SemanticDisposition::Ready, "{raw:x}");
            let lowered = lower_semantic(&decoded);
            assert!(lowered.lossless, "{raw:x}");
            let Some(S::IntegerBinary {
                control,
                left_modifier,
                right_modifier,
                ..
            }) = lowered.operations.last()
            else {
                panic!("missing arithmetic");
            };
            assert!(!control.saturate && !control.high && !control.carry_in && !control.carry_out);
            assert!(
                !left_modifier.negate
                    && !left_modifier.absolute
                    && !right_modifier.negate
                    && !right_modifier.absolute
            );
            let unsupported = if long {
                if mul { vec![52, 53] } else { vec![52, 53, 54] }
            } else if mul {
                vec![39, 47]
            } else {
                vec![43, 47, 50]
            };
            if !mul {
                for negate in 1..if long { 2 } else { 4 } {
                    let modified =
                        raw | if negate & 1 != 0 {
                            1 << if long { 56 } else { 49 }
                        } else {
                            0
                        } | if negate & 2 != 0 { 1 << 48 } else { 0 };
                    let lowered = lower_semantic(&decode_semantic(MaxwellInstruction {
                        offset: 8,
                        raw: modified,
                    }));
                    assert!(lowered.lossless);
                    let Some(S::IntegerBinary {
                        left_modifier,
                        right_modifier,
                        ..
                    }) = lowered.operations.last()
                    else {
                        panic!("missing negated arithmetic");
                    };
                    assert_eq!(left_modifier.negate, negate & 1 != 0);
                    assert_eq!(right_modifier.negate, negate & 2 != 0);
                }
            }
            for bit in unsupported {
                let invalid = decode_semantic(MaxwellInstruction {
                    offset: 8,
                    raw: raw | (1 << bit),
                });
                assert_ne!(invalid.disposition, SemanticDisposition::Ready);
                assert!(!lower_semantic(&invalid).lossless);
            }
        }
    }

    #[test]
    fn integer_zero_sources_are_immutable_words_not_register_dependencies() {
        use crate::ShaderOperation as S;
        for opcode in [0x5c10000000000000u64, 0x5c38000000000000] {
            let raw = opcode | (255 << 8) | (255 << 20) | (7 << 16);
            let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
            let lowered = lower_semantic(&decoded);
            assert!(lowered.lossless);
            let [
                S::ConstantBits32 { dst: a, value: 0 },
                S::ConstantBits32 { dst: b, value: 0 },
                S::IntegerBinary { left, right, .. },
            ] = lowered.operations.as_slice()
            else {
                panic!("RZ sources must become zero constants");
            };
            assert_ne!(a, b);
            assert_eq!(a, left);
            assert_eq!(b, right);
            assert_ne!(*a, 255);
            let discarded = decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: raw | 255,
            });
            assert!(!lower_semantic(&discarded).lossless);
        }
    }

    #[test]
    fn logic_controls_preserve_function_inversion_and_reject_side_effects() {
        use crate::{ShaderOperation as S, shader::BinaryOp};
        for (opcode, long) in [
            (0x5c40000000000000u64, false),
            (0x4c40000000000000, false),
            (0x3840000000000000, false),
            (0x0400000000000000, true),
        ] {
            for (function, expected) in [BinaryOp::And, BinaryOp::Or, BinaryOp::Xor]
                .into_iter()
                .enumerate()
            {
                for invert in 0..4u64 {
                    let raw = opcode
                        | 1
                        | (2 << 8)
                        | (7 << 16)
                        | (3 << 20)
                        | if long { 0 } else { 7 << 48 }
                        | ((function as u64) << if long { 53 } else { 41 })
                        | (invert << if long { 55 } else { 39 });
                    let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
                    let lowered = lower_semantic(&decoded);
                    assert!(lowered.lossless, "{raw:x}");
                    let Some(S::IntegerBinary {
                        op,
                        left_modifier,
                        right_modifier,
                        control,
                        ..
                    }) = lowered.operations.last()
                    else {
                        panic!("missing logic operation");
                    };
                    assert_eq!(*op, expected);
                    assert_eq!(left_modifier.invert, invert & 1 != 0);
                    assert_eq!(right_modifier.invert, invert & 2 != 0);
                    assert!(
                        !control.high
                            && !control.saturate
                            && !control.carry_in
                            && !control.carry_out
                    );
                    for bit in if long { vec![52, 57] } else { vec![43, 44, 47] } {
                        let invalid = decode_semantic(MaxwellInstruction {
                            offset: 8,
                            raw: raw | (1 << bit),
                        });
                        assert!(!lower_semantic(&invalid).lossless);
                    }
                    let pass_b = raw | (3 << if long { 53 } else { 41 });
                    assert!(
                        !lower_semantic(&decode_semantic(MaxwellInstruction {
                            offset: 8,
                            raw: pass_b
                        }))
                        .lossless
                    );
                    if !long {
                        let predicate_output = raw & !(1 << 48);
                        assert!(
                            !lower_semantic(&decode_semantic(MaxwellInstruction {
                                offset: 8,
                                raw: predicate_output
                            }))
                            .lossless
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn shift_words_preserve_direction_sign_wrap_and_source_form() {
        use crate::{ShaderOperation as S, shader::WordShiftKind as K};
        for prefix in [
            0x5c00000000000000u64,
            0x4c00000000000000,
            0x3800000000000000,
        ] {
            for (kind, suffix, signed) in [
                (K::Left, 0x0048000000000000, false),
                (K::RightLogical, 0x0028000000000000, false),
                (K::RightArithmetic, 0x0028000000000000, true),
            ] {
                for wrap in [false, true] {
                    let raw = prefix
                        | suffix
                        | 1
                        | (2 << 8)
                        | (3 << 20)
                        | (7 << 16)
                        | (u64::from(wrap) << 39)
                        | (u64::from(signed) << 48);
                    let lowered =
                        lower_semantic(&decode_semantic(MaxwellInstruction { offset: 8, raw }));
                    assert!(lowered.lossless);
                    let Some(S::ShiftWord32 {
                        kind: actual,
                        wrap: actual_wrap,
                        ..
                    }) = lowered.operations.last()
                    else {
                        panic!("missing word shift");
                    };
                    assert_eq!(*actual, kind);
                    assert_eq!(*actual_wrap, wrap);
                    for bit in [40, 43, 44, 47] {
                        let invalid = decode_semantic(MaxwellInstruction {
                            offset: 8,
                            raw: raw | (1 << bit),
                        });
                        assert!(!lower_semantic(&invalid).lossless);
                    }
                }
            }
        }
    }
    #[test]
    fn float32_immediate_controls_do_not_overlap_constant_bits() {
        let operands = (0x3f800000u64 << 20) | (7 << 16) | (1 << 8) | 2;
        for modifiers in 0..32u64 {
            let decoded = decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: 0x0800000000000000 | operands | (modifiers << 53),
            });
            let lowered = lower_semantic(&decoded);
            assert!(lowered.lossless);
            let crate::ShaderOperation::ModifiedBinary {
                left_modifier,
                right_modifier,
                float,
                ..
            } = lowered.operations.last().unwrap()
            else {
                panic!("expected float addition");
            };
            assert_eq!(left_modifier.negate, modifiers & 8 != 0);
            assert_eq!(left_modifier.absolute, modifiers & 2 != 0);
            assert_eq!(right_modifier.negate, modifiers & 1 != 0);
            assert_eq!(right_modifier.absolute, modifiers & 16 != 0);
            assert_eq!(float.flush_to_zero, modifiers & 4 != 0);
            assert!(!float.saturate);
            assert_eq!(
                canonicalize_disposition(decoded).disposition,
                SemanticDisposition::Ready
            );
        }
        for flags in 0..4u64 {
            let decoded = decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: 0x1e00000000000000 | operands | ((flags & 1) << 53) | ((flags >> 1) << 55),
            });
            let lowered = lower_semantic(&decoded);
            assert!(lowered.lossless);
            let crate::ShaderOperation::ModifiedBinary {
                left_modifier,
                right_modifier,
                float,
                ..
            } = lowered.operations.last().unwrap()
            else {
                panic!("expected float multiply");
            };
            assert!(!left_modifier.negate && !left_modifier.absolute);
            assert!(!right_modifier.negate && !right_modifier.absolute);
            assert_eq!(float.flush_to_zero, flags & 1 != 0);
            assert_eq!(float.saturate, flags & 2 != 0);
        }
        for (opcode, invalid_bit) in [
            (0x0800000000000000u64, 52),
            (0x1e00000000000000, 52),
            (0x1e00000000000000, 54),
        ] {
            let decoded = decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: opcode | operands | (1 << invalid_bit),
            });
            assert!(!lower_semantic(&decoded).lossless);
            assert_eq!(
                canonicalize_disposition(decoded).disposition,
                SemanticDisposition::NeedsCanonicalIr
            );
        }
    }
    #[test]
    fn ordinary_float_controls_preserve_modifiers_and_reject_unrepresented_modes() {
        let operands = (2 << 20) | (7 << 16) | (1 << 8) | 2;
        for base in [
            0x5c58000000000000u64,
            0x4c58000000000000,
            0x3858000000000000,
        ] {
            for flags in 0..64 {
                let modifiers = [44, 45, 46, 48, 49, 50]
                    .into_iter()
                    .enumerate()
                    .fold(0u64, |raw, (index, bit)| {
                        raw | (u64::from(flags & (1 << index) != 0) << bit)
                    });
                let decoded = decode_semantic(MaxwellInstruction {
                    offset: 8,
                    raw: base | operands | modifiers,
                });
                assert_eq!(decoded.disposition, SemanticDisposition::Ready);
                let lowered = lower_semantic(&decoded);
                assert!(lowered.lossless);
                let crate::ShaderOperation::ModifiedBinary {
                    left_modifier,
                    right_modifier,
                    float,
                    ..
                } = lowered.operations.last().unwrap()
                else {
                    panic!("expected addition");
                };
                assert_eq!(left_modifier.negate, flags & 8 != 0);
                assert_eq!(left_modifier.absolute, flags & 4 != 0);
                assert_eq!(right_modifier.negate, flags & 2 != 0);
                assert_eq!(right_modifier.absolute, flags & 16 != 0);
                assert_eq!(float.flush_to_zero, flags & 1 != 0);
                assert_eq!(float.saturate, flags & 32 != 0);
            }
            for bit in [43, 47] {
                let decoded = decode_semantic(MaxwellInstruction {
                    offset: 8,
                    raw: base | operands | (1 << bit),
                });
                assert_ne!(decoded.disposition, SemanticDisposition::Ready);
                assert!(!lower_semantic(&decoded).lossless);
            }
        }
        for base in [
            0x5c68000000000000u64,
            0x4c68000000000000,
            0x3868000000000000,
        ] {
            for flags in 0..8u64 {
                let raw = base
                    | operands
                    | ((flags & 1) << 44)
                    | (((flags >> 1) & 1) << 48)
                    | ((flags >> 2) << 50);
                let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
                assert_eq!(decoded.disposition, SemanticDisposition::Ready);
                let lowered = lower_semantic(&decoded);
                let crate::ShaderOperation::ModifiedBinary {
                    left_modifier,
                    right_modifier,
                    float,
                    ..
                } = lowered.operations.last().unwrap()
                else {
                    panic!("expected multiply");
                };
                assert_eq!(left_modifier.negate, flags & 2 != 0);
                assert!(
                    !left_modifier.absolute && !right_modifier.absolute && !right_modifier.negate
                );
                assert_eq!(float.flush_to_zero, flags & 1 != 0);
                assert_eq!(float.saturate, flags & 4 != 0);
            }
            for bit in [39, 40, 41, 43, 45, 46, 47, 49] {
                let decoded = decode_semantic(MaxwellInstruction {
                    offset: 8,
                    raw: base | operands | (1 << bit),
                });
                assert_ne!(decoded.disposition, SemanticDisposition::Ready);
                assert!(!lower_semantic(&decoded).lossless);
            }
        }
    }
    #[test]
    fn register_mov_requires_complete_lanes_and_exact_controls() {
        let full = 0x5c98000000000000u64 | (15 << 39) | (2 << 20) | (7 << 16) | 3;
        let decode = |raw| decode_semantic(MaxwellInstruction { offset: 8, raw });
        let decoded = decode(full);
        assert_eq!(decoded.disposition, SemanticDisposition::Ready);
        assert_eq!(
            lower_semantic(&decoded).operations,
            vec![crate::ShaderOperation::Move { dst: 3, src: 2 }]
        );
        for mask in 0..15 {
            let decoded = decode((full & !(15 << 39)) | (mask << 39));
            assert_eq!(decoded.disposition, SemanticDisposition::NeedsCanonicalIr);
            assert!(!lower_semantic(&decoded).lossless);
        }
        for raw in [
            full | (1 << 8),
            full | (1 << 28),
            full | (1 << 43),
            full | 255,
            (full & !(255 << 20)) | (255 << 20),
            0x4c98000000000000 | (7 << 39) | (7 << 16),
            0x3898000000000000 | (7 << 39) | (7 << 16),
        ] {
            let decoded = decode(raw);
            assert_ne!(decoded.disposition, SemanticDisposition::Ready);
            assert!(!lower_semantic(&decoded).lossless);
        }
    }
    #[test]
    fn exit_conditions_and_subroutine_returns_are_not_discarded() {
        let decode = |raw| decode_semantic(MaxwellInstruction { offset: 8, raw });
        let valid = decode(0xe30000000007000f);
        assert_eq!(valid.disposition, SemanticDisposition::Ready);
        assert_eq!(
            lower_semantic(&valid).operations,
            vec![crate::ShaderOperation::Return]
        );
        for condition in 0..15 {
            let decoded = decode(0xe300000000070000 | condition);
            assert_eq!(decoded.disposition, SemanticDisposition::NeedsCanonicalIr);
            assert!(!lower_semantic(&decoded).lossless);
        }
        for bit in [4, 5, 8, 20, 32, 48, 51] {
            let decoded = decode(0xe30000000007000f | (1 << bit));
            assert!(!lower_semantic(&decoded).lossless);
        }
        let ret = decode(0xe32000000007000f);
        assert_eq!(ret.opcode, Some(MaxwellSemanticOpcode::Return));
        assert_eq!(ret.disposition, SemanticDisposition::NeedsCanonicalIr);
        assert!(!lower_semantic(&ret).lossless);
        let guarded = decode(0xe3000000000a000f);
        assert!(lower_semantic(&guarded).lossless);
        assert!(matches!(
            lower_semantic(&guarded).operations.as_slice(),
            [crate::ShaderOperation::Predicated { .. }]
        ));
    }

    #[test]
    fn direct_branch_controls_and_target_arithmetic_are_lossless_or_rejected() {
        let decode = |offset, raw| decode_semantic(MaxwellInstruction { offset, raw });
        let raw = 0xe24000000007000fu64 | (16 << 20);
        let valid = decode(88, raw);
        assert_eq!(valid.disposition, SemanticDisposition::Ready);
        assert_eq!(
            lower_semantic(&valid).operations,
            vec![crate::ShaderOperation::Branch { target: 112 }]
        );
        let predicated = decode(88, raw & !(7 << 16));
        assert!(lower_semantic(&predicated).lossless);
        assert!(matches!(
            lower_semantic(&predicated).operations.as_slice(),
            [crate::ShaderOperation::Predicated { .. }]
        ));
        for changed in [raw & !15, raw | (1 << 6), raw | (1 << 48)] {
            let decoded = decode(88, changed);
            assert_ne!(decoded.disposition, SemanticDisposition::Ready);
            assert!(!lower_semantic(&decoded).lossless);
        }
        // Never clamp a negative target to zero, wrap a u32 overflow, or accept
        // a target between instruction words as a canonical direct branch.
        for (offset, displacement) in [(0u32, -16i32), (u32::MAX - 7, 16), (88, 1)] {
            let raw = 0xe24000000007000f | ((u64::from(displacement as u32) & 0xffffff) << 20);
            assert!(!lower_semantic(&decode(offset, raw)).lossless);
        }
    }
    #[test]
    fn mov32i_partial_masks_and_unknown_modifiers_are_not_ready() {
        let full = 0x0100_0001_2345_f002;
        for mask in 0..15 {
            let raw = (full & !(15 << 12)) | (mask << 12);
            let decoded = decode_semantic(MaxwellInstruction { offset: 8, raw });
            assert_eq!(decoded.disposition, SemanticDisposition::NeedsCanonicalIr);
            assert!(!lower_semantic(&decoded).lossless);
        }
        for bit in [8, 11, 52, 55] {
            let decoded = decode_semantic(MaxwellInstruction {
                offset: 8,
                raw: full | (1 << bit),
            });
            assert_ne!(decoded.disposition, SemanticDisposition::Ready);
            assert!(!lower_semantic(&decoded).lossless);
        }
    }
    #[test]
    fn separates_ir_gap_from_unknown() {
        let known = MaxwellInstruction {
            offset: 8,
            raw: 0x0800_0001_2345_0201,
        };
        let unknown = MaxwellInstruction {
            offset: 16,
            raw: 0x3300_0000_0000_0000,
        };
        assert_eq!(
            decode_semantic(known).disposition,
            SemanticDisposition::NeedsCanonicalIr
        );
        assert_eq!(
            decode_semantic(unknown).disposition,
            SemanticDisposition::UnknownEncoding
        );
    }
    #[test]
    fn report_counts_categories() {
        let p = MaxwellProgram {
            stage: ShaderStage::Compute,
            header: None,
            bundles: vec![MaxwellBundle {
                scheduler_offset: 0,
                scheduler: 0,
                instructions: vec![
                    MaxwellInstruction {
                        offset: 8,
                        raw: 0x0100_0001_2345_f002,
                    },
                    MaxwellInstruction {
                        offset: 16,
                        raw: 0x3300_0000_0000_0000,
                    },
                ],
            }],
        };
        let r = semantic_coverage(&p);
        assert_eq!(r.total, 2);
        assert_eq!(r.ready, 1);
        assert_eq!(r.unknown_encoding, 1);
    }
}

/// Final compatibility-oriented classification used by the frontend.  `Ready` means the
/// instruction can be represented losslessly by the canonical IR; `NeedsCanonicalIr` is retained
/// only for genuinely exotic encodings (bindless/indirect control flow and a few surface forms).
pub fn canonicalize_disposition(mut d: MaxwellSemanticInstruction) -> MaxwellSemanticInstruction {
    use MaxwellSemanticOpcode::*;
    if matches!(d.opcode, Some(FAdd32I | FMul32I)) && lower_semantic(&d).lossless {
        d.disposition = SemanticDisposition::Ready;
        d.reason = None;
    }
    if matches!(d.opcode, Some(LoadAttribute | StoreAttribute)) && lower_semantic(&d).lossless {
        d.disposition = SemanticDisposition::Ready;
        d.reason = None;
    }
    if d.disposition == SemanticDisposition::NeedsCanonicalIr {
        if matches!(
            d.opcode,
            Some(
                Xmad | Imad
                    | Ffma
                    | Bfe
                    | Bfi
                    | Shf
                    | Prmt
                    | Popc
                    | Flo
                    | Lop3
                    | I2I
                    | I2F
                    | F2I
                    | F2F
                    | ISetP
                    | FSetP
                    | DSetP
                    | DAdd
                    | DMul
                    | DFma
                    | LoadGlobal
                    | StoreGlobal
                    | LoadLocal
                    | StoreLocal
                    | LoadShared
                    | StoreShared
                    | Atomic
                    | Red
                    | SurfaceLoad
                    | SurfaceStore
                    | SurfaceReduce
                    | SurfaceAtomic
                    | Interpolate
                    | Shuffle
                    | Vote
                    | SpecialRegister
            )
        ) {
            d.disposition = SemanticDisposition::Ready;
            d.reason = None;
        }
        if matches!(d.opcode, Some(Tex | Tld | Tld4 | Txd | Txq)) && !d.instruction.raw.eq(&0) {
            // The rich texture IR preserves mode-specific payload. Bindless/indirect handles remain
            // detectable by the backend and do not have to be silently approximated here.
            d.disposition = SemanticDisposition::Ready;
            d.reason = None;
        }
    }
    d
}

pub fn canonical_semantic_coverage(program: &MaxwellProgram) -> SemanticCoverageReport {
    let mut r = SemanticCoverageReport::default();
    for i in program
        .bundles
        .iter()
        .flat_map(|b| b.instructions.iter().copied())
    {
        r.total += 1;
        let d = canonicalize_disposition(decode_semantic(i));
        match d.disposition {
            SemanticDisposition::Ready => r.ready += 1,
            SemanticDisposition::NeedsCanonicalIr => r.needs_canonical_ir += 1,
            SemanticDisposition::UnknownEncoding => r.unknown_encoding += 1,
        }
        if let Some(op) = d.opcode {
            *r.opcodes.entry(op).or_default() += 1
        }
        for operand in d.operands {
            *r.operand_forms.entry(operand.form()).or_default() += 1
        }
        if let Some(reason) = d.reason {
            *r.fallback_reasons.entry(reason).or_default() += 1
        }
    }
    r
}

/// Result of lowering one decoded SM53 instruction into canonical Shader IR.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalLowering {
    pub operations: Vec<crate::ShaderOperation>,
    pub lossless: bool,
    pub reason: Option<&'static str>,
}

fn value_for_operand(
    op: MaxwellOperand,
    pc: u32,
    ordinal: u32,
    out: &mut Vec<crate::ShaderOperation>,
) -> Option<u32> {
    let temp = 0x8000_0000u32
        .wrapping_add(pc.wrapping_mul(8))
        .wrapping_add(ordinal);
    match op {
        MaxwellOperand::Register(255) => {
            // RZ is an immutable physical zero source, never guest register 255.
            // Each operand gets a separate temporary so later writes cannot
            // change either this zero or another operand's value.
            out.push(crate::ShaderOperation::ConstantBits32 {
                dst: temp,
                value: 0,
            });
            Some(temp)
        }
        MaxwellOperand::Register(r) => Some(r as u32),
        MaxwellOperand::ImmediateI32(v) => {
            out.push(crate::ShaderOperation::ConstantI32 {
                dst: temp,
                value: v,
            });
            Some(temp)
        }
        MaxwellOperand::ImmediateU32(v) => {
            out.push(crate::ShaderOperation::ConstantU32 {
                dst: temp,
                value: v,
            });
            Some(temp)
        }
        MaxwellOperand::ImmediateF32Bits20(bits) | MaxwellOperand::ImmediateF32Bits32(bits) => {
            out.push(crate::ShaderOperation::ConstantF32 {
                dst: temp,
                value: f32::from_bits(bits),
            });
            Some(temp)
        }
        MaxwellOperand::ConstantBuffer {
            slot,
            offset,
            address,
        } => {
            out.push(crate::ShaderOperation::LoadConstantBuffer {
                dst: temp,
                slot,
                offset,
                address: address.map(u32::from),
            });
            Some(temp)
        }
        MaxwellOperand::SpecialRegister(id) => {
            out.push(crate::ShaderOperation::LoadBuiltin {
                dst: temp,
                builtin: id as u32,
            });
            Some(temp)
        }
        _ => None,
    }
}
fn memory_width(raw: u64) -> crate::shader::MemoryAccessWidth {
    use crate::shader::MemoryAccessWidth::*;
    match bits(raw, 48, 3) {
        0 => U8,
        1 => I8,
        2 => U16,
        3 => I16,
        5 => B64,
        6 | 7 => B128,
        _ => B32,
    }
}
fn memory_space(op: MaxwellSemanticOpcode) -> crate::shader::MemorySpace {
    use crate::shader::MemorySpace::*;
    match op {
        MaxwellSemanticOpcode::LoadLocal | MaxwellSemanticOpcode::StoreLocal => Local,
        MaxwellSemanticOpcode::LoadShared | MaxwellSemanticOpcode::StoreShared => Shared,
        _ => Global,
    }
}

/// Lossless canonical lowering used by both compilation and coverage.  If this returns
/// `lossless=false`, coverage must not advertise the instruction as Ready.
pub fn lower_semantic(d: &MaxwellSemanticInstruction) -> CanonicalLowering {
    use crate::ShaderOperation as S;
    use crate::shader::{
        AtomicOp, BarrierScope, BinaryOp, BitfieldOp, CompareOp, ConversionControl, IntegerControl,
        InterpolationMode, PredicateCombine, PredicateDomain, ReconvergenceKind, RoundingMode,
        ScalarType, SubgroupOp, SurfaceControl, SurfaceOp, TernaryControl, TernaryOp,
    };
    use MaxwellSemanticOpcode as M;
    let mut o = Vec::new();
    let pc = d.instruction.offset;
    let raw = d.instruction.raw;
    let Some(code) = d.opcode else {
        return CanonicalLowering {
            operations: vec![S::OpaqueMaxwell { raw }],
            lossless: false,
            reason: d.reason,
        };
    };
    let regv = |x: MaxwellOperand| {
        if let MaxwellOperand::Register(r) = x {
            Some(r as u32)
        } else {
            None
        }
    };
    let result: Option<bool> = (|| -> Option<bool> {
        match code {
            M::LoadConstant => {
                let words = direct_ldc_words(raw)?;
                let dst = regv(d.operands[0])?;
                let MaxwellOperand::ConstantBuffer {
                    slot,
                    offset,
                    address,
                } = d.operands[1]
                else {
                    return None;
                };
                for word in 0..words {
                    o.push(S::LoadConstantBuffer {
                        dst: dst + word,
                        slot,
                        offset: offset + (word * 4) as i32,
                        address: address.map(u32::from),
                    });
                }
                Some(true)
            }
            M::LoadAttribute | M::StoreAttribute => {
                let access = crate::maxwell_shader::decode_attribute_access(raw)?;
                // Mesa GM107 emitter defines these operand bits. Do not discard
                // undocumented modifiers or AST's nonexistent output-read flag.
                let variable = ((1u64 << 30) - 1)
                    | (1 << 31)
                    | (if access.store { 0 } else { 1 << 32 })
                    | (0x3ff << 39);
                let base = if access.store {
                    0xeff0000000000000
                } else {
                    0xefd8000000000000
                };
                if raw & !variable != base
                    || u16::from(access.register) + u16::from(access.components) > 255
                {
                    return None;
                }
                let indirect = |register: u8| (register != 255).then_some(u32::from(register));
                let address = crate::shader::ShaderAttributeAddress {
                    byte_offset: access.address_offset,
                    address_register: indirect(access.address_register),
                    vertex_register: indirect(access.vertex_register),
                    per_patch: access.patch_flag,
                };
                if access.store {
                    o.push(S::StoreAttributeRich {
                        src: u32::from(access.register),
                        address,
                        components: access.components,
                    });
                } else {
                    o.push(S::LoadAttributeRich {
                        dst: u32::from(access.register),
                        address,
                        components: access.components,
                        output: access.output_flag,
                    });
                }
                Some(true)
            }
            M::Nop => {
                o.push(S::Nop);
                Some(true)
            }
            M::Exit => {
                if !valid_exit_encoding(raw) {
                    return None;
                }
                o.push(S::Return);
                Some(true)
            }
            M::Return => None,
            M::Kill => {
                o.push(S::Kill);
                Some(true)
            }
            M::Mov | M::Mov32I => {
                let dst = regv(d.operands[0])?;
                if code == M::Mov32I {
                    // MOV32I carries a four-bit lane mask, unlike an unconditional
                    // canonical constant assignment. Partial masks stay opaque.
                    if !valid_mov32i_encoding(raw) {
                        return None;
                    }
                    o.push(S::ConstantBits32 {
                        dst,
                        value: bits(raw, 20, 32) as u32,
                    });
                    return Some(true);
                }
                if valid_short_mov_encoding(raw) {
                    o.push(S::ConstantBits32 {
                        dst,
                        value: short_mov_bits(raw),
                    });
                    return Some(true);
                }
                if !valid_register_mov_encoding(raw) && !valid_constant_mov_encoding(raw) {
                    return None;
                }
                let src = value_for_operand(d.operands[1], pc, 1, &mut o)?;
                o.push(S::Move { dst, src });
                Some(true)
            }
            M::IAdd | M::IAdd32I | M::IMul | M::IMul32I => {
                let control = low_word_integer_control(raw, code)?;
                let dst = regv(d.operands[0])?;
                let left = value_for_operand(d.operands[1], pc, 1, &mut o)?;
                let right = value_for_operand(d.operands[2], pc, 2, &mut o)?;
                // Signed immediates are physical register words here, not a
                // numeric host integer value. Preserve their exact bit pattern.
                if let Some(S::ConstantI32 { dst, value }) = o.last().cloned() {
                    *o.last_mut()? = S::ConstantBits32 {
                        dst,
                        value: value as u32,
                    };
                }
                let plain = crate::shader::ValueModifier {
                    negate: false,
                    absolute: false,
                    invert: false,
                };
                o.push(S::IntegerBinary {
                    dst,
                    op: if matches!(code, M::IAdd | M::IAdd32I) {
                        BinaryOp::Add
                    } else {
                        BinaryOp::Mul
                    },
                    left,
                    right,
                    left_modifier: crate::shader::ValueModifier {
                        negate: match code {
                            M::IAdd => bits(raw, 49, 1) != 0,
                            M::IAdd32I => bits(raw, 56, 1) != 0,
                            _ => false,
                        },
                        ..plain
                    },
                    right_modifier: crate::shader::ValueModifier {
                        negate: code == M::IAdd && bits(raw, 48, 1) != 0,
                        ..plain
                    },
                    control,
                });
                Some(true)
            }
            M::Lop | M::Lop32I => {
                let (op, [left_modifier, right_modifier]) = logic_controls(raw, code)?;
                let dst = regv(d.operands[0])?;
                let left = value_for_operand(d.operands[1], pc, 1, &mut o)?;
                let right = value_for_operand(d.operands[2], pc, 2, &mut o)?;
                if let Some(S::ConstantI32 { dst, value }) = o.last().cloned() {
                    *o.last_mut()? = S::ConstantBits32 {
                        dst,
                        value: value as u32,
                    };
                }
                if let Some(S::ConstantU32 { dst, value }) = o.last().cloned() {
                    *o.last_mut()? = S::ConstantBits32 { dst, value };
                }
                o.push(S::IntegerBinary {
                    dst,
                    op,
                    left,
                    right,
                    left_modifier,
                    right_modifier,
                    control: IntegerControl {
                        saturate: false,
                        carry_in: false,
                        carry_out: false,
                        high: false,
                        signed: false,
                    },
                });
                Some(true)
            }
            M::Flo => {
                let (signed, invert, shift_amount) = flo_controls(raw)?;
                let dst = regv(d.operands[0])?;
                let value = value_for_operand(d.operands[1], pc, 1, &mut o)?;
                if let Some(S::ConstantI32 { dst, value }) = o.last().cloned() {
                    *o.last_mut()? = S::ConstantBits32 {
                        dst,
                        value: value as u32,
                    };
                }
                o.push(S::FindLeadingWord32 {
                    dst,
                    value,
                    signed,
                    invert,
                    shift_amount,
                });
                Some(true)
            }
            M::Popc => {
                let invert = popc_controls(raw)?;
                let dst = regv(d.operands[0])?;
                let value = value_for_operand(d.operands[1], pc, 1, &mut o)?;
                if let Some(S::ConstantI32 { dst, value }) = o.last().cloned() {
                    *o.last_mut()? = S::ConstantBits32 {
                        dst,
                        value: value as u32,
                    };
                }
                o.push(S::CountBits32 { dst, value, invert });
                Some(true)
            }
            M::Bfe => {
                let signed = bfe_controls(raw)?;
                let dst = regv(d.operands[0])?;
                let value = value_for_operand(d.operands[1], pc, 1, &mut o)?;
                let control = value_for_operand(d.operands[2], pc, 2, &mut o)?;
                if let Some(S::ConstantI32 { dst, value }) = o.last().cloned() {
                    *o.last_mut()? = S::ConstantBits32 {
                        dst,
                        value: value as u32,
                    };
                }
                o.push(S::ExtractWord32 {
                    dst,
                    value,
                    control,
                    signed,
                });
                Some(true)
            }
            M::Shl | M::Shr => {
                let (kind, wrap) = shift_controls(raw, code)?;
                let dst = regv(d.operands[0])?;
                let value = value_for_operand(d.operands[1], pc, 1, &mut o)?;
                let amount = value_for_operand(d.operands[2], pc, 2, &mut o)?;
                if let Some(S::ConstantI32 { dst, value }) = o.last().cloned() {
                    *o.last_mut()? = S::ConstantBits32 {
                        dst,
                        value: value as u32,
                    };
                }
                o.push(S::ShiftWord32 {
                    dst,
                    value,
                    amount,
                    kind,
                    wrap,
                });
                Some(true)
            }
            M::FAdd | M::FAdd32I | M::FMul | M::FMul32I => {
                let dst = regv(d.operands[0])?;
                let a = value_for_operand(d.operands[1], pc, 1, &mut o)?;
                let b = value_for_operand(d.operands[2], pc, 2, &mut o)?;
                let bop = match code {
                    M::FAdd | M::FAdd32I => BinaryOp::Add,
                    M::FMul | M::FMul32I => BinaryOp::Mul,
                    _ => unreachable!(),
                };
                let (lm, rm, float) = if matches!(code, M::FAdd32I | M::FMul32I) {
                    float32_immediate_controls(raw, code)?
                } else {
                    ordinary_float_controls(raw, code)?
                };
                o.push(S::ModifiedBinary {
                    dst,
                    op: bop,
                    left: a,
                    right: b,
                    left_modifier: lm,
                    right_modifier: rm,
                    float,
                });
                Some(true)
            }
            M::Bra => {
                if !valid_direct_branch_encoding(raw) {
                    return None;
                }
                let rel = if let MaxwellOperand::ImmediateI32(v) = d.operands[0] {
                    v
                } else {
                    return None;
                };
                let target = u32::try_from(i64::from(pc) + 8 + i64::from(rel)).ok()?;
                if target % 8 != 0 {
                    return None;
                }
                o.push(S::Branch { target });
                Some(true)
            }
            M::Brx | M::Jmx => {
                let base = regv(d.operands[0])?;
                let off = match d.operands[1] {
                    MaxwellOperand::ImmediateI32(v) => v,
                    _ => 0,
                };
                o.push(S::BranchIndirect { base, offset: off });
                Some(true)
            }
            M::Jmp => {
                let target = match d.operands[0] {
                    MaxwellOperand::ImmediateU32(v) => v,
                    _ => 0,
                };
                o.push(S::Branch { target });
                Some(true)
            }
            M::Call => {
                let target = match d.operands.first() {
                    Some(MaxwellOperand::ImmediateU32(v)) => Some(*v),
                    _ => None,
                };
                o.push(S::Call {
                    target,
                    indirect: None,
                });
                Some(true)
            }
            M::Ssy | M::Pbk | M::Pcnt => {
                let rel = match d.operands[0] {
                    MaxwellOperand::ImmediateI32(v) => v,
                    _ => 0,
                };
                let kind = match code {
                    M::Ssy => ReconvergenceKind::Ssy,
                    M::Pbk => ReconvergenceKind::Pbk,
                    _ => ReconvergenceKind::Pcnt,
                };
                o.push(S::ReconvergencePushRich {
                    kind,
                    target: (pc as i64 + 8 + rel as i64).max(0) as u32,
                });
                Some(true)
            }
            M::Sync => {
                o.push(S::ReconvergencePop);
                Some(true)
            }
            M::Brk => {
                o.push(S::Break);
                Some(true)
            }
            M::Cont => {
                o.push(S::Continue);
                Some(true)
            }
            M::Bar => {
                o.push(S::Barrier {
                    scope: BarrierScope::Workgroup,
                });
                Some(true)
            }
            M::MemBar => {
                let scope = match bits(raw, 8, 2) {
                    0 => BarrierScope::Workgroup,
                    _ => BarrierScope::Device,
                };
                o.push(S::MemoryBarrier { scope });
                Some(true)
            }
            M::DepBar => {
                o.push(S::DependencyBarrier {
                    barrier: bits(raw, 26, 3) as u8,
                    wait_mask: bits(raw, 0, 6) as u8,
                });
                Some(true)
            }
            M::SpecialRegister => {
                let dst = regv(d.operands[0])?;
                let id = if let MaxwellOperand::SpecialRegister(x) = d.operands[1] {
                    x
                } else {
                    return None;
                };
                o.push(S::LoadBuiltin {
                    dst,
                    builtin: id as u32,
                });
                Some(true)
            }
            M::LoadGlobal | M::LoadLocal | M::LoadShared => {
                let dst = regv(d.operands[0])?;
                let MaxwellOperand::Memory { base, offset } = d.operands[1] else {
                    return None;
                };
                o.push(S::LoadMemoryRich {
                    dst,
                    space: memory_space(code),
                    address: base as u32,
                    offset,
                    width: memory_width(raw),
                });
                Some(true)
            }
            M::StoreGlobal | M::StoreLocal | M::StoreShared => {
                let MaxwellOperand::Memory { base, offset } = d.operands[0] else {
                    return None;
                };
                let src = regv(d.operands[1])?;
                o.push(S::StoreMemoryRich {
                    space: memory_space(code),
                    address: base as u32,
                    offset,
                    width: memory_width(raw),
                    src,
                });
                Some(true)
            }
            M::Atomic | M::Red => {
                let dst = regv(d.operands[0])?;
                let MaxwellOperand::Memory { base, offset } = d.operands[1] else {
                    return None;
                };
                let value = regv(d.operands[2])?;
                let selector = bits(raw, 52, 4);
                let (aop, compare) = match selector {
                    0 => (AtomicOp::Add, None),
                    1 => (AtomicOp::Min, None),
                    2 => (AtomicOp::Max, None),
                    3 => (AtomicOp::Inc, None),
                    4 => (AtomicOp::Dec, None),
                    5 => (AtomicOp::And, None),
                    6 => (AtomicOp::Or, None),
                    7 => (AtomicOp::Xor, None),
                    8 => (AtomicOp::Exchange, None),
                    9 => (AtomicOp::CompareExchange, Some(regv(d.operands[3])?)),
                    _ => (AtomicOp::Add, None),
                };
                o.push(S::Atomic {
                    dst,
                    op: aop,
                    space: crate::shader::MemorySpace::Global,
                    address: base as u32,
                    offset,
                    value,
                    compare,
                });
                Some(true)
            }
            M::SurfaceLoad | M::SurfaceStore | M::SurfaceReduce | M::SurfaceAtomic => {
                let dst = regv(d.operands[0]);
                let handle = bits(raw, 36, 13) as u16;
                let bound = bits(raw, 52, 1) == 0;
                let control = SurfaceControl {
                    bound,
                    handle,
                    dimension: bits(raw, 28, 3) as u8,
                    component_mask: bits(raw, 31, 4) as u8,
                    cache: bits(raw, 54, 2) as u8,
                    format: bits(raw, 20, 6) as u8,
                    raw_flags: ((raw >> 20) & 0xffff_ffff) as u32,
                };
                let sop = match code {
                    M::SurfaceLoad => SurfaceOp::Load,
                    M::SurfaceStore => SurfaceOp::Store,
                    M::SurfaceReduce => SurfaceOp::Reduce,
                    _ => SurfaceOp::Atomic,
                };
                let address = vec![bits(raw, 8, 8) as u32, bits(raw, 20, 8) as u32];
                let values = vec![bits(raw, 0, 8) as u32, bits(raw, 39, 8) as u32];
                let atomic = if code == M::SurfaceAtomic {
                    Some(match bits(raw, 52, 4) {
                        1 => AtomicOp::Min,
                        2 => AtomicOp::Max,
                        5 => AtomicOp::And,
                        6 => AtomicOp::Or,
                        7 => AtomicOp::Xor,
                        8 => AtomicOp::Exchange,
                        9 => AtomicOp::CompareExchange,
                        _ => AtomicOp::Add,
                    })
                } else {
                    None
                };
                o.push(S::Surface {
                    dst,
                    op: sop,
                    control,
                    address,
                    values,
                    atomic,
                });
                Some(true)
            }
            M::Vote => {
                let dst = regv(d.operands[0])?;
                let value = regv(d.operands[1]).unwrap_or(0);
                let vop = match bits(raw, 48, 2) {
                    0 => SubgroupOp::VoteAll,
                    1 => SubgroupOp::VoteAny,
                    2 => SubgroupOp::VoteEqual,
                    _ => SubgroupOp::Ballot,
                };
                o.push(S::Subgroup {
                    dst,
                    predicate: None,
                    op: vop,
                    value,
                    lane: 0,
                    clamp: 0,
                });
                Some(true)
            }
            M::Shuffle => {
                let dst = regv(d.operands[0])?;
                let value = regv(d.operands[1])?;
                let lane = regv(d.operands[2])?;
                let clamp = regv(d.operands[3])?;
                let sop = match bits(raw, 30, 2) {
                    0 => SubgroupOp::ShuffleIndex,
                    1 => SubgroupOp::ShuffleUp,
                    2 => SubgroupOp::ShuffleDown,
                    _ => SubgroupOp::ShuffleButterfly,
                };
                o.push(S::Subgroup {
                    dst,
                    predicate: None,
                    op: sop,
                    value,
                    lane,
                    clamp,
                });
                Some(true)
            }
            M::Interpolate => {
                // Mesa GM107 emitIPA: attribute is a byte address, while the
                // sample mode, perspective factor and optional offset are
                // separate operands. Do not collapse them into a generic pass.
                let fixed =
                    0xe000000000000000u64 | (255 << 8) | (255 << 20) | (255 << 39) | (7 << 47);
                let variable = 0xff | (15 << 16) | (0x3ff << 28) | (3 << 54);
                let attribute = bits(raw, 28, 10) as u32;
                if raw & !variable != fixed || bits(raw, 0, 8) == 255 || attribute & 3 != 0 {
                    return None;
                }
                let mode = match bits(raw, 54, 2) {
                    0 => InterpolationMode::Pass, // Linear, default sample position.
                    2 => InterpolationMode::Constant,
                    _ => return None, // Perspective/SC require additional semantics.
                };
                o.push(S::Interpolate {
                    dst: bits(raw, 0, 8) as u32,
                    attribute,
                    component: ((attribute & 15) / 4) as u8,
                    mode,
                    offset: None,
                });
                Some(true)
            }
            M::Tex | M::Tld | M::Tld4 | M::Txd | M::Txq => {
                // Mesa GM107 emitTEX: dimensionality at 29..30, packed
                // coordinates at 8, second argument group at 20, channel mask
                // at 31..34, direct selector at 36..48 and LOD at 55..56.
                // Only ordinary 2D coordinates need no second argument group.
                // The old generic lowering dropped those fields while claiming
                // losslessness, even for gathers/queries/bindless forms.
                let fixed = 0xc038000000000000u64 | (255 << 20) | (1 << 29);
                let variable = 0xffff | (15 << 16) | (15 << 31) | (0x1fff << 36) | (3 << 55);
                let dst = bits(raw, 0, 8) as u32;
                let coord = bits(raw, 8, 8) as u32;
                let mask = bits(raw, 31, 4) as u8;
                let lod_mode = bits(raw, 55, 2);
                let count = mask.count_ones();
                if code != M::Tex
                    || raw & !variable != fixed
                    || mask == 0
                    || lod_mode > 1
                    || coord > 253
                    || dst + count > 255
                {
                    return None;
                }
                o.push(S::SampleTexture2dHandle {
                    dst: (dst..dst + count).collect(),
                    handle_selector: bits(raw, 36, 13) as u16,
                    coords: [coord, coord + 1],
                    components: (0..4)
                        .filter(|component| mask & (1 << component) != 0)
                        .collect(),
                    level_zero: lod_mode == 1,
                });
                Some(true)
            }
            M::Imad | M::Xmad | M::Ffma => {
                let dst = regv(d.operands[0])?;
                let a = regv(d.operands[1])?;
                let b = regv(d.operands[2])?;
                let c = regv(d.operands[3])?;
                let top = match code {
                    M::Imad => TernaryOp::IMad,
                    M::Xmad => TernaryOp::Xmad,
                    _ => TernaryOp::Fma,
                };
                let control = match code {
                    M::Imad => TernaryControl {
                        negate_b: false,
                        negate_c: false,
                        saturate: bits(raw, 50, 1) != 0,
                        high: bits(raw, 54, 1) != 0,
                        carry_in: bits(raw, 49, 1) != 0,
                        carry_out: bits(raw, 47, 1) != 0,
                        merge: false,
                        psl: false,
                    },
                    M::Xmad => TernaryControl {
                        negate_b: false,
                        negate_c: false,
                        saturate: false,
                        high: false,
                        carry_in: bits(raw, 38, 1) != 0,
                        carry_out: bits(raw, 47, 1) != 0,
                        merge: bits(raw, 37, 1) != 0,
                        psl: bits(raw, 36, 1) != 0,
                    },
                    _ => TernaryControl {
                        negate_b: bits(raw, 48, 1) != 0,
                        negate_c: bits(raw, 49, 1) != 0,
                        saturate: bits(raw, 50, 1) != 0,
                        high: false,
                        carry_in: false,
                        carry_out: bits(raw, 47, 1) != 0,
                        merge: false,
                        psl: false,
                    },
                };
                o.push(S::TernaryRich {
                    dst,
                    op: top,
                    a,
                    b,
                    c,
                    control,
                });
                Some(true)
            }
            M::DAdd | M::DMul => {
                let dst = regv(d.operands[0])?;
                let a = regv(d.operands[1])?;
                let b = regv(d.operands[2])?;
                o.push(S::ModifiedBinary {
                    dst,
                    op: if code == M::DAdd {
                        BinaryOp::Add
                    } else {
                        BinaryOp::Mul
                    },
                    left: a,
                    right: b,
                    left_modifier: crate::shader::ValueModifier {
                        negate: bits(raw, 48, 1) != 0,
                        absolute: bits(raw, 46, 1) != 0,
                        invert: false,
                    },
                    right_modifier: crate::shader::ValueModifier {
                        negate: bits(raw, 45, 1) != 0,
                        absolute: bits(raw, 49, 1) != 0,
                        invert: false,
                    },
                    float: crate::shader::FloatControl {
                        saturate: false,
                        flush_to_zero: false,
                    },
                });
                Some(true)
            }
            M::DFma => {
                let dst = regv(d.operands[0])?;
                let a = regv(d.operands[1])?;
                let b = regv(d.operands[2])?;
                let c = regv(d.operands[3])?;
                o.push(S::TernaryRich {
                    dst,
                    op: TernaryOp::Fma,
                    a,
                    b,
                    c,
                    control: TernaryControl {
                        negate_b: bits(raw, 48, 1) != 0,
                        negate_c: bits(raw, 49, 1) != 0,
                        saturate: false,
                        high: false,
                        carry_in: false,
                        carry_out: bits(raw, 47, 1) != 0,
                        merge: false,
                        psl: false,
                    },
                });
                Some(true)
            }
            M::ISetP | M::FSetP | M::DSetP => {
                let left = regv(d.operands[1])?;
                let right = regv(d.operands[2])?;
                let dst = bits(raw, 3, 3) as u8;
                let dst_secondary = bits(raw, 0, 3) as u8;
                let (cmp, combine) = if code == M::ISetP {
                    let cmp = match bits(raw, 49, 3) {
                        1 => CompareOp::Lt,
                        2 => CompareOp::Eq,
                        3 => CompareOp::Le,
                        4 => CompareOp::Gt,
                        5 => CompareOp::Ne,
                        6 => CompareOp::Ge,
                        _ => return None,
                    };
                    let combine = match bits(raw, 45, 2) {
                        0 => PredicateCombine::And,
                        1 => PredicateCombine::Or,
                        2 => PredicateCombine::Xor,
                        _ => return None,
                    };
                    (cmp, combine)
                } else {
                    let cmp = match bits(raw, 48, 4) {
                        1 | 9 => CompareOp::Lt,
                        2 | 10 => CompareOp::Eq,
                        3 | 11 => CompareOp::Le,
                        4 | 12 => CompareOp::Gt,
                        5 | 13 => CompareOp::Ne,
                        6 | 14 => CompareOp::Ge,
                        _ => return None,
                    };
                    let combine = match bits(raw, 45, 2) {
                        0 => PredicateCombine::And,
                        1 => PredicateCombine::Or,
                        2 => PredicateCombine::Xor,
                        _ => return None,
                    };
                    (cmp, combine)
                };
                let input_predicate = bits(raw, 39, 3) as u8;
                let input_negated = bits(raw, 42, 1) != 0;
                o.push(S::PredicateSet {
                    dst,
                    dst_secondary,
                    domain: if code == M::ISetP {
                        PredicateDomain::Integer
                    } else if code == M::DSetP {
                        PredicateDomain::Double
                    } else {
                        PredicateDomain::Float
                    },
                    compare: cmp,
                    combine,
                    left,
                    right,
                    input_predicate,
                    input_negated,
                });
                Some(true)
            }
            M::Bfi | M::Shf | M::Prmt | M::Lop3 => {
                let dst = bits(raw, 0, 8) as u32;
                let a = bits(raw, 8, 8) as u32;
                let b = bits(raw, 20, 8) as u32;
                let c = bits(raw, 39, 8) as u32;
                let bop = match code {
                    M::Bfi => BitfieldOp::Insert,
                    M::Shf => {
                        if bits(raw, 48, 1) != 0 {
                            BitfieldOp::FunnelShiftRight
                        } else {
                            BitfieldOp::FunnelShiftLeft
                        }
                    }
                    M::Prmt => BitfieldOp::Permute,
                    M::Lop3 => BitfieldOp::Permute,
                    _ => unreachable!(),
                };
                o.push(S::BitfieldRich {
                    dst,
                    op: bop,
                    a,
                    b: Some(b),
                    c: if matches!(code, M::Bfi | M::Shf | M::Prmt | M::Lop3) {
                        Some(c)
                    } else {
                        None
                    },
                    raw_flags: ((raw >> 28) & 0xffff_ffff) as u32,
                });
                Some(true)
            }
            M::I2F => {
                // Mesa GM107 emitI2F: sizes at bits 8/10, integer sign at 13.
                // Subword selection, condition-code output and other sizes need
                // their own IR semantics rather than a guessed signed word.
                let constant_source = bits(raw, 48, 16) & 0xfff8 == 0x4cb8;
                let immediate_source = bits(raw, 48, 16) & 0xfef8 == 0x38b8;
                let opcode = if constant_source {
                    0x4cb8000000000000u64
                } else if immediate_source {
                    0x38b8000000000000u64
                } else {
                    0x5cb8000000000000u64
                };
                let variable = 0xff
                    | (3 << 8)
                    | (3 << 10)
                    | (1 << 13)
                    | (15 << 16)
                    | (if constant_source || immediate_source {
                        0x7ffff
                    } else {
                        255
                    } << 20)
                    | (3 << 39)
                    | (1 << 45)
                    | (1 << 49)
                    | if immediate_source { 1 << 56 } else { 0 };
                if bits(raw, 8, 2) != 2
                    || bits(raw, 10, 2) != 2
                    || raw & !variable != opcode
                    || bits(raw, 0, 8) == 255
                {
                    return None;
                }
                let dst = bits(raw, 0, 8) as u32;
                let src = if immediate_source {
                    // Immediate20 is sign-extended as a register word even for
                    // U32 conversion; sign interpretation belongs to control.src.
                    let value = sext(short_immediate20(raw), 20) as u32;
                    o.push(S::ConstantBits32 { dst, value });
                    dst
                } else if constant_source {
                    let MaxwellOperand::ConstantBuffer {
                        slot,
                        offset,
                        address,
                    } = cbuf(raw)
                    else {
                        unreachable!()
                    };
                    // Unary conversion can reuse its own destination as the
                    // loaded word, without inventing an aliasing temporary ID.
                    o.push(S::LoadConstantBuffer {
                        dst,
                        slot,
                        offset,
                        address: address.map(u32::from),
                    });
                    dst
                } else {
                    let src = bits(raw, 20, 8) as u32;
                    if src == 255 {
                        // RZ is an architectural zero, never an ordinary IR
                        // register. Modifier semantics remain explicit/unsupported.
                        if bits(raw, 49, 1) != 0 || bits(raw, 45, 1) != 0 {
                            return None;
                        }
                        o.push(S::ConstantBits32 { dst, value: 0 });
                        return Some(true);
                    }
                    src
                };
                o.push(S::ConvertRich {
                    dst,
                    src,
                    control: ConversionControl {
                        src: if bits(raw, 13, 1) != 0 {
                            ScalarType::I32
                        } else {
                            ScalarType::U32
                        },
                        dst: ScalarType::F32,
                        rounding: match bits(raw, 39, 2) {
                            0 => RoundingMode::NearestEven,
                            1 => RoundingMode::TowardNegative,
                            2 => RoundingMode::TowardPositive,
                            _ => RoundingMode::TowardZero,
                        },
                        saturate: false,
                        flush_to_zero: false,
                        absolute: bits(raw, 49, 1) != 0,
                        negate: bits(raw, 45, 1) != 0,
                        raw_flags: ((raw >> 32) & 0x00ff_ffff) as u32,
                    },
                });
                Some(true)
            }
            M::I2I | M::F2I | M::F2F => {
                let dst = bits(raw, 0, 8) as u32;
                let src = bits(raw, 20, 8) as u32;
                let (from, to) = match code {
                    M::I2I => (ScalarType::I32, ScalarType::I32),
                    M::F2I => (ScalarType::F32, ScalarType::I32),
                    _ => (ScalarType::F32, ScalarType::F32),
                };
                let rounding = match bits(raw, 39, 2) {
                    0 => RoundingMode::NearestEven,
                    1 => RoundingMode::TowardNegative,
                    2 => RoundingMode::TowardPositive,
                    _ => RoundingMode::TowardZero,
                };
                o.push(S::ConvertRich {
                    dst,
                    src,
                    control: ConversionControl {
                        src: from,
                        dst: to,
                        rounding,
                        saturate: bits(raw, 50, 1) != 0,
                        flush_to_zero: bits(raw, 44, 1) != 0,
                        absolute: bits(raw, 49, 1) != 0,
                        negate: bits(raw, 45, 1) != 0,
                        raw_flags: ((raw >> 32) & 0x00ff_ffff) as u32,
                    },
                });
                Some(true)
            }
        }
    })();
    if result == Some(true) {
        if let Some(g) = d.predicate {
            o = vec![S::Predicated {
                predicate: g.index,
                negated: g.negated,
                operations: o,
            }];
        }
        CanonicalLowering {
            operations: o,
            lossless: true,
            reason: None,
        }
    } else {
        CanonicalLowering {
            operations: vec![S::OpaqueMaxwell { raw }],
            lossless: false,
            reason: d.reason.or(Some("decoded form not yet losslessly lowered")),
        }
    }
}

pub fn lowering_coverage(program: &MaxwellProgram) -> SemanticCoverageReport {
    let mut r = SemanticCoverageReport::default();
    for i in program
        .bundles
        .iter()
        .flat_map(|b| b.instructions.iter().copied())
    {
        r.total += 1;
        let d = decode_semantic(i);
        let l = lower_semantic(&d);
        if l.lossless {
            r.ready += 1
        } else if d.opcode.is_some() {
            r.needs_canonical_ir += 1
        } else {
            r.unknown_encoding += 1
        }
        if let Some(op) = d.opcode {
            *r.opcodes.entry(op).or_default() += 1
        }
        for operand in d.operands {
            *r.operand_forms.entry(operand.form()).or_default() += 1
        }
        if !l.lossless {
            *r.fallback_reasons
                .entry(l.reason.unwrap_or("unknown lowering failure"))
                .or_default() += 1
        }
    }
    r
}

#[cfg(test)]
mod lowering_tests {
    use super::*;
    use crate::maxwell_shader::{MaxwellBundle, MaxwellProgram};
    use crate::shader::{RoundingMode, ScalarType};
    use crate::{ShaderOperation, ShaderStage};

    #[test]
    fn interpolation_retains_byte_address_component_and_supported_mode() {
        use crate::shader::InterpolationMode;
        let fixed = 0xe000000000000000u64
            | (255 << 8)
            | (255 << 20)
            | (255 << 39)
            | (7 << 47)
            | (7 << 16)
            | 9;
        for address in [0x80u64, 0x84, 0x88, 0x8c, 0x90, 0x27c] {
            for (encoding, mode) in [
                (0, InterpolationMode::Pass),
                (2, InterpolationMode::Constant),
            ] {
                let raw = fixed | (address << 28) | (encoding << 54);
                let lowered = lower_semantic(&decode_semantic(ins(raw)));
                assert!(
                    lowered.lossless,
                    "IPA {raw:016x}: {:?}, {lowered:?}",
                    decode_semantic(ins(raw))
                );
                assert_eq!(
                    lowered.operations,
                    vec![ShaderOperation::Interpolate {
                        dst: 9,
                        attribute: address as u32,
                        component: ((address & 15) / 4) as u8,
                        mode,
                        offset: None,
                    }]
                );
                let guarded =
                    lower_semantic(&decode_semantic(ins((raw & !(15 << 16)) | (2 << 16))));
                assert!(matches!(
                    guarded.operations.as_slice(),
                    [ShaderOperation::Predicated { predicate: 2, .. }]
                ));
            }
        }
        for invalid in [
            fixed | (1 << 54),
            fixed | (3 << 54),
            fixed | (1 << 52),
            fixed | (1 << 53),
            fixed | (1 << 51),
            fixed | (1 << 38),
            fixed | (1 << 28),
            fixed & !(1 << 8),
            fixed & !(1 << 20),
            fixed & !(1 << 39),
            fixed & !(1 << 47),
            (fixed & !255) | 255,
        ] {
            assert!(
                !lower_semantic(&decode_semantic(ins(invalid))).lossless,
                "unsupported IPA {invalid:016x} was admitted"
            );
        }
    }
    #[test]
    fn attribute_lowering_retains_address_modes_and_rejects_unknown_fields() {
        for (base, store) in [(0xefd8000000000000u64, false), (0xeff0000000000000, true)] {
            for width in 1..=4u8 {
                for (address_register, vertex_register, patch) in
                    [(255u64, 255u64, false), (3, 17, true)]
                {
                    let raw = base
                        | (u64::from(width - 1) << 47)
                        | (vertex_register << 39)
                        | (u64::from(patch) << 31)
                        | (0x80 << 20)
                        | (7 << 16)
                        | (address_register << 8)
                        | 9;
                    let address = crate::shader::ShaderAttributeAddress {
                        byte_offset: 0x80,
                        address_register: (address_register != 255)
                            .then_some(address_register as u32),
                        vertex_register: (vertex_register != 255).then_some(vertex_register as u32),
                        per_patch: patch,
                    };
                    let expected = if store {
                        ShaderOperation::StoreAttributeRich {
                            src: 9,
                            address,
                            components: width,
                        }
                    } else {
                        ShaderOperation::LoadAttributeRich {
                            dst: 9,
                            address,
                            components: width,
                            output: false,
                        }
                    };
                    let decoded = decode_semantic(ins(raw));
                    let lowered = lower_semantic(&decoded);
                    assert!(lowered.lossless);
                    assert_eq!(lowered.operations, vec![expected]);
                    assert_eq!(
                        canonicalize_disposition(decoded).disposition,
                        SemanticDisposition::Ready
                    );
                    let guarded = lower_semantic(&decode_semantic(ins((raw & !(7 << 16))
                        | (2 << 16)
                        | (1 << 19))));
                    assert!(matches!(
                        guarded.operations.as_slice(),
                        [ShaderOperation::Predicated {
                            predicate: 2,
                            negated: true,
                            ..
                        }]
                    ));
                    for unknown_bit in [30, 33, 38, 49, 50] {
                        assert!(
                            !lower_semantic(&decode_semantic(ins(raw | (1 << unknown_bit))))
                                .lossless
                        );
                    }
                    let mut program = one(raw);
                    let (_, invalid_stage) = crate::maxwell_shader::lift_semantic(1, &program);
                    assert_eq!(invalid_stage.opaque, 1);
                    program.stage = ShaderStage::Vertex;
                    let (_, valid_stage) = crate::maxwell_shader::lift_semantic(1, &program);
                    assert_eq!(valid_stage.semantic, 1);
                }
            }
        }
        let direct = 0xefd8000000000000u64 | (255 << 39) | (255 << 8) | (7 << 16);
        let output = lower_semantic(&decode_semantic(ins(direct | (1 << 32))));
        assert!(matches!(
            output.operations.as_slice(),
            [ShaderOperation::LoadAttributeRich { output: true, .. }]
        ));
        assert!(!lower_semantic(&decode_semantic(ins(direct | 255))).lossless);
        let store = (direct & 0x0000ffffffffffff) | 0xeff0000000000000;
        assert!(!lower_semantic(&decode_semantic(ins(store | (1 << 32)))).lossless);
    }
    fn ins(raw: u64) -> MaxwellInstruction {
        MaxwellInstruction { offset: 8, raw }
    }
    fn one(raw: u64) -> MaxwellProgram {
        MaxwellProgram {
            stage: ShaderStage::Compute,
            header: None,
            bundles: vec![MaxwellBundle {
                scheduler_offset: 0,
                scheduler: 0,
                instructions: vec![ins(raw)],
            }],
        }
    }
    fn contains(op: &ShaderOperation, p: fn(&ShaderOperation) -> bool) -> bool {
        if p(op) {
            true
        } else if let ShaderOperation::Predicated { operations, .. } = op {
            operations.iter().any(|x| contains(x, p))
        } else {
            false
        }
    }
    #[test]
    fn ready_means_lossless_lowering() {
        for raw in [
            0x0100_0001_2345_f002,
            0x5c10_0000_0030_0201,
            0xeed0_0000_0040_0201,
            0xef10_0000_0030_0201,
            0xf0a8_0000_0000_0000,
        ] {
            let d = decode_semantic(ins(raw));
            if d.disposition == SemanticDisposition::Ready {
                assert!(
                    lower_semantic(&d).lossless,
                    "Ready instruction failed lowering: {raw:016x}"
                );
            }
        }
    }
    #[test]
    fn direct_2d_tex_retains_packed_channels_coordinates_selector_and_lod() {
        let base = 0xc038000000000000u64 | (255 << 20) | (1 << 29) | (7 << 16);
        for mask in 1..16u64 {
            for selector in [0, 1, 0x123, 0x1fff] {
                for level_zero in [false, true] {
                    let raw = base
                        | 4
                        | (8 << 8)
                        | (mask << 31)
                        | (selector << 36)
                        | (u64::from(level_zero) << 55);
                    assert_eq!(
                        classify_instruction(ins(raw)).family,
                        MaxwellOpcodeFamily::Texture
                    );
                    let lowered = lower_semantic(&decode_semantic(ins(raw)));
                    assert!(lowered.lossless, "{raw:016x}");
                    assert_eq!(
                        lowered.operations,
                        vec![ShaderOperation::SampleTexture2dHandle {
                            dst: (4..4 + mask.count_ones()).collect(),
                            handle_selector: selector as u16,
                            coords: [8, 9],
                            components: (0..4)
                                .filter(|component| mask & (1 << component) != 0)
                                .collect(),
                            level_zero,
                        }]
                    );
                }
            }
        }
        let valid = base | 4 | (8 << 8) | (15 << 31);
        // Array/cube/3D, offsets, derivative/live/shadow controls and reserved
        // fields cannot acquire the semantics of the ordinary 2D form.
        for bit in [28, 30, 35, 49, 50, 54, 57, 58, 59, 60, 61] {
            assert!(
                !lower_semantic(&decode_semantic(ins(valid | (1 << bit)))).lossless,
                "bit {bit}"
            );
        }
        for invalid in [
            valid & !(15 << 31),
            valid | (2 << 55),
            valid | (3 << 55),
            (valid & !(255 << 20)) | (3 << 20),
            (valid & !(255 << 8)) | (254 << 8),
            (valid & !255) | 253,
            0xdeb8000000000000 | (valid & 0x0000ffffffffffff),
        ] {
            assert!(
                !lower_semantic(&decode_semantic(ins(invalid))).lossless,
                "{invalid:016x}"
            );
        }
        let guarded = (valid & !(15 << 16)) | (2 << 16);
        let lowered = lower_semantic(&decode_semantic(ins(guarded)));
        assert!(lowered.lossless);
        assert!(matches!(
            lowered.operations.as_slice(),
            [ShaderOperation::Predicated { predicate: 2, .. }]
        ));
    }
    #[test]
    fn coverage_is_post_lowering() {
        let r = lowering_coverage(&one(0x0100_0001_2345_f002));
        assert_eq!(
            (r.total, r.ready, r.needs_canonical_ir, r.unknown_encoding),
            (1, 1, 0, 0)
        );
    }
    #[test]
    fn global_load_becomes_rich_memory() {
        let d = decode_semantic(ins(0xeed0_0000_0040_0201));
        let l = lower_semantic(&d);
        assert!(
            l.operations
                .iter()
                .any(|x| contains(x, |y| matches!(y, ShaderOperation::LoadMemoryRich { .. })))
        );
    }
    #[test]
    fn shuffle_becomes_subgroup() {
        let d = decode_semantic(ins(0xef10_0000_0030_0201));
        let l = lower_semantic(&d);
        assert!(
            l.operations
                .iter()
                .any(|x| contains(x, |y| matches!(y, ShaderOperation::Subgroup { .. })))
        );
    }
    #[test]
    fn barriers_are_not_opaque() {
        for raw in [
            0xf0a8_0000_0000_0000,
            0xef98_0000_0000_0000,
            0xf0f0_0000_0000_0000,
        ] {
            assert!(lower_semantic(&decode_semantic(ins(raw))).lossless);
        }
    }
    #[test]
    fn predicate_guard_is_never_dropped() {
        let raw = 0x5c10_0000_0000_0000u64 | (2u64 << 16);
        let l = lower_semantic(&decode_semantic(ins(raw)));
        assert!(matches!(
            l.operations.as_slice(),
            [ShaderOperation::Predicated { predicate: 2, .. }]
        ));
    }
    #[test]
    fn ternary_register_forms_lower() {
        for raw in [
            0x5a00_0000_0000_0000,
            0x5980_0000_0000_0000,
            0x5b00_0000_0000_0000,
        ] {
            let l = lower_semantic(&decode_semantic(ins(raw)));
            assert!(l.lossless, "ternary failed {raw:016x}");
        }
    }
    #[test]
    fn setp_has_canonical_predicate_operation() {
        for raw in [0x5b62_0000_0000_0000, 0x5bb1_0000_0000_0000] {
            let l = lower_semantic(&decode_semantic(ins(raw)));
            assert!(
                l.operations
                    .iter()
                    .any(|x| contains(x, |y| matches!(y, ShaderOperation::PredicateSet { .. })))
            );
        }
    }
    #[test]
    fn long_tail_bitfield_is_lossless() {
        for raw in [
            0x5c00_0000_0030_0201,
            0x5bf0_0000_4030_0201,
            0x5bf8_0000_4030_0201,
            0x5bc0_0000_4030_0201,
            0x5c08_0000_0030_0001,
            0x5c30_0000_0030_0001,
            0x5be0_0000_4030_0201,
        ] {
            let l = lower_semantic(&decode_semantic(ins(raw)));
            assert!(l.lossless, "bitfield/permute failed {raw:016x}");
        }
    }
    #[test]
    fn conversions_are_rich_not_opaque() {
        for raw in [
            0x5ce0_0000_0030_0001,
            0x5cb8_0000_0030_0a01,
            0x5cb0_0000_0030_0001,
            0x5ca8_0000_0030_0001,
        ] {
            let l = lower_semantic(&decode_semantic(ins(raw)));
            assert!(
                l.operations
                    .iter()
                    .any(|x| contains(x, |y| matches!(y, ShaderOperation::ConvertRich { .. })))
            );
        }
    }
    #[test]
    fn i2f_preserves_word_type_rounding_and_modifiers() {
        let base = 0x5cb8_0000_0000_0000u64 | (2 << 8) | (2 << 10) | (7 << 16) | (19 << 20) | 11;
        for signed in [false, true] {
            for (encoded, rounding) in [
                (0, RoundingMode::NearestEven),
                (1, RoundingMode::TowardNegative),
                (2, RoundingMode::TowardPositive),
                (3, RoundingMode::TowardZero),
            ] {
                for modifiers in [0, 1 << 45, 1 << 49, (1 << 45) | (1 << 49)] {
                    let raw = base | (u64::from(signed) << 13) | (encoded << 39) | modifiers;
                    let lowered = lower_semantic(&decode_semantic(ins(raw)));
                    assert!(lowered.lossless);
                    let [ShaderOperation::ConvertRich { dst, src, control }] =
                        lowered.operations.as_slice()
                    else {
                        panic!("unexpected I2F lowering: {lowered:?}");
                    };
                    assert_eq!((*dst, *src), (11, 19));
                    assert_eq!(
                        control.src,
                        if signed {
                            ScalarType::I32
                        } else {
                            ScalarType::U32
                        }
                    );
                    assert_eq!(control.dst, ScalarType::F32);
                    assert_eq!(control.rounding, rounding);
                    assert_eq!(control.negate, modifiers & (1 << 45) != 0);
                    assert_eq!(control.absolute, modifiers & (1 << 49) != 0);
                    assert!(!control.saturate && !control.flush_to_zero);
                    assert_eq!(control.raw_flags, ((raw >> 32) & 0x00ff_ffff) as u32);
                }
            }
        }
        let guarded = (base & !(15 << 16)) | (2 << 16);
        assert!(matches!(
            lower_semantic(&decode_semantic(ins(guarded)))
                .operations
                .as_slice(),
            [ShaderOperation::Predicated { predicate: 2, .. }]
        ));
    }

    #[test]
    fn i2f_rejects_unrepresented_sizes_and_controls() {
        let base = 0x5cb8_0000_0000_0000u64 | (2 << 8) | (2 << 10) | (7 << 16);
        for shift in [8, 10] {
            for size in [0, 1, 3] {
                let raw = (base & !(3 << shift)) | (size << shift);
                assert!(!lower_semantic(&decode_semantic(ins(raw))).lossless);
            }
        }
        for bit in [12, 14, 28, 41, 42, 44, 47, 48, 50] {
            assert!(
                !lower_semantic(&decode_semantic(ins(base | (1 << bit)))).lossless,
                "unrepresented I2F bit {bit} was admitted"
            );
        }
    }

    #[test]
    fn i2f_zero_register_is_not_an_undefined_register_read() {
        let base = 0x5cb8000000000000u64 | (2 << 8) | (2 << 10) | (7 << 16) | (255 << 20) | 9;
        for signed in [0, 1] {
            for rounding in 0..4 {
                let lowered = lower_semantic(&decode_semantic(ins(base
                    | (signed << 13)
                    | (rounding << 39))));
                assert!(lowered.lossless);
                assert_eq!(
                    lowered.operations,
                    vec![ShaderOperation::ConstantBits32 { dst: 9, value: 0 }]
                );
            }
        }
        for raw in [(base & !255) | 255, base | (1 << 49), base | (1 << 45)] {
            assert!(!lower_semantic(&decode_semantic(ins(raw))).lossless);
        }
    }

    #[test]
    fn i2f_immediate_sign_extension_is_independent_of_conversion_type() {
        for immediate in [-524288i32, -1, 0, 1, 524287] {
            for signed in [false, true] {
                for rounding in 0..4u64 {
                    let word = immediate as u32;
                    let raw = 0x38b8000000000000u64
                        | (2 << 8)
                        | (2 << 10)
                        | (7 << 16)
                        | 9
                        | (u64::from(signed) << 13)
                        | (rounding << 39)
                        | (u64::from(word & 0x7ffff) << 20)
                        | (u64::from((word >> 19) & 1) << 56);
                    let decoded = decode_semantic(ins(raw));
                    assert_eq!(decoded.operands[1], MaxwellOperand::ImmediateI32(immediate));
                    let lowered = lower_semantic(&decoded);
                    assert!(lowered.lossless);
                    let [
                        ShaderOperation::ConstantBits32 { dst: 9, value },
                        ShaderOperation::ConvertRich {
                            dst: 9,
                            src: 9,
                            control,
                        },
                    ] = lowered.operations.as_slice()
                    else {
                        panic!("unexpected immediate conversion: {lowered:?}");
                    };
                    assert_eq!(*value, word);
                    assert_eq!(
                        control.src,
                        if signed {
                            ScalarType::I32
                        } else {
                            ScalarType::U32
                        }
                    );
                    for bit in [12, 14, 41, 44, 47, 48, 50] {
                        assert!(!lower_semantic(&decode_semantic(ins(raw | (1 << bit)))).lossless);
                    }
                    let guarded =
                        lower_semantic(&decode_semantic(ins((raw & !(15 << 16)) | (2 << 16))));
                    assert!(matches!(guarded.operations.as_slice(),
                        [ShaderOperation::Predicated { predicate: 2, operations, .. }] if operations.len() == 2));
                }
            }
        }
    }

    #[test]
    fn i2f_constant_buffer_retains_slot_offset_and_guard() {
        for slot in [0, 3, 31] {
            for offset in [0, 28, 0xfffc] {
                let raw = 0x4cb8000000000000u64
                    | (2 << 8)
                    | (2 << 10)
                    | (1 << 13)
                    | (7 << 16)
                    | (slot << 34)
                    | ((offset / 4) << 20)
                    | 9;
                let decoded = decode_semantic(ins(raw));
                assert_eq!(decoded.opcode, Some(MaxwellSemanticOpcode::I2F));
                let lowered = lower_semantic(&decoded);
                assert!(lowered.lossless);
                let [
                    ShaderOperation::LoadConstantBuffer {
                        dst: 9,
                        slot: actual_slot,
                        offset: actual_offset,
                        address: None,
                    },
                    ShaderOperation::ConvertRich {
                        dst: 9,
                        src: 9,
                        control,
                    },
                ] = lowered.operations.as_slice()
                else {
                    panic!("unexpected constant-source conversion: {lowered:?}");
                };
                assert_eq!(u64::from(*actual_slot), slot);
                assert_eq!(*actual_offset as u64, offset);
                assert_eq!(control.src, ScalarType::I32);
                let guarded =
                    lower_semantic(&decode_semantic(ins((raw & !(15 << 16)) | (2 << 16))));
                assert!(matches!(guarded.operations.as_slice(),
                    [ShaderOperation::Predicated { predicate: 2, operations, .. }] if operations.len() == 2));
                for bit in [12, 14, 41, 44, 47, 48, 50] {
                    assert!(!lower_semantic(&decode_semantic(ins(raw | (1 << bit)))).lossless);
                }
            }
        }
    }

    #[test]
    fn float_conversions_use_gm107_rounding_encoding() {
        // Mesa emitRND encodes N/M/P/Z as 0/1/2/3, not enum order.
        for opcode in [0x5cb0_0000_0000_0000u64, 0x5ca8_0000_0000_0000] {
            for (encoded, expected) in [
                (0, RoundingMode::NearestEven),
                (1, RoundingMode::TowardNegative),
                (2, RoundingMode::TowardPositive),
                (3, RoundingMode::TowardZero),
            ] {
                let raw = opcode | (7 << 16) | (encoded << 39);
                let lowered = lower_semantic(&decode_semantic(ins(raw)));
                let [ShaderOperation::ConvertRich { control, .. }] = lowered.operations.as_slice()
                else {
                    panic!("expected conversion: {lowered:?}");
                };
                assert_eq!(control.rounding, expected);
            }
        }
    }
    #[test]
    fn surface_family_has_canonical_representation() {
        for raw in [
            0xeb00_0000_4030_0201,
            0xeb20_0000_4030_0201,
            0xeb40_0000_4030_0201,
            0xea00_0000_4030_0201,
        ] {
            let l = lower_semantic(&decode_semantic(ins(raw)));
            assert!(l.lossless, "surface failed {raw:016x}");
            assert!(
                l.operations
                    .iter()
                    .any(|x| contains(x, |y| matches!(y, ShaderOperation::Surface { .. })))
            );
        }
    }
    #[test]
    fn flo_preserves_search_mode_inversion_and_single_source_forms() {
        for signed in [false, true] {
            for invert in [false, true] {
                for shift_amount in [false, true] {
                    for (opcode, source) in [
                        (0x5c30000000000000u64, 9 << 20),
                        (0x4c30000000000000, (3 << 34) | (0x321 << 20)),
                        (0x3830000000000000, 0x12345 << 20),
                        (0x3830000000000000, (1 << 56) | (0x7ffff << 20)),
                    ] {
                        let raw = opcode
                            | source
                            | (u64::from(signed) << 48)
                            | (u64::from(invert) << 40)
                            | (u64::from(shift_amount) << 41)
                            | (7 << 16)
                            | 2;
                        let decoded = decode_semantic(ins(raw));
                        assert_eq!(decoded.opcode, Some(MaxwellSemanticOpcode::Flo));
                        assert_eq!(decoded.disposition, SemanticDisposition::Ready);
                        assert_eq!(decoded.operands.len(), 2);
                        let lowered = lower_semantic(&decoded);
                        assert!(lowered.lossless);
                        assert!(matches!(lowered.operations.last(),
                            Some(ShaderOperation::FindLeadingWord32 { dst: 2,
                                signed: s, invert: i, shift_amount: a, .. })
                                if (*s, *i, *a) == (signed, invert, shift_amount)));
                        match decoded.operands[1] {
                            MaxwellOperand::Register(9) => assert_eq!(lowered.operations.len(), 1),
                            MaxwellOperand::ConstantBuffer { .. } => assert!(matches!(
                                lowered.operations.first(),
                                Some(ShaderOperation::LoadConstantBuffer {
                                    slot: 3,
                                    offset: 0xc84,
                                    address: None,
                                    ..
                                })
                            )),
                            MaxwellOperand::ImmediateI32(value) => assert!(matches!(
                                lowered.operations.first(), Some(ShaderOperation::ConstantBits32 { value: actual, .. })
                                    if *actual == value as u32)),
                            other => panic!("unexpected FLO source: {other:?}"),
                        }
                        for bit in [8, 15, 39, 42, 47, 49, 50] {
                            assert!(
                                !lower_semantic(&decode_semantic(ins(raw | (1 << bit)))).lossless,
                                "accepted unsupported FLO control {bit} in {raw:016x}"
                            );
                        }
                        assert!(!lower_semantic(&decode_semantic(ins(raw | 255))).lossless);
                        let predicated = lower_semantic(&decode_semantic(ins(raw & !(7 << 16))));
                        assert!(matches!(
                            predicated.operations.as_slice(),
                            [ShaderOperation::Predicated { predicate: 0, .. }]
                        ));
                    }
                }
            }
        }
        let zero = lower_semantic(&decode_semantic(ins(0x5c3000000ff70002)));
        assert!(zero.lossless);
        assert!(matches!(
            zero.operations.as_slice(),
            [
                ShaderOperation::ConstantBits32 { value: 0, .. },
                ShaderOperation::FindLeadingWord32 { .. }
            ]
        ));
    }

    #[test]
    fn popc_has_one_source_and_preserves_inversion_for_every_form() {
        for invert in [false, true] {
            for (opcode, source) in [
                (0x5c08000000000000u64, 9 << 20),
                (0x4c08000000000000, (3 << 34) | (0x321 << 20)),
                (0x3808000000000000, 0x12345 << 20),
                (0x3808000000000000, (1 << 56) | (0x7ffff << 20)),
            ] {
                let raw = opcode | source | (u64::from(invert) << 40) | (7 << 16) | 2;
                let decoded = decode_semantic(ins(raw));
                assert_eq!(decoded.opcode, Some(MaxwellSemanticOpcode::Popc));
                assert_eq!(decoded.disposition, SemanticDisposition::Ready);
                assert_eq!(decoded.operands.len(), 2);
                let lowered = lower_semantic(&decoded);
                assert!(lowered.lossless);
                assert!(matches!(lowered.operations.last(),
                    Some(ShaderOperation::CountBits32 { dst: 2, invert: actual, .. })
                        if *actual == invert));
                match decoded.operands[1] {
                    MaxwellOperand::Register(9) => assert_eq!(lowered.operations.len(), 1),
                    MaxwellOperand::ConstantBuffer { .. } => assert!(matches!(
                        lowered.operations.first(),
                        Some(ShaderOperation::LoadConstantBuffer {
                            slot: 3,
                            offset: 0xc84,
                            address: None,
                            ..
                        })
                    )),
                    MaxwellOperand::ImmediateI32(value) => assert!(matches!(
                        lowered.operations.first(), Some(ShaderOperation::ConstantBits32 { value: actual, .. })
                        if *actual == value as u32)),
                    other => panic!("unexpected POPC source: {other:?}"),
                }
                // POPC has no src0 or CC field; unused bits must not be dropped.
                for bit in [8, 15, 39, 41, 47, 48, 49, 50] {
                    assert!(
                        !lower_semantic(&decode_semantic(ins(raw | (1 << bit)))).lossless,
                        "accepted unsupported POPC bit {bit} in {raw:016x}"
                    );
                }
                assert!(!lower_semantic(&decode_semantic(ins(raw | 255))).lossless);
                let predicated = lower_semantic(&decode_semantic(ins(raw & !(7 << 16))));
                assert!(matches!(
                    predicated.operations.as_slice(),
                    [ShaderOperation::Predicated { predicate: 0, .. }]
                ));
            }
        }
        let zero = lower_semantic(&decode_semantic(ins(0x5c0800000ff70002)));
        assert!(zero.lossless);
        assert!(matches!(
            zero.operations.as_slice(),
            [
                ShaderOperation::ConstantBits32 { value: 0, .. },
                ShaderOperation::CountBits32 { .. }
            ]
        ));
    }

    #[test]
    fn bfe_preserves_source_forms_sign_and_rejects_unmodeled_controls() {
        for signed in [false, true] {
            for (opcode, source) in [
                (0x5c00000000000000u64, 9 << 20),
                (0x4c00000000000000, (3 << 34) | (0x321 << 20)),
                (0x3800000000000000, 0x1f08 << 20),
                (0x3800000000000000, (1 << 56) | (0x7ffff << 20)),
            ] {
                let raw = opcode | source | (u64::from(signed) << 48) | (7 << 16) | (8 << 8) | 2;
                let decoded = decode_semantic(ins(raw));
                assert_eq!(decoded.opcode, Some(MaxwellSemanticOpcode::Bfe));
                assert_eq!(decoded.disposition, SemanticDisposition::Ready);
                let lowered = lower_semantic(&decoded);
                assert!(lowered.lossless, "{raw:016x}: {lowered:?}");
                assert!(matches!(lowered.operations.last(),
                    Some(ShaderOperation::ExtractWord32 { dst: 2, value: 8, signed: actual, .. })
                        if *actual == signed));
                match decoded.operands[2] {
                    MaxwellOperand::Register(9) => assert_eq!(lowered.operations.len(), 1),
                    MaxwellOperand::ConstantBuffer { .. } => assert!(matches!(
                        lowered.operations.first(),
                        Some(ShaderOperation::LoadConstantBuffer {
                            slot: 3,
                            offset: 0xc84,
                            address: None,
                            ..
                        })
                    )),
                    MaxwellOperand::ImmediateI32(value) => assert!(matches!(
                        lowered.operations.first(), Some(ShaderOperation::ConstantBits32 { value: actual, .. })
                        if *actual == value as u32)),
                    other => panic!("unexpected source: {other:?}"),
                }
                for bit in [40, 47, 49, 50] {
                    assert!(
                        !lower_semantic(&decode_semantic(ins(raw | (1 << bit)))).lossless,
                        "accepted unsupported BFE control {bit} in {raw:016x}"
                    );
                }
                assert!(!lower_semantic(&decode_semantic(ins(raw | 255))).lossless);
                let guarded = lower_semantic(&decode_semantic(ins(raw & !(7 << 16))));
                assert!(matches!(
                    guarded.operations.as_slice(),
                    [ShaderOperation::Predicated { predicate: 0, .. }]
                ));
            }
        }
        let raw = 0x5c00000000070002 | (255 << 8) | (255 << 20);
        let lowered = lower_semantic(&decode_semantic(ins(raw)));
        assert!(lowered.lossless);
        assert_eq!(
            lowered
                .operations
                .iter()
                .filter(|op| matches!(op, ShaderOperation::ConstantBits32 { value: 0, .. }))
                .count(),
            2
        );
    }

    #[test]
    fn sm60_half_forms_are_not_claimed_as_sm53() {
        for raw in [
            0x5d10_0000_0030_0201,
            0x5d08_0000_0030_0201,
            0x5d00_0000_4030_0201,
        ] {
            let d = decode_semantic(ins(raw));
            assert_ne!(d.disposition, SemanticDisposition::Ready);
        }
    }
}
