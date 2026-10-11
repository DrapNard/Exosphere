// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Cranelift JIT backend for the semantic CPU IR.
//!
//! v0.1 intentionally JITs only operations whose semantics are explicit in Exosphere IR. If a
//! function contains an opaque ARM64 operation, syscall, guest-memory access, or unsupported
//! control flow, compilation returns `NeedsFallback` instead of risking a miscompile.

use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlags, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use exosphere_cpu_ir::{BlockId, Function, Operation, Terminator, ValueId};
use std::collections::HashMap;
use thiserror::Error;

/// Register file shared between JIT code and the runtime.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct CpuContext {
    /// AArch64 X0-X30 plus slot 31. Register-31 SP/ZR semantics are resolved by the lifter.
    pub regs: [u64; 32],
}

#[derive(Debug, Error)]
pub enum JitError {
    #[error("function requires baseline ARM64 fallback: {0}")]
    NeedsFallback(&'static str),
    #[error("Cranelift module error: {0}")]
    Module(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoweringStats {
    pub native_ops: usize,
    pub fallback_ops: usize,
}
pub fn validate_lowering(f: &Function) -> LoweringStats {
    let mut s = LoweringStats {
        native_ops: 0,
        fallback_ops: 0,
    };
    for b in &f.blocks {
        for o in &b.operations {
            if matches!(o, Operation::FallbackArm64 { .. }) {
                s.fallback_ops += 1
            } else {
                s.native_ops += 1
            }
        }
    }
    s
}

/// Owns the JIT module so finalized machine code remains alive.
pub struct JitProgram {
    module: JITModule,
    entry: *const u8,
}
impl JitProgram {
    /// Executes the compiled function against one register context.
    ///
    /// # Safety
    /// `entry` was emitted with the exact `extern "C" fn(*mut CpuContext)` signature and remains
    /// valid because the executable allocation is owned by `self.module`.
    pub unsafe fn call(&self, ctx: &mut CpuContext) {
        let f: unsafe extern "C" fn(*mut CpuContext) = unsafe { std::mem::transmute(self.entry) };
        unsafe { f(ctx as *mut CpuContext) };
    }
    pub fn module(&self) -> &JITModule {
        &self.module
    }
}

/// Compiles the currently native-safe subset of one semantic function.
pub fn compile(function: &Function) -> Result<JitProgram, JitError> {
    for b in &function.blocks {
        for o in &b.operations {
            match o {
                Operation::Constant { .. }
                | Operation::ReadReg { .. }
                | Operation::WriteReg { .. }
                | Operation::Add { .. }
                | Operation::Sub { .. } => {}
                Operation::Load { .. } | Operation::Store { .. } => {
                    return Err(JitError::NeedsFallback("guest memory helper not wired"));
                }
                Operation::Syscall { .. } => {
                    return Err(JitError::NeedsFallback("Horizon SVC helper not wired"));
                }
                Operation::FallbackArm64 { .. } => {
                    return Err(JitError::NeedsFallback("opaque ARM64 operation"));
                }
            }
        }
        match b.terminator {
            Terminator::Jump(_) | Terminator::Return | Terminator::Fallthrough(_) => {}
            _ => {
                return Err(JitError::NeedsFallback(
                    "control flow requires runtime helper",
                ));
            }
        }
    }

    let builder =
        JITBuilder::new(default_libcall_names()).map_err(|e| JitError::Module(e.to_string()))?;
    let mut module = JITModule::new(builder);
    let ptr_ty = module.target_config().pointer_type();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_ty));
    let id = module
        .declare_function("guest_fn", Linkage::Local, &sig)
        .map_err(|e| JitError::Module(e.to_string()))?;
    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    let mut fb_ctx = FunctionBuilderContext::new();
    {
        let mut bld = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
        let mut blocks: HashMap<BlockId, _> = HashMap::new();
        for b in &function.blocks {
            blocks.insert(b.id, bld.create_block());
        }
        let first = function
            .blocks
            .first()
            .ok_or(JitError::NeedsFallback("empty function"))?;
        let entry_block = blocks[&first.id];
        bld.append_block_params_for_function_params(entry_block);
        let ctx_ptr = bld.block_params(entry_block)[0];
        let mut values: HashMap<ValueId, _> = HashMap::new();
        for block in &function.blocks {
            let cb = blocks[&block.id];
            bld.switch_to_block(cb);
            for op in &block.operations {
                match *op {
                    Operation::Constant { out, value } => {
                        values.insert(out, bld.ins().iconst(types::I64, value as i64));
                    }
                    Operation::ReadReg { out, reg } => {
                        let v = bld.ins().load(
                            types::I64,
                            MemFlags::trusted(),
                            ctx_ptr,
                            i32::from(reg.0) * 8,
                        );
                        values.insert(out, v);
                    }
                    Operation::WriteReg { reg, value } => {
                        bld.ins().store(
                            MemFlags::trusted(),
                            values[&value],
                            ctx_ptr,
                            i32::from(reg.0) * 8,
                        );
                    }
                    Operation::Add { out, lhs, rhs } => {
                        values.insert(out, bld.ins().iadd(values[&lhs], values[&rhs]));
                    }
                    Operation::Sub { out, lhs, rhs } => {
                        values.insert(out, bld.ins().isub(values[&lhs], values[&rhs]));
                    }
                    _ => unreachable!("validated above"),
                }
            }
            match block.terminator {
                Terminator::Jump(target) => {
                    bld.ins().jump(blocks[&target], &[]);
                }
                Terminator::Fallthrough(Some(target)) => {
                    bld.ins().jump(blocks[&target], &[]);
                }
                Terminator::Fallthrough(None) | Terminator::Return => {
                    bld.ins().return_(&[]);
                }
                _ => unreachable!("validated above"),
            }
        }
        bld.seal_all_blocks();
        bld.finalize();
    }
    module
        .define_function(id, &mut ctx)
        .map_err(|e| JitError::Module(e.to_string()))?;
    module.clear_context(&mut ctx);
    module
        .finalize_definitions()
        .map_err(|e| JitError::Module(e.to_string()))?;
    let entry = module.get_finalized_function(id);
    Ok(JitProgram { module, entry })
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_cpu_ir::{
        BasicBlock, BlockId, Function, Operation, Register, Terminator, ValueId,
    };
    #[test]
    fn jits_simple_register_add() {
        let f = Function {
            guest_entry: 0x1000,
            blocks: vec![BasicBlock {
                id: BlockId(0),
                guest_start: 0x1000,
                operations: vec![
                    Operation::ReadReg {
                        out: ValueId(0),
                        reg: Register(0),
                    },
                    Operation::Constant {
                        out: ValueId(1),
                        value: 5,
                    },
                    Operation::Add {
                        out: ValueId(2),
                        lhs: ValueId(0),
                        rhs: ValueId(1),
                    },
                    Operation::WriteReg {
                        reg: Register(0),
                        value: ValueId(2),
                    },
                ],
                terminator: Terminator::Return,
            }],
        };
        let p = compile(&f).unwrap();
        let mut c = CpuContext::default();
        c.regs[0] = 7;
        unsafe {
            p.call(&mut c);
        }
        assert_eq!(c.regs[0], 12);
    }
}
