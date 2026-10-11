// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded symbolic argument provenance; never emits instruction bytes.

use exosphere_cpu_ir::{DecodedOpcode, MoveWideKind, decode::decode_one_at};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expression {
    Input(u8),
    Constant(u64),
    Truncate32(Box<Expression>),
    Offset(Box<Expression>, u64),
    Subtract(Box<Expression>, u64),
    LeftShift(Box<Expression>, u8),
    LeadingZeros {
        input: Box<Expression>,
        bits: u8,
    },
    LogicalRightShift {
        input: Box<Expression>,
        amount: Box<Expression>,
        bits: u8,
    },
    UnsignedBitfield {
        input: Box<Expression>,
        rotate: u8,
        end: u8,
        bits: u8,
    },
    Sum(Box<Expression>, Box<Expression>),
    Difference(Box<Expression>, Box<Expression>),
    Select {
        condition: u8,
        when_true: Box<Expression>,
        when_false: Box<Expression>,
    },
    Product(Box<Expression>, Box<Expression>),
    LogicalMask {
        input: Box<Expression>,
        mask: u64,
        operation: u8,
    },
    Load {
        base: Box<Expression>,
        offset: u64,
        bits: u16,
    },
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CallArguments {
    pub site: usize,
    pub target: u64,
    pub argument0: Expression,
    pub argument1: Expression,
    pub argument2: Expression,
    /// AAPCS64 integer argument registers X3 through X7, in that order.
    pub additional_arguments: [Expression; 5],
    pub unmodeled: Vec<(usize, &'static str)>,
}

#[derive(Debug)]
pub struct IndirectCall {
    pub site: usize,
    pub register: u8,
    pub target: Expression,
    pub argument0: Expression,
    pub argument1: Expression,
    pub argument2: Expression,
    pub unmodeled: Vec<(usize, &'static str)>,
}

pub fn indirect_call_sites(bytes: &[u8], start: usize, length: usize) -> Vec<usize> {
    if start % 4 != 0 {
        return vec![];
    }
    let Some(bytes) = bytes.get(start..start.saturating_add(length).min(bytes.len())) else {
        return vec![];
    };
    bytes
        .chunks_exact(4)
        .enumerate()
        .filter_map(|(index, word)| {
            matches!(
                decode_one_at(u32::from_le_bytes(word.try_into().unwrap()), 0),
                DecodedOpcode::BranchReg { link: true, .. }
            )
            .then_some(start + index * 4)
        })
        .collect()
}

pub fn first_indirect_call(bytes: &[u8], start: usize) -> Option<IndirectCall> {
    let state = probe_registers(bytes, start, None)?;
    let register = state.target_register?;
    Some(IndirectCall {
        site: state.site,
        register,
        target: state.registers[register as usize].clone(),
        argument0: state.registers[0].clone(),
        argument1: state.registers[1].clone(),
        argument2: state.registers[2].clone(),
        unmodeled: state.unmodeled,
    })
}

/// Resolve a specific call from a bounded straight-line predecessor window.
pub fn indirect_call_at(bytes: &[u8], site: usize) -> Option<IndirectCall> {
    let call = first_indirect_call(bytes, straight_line_start(bytes, site)?)?;
    (call.site == site).then_some(call)
}

/// A tail BR transfers control without setting LR. Snapshot its target and
/// arguments before the branch; never treat nearby BLR sites as equivalent.
pub fn indirect_tail_at(bytes: &[u8], site: usize) -> Option<IndirectCall> {
    let word: [u8; 4] = bytes.get(site..site.checked_add(4)?)?.try_into().ok()?;
    let DecodedOpcode::BranchReg { rn, link: false } =
        decode_one_at(u32::from_le_bytes(word), site as u64)
    else {
        return None;
    };
    let state = probe_registers(bytes, straight_line_start(bytes, site)?, Some(site))?;
    if state.site != site {
        return None;
    }
    Some(IndirectCall {
        site,
        register: rn,
        target: state.registers[rn as usize].clone(),
        argument0: state.registers[0].clone(),
        argument1: state.registers[1].clone(),
        argument2: state.registers[2].clone(),
        unmodeled: state.unmodeled,
    })
}

pub fn direct_call_at(bytes: &[u8], site: usize) -> Option<CallArguments> {
    let call = first_call(bytes, straight_line_start(bytes, site)?)?;
    (call.site == site).then_some(call)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterBank {
    Integer,
    Simd,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FieldWrite {
    pub site: usize,
    pub source: u8,
    pub base: u8,
    pub bank: RegisterBank,
    pub source_bits: u16,
    pub source_bit_offset: u16,
    pub field_bit_offset: u16,
    pub bits_written: u16,
}

/// Report every overlap with a field, including partial and wider stores. Only
/// offset-mode addressing is admitted: base identity across writeback remains
/// outside this probe. A matching offset does not prove matching object identity.
pub fn field_writes(
    bytes: &[u8],
    start: usize,
    length: usize,
    field: u64,
    bits: u16,
) -> Vec<FieldWrite> {
    if start % 4 != 0 || bits == 0 || bits % 8 != 0 {
        return vec![];
    }
    let Ok(field_start) = i64::try_from(field) else {
        return vec![];
    };
    let Some(field_end) = field_start.checked_add(i64::from(bits / 8)) else {
        return vec![];
    };
    let Some(bytes) = bytes.get(start..start.saturating_add(length).min(bytes.len())) else {
        return vec![];
    };
    let mut output = Vec::new();
    for (index, word) in bytes.chunks_exact(4).enumerate() {
        let raw = u32::from_le_bytes(word.try_into().unwrap());
        let base = ((raw >> 5) & 31) as u8;
        for (source, offset, source_bits, bank) in store_operands(raw) {
            let end = offset + i64::from(source_bits / 8);
            let overlap_start = offset.max(field_start);
            let overlap_end = end.min(field_end);
            if overlap_start >= overlap_end {
                continue;
            }
            output.push(FieldWrite {
                site: start + index * 4,
                source,
                base,
                bank,
                source_bits,
                source_bit_offset: ((overlap_start - offset) * 8) as u16,
                field_bit_offset: ((overlap_start - field_start) * 8) as u16,
                bits_written: ((overlap_end - overlap_start) * 8) as u16,
            });
        }
    }
    output
}

fn store_operands(raw: u32) -> Vec<(u8, i64, u16, RegisterBank)> {
    // STUR uses a signed byte offset, even for vector registers. Mask out
    // only the immediate and register fields; loads and writeback must not
    // masquerade as an offset-mode store.
    let unscaled = match raw & 0xffe00c00 {
        0x38000000 => Some((8, RegisterBank::Integer)),
        0x78000000 => Some((16, RegisterBank::Integer)),
        0xb8000000 => Some((32, RegisterBank::Integer)),
        0xf8000000 => Some((64, RegisterBank::Integer)),
        0x3c000000 => Some((8, RegisterBank::Simd)),
        0x7c000000 => Some((16, RegisterBank::Simd)),
        0xbc000000 => Some((32, RegisterBank::Simd)),
        0xfc000000 => Some((64, RegisterBank::Simd)),
        0x3c800000 => Some((128, RegisterBank::Simd)),
        _ => None,
    };
    if let Some((bits, bank)) = unscaled {
        let offset = (((raw >> 12) & 0x1ff) as i64) << 55 >> 55;
        return vec![((raw & 31) as u8, offset, bits, bank)];
    }
    if let DecodedOpcode::LoadStoreUnsigned {
        rt,
        offset,
        load: false,
        size_bits,
        ..
    } = decode_one_at(raw, 0)
    {
        return vec![(rt, offset as i64, size_bits, RegisterBank::Integer)];
    }
    let class = raw & 0xffc00000;
    let pair = match class {
        0x29000000 => Some((32, RegisterBank::Integer)),
        0xa9000000 => Some((64, RegisterBank::Integer)),
        0x2d000000 => Some((32, RegisterBank::Simd)),
        0x6d000000 => Some((64, RegisterBank::Simd)),
        0xad000000 => Some((128, RegisterBank::Simd)),
        _ => None,
    };
    if let Some((bits, bank)) = pair {
        let immediate = (((raw >> 15) & 0x7f) as i64) << 57 >> 57;
        let offset = immediate * i64::from(bits / 8);
        return vec![
            ((raw & 31) as u8, offset, bits, bank),
            (
                ((raw >> 10) & 31) as u8,
                offset + i64::from(bits / 8),
                bits,
                bank,
            ),
        ];
    }
    let bits = match class {
        0x3d000000 => 8,
        0x7d000000 => 16,
        0xbd000000 => 32,
        0xfd000000 => 64,
        0x3d800000 => 128,
        _ => return vec![],
    };
    let offset = i64::from((raw >> 10) & 0xfff) * i64::from(bits / 8);
    vec![((raw & 31) as u8, offset, bits, RegisterBank::Simd)]
}

/// Analyze at most sixteen straight-line instructions. Inputs refer to the
/// specified start, not automatically to function parameters. Unknown operations
/// invalidate state; a branch stops analysis rather than assuming fallthrough.
pub fn first_call(bytes: &[u8], start: usize) -> Option<CallArguments> {
    let state = probe_registers(bytes, start, None)?;
    Some(CallArguments {
        site: state.site,
        target: state.target?,
        argument0: state.registers[0].clone(),
        argument1: state.registers[1].clone(),
        argument2: state.registers[2].clone(),
        additional_arguments: std::array::from_fn(|index| state.registers[index + 3].clone()),
        unmodeled: state.unmodeled,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct RegisterOrigin {
    pub value: Expression,
    pub unmodeled: Vec<(usize, &'static str)>,
}

/// Snapshot a register at an exact site, never crossing a call or branch.
pub fn register_origin(
    bytes: &[u8],
    start: usize,
    site: usize,
    register: u8,
) -> Option<RegisterOrigin> {
    if register >= 32 || site % 4 != 0 || site < start {
        return None;
    }
    let state = probe_registers(bytes, start, Some(site))?;
    if state.site != site {
        return None;
    }
    Some(RegisterOrigin {
        value: state.registers[register as usize].clone(),
        unmodeled: state.unmodeled,
    })
}

#[derive(Debug)]
pub struct JoinedCallArguments {
    pub start: usize,
    pub reaching_paths: usize,
    pub call: CallArguments,
}

pub fn preceding_transfer_kind(bytes: &[u8], entry: usize) -> Option<&'static str> {
    let site = entry.checked_sub(4)?;
    if site % 4 != 0 {
        return None;
    }
    let word = u32::from_le_bytes(bytes.get(site..entry)?.try_into().ok()?);
    match decode_one_at(word, site as u64) {
        DecodedOpcode::Ret { .. } => Some("return"),
        DecodedOpcode::BranchReg { link: true, .. } => Some("indirect-call"),
        DecodedOpcode::BranchReg { link: false, .. } => Some("indirect-branch"),
        DecodedOpcode::BranchImm { link: true, .. } => Some("direct-call"),
        _ => None,
    }
}

/// Choose a bounded suffix after the last call/return/indirect transfer. Unlike
/// straight-line probes, conditional branches do not reset incoming values.
/// This remains an assumed local entry, not whole-function dominance analysis.
pub fn direct_call_paths_at(bytes: &[u8], site: usize) -> Option<JoinedCallArguments> {
    if site % 4 != 0 {
        return None;
    }
    let window = site.saturating_sub(60);
    let mut start = window;
    for (index, word) in bytes.get(window..site)?.chunks_exact(4).enumerate() {
        if matches!(
            decode_one_at(
                u32::from_le_bytes(word.try_into().ok()?),
                (window + index * 4) as u64
            ),
            DecodedOpcode::BranchImm { link: true, .. }
                | DecodedOpcode::BranchReg { .. }
                | DecodedOpcode::Ret { .. }
        ) {
            start = window + index * 4 + 4;
        }
    }
    direct_call_paths(bytes, start, site)
}

/// Join forward paths from an explicitly assumed window entry, not a proven
/// function entry. Flags are unknown: both conditional successors are explored.
/// Loops, intervening calls and excessive path growth reject the whole probe.
pub fn direct_call_paths(bytes: &[u8], start: usize, site: usize) -> Option<JoinedCallArguments> {
    if start % 4 != 0 || site % 4 != 0 || site < start || site - start > 1024 {
        return None;
    }
    let word_at = |pc: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            bytes.get(pc..pc.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    let DecodedOpcode::BranchImm { target, link: true } =
        decode_one_at(word_at(site)?, site as u64)
    else {
        return None;
    };
    let reachable = call_reachable_offsets(bytes, start, site)?;
    let inputs = std::array::from_fn(|index| Expression::Input(index as u8));
    let mut pending = vec![(start, inputs, Vec::new())];
    let mut joined: Option<[Expression; 32]> = None;
    let mut unmodeled = Vec::new();
    let mut reaching_paths = 0;
    let mut visited = 0;
    while let Some((pc, registers, prior_unmodeled)) = pending.pop() {
        visited += 1;
        if visited > 512 {
            return None;
        }
        if pc < start || pc > site {
            continue;
        }
        if !reachable[(pc - start) / 4] {
            continue;
        }
        if pc == site {
            reaching_paths += 1;
            if let Some(joined) = &mut joined {
                for (value, incoming) in joined.iter_mut().zip(registers) {
                    if *value != incoming {
                        *value = Expression::Unknown;
                    }
                }
            } else {
                joined = Some(registers);
            }
            unmodeled.extend(prior_unmodeled);
            continue;
        }
        let opcode = decode_one_at(word_at(pc)?, pc as u64);
        match opcode {
            DecodedOpcode::CondBranch { target, .. }
            | DecodedOpcode::CompareBranch { target, .. }
            | DecodedOpcode::TestBranch { target, .. }
            | DecodedOpcode::BranchImm {
                target,
                link: false,
            } => {
                let destination = usize::try_from(target).ok()?;
                if destination <= pc {
                    return None;
                }
                if !matches!(opcode, DecodedOpcode::BranchImm { .. }) {
                    pending.push((pc + 4, registers.clone(), prior_unmodeled.clone()));
                }
                pending.push((destination, registers, prior_unmodeled));
            }
            DecodedOpcode::Ret { .. } => {}
            DecodedOpcode::BranchImm { link: true, .. } | DecodedOpcode::BranchReg { .. } => {
                return None;
            }
            _ => {
                let state = probe_registers_with_inputs(bytes, pc, Some(pc + 4), registers)?;
                if state.site != pc + 4 {
                    return None;
                }
                let mut path_unmodeled = prior_unmodeled;
                path_unmodeled.extend(state.unmodeled);
                pending.push((pc + 4, state.registers, path_unmodeled));
            }
        }
    }
    let registers = joined?;
    unmodeled.sort_unstable();
    unmodeled.dedup();
    Some(JoinedCallArguments {
        start,
        reaching_paths,
        call: CallArguments {
            site,
            target,
            argument0: registers[0].clone(),
            argument1: registers[1].clone(),
            argument2: registers[2].clone(),
            additional_arguments: std::array::from_fn(|index| registers[index + 3].clone()),
            unmodeled,
        },
    })
}

/// Reverse local reachability prunes paths that end in RET before the queried
/// call. Calls retain a possible return edge, and unknown indirect transfers
/// conservatively reach the query, so they cannot hide an unsupported path.
fn call_reachable_offsets(bytes: &[u8], start: usize, site: usize) -> Option<Vec<bool>> {
    let last = (site - start) / 4;
    let mut predecessors = vec![Vec::new(); last + 1];
    for index in 0..last {
        let pc = start + index * 4;
        let raw = u32::from_le_bytes(bytes.get(pc..pc + 4)?.try_into().ok()?);
        let mut add_target = |target: u64| {
            if let Ok(target) = usize::try_from(target)
                && target >= start
                && target <= site
                && target % 4 == 0
            {
                predecessors[(target - start) / 4].push(index);
            }
        };
        match decode_one_at(raw, pc as u64) {
            DecodedOpcode::Ret { .. } => {}
            DecodedOpcode::BranchReg { link: false, .. } => add_target(site as u64),
            DecodedOpcode::BranchImm {
                target,
                link: false,
            } => add_target(target),
            DecodedOpcode::CondBranch { target, .. }
            | DecodedOpcode::CompareBranch { target, .. }
            | DecodedOpcode::TestBranch { target, .. } => {
                add_target(target);
                add_target((pc + 4) as u64);
            }
            _ => add_target((pc + 4) as u64),
        }
    }
    let mut reachable = vec![false; last + 1];
    let mut pending = vec![last];
    while let Some(index) = pending.pop() {
        if reachable[index] {
            continue;
        }
        reachable[index] = true;
        pending.extend(predecessors[index].iter().copied());
    }
    Some(reachable)
}

/// Explain conservative barriers without exposing instruction words. A call on
/// a path that provably ends in RET before the query is not a barrier here.
pub fn call_path_barriers(
    bytes: &[u8],
    start: usize,
    site: usize,
) -> Option<Vec<(usize, &'static str)>> {
    if start % 4 != 0 || site % 4 != 0 || site < start || site - start > 1024 {
        return None;
    }
    let reachable = call_reachable_offsets(bytes, start, site)?;
    let mut barriers = Vec::new();
    for (index, &reaches_query) in reachable.iter().enumerate().take(reachable.len() - 1) {
        if !reaches_query {
            continue;
        }
        let pc = start + index * 4;
        let word = u32::from_le_bytes(bytes.get(pc..pc + 4)?.try_into().ok()?);
        let kind = match decode_one_at(word, pc as u64) {
            DecodedOpcode::BranchImm { link: true, .. } => "direct-call",
            DecodedOpcode::BranchReg { link: true, .. } => "indirect-call",
            DecodedOpcode::BranchReg { link: false, .. } => "indirect-branch",
            DecodedOpcode::BranchImm {
                target,
                link: false,
            }
            | DecodedOpcode::CondBranch { target, .. }
            | DecodedOpcode::CompareBranch { target, .. }
            | DecodedOpcode::TestBranch { target, .. }
                if target <= pc as u64 =>
            {
                "backward-edge"
            }
            _ => continue,
        };
        barriers.push((pc, kind));
    }
    Some(barriers)
}

#[derive(Debug, PartialEq, Eq)]
pub struct WriteOrigin {
    pub start: usize,
    pub source: Expression,
    pub base: Expression,
    pub unmodeled: Vec<(usize, &'static str)>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct StoreOrigin {
    pub offset: i64,
    pub bits: u16,
    pub source: Expression,
    pub base: Expression,
}

/// Offset-mode integer stores, including negative frame offsets. Each value
/// comes from a fresh bounded straight-line suffix; no memory is forwarded.
pub fn store_origins_at(bytes: &[u8], site: usize) -> Vec<StoreOrigin> {
    let Some(word) = bytes.get(site..site.saturating_add(4)) else {
        return vec![];
    };
    if site % 4 != 0 {
        return vec![];
    }
    let raw = u32::from_le_bytes(word.try_into().unwrap());
    let Some(start) = straight_line_start(bytes, site) else {
        return vec![];
    };
    let Some(state) = probe_registers(bytes, start, Some(site)) else {
        return vec![];
    };
    if state.site != site {
        return vec![];
    }
    store_operands(raw)
        .into_iter()
        .filter_map(|(source, offset, bits, bank)| {
            if bank != RegisterBank::Integer {
                return None;
            }
            Some(StoreOrigin {
                offset,
                bits,
                source: if source == 31 {
                    Expression::Constant(0)
                } else {
                    state.registers[source as usize].clone()
                },
                base: state.registers[((raw >> 5) & 31) as usize].clone(),
            })
        })
        .collect()
}

/// Inspect only the straight-line suffix before a store. Resetting at the last
/// control-flow edge deliberately loses earlier provenance rather than guessing
/// a path or carrying volatile registers across a call.
pub fn write_origin(bytes: &[u8], write: &FieldWrite) -> Option<WriteOrigin> {
    if write.bank != RegisterBank::Integer
        || write.site % 4 != 0
        || write.source >= 32
        || write.base >= 32
    {
        return None;
    }
    let start = straight_line_start(bytes, write.site)?;
    let state = probe_registers(bytes, start, Some(write.site))?;
    if state.site != write.site {
        return None;
    }
    Some(WriteOrigin {
        start,
        source: if write.source == 31 {
            Expression::Constant(0)
        } else {
            state.registers[write.source as usize].clone()
        },
        base: state.registers[write.base as usize].clone(),
        unmodeled: state.unmodeled,
    })
}

fn straight_line_start(bytes: &[u8], site: usize) -> Option<usize> {
    if site % 4 != 0 {
        return None;
    }
    let window = site.saturating_sub(60);
    let mut start = window;
    for (index, word) in bytes.get(window..site)?.chunks_exact(4).enumerate() {
        if matches!(
            decode_one_at(u32::from_le_bytes(word.try_into().unwrap()), 0),
            DecodedOpcode::BranchImm { .. }
                | DecodedOpcode::BranchReg { .. }
                | DecodedOpcode::CondBranch { .. }
                | DecodedOpcode::CompareBranch { .. }
                | DecodedOpcode::TestBranch { .. }
                | DecodedOpcode::Ret { .. }
        ) {
            // Indexing is relative to the original bounded window.
            start = window + index * 4 + 4;
        }
    }
    Some(start)
}

struct RegisterState {
    site: usize,
    target: Option<u64>,
    target_register: Option<u8>,
    registers: [Expression; 32],
    unmodeled: Vec<(usize, &'static str)>,
}

/// ADD/SUB (shifted register), LSL only. Register 31 means ZR here,
/// unlike the SP meaning in immediate and extended-register addressing.
fn shifted_arithmetic(raw: u32, registers: &[Expression; 32]) -> Option<(usize, Expression)> {
    if raw & 0x1fe00000 != 0x0b000000 {
        return None;
    }
    let wide = raw >> 31 != 0;
    let shift = ((raw >> 10) & 63) as u8;
    if !wide && shift >= 32 {
        return None;
    }
    let read = |index: usize| {
        if index == 31 {
            Expression::Constant(0)
        } else {
            registers[index].clone()
        }
    };
    let left = read(((raw >> 5) & 31) as usize);
    let right = read(((raw >> 16) & 31) as usize);
    let right = match right {
        Expression::Constant(value) => Expression::Constant(value.wrapping_shl(u32::from(shift))),
        Expression::Unknown => Expression::Unknown,
        _ if shift == 0 => right,
        _ => Expression::LeftShift(Box::new(right), shift),
    };
    let subtract = raw & (1 << 30) != 0;
    let value = match (left, right) {
        (Expression::Unknown, _) | (_, Expression::Unknown) => Expression::Unknown,
        (Expression::Constant(left), Expression::Constant(right)) => {
            Expression::Constant(if subtract {
                left.wrapping_sub(right)
            } else {
                left.wrapping_add(right)
            })
        }
        (left, Expression::Constant(0)) => left,
        (left, Expression::Constant(right)) if subtract => {
            Expression::Subtract(Box::new(left), right)
        }
        (left, Expression::Constant(right)) => Expression::Offset(Box::new(left), right),
        (left, right) if subtract => Expression::Difference(Box::new(left), Box::new(right)),
        (left, right) => Expression::Sum(Box::new(left), Box::new(right)),
    };
    let value = if wide {
        value
    } else {
        match value {
            Expression::Constant(value) => Expression::Constant(value & u64::from(u32::MAX)),
            Expression::Unknown => Expression::Unknown,
            _ => Expression::Truncate32(Box::new(value)),
        }
    };
    Some(((raw & 31) as usize, value))
}

/// A64 DecodeBitMasks for logical immediates: rotate a nonempty run of ones
/// within its element, then replicate. All-ones elements are reserved here.
/// See Arm's AND (immediate) / DecodeBitMasks pseudocode.
fn logical_mask(encoded: u32, wide: bool) -> Option<u64> {
    if encoded > 0x1fff || (!wide && encoded & 0x1000 != 0) {
        return None;
    }
    let imms = encoded & 63;
    let immr = (encoded >> 6) & 63;
    let selector = ((encoded >> 12) << 6) | ((!imms) & 63);
    if selector < 2 {
        return None;
    }
    let length = 31 - selector.leading_zeros();
    let element_bits = 1u32 << length;
    let levels = element_bits - 1;
    let ones = imms & levels;
    if ones == levels {
        return None;
    }
    let rotation = immr & levels;
    let element_mask = u64::MAX >> (64 - element_bits);
    let run = u64::MAX >> (63 - ones);
    let element = if rotation == 0 {
        run
    } else {
        ((run >> rotation) | (run << (element_bits - rotation))) & element_mask
    };
    let mut value = 0;
    for shift in (0..if wide { 64 } else { 32 }).step_by(element_bits as usize) {
        value |= element << shift;
    }
    Some(value)
}

fn probe_registers(bytes: &[u8], start: usize, stop: Option<usize>) -> Option<RegisterState> {
    probe_registers_with_inputs(
        bytes,
        start,
        stop,
        std::array::from_fn(|index| Expression::Input(index as u8)),
    )
}

fn probe_registers_with_inputs(
    bytes: &[u8],
    start: usize,
    stop: Option<usize>,
    mut registers: [Expression; 32],
) -> Option<RegisterState> {
    if start % 4 != 0 {
        return None;
    }
    let bytes = bytes.get(start..)?;
    let mut unmodeled = Vec::new();
    for (index, word) in bytes.chunks_exact(4).take(16).enumerate() {
        let raw = u32::from_le_bytes(word.try_into().unwrap());
        let pc = start + index * 4;
        if stop == Some(pc) {
            return Some(RegisterState {
                site: pc,
                target: None,
                target_register: None,
                registers,
                unmodeled,
            });
        }
        if let Some((destination, value)) = shifted_arithmetic(raw, &registers) {
            if destination != 31 {
                registers[destination] = value;
            }
            continue;
        }
        if let Some((destination, value)) = bound_arithmetic(raw, &registers) {
            if destination != 31 {
                registers[destination] = value;
            }
            continue;
        }
        if let Some((destination, value)) = unsigned_bitfield(raw, &registers) {
            if destination != 31 {
                registers[destination] = value;
            }
            continue;
        }
        // CSEL does not modify flags or other registers. Preserve both possible
        // values; this walker does not evaluate the incoming condition flags.
        if raw & 0x7fe00c00 == 0x1a800000 {
            let destination = (raw & 31) as usize;
            if destination != 31 {
                let read = |index: usize| {
                    if index == 31 {
                        Expression::Constant(0)
                    } else {
                        registers[index].clone()
                    }
                };
                let left = read(((raw >> 5) & 31) as usize);
                let right = read(((raw >> 16) & 31) as usize);
                let value = if left == right {
                    left
                } else {
                    Expression::Select {
                        condition: ((raw >> 12) & 15) as u8,
                        when_true: Box::new(left),
                        when_false: Box::new(right),
                    }
                };
                registers[destination] = word_width(value, raw >> 31 != 0);
            }
            continue;
        }
        // Unsigned scalar LDR with register-offset LSL/UXTX addressing only.
        // Sign-extending index modes are intentionally left unmodeled.
        if raw & 0x3be00c00 == 0x38600800 && (raw >> 13) & 7 == 3 {
            let destination = (raw & 31) as usize;
            if destination != 31 {
                let base = registers[((raw >> 5) & 31) as usize].clone();
                let index = ((raw >> 16) & 31) as usize;
                let index = if index == 31 {
                    Expression::Constant(0)
                } else {
                    registers[index].clone()
                };
                let size = (raw >> 30) as u8;
                let offset = if raw & (1 << 12) != 0 {
                    Expression::LeftShift(Box::new(index), size)
                } else {
                    index
                };
                registers[destination] = Expression::Load {
                    base: Box::new(Expression::Sum(Box::new(base), Box::new(offset))),
                    offset: 0,
                    bits: 8u16 << size,
                };
            }
            continue;
        }
        // Integer low-half MADD/MSUB and UMULL. The latter reads W sources
        // and writes their unsigned 64-bit product; signed widening is excluded.
        let unsigned_long = raw & 0xffe0fc00 == 0x9ba07c00;
        if raw & 0x7fe00000 == 0x1b000000 || unsigned_long {
            let destination = (raw & 31) as usize;
            if destination != 31 {
                let read = |index: usize| {
                    if index == 31 {
                        Expression::Constant(0)
                    } else {
                        registers[index].clone()
                    }
                };
                let left = word_width(read(((raw >> 5) & 31) as usize), !unsigned_long);
                let right = word_width(read(((raw >> 16) & 31) as usize), !unsigned_long);
                let product = match (&left, &right) {
                    (Expression::Constant(left), Expression::Constant(right)) => {
                        Expression::Constant(left.wrapping_mul(*right))
                    }
                    _ => Expression::Product(Box::new(left), Box::new(right)),
                };
                let accumulator = read(((raw >> 10) & 31) as usize);
                let subtract = raw & (1 << 15) != 0;
                let value = match (product, accumulator) {
                    (Expression::Constant(product), Expression::Constant(accumulator)) => {
                        Expression::Constant(if subtract {
                            accumulator.wrapping_sub(product)
                        } else {
                            accumulator.wrapping_add(product)
                        })
                    }
                    (product, Expression::Constant(0)) if !subtract => product,
                    (product, accumulator) if subtract => {
                        Expression::Difference(Box::new(accumulator), Box::new(product))
                    }
                    (product, accumulator) => {
                        Expression::Sum(Box::new(accumulator), Box::new(product))
                    }
                };
                registers[destination] = word_width(value, raw >> 31 != 0);
            }
            continue;
        }
        // Offset-mode stores change memory, not integer argument registers.
        // They do not establish memory contents in this register-only model.
        // The shared store decoder excludes pre/post-index writeback.
        if !store_operands(raw).is_empty() {
            continue;
        }
        // Compiler prologues commonly save pairs with pre-indexed SP. This
        // narrow form updates only SP; no memory forwarding is inferred.
        if (raw >> 5) & 31 == 31
            && matches!(
                raw & 0xffc00000,
                0x29800000 | 0xa9800000 | 0x2d800000 | 0x6d800000 | 0xad800000
            )
        {
            let bytes_per_register = match raw & 0xffc00000 {
                0x29800000 | 0x2d800000 => 4,
                0xad800000 => 16,
                _ => 8,
            };
            let offset = ((((raw >> 15) & 0x7f) as i64) << 57 >> 57) * bytes_per_register;
            let base = registers[31].clone();
            registers[31] = if offset < 0 {
                Expression::Subtract(Box::new(base), offset.unsigned_abs())
            } else {
                Expression::Offset(Box::new(base), offset as u64)
            };
            continue;
        }
        let unscaled_bits = match raw & 0xffe00c00 {
            0x38400000 => Some(8),
            0x78400000 => Some(16),
            0xb8400000 => Some(32),
            0xf8400000 => Some(64),
            _ => None,
        };
        if let Some(bits) = unscaled_bits {
            let destination = (raw & 31) as usize;
            let base = registers[((raw >> 5) & 31) as usize].clone();
            let offset = (((raw >> 12) & 0x1ff) as i64) << 55 >> 55;
            if destination != 31 {
                let (base, offset) = if offset < 0 {
                    (
                        Expression::Subtract(Box::new(base), offset.unsigned_abs()),
                        0,
                    )
                } else {
                    (base, offset as u64)
                };
                registers[destination] = Expression::Load {
                    base: Box::new(base),
                    offset,
                    bits,
                };
            }
            continue;
        }
        if matches!(raw & 0xffc00000, 0xa9400000 | 0x29400000) {
            // Offset LDP reads both values from the original base, even when
            // one destination aliases it. No base writeback is admitted.
            let bits = if raw >> 31 != 0 { 64 } else { 32 };
            let offset = ((((raw >> 15) & 0x7f) as i64) << 57 >> 57) * i64::from(bits / 8);
            let base = registers[((raw >> 5) & 31) as usize].clone();
            for (destination, offset) in [
                ((raw & 31) as usize, offset),
                (((raw >> 10) & 31) as usize, offset + i64::from(bits / 8)),
            ] {
                if destination != 31 {
                    registers[destination] = match u64::try_from(offset) {
                        Ok(offset) => Expression::Load {
                            base: Box::new(base.clone()),
                            offset,
                            bits,
                        },
                        // Negative offsets are outside this expression model.
                        Err(_) => Expression::Unknown,
                    };
                }
            }
            continue;
        }
        // MOV Rd, Rm is ORR Rd, ZR, Rm without a shift. The conservative
        // project decoder leaves this alias unknown, so admit only this encoding.
        if raw & 0x7fe0ffe0 == 0x2a0003e0 {
            let source = ((raw >> 16) & 31) as usize;
            let destination = (raw & 31) as usize;
            let value = if source == 31 {
                Expression::Constant(0)
            } else {
                registers[source].clone()
            };
            registers[destination] = if raw >> 31 != 0 {
                value
            } else {
                match value {
                    Expression::Constant(value) => {
                        Expression::Constant(value & u64::from(u32::MAX))
                    }
                    Expression::Unknown => Expression::Unknown,
                    _ => Expression::Truncate32(Box::new(value)),
                }
            };
            continue;
        }
        match decode_one_at(raw, pc as u64) {
            DecodedOpcode::LogicalImm {
                rd,
                rn,
                raw_imm,
                op,
                is64,
            } => {
                let Some(mask) = logical_mask(raw_imm, is64) else {
                    unmodeled.push((pc, "invalid-logical-immediate"));
                    registers.fill(Expression::Unknown);
                    continue;
                };
                // ANDS/TST with Rd=ZR only affects flags. For other logical
                // immediates Rd=31 is SP, whereas source register 31 is ZR.
                if rd == 31 && op == 3 {
                    continue;
                }
                let input = if rn == 31 {
                    Expression::Constant(0)
                } else {
                    registers[rn as usize].clone()
                };
                let value = match input {
                    Expression::Constant(value) => Expression::Constant(match op {
                        0 | 3 => value & mask,
                        1 => value | mask,
                        2 => value ^ mask,
                        _ => unreachable!("decoder exposes a two-bit opcode"),
                    }),
                    Expression::Unknown => Expression::Unknown,
                    _ => Expression::LogicalMask {
                        input: Box::new(input),
                        mask,
                        operation: op,
                    },
                };
                registers[rd as usize] = if is64 {
                    value
                } else {
                    match value {
                        Expression::Constant(value) => {
                            Expression::Constant(value & u64::from(u32::MAX))
                        }
                        Expression::Unknown => Expression::Unknown,
                        _ => Expression::Truncate32(Box::new(value)),
                    }
                };
            }
            DecodedOpcode::BranchImm { target, link: true } => {
                return Some(RegisterState {
                    site: pc,
                    target: Some(target),
                    target_register: None,
                    registers,
                    unmodeled,
                });
            }
            DecodedOpcode::BranchReg { rn, link: true } => {
                return Some(RegisterState {
                    site: pc,
                    target: None,
                    target_register: Some(rn),
                    registers,
                    unmodeled,
                });
            }
            DecodedOpcode::BranchImm { .. }
            | DecodedOpcode::BranchReg { .. }
            | DecodedOpcode::CondBranch { .. }
            | DecodedOpcode::CompareBranch { .. }
            | DecodedOpcode::TestBranch { .. }
            | DecodedOpcode::Ret { .. } => return None,
            DecodedOpcode::MoveWide {
                rd,
                imm,
                kind,
                is64,
            } if rd != 31 => {
                let mask = if is64 { u64::MAX } else { u64::from(u32::MAX) };
                registers[rd as usize] = match kind {
                    MoveWideKind::MovZ => Expression::Constant(imm & mask),
                    MoveWideKind::MovN => Expression::Constant(!imm & mask),
                    MoveWideKind::MovK => match registers[rd as usize] {
                        Expression::Constant(old) => {
                            let shift = ((raw >> 21) & 3) * 16;
                            Expression::Constant(((old & !(0xffff << shift)) | imm) & mask)
                        }
                        _ => Expression::Unknown,
                    },
                };
            }
            DecodedOpcode::Adr { rd, target, .. } if rd != 31 => {
                registers[rd as usize] = Expression::Constant(target);
            }
            DecodedOpcode::AddSubImm {
                rd,
                rn,
                imm,
                sub,
                set_flags,
                is64,
            } => {
                // CMP/CMN aliases write flags only. Flags are deliberately not
                // modeled, since the walker never follows a conditional edge.
                if rd == 31 && set_flags {
                    continue;
                }
                let base = registers[rn as usize].clone();
                let value = match base {
                    Expression::Constant(value) => Expression::Constant(if sub {
                        value.wrapping_sub(imm)
                    } else {
                        value.wrapping_add(imm)
                    }),
                    Expression::Unknown => Expression::Unknown,
                    _ if imm == 0 => base,
                    _ if sub => Expression::Subtract(Box::new(base), imm),
                    _ => Expression::Offset(Box::new(base), imm),
                };
                registers[rd as usize] = if is64 {
                    value
                } else {
                    match value {
                        Expression::Constant(value) => {
                            Expression::Constant(value & u64::from(u32::MAX))
                        }
                        Expression::Unknown => Expression::Unknown,
                        _ => Expression::Truncate32(Box::new(value)),
                    }
                };
            }
            DecodedOpcode::LoadStoreUnsigned {
                rt,
                rn,
                offset,
                load: true,
                size_bits,
            } if rt != 31 => {
                registers[rt as usize] = Expression::Load {
                    base: Box::new(registers[rn as usize].clone()),
                    offset,
                    bits: size_bits,
                };
            }
            DecodedOpcode::LoadStoreUnsigned { load: false, .. } | DecodedOpcode::Nop => {}
            _ => {
                let kind = if raw & 0x7ffffc00 == 0x5ac01000 {
                    "count-leading-zero"
                } else if raw & 0x7fe0fc00 == 0x1ac00800 {
                    "unsigned-divide"
                } else if raw & 0x7fe0fc00 == 0x1ac02000 {
                    "variable-left-shift"
                } else if raw & 0x7fe0fc00 == 0x1ac02400 {
                    "variable-logical-right-shift"
                } else if raw & 0x3e000000 == 0x28000000 {
                    "integer-pair"
                } else if raw & 0x1f000000 == 0x0a000000 {
                    "logical-register"
                } else if raw & 0x1f000000 == 0x0b000000 {
                    "add-sub-register"
                } else if raw & 0x3b200c00 == 0x38000000 {
                    "unscaled-integer-memory"
                } else if raw & 0x3b200c00 == 0x38200800 {
                    "register-offset-memory"
                } else if raw & 0x1f800000 == 0x13000000 {
                    "bitfield"
                } else if raw & 0x1f800000 == 0x12000000 {
                    "logical-immediate"
                } else if raw & 0x1f800000 == 0x13800000 {
                    "extract"
                } else if raw & 0x1fe00000 == 0x1a800000 {
                    "conditional-select"
                } else if raw & 0x1f000000 == 0x11000000 {
                    "add-sub-immediate"
                } else if raw & 0x1f000000 == 0x1b000000 {
                    "multiply"
                } else if raw & 0x3b000000 == 0x18000000 {
                    "literal-memory"
                } else {
                    "unclassified"
                };
                unmodeled.push((pc, kind));
                registers.fill(Expression::Unknown);
            }
        }
    }
    None
}

fn word_width(value: Expression, is64: bool) -> Expression {
    if is64 {
        return value;
    }
    match value {
        Expression::Constant(value) => Expression::Constant(value & u64::from(u32::MAX)),
        Expression::Unknown => Expression::Unknown,
        _ => Expression::Truncate32(Box::new(value)),
    }
}

/// Narrow unsigned operations used by VA descriptor construction. Variable
/// shift counts wrap modulo the operand width; immediate LSR aliases do not.
fn bound_arithmetic(raw: u32, registers: &[Expression; 32]) -> Option<(usize, Expression)> {
    let wide = raw >> 31 != 0;
    let bits = if wide { 64 } else { 32 };
    let read = |index: usize| {
        word_width(
            if index == 31 {
                Expression::Constant(0)
            } else {
                registers[index].clone()
            },
            wide,
        )
    };
    let input = read(((raw >> 5) & 31) as usize);
    let value = if raw & 0x7ffffc00 == 0x5ac01000 {
        match input {
            Expression::Constant(value) => Expression::Constant(if wide {
                value.leading_zeros()
            } else {
                (value as u32).leading_zeros()
            } as u64),
            Expression::Unknown => Expression::Unknown,
            input => Expression::LeadingZeros {
                input: Box::new(input),
                bits,
            },
        }
    } else {
        let amount = if raw & 0x7fe0fc00 == 0x1ac02400 {
            read(((raw >> 16) & 31) as usize)
        } else if raw & 0x7f800000 == 0x53000000
            && ((raw >> 22) & 1 != 0) == wide
            && (raw >> 10) & 63 == u32::from(bits - 1)
            && (wide || (raw >> 16) & 63 < 32)
        {
            Expression::Constant(u64::from((raw >> 16) & 63))
        } else {
            return None;
        };
        match (input, amount) {
            (Expression::Constant(value), Expression::Constant(amount)) => {
                Expression::Constant(value >> (amount & u64::from(bits - 1)))
            }
            (Expression::Unknown, _) | (_, Expression::Unknown) => Expression::Unknown,
            (input, amount) => Expression::LogicalRightShift {
                input: Box::new(input),
                amount: Box::new(amount),
                bits,
            },
        }
    };
    Some(((raw & 31) as usize, value))
}

/// UBFM covers both extraction (non-wrapping) and zero-inserting left shifts
/// (wrapping). Unlike BFM it never reads the previous destination value.
fn unsigned_bitfield(raw: u32, registers: &[Expression; 32]) -> Option<(usize, Expression)> {
    let wide = raw >> 31 != 0;
    let bits = if wide { 64 } else { 32 };
    let rotate = ((raw >> 16) & 63) as u8;
    let end = ((raw >> 10) & 63) as u8;
    if raw & 0x7f800000 != 0x53000000
        || ((raw >> 22) & 1 != 0) != wide
        || rotate >= bits
        || end >= bits
    {
        return None;
    }
    let source = ((raw >> 5) & 31) as usize;
    let input = word_width(
        if source == 31 {
            Expression::Constant(0)
        } else {
            registers[source].clone()
        },
        wide,
    );
    let value = match input {
        Expression::Constant(input) => {
            let value = if end >= rotate {
                (input >> rotate) & (u64::MAX >> (63 - (end - rotate)))
            } else {
                (input & (u64::MAX >> (63 - end))) << (bits - rotate)
            };
            word_width(Expression::Constant(value), wide)
        }
        Expression::Unknown => Expression::Unknown,
        input => Expression::UnsignedBitfield {
            input: Box::new(input),
            rotate,
            end,
            bits,
        },
    };
    Some(((raw & 31) as usize, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[test]
    fn forward_call_paths_join_only_agreeing_values() {
        let bytes = words(&[0xd28000e1, 0x34000042, 0x14000002, 0xd28000e1, 0x94000000]);
        let joined = direct_call_paths(&bytes, 0, 16).unwrap();
        assert_eq!(joined.start, 0);
        assert_eq!(joined.reaching_paths, 2);
        assert_eq!(joined.call.argument1, Expression::Constant(7));
        assert!(joined.call.unmodeled.is_empty());
        let bytes = words(&[0xd28000e1, 0x34000042, 0x14000002, 0xd2800101, 0x94000000]);
        assert_eq!(
            direct_call_paths(&bytes, 0, 16).unwrap().call.argument1,
            Expression::Unknown
        );
        // An external successor does not reach this local call.
        let bytes = words(&[0xd28000e1, 0x34000082, 0x14000002, 0xd2800101, 0x94000000]);
        assert_eq!(direct_call_paths(&bytes, 0, 16).unwrap().reaching_paths, 1);
    }

    #[test]
    fn call_paths_prune_calls_only_on_paths_ending_before_query() {
        let bytes = words(&[0xd28000e1, 0x34000062, 0x94000000, 0xd65f03c0, 0x94000000]);
        let joined = direct_call_paths(&bytes, 0, 16).unwrap();
        assert_eq!(joined.reaching_paths, 1);
        assert_eq!(joined.call.argument1, Expression::Constant(7));
        assert_eq!(call_path_barriers(&bytes, 0, 16), Some(vec![]));
        let bytes = words(&[0xd28000e1, 0x34000062, 0xd63f0120, 0xd65f03c0, 0x94000000]);
        assert_eq!(direct_call_paths(&bytes, 0, 16).unwrap().reaching_paths, 1);
        let bytes = words(&[0xd28000e1, 0x34000062, 0x94000000, 0xd503201f, 0x94000000]);
        assert!(direct_call_paths(&bytes, 0, 16).is_none());
        assert_eq!(
            call_path_barriers(&bytes, 0, 16),
            Some(vec![(8, "direct-call")])
        );
        let bytes = words(&[0xd28000e1, 0x34000062, 0xd61f0120, 0xd65f03c0, 0x94000000]);
        assert!(direct_call_paths(&bytes, 0, 16).is_none());
        assert_eq!(
            call_path_barriers(&bytes, 0, 16),
            Some(vec![(8, "indirect-branch")])
        );
    }

    #[test]
    fn extended_call_windows_follow_forward_jumps_but_remain_bounded() {
        let mut opcodes = vec![0xd503201f; 148];
        opcodes[0] = 0xd28000e1;
        opcodes[1] = 0x14000092; // Skip to the query at byte 588.
        opcodes[147] = 0x94000000;
        assert_eq!(
            direct_call_paths(&words(&opcodes), 0, 588)
                .unwrap()
                .call
                .argument1,
            Expression::Constant(7)
        );
        assert!(direct_call_paths(&words(&opcodes), 0, 1028).is_none());
        assert!(call_path_barriers(&words(&opcodes), 0, 1028).is_none());
    }

    #[test]
    fn preceding_transfer_metadata_distinguishes_calls_returns_and_padding() {
        for (word, kind) in [
            (0xd65f03c0, Some("return")),
            (0xd63f0120, Some("indirect-call")),
            (0xd61f0120, Some("indirect-branch")),
            (0x94000000, Some("direct-call")),
            (0xd503201f, None),
        ] {
            assert_eq!(preceding_transfer_kind(&words(&[word]), 4), kind);
        }
        assert_eq!(preceding_transfer_kind(&[], 0), None);
        assert_eq!(preceding_transfer_kind(&words(&[0xd65f03c0]), 3), None);
    }

    #[test]
    fn unsigned_bitfields_match_bit_positions_for_all_valid_immediates() {
        let mut registers = std::array::from_fn(|i| Expression::Input(i as u8));
        for bits in [32u8, 64] {
            for rotate in 0..bits {
                for end in 0..bits {
                    let raw = if bits == 64 { 0xd3400020 } else { 0x53000020 }
                        | (u32::from(rotate) << 16)
                        | (u32::from(end) << 10);
                    for input in [0, u64::MAX, 0x8a57_9c31_e26b_40fd] {
                        registers[1] = Expression::Constant(input);
                        let mut expected = 0;
                        for source in 0..=end {
                            let destination = (source + bits - rotate) % bits;
                            if (end < rotate || source >= rotate) && input & (1u64 << source) != 0 {
                                expected |= 1u64 << destination;
                            }
                        }
                        assert_eq!(
                            unsigned_bitfield(raw, &registers),
                            Some((0, Expression::Constant(expected)))
                        );
                    }
                }
            }
        }
        for raw in [0x93483c20, 0xb3483c20, 0x5301fc20, 0xd301fc20] {
            assert_eq!(unsigned_bitfield(raw, &registers), None);
        }
        assert_eq!(
            unsigned_bitfield(0xd3483fe0, &registers),
            Some((0, Expression::Constant(0)))
        );
        registers[1] = Expression::Input(1);
        assert!(matches!(
            unsigned_bitfield(0xd3483c20, &registers),
            Some((
                0,
                Expression::UnsignedBitfield {
                    rotate: 8,
                    end: 15,
                    bits: 64,
                    ..
                }
            ))
        ));
    }

    #[test]
    fn call_path_suffix_keeps_conditionals_but_resets_at_returns_and_calls() {
        for barrier in [0xd65f03c0, 0x94000000, 0xd63f0120] {
            let bytes = words(&[
                barrier, 0xd28000e1, 0x34000042, 0x14000002, 0xd28000e1, 0x94000000,
            ]);
            let joined = direct_call_paths_at(&bytes, 20).unwrap();
            assert_eq!(joined.start, 4);
            assert_eq!(joined.reaching_paths, 2);
            assert_eq!(joined.call.argument1, Expression::Constant(7));
        }
        assert!(direct_call_paths_at(&[], 0).is_none());
        assert!(direct_call_paths_at(&words(&[0x94000000]), 1).is_none());
    }

    #[test]
    fn forward_call_paths_reject_loops_calls_and_excessive_growth() {
        for middle in [0x17fffffe, 0x94000000, 0xd63f0120, 0xd61f0120] {
            let bytes = words(&[0xd28000e1, 0x34000042, middle, 0xd28000e1, 0x94000000]);
            assert!(direct_call_paths(&bytes, 0, 16).is_none());
        }
        let mut opcodes = vec![0x34000020; 10];
        opcodes.push(0x94000000);
        assert!(direct_call_paths(&words(&opcodes), 0, 40).is_none());
        assert!(direct_call_paths(&[], 0, 0).is_none());
        assert!(direct_call_paths(&words(&[0xd503201f]), 0, 0).is_none());
        assert!(direct_call_paths(&words(&[0x94000000]), 1, 0).is_none());
    }

    #[test]
    fn bound_arithmetic_respects_width_zero_register_and_shift_modulo() {
        let mut registers = std::array::from_fn(|i| Expression::Input(i as u8));
        registers[1] = Expression::Constant(0x8000_0000_0000_0000);
        registers[2] = Expression::Constant(65);
        assert_eq!(
            bound_arithmetic(0xdac01020, &registers),
            Some((0, Expression::Constant(0)))
        );
        assert_eq!(
            bound_arithmetic(0x5ac01020, &registers),
            Some((0, Expression::Constant(32)))
        );
        assert_eq!(
            bound_arithmetic(0xdac013e0, &registers),
            Some((0, Expression::Constant(64)))
        );
        assert_eq!(
            bound_arithmetic(0x9ac22420, &registers),
            Some((0, Expression::Constant(0x4000_0000_0000_0000)))
        );
        registers[1] = Expression::Constant(u64::MAX);
        assert_eq!(
            bound_arithmetic(0x1ac22420, &registers),
            Some((0, Expression::Constant(0x7fff_ffff)))
        );
        // LSR X0, X1, #63 and LSR W0, W1, #31.
        assert_eq!(
            bound_arithmetic(0xd37ffc20, &registers),
            Some((0, Expression::Constant(1)))
        );
        assert_eq!(
            bound_arithmetic(0x531f7c20, &registers),
            Some((0, Expression::Constant(1)))
        );
        for raw in [0x9341fc20, 0x5301fc20, 0xd301fc20, 0x1ac22820] {
            assert_eq!(bound_arithmetic(raw, &registers), None);
        }
        registers[1] = Expression::Input(1);
        assert!(matches!(
            bound_arithmetic(0xdac01020, &registers),
            Some((0, Expression::LeadingZeros { bits: 64, .. }))
        ));
        assert!(matches!(
            bound_arithmetic(0x9ac22420, &registers),
            Some((0, Expression::LogicalRightShift { bits: 64, .. }))
        ));
    }

    #[test]
    fn unsupported_bound_arithmetic_reports_family_without_inventing_values() {
        for (opcode, family) in [
            (0x1ac20820, "unsigned-divide"),
            (0x1ac22020, "variable-left-shift"),
        ] {
            for width in [0, 1 << 31] {
                let call = first_call(&words(&[opcode | width, 0x94000000]), 0).unwrap();
                assert_eq!(call.unmodeled, vec![(0, family)]);
                assert_eq!(call.argument0, Expression::Unknown);
                assert_eq!(call.argument1, Expression::Unknown);
            }
        }
        // Signed division and arithmetic right shift are distinct operations.
        for opcode in [0x1ac20c20, 0x1ac22820] {
            let call = first_call(&words(&[opcode, 0x94000000]), 0).unwrap();
            assert_eq!(call.unmodeled, vec![(0, "unclassified")]);
        }
    }

    #[test]
    fn direct_calls_report_all_integer_argument_registers() {
        let bytes = words(&[0xd2800225, 0xd2800427, 0x94000000]);
        let call = first_call(&bytes, 0).unwrap();
        assert_eq!(
            call.additional_arguments,
            [
                Expression::Input(3),
                Expression::Input(4),
                Expression::Constant(17),
                Expression::Input(6),
                Expression::Constant(33)
            ]
        );
        assert_eq!(
            direct_call_at(&bytes, 8).unwrap().additional_arguments,
            call.additional_arguments
        );
    }

    #[test]
    fn indirect_tail_snapshot_requires_exact_nonlink_branch() {
        let bytes = words(&[0xf9402808, 0xd61f0100]);
        let tail = indirect_tail_at(&bytes, 4).unwrap();
        assert_eq!(tail.register, 8);
        assert_eq!(
            tail.target,
            Expression::Load {
                base: Box::new(Expression::Input(0)),
                offset: 80,
                bits: 64
            }
        );
        assert_eq!(tail.argument1, Expression::Input(1));
        assert!(indirect_tail_at(&words(&[0xd63f0100]), 0).is_none());
        assert!(indirect_tail_at(&bytes, 0).is_none());
        assert!(indirect_tail_at(&bytes, 3).is_none());
        assert!(indirect_tail_at(&bytes, bytes.len()).is_none());
    }

    #[test]
    fn signed_offset_store_origins_snapshot_each_integer_pair_source() {
        let bytes = words(&[0xd2800061, 0xd2800082, 0xa93f8ba1]);
        let origins = store_origins_at(&bytes, 8);
        assert_eq!(
            origins,
            [
                StoreOrigin {
                    offset: -8,
                    bits: 64,
                    source: Expression::Constant(3),
                    base: Expression::Input(29)
                },
                StoreOrigin {
                    offset: 0,
                    bits: 64,
                    source: Expression::Constant(4),
                    base: Expression::Input(29)
                },
            ]
        );
        let bytes = words(&[0xd2800061, 0x94000000, 0xf81f83a1]);
        assert_eq!(store_origins_at(&bytes, 8)[0].source, Expression::Input(1));
        assert_eq!(store_origins_at(&bytes, 8)[0].offset, -8);
        assert!(store_origins_at(&bytes, 7).is_empty());
        assert!(store_origins_at(&bytes, bytes.len()).is_empty());
        assert!(store_origins_at(&words(&[0xf9400020]), 0).is_empty());
        assert!(store_origins_at(&words(&[0xad0007e0]), 0).is_empty());
    }

    #[test]
    fn conditional_select_retains_alternatives_without_evaluating_flags() {
        let bytes = words(&[0x9a820020, 0x94000000]);
        let call = first_call(&bytes, 0).unwrap();
        assert_eq!(
            call.argument0,
            Expression::Select {
                condition: 0,
                when_true: Box::new(Expression::Input(1)),
                when_false: Box::new(Expression::Input(2)),
            }
        );
        assert_eq!(call.argument1, Expression::Input(1));
        assert!(call.unmodeled.is_empty());
        let same = first_call(&words(&[0x9a810020, 0x94000000]), 0).unwrap();
        assert_eq!(same.argument0, Expression::Input(1));
        let unsupported = first_call(&words(&[0x9a820420, 0x94000000]), 0).unwrap();
        assert_eq!(unsupported.argument0, Expression::Unknown);
    }

    #[test]
    fn indexed_load_tracks_lsl_scale_and_preserves_other_registers() {
        for (instruction, shift) in [(0xf8626820, None), (0xf8627820, Some(3))] {
            let call = first_call(&words(&[instruction, 0x94000000]), 0).unwrap();
            let index = match shift {
                Some(shift) => Expression::LeftShift(Box::new(Expression::Input(2)), shift),
                None => Expression::Input(2),
            };
            assert_eq!(
                call.argument0,
                Expression::Load {
                    base: Box::new(Expression::Sum(
                        Box::new(Expression::Input(1)),
                        Box::new(index)
                    )),
                    offset: 0,
                    bits: 64,
                }
            );
            assert_eq!(call.argument1, Expression::Input(1));
            assert!(call.unmodeled.is_empty());
        }
        let unsupported = first_call(&words(&[0xf862c820, 0x94000000]), 0).unwrap();
        assert_eq!(unsupported.argument0, Expression::Unknown);
    }

    #[test]
    fn low_half_multiply_propagates_values_and_word_wraparound() {
        for (instruction, expected) in [(0x9b020020, 17), (0x9b028020, u64::MAX - 6)] {
            let call = first_call(
                &words(&[0xd28000a0, 0xd2800061, 0xd2800082, instruction, 0x94000000]),
                0,
            )
            .unwrap();
            assert_eq!(call.argument0, Expression::Constant(expected));
        }
        // UMULL must truncate both X-register expressions to W operands first.
        let call =
            first_call(&words(&[0x92800001, 0xd2800042, 0x9ba27c20, 0x94000000]), 0).unwrap();
        assert_eq!(call.argument0, Expression::Constant(0x1fffffffe));
        let signed = first_call(&words(&[0x9b227c20, 0x94000000]), 0).unwrap();
        assert_eq!(signed.argument0, Expression::Unknown);
        let call =
            first_call(&words(&[0xd2800061, 0xd2800082, 0x9b027c20, 0x94000000]), 0).unwrap();
        assert_eq!(call.argument0, Expression::Constant(12));
        let call =
            first_call(&words(&[0x12800001, 0x52800082, 0x1b027c20, 0x94000000]), 0).unwrap();
        assert_eq!(call.argument0, Expression::Constant(0xfffffffc));
        let call = first_call(&words(&[0x9b027c20, 0x94000000]), 0).unwrap();
        assert_eq!(
            call.argument0,
            Expression::Product(
                Box::new(Expression::Input(1)),
                Box::new(Expression::Input(2))
            )
        );
        let call = first_call(&words(&[0x9b027c3f, 0x94000000]), 0).unwrap();
        assert_eq!(call.argument0, Expression::Input(0));
    }

    #[test]
    fn stack_pair_prologues_preserve_arguments_and_track_sp_writeback() {
        for save in [0xa9be7bfd, 0xadbf07e0] {
            let bytes = words(&[save, 0x910003e0, 0x94000000]);
            let call = first_call(&bytes, 0).unwrap();
            assert_eq!(
                call.argument0,
                Expression::Subtract(Box::new(Expression::Input(31)), 32)
            );
            assert_eq!(call.argument1, Expression::Input(1));
            assert!(call.unmodeled.is_empty());
        }
        let call = first_call(&words(&[0xa9be781d, 0x94000000]), 0).unwrap();
        assert_eq!(call.argument1, Expression::Unknown);
        assert!(!call.unmodeled.is_empty());
    }

    #[test]
    fn direct_call_at_reports_requested_call_not_an_earlier_neighbor() {
        let bytes = words(&[0x94000000, 0x9100a260, 0x94000002, 0xd503201f]);
        let call = direct_call_at(&bytes, 8).unwrap();
        assert_eq!(call.site, 8);
        assert_eq!(call.target, 16);
        assert_eq!(
            call.argument0,
            Expression::Offset(Box::new(Expression::Input(19)), 40)
        );
        assert!(direct_call_at(&bytes, 12).is_none());
        assert!(direct_call_at(&bytes, 9).is_none());
        assert!(direct_call_at(&bytes, 100).is_none());
    }

    #[test]
    fn register_snapshots_require_exact_reachable_sites() {
        let bytes = words(&[0xaa0003f4, 0x94000000, 0xd503201f]);
        let origin = register_origin(&bytes, 0, 4, 20).unwrap();
        assert_eq!(origin.value, Expression::Input(0));
        assert!(origin.unmodeled.is_empty());
        assert!(register_origin(&bytes, 0, 8, 20).is_none());
        assert!(register_origin(&bytes, 0, 4, 32).is_none());
        assert!(register_origin(&bytes, 0, 5, 20).is_none());
        assert!(register_origin(&bytes, 4, 0, 20).is_none());
        assert!(register_origin(&bytes, 0, 100, 20).is_none());
    }

    #[test]
    fn indirect_call_at_uses_full_suffix_but_never_crosses_prior_calls() {
        let bytes = words(&[
            0x52800029, 0x94000000, 0xf9402d09, 0xd503201f, 0xd503201f, 0xd503201f, 0xd63f0120,
        ]);
        let call = indirect_call_at(&bytes, 24).unwrap();
        assert_eq!(
            call.target,
            Expression::Load {
                base: Box::new(Expression::Input(8)),
                offset: 88,
                bits: 64
            }
        );
        assert!(call.unmodeled.is_empty());
        assert!(indirect_call_at(&bytes, 20).is_none());
        assert!(indirect_call_at(&bytes, 25).is_none());
        assert!(indirect_call_at(&bytes, 100).is_none());
    }

    #[test]
    fn unscaled_loads_retain_signed_address_without_reading_memory() {
        let bytes = words(&[0xf85e03a9, 0xb8428020, 0xd63f0120]);
        let call = first_indirect_call(&bytes, 0).unwrap();
        assert_eq!(
            call.target,
            Expression::Load {
                base: Box::new(Expression::Subtract(Box::new(Expression::Input(29)), 32)),
                offset: 0,
                bits: 64
            }
        );
        assert_eq!(
            call.argument0,
            Expression::Load {
                base: Box::new(Expression::Input(1)),
                offset: 40,
                bits: 32
            }
        );
        assert!(call.unmodeled.is_empty());
        for word in [0x78828020, 0xf85e0fa9] {
            let call = first_call(&words(&[word, 0x94000000]), 0).unwrap();
            assert_eq!(call.argument0, Expression::Unknown);
            assert!(!call.unmodeled.is_empty());
        }
    }

    #[test]
    fn indirect_calls_retain_loaded_target_without_claiming_a_static_address() {
        let bytes = words(&[0xf9400028, 0xf9402d09, 0xd63f0120, 0xd61f0120]);
        assert_eq!(indirect_call_sites(&bytes, 0, bytes.len()), [8]);
        let call = first_indirect_call(&bytes, 0).unwrap();
        assert_eq!(call.site, 8);
        assert_eq!(call.register, 9);
        assert_eq!(
            call.target,
            Expression::Load {
                base: Box::new(Expression::Load {
                    base: Box::new(Expression::Input(1)),
                    offset: 0,
                    bits: 64
                }),
                offset: 88,
                bits: 64
            }
        );
        assert_eq!(call.argument1, Expression::Input(1));
        assert!(call.unmodeled.is_empty());
        assert!(first_call(&bytes, 0).is_none());
        assert!(first_indirect_call(&bytes, 12).is_none());
        assert!(first_indirect_call(&words(&[0x94000000, 0xd63f0120]), 0).is_none());
        assert!(indirect_call_sites(&bytes, 1, 12).is_empty());
        assert!(indirect_call_sites(&bytes, 100, 12).is_empty());
        assert!(indirect_call_sites(&bytes, 0, 11).is_empty());
    }

    #[test]
    fn logical_immediates_decode_rotated_replicated_and_word_masks() {
        for (word, wide, expected) in [
            (0x927c6c80, true, 0xfffffff0),
            (0xb2089fe1, true, 0xff00ff00ff00ff00),
            (0x52010442, false, 0x80000001),
        ] {
            assert_eq!(logical_mask((word >> 10) & 0x1fff, wide), Some(expected));
        }
        assert!(logical_mask(0x103f, true).is_none());
        assert!(logical_mask(0x1000, false).is_none());
        assert!(logical_mask(0x2000, true).is_none());
        assert!(logical_mask(0x003f, true).is_none());
        let call = first_call(&words(&[0xb2089fe1, 0x94000000]), 0).unwrap();
        assert_eq!(call.argument1, Expression::Constant(0xff00ff00ff00ff00));
        assert_eq!(call.argument0, Expression::Input(0));
    }

    #[test]
    fn logical_masks_match_independent_bitwise_construction_exhaustively() {
        for wide in [false, true] {
            let width = if wide { 64 } else { 32 };
            let mut expected = [None; 8192];
            for element_bits in [2u32, 4, 8, 16, 32, 64] {
                if element_bits > width {
                    continue;
                }
                for ones in 1..element_bits {
                    for rotation in 0..64 {
                        let n = u32::from(element_bits == 64);
                        let imms = (((!(element_bits - 1)) << 1) | (ones - 1)) & 63;
                        let encoded = (n << 12) | (rotation << 6) | imms;
                        // Construct the result one bit at a time, without
                        // the production decoder's rotate/replicate operations.
                        let mut mask = 0;
                        for bit in 0..width {
                            if (bit + rotation) % element_bits < ones {
                                mask |= 1u64 << bit;
                            }
                        }
                        expected[encoded as usize] = Some(mask);
                    }
                }
            }
            for (encoded, expected) in expected.into_iter().enumerate() {
                assert_eq!(
                    logical_mask(encoded as u32, wide),
                    expected,
                    "encoded={encoded:#x} wide={wide}"
                );
            }
        }
    }

    #[test]
    fn shifted_register_arithmetic_retains_pointer_expressions() {
        let call = first_call(&words(&[0x8b050080, 0xcb050c81, 0x94000000]), 0).unwrap();
        assert_eq!(
            call.argument0,
            Expression::Sum(
                Box::new(Expression::Input(4)),
                Box::new(Expression::Input(5))
            )
        );
        assert_eq!(
            call.argument1,
            Expression::Difference(
                Box::new(Expression::Input(4)),
                Box::new(Expression::LeftShift(Box::new(Expression::Input(5)), 3))
            )
        );
        assert_eq!(call.argument2, Expression::Input(2));
        assert!(call.unmodeled.is_empty());
    }

    #[test]
    fn shifted_arithmetic_checks_width_and_distinguishes_sp_forms() {
        let mut registers = std::array::from_fn(|index| Expression::Input(index as u8));
        registers[1] = Expression::Constant(u64::from(u32::MAX));
        registers[2] = Expression::Constant(1);
        assert_eq!(
            shifted_arithmetic(0x0b020820, &registers),
            Some((0, Expression::Constant(3)))
        );
        assert!(shifted_arithmetic(0x8b450c80, &registers).is_none());
        assert!(shifted_arithmetic(0x8b2563e0, &registers).is_none());
        assert!(shifted_arithmetic(0x0b028020, &registers).is_none());
    }

    #[test]
    fn write_origin_retains_loads_but_resets_at_control_flow() {
        // MOV W8,#1; BL; LDR W8,[X23,#44]; STR W8,[X23,#40].
        let bytes = words(&[0x52800028, 0x94000000, 0xb9402ee8, 0xb9002ae8]);
        let mut write = field_writes(&bytes, 0, bytes.len(), 40, 32).remove(0);
        let origin = write_origin(&bytes, &write).unwrap();
        assert_eq!(origin.start, 8);
        assert_eq!(origin.base, Expression::Input(23));
        assert_eq!(
            origin.source,
            Expression::Load {
                base: Box::new(Expression::Input(23)),
                offset: 44,
                bits: 32
            }
        );
        assert!(origin.unmodeled.is_empty());
        write.source = 32;
        assert!(write_origin(&bytes, &write).is_none());
        write.source = 8;
        write.site = bytes.len() + 4;
        assert!(write_origin(&bytes, &write).is_none());
    }

    #[test]
    fn write_origin_never_carries_constants_through_branches_or_unknowns() {
        for barrier in [0x54000040, 0x14000001, 0xffffffff] {
            let bytes = words(&[0x52800028, barrier, 0xb9002ae8]);
            let write = field_writes(&bytes, 0, bytes.len(), 40, 32).remove(0);
            assert!(!matches!(
                write_origin(&bytes, &write).unwrap().source,
                Expression::Constant(_)
            ));
        }
        let bytes = words(&[0x52800028, 0xb9002ae8]);
        let write = field_writes(&bytes, 0, bytes.len(), 40, 32).remove(0);
        assert_eq!(
            write_origin(&bytes, &write).unwrap().source,
            Expression::Constant(1)
        );
    }

    #[test]
    fn offset_pair_loads_snapshot_the_base_before_aliasing_destinations() {
        // LDP X0,X1,[X0,#32]; BL. Both inputs must use the old X0.
        let call = first_call(&words(&[0xa9420400, 0x94000000]), 0).unwrap();
        assert_eq!(
            call.argument0,
            Expression::Load {
                base: Box::new(Expression::Input(0)),
                offset: 32,
                bits: 64
            }
        );
        assert_eq!(
            call.argument1,
            Expression::Load {
                base: Box::new(Expression::Input(0)),
                offset: 40,
                bits: 64
            }
        );
        assert!(call.unmodeled.is_empty());
    }

    #[test]
    fn immediate_subtraction_and_comparisons_preserve_other_registers() {
        let call = first_call(
            &words(&[
                0xf100049f, // CMP X4,#1: no general-purpose destination.
                0xd100a080, // SUB X0,X4,#40.
                0x52800001, // MOV W1,#0.
                0x51000421, // SUB W1,W1,#1: 32-bit wrap, then zero extension.
                0x94000000,
            ]),
            0,
        )
        .unwrap();
        assert_eq!(
            call.argument0,
            Expression::Subtract(Box::new(Expression::Input(4)), 40)
        );
        assert_eq!(call.argument1, Expression::Constant(u64::from(u32::MAX)));
        assert_eq!(call.argument2, Expression::Input(2));
        assert!(call.unmodeled.is_empty());
    }

    #[test]
    fn field_writes_include_wide_simd_and_partial_stores() {
        // STR X3,[X4,#32]; STP X5,X6,[X4,#32]; STR Q7,[X4,#32].
        let bytes = words(&[0xf9001083, 0xa9021885, 0x3d800887]);
        let writes = field_writes(&bytes, 0, bytes.len(), 40, 32);
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].source, 6);
        assert_eq!(writes[0].source_bits, 64);
        assert_eq!(writes[1].bank, RegisterBank::Simd);
        assert_eq!(writes[1].source_bit_offset, 64);
        assert!(writes.iter().all(|write| write.bits_written == 32));
        let partial = field_writes(&words(&[0x3900a883]), 0, 4, 40, 32);
        assert_eq!(partial[0].field_bit_offset, 16);
        assert_eq!(partial[0].bits_written, 8);
    }

    #[test]
    fn field_writes_reject_loads_writeback_and_invalid_ranges() {
        let bytes = words(&[0xf9401483, 0xa9a21885, 0x3dc00887]);
        assert!(field_writes(&bytes, 0, bytes.len(), 40, 32).is_empty());
        assert!(field_writes(&bytes, 1, bytes.len(), 40, 32).is_empty());
        assert!(field_writes(&bytes, 100, 4, 40, 32).is_empty());
        assert!(field_writes(&bytes, 0, 4, u64::MAX, 32).is_empty());
        assert!(field_writes(&bytes, 0, 4, 40, 7).is_empty());
    }

    #[test]
    fn unscaled_stores_keep_signed_byte_offsets_and_exclude_loads() {
        // Synthetic encodings independently checked with LLVM's assembler.
        let bytes = words(&[0xf8028083, 0x3c820087, 0xb81fc085, 0xf8428083]);
        let writes = field_writes(&bytes, 0, bytes.len(), 40, 32);
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].source, 3);
        assert_eq!(writes[1].source_bit_offset, 64);
        assert_eq!(writes[1].bank, RegisterBank::Simd);
        assert_eq!(store_operands(0xb81fc085)[0].1, -4);
        assert!(store_operands(0xf8428083).is_empty());
        assert!(store_operands(0xf8028c83).is_empty());
    }

    #[test]
    fn argument_aliases_retain_start_relative_provenance() {
        let call = first_call(&words(&[0xaa1303e1, 0x9100a3e0, 0x94000002]), 0).unwrap();
        assert_eq!(call.argument1, Expression::Input(19));
        assert_eq!(
            call.argument0,
            Expression::Offset(Box::new(Expression::Input(31)), 40)
        );
        assert_eq!(call.target, 16);
    }

    #[test]
    fn unknowns_and_branches_do_not_invent_provenance() {
        let call = first_call(&words(&[0xffffffff, 0x94000000]), 0).unwrap();
        assert_eq!(call.argument1, Expression::Unknown);
        assert!(first_call(&words(&[0x54000000, 0x94000000]), 0).is_none());
        assert!(first_call(&words(&[0x94000000]), 1).is_none());
    }

    #[test]
    fn word_moves_zero_extend_without_losing_other_argument_provenance() {
        let call =
            first_call(&words(&[0xaa1303e0, 0x2a1f03e1, 0x52800602, 0x94000000]), 0).unwrap();
        assert_eq!(call.argument0, Expression::Input(19));
        assert_eq!(call.argument1, Expression::Constant(0));
        assert_eq!(call.argument2, Expression::Constant(48));
        assert!(call.unmodeled.is_empty());
        let call = first_call(&words(&[0x2a1303e1, 0x94000000]), 0).unwrap();
        assert_eq!(
            call.argument1,
            Expression::Truncate32(Box::new(Expression::Input(19)))
        );
    }
}
