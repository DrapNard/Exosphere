// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Maxwell MME decoder/interpreter.
//!
//! The raw decoder implements the documented Fermi/Maxwell PGRAPH macro encoding used by Tegra.
//! Complex destination side effects are represented explicitly so method emission is not guessed.

use crate::mme::MmeProgram;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmeAluOp {
    Add,
    AddImmediate,
    AddCarry,
    Sub,
    SubBorrow,
    Xor,
    And,
    AndNot,
    Nand,
    Or,
    ExtractInsert,
    ExtractShiftLeftRegister,
    ExtractShiftLeftImmediate,
    ShiftLeft,
    ShiftRightLogical,
    Move,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmeResultAction {
    Ignore,
    Move,
    MoveAndSetMethod,
    FetchAndSend,
    Send,
    FetchAndSetMethod,
    FetchSendAndSetMethod,
    SetMethodAndSend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmeInstruction {
    Alu {
        op: MmeAluOp,
        dst: u8,
        src_a: u8,
        src_b: u8,
        immediate: i32,
        bit_src_pos: u8,
        bit_size: u8,
        bit_dst_pos: u8,
        action: MmeResultAction,
        exit: bool,
    },
    LoadParameter {
        dst: u8,
        exit: bool,
    },
    ReadMethod {
        dst: u8,
        address_reg: Option<u8>,
        immediate: i32,
        exit: bool,
    },
    SetMethodAddress {
        src: u8,
        exit: bool,
    },
    Send {
        src: u8,
        exit: bool,
    },
    Branch {
        condition_reg: Option<u8>,
        non_zero: bool,
        relative_instructions: i32,
        annul: bool,
        exit: bool,
    },
    Exit,
    Nop,
    Opaque(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmeMethodEmission {
    pub method: u16,
    pub argument: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MmeStopReason {
    Exit,
    EndOfProgram,
    OpaqueInstruction { pc: usize, raw: u32 },
    StepLimit,
    InvalidBranch,
    ParameterUnderflow { pc: usize, parameter: usize },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmeExecutionResult {
    pub registers: [u32; 32],
    pub emissions: Vec<MmeMethodEmission>,
    pub parameters_consumed: usize,
    pub steps: usize,
    pub stop_reason: MmeStopReason,
}

fn bits(v: u32, shift: u32, width: u32) -> u32 {
    (v >> shift) & ((1u32 << width) - 1)
}
fn sign_extend_18(v: u32) -> i32 {
    ((v << 14) as i32) >> 14
}
fn reg(raw: u32, shift: u32) -> u8 {
    bits(raw, shift, 3) as u8
}
fn action(raw: u32) -> MmeResultAction {
    match bits(raw, 4, 3) {
        0 => MmeResultAction::Ignore,
        1 => MmeResultAction::Move,
        2 => MmeResultAction::MoveAndSetMethod,
        3 => MmeResultAction::FetchAndSend,
        4 => MmeResultAction::Send,
        5 => MmeResultAction::FetchAndSetMethod,
        6 => MmeResultAction::FetchSendAndSetMethod,
        _ => MmeResultAction::SetMethodAndSend,
    }
}

/// Decode the documented PGRAPH macro ISA subset. Unknown encodings stay opaque.
pub fn decode_word(raw: u32) -> MmeInstruction {
    if raw == 0x0000_0011 {
        return MmeInstruction::Nop;
    }
    if raw == 0x0000_0091 {
        return MmeInstruction::Exit;
    }
    let exit = (raw & (1 << 7)) != 0;
    let opcode = raw & 7;
    let r1 = reg(raw, 8);
    let r2 = reg(raw, 11);
    let r3 = reg(raw, 14);
    if opcode == 7 {
        let kind = raw & 0x17;
        let annul = (raw & (1 << 5)) != 0;
        let rel = sign_extend_18(bits(raw, 14, 18));
        return match kind {
            0x07 => MmeInstruction::Branch {
                condition_reg: None,
                non_zero: true,
                relative_instructions: rel,
                annul,
                exit,
            },
            0x17 => MmeInstruction::Branch {
                condition_reg: Some(r2),
                non_zero: true,
                relative_instructions: rel,
                annul,
                exit,
            },
            _ => MmeInstruction::Branch {
                condition_reg: Some(r2),
                non_zero: false,
                relative_instructions: rel,
                annul,
                exit,
            },
        };
    }
    if opcode == 1 {
        if raw & 0xffff_f87f == 1 {
            return MmeInstruction::LoadParameter { dst: r1, exit };
        }
        let imm = sign_extend_18(bits(raw, 14, 18));
        return MmeInstruction::Alu {
            op: MmeAluOp::AddImmediate,
            dst: r1,
            src_a: r2,
            src_b: 0,
            immediate: imm,
            bit_src_pos: 0,
            bit_size: 0,
            bit_dst_pos: 0,
            action: action(raw),
            exit,
        };
    }
    if opcode == 2 {
        return MmeInstruction::Alu {
            op: MmeAluOp::ExtractInsert,
            dst: r1,
            src_a: r2,
            src_b: r3,
            immediate: 0,
            bit_src_pos: bits(raw, 17, 5) as u8,
            bit_size: bits(raw, 22, 5) as u8,
            bit_dst_pos: bits(raw, 27, 5) as u8,
            action: action(raw),
            exit,
        };
    }
    if opcode == 3 {
        return MmeInstruction::Alu {
            op: MmeAluOp::ExtractShiftLeftRegister,
            dst: r1,
            src_a: r3,
            src_b: r2,
            immediate: 0,
            bit_src_pos: 0,
            bit_size: bits(raw, 22, 5) as u8,
            bit_dst_pos: bits(raw, 27, 5) as u8,
            action: action(raw),
            exit,
        };
    }
    if opcode == 4 {
        return MmeInstruction::Alu {
            op: MmeAluOp::ExtractShiftLeftImmediate,
            dst: r1,
            src_a: r3,
            src_b: r2,
            immediate: 0,
            bit_src_pos: bits(raw, 17, 5) as u8,
            bit_size: bits(raw, 22, 5) as u8,
            bit_dst_pos: 0,
            action: action(raw),
            exit,
        };
    }
    if opcode == 5 {
        let imm = sign_extend_18(bits(raw, 14, 18));
        return MmeInstruction::ReadMethod {
            dst: r1,
            address_reg: Some(r2),
            immediate: imm,
            exit,
        };
    }
    if opcode == 0 {
        let alu = match raw & 0x003e_0007 {
            0x0000_0000 => MmeAluOp::Add,
            0x0002_0000 => MmeAluOp::AddCarry,
            0x0004_0000 => MmeAluOp::Sub,
            0x0006_0000 => MmeAluOp::SubBorrow,
            0x0010_0000 => MmeAluOp::Xor,
            0x0012_0000 => MmeAluOp::Or,
            0x0014_0000 => MmeAluOp::And,
            0x0016_0000 => MmeAluOp::AndNot,
            0x0018_0000 => MmeAluOp::Nand,
            _ => return MmeInstruction::Opaque(raw),
        };
        return MmeInstruction::Alu {
            op: alu,
            dst: r1,
            src_a: r2,
            src_b: r3,
            immediate: 0,
            bit_src_pos: 0,
            bit_size: 0,
            bit_dst_pos: 0,
            action: action(raw),
            exit,
        };
    }
    MmeInstruction::Opaque(raw)
}

pub fn decode_program(program: &MmeProgram) -> Vec<MmeInstruction> {
    program.words.iter().copied().map(decode_word).collect()
}
pub fn execute(program: &MmeProgram, parameters: &[u32], step_limit: usize) -> MmeExecutionResult {
    execute_with_method_reader(program, parameters, step_limit, |_| 0)
}
pub fn execute_with_method_reader<F: FnMut(u16) -> u32>(
    program: &MmeProgram,
    parameters: &[u32],
    step_limit: usize,
    reader: F,
) -> MmeExecutionResult {
    execute_decoded_with_method_reader(&decode_program(program), parameters, step_limit, reader)
}
pub fn execute_decoded(
    ins: &[MmeInstruction],
    parameters: &[u32],
    step_limit: usize,
) -> MmeExecutionResult {
    execute_decoded_with_method_reader(ins, parameters, step_limit, |_| 0)
}

pub fn execute_decoded_with_method_reader<F: FnMut(u16) -> u32>(
    ins: &[MmeInstruction],
    parameters: &[u32],
    step_limit: usize,
    mut method_reader: F,
) -> MmeExecutionResult {
    #[derive(Clone, Copy)]
    enum PendingControl {
        Jump(usize),
        Exit,
    }
    let mut registers = [0u32; 32];
    let mut emissions: Vec<MmeMethodEmission> = Vec::new();
    let mut pi = 0usize;
    let mut pc = 0usize;
    let mut steps = 0usize;
    let mut method_addr = 0u32;
    let mut method_inc = 0u32;
    let mut carry = false;
    let mut pending: Option<PendingControl> = None;
    let stop_reason = loop {
        if steps >= step_limit {
            break MmeStopReason::StepLimit;
        }
        let Some(i) = ins.get(pc).copied() else {
            break MmeStopReason::EndOfProgram;
        };
        let fetches_parameter = match i {
            MmeInstruction::LoadParameter { .. } => true,
            MmeInstruction::Alu { action, .. } => matches!(
                action,
                MmeResultAction::FetchAndSend
                    | MmeResultAction::FetchAndSetMethod
                    | MmeResultAction::FetchSendAndSetMethod
            ),
            _ => false,
        };
        // Missing words indicate an incomplete invocation, not an implicit zero.
        // Stop before changing registers, method state or emitting commands.
        if fetches_parameter && pi >= parameters.len() {
            break MmeStopReason::ParameterUnderflow { pc, parameter: pi };
        }
        steps += 1;
        let apply_after_this = pending.take();
        if apply_after_this.is_some() && matches!(i, MmeInstruction::Branch { .. }) {
            break MmeStopReason::InvalidBranch;
        }
        let mut should_exit = false;
        let apply_result = |value: u32,
                            dst: u8,
                            act: MmeResultAction,
                            regs: &mut [u32; 32],
                            em: &mut Vec<MmeMethodEmission>,
                            pi: &mut usize,
                            ma: &mut u32,
                            mi: &mut u32| {
            let set_method = |v: u32, ma: &mut u32, mi: &mut u32| {
                *ma = (v & 0xfff) << 2;
                *mi = ((v >> 12) & 0x3f) << 2;
            };
            let send = |v: u32, em: &mut Vec<MmeMethodEmission>, ma: &mut u32, mi: u32| {
                em.push(MmeMethodEmission {
                    method: (*ma >> 2) as u16,
                    argument: v,
                });
                *ma = ma.wrapping_add(mi);
            };
            match act {
                MmeResultAction::Ignore => {}
                MmeResultAction::Move => regs[dst as usize] = value,
                MmeResultAction::MoveAndSetMethod => {
                    set_method(value, ma, mi);
                    regs[dst as usize] = value
                }
                MmeResultAction::FetchAndSend => {
                    send(value, em, ma, *mi);
                    regs[dst as usize] = parameters[*pi];
                    *pi += 1
                }
                MmeResultAction::Send => {
                    send(value, em, ma, *mi);
                    regs[dst as usize] = value
                }
                MmeResultAction::FetchAndSetMethod => {
                    set_method(value, ma, mi);
                    regs[dst as usize] = parameters[*pi];
                    *pi += 1
                }
                MmeResultAction::FetchSendAndSetMethod => {
                    set_method(value, ma, mi);
                    let p = parameters[*pi];
                    *pi += 1;
                    send(p, em, ma, *mi);
                    regs[dst as usize] = value
                }
                MmeResultAction::SetMethodAndSend => {
                    set_method(value, ma, mi);
                    send((value >> 12) & 0x3f, em, ma, *mi);
                    regs[dst as usize] = value
                }
            }
        };
        match i {
            MmeInstruction::Exit => {
                pc += 1;
                should_exit = true
            }
            MmeInstruction::Nop => pc += 1,
            MmeInstruction::Opaque(raw) => break MmeStopReason::OpaqueInstruction { pc, raw },
            MmeInstruction::LoadParameter { dst, exit } => {
                registers[dst as usize] = parameters[pi];
                pi += 1;
                pc += 1;
                should_exit = exit
            }
            MmeInstruction::ReadMethod {
                dst,
                address_reg,
                immediate,
                exit,
            } => {
                let base = address_reg.map(|r| registers[r as usize]).unwrap_or(0);
                let addr = base.wrapping_add_signed(immediate);
                // Emissions and the front-end register mirror use dword method
                // indices. Byte-address scaling is internal to the send cursor.
                let method = (addr & 0xfff) as u16;
                registers[dst as usize] = emissions
                    .iter()
                    .rev()
                    .find(|e| e.method == method)
                    .map(|e| e.argument)
                    .unwrap_or_else(|| method_reader(method));
                pc += 1;
                should_exit = exit
            }
            MmeInstruction::SetMethodAddress { src, exit } => {
                let v = registers[src as usize];
                method_addr = (v & 0xfff) << 2;
                method_inc = ((v >> 12) & 0x3f) << 2;
                pc += 1;
                should_exit = exit
            }
            MmeInstruction::Send { src, exit } => {
                emissions.push(MmeMethodEmission {
                    method: (method_addr >> 2) as u16,
                    argument: registers[src as usize],
                });
                method_addr = method_addr.wrapping_add(method_inc);
                pc += 1;
                should_exit = exit
            }
            MmeInstruction::Branch {
                condition_reg,
                non_zero,
                relative_instructions,
                annul,
                exit,
            } => {
                let take = condition_reg
                    .map(|r| (registers[r as usize] != 0) == non_zero)
                    .unwrap_or(true);
                if take {
                    let n = (pc as i64) + 1 + i64::from(relative_instructions);
                    if n < 0 || n as usize >= ins.len() {
                        break MmeStopReason::InvalidBranch;
                    }
                    if annul {
                        pc = n as usize
                    } else {
                        pending = Some(PendingControl::Jump(n as usize));
                        pc += 1
                    }
                } else {
                    pc += 1;
                    if exit {
                        pending = Some(PendingControl::Exit)
                    }
                }
            }
            MmeInstruction::Alu {
                op,
                dst,
                src_a,
                src_b,
                immediate,
                bit_src_pos,
                bit_size,
                bit_dst_pos,
                action,
                exit,
            } => {
                let a = registers[src_a as usize];
                let b = registers[src_b as usize];
                let (v, new_carry) = match op {
                    MmeAluOp::Add => a.overflowing_add(b),
                    MmeAluOp::AddImmediate => (a.wrapping_add_signed(immediate), false),
                    MmeAluOp::AddCarry => {
                        let (x, c1) = a.overflowing_add(b);
                        let (y, c2) = x.overflowing_add(carry as u32);
                        (y, c1 || c2)
                    }
                    MmeAluOp::Sub => {
                        let (x, c) = a.overflowing_sub(b);
                        (x, !c)
                    }
                    MmeAluOp::SubBorrow => {
                        let (x, c1) = a.overflowing_sub(b);
                        let (y, c2) = x.overflowing_sub((!carry) as u32);
                        (y, !(c1 || c2))
                    }
                    MmeAluOp::Xor => (a ^ b, false),
                    MmeAluOp::And => (a & b, false),
                    MmeAluOp::AndNot => (a & !b, false),
                    MmeAluOp::Nand => (!(a & b), false),
                    MmeAluOp::Or => (a | b, false),
                    MmeAluOp::ShiftLeft => (a.wrapping_shl(b & 31), false),
                    MmeAluOp::ShiftRightLogical => (a.wrapping_shr(b & 31), false),
                    MmeAluOp::ExtractInsert => {
                        let size = bit_size.min(32) as u32;
                        let mask = if size == 32 {
                            u32::MAX
                        } else if size == 0 {
                            0
                        } else {
                            (1u32 << size) - 1
                        };
                        let extracted = (b >> bit_src_pos) & mask;
                        let dmask = mask.checked_shl(bit_dst_pos as u32).unwrap_or(0);
                        (
                            (a & !dmask) | (extracted.checked_shl(bit_dst_pos as u32).unwrap_or(0)),
                            false,
                        )
                    }
                    MmeAluOp::ExtractShiftLeftRegister => {
                        let size = bit_size.min(32) as u32;
                        let mask = if size == 32 {
                            u32::MAX
                        } else if size == 0 {
                            0
                        } else {
                            (1u32 << size) - 1
                        };
                        (
                            ((a >> (b & 31)) & mask)
                                .checked_shl(bit_dst_pos as u32)
                                .unwrap_or(0),
                            false,
                        )
                    }
                    MmeAluOp::ExtractShiftLeftImmediate => {
                        let size = bit_size.min(32) as u32;
                        let mask = if size == 32 {
                            u32::MAX
                        } else if size == 0 {
                            0
                        } else {
                            (1u32 << size) - 1
                        };
                        (((a >> bit_src_pos) & mask).wrapping_shl(b & 31), false)
                    }
                    MmeAluOp::Move => (a, false),
                };
                carry = new_carry;
                apply_result(
                    v,
                    dst,
                    action,
                    &mut registers,
                    &mut emissions,
                    &mut pi,
                    &mut method_addr,
                    &mut method_inc,
                );
                pc += 1;
                should_exit = exit
            }
        }
        registers[0] = 0;
        if should_exit {
            pending = Some(PendingControl::Exit);
        }
        if let Some(control) = apply_after_this {
            match control {
                PendingControl::Jump(target) => pc = target,
                PendingControl::Exit => break MmeStopReason::Exit,
            }
        }
    };
    MmeExecutionResult {
        registers,
        emissions,
        parameters_consumed: pi,
        steps,
        stop_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_parameters_stop_before_register_or_emission_changes() {
        let mut instructions = vec![MmeInstruction::LoadParameter {
            dst: 1,
            exit: false,
        }];
        for action in [
            MmeResultAction::FetchAndSend,
            MmeResultAction::FetchAndSetMethod,
            MmeResultAction::FetchSendAndSetMethod,
        ] {
            instructions.push(MmeInstruction::Alu {
                op: MmeAluOp::Add,
                dst: 2,
                src_a: 0,
                src_b: 0,
                immediate: 0,
                bit_src_pos: 0,
                bit_size: 0,
                bit_dst_pos: 0,
                action,
                exit: false,
            });
        }
        for instruction in instructions {
            let result = execute_decoded(&[instruction], &[], 16);
            assert_eq!(
                result.stop_reason,
                MmeStopReason::ParameterUnderflow {
                    pc: 0,
                    parameter: 0
                }
            );
            assert_eq!(result.registers, [0; 32]);
            assert_eq!(result.parameters_consumed, 0);
            assert_eq!(result.steps, 0);
            assert!(result.emissions.is_empty());
            // An explicitly provided zero is valid and must not be confused
            // with an absent parameter.
            assert_ne!(
                execute_decoded(&[instruction], &[0], 16).stop_reason,
                MmeStopReason::ParameterUnderflow {
                    pc: 0,
                    parameter: 0
                }
            );
        }
        let result = execute_decoded(
            &[
                MmeInstruction::LoadParameter {
                    dst: 1,
                    exit: false,
                },
                MmeInstruction::LoadParameter {
                    dst: 1,
                    exit: false,
                },
            ],
            &[0x1234],
            16,
        );
        assert_eq!(
            result.stop_reason,
            MmeStopReason::ParameterUnderflow {
                pc: 1,
                parameter: 1
            }
        );
        assert_eq!(result.registers[1], 0x1234);
        assert_eq!(result.parameters_consumed, 1);
    }
    #[test]
    fn decodes_documented_nop_exit() {
        assert_eq!(decode_word(0x11), MmeInstruction::Nop);
        assert_eq!(decode_word(0x91), MmeInstruction::Exit);
    }
    #[test]
    fn decodes_add() {
        assert!(matches!(
            decode_word(0),
            MmeInstruction::Alu {
                op: MmeAluOp::Add,
                ..
            }
        ));
    }
    #[test]
    fn decodes_extract_insert() {
        assert!(matches!(
            decode_word(2),
            MmeInstruction::Alu {
                op: MmeAluOp::ExtractInsert,
                ..
            }
        ));
    }
    #[test]
    fn normal_branch_executes_delay_slot() {
        let ins = [
            MmeInstruction::Branch {
                condition_reg: None,
                non_zero: true,
                relative_instructions: 1,
                annul: false,
                exit: false,
            },
            MmeInstruction::LoadParameter {
                dst: 1,
                exit: false,
            },
            MmeInstruction::Nop,
        ];
        let r = execute_decoded(&ins, &[0x1234], 16);
        assert_eq!(r.registers[1], 0x1234);
    }
    #[test]
    fn annul_taken_branch_skips_delay_slot() {
        let ins = [
            MmeInstruction::Branch {
                condition_reg: None,
                non_zero: true,
                relative_instructions: 1,
                annul: true,
                exit: false,
            },
            MmeInstruction::LoadParameter {
                dst: 1,
                exit: false,
            },
            MmeInstruction::Nop,
        ];
        let r = execute_decoded(&ins, &[0x1234], 16);
        assert_eq!(r.registers[1], 0);
    }
    #[test]
    fn method_read_observes_prior_emission() {
        let ins = [
            MmeInstruction::LoadParameter {
                dst: 1,
                exit: false,
            },
            MmeInstruction::SetMethodAddress {
                src: 1,
                exit: false,
            },
            MmeInstruction::LoadParameter {
                dst: 2,
                exit: false,
            },
            MmeInstruction::Send {
                src: 2,
                exit: false,
            },
            MmeInstruction::LoadParameter {
                dst: 2,
                exit: false,
            },
            MmeInstruction::Send {
                src: 2,
                exit: false,
            },
            MmeInstruction::SetMethodAddress {
                src: 1,
                exit: false,
            },
            MmeInstruction::Send {
                src: 2,
                exit: false,
            },
            MmeInstruction::ReadMethod {
                dst: 3,
                address_reg: None,
                immediate: 0x35e,
                exit: false,
            },
            MmeInstruction::ReadMethod {
                dst: 4,
                address_reg: None,
                immediate: 0x35f,
                exit: false,
            },
            MmeInstruction::ReadMethod {
                dst: 5,
                address_reg: None,
                immediate: 0x35d,
                exit: false,
            },
            MmeInstruction::ReadMethod {
                dst: 6,
                address_reg: Some(1),
                immediate: -1,
                exit: false,
            },
            MmeInstruction::Exit,
            MmeInstruction::Nop,
        ];
        let mut reads = Vec::new();
        let r = execute_decoded_with_method_reader(&ins, &[0x135e, 7, 9], 32, |method| {
            reads.push(method);
            0xdeadbeef
        });
        assert_eq!(r.stop_reason, MmeStopReason::Exit);
        assert_eq!(r.parameters_consumed, 3);
        assert_eq!(r.registers[3], 9);
        assert_eq!(r.registers[4], 9);
        assert_eq!(r.registers[5], 0xdeadbeef);
        assert_eq!(r.registers[6], 0xdeadbeef);
        assert_eq!(reads, [0x35d, 0x35d]);
        assert_eq!(
            r.emissions,
            vec![
                MmeMethodEmission {
                    method: 0x35e,
                    argument: 7
                },
                MmeMethodEmission {
                    method: 0x35f,
                    argument: 9
                },
                MmeMethodEmission {
                    method: 0x35e,
                    argument: 9
                },
            ]
        );
    }
}
