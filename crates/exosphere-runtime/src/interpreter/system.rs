// SPDX-License-Identifier: GPL-3.0-or-later
//! Audited user-accessible system registers and barriers for the ARMv8.0 guest profile.

use super::{read_register, write_register};
use exosphere_horizon::thread::ThreadContext;

// ARMv8.0 AArch64 control: AHP, DN, FZ and rounding mode. Legacy vector-length
// controls and exception enables read as zero in this non-trapping FP profile.
const FPCR_MASK: u32 = 0x07c0_0000;
const FPSR_MASK: u32 = 0x0800_009f;

/// The serial reference backend has no cache/timing model. Its prefetch policy
/// performs no data access, allocation or translation, and cannot fault on the
/// hinted address. Instruction fetch still has its ordinary permission checks.
pub(super) fn prefetch_hint(raw: u32) -> bool {
    if raw & 0xffc0_0000 == 0xf980_0000 // PRFM unsigned immediate
        || raw & 0xffe0_0c00 == 0xf880_0000 // PRFUM signed unscaled
        || raw & 0xff00_0000 == 0xd800_0000
    // PRFM PC-relative literal
    {
        return true;
    }
    // Register form admits UXTW, LSL, SXTW and SXTX, never sub-word indices.
    // The newer RPRFM encoding space is outside the baseline guest profile.
    raw & 0xffe0_0c00 == 0xf8a0_0800 && raw & 0x4000 != 0 && raw & 24 != 24
}

pub(super) fn execute(context: &mut ThreadContext, raw: u32) -> Result<bool, super::CpuError> {
    if serial_memory_barrier(raw) {
        return Ok(true);
    }
    let register = (raw & 31) as u8;
    if matches!(raw & 0xffff_ffe0, 0xd53b_e020 | 0xd53b_e000) {
        // Sample at instruction execution, not at slice start: timer-driven
        // loops must see advancing time even within one long CPU budget.
        return Err(super::CpuError::CounterRead {
            pc: context.pc,
            register,
            frequency: raw & 0xffff_ffe0 == 0xd53b_e000,
        });
    }
    match raw & 0xffff_ffe0 {
        0xd53b_d060 => write_register(context, register, context.tpidrro_el0, false, true),
        0xd53b_d040 => write_register(context, register, context.tpidr_el0, false, true),
        0xd51b_d040 => context.tpidr_el0 = read_register(context, register, false),
        0xd51b_4400 => context.fpcr = read_register(context, register, false) as u32 & FPCR_MASK,
        0xd51b_4420 => context.fpsr = read_register(context, register, false) as u32 & FPSR_MASK,
        0xd53b_4400 => write_register(context, register, u64::from(context.fpcr), false, true),
        0xd53b_4420 => write_register(context, register, u64::from(context.fpsr), false, true),
        _ => return Ok(false),
    }
    Ok(true)
}

fn serial_memory_barrier(raw: u32) -> bool {
    let option = (raw >> 8) & 15;
    match raw & 0xffff_f0ff {
        0xd503_30bf | 0xd503_309f => {
            // DMB/DSB: OSH, NSH, ISH and SY, each with LD/ST/all access forms.
            // All guest accesses complete synchronously through one exclusively
            // borrowed AddressSpace. There are no pending stores or device jobs
            // to drain here. A parallel/device backend needs its own barrier path;
            // a host atomic fence alone would not wait for GPU or MMIO completion.
            option & 3 != 0
        }
        // ISB SY: step fetches the next instruction from memory only after this
        // instruction completes; no prefetched instructions or decoded cache exist.
        0xd503_30df => option == 15,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn barriers_preserve_registers_flags_and_exclusive_reservation() {
        let mut context = ThreadContext {
            x: [0x1234; 31],
            v: [u128::MAX; 32],
            sp: 0x8000,
            pc: 0x1000,
            pstate: 0xa000_0000,
            fpcr: FPCR_MASK,
            fpsr: FPSR_MASK,
            exclusive: Some(exosphere_horizon::thread::ExclusiveReservation {
                address: 0x2000,
                size: 8,
                epoch: 17,
            }),
            ..Default::default()
        };
        let snapshot = |context: &ThreadContext| {
            (
                context.x,
                context.v,
                context.sp,
                context.pc,
                context.pstate,
                context.fpcr,
                context.fpsr,
                context.tpidr_el0,
                context.tpidrro_el0,
                context.exclusive,
            )
        };
        let before = snapshot(&context);
        for base in [0xd503_309f, 0xd503_30bf] {
            for option in 0..16 {
                assert_eq!(
                    execute(&mut context, base | (option << 8)).unwrap(),
                    option & 3 != 0
                );
                assert_eq!(snapshot(&context), before);
            }
        }
        assert!(execute(&mut context, 0xd503_3fdf).unwrap());
        assert_eq!(snapshot(&context), before);
        // Reserved ISB options and adjacent system instructions remain unaudited.
        for raw in [0xd503_3bdf, 0xd503_3bff, 0xd503_3b9e] {
            assert!(!execute(&mut context, raw).unwrap());
        }
    }

    #[test]
    fn thread_registers_are_separate_and_readonly_tls_cannot_be_written() {
        let mut context = ThreadContext {
            tpidrro_el0: 0x7000000200,
            ..Default::default()
        };
        context.x[1] = 0x12345678;
        execute(&mut context, 0xd51bd041).unwrap();
        execute(&mut context, 0xd53bd042).unwrap();
        execute(&mut context, 0xd53bd063).unwrap();
        assert_eq!(context.x[2], 0x12345678);
        assert_eq!(context.x[3], 0x7000000200);
        assert!(!execute(&mut context, 0xd51bd061).unwrap());
        assert_eq!(context.tpidrro_el0, 0x7000000200);
    }

    #[test]
    fn floating_point_registers_mask_reserved_bits_and_are_independent() {
        let mut context = ThreadContext::default();
        context.x[1] = u64::MAX;
        context.pstate = 0xa0000000;
        execute(&mut context, 0xd51b4401).unwrap();
        execute(&mut context, 0xd51b4421).unwrap();
        execute(&mut context, 0xd53b4402).unwrap();
        execute(&mut context, 0xd53b4423).unwrap();
        assert_eq!(context.x[2], u64::from(FPCR_MASK));
        assert_eq!(context.x[3], u64::from(FPSR_MASK));
        execute(&mut context, 0xd51b441f).unwrap(); // MSR FPCR,XZR
        assert_eq!(context.fpcr, 0);
        assert_eq!(context.fpsr, FPSR_MASK);
        assert_eq!(context.pstate, 0xa0000000);
        assert!(!execute(&mut context, 0xd5181001).unwrap()); // SCTLR_EL1 is not exposed.
    }
}
