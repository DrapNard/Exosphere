// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Physical-register Maxwell IR and conservative CFG/SSA preparation.
use crate::maxwell_shader::{
    MaxwellOpcode, MaxwellPredicateGuard, MaxwellProgram, classify_instruction,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reg(pub u8);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pred(pub u8);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegBinary {
    IAdd,
    IMul,
    FAdd,
    FMul,
    Shl,
    Shr,
    Logic,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegOp {
    Nop,
    Mov {
        dst: Reg,
        src: Reg,
    },
    Binary {
        dst: Reg,
        op: RegBinary,
        a: Reg,
        b: Reg,
    },
    Branch {
        target: u32,
        guard: Option<MaxwellPredicateGuard>,
    },
    Exit,
    Kill,
    Opaque {
        raw: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegInst {
    pub pc: u32,
    pub op: RegOp,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub id: u32,
    pub start: u32,
    pub instructions: Vec<RegInst>,
    pub successors: Vec<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterCfg {
    pub blocks: Vec<BasicBlock>,
    pub entry: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SsaValue(pub u32);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhiNode {
    pub block: u32,
    pub register: Reg,
    pub value: SsaValue,
    pub incoming: Vec<(u32, SsaValue)>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SsaPlan {
    pub definitions: BTreeMap<(u32, Reg), SsaValue>,
    pub phis: Vec<PhiNode>,
}

fn lower_one(pc: u32, d: crate::maxwell_shader::MaxwellDecodedInstruction) -> RegInst {
    let c = d.common;
    let r = |x| Reg(x);
    let op = match d.opcode {
        MaxwellOpcode::Nop => RegOp::Nop,
        MaxwellOpcode::Mov => RegOp::Mov {
            dst: r(c.dst),
            src: r(c.src_a),
        },
        MaxwellOpcode::IAdd => RegOp::Binary {
            dst: r(c.dst),
            op: RegBinary::IAdd,
            a: r(c.src_a),
            b: r(c.src_b),
        },
        MaxwellOpcode::IMul => RegOp::Binary {
            dst: r(c.dst),
            op: RegBinary::IMul,
            a: r(c.src_a),
            b: r(c.src_b),
        },
        MaxwellOpcode::FAdd => RegOp::Binary {
            dst: r(c.dst),
            op: RegBinary::FAdd,
            a: r(c.src_a),
            b: r(c.src_b),
        },
        MaxwellOpcode::FMul => RegOp::Binary {
            dst: r(c.dst),
            op: RegBinary::FMul,
            a: r(c.src_a),
            b: r(c.src_b),
        },
        MaxwellOpcode::ShiftLeft => RegOp::Binary {
            dst: r(c.dst),
            op: RegBinary::Shl,
            a: r(c.src_a),
            b: r(c.src_b),
        },
        MaxwellOpcode::ShiftRight => RegOp::Binary {
            dst: r(c.dst),
            op: RegBinary::Shr,
            a: r(c.src_a),
            b: r(c.src_b),
        },
        MaxwellOpcode::Logic => RegOp::Binary {
            dst: r(c.dst),
            op: RegBinary::Logic,
            a: r(c.src_a),
            b: r(c.src_b),
        },
        MaxwellOpcode::Bra | MaxwellOpcode::Jmp => RegOp::Branch {
            target: d.branch_target.unwrap_or(pc + 8),
            guard: c.predicate,
        },
        MaxwellOpcode::Exit | MaxwellOpcode::Return => RegOp::Exit,
        MaxwellOpcode::Kill => RegOp::Kill,
        _ => RegOp::Opaque {
            raw: d.instruction.raw,
        },
    };
    RegInst { pc, op }
}

pub fn build_register_cfg(program: &MaxwellProgram) -> RegisterCfg {
    let mut ins: Vec<RegInst> = program
        .bundles
        .iter()
        .flat_map(|b| b.instructions.iter().copied())
        .map(|i| lower_one(i.offset, classify_instruction(i)))
        .collect();
    ins.sort_by_key(|i| i.pc);
    if ins.is_empty() {
        return RegisterCfg {
            blocks: vec![],
            entry: 0,
        };
    }
    let mut leaders = BTreeSet::from([ins[0].pc]);
    for (n, i) in ins.iter().enumerate() {
        if let RegOp::Branch { target, .. } = &i.op {
            leaders.insert(*target);
            if let Some(next) = ins.get(n + 1) {
                leaders.insert(next.pc);
            }
        } else if matches!(&i.op, RegOp::Exit | RegOp::Kill) {
            if let Some(next) = ins.get(n + 1) {
                leaders.insert(next.pc);
            }
        }
    }
    let mut blocks = Vec::new();
    let mut current: Option<BasicBlock> = None;
    for i in ins {
        if leaders.contains(&i.pc) {
            if let Some(b) = current.take() {
                blocks.push(b);
            }
            current = Some(BasicBlock {
                id: blocks.len() as u32,
                start: i.pc,
                instructions: vec![],
                successors: vec![],
            });
        }
        current.as_mut().unwrap().instructions.push(i);
    }
    if let Some(b) = current {
        blocks.push(b)
    }
    let by_start: BTreeMap<u32, u32> = blocks.iter().map(|b| (b.start, b.id)).collect();
    for n in 0..blocks.len() {
        let next = blocks.get(n + 1).map(|b| b.id);
        let last = blocks[n].instructions.last().map(|i| &i.op);
        let mut s = Vec::new();
        match last {
            Some(RegOp::Branch { target, guard }) => {
                if let Some(id) = by_start.get(target) {
                    s.push(*id)
                }
                if guard.is_some() {
                    if let Some(x) = next {
                        s.push(x)
                    }
                }
            }
            Some(RegOp::Exit | RegOp::Kill) => {}
            _ => {
                if let Some(x) = next {
                    s.push(x)
                }
            }
        }
        s.sort();
        s.dedup();
        blocks[n].successors = s;
    }
    // Discard blocks that cannot be reached from entry. Keeping dead Maxwell code in SSA can
    // manufacture phis and make later SIMT structuring appear more complex than the executable graph.
    let entry = blocks[0].id;
    let mut reachable = BTreeSet::from([entry]);
    let mut work = vec![entry];
    while let Some(id) = work.pop() {
        if let Some(b) = blocks.iter().find(|b| b.id == id) {
            for s in &b.successors {
                if reachable.insert(*s) {
                    work.push(*s);
                }
            }
        }
    }
    blocks.retain(|b| reachable.contains(&b.id));
    RegisterCfg { entry, blocks }
}

fn written(op: &RegOp) -> Option<Reg> {
    match op {
        RegOp::Mov { dst, .. } | RegOp::Binary { dst, .. } => Some(*dst),
        _ => None,
    }
}
/// Conservative SSA preparation: every physical-register write receives a value id; merge blocks
/// receive phi nodes for registers whose predecessor reaching definitions disagree.
pub fn plan_ssa(cfg: &RegisterCfg) -> SsaPlan {
    let mut next = 1u32;
    let mut plan = SsaPlan::default();
    let mut out: BTreeMap<u32, BTreeMap<Reg, SsaValue>> = BTreeMap::new();
    let preds: BTreeMap<u32, Vec<u32>> = cfg.blocks.iter().fold(BTreeMap::new(), |mut m, b| {
        for s in &b.successors {
            m.entry(*s).or_default().push(b.id)
        }
        m
    });
    for b in &cfg.blocks {
        let mut state = BTreeMap::new();
        if let Some(ps) = preds.get(&b.id) {
            let regs: BTreeSet<Reg> = ps
                .iter()
                .flat_map(|p| out.get(p).into_iter().flat_map(|m| m.keys().copied()))
                .collect();
            for r in regs {
                let vals: Vec<(u32, SsaValue)> = ps
                    .iter()
                    .filter_map(|p| out.get(p).and_then(|m| m.get(&r)).copied().map(|v| (*p, v)))
                    .collect();
                if vals.is_empty() {
                    continue;
                }
                let first = vals[0].1;
                if vals.iter().all(|x| x.1 == first) {
                    state.insert(r, first);
                } else {
                    let v = SsaValue(next);
                    next += 1;
                    plan.phis.push(PhiNode {
                        block: b.id,
                        register: r,
                        value: v,
                        incoming: vals,
                    });
                    state.insert(r, v);
                }
            }
        }
        for i in &b.instructions {
            if let Some(r) = written(&i.op) {
                let v = SsaValue(next);
                next += 1;
                plan.definitions.insert((i.pc, r), v);
                state.insert(r, v);
            }
        }
        out.insert(b.id, state);
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShaderStage;
    use crate::maxwell_shader::{MaxwellBundle, MaxwellInstruction};
    #[test]
    fn cfg_splits_branch_target() {
        let p = MaxwellProgram {
            stage: ShaderStage::Vertex,
            header: None,
            bundles: vec![MaxwellBundle {
                scheduler_offset: 0,
                scheduler: 0,
                instructions: vec![
                    MaxwellInstruction {
                        offset: 8,
                        raw: (0xe240u64 << 48) | (8u64 << 20),
                    },
                    MaxwellInstruction { offset: 16, raw: 0 },
                    MaxwellInstruction {
                        offset: 24,
                        raw: (0xe300u64 << 48),
                    },
                ],
            }],
        };
        let c = build_register_cfg(&p);
        assert!(c.blocks.len() >= 2);
    }
}

/// Resolved SSA form used by the semantic Maxwell lifter. Unlike `SsaPlan`, this records the
/// reaching value of every source operand and therefore never aliases a physical GPR number with
/// a canonical Shader IR value id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsaInstruction {
    pub pc: u32,
    pub op: RegOp,
    pub dst: Option<SsaValue>,
    pub sources: Vec<SsaValue>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsaBlock {
    pub id: u32,
    pub phis: Vec<PhiNode>,
    pub instructions: Vec<SsaInstruction>,
    pub successors: Vec<u32>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SsaProgram {
    pub blocks: Vec<SsaBlock>,
}

fn read_regs(op: &RegOp) -> Vec<Reg> {
    match op {
        RegOp::Mov { src, .. } => vec![*src],
        RegOp::Binary { a, b, .. } => vec![*a, *b],
        _ => Vec::new(),
    }
}

/// Build a loop-aware SSA graph by iterating reaching definitions to a fixed point. Phi ids are
/// stable per (block,register), so backedges converge instead of manufacturing new values forever.
pub fn build_ssa(cfg: &RegisterCfg) -> SsaProgram {
    let preds: BTreeMap<u32, Vec<u32>> = cfg.blocks.iter().fold(BTreeMap::new(), |mut m, b| {
        for s in &b.successors {
            m.entry(*s).or_default().push(b.id)
        }
        m
    });
    let mut next = 1u32;
    let mut defs: BTreeMap<(u32, Reg), SsaValue> = BTreeMap::new();
    for b in &cfg.blocks {
        for i in &b.instructions {
            if let Some(r) = written(&i.op) {
                defs.entry((i.pc, r)).or_insert_with(|| {
                    let v = SsaValue(next);
                    next += 1;
                    v
                });
            }
        }
    }
    let mut phi_ids: BTreeMap<(u32, Reg), SsaValue> = BTreeMap::new();
    let mut ins: BTreeMap<u32, BTreeMap<Reg, SsaValue>> = BTreeMap::new();
    let mut outs: BTreeMap<u32, BTreeMap<Reg, SsaValue>> = BTreeMap::new();
    let mut iterations = 0usize;
    loop {
        iterations += 1;
        let old = outs.clone();
        for b in &cfg.blocks {
            let ps = preds.get(&b.id).cloned().unwrap_or_default();
            let regs: BTreeSet<Reg> = ps
                .iter()
                .flat_map(|p| old.get(p).into_iter().flat_map(|m| m.keys().copied()))
                .collect();
            let mut state = BTreeMap::new();
            for r in regs {
                let vals: Vec<SsaValue> = ps
                    .iter()
                    .filter_map(|p| old.get(p).and_then(|m| m.get(&r)).copied())
                    .collect();
                if vals.is_empty() {
                    continue;
                }
                if vals.iter().all(|v| *v == vals[0]) {
                    state.insert(r, vals[0]);
                } else {
                    let v = *phi_ids.entry((b.id, r)).or_insert_with(|| {
                        let v = SsaValue(next);
                        next += 1;
                        v
                    });
                    state.insert(r, v);
                }
            }
            ins.insert(b.id, state.clone());
            for i in &b.instructions {
                if let Some(r) = written(&i.op) {
                    state.insert(r, defs[&(i.pc, r)]);
                }
            }
            outs.insert(b.id, state);
        }
        if outs == old {
            break;
        }
        // The transfer function is monotone over a finite set of stable definition/phi ids. This
        // guard is diagnostic only; unlike the old 4*blocks cap it is not used as normal convergence.
        if iterations > cfg.blocks.len().max(1) * 256 {
            panic!("Maxwell SSA reaching-definition analysis did not converge");
        }
    }
    let mut blocks = Vec::new();
    for b in &cfg.blocks {
        let ps = preds.get(&b.id).cloned().unwrap_or_default();
        let mut phis = Vec::new();
        for ((bid, r), v) in phi_ids.iter().filter(|((bid, _), _)| *bid == b.id) {
            let incoming = ps
                .iter()
                .filter_map(|p| outs.get(p).and_then(|m| m.get(r)).copied().map(|x| (*p, x)))
                .collect();
            phis.push(PhiNode {
                block: *bid,
                register: *r,
                value: *v,
                incoming,
            });
        }
        let mut state = ins.get(&b.id).cloned().unwrap_or_default();
        let mut instructions = Vec::new();
        for i in &b.instructions {
            let sources = read_regs(&i.op)
                .into_iter()
                .map(|r| state.get(&r).copied().unwrap_or(SsaValue(0)))
                .collect();
            let dst = written(&i.op).map(|r| {
                let v = defs[&(i.pc, r)];
                state.insert(r, v);
                v
            });
            instructions.push(SsaInstruction {
                pc: i.pc,
                op: i.op.clone(),
                dst,
                sources,
            });
        }
        blocks.push(SsaBlock {
            id: b.id,
            phis,
            instructions,
            successors: b.successors.clone(),
        });
    }
    SsaProgram { blocks }
}
