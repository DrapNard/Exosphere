// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! SSA-oriented semantic lifting. Unsupported instructions become explicit fallback operations.
use crate::{DecodedOpcode as D, cfg::DecodedBlock, *};
use std::collections::BTreeMap;
pub fn lift(entry: u64, blocks: &[DecodedBlock]) -> Function {
    let ids: BTreeMap<u64, BlockId> = blocks.iter().map(|b| (b.start, b.id)).collect();
    let mut next = 0u32;
    let mut val = || {
        let v = ValueId(next);
        next += 1;
        v
    };
    let mut out = Vec::new();
    for (bi, b) in blocks.iter().enumerate() {
        let mut ops = Vec::new();
        let mut term = None;
        for i in &b.instructions {
            match i.opcode.clone() {
                D::Nop => {}
                D::Svc { imm } => ops.push(Operation::Syscall { imm }),
                D::AddSubImm {
                    rd, rn, imm, sub, ..
                } => {
                    let a = val();
                    ops.push(Operation::ReadReg {
                        out: a,
                        reg: Register(rn),
                    });
                    let c = val();
                    ops.push(Operation::Constant { out: c, value: imm });
                    let o = val();
                    ops.push(if sub {
                        Operation::Sub {
                            out: o,
                            lhs: a,
                            rhs: c,
                        }
                    } else {
                        Operation::Add {
                            out: o,
                            lhs: a,
                            rhs: c,
                        }
                    });
                    if rd != 31 {
                        ops.push(Operation::WriteReg {
                            reg: Register(rd),
                            value: o,
                        })
                    }
                }
                D::MoveWide {
                    rd,
                    imm,
                    kind: MoveWideKind::MovZ,
                    ..
                } => {
                    let v = val();
                    ops.push(Operation::Constant { out: v, value: imm });
                    if rd != 31 {
                        ops.push(Operation::WriteReg {
                            reg: Register(rd),
                            value: v,
                        })
                    }
                }
                D::MoveWide { .. } => ops.push(Operation::FallbackArm64 {
                    guest_pc: i.guest_pc,
                    raw: i.raw,
                }),
                D::LoadStoreUnsigned {
                    rt,
                    rn,
                    offset,
                    load,
                    size_bits,
                } => {
                    let base = val();
                    ops.push(Operation::ReadReg {
                        out: base,
                        reg: Register(rn),
                    });
                    let c = val();
                    ops.push(Operation::Constant {
                        out: c,
                        value: offset,
                    });
                    let addr = val();
                    ops.push(Operation::Add {
                        out: addr,
                        lhs: base,
                        rhs: c,
                    });
                    if load {
                        let o = val();
                        ops.push(Operation::Load {
                            out: o,
                            address: addr,
                            bits: size_bits,
                        });
                        if rt != 31 {
                            ops.push(Operation::WriteReg {
                                reg: Register(rt),
                                value: o,
                            })
                        }
                    } else {
                        let v = val();
                        ops.push(Operation::ReadReg {
                            out: v,
                            reg: Register(rt),
                        });
                        ops.push(Operation::Store {
                            address: addr,
                            value: v,
                            bits: size_bits,
                        })
                    }
                }
                D::Adr { rd, target, .. } => {
                    let v = val();
                    ops.push(Operation::Constant {
                        out: v,
                        value: target,
                    });
                    if rd != 31 {
                        ops.push(Operation::WriteReg {
                            reg: Register(rd),
                            value: v,
                        })
                    }
                }
                D::BranchImm { target, link } => {
                    term = Some(if link {
                        Terminator::Call {
                            target,
                            continuation: ids
                                .get(&(i.guest_pc + 4))
                                .copied()
                                .unwrap_or(BlockId(u32::MAX)),
                        }
                    } else {
                        Terminator::Jump(ids.get(&target).copied().unwrap_or(BlockId(u32::MAX)))
                    })
                }
                D::CondBranch { target, cond } => {
                    term = Some(Terminator::Branch {
                        condition: Condition::Flags(cond),
                        then_block: ids.get(&target).copied().unwrap_or(BlockId(u32::MAX)),
                        else_block: ids
                            .get(&(i.guest_pc + 4))
                            .copied()
                            .unwrap_or(BlockId(u32::MAX)),
                    })
                }
                D::CompareBranch {
                    target,
                    rt,
                    nonzero,
                    is64,
                } => {
                    term = Some(Terminator::Branch {
                        condition: Condition::RegisterZero {
                            reg: Register(rt),
                            nonzero,
                            is64,
                        },
                        then_block: ids.get(&target).copied().unwrap_or(BlockId(u32::MAX)),
                        else_block: ids
                            .get(&(i.guest_pc + 4))
                            .copied()
                            .unwrap_or(BlockId(u32::MAX)),
                    })
                }
                D::TestBranch {
                    target,
                    rt,
                    bit,
                    nonzero,
                } => {
                    term = Some(Terminator::Branch {
                        condition: Condition::TestBit {
                            reg: Register(rt),
                            bit,
                            nonzero,
                        },
                        then_block: ids.get(&target).copied().unwrap_or(BlockId(u32::MAX)),
                        else_block: ids
                            .get(&(i.guest_pc + 4))
                            .copied()
                            .unwrap_or(BlockId(u32::MAX)),
                    })
                }
                D::Ret { .. } => term = Some(Terminator::Return),
                D::BranchReg { rn, link } => {
                    term = Some(Terminator::DynamicBranch {
                        register: Register(rn),
                        link,
                    })
                }
                _ => ops.push(Operation::FallbackArm64 {
                    guest_pc: i.guest_pc,
                    raw: i.raw,
                }),
            }
        }
        let fall = blocks.get(bi + 1).map(|x| x.id);
        out.push(BasicBlock {
            id: b.id,
            guest_start: b.start,
            operations: ops,
            terminator: term.unwrap_or(Terminator::Fallthrough(fall)),
        })
    }
    Function {
        guest_entry: entry,
        blocks: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cfg, decode};
    #[test]
    fn unknown_becomes_fallback() {
        let bytes = 0u32.to_le_bytes();
        let d = decode::decode_text(&bytes, 0x1000);
        let b = cfg::build(&d);
        let f = lift(0x1000, &b);
        assert!(matches!(
            f.blocks[0].operations[0],
            Operation::FallbackArm64 { .. }
        ));
    }
}
