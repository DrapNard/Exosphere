// SPDX-License-Identifier: GPL-3.0-or-later
//! Prepares verified ExeFS modules for an AArch64 Horizon process.

use exosphere_horizon::{
    error::HorizonError,
    memory::{AddressSpace, MemoryKind, MemoryPermission, PAGE_SIZE},
};
use exosphere_ingest::{
    IngestError,
    nca_partition::VerifiedPartition,
    npdm::{self, ProcessMetadata},
    nso::{self, LoadedNso},
};

#[derive(Debug, thiserror::Error)]
pub enum ProgramError {
    #[error(transparent)]
    Ingest(#[from] IngestError),
    #[error(transparent)]
    Memory(#[from] HorizonError),
    #[error("invalid program layout: {0}")]
    Layout(&'static str),
}

#[derive(Debug)]
pub struct MappedModule {
    pub name: String,
    pub base: u64,
    pub end: u64,
    pub symbols: Vec<nso::FunctionSymbol>,
    pub symbol_error: Option<String>,
}

pub struct PreparedProgram {
    pub metadata: ProcessMetadata,
    pub memory: AddressSpace,
    pub modules: Vec<MappedModule>,
    pub entry: u64,
    pub stack_top: u64,
    pub layout: exosphere_horizon::layout::ProcessLayout,
}

fn align_page(size: u64) -> Result<u64, ProgramError> {
    size.checked_add(PAGE_SIZE - 1)
        .map(|value| value & !(PAGE_SIZE - 1))
        .ok_or(ProgramError::Layout("page alignment overflow"))
}

fn file_bytes<'a>(partition: &'a VerifiedPartition, name: &str) -> Result<&'a [u8], ProgramError> {
    let mut files = partition
        .index
        .entries
        .iter()
        .filter(|file| file.name == name);
    let file = files
        .next()
        .ok_or(ProgramError::Layout("required ExeFS file missing"))?;
    if files.next().is_some() {
        return Err(ProgramError::Layout("duplicate ExeFS file"));
    }
    let start =
        usize::try_from(file.offset).map_err(|_| ProgramError::Layout("file offset overflow"))?;
    let end = file
        .offset
        .checked_add(file.size)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(ProgramError::Layout("file range overflow"))?;
    partition
        .bytes
        .get(start..end)
        .ok_or(ProgramError::Layout("file outside partition"))
}

fn map_module(
    memory: &mut AddressSpace,
    base: u64,
    module: &LoadedNso,
) -> Result<u64, ProgramError> {
    let header = &module.header;
    let segments = [
        (header.text, &module.text, MemoryPermission::READ_EXECUTE, 0),
        (header.rodata, &module.rodata, MemoryPermission::READ, 0),
        (
            header.data,
            &module.data,
            MemoryPermission::READ_WRITE,
            header.bss_size,
        ),
    ];
    let mut module_end = base;
    for (segment, bytes, permission, bss_size) in segments {
        let address = base
            .checked_add(segment.memory_offset as u64)
            .ok_or(ProgramError::Layout("segment address overflow"))?;
        let size = align_page(bytes.len() as u64 + bss_size as u64)?;
        if size == 0 {
            continue;
        }
        // BSS follows initialized data and remains zero in the fresh mapping.
        memory.map(address, size, permission, MemoryKind::Code)?;
        memory.load_bytes(address, bytes)?;
        module_end = module_end.max(
            address
                .checked_add(size)
                .ok_or(ProgramError::Layout("segment end overflow"))?,
        );
    }
    Ok(module_end)
}

/// Builds a fresh address space before committing anything to the kernel. The caller
/// supplies the layout policy; this diagnostic path currently supports 39-bit processes.
pub fn prepare(
    partition: &VerifiedPartition,
    program_id: u64,
    code_base: u64,
    stack_top: u64,
) -> Result<PreparedProgram, ProgramError> {
    let metadata = npdm::parse(file_bytes(partition, "main.npdm")?)?;
    if metadata.program_id != program_id {
        return Err(ProgramError::Layout("NCA/NPDM program ID mismatch"));
    }
    if metadata.address_space_type != 3 {
        return Err(ProgramError::Layout(
            "process requires another address-space policy",
        ));
    }
    if code_base % PAGE_SIZE != 0
        || stack_top % PAGE_SIZE != 0
        || code_base == 0
        || code_base >= (1u64 << 39)
        || stack_top >= (1u64 << 39)
    {
        return Err(ProgramError::Layout("unaligned or out-of-range layout"));
    }
    let mut memory = AddressSpace::default();
    // Explicit diagnostic application quota. This is configurable allocator policy,
    // not the 6-GiB virtual heap reservation or a firmware-derived physical pool.
    memory.set_memory_limit(3 * 1024 * 1024 * 1024)?;
    let mut modules = Vec::new();
    let mut base = code_base;
    // rtld is entered first and handles guest dynamic linking. The loader only maps
    // modules here; it must not claim that relocations have already been applied.
    let names = [
        "rtld", "main", "subsdk0", "subsdk1", "subsdk2", "subsdk3", "subsdk4", "subsdk5",
        "subsdk6", "subsdk7", "subsdk8", "subsdk9", "sdk",
    ];
    for name in names {
        if !partition.index.entries.iter().any(|file| file.name == name) {
            if name == "rtld" || name == "main" {
                return Err(ProgramError::Layout("rtld/main missing"));
            }
            continue;
        }
        let module = nso::load(file_bytes(partition, name)?)?;
        if module.header.text.memory_offset != 0 {
            return Err(ProgramError::Layout("NSO text must start at module base"));
        }
        let end = map_module(&mut memory, base, &module)?;
        if end >= (1u64 << 39) {
            return Err(ProgramError::Layout("code exceeds address space"));
        }
        // Optional diagnostic tables must not prevent an otherwise valid image
        // from running. Retain the parse error so callers can report lost symbols.
        let (symbols, symbol_error) = match nso::function_symbols(&module) {
            Ok(symbols) => (symbols, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        modules.push(MappedModule {
            name: name.into(),
            base,
            end,
            symbols,
            symbol_error,
        });
        base = end;
    }
    let stack_size = metadata.main_thread_stack_size as u64;
    let layout = exosphere_horizon::layout::ProcessLayout::diagnostic_39_bit(
        code_base,
        base - code_base,
        stack_top,
        stack_size,
    )?;
    let initial_sp = if stack_size == 0 {
        0
    } else {
        let stack_base = stack_top
            .checked_sub(stack_size)
            .ok_or(ProgramError::Layout("stack underflow"))?;
        memory.map(
            stack_base,
            stack_size,
            MemoryPermission::READ_WRITE,
            MemoryKind::Stack,
        )?;
        stack_top
    };
    Ok(PreparedProgram {
        metadata,
        memory,
        modules,
        entry: code_base,
        stack_top: initial_sp,
        layout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module() -> LoadedNso {
        let mut bytes = vec![0u8; 0x140];
        bytes[..4].copy_from_slice(b"NSO0");
        let fields = [
            (0x10, 0x100u32),
            (0x14, 0),
            (0x18, 32),
            (0x60, 32),
            (0x20, 0x120),
            (0x24, 0x1000),
            (0x28, 16),
            (0x64, 16),
            (0x30, 0x130),
            (0x34, 0x2000),
            (0x38, 16),
            (0x68, 16),
            (0x3c, 0x1000),
        ];
        for (offset, value) in fields {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[0x100..0x120].fill(0xaa);
        bytes[0x120..0x130].fill(0xbb);
        bytes[0x130..0x140].fill(0xcc);
        nso::load(&bytes).unwrap()
    }

    #[test]
    fn maps_permissions_data_and_zeroed_bss() {
        let mut memory = AddressSpace::default();
        assert_eq!(
            map_module(&mut memory, 0x10000, &module()).unwrap(),
            0x14000
        );
        let mut byte = [0u8; 1];
        memory.read(0x10000, &mut byte).unwrap();
        assert_eq!(byte, [0xaa]);
        assert!(memory.write(0x10000, &[1]).is_err());
        assert!(memory.write(0x11000, &[1]).is_err());
        memory.read(0x12000, &mut byte).unwrap();
        assert_eq!(byte, [0xcc]);
        let mut bss = vec![1u8; 0x1000];
        memory.read(0x12010, &mut bss).unwrap();
        assert!(bss.iter().all(|byte| *byte == 0));
        memory.write(0x12000, &[1]).unwrap();
    }

    #[test]
    fn rejects_overlapping_segments() {
        let mut invalid = module();
        invalid.header.rodata.memory_offset = 0;
        assert!(map_module(&mut AddressSpace::default(), 0x10000, &invalid).is_err());
    }
}
