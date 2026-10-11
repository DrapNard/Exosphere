// SPDX-License-Identifier: GPL-3.0-or-later
//! Content diagnostics backed by verified ingest operations.

use std::{fs, path::PathBuf, sync::Arc};

pub fn inspect(
    path: PathBuf,
    keys: PathBuf,
    cpu_steps: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_ingest::{nca, nca_crypto::UserKeyProvider};
    use std::io::{Read, Seek, SeekFrom};
    let mut provider = UserKeyProvider::from_key_file(keys)?;
    let mut input = fs::File::open(&path)?;
    let index = exosphere_ingest::pfs0::read_index(&mut input)?;
    for entry in index
        .entries
        .iter()
        .filter(|entry| entry.name.ends_with(".tik"))
    {
        if entry.size > 1024 * 1024 {
            return Err(exosphere_ingest::IngestError::Invalid.into());
        }
        input.seek(SeekFrom::Start(entry.offset))?;
        let mut ticket = vec![0u8; entry.size as usize];
        input.read_exact(&mut ticket)?;
        provider.import_ticket(&ticket)?;
    }
    let provider = Arc::new(provider);
    for entry in index
        .entries
        .iter()
        .filter(|entry| entry.name.ends_with(".nca"))
    {
        if entry.size < nca::HEADER_AND_FS_HEADERS as u64 {
            return Err(exosphere_ingest::IngestError::Invalid.into());
        }
        input.seek(SeekFrom::Start(entry.offset))?;
        let mut encrypted = [0u8; nca::HEADER_AND_FS_HEADERS];
        input.read_exact(&mut encrypted)?;
        let header = nca::parse_with_provider(&encrypted, provider.as_ref())?;
        if header.content_size != entry.size {
            return Err(
                exosphere_ingest::IngestError::Integrity("NCA/container size mismatch").into(),
            );
        }
        println!(
            "{}: program={:016x} content_type={} generation={} size={}",
            entry.name,
            header.program_id,
            header.content_type,
            header.key_generation.max(header.key_generation_old),
            header.content_size
        );
        for (i, section) in header.sections.iter().enumerate() {
            if let Some(section) = section {
                let end = section
                    .byte_offset()
                    .checked_add(section.byte_size())
                    .ok_or(exosphere_ingest::IngestError::Invalid)?;
                if end > entry.size {
                    return Err(exosphere_ingest::IngestError::Invalid.into());
                }
                println!(
                    "  section {i}: offset={} size={} fs={:?} encryption={:?} hash_type={} title_rights={}",
                    section.byte_offset(),
                    section.byte_size(),
                    header.fs_headers[i].fs_type,
                    header.fs_headers[i].encryption,
                    header.fs_headers[i].hash_type,
                    header.rights_id != [0u8; 16]
                );
                if header.fs_headers[i].fs_type == nca::FsType::PartitionFs {
                    let partition = exosphere_ingest::nca_partition::read_partition(
                        &mut input,
                        entry.offset,
                        &header,
                        i,
                        &provider,
                        64 * 1024 * 1024,
                    )?;
                    if header.content_type == 0
                        && partition
                            .index
                            .entries
                            .iter()
                            .any(|file| file.name == "main.npdm")
                    {
                        let program = exosphere_runtime::program::prepare(
                            &partition,
                            header.program_id,
                            0x0071_0000_0000,
                            0x007f_0000_0000,
                        )?;
                        println!(
                            "    process image: {} entry=0x{:x} stack=0x{:x} priority={}",
                            program.metadata.name,
                            program.entry,
                            program.stack_top,
                            program.metadata.main_thread_priority
                        );
                        for module in &program.modules {
                            println!(
                                "      mapped {}: 0x{:x}..0x{:x}",
                                module.name, module.base, module.end
                            );
                            if let Some(error) = &module.symbol_error {
                                println!("      symbol diagnostics unavailable: {error}");
                            }
                        }
                        let mut first_instruction = [0u8; 4];
                        program.memory.read(program.entry, &mut first_instruction)?;
                        let raw = u32::from_le_bytes(first_instruction);
                        println!(
                            "      entry instruction: {raw:08x} {:?}",
                            exosphere_cpu_ir::decode::decode_one_at(raw, program.entry)
                        );
                        if cpu_steps != 0 {
                            let data_section = header
                                .sections
                                .iter()
                                .enumerate()
                                .filter(|(index, section)| {
                                    section.is_some()
                                        && header.fs_headers[*index].fs_type == nca::FsType::RomFs
                                })
                                .map(|(index, _)| index)
                                .collect::<Vec<_>>();
                            if data_section.len() != 1 {
                                return Err(exosphere_ingest::IngestError::Unsupported.into());
                            }
                            // Open a separate descriptor: diagnostic reads must not move
                            // the inspector's cursor or load the complete data image.
                            let storage = exosphere_ingest::nca_rom_storage::NcaRomStorage::new(
                                fs::File::open(&path)?,
                                entry.offset,
                                header.clone(),
                                data_section[0],
                                Arc::clone(&provider),
                            )?;
                            probe_cpu(program, cpu_steps, Arc::new(storage))?;
                        }
                    }
                    for file in partition.index.entries {
                        println!("    verified file: {} ({} bytes)", file.name, file.size);
                        let start = usize::try_from(file.offset)?;
                        let end = usize::try_from(
                            file.offset
                                .checked_add(file.size)
                                .ok_or(exosphere_ingest::IngestError::Invalid)?,
                        )?;
                        let bytes = &partition.bytes[start..end];
                        if bytes.starts_with(b"NSO0") {
                            let module = exosphere_ingest::nso::load(bytes)?;
                            println!(
                                "      NSO loaded: text={} rodata={} data={} bss={}",
                                module.text.len(),
                                module.rodata.len(),
                                module.data.len(),
                                module.header.bss_size
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn probe_cpu(
    program: exosphere_runtime::program::PreparedProgram,
    budget: usize,
    storage: Arc<dyn exosphere_vfs::VirtualStorage>,
) -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_horizon::{HorizonKernel, HorizonProfileId};
    let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
    kernel.install_gpu_ioctl_backend(Box::new(
        exosphere_platform_linux::create_native_gpu_backend()?,
    ))?;
    kernel.fs.mount_storage("rom-data", storage);
    let process_id = kernel.create_process(program.metadata.program_id, program.metadata.name);
    kernel.process_mut(process_id)?.memory = program.memory;
    kernel.process_mut(process_id)?.layout = Some(program.layout);
    kernel.process_mut(process_id)?.ideal_core = Some(program.metadata.main_thread_core);
    kernel.process_mut(process_id)?.scheduling = program.metadata.scheduling.map(|grants| {
        exosphere_horizon::process::ProcessSchedulingCapabilities {
            core_mask: grants.core_mask,
            priority_mask: grants.priority_mask,
        }
    });
    let (thread_id, handle) = kernel.create_thread(
        process_id,
        program.entry,
        program.stack_top,
        program.metadata.main_thread_priority as i32,
    )?;
    let mut context = kernel.thread(thread_id)?.context.clone();
    // Native process entry ABI: x0 is the argument pointer (none), x1 the main-thread handle.
    context.x[1] = handle.0 as u64;
    kernel.thread_mut(thread_id)?.context = context;
    kernel.start_thread(thread_id)?;
    let execution = exosphere_runtime::execution::run_process(&mut kernel, process_id, budget)?;
    let (completed, stop) = (execution.completed, execution.stop);
    let thread = kernel.thread(execution.last_thread.unwrap_or(thread_id))?;
    if execution.idle {
        println!("      CPU probe suspended: no ready thread");
    }
    println!("      CPU probe thread: {}", thread.id);
    let context = &thread.context;
    let terminate_result = kernel.process(process_id)?.application.terminate_result;
    if terminate_result != 0 {
        println!("      Application terminate result: 0x{terminate_result:08x}");
    }
    // Kernel-dispatched SVC failures arrive as memory/kernel errors, not as
    // SupervisorCall. Decode the stopped instruction to retain their context.
    let stopped_svc = stop.as_ref().and_then(|_| {
        kernel
            .process(process_id)
            .ok()?
            .memory
            .fetch_instruction(context.pc)
            .ok()
            .and_then(
                |raw| match exosphere_cpu_ir::decode::decode_one_at(raw, context.pc) {
                    exosphere_cpu_ir::DecodedOpcode::Svc { imm } => Some(imm),
                    _ => None,
                },
            )
    });
    if let Some(number) = stopped_svc {
        println!(
            "      SVC arguments: number=0x{number:x} x0=0x{:x} x1=0x{:x} x2=0x{:x} x3=0x{:x}",
            context.x[0], context.x[1], context.x[2], context.x[3]
        );
    }
    if stop.is_some() {
        for (service, command, result) in &kernel.ipc_diagnostics {
            println!("      Guest recent IPC: {service} command={command} result=0x{result:08x}");
        }
    }
    if matches!(
        stop,
        Some(exosphere_runtime::interpreter::CpuError::SupervisorCall { number: 0x26, .. })
    ) {
        let memory = &kernel.process(process_id)?.memory;
        let returns = guest_frame_returns(memory, context.x[29], context.x[30]);
        for (index, address) in returns.iter().enumerate() {
            let location = program
                .modules
                .iter()
                .find(|module| *address >= module.base && *address < module.end)
                .map(|module| {
                    let offset = address - module.base;
                    // The instruction before a return address belongs to the caller.
                    let caller = offset.saturating_sub(4);
                    let symbol = module.symbols.iter().find(|symbol| {
                        caller >= symbol.value && caller < symbol.value.saturating_add(symbol.size)
                    });
                    let mut location = format!("{}+0x{offset:x}", module.name);
                    if let Some(symbol) = symbol {
                        location.push_str(&format!(
                            " {}+0x{:x}",
                            symbol.name,
                            caller - symbol.value
                        ));
                    }
                    location
                })
                .unwrap_or_else(|| "unknown module".into());
            println!("      Guest abort frame {index}: 0x{address:x} ({location})");
            // Bounded caller code helps identify the failed check without
            // exposing stack data or reading non-executable memory.
            if index >= 3 {
                let symbol_start = program.modules.iter().find_map(|module| {
                    let caller = address.checked_sub(module.base)?.checked_sub(4)?;
                    module
                        .symbols
                        .iter()
                        .find(|symbol| {
                            caller >= symbol.value
                                && caller < symbol.value.saturating_add(symbol.size)
                        })
                        .and_then(|symbol| module.base.checked_add(symbol.value))
                });
                // Include the first caller's earlier checks, not just the final
                // abort call. Keep the expansion bounded even for huge symbols.
                let start = if index == 3 || index == 4 {
                    symbol_start
                        .unwrap_or(address.saturating_sub(96))
                        .max(address.saturating_sub(1024))
                } else {
                    address.saturating_sub(96)
                };
                for pc in (start..*address).step_by(4) {
                    if let Ok(raw) = memory.fetch_instruction(pc) {
                        println!(
                            "      Guest abort code: 0x{pc:x} {raw:08x} {:?}",
                            exosphere_cpu_ir::decode::decode_one_at(raw, pc)
                        );
                    }
                }
                if index == 3 {
                    // Inspect a bounded set of direct callees of the immediate
                    // aborting function, without reading guest stack/payload data.
                    let mut targets = std::collections::BTreeSet::new();
                    for pc in (start..*address).step_by(4) {
                        let Ok(raw) = memory.fetch_instruction(pc) else {
                            continue;
                        };
                        if let exosphere_cpu_ir::DecodedOpcode::BranchImm { target, link: true } =
                            exosphere_cpu_ir::decode::decode_one_at(raw, pc)
                        {
                            if targets.len() < 16 {
                                targets.insert(target);
                            }
                        }
                    }
                    let mut nested_targets = std::collections::BTreeSet::new();
                    for target in targets {
                        let resolved = guest_plt_target(memory, target);
                        if let Some(destination) = resolved {
                            println!("      Guest abort import: 0x{target:x} -> 0x{destination:x}");
                        }
                        let target = resolved.unwrap_or(target);
                        for pc in (target..target.saturating_add(128)).step_by(4) {
                            if let Ok(raw) = memory.fetch_instruction(pc) {
                                if let exosphere_cpu_ir::DecodedOpcode::BranchImm {
                                    target,
                                    link: true,
                                } = exosphere_cpu_ir::decode::decode_one_at(raw, pc)
                                {
                                    if nested_targets.len() < 16 {
                                        nested_targets.insert(target);
                                    }
                                }
                                println!(
                                    "      Guest abort callee: 0x{pc:x} {raw:08x} {:?}",
                                    exosphere_cpu_ir::decode::decode_one_at(raw, pc)
                                );
                            }
                        }
                    }
                    // One additional level reveals validators hidden behind a
                    // conversion wrapper. Keep both depth and output bounded.
                    for target in nested_targets {
                        let destination = guest_plt_target(memory, target).unwrap_or(target);
                        println!(
                            "      Guest abort nested import: 0x{target:x} -> 0x{destination:x}"
                        );
                        for pc in (destination..destination.saturating_add(512)).step_by(4) {
                            if let Ok(raw) = memory.fetch_instruction(pc) {
                                println!(
                                    "      Guest abort nested code: 0x{pc:x} {raw:08x} {:?}",
                                    exosphere_cpu_ir::decode::decode_one_at(raw, pc)
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "      CPU probe: completed={completed} pc=0x{:x} stop={}",
        context.pc,
        stop.map(|error| error.to_string()).unwrap_or_else(|| {
            if completed == budget {
                "instruction budget reached".into()
            } else {
                format!("thread yielded: {:?}", thread.state)
            }
        })
    );
    if completed == budget {
        println!(
            "      CPU budget state: sp=0x{:x} nzcv=0x{:x}",
            context.sp,
            context.pstate >> 28 & 15
        );
        for (index, registers) in context.x.chunks(4).enumerate() {
            let line = registers
                .iter()
                .enumerate()
                .map(|(offset, value)| format!("x{}=0x{value:x}", index * 4 + offset))
                .collect::<Vec<_>>()
                .join(" ");
            println!("        {line}");
        }
        let memory = &kernel.process(process_id)?.memory;
        for address in (context.pc.saturating_sub(32)..context.pc.saturating_add(32)).step_by(4) {
            if let Ok(raw) = memory.fetch_instruction(address) {
                println!(
                    "        0x{address:x}: {raw:08x} {:?}",
                    exosphere_cpu_ir::decode::decode_one_at(raw, address)
                );
            }
        }
    }
    Ok(())
}

/// Resolve only the standard AArch64 import stub, never arbitrary guest code.
/// The loaded address is printed only if it resolves to executable memory.
fn guest_plt_target(memory: &exosphere_horizon::memory::AddressSpace, stub: u64) -> Option<u64> {
    use exosphere_cpu_ir::DecodedOpcode;
    let DecodedOpcode::Adr {
        rd: 16,
        target: page,
        page: true,
    } = exosphere_cpu_ir::decode::decode_one_at(memory.fetch_instruction(stub).ok()?, stub)
    else {
        return None;
    };
    let DecodedOpcode::LoadStoreUnsigned {
        rt: 17,
        rn: 16,
        offset,
        load: true,
        size_bits: 64,
    } = exosphere_cpu_ir::decode::decode_one_at(
        memory.fetch_instruction(stub.checked_add(4)?).ok()?,
        stub.checked_add(4)?,
    )
    else {
        return None;
    };
    let DecodedOpcode::AddSubImm {
        rd: 16,
        rn: 16,
        imm,
        sub: false,
        set_flags: false,
        is64: true,
    } = exosphere_cpu_ir::decode::decode_one_at(
        memory.fetch_instruction(stub.checked_add(8)?).ok()?,
        stub.checked_add(8)?,
    )
    else {
        return None;
    };
    if imm != offset || memory.fetch_instruction(stub.checked_add(12)?).ok()? != 0xd61f0220 {
        return None;
    }
    let mut bytes = [0; 8];
    memory.read(page.checked_add(offset)?, &mut bytes).ok()?;
    let target = u64::from_le_bytes(bytes);
    memory.fetch_instruction(target).ok()?;
    Some(target)
}

/// Best-effort frame-pointer diagnostics, not a complete unwinder. Only code
/// return addresses are emitted; arbitrary guest stack payloads are never dumped.
fn guest_frame_returns(
    memory: &exosphere_horizon::memory::AddressSpace,
    mut frame: u64,
    link: u64,
) -> Vec<u64> {
    let is_code_return = |address: u64| {
        address
            .checked_sub(4)
            .is_some_and(|pc| memory.fetch_instruction(pc).is_ok())
    };
    let mut returns = Vec::new();
    if is_code_return(link) {
        returns.push(link);
    }
    for _ in 0..16 {
        if frame == 0 || frame & 15 != 0 {
            break;
        }
        let mut record = [0u8; 16];
        if memory.read(frame, &mut record).is_err() {
            break;
        }
        let next = u64::from_le_bytes(record[..8].try_into().unwrap());
        let address = u64::from_le_bytes(record[8..].try_into().unwrap());
        if !is_code_return(address) {
            break;
        }
        if returns.last() != Some(&address) {
            returns.push(address);
        }
        // Frames grow upwards on the descending stack; reject cycles/backlinks.
        if next <= frame {
            break;
        }
        frame = next;
    }
    returns
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_horizon::memory::{AddressSpace, MemoryKind, MemoryPermission, PAGE_SIZE};

    #[test]
    fn import_stub_resolution_requires_exact_sequence_and_executable_destination() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_EXECUTE,
                MemoryKind::Code,
            )
            .unwrap();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Stack,
            )
            .unwrap();
        // ADRP X16, 0x8000; LDR X17, [X16,#632]; ADD X16,X16,#632; BR X17.
        let code: Vec<u8> = [0xf0000030u32, 0xf9413e11, 0x9109e210, 0xd61f0220]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        memory.load_bytes(0x1000, &code).unwrap();
        memory.write(0x8278, &0x1100u64.to_le_bytes()).unwrap();
        assert_eq!(guest_plt_target(&memory, 0x1000), Some(0x1100));
        memory.write(0x8278, &0x8000u64.to_le_bytes()).unwrap();
        assert_eq!(guest_plt_target(&memory, 0x1000), None);
        memory.write(0x8278, &0x1100u64.to_le_bytes()).unwrap();
        memory
            .load_bytes(0x1008, &0x9109c210u32.to_le_bytes())
            .unwrap();
        assert_eq!(guest_plt_target(&memory, 0x1000), None);
        assert_eq!(guest_plt_target(&memory, u64::MAX), None);
    }

    #[test]
    fn abort_frames_are_bounded_and_reject_invalid_or_non_code_returns() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_EXECUTE,
                MemoryKind::Code,
            )
            .unwrap();
        memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Stack,
            )
            .unwrap();
        memory
            .write(
                0x8000,
                &[0x8010u64.to_le_bytes(), 0x1104u64.to_le_bytes()].concat(),
            )
            .unwrap();
        memory
            .write(
                0x8010,
                &[0x8000u64.to_le_bytes(), 0x1204u64.to_le_bytes()].concat(),
            )
            .unwrap();
        assert_eq!(
            guest_frame_returns(&memory, 0x8000, 0x1004),
            vec![0x1004, 0x1104, 0x1204]
        );
        assert_eq!(guest_frame_returns(&memory, 0x8001, 0), Vec::<u64>::new());
        assert_eq!(
            guest_frame_returns(&memory, 0x9000, 0x8000),
            Vec::<u64>::new()
        );
        memory.write(0x8008, &0x8004u64.to_le_bytes()).unwrap();
        assert_eq!(guest_frame_returns(&memory, 0x8000, 0x1004), vec![0x1004]);
        for index in 0..32u64 {
            let frame = 0x8000 + index * 16;
            memory
                .write(
                    frame,
                    &[
                        (frame + 16).to_le_bytes(),
                        (0x1104 + index * 4).to_le_bytes(),
                    ]
                    .concat(),
                )
                .unwrap();
        }
        assert_eq!(guest_frame_returns(&memory, 0x8000, 0x1004).len(), 17);
    }
}
