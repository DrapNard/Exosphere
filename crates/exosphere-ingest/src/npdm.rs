// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounds-checked process metadata. Signature and capability enforcement are separate stages.

use crate::{IngestError, reader::Reader};

#[derive(Debug, Clone)]
pub struct ProcessMetadata {
    pub name: String,
    pub program_id: u64,
    pub address_space_type: u8,
    pub main_thread_priority: u8,
    pub main_thread_core: u8,
    pub main_thread_stack_size: u32,
    /// ACI0 scheduling grants, bounded by the ACID descriptor when present.
    pub scheduling: Option<SchedulingCapabilities>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulingCapabilities {
    pub core_mask: u64,
    pub priority_mask: u64,
}

fn parse_scheduling(
    reader: &Reader<'_>,
    offset_field: usize,
    header_size: usize,
) -> Result<Option<SchedulingCapabilities>, IngestError> {
    let offset = reader.u32(offset_field)? as usize;
    let size = reader.u32(offset_field + 4)? as usize;
    if size == 0 {
        return Ok(None);
    }
    if offset < header_size || offset % 4 != 0 || size % 4 != 0 {
        return Err(IngestError::Invalid);
    }
    let bytes = reader.slice(offset, size)?;
    let mut result = None;
    for word in bytes.chunks_exact(4) {
        let word = u32::from_le_bytes(word.try_into().unwrap());
        if word & 15 != 7 {
            continue;
        }
        if result.is_some() {
            return Err(IngestError::Invalid);
        }
        let lowest_priority = (word >> 4) & 63;
        let highest_priority = (word >> 10) & 63;
        let minimum_core = (word >> 16) & 255;
        let maximum_core = word >> 24;
        // This loader targets the original Switch's four guest cores, not
        // host CPU indices. Priorities 0..3 belong to kernel threads.
        if minimum_core > maximum_core
            || maximum_core > 3
            || highest_priority < 4
            || highest_priority > lowest_priority
        {
            return Err(IngestError::Invalid);
        }
        let range_mask = |first: u32, last: u32| (u64::MAX >> (63 - last)) & (u64::MAX << first);
        result = Some(SchedulingCapabilities {
            core_mask: range_mask(minimum_core, maximum_core),
            priority_mask: range_mask(highest_priority, lowest_priority),
        });
    }
    Ok(result)
}

pub fn parse(bytes: &[u8]) -> Result<ProcessMetadata, IngestError> {
    let reader = Reader::new(bytes);
    reader.slice(0, 0x80)?;
    if reader.slice(0, 4)? != b"META" {
        return Err(IngestError::Invalid);
    }
    let flags = reader.u8(0xc)?;
    if flags & 1 == 0 {
        return Err(IngestError::Unsupported);
    }
    let priority = reader.u8(0xe)?;
    let stack_size = reader.u32(0x1c)?;
    if priority > 63 || stack_size % 0x1000 != 0 {
        return Err(IngestError::Invalid);
    }
    let name_bytes = reader.slice(0x20, 16)?;
    let name_end = name_bytes.iter().position(|byte| *byte == 0).unwrap_or(16);
    let name = std::str::from_utf8(&name_bytes[..name_end])
        .map_err(|_| IngestError::Invalid)?
        .to_owned();
    let aci = Reader::new(reader.slice(reader.u32(0x70)? as usize, reader.u32(0x74)? as usize)?);
    let acid = Reader::new(reader.slice(reader.u32(0x78)? as usize, reader.u32(0x7c)? as usize)?);
    aci.slice(0, 0x40)?;
    acid.slice(0, 0x240)?;
    if aci.slice(0, 4)? != b"ACI0" || acid.slice(0x200, 4)? != b"ACID" {
        return Err(IngestError::Invalid);
    }
    let program_id = aci.u64(0x10)?;
    if program_id < acid.u64(0x210)? || program_id > acid.u64(0x218)? {
        return Err(IngestError::Integrity(
            "NPDM program ID is outside ACID range",
        ));
    }
    let scheduling = parse_scheduling(&aci, 0x30, 0x40)?;
    let ceiling = parse_scheduling(&acid, 0x230, 0x240)?;
    if let Some(grants) = scheduling {
        let ceiling = ceiling.ok_or(IngestError::Integrity(
            "ACI0 scheduling grant lacks ACID ceiling",
        ))?;
        if grants.core_mask & !ceiling.core_mask != 0
            || grants.priority_mask & !ceiling.priority_mask != 0
        {
            return Err(IngestError::Integrity(
                "ACI0 scheduling grant exceeds ACID ceiling",
            ));
        }
        let core = reader.u8(0xf)?;
        if core >= 64
            || grants.core_mask & (1u64 << core) == 0
            || grants.priority_mask & (1u64 << priority) == 0
        {
            return Err(IngestError::Integrity(
                "main thread exceeds scheduling grant",
            ));
        }
    }
    Ok(ProcessMetadata {
        name,
        program_id,
        address_space_type: (flags >> 1) & 7,
        main_thread_priority: priority,
        main_thread_core: reader.u8(0xf)?,
        main_thread_stack_size: stack_size,
        scheduling,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x300];
        bytes[..4].copy_from_slice(b"META");
        bytes[0xc] = 7; // AArch64, 39-bit address space.
        bytes[0xe] = 44;
        bytes[0x1c..0x20].copy_from_slice(&0x4000u32.to_le_bytes());
        bytes[0x20..0x23].copy_from_slice(b"App");
        for (offset, value) in [(0x70, 0x80u32), (0x74, 0x40), (0x78, 0xc0), (0x7c, 0x240)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[0x80..0x84].copy_from_slice(b"ACI0");
        bytes[0x90..0x98].copy_from_slice(&42u64.to_le_bytes());
        bytes[0x2c0..0x2c4].copy_from_slice(b"ACID");
        bytes[0x2d0..0x2d8].copy_from_slice(&40u64.to_le_bytes());
        bytes[0x2d8..0x2e0].copy_from_slice(&50u64.to_le_bytes());
        bytes
    }

    #[test]
    fn scheduling_grants_are_bounded_by_acid_and_main_thread() {
        let mut bytes = metadata();
        bytes.resize(0x318, 0);
        bytes.copy_within(0xc0..0x300, 0xd0);
        for (offset, value) in [
            (0x74, 0x44u32),
            (0x78, 0xd0),
            (0x7c, 0x244),
            (0xb0, 0x40),
            (0xb4, 4),
            (0x300, 0x240),
            (0x304, 4),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let descriptor = |low: u32, high: u32, first: u32, last: u32| {
            7 | (low << 4) | (high << 10) | (first << 16) | (last << 24)
        };
        let grant = descriptor(63, 24, 0, 2);
        bytes[0xc0..0xc4].copy_from_slice(&grant.to_le_bytes());
        bytes[0x310..0x314].copy_from_slice(&descriptor(63, 4, 0, 3).to_le_bytes());
        let parsed = parse(&bytes).unwrap().scheduling.unwrap();
        assert_eq!(parsed.core_mask, 7);
        assert_eq!(parsed.priority_mask, u64::MAX << 24);
        for invalid in [
            descriptor(24, 63, 0, 2),
            descriptor(63, 0, 0, 2),
            descriptor(63, 24, 2, 0),
            descriptor(63, 24, 0, 4),
        ] {
            let mut malformed = bytes.clone();
            malformed[0xc0..0xc4].copy_from_slice(&invalid.to_le_bytes());
            assert!(parse(&malformed).is_err());
        }
        for (offset, value) in [(0xf, 3), (0xe, 23)] {
            let mut malformed = bytes.clone();
            malformed[offset] = value;
            assert!(matches!(parse(&malformed), Err(IngestError::Integrity(_))));
        }
        let mut narrowed = bytes.clone();
        narrowed[0x310..0x314].copy_from_slice(&descriptor(63, 24, 0, 1).to_le_bytes());
        assert!(matches!(parse(&narrowed), Err(IngestError::Integrity(_))));
        // Missing ceiling, unaligned/truncated region, and duplicate grants fail.
        for (offset, value) in [(0x304, 0u32), (0xb0, 0x41), (0xb4, 3), (0xb4, 8)] {
            let mut malformed = bytes.clone();
            malformed[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(parse(&malformed).is_err());
        }
        bytes[0x74..0x78].copy_from_slice(&0x48u32.to_le_bytes());
        bytes[0xb4..0xb8].copy_from_slice(&8u32.to_le_bytes());
        bytes[0xc4..0xc8].copy_from_slice(&grant.to_le_bytes());
        assert!(parse(&bytes).is_err());
    }

    #[test]
    fn reads_process_parameters() {
        let parsed = parse(&metadata()).unwrap();
        assert_eq!(parsed.name, "App");
        assert_eq!(parsed.program_id, 42);
        assert_eq!(parsed.main_thread_stack_size, 0x4000);
        assert_eq!(parsed.address_space_type, 3);
    }

    #[test]
    fn rejects_truncated_and_mismatched_program_metadata() {
        let mut bytes = metadata();
        assert!(parse(&bytes[..0x2ff]).is_err());
        bytes[0x90..0x98].copy_from_slice(&51u64.to_le_bytes());
        assert!(matches!(parse(&bytes), Err(IngestError::Integrity(_))));
        bytes[0xe] = 64;
        assert!(parse(&bytes).is_err());
    }
}
