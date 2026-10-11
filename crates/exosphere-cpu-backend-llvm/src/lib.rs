// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! LLVM backend boundary. v0.1 emits portable textual LLVM IR without linking libLLVM.
//! This keeps the compiler bootstrap testable with clang/LLVM tools while the future in-process
//! backend can replace the emitter behind the same API.
use exosphere_cpu_ir::{Function, Operation, Terminator};
pub fn emit_llvm_ir(f: &Function) -> String {
    let mut s = String::new();
    s.push_str("; Exosphere generated LLVM IR\ntarget triple = \"x86_64-unknown-linux-gnu\"\ndeclare i64 @exo_read_reg(i32)\ndeclare void @exo_write_reg(i32,i64)\ndeclare i64 @exo_load(i64,i32)\ndeclare void @exo_store(i64,i64,i32)\ndeclare void @exo_fallback_arm64(i64,i32)\ndeclare void @exo_svc(i32)\n\ndefine void @guest_fn() {\n");
    for b in &f.blocks {
        s.push_str(&format!("b{}:\n", b.id.0));
        for op in &b.operations {
            match op {
                Operation::Constant { out, value } => {
                    s.push_str(&format!("  %v{} = add i64 0, {}\n", out.0, value))
                }
                Operation::ReadReg { out, reg } => s.push_str(&format!(
                    "  %v{} = call i64 @exo_read_reg(i32 {})\n",
                    out.0, reg.0
                )),
                Operation::WriteReg { reg, value } => s.push_str(&format!(
                    "  call void @exo_write_reg(i32 {}, i64 %v{})\n",
                    reg.0, value.0
                )),
                Operation::Add { out, lhs, rhs } => s.push_str(&format!(
                    "  %v{} = add i64 %v{}, %v{}\n",
                    out.0, lhs.0, rhs.0
                )),
                Operation::Sub { out, lhs, rhs } => s.push_str(&format!(
                    "  %v{} = sub i64 %v{}, %v{}\n",
                    out.0, lhs.0, rhs.0
                )),
                Operation::Load { out, address, bits } => s.push_str(&format!(
                    "  %v{} = call i64 @exo_load(i64 %v{}, i32 {})\n",
                    out.0, address.0, bits
                )),
                Operation::Store {
                    address,
                    value,
                    bits,
                } => s.push_str(&format!(
                    "  call void @exo_store(i64 %v{}, i64 %v{}, i32 {})\n",
                    address.0, value.0, bits
                )),
                Operation::Syscall { imm } => {
                    s.push_str(&format!("  call void @exo_svc(i32 {})\n", imm))
                }
                Operation::FallbackArm64 { guest_pc, raw } => s.push_str(&format!(
                    "  call void @exo_fallback_arm64(i64 {}, i32 {})\n",
                    guest_pc, raw
                )),
            }
        }
        match &b.terminator {
            Terminator::Jump(x) => s.push_str(&format!("  br label %b{}\n", x.0)),
            Terminator::Return => s.push_str("  ret void\n"),
            Terminator::Fallthrough(Some(x)) => s.push_str(&format!("  br label %b{}\n", x.0)),
            _ => s.push_str("  ret void ; complex control flow handled by runtime baseline path\n"),
        }
    }
    s.push_str("}\n");
    s
}
