// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Conservative AArch64 decoder. Unsupported instructions stay explicit and execute through fallback.
use crate::{DecodedInstruction, DecodedOpcode, MoveWideKind};
pub fn decode_text(bytes: &[u8], guest_pc: u64) -> Vec<DecodedInstruction> {
    bytes
        .chunks_exact(4)
        .enumerate()
        .map(|(i, c)| {
            let raw = u32::from_le_bytes(c.try_into().unwrap());
            let pc = guest_pc + i as u64 * 4;
            DecodedInstruction {
                guest_pc: pc,
                raw,
                opcode: decode_one_at(raw, pc),
            }
        })
        .collect()
}
pub fn decode_one(raw: u32) -> DecodedOpcode {
    decode_one_at(raw, 0)
}

#[test]
fn prefetch_unsigned_is_not_an_integer_load_or_store() {
    assert!(matches!(decode_one(0xf9800261), DecodedOpcode::Unknown));
    assert!(matches!(
        decode_one(0xf9400260),
        DecodedOpcode::LoadStoreUnsigned { load: true, .. }
    ));
    assert!(matches!(
        decode_one(0xf9000260),
        DecodedOpcode::LoadStoreUnsigned { load: false, .. }
    ));
}
pub fn decode_one_at(raw: u32, pc: u64) -> DecodedOpcode {
    if raw == 0xd503201f {
        return DecodedOpcode::Nop;
    }
    if raw & 0xffff_fc1f == 0xd65f_0000 {
        return DecodedOpcode::Ret {
            rn: ((raw >> 5) & 31) as u8,
        };
    }
    if raw & 0xffff_fc1f == 0xd61f_0000 {
        return DecodedOpcode::BranchReg {
            rn: ((raw >> 5) & 31) as u8,
            link: false,
        };
    }
    if raw & 0xffff_fc1f == 0xd63f_0000 {
        return DecodedOpcode::BranchReg {
            rn: ((raw >> 5) & 31) as u8,
            link: true,
        };
    }
    if raw & 0xfc00_0000 == 0x9400_0000 {
        return DecodedOpcode::BranchImm {
            target: add(pc, branch26(raw)),
            link: true,
        };
    }
    if raw & 0x7c00_0000 == 0x1400_0000 {
        return DecodedOpcode::BranchImm {
            target: add(pc, branch26(raw)),
            link: false,
        };
    }
    if raw & 0xff00_0010 == 0x5400_0000 {
        return DecodedOpcode::CondBranch {
            target: add(pc, sx(((raw >> 5) & 0x7ffff) as u64, 19) << 2),
            cond: (raw & 15) as u8,
        };
    }
    if raw & 0x7e00_0000 == 0x3400_0000 {
        return DecodedOpcode::CompareBranch {
            target: add(pc, sx(((raw >> 5) & 0x7ffff) as u64, 19) << 2),
            rt: (raw & 31) as u8,
            nonzero: raw & (1 << 24) != 0,
            is64: raw & (1 << 31) != 0,
        };
    }
    if raw & 0x7e00_0000 == 0x3600_0000 {
        let b5 = ((raw >> 31) & 1) as u8;
        let b40 = ((raw >> 19) & 0x1f) as u8;
        return DecodedOpcode::TestBranch {
            target: add(pc, sx(((raw >> 5) & 0x3fff) as u64, 14) << 2),
            rt: (raw & 31) as u8,
            bit: (b5 << 5) | b40,
            nonzero: raw & (1 << 24) != 0,
        };
    }
    if raw & 0xffe0_001f == 0xd400_0001 {
        return DecodedOpcode::Svc {
            imm: ((raw >> 5) & 0xffff) as u16,
        };
    }
    if raw & 0x1f00_0000 == 0x1100_0000 {
        let sh = (raw >> 22) & 1;
        let imm = (((raw >> 10) & 0xfff) as u64) << (if sh != 0 { 12 } else { 0 });
        return DecodedOpcode::AddSubImm {
            rd: (raw & 31) as u8,
            rn: ((raw >> 5) & 31) as u8,
            imm,
            sub: raw & (1 << 30) != 0,
            set_flags: raw & (1 << 29) != 0,
            is64: raw & (1 << 31) != 0,
        };
    }
    if raw & 0x1f80_0000 == 0x1280_0000 {
        let opc = (raw >> 29) & 3;
        let kind = match opc {
            0 => MoveWideKind::MovN,
            2 => MoveWideKind::MovZ,
            3 => MoveWideKind::MovK,
            _ => return DecodedOpcode::Unknown,
        };
        let hw = (raw >> 21) & 3;
        let imm = (((raw >> 5) & 0xffff) as u64) << (hw * 16);
        return DecodedOpcode::MoveWide {
            rd: (raw & 31) as u8,
            imm,
            kind,
            is64: raw & (1 << 31) != 0,
        };
    }
    if raw & 0x1f80_0000 == 0x1200_0000 {
        return DecodedOpcode::LogicalImm {
            rd: (raw & 31) as u8,
            rn: ((raw >> 5) & 31) as u8,
            raw_imm: (raw >> 10) & 0x1fff,
            op: ((raw >> 29) & 3) as u8,
            is64: raw & (1 << 31) != 0,
        };
    }
    // Keep the SIMD/FP register bank out of the integer load/store IR.
    // PRFM occupies an integer addressing encoding but is a hint, not a load.
    if raw & 0xffc0_0000 == 0xf980_0000 {
        return DecodedOpcode::Unknown;
    }
    if raw & 0x3f00_0000 == 0x3900_0000 {
        let size = 8u16 << ((raw >> 30) & 3);
        let load = raw & (1 << 22) != 0;
        let scale = (raw >> 30) & 3;
        let off = (((raw >> 10) & 0xfff) as u64) << scale;
        return DecodedOpcode::LoadStoreUnsigned {
            rt: (raw & 31) as u8,
            rn: ((raw >> 5) & 31) as u8,
            offset: off,
            load,
            size_bits: size,
        };
    }
    if raw & 0x1f00_0000 == 0x1000_0000 {
        let page = raw & (1 << 31) != 0;
        let immlo = ((raw >> 29) & 3) as u64;
        let immhi = ((raw >> 5) & 0x7ffff) as u64;
        let imm = sx((immhi << 2) | immlo, 21);
        let base = if page {
            (pc & !0xfff) as i64
        } else {
            pc as i64
        };
        let d = if page { imm << 12 } else { imm };
        return DecodedOpcode::Adr {
            rd: (raw & 31) as u8,
            target: (base.wrapping_add(d)) as u64,
            page,
        };
    }
    DecodedOpcode::Unknown
}
fn branch26(raw: u32) -> i64 {
    sx((raw & 0x03ff_ffff) as u64, 26) << 2
}
fn sx(v: u64, b: u32) -> i64 {
    let s = 64 - b;
    ((v << s) as i64) >> s
}
fn add(pc: u64, d: i64) -> u64 {
    (pc as i64).wrapping_add(d) as u64
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vector_unsigned_transfers_do_not_decode_as_integer_transfers() {
        for raw in [0x3d80_0100, 0x7d40_1484, 0xbd40_0000, 0xfd00_0000] {
            assert!(matches!(decode_one(raw), DecodedOpcode::Unknown));
        }
        assert!(matches!(
            decode_one(0xf900_0100),
            DecodedOpcode::LoadStoreUnsigned { .. }
        ));
    }
    #[test]
    fn basics() {
        assert!(matches!(decode_one_at(0xd503201f, 0), DecodedOpcode::Nop));
        assert!(matches!(
            decode_one_at(0xd65f03c0, 0),
            DecodedOpcode::Ret { .. }
        ));
        assert!(matches!(
            decode_one_at(0x14000001, 0x1000),
            DecodedOpcode::BranchImm { target: 0x1004, .. }
        ));
    }
}
