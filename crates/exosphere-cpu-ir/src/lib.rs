// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Canonical project-owned CPU IR.
//! The raw decoded layer stays architecture-faithful; semantic IR is SSA-oriented and backend-neutral.
pub mod cache;
pub mod cfg;
pub mod decode;
pub mod lift;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Register(pub u8);
#[derive(Debug, Clone)]
pub struct DecodedInstruction {
    pub guest_pc: u64,
    pub raw: u32,
    pub opcode: DecodedOpcode,
}
#[derive(Debug, Clone)]
pub enum DecodedOpcode {
    Nop,
    Ret {
        rn: u8,
    },
    BranchImm {
        target: u64,
        link: bool,
    },
    BranchReg {
        rn: u8,
        link: bool,
    },
    CondBranch {
        target: u64,
        cond: u8,
    },
    CompareBranch {
        target: u64,
        rt: u8,
        nonzero: bool,
        is64: bool,
    },
    TestBranch {
        target: u64,
        rt: u8,
        bit: u8,
        nonzero: bool,
    },
    Svc {
        imm: u16,
    },
    AddSubImm {
        rd: u8,
        rn: u8,
        imm: u64,
        sub: bool,
        set_flags: bool,
        is64: bool,
    },
    MoveWide {
        rd: u8,
        imm: u64,
        kind: MoveWideKind,
        is64: bool,
    },
    LogicalImm {
        rd: u8,
        rn: u8,
        raw_imm: u32,
        op: u8,
        is64: bool,
    },
    LoadStoreUnsigned {
        rt: u8,
        rn: u8,
        offset: u64,
        load: bool,
        size_bits: u16,
    },
    Adr {
        rd: u8,
        target: u64,
        page: bool,
    },
    Unknown,
}
#[derive(Debug, Clone, Copy)]
pub enum MoveWideKind {
    MovN,
    MovZ,
    MovK,
}
#[derive(Debug, Clone)]
pub struct Function {
    pub guest_entry: u64,
    pub blocks: Vec<BasicBlock>,
}
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: BlockId,
    pub guest_start: u64,
    pub operations: Vec<Operation>,
    pub terminator: Terminator,
}
#[derive(Debug, Clone)]
pub enum Operation {
    Constant {
        out: ValueId,
        value: u64,
    },
    ReadReg {
        out: ValueId,
        reg: Register,
    },
    WriteReg {
        reg: Register,
        value: ValueId,
    },
    Add {
        out: ValueId,
        lhs: ValueId,
        rhs: ValueId,
    },
    Sub {
        out: ValueId,
        lhs: ValueId,
        rhs: ValueId,
    },
    Load {
        out: ValueId,
        address: ValueId,
        bits: u16,
    },
    Store {
        address: ValueId,
        value: ValueId,
        bits: u16,
    },
    Syscall {
        imm: u16,
    },
    FallbackArm64 {
        guest_pc: u64,
        raw: u32,
    },
}
#[derive(Debug, Clone)]
pub enum Terminator {
    Jump(BlockId),
    Call {
        target: u64,
        continuation: BlockId,
    },
    Branch {
        condition: Condition,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return,
    DynamicBranch {
        register: Register,
        link: bool,
    },
    Fallthrough(Option<BlockId>),
}
#[derive(Debug, Clone)]
pub enum Condition {
    Flags(u8),
    RegisterZero {
        reg: Register,
        nonzero: bool,
        is64: bool,
    },
    TestBit {
        reg: Register,
        bit: u8,
        nonzero: bool,
    },
}
