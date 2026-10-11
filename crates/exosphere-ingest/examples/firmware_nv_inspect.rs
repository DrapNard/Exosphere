// SPDX-License-Identifier: GPL-3.0-or-later
//! Locate the user-dumped NV services program and verify its executable partition.
//! Prints metadata only: never keys, rights IDs, or decrypted firmware payloads.

use exosphere_ingest::{nca, nca_crypto::UserKeyProvider, nca_partition, nso, reader::Reader};
use std::{fs, io::Read, path::PathBuf};

#[path = "firmware_nv_inspect/falcon.rs"]
mod falcon;

#[path = "firmware_nv_inspect/arm64.rs"]
mod arm64;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let directory = PathBuf::from(arguments.next().ok_or("missing firmware directory")?);
    let keys = PathBuf::from(arguments.next().ok_or("missing user key file")?);
    if arguments.next().is_some() {
        return Err("unexpected argument".into());
    }
    let provider = UserKeyProvider::from_key_file(keys)?;
    let mut paths = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut found = false;
    for path in paths {
        if path.extension().is_none_or(|extension| extension != "nca") {
            continue;
        }
        let mut file = fs::File::open(&path)?;
        let mut encrypted = [0; nca::HEADER_AND_FS_HEADERS];
        file.read_exact(&mut encrypted)?;
        let header = nca::parse_with_provider(&encrypted, &provider)?;
        if header.program_id != 0x0100000000000019 || header.content_type != 0 {
            continue;
        }
        if file.metadata()?.len() != header.content_size {
            return Err("NCA size mismatch".into());
        }
        if found {
            return Err("multiple NV program candidates; select a firmware version".into());
        }
        found = true;
        println!("NV services program: {}", path.display());
        println!(
            "Key generation: {}",
            header.key_generation.max(header.key_generation_old)
        );
        for (index, section) in header.sections.iter().enumerate() {
            if section.is_none() || header.fs_headers[index].fs_type != nca::FsType::PartitionFs {
                continue;
            }
            let partition = nca_partition::read_partition(
                &mut file,
                0,
                &header,
                index,
                &provider,
                64 * 1024 * 1024,
            )
            .map_err(|error| {
                if !matches!(error, exosphere_ingest::IngestError::MissingKey) {
                    return Box::<dyn std::error::Error>::from(error);
                }
                // Key names/revisions are metadata, not secret key material.
                // Never include the rights ID or decrypted key area in errors.
                let requirement = if header.rights_id != [0; 16] {
                    "a resolved title key (no ticket was loaded by this inspector)".to_owned()
                } else {
                    let kind = match header.key_area_key_index {
                        0 => "application",
                        1 => "ocean",
                        2 => "system",
                        _ => return Box::<dyn std::error::Error>::from(error),
                    };
                    let revision = header.key_generation.max(header.key_generation_old)
                        .saturating_sub(1);
                    format!("key_area_key_{kind}_{revision:02x}")
                };
                Box::<dyn std::error::Error>::from(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("NV executable section {index} requires {requirement}; use matching user-dumped keys"),
                ))
            })?;
            println!("Verified PartitionFS section {index}");
            for entry in &partition.index.entries {
                println!("  {}: {} bytes", entry.name, entry.size);
                if entry.name == "main" {
                    let offset = usize::try_from(entry.offset)?;
                    let size = usize::try_from(entry.size)?;
                    let bytes = Reader::new(&partition.bytes).slice(offset, size)?;
                    let header = nso::parse(bytes)?;
                    println!(
                        "  NSO flags: {:#x}; ZBIC: {}",
                        header.flags, header.zbic_compression
                    );
                    inspect_nv_module(&nso::load(bytes)?)?;
                }
            }
        }
    }
    if !found {
        return Err("NV services program not found".into());
    }
    Ok(())
}

/// Report only bounded module metadata and locations of predefined search
/// markers. Never print arbitrary firmware strings, instructions or payloads.
fn inspect_nv_module(module: &nso::LoadedNso) -> Result<(), exosphere_ingest::IngestError> {
    inspect_va_callback_tables(module);
    if let Ok(value) = std::env::var("EXOSPHERE_NV_ENTRY_PROBE") {
        let entry = u64::from_str_radix(value.strip_prefix("0x").unwrap_or(&value), 16)
            .map_err(|_| exosphere_ingest::IngestError::Invalid)?;
        inspect_entry_pointer_references(module, entry);
        if let Ok(value) = std::env::var("EXOSPHERE_NV_CALL_PROBE") {
            let site = u64::from_str_radix(value.strip_prefix("0x").unwrap_or(&value), 16)
                .map_err(|_| exosphere_ingest::IngestError::Invalid)?;
            let joined = usize::try_from(entry)
                .ok()
                .zip(usize::try_from(site).ok())
                .and_then(|(start, site)| arm64::direct_call_paths(&module.text, start, site));
            println!("    explicit call-window probe: {joined:?}");
            if let Some(joined) = &joined
                && let Ok(target) = usize::try_from(joined.call.target)
            {
                println!(
                    "    explicit call-window callee: target={target:#x} entry={:?} edges={:x?}",
                    entry_memory_provenance(&module.text, target),
                    arm64_edges(&module.text, target, 128)
                );
            }
            let barriers = usize::try_from(entry)
                .ok()
                .zip(usize::try_from(site).ok())
                .and_then(|(start, site)| arm64::call_path_barriers(&module.text, start, site));
            println!("    explicit call-window barriers: {barriers:x?}");
        }
    }
    for (name, segment, bytes) in [
        ("text", module.header.text, &module.text),
        ("rodata", module.header.rodata, &module.rodata),
        ("data", module.header.data, &module.data),
    ] {
        println!(
            "  {name}: base={:#x} bytes={} hash_verified={}",
            segment.memory_offset,
            bytes.len(),
            segment.verify_hash
        );
        if name == "text" {
            inspect_ioctl_code(bytes, segment.memory_offset);
        }
        if name != "text" {
            for (query, command) in [
                ("zcull-context-size", 0x80044701u32),
                ("zcull-info", 0x80284702),
            ] {
                let offsets: Vec<_> = bytes
                    .windows(4)
                    .enumerate()
                    .filter_map(|(offset, word)| (word == command.to_le_bytes()).then_some(offset))
                    .collect();
                println!("    {query} ioctl constants: {offsets:x?}");
            }
            for marker in ["zcull", "fecs", "gpccs", "netlist", "nvhost-ctrl-gpu"] {
                let offsets = marker_offsets(bytes, marker.as_bytes());
                println!(
                    "    {marker} markers: {} offsets={:x?}",
                    offsets.len(),
                    offsets.iter().take(16).collect::<Vec<_>>()
                );
            }
            for (offset, filename) in firmware_filenames(bytes) {
                let address = u64::from(segment.memory_offset) + offset as u64;
                println!("    asset filename candidate: {filename} at={address:#x}");
                inspect_asset_references(module, address, filename)?;
            }
        }
    }
    let symbols = nso::function_symbols(module)?;
    println!("  Defined function symbols: {}", symbols.len());
    for marker in ["zcull", "fecs", "gpccs"] {
        let matches: Vec<_> = symbols
            .iter()
            .filter(|symbol| symbol.name.to_ascii_lowercase().contains(marker))
            .map(|symbol| (symbol.value, symbol.size))
            .collect();
        println!("    {marker} symbol ranges: {matches:x?}");
    }
    Ok(())
}

/// Raw table locations only: no relocation application, function-boundary or
/// live-object claims. The opt-in address must identify an aligned text word.
fn inspect_entry_pointer_references(module: &nso::LoadedNso, entry: u64) {
    if entry == 0
        || entry % 4 != 0
        || entry
            .checked_add(4)
            .is_none_or(|end| end > module.text.len() as u64)
    {
        println!("    entry-pointer probe: invalid text address");
        return;
    }
    println!(
        "    entry-pointer probe boundary: entry={entry:#x} preceding_transfer={:?}",
        arm64::preceding_transfer_kind(&module.text, entry as usize)
    );
    let incoming: Vec<_> = arm64_edges(&module.text, 0, module.text.len())
        .into_iter()
        .filter(|(_, _, target)| *target == entry as i64)
        .collect();
    println!("    entry-pointer probe incoming edges: {incoming:x?}");
    for (name, segment, bytes) in [
        ("rodata", module.header.rodata, &module.rodata),
        ("data", module.header.data, &module.data),
    ] {
        let locations = aligned_pointer_locations(bytes, u64::from(segment.memory_offset), entry);
        println!(
            "    entry-pointer probe: entry={entry:#x} segment={name} raw_locations={locations:x?}"
        );
        for address in locations {
            let offset = (address - u64::from(segment.memory_offset)) as usize;
            let start = offset.saturating_sub(32);
            let end = offset.saturating_add(40).min(bytes.len());
            for slot in (start..end).step_by(8) {
                let Ok(target) = Reader::new(bytes).u64(slot) else {
                    continue;
                };
                if target != 0
                    && target % 4 == 0
                    && target
                        .checked_add(4)
                        .is_some_and(|end| end <= module.text.len() as u64)
                {
                    println!(
                        "    entry-pointer neighboring raw text-pointer candidate: address={:#x} target={target:#x}",
                        u64::from(segment.memory_offset) + slot as u64
                    );
                }
            }
        }
    }
}

fn aligned_pointer_locations(bytes: &[u8], base: u64, target: u64) -> Vec<u64> {
    let start = ((8 - base % 8) % 8) as usize;
    bytes
        .get(start..)
        .unwrap_or_default()
        .chunks_exact(8)
        .enumerate()
        .filter_map(|(index, word)| {
            let address = base
                .checked_add(start as u64)?
                .checked_add((index as u64).checked_mul(8)?)?;
            (word == target.to_le_bytes()).then_some(address)
        })
        .collect()
}

/// Candidate table stores, not a proof that an initialized object reaches the
/// observed callback. Only bounded pointer, control-flow and store metadata is
/// reported; table entries are not treated as relocation-validated functions.
fn inspect_va_callback_tables(module: &nso::LoadedNso) {
    for (name, segment, data) in [
        ("rodata", module.header.rodata, &module.rodata),
        ("data", module.header.data, &module.data),
    ] {
        let start = u64::from(segment.memory_offset);
        let end = start + data.len() as u64;
        for (site, pointer, origin) in pointer_store_candidates(&module.text, 0x38, start, end) {
            let offset = (pointer - start) as usize;
            let target = Reader::new(data).u64(offset).ok().filter(|target| {
                *target != 0 && *target % 4 == 0 && *target < module.text.len() as u64
            });
            println!(
                "    VA-callback table candidate: store={site:#x} segment={name} pointer={pointer:#x} base={:?} raw_slot0_text_target={target:x?}",
                origin.base
            );
            if let Some(target) = target {
                println!(
                    "    VA-callback target entry: {:?} edges={:x?}",
                    entry_memory_provenance(&module.text, target as usize),
                    arm64_edges(&module.text, target as usize, 512)
                );
                for (site, kind, _) in arm64_edges(&module.text, target as usize, 512) {
                    if kind == "call" {
                        if let Some(call) = arm64::direct_call_at(&module.text, site)
                            && call.additional_arguments[4] == arm64::Expression::Constant(1)
                        {
                            inspect_va_constructor_candidate(&module.text, call.target as usize);
                        }
                        println!(
                            "    VA-callback target call candidate: {:?}",
                            arm64::direct_call_at(&module.text, site)
                        );
                    }
                }
                for site in (target as usize
                    ..(target as usize).saturating_add(512).min(module.text.len()))
                    .step_by(4)
                {
                    for store in arm64::store_origins_at(&module.text, site) {
                        if matches!(store.source, arm64::Expression::Constant(_)) {
                            println!(
                                "    VA-callback constant-store candidate: site={site:#x} value={store:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// A bounded neighborhood of a selected call target, not a function extent or
/// proof of constructor semantics. Unknown expressions remain visible.
fn inspect_va_constructor_candidate(bytes: &[u8], entry: usize) {
    let mut range_targets = std::collections::BTreeSet::new();
    println!(
        "    VA-constructor candidate: target={entry:#x} entry={:?} edges={:x?}",
        entry_memory_provenance(bytes, entry),
        arm64_edges(bytes, entry, 512)
    );
    for (site, kind, _) in arm64_edges(bytes, entry, 512) {
        if kind == "call" {
            if let Some(call) = arm64::direct_call_at(bytes, site)
                && call.additional_arguments[1] == arm64::Expression::Constant(1)
                && matches!(call.argument0, arm64::Expression::Offset(_, 248 | 400))
            {
                range_targets.insert(call.target as usize);
            }
            println!(
                "    VA-constructor nested call candidate: {:?}",
                arm64::direct_call_at(bytes, site)
            );
        }
    }
    for site in (entry..entry.saturating_add(512).min(bytes.len())).step_by(4) {
        for store in arm64::store_origins_at(bytes, site) {
            println!("    VA-constructor store candidate: site={site:#x} value={store:?}");
        }
    }
    for target in range_targets {
        inspect_va_range_initializer_candidate(bytes, target);
    }
}

/// Inspect only a bounded callee neighborhood. Matching caller arguments select
/// a candidate; they do not establish object identity or reachable store paths.
fn inspect_va_range_initializer_candidate(bytes: &[u8], entry: usize) {
    println!(
        "    VA-range initializer candidate: target={entry:#x} entry={:?} edges={:x?}",
        entry_memory_provenance(bytes, entry),
        arm64_edges(bytes, entry, 256)
    );
    if let Some(call) = arm64::first_call(bytes, entry) {
        for register in [19, 20, 21, 23] {
            println!(
                "    VA-range initializer saved-input candidate: site={:#x} register={register} origin={:?}",
                call.site,
                arm64::register_origin(bytes, entry, call.site, register)
            );
        }
    }
    for site in (entry..entry.saturating_add(256).min(bytes.len())).step_by(4) {
        for store in arm64::store_origins_at(bytes, site) {
            println!("    VA-range initializer store candidate: site={site:#x} value={store:?}");
        }
    }
}

fn pointer_store_candidates(
    bytes: &[u8],
    field: u64,
    start: u64,
    end: u64,
) -> Vec<(usize, u64, arm64::WriteOrigin)> {
    arm64::field_writes(bytes, 0, bytes.len(), field, 64)
        .into_iter()
        .filter_map(|write| {
            if write.bank != arm64::RegisterBank::Integer
                || write.base == 31
                || write.bits_written != 64
                || write.source_bit_offset != 0
                || write.field_bit_offset != 0
            {
                return None;
            }
            let origin = arm64::write_origin(bytes, &write)?;
            let arm64::Expression::Constant(pointer) = origin.source else {
                return None;
            };
            (pointer >= start
                && pointer.checked_add(8).is_some_and(|limit| limit <= end)
                && pointer != 0
                && pointer % 8 == 0)
                .then_some((write.site, pointer, origin))
        })
        .collect()
}

fn inspect_ioctl_code(bytes: &[u8], base: u32) {
    inspect_va_regions_dispatch(bytes);
    let mut callback_offsets = std::collections::BTreeSet::new();
    for (offset, field) in ioctl_immediate_candidates(bytes) {
        println!(
            "    ioctl immediate candidate at={:#x} field={field:#x}",
            u64::from(base) + offset as u64
        );
        if field == 0x4701 || field == 0x4702 {
            let edges = arm64_edges(bytes, offset, 48);
            println!("      nearby ARM64 edges: {edges:x?}");
            if let Some((command, target)) = arm64_ioctl_dispatch(bytes, offset) {
                println!("      exact ioctl dispatch: command={command:#x} handler={target:#x}");
                println!(
                    "      handler argument provenance: {:?}",
                    arm64::first_call(bytes, target)
                );
                println!(
                    "      handler candidate edges: {:x?}",
                    arm64_edges(bytes, target, 96)
                );
                for (_, kind, callee) in arm64_edges(bytes, target, 16) {
                    if kind == "call"
                        && let Ok(callee) = usize::try_from(callee)
                    {
                        println!(
                            "      callee constant-return probe at={callee:#x}: {:?}",
                            arm64_constant_return(bytes, callee)
                        );
                        println!(
                            "      callee access metadata: {:x?}",
                            arm64_access_metadata(bytes, callee)
                        );
                        if let Some((size, field, base)) = arm64_getter(bytes, callee) {
                            println!(
                                "      exact getter: bits={size} object_offset={field:#x} base_register={base}"
                            );
                            println!(
                                "      constant field-store candidates: {:x?}",
                                arm64_field_stores(bytes, field, size)
                            );
                            inspect_field_output_calls(bytes, field);
                            if base == 1
                                && let Some(call) = arm64::first_call(bytes, target)
                                && let arm64::Expression::Offset(_, object_offset) = call.argument1
                                && let Some(offset) = object_offset.checked_add(field)
                            {
                                println!(
                                    "      enclosing object field={offset:#x} constant-store candidates: {:x?}",
                                    arm64_field_stores(bytes, offset, size)
                                );
                                inspect_field_output_calls(bytes, offset);
                                for write in
                                    arm64::field_writes(bytes, 0, bytes.len(), offset, size)
                                        .into_iter()
                                        .filter(|write| write.base != 31)
                                {
                                    println!(
                                        "      enclosing object overlapping-write candidate {write:?} preceding_edges={:x?}",
                                        arm64_edges(bytes, write.site.saturating_sub(48), 48)
                                    );
                                }
                                for site in arm64_offset_candidates(bytes, object_offset) {
                                    if let Some(call) = arm64::first_call(bytes, site)
                                        && (matches!(
                                            call.argument0,
                                            arm64::Expression::Offset(_, _)
                                        ) || matches!(
                                            call.argument1,
                                            arm64::Expression::Offset(_, _)
                                        ))
                                        && let Ok(callee) = usize::try_from(call.target)
                                    {
                                        println!(
                                            "        subobject callee access metadata: {:x?}",
                                            arm64_access_metadata(bytes, callee)
                                        );
                                        println!(
                                            "        subobject entry argument provenance: {:?}",
                                            arm64::first_call(bytes, callee)
                                        );
                                        println!(
                                            "        subobject bounded-region edges: {:x?}",
                                            arm64_edges(bytes, callee, 1024)
                                        );
                                        for site in arm64::indirect_call_sites(bytes, callee, 1024)
                                        {
                                            if let Some(call) = arm64::indirect_call_at(bytes, site)
                                            {
                                                if let arm64::Expression::Load {
                                                    offset,
                                                    bits: 64,
                                                    base,
                                                } = &call.target
                                                {
                                                    if let arm64::Expression::Input(register) =
                                                        **base
                                                        && let Some(entry_call) =
                                                            arm64::first_call(bytes, callee)
                                                    {
                                                        println!(
                                                            "        callback base before first direct call: {:?}",
                                                            arm64::register_origin(
                                                                bytes,
                                                                callee,
                                                                entry_call.site,
                                                                register
                                                            )
                                                        );
                                                    }
                                                    if callback_offsets.insert(*offset) {
                                                        inspect_callback_installations(
                                                            bytes, *offset,
                                                        );
                                                    }
                                                }
                                                println!(
                                                    "        subobject indirect-call at={site:#x} register={} target={:?} arguments={:?} unmodeled={:?}",
                                                    call.register,
                                                    call.target,
                                                    (
                                                        call.argument0,
                                                        call.argument1,
                                                        call.argument2
                                                    ),
                                                    call.unmodeled
                                                );
                                            } else {
                                                println!(
                                                    "        subobject indirect-call at={site:#x} provenance=unknown"
                                                );
                                            }
                                        }
                                        for (site, kind, _) in arm64_edges(bytes, callee, 1024) {
                                            if kind == "call" {
                                                println!(
                                                    "        nested-call argument candidates: {:?}",
                                                    arm64::direct_call_at(bytes, site)
                                                );
                                            }
                                        }
                                        for write in
                                            arm64::field_writes(bytes, callee, 1024, field, size)
                                        {
                                            println!(
                                                "        subobject field-write candidate {write:?} preceding_edges={:x?}",
                                                arm64_edges(
                                                    bytes,
                                                    write.site.saturating_sub(48),
                                                    48
                                                )
                                            );
                                        }
                                    }
                                    println!(
                                        "      subobject reference={site:#x} first-call provenance: {:?}",
                                        arm64::first_call(bytes, site)
                                    );
                                }
                                // A cached result may be written through a subobject
                                // pointer rather than the enclosing object's base.
                                // Nearby calls are leads, not proof of provenance.
                                for write in arm64::field_writes(bytes, 0, bytes.len(), field, size)
                                    .into_iter()
                                    .filter(|write| {
                                        write.base != 31
                                            && write.source != 31
                                            && write.bank == arm64::RegisterBank::Integer
                                            && write.source_bits == size
                                            && write.bits_written == size
                                    })
                                {
                                    let edges =
                                        arm64_edges(bytes, write.site.saturating_sub(32), 32);
                                    if edges.iter().any(|(_, kind, _)| *kind == "call") {
                                        println!(
                                            "      subobject call-adjacent write candidate {write:?} preceding_edges={edges:x?} origin={:?}",
                                            arm64::write_origin(bytes, &write)
                                        );
                                        for (site, kind, callee) in edges {
                                            if kind == "call" {
                                                println!(
                                                    "        write-adjacent call at={site:#x} arguments={:?} callee_access={:x?}",
                                                    arm64::first_call(
                                                        bytes,
                                                        site.saturating_sub(12)
                                                    ),
                                                    arm64_access_metadata(bytes, callee as usize)
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Resolve the exact query dispatcher before treating any cached bounds as VA
/// policy. Only metadata escapes; offset matches alone do not prove ownership.
fn inspect_va_regions_dispatch(bytes: &[u8]) {
    for site in (0..bytes.len()).step_by(4) {
        if let Some((0x40284109, handler)) = arm64_ioctl_dispatch(bytes, site) {
            println!("    VA-initialize exact dispatch: site={site:#x} handler={handler:#x}");
            println!(
                "    VA-initialize handler edges: {:x?}",
                arm64_edges(bytes, handler, 128)
            );
            println!(
                "    VA-initialize entry memory: {:?}",
                entry_memory_provenance(bytes, handler)
            );
            for (_, kind, target) in arm64_edges(bytes, handler, 16) {
                if matches!(
                    kind,
                    "branch"
                        | "compare-zero"
                        | "compare-nonzero"
                        | "test-bit-set"
                        | "test-bit-clear"
                ) {
                    let target = target as usize;
                    println!(
                        "    VA-initialize continuation edges: {:x?}",
                        arm64_edges(bytes, target, 32)
                    );
                    println!(
                        "    VA-initialize continuation: target={target:#x} direct={:?} entry={:?}",
                        arm64::first_call(bytes, target),
                        entry_memory_provenance(bytes, target)
                    );
                    if let Some(call) = arm64::first_call(bytes, target) {
                        let after = call.site + 4;
                        println!(
                            "    VA-initialize after-call edges: {:x?}",
                            arm64_edges(bytes, after, 64)
                        );
                        println!(
                            "    VA-initialize after-call next: {:?}",
                            arm64::first_call(bytes, after)
                        );
                        for (site, kind, target) in arm64_edges(bytes, after, 64) {
                            if kind == "call" {
                                println!(
                                    "    VA-initialize later call candidate: {:?} entry={:?}",
                                    arm64::direct_call_at(bytes, site),
                                    entry_memory_provenance(bytes, target as usize)
                                );
                                println!(
                                    "    VA-initialize later callee edges: {:x?}",
                                    arm64_edges(bytes, target as usize, 256)
                                );
                                for site in arm64::indirect_call_sites(bytes, target as usize, 256)
                                {
                                    println!(
                                        "    VA-initialize later indirect candidate: {:?}",
                                        arm64::indirect_call_at(bytes, site)
                                    );
                                }
                            }
                        }
                        for site in (after..after.saturating_add(64).min(bytes.len())).step_by(4) {
                            let stores = arm64::store_origins_at(bytes, site);
                            if !stores.is_empty() {
                                println!(
                                    "    VA-initialize after-call store: site={site:#x} values={stores:?}"
                                );
                            }
                        }
                    }
                    for site in arm64::indirect_call_sites(bytes, target, 32) {
                        println!(
                            "    VA-initialize continuation indirect candidate: {:?}",
                            arm64::indirect_call_at(bytes, site)
                        );
                    }
                    for site in (target..target.saturating_add(32).min(bytes.len())).step_by(4) {
                        if let Some(tail) = arm64::indirect_tail_at(bytes, site) {
                            println!("    VA-initialize tail transfer: {tail:?}");
                        }
                    }
                }
            }
        }
    }
    for field in [0x28, 0x2c, 0x7c, 0x80] {
        let writes = arm64::field_writes(bytes, 0, bytes.len(), field, 32);
        let mut constants = 0;
        for write in writes.iter().filter(|write| write.base != 31) {
            if let Some(origin) = arm64::write_origin(bytes, write)
                && let arm64::Expression::Constant(value) = origin.source
                && value != 0
            {
                constants += 1;
                println!(
                    "    VA-field constant-store candidate: field={field:#x} site={:#x} value={value:#x} base={:?} bits={} source_bit_offset={}",
                    write.site, origin.base, write.bits_written, write.source_bit_offset
                );
            }
        }
        println!(
            "    VA-field store counts: field={field:#x} overlapping={} nonzero_constant={constants}",
            writes.len()
        );
    }
    for site in (0..bytes.len()).step_by(4) {
        if let Some((0xc0404108, handler)) = arm64_ioctl_dispatch(bytes, site) {
            let call = arm64::first_call(bytes, handler);
            println!(
                "    VA-regions exact dispatch: site={site:#x} handler={handler:#x} call={call:?}"
            );
            println!(
                "    VA-regions handler entry memory: {:?}",
                entry_memory_provenance(bytes, handler)
            );
            println!(
                "    VA-regions handler edges: {:x?}",
                arm64_edges(bytes, handler, 128)
            );
            for (site, kind, _) in arm64_edges(bytes, handler, 128) {
                if kind == "call" {
                    println!(
                        "    VA-regions handler call candidate: {:?}",
                        arm64::direct_call_at(bytes, site)
                    );
                }
            }
            for site in (handler..handler.saturating_add(128).min(bytes.len())).step_by(4) {
                let stores = arm64::store_origins_at(bytes, site);
                if !stores.is_empty() {
                    println!(
                        "    VA-regions handler store origins: site={site:#x} stores={stores:?}"
                    );
                }
                let Some(word) = bytes.get(site..site + 4) else {
                    continue;
                };
                let raw = u32::from_le_bytes(word.try_into().unwrap());
                if raw & 0x3be00c00 == 0x38600800 && (raw >> 13) & 7 == 3 {
                    let start = arm64_edges(bytes, handler, site - handler)
                        .last()
                        .map(|(edge, _, _)| edge + 4)
                        .unwrap_or(handler);
                    println!(
                        "    VA-regions indexed read provenance: site={site:#x} value={:?}",
                        arm64::register_origin(bytes, start, site + 4, (raw & 31) as u8)
                    );
                }
            }
            if let Some(call) = call {
                let target = call.target as usize;
                println!(
                    "    VA-regions callee entry memory: {:?}",
                    entry_memory_provenance(bytes, target)
                );
                println!(
                    "    VA-regions callee edges: {:x?}",
                    arm64_edges(bytes, target, 128)
                );
                for field in (0..64).step_by(8) {
                    let writes = arm64::field_writes(bytes, target, 128, field, 64);
                    if !writes.is_empty() {
                        println!("    VA-regions candidate writes field={field:#x}: {writes:?}");
                    }
                }
            }
        }
    }
}

/// Only report bounded relative firmware filenames, never arbitrary strings or
/// absolute paths. An unterminated final byte run is not a filename field.
fn firmware_filenames(bytes: &[u8]) -> Vec<(usize, &str)> {
    let mut output = Vec::new();
    let mut offset = 0;
    for run in bytes.split_inclusive(|byte| *byte == 0) {
        if let Some(name) = run.strip_suffix(&[0])
            && !name.is_empty()
            && name.len() <= 96
            && name.ends_with(b".bin")
            && !name.starts_with(b"/")
            && name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(byte))
            && let Ok(name) = std::str::from_utf8(name)
            && !name.contains("..")
        {
            output.push((offset, name));
        }
        offset += run.len();
    }
    output
}

fn inspect_asset_references(
    module: &nso::LoadedNso,
    name_address: u64,
    filename: &str,
) -> Result<(), exosphere_ingest::IngestError> {
    let regions = [
        (
            "rodata",
            module.header.rodata.memory_offset,
            module.rodata.as_slice(),
        ),
        (
            "data",
            module.header.data.memory_offset,
            module.data.as_slice(),
        ),
    ];
    for (record_kind, record_base, records) in regions {
        for (payload_kind, payload_base, payload) in regions {
            for (record, address, size) in asset_candidates(
                records,
                name_address,
                u64::from(payload_base),
                payload.len(),
            ) {
                println!(
                    "      bounded asset candidate: record={record_kind}:{:#x} payload={payload_kind}:{address:#x} bytes={size}",
                    u64::from(record_base) + record as u64
                );
                if filename == "NETB_img.bin" {
                    let start = (address - u64::from(payload_base)) as usize;
                    let bytes = &payload[start..start + size as usize];
                    match netlist_regions(bytes) {
                        Some((version, regions)) => {
                            println!(
                                "        netlist candidate: version={version} regions={}",
                                regions.len()
                            );
                            for region in regions {
                                println!(
                                    "          region={} kind={} offset={:#x} bytes={}",
                                    region.id,
                                    region.kind(),
                                    region.offset,
                                    region.size
                                );
                                if (15..=18).contains(&region.id) && region.size == 4 {
                                    // These four fields are explicitly scalar metadata in
                                    // Tegra's netlist loader, not arbitrary payload words.
                                    let value = Reader::new(bytes).u32(region.offset as usize)?;
                                    println!("            metadata value={value:#x}");
                                }
                                if region.id == 11 {
                                    let records = Reader::new(bytes)
                                        .slice(region.offset as usize, region.size as usize)?;
                                    if let Some((count, min_index, max_index)) =
                                        register_list_summary(records)
                                    {
                                        println!(
                                            "            ZCull register metadata: entries={count} index_range={min_index:#x}..={max_index:#x}"
                                        );
                                    }
                                }
                                if region.id == 1
                                    && let Some(decoder) =
                                        std::env::var_os("EXOSPHERE_FALCON_DECODER")
                                {
                                    let instructions = Reader::new(bytes)
                                        .slice(region.offset as usize, region.size as usize)?;
                                    match falcon::inspect(
                                        std::path::Path::new(&decoder),
                                        instructions,
                                    ) {
                                        Ok(summary) => println!(
                                            "            FECS decode metadata: {summary:?}"
                                        ),
                                        Err(_) => println!(
                                            "            FECS decode metadata: unavailable"
                                        ),
                                    }
                                }
                            }
                        }
                        None => println!("        unsupported netlist candidate layout"),
                    }
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct NetlistRegion {
    id: u32,
    offset: u32,
    size: u32,
}

impl NetlistRegion {
    // These identifiers are shared with the Nouveau loader. Do not guess the
    // meanings of other identifiers from their order or their payload sizes.
    fn kind(&self) -> &'static str {
        match self.id {
            0 => "fecs-data",
            1 => "fecs-instructions",
            2 => "gpccs-data",
            3 => "gpccs-instructions",
            11 => "zcull-gpc-register-list",
            15 => "major-version",
            16 => "general-buffer-size-not-zcull-size",
            17 => "context-register-base-index",
            18 => "netlist-number",
            _ => "unclassified",
        }
    }
}

/// Tegra AIV records contain address/index/value u32s. Summarize only indices;
/// neither register initialization values nor microcode bytes are disclosed.
fn register_list_summary(bytes: &[u8]) -> Option<(usize, u32, u32)> {
    if bytes.is_empty() || bytes.len() % 12 != 0 {
        return None;
    }
    let mut min = u32::MAX;
    let mut max = 0;
    for record in bytes.chunks_exact(12) {
        let index = Reader::new(record).u32(4).ok()?;
        min = min.min(index);
        max = max.max(index);
    }
    Some((bytes.len() / 12, min, max))
}

/// Probe the version/count + (id, size, offset) layout used by Nouveau netlists:
/// https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/nouveau/nvkm/engine/gr/ga102.c
/// A matching bounded table is structural evidence only, not a ZCull size.
fn netlist_regions(bytes: &[u8]) -> Option<(u32, Vec<NetlistRegion>)> {
    let reader = Reader::new(bytes);
    let version = reader.u32(0).ok()?;
    let count = usize::try_from(reader.u32(4).ok()?).ok()?;
    let table_end = 8usize.checked_add(count.checked_mul(12)?)?;
    if count == 0 || table_end > bytes.len() {
        return None;
    }
    let mut regions: Vec<NetlistRegion> = Vec::with_capacity(count);
    for index in 0..count {
        let record = 8 + index * 12;
        let id = reader.u32(record).ok()?;
        let size = reader.u32(record + 4).ok()?;
        let offset = reader.u32(record + 8).ok()?;
        let start = usize::try_from(offset).ok()?;
        let end = start.checked_add(usize::try_from(size).ok()?)?;
        if start < table_end || end > bytes.len() {
            return None;
        }
        // Empty regions may share an endpoint. Nonempty regions must not alias
        // another payload, and duplicate IDs would make lookup ambiguous.
        if regions.iter().any(|region| {
            region.id == id
                || (size != 0
                    && region.size != 0
                    && u64::from(offset) < u64::from(region.offset) + u64::from(region.size)
                    && u64::from(region.offset) < u64::from(offset) + u64::from(size))
        }) {
            return None;
        }
        regions.push(NetlistRegion { id, offset, size });
    }
    Some((version, regions))
}

/// Candidate table layout: name pointer, metadata word, payload pointer, size.
/// These bounds establish a contained asset, not its microcode semantics or a
/// context image size. No bytes from the candidate payload are returned/logged.
fn asset_candidates(
    records: &[u8],
    name: u64,
    payload_base: u64,
    payload_len: usize,
) -> Vec<(usize, u64, u64)> {
    let reader = Reader::new(records);
    records
        .chunks_exact(8)
        .enumerate()
        .filter_map(|(index, word)| {
            let offset = index * 8;
            if u64::from_le_bytes(word.try_into().unwrap()) != name {
                return None;
            }
            let address = reader.u64(offset.checked_add(16)?).ok()?;
            let size = reader.u64(offset.checked_add(24)?).ok()?;
            let start = address.checked_sub(payload_base)?;
            if size == 0 || start.checked_add(size)? > payload_len as u64 {
                return None;
            }
            Some((offset, address, size))
        })
        .collect()
}

fn marker_offsets(bytes: &[u8], marker: &[u8]) -> Vec<usize> {
    if marker.is_empty() {
        return Vec::new();
    }
    bytes
        .windows(marker.len())
        .enumerate()
        .filter_map(|(offset, bytes)| bytes.eq_ignore_ascii_case(marker).then_some(offset))
        .collect()
}

/// AArch64 MOVZ/MOVK fields for predefined ioctl constants only. These are
/// candidates, not proof that an instruction belongs to the matching handler.
fn ioctl_immediate_candidates(bytes: &[u8]) -> Vec<(usize, u32)> {
    bytes
        .chunks_exact(4)
        .enumerate()
        .filter_map(|(index, word)| {
            let word = u32::from_le_bytes(word.try_into().unwrap());
            let field = (word >> 5) & 0xffff;
            (matches!(word & 0x7f800000, 0x52800000 | 0x72800000)
                && matches!(field, 0x4701 | 0x4702 | 0x8004 | 0x8028))
            .then_some((index * 4, field))
        })
        .collect()
}

/// Offset matches alone do not prove that a write and call use the same object.
/// A size field may be initialized through an output pointer, not a direct STR.
/// These call arguments remain candidates until their base object is proven.
fn inspect_field_output_calls(bytes: &[u8], field: u64) {
    let mut targets = std::collections::BTreeSet::new();
    let direct = field_output_call_candidates(bytes, field);
    let indirect = field_indirect_output_candidates(bytes, field);
    println!(
        "      field pointer candidate summary field={field:#x} direct={} indirect={}",
        direct.len(),
        indirect.len()
    );
    for (site, call) in direct {
        let locations = field_pointer_locations(
            [&call.argument0, &call.argument1, &call.argument2]
                .into_iter()
                .chain(call.additional_arguments.iter()),
            field,
        );
        println!(
            "      field pointer locations: field={field:#x} call={:#x} target={:#x} locations={locations:?}",
            call.site, call.target
        );
        if locations.contains(&(0, "register-derived"))
            && matches!(call.argument2, arm64::Expression::Constant(length) if length >= 4)
        {
            // A constant third argument can be a copy length, but the callee's
            // semantics and destination object still require separate proof.
            println!(
                "      field bulk-output candidate: call={:#x} nearby_edges={:x?}",
                call.site,
                arm64_edges(bytes, call.site.saturating_sub(32), 128)
            );
            for (next, kind, _) in arm64_edges(bytes, call.site + 4, 92) {
                if kind == "call" {
                    println!(
                        "        field bulk-output following-call candidate: {:?}",
                        arm64::direct_call_at(bytes, next)
                    );
                }
            }
            for (caller, kind, target) in arm64_edges(bytes, 0, bytes.len()) {
                if kind == "call"
                    && target >= 0
                    && (call.site.saturating_sub(64)..=call.site).contains(&(target as usize))
                {
                    inspect_bulk_output_caller(bytes, caller, target);
                }
            }
        }
        println!(
            "      field output-pointer call candidate from={site:#x} call={call:?} target_access={:x?}",
            arm64_access_metadata(bytes, call.target as usize)
        );
        if targets.insert(call.target) {
            println!(
                "        output-callee entry memory provenance: {:?}",
                entry_memory_provenance(bytes, call.target as usize)
            );
        }
    }
    for (site, call) in indirect {
        println!("      field indirect output-pointer candidate from={site:#x} call={call:?}");
    }
}

fn direct_entry_references(bytes: &[u8], entry: usize) -> Vec<(usize, &'static str, i64)> {
    arm64_edges(bytes, 0, bytes.len())
        .into_iter()
        .filter(|(_, kind, target)| matches!(*kind, "call" | "branch") && *target == entry as i64)
        .collect()
}

/// Two bounded upstream levels only. Assumed window entries and neighboring
/// calls are reported as candidates, never treated as recovered function bounds.
fn inspect_bulk_output_caller(bytes: &[u8], caller: usize, target: i64) {
    println!(
        "        field bulk-output nearby-entry caller candidate: target={target:#x} call={:?} surrounding_edges={:x?}",
        arm64::direct_call_at(bytes, caller),
        arm64_edges(bytes, caller.saturating_sub(64), 128)
    );
    let Some(joined) = arm64::direct_call_paths_at(bytes, caller) else {
        return;
    };
    println!(
        "        field bulk-output caller path join: assumed_start={:#x} reaching_paths={} call={:?}",
        joined.start, joined.reaching_paths, joined.call
    );
    let references = direct_entry_references(bytes, joined.start);
    println!("        field bulk-output joined-entry references: {references:x?}");
    for (site, kind, _) in references {
        if kind != "call" {
            continue;
        }
        let upstream = arm64::direct_call_paths_at(bytes, site);
        println!("        field bulk-output upstream-call candidate: {upstream:?}");
        if let Some(upstream) = upstream {
            println!(
                "        field bulk-output upstream preceding-indirect-call candidate: {:?}",
                upstream
                    .start
                    .checked_sub(4)
                    .and_then(|site| arm64::indirect_call_at(bytes, site))
            );
            println!(
                "        field bulk-output upstream-entry references: {:x?}",
                direct_entry_references(bytes, upstream.start)
            );
        }
    }
}

/// Classify the symbolic base, not memory ownership. In particular an incoming
/// general-purpose register may still alias the stack or an unrelated object.
fn field_pointer_locations<'a>(
    arguments: impl Iterator<Item = &'a arm64::Expression>,
    field: u64,
) -> Vec<(usize, &'static str)> {
    fn base_kind(base: &arm64::Expression) -> &'static str {
        match base {
            arm64::Expression::Input(31) => "stack-relative",
            arm64::Expression::Offset(base, _) | arm64::Expression::Subtract(base, _) => {
                base_kind(base)
            }
            arm64::Expression::Constant(_) => "constant-derived",
            arm64::Expression::Unknown => "unknown-derived",
            _ => "register-derived",
        }
    }
    arguments
        .enumerate()
        .filter_map(|(index, argument)| match argument {
            arm64::Expression::Offset(base, offset) if *offset == field => {
                Some((index, base_kind(base)))
            }
            _ => None,
        })
        .collect()
}

fn field_indirect_output_candidates(bytes: &[u8], field: u64) -> Vec<(usize, arm64::IndirectCall)> {
    arm64_offset_candidates(bytes, field)
        .into_iter()
        .filter_map(|site| {
            let call = arm64::first_indirect_call(bytes, site)?;
            let is_field = |argument: &arm64::Expression| {
                matches!(argument,
            arm64::Expression::Offset(_, offset) if *offset == field)
            };
            (is_field(&call.argument0) || is_field(&call.argument1) || is_field(&call.argument2))
                .then_some((site, call))
        })
        .collect()
}

/// Entry-relative bases for scalar unsigned accesses before control flow.
/// A missing origin is omitted, not treated as absence of a memory access.
fn entry_memory_provenance(
    bytes: &[u8],
    start: usize,
) -> Vec<(usize, bool, u64, u16, arm64::Expression)> {
    use exosphere_cpu_ir::{DecodedOpcode, decode::decode_one_at};
    let Some(window) = bytes.get(start..start.saturating_add(64).min(bytes.len())) else {
        return vec![];
    };
    window
        .chunks_exact(4)
        .enumerate()
        .filter_map(|(index, word)| {
            let site = start + index * 4;
            let DecodedOpcode::LoadStoreUnsigned {
                rn,
                offset,
                load,
                size_bits,
                ..
            } = decode_one_at(u32::from_le_bytes(word.try_into().unwrap()), site as u64)
            else {
                return None;
            };
            let origin = arm64::register_origin(bytes, start, site, rn)?;
            Some((site, load, offset, size_bits, origin.value))
        })
        .collect()
}

fn field_output_call_candidates(bytes: &[u8], field: u64) -> Vec<(usize, arm64::CallArguments)> {
    let mut candidates = Vec::new();
    for site in arm64_offset_candidates(bytes, field) {
        let Some(call) = arm64::first_call(bytes, site) else {
            continue;
        };
        let is_field = |argument: &arm64::Expression| {
            matches!(argument,
            arm64::Expression::Offset(_, offset) if *offset == field)
        };
        // A memcpy-like destination is commonly X0; other APIs may place
        // output pointers in any AAPCS64 integer argument register.
        if [&call.argument0, &call.argument1, &call.argument2]
            .into_iter()
            .chain(call.additional_arguments.iter())
            .any(is_field)
        {
            candidates.push((site, call));
        }
    }
    candidates
}

fn inspect_callback_installations(bytes: &[u8], offset: u64) {
    for (site, target, origin) in callback_candidates(bytes, offset) {
        println!(
            "        callback installation candidate at={site:#x} field={offset:#x} target={target:#x} base={:?} unmodeled={:?}",
            origin.base, origin.unmodeled
        );
        println!(
            "          callback target access={:x?} edges={:x?}",
            arm64_access_metadata(bytes, target as usize),
            arm64_edges(bytes, target as usize, 128)
        );
        let callers: Vec<_> = arm64_edges(bytes, 0, bytes.len())
            .into_iter()
            .filter(|(_, kind, target)| {
                *kind == "call"
                    && *target >= 0
                    && (site.saturating_sub(256)..=site).contains(&(*target as usize))
            })
            .collect();
        println!("          installer nearby-entry caller candidates: {callers:x?}");
        for (site, _, _) in callers {
            println!(
                "          installer caller argument candidates: {:?}",
                arm64::direct_call_at(bytes, site)
            );
        }
    }
}

fn callback_candidates(bytes: &[u8], offset: u64) -> Vec<(usize, u64, arm64::WriteOrigin)> {
    let mut candidates = Vec::new();
    for write in arm64::field_writes(bytes, 0, bytes.len(), offset, 64) {
        if write.bank != arm64::RegisterBank::Integer
            || write.base == 31
            || write.source_bits != 64
            || write.source_bit_offset != 0
            || write.bits_written != 64
        {
            continue;
        }
        let Some(origin) = arm64::write_origin(bytes, &write) else {
            continue;
        };
        if let arm64::Expression::Constant(target) = origin.source
            && target != 0
            && target % 4 == 0
            && target < bytes.len() as u64
        {
            candidates.push((write.site, target, origin));
        }
    }
    candidates
}

fn arm64_edges(bytes: &[u8], start: usize, length: usize) -> Vec<(usize, &'static str, i64)> {
    if start % 4 != 0 {
        return vec![];
    }
    let end = start.saturating_add(length).min(bytes.len());
    let Some(bytes) = bytes.get(start..end) else {
        return vec![];
    };
    bytes
        .chunks_exact(4)
        .enumerate()
        .filter_map(|(index, word)| {
            let word = u32::from_le_bytes(word.try_into().unwrap());
            let pc = start + index * 4;
            let (kind, immediate, bits) = if word & 0xff000010 == 0x54000000 {
                (
                    match word & 15 {
                        0 => "equal",
                        1 => "not-equal",
                        8 => "unsigned-above",
                        9 => "unsigned-below-equal",
                        _ => "conditional",
                    },
                    (word >> 5) & 0x7ffff,
                    19,
                )
            } else if word & 0x7e000000 == 0x34000000 {
                (
                    if word & (1 << 24) != 0 {
                        "compare-nonzero"
                    } else {
                        "compare-zero"
                    },
                    (word >> 5) & 0x7ffff,
                    19,
                )
            } else if word & 0x7e000000 == 0x36000000 {
                (
                    if word & (1 << 24) != 0 {
                        "test-bit-set"
                    } else {
                        "test-bit-clear"
                    },
                    (word >> 5) & 0x3fff,
                    14,
                )
            } else if word & 0x7c000000 == 0x14000000 {
                (
                    if word & 0x80000000 != 0 {
                        "call"
                    } else {
                        "branch"
                    },
                    word & 0x3ffffff,
                    26,
                )
            } else {
                return None;
            };
            let displacement = (i64::from(immediate) << (64 - bits)) >> (64 - bits);
            Some((pc, kind, pc as i64 + displacement * 4))
        })
        .collect()
}

/// Recognize only wide-immediate writes followed by RET. Any other operation
/// stops the probe, so this cannot infer a value across unknown memory/calls.
fn arm64_constant_return(bytes: &[u8], start: usize) -> Option<u64> {
    let bytes = bytes.get(start..)?;
    let mut registers = [None; 32];
    for word in bytes.chunks_exact(4).take(16) {
        let word = u32::from_le_bytes(word.try_into().unwrap());
        if word == 0xd65f03c0 {
            return registers[0];
        }
        let kind = word & 0x7f800000;
        if !matches!(kind, 0x12800000 | 0x52800000 | 0x72800000) {
            return None;
        }
        let wide = word >> 31 != 0;
        let shift = ((word >> 21) & 3) * 16;
        if !wide && shift > 16 {
            return None;
        }
        let destination = (word & 31) as usize;
        let value = u64::from((word >> 5) & 0xffff) << shift;
        let mask = if wide { u64::MAX } else { u64::from(u32::MAX) };
        registers[destination] = Some(match kind {
            0x12800000 => !value & mask,
            0x52800000 => value,
            0x72800000 => (registers[destination]? & !(0xffff << shift) | value) & mask,
            _ => return None,
        });
    }
    None
}

/// Use the project decoder for bounded access metadata, not raw instructions.
fn arm64_access_metadata(bytes: &[u8], start: usize) -> Vec<(usize, &'static str, u64)> {
    use exosphere_cpu_ir::{DecodedOpcode, decode::decode_one_at};
    let Some(bytes) = bytes.get(start..start.saturating_add(64).min(bytes.len())) else {
        return vec![];
    };
    let mut output = Vec::new();
    for (index, word) in bytes.chunks_exact(4).enumerate() {
        let pc = start + index * 4;
        match decode_one_at(u32::from_le_bytes(word.try_into().unwrap()), pc as u64) {
            DecodedOpcode::LoadStoreUnsigned {
                offset,
                load,
                size_bits,
                ..
            } => {
                let kind = match (load, size_bits) {
                    (true, 32) => "read32-offset",
                    (true, 64) => "read64-offset",
                    (true, _) => "read-offset",
                    (false, _) => "write-offset",
                };
                output.push((pc, kind, offset));
            }
            DecodedOpcode::BranchReg { rn, link } => {
                output.push((
                    pc,
                    if link {
                        "call-register"
                    } else {
                        "branch-register"
                    },
                    u64::from(rn),
                ));
                if !link {
                    break;
                }
            }
            DecodedOpcode::Unknown => output.push((pc, "unclassified", 0)),
            DecodedOpcode::BranchImm { target, link } => {
                output.push((pc, if link { "call" } else { "branch" }, target));
                if !link {
                    break;
                }
            }
            DecodedOpcode::Ret { .. } => break,
            _ => {}
        }
    }
    output
}

fn arm64_getter(bytes: &[u8], start: usize) -> Option<(u16, u64, u8)> {
    use exosphere_cpu_ir::{DecodedOpcode, decode::decode_one};
    let reader = Reader::new(bytes);
    let DecodedOpcode::LoadStoreUnsigned {
        rt: 0,
        rn,
        offset,
        load: true,
        size_bits,
    } = decode_one(reader.u32(start).ok()?)
    else {
        return None;
    };
    if !matches!(
        decode_one(reader.u32(start.checked_add(4)?).ok()?),
        DecodedOpcode::Ret { rn: 30 }
    ) {
        return None;
    }
    Some((size_bits, offset, rn))
}

/// Require the full MOVZ/MOVK/CMP/B.EQ chain, including register identity.
/// A nearby branch alone is not evidence for an ioctl's equality handler.
fn arm64_ioctl_dispatch(bytes: &[u8], start: usize) -> Option<(u32, usize)> {
    if start % 4 != 0 {
        return None;
    }
    let reader = Reader::new(bytes);
    let low = reader.u32(start).ok()?;
    let high = reader.u32(start.checked_add(4)?).ok()?;
    let compare = reader.u32(start.checked_add(8)?).ok()?;
    let branch = reader.u32(start.checked_add(12)?).ok()?;
    if low & 0xffe00000 != 0x52800000
        || high & 0xffe00000 != 0x72a00000
        || low & 31 != high & 31
        || low & 31 == 31
        || compare & 0xffe0fc1f != 0x6b00001f
        || (compare >> 16) & 31 != low & 31
        || branch & 0xff00001f != 0x54000000
    {
        return None;
    }
    let command = ((low >> 5) & 0xffff) | (((high >> 5) & 0xffff) << 16);
    if !matches!(command, 0x80044701 | 0x80284702 | 0xc0404108 | 0x40284109) {
        return None;
    }
    let immediate = ((branch >> 5) & 0x7ffff) as i64;
    let displacement = (immediate << 45 >> 45) * 4;
    let target =
        usize::try_from((start as i64).checked_add(12)?.checked_add(displacement)?).ok()?;
    (target < bytes.len()).then_some((command, target))
}

/// Find local constant stores to a known field offset. Object identity remains
/// unproven, so these are constructor candidates, not initialization evidence.
fn arm64_field_stores(bytes: &[u8], field: u64, bits: u16) -> Vec<(usize, u8, u64)> {
    use exosphere_cpu_ir::{DecodedOpcode, MoveWideKind, decode::decode_one};
    let mut registers = [None; 32];
    let mut output = Vec::new();
    for (index, word) in bytes.chunks_exact(4).enumerate() {
        match decode_one(u32::from_le_bytes(word.try_into().unwrap())) {
            DecodedOpcode::MoveWide {
                rd,
                imm,
                kind: MoveWideKind::MovZ,
                ..
            } if rd != 31 => registers[rd as usize] = Some(imm),
            DecodedOpcode::LoadStoreUnsigned {
                rt,
                rn,
                offset,
                load: false,
                size_bits,
            } => {
                if offset == field
                    && size_bits == bits
                    && let Some(value) = registers[rt as usize]
                {
                    output.push((index * 4, rn, value));
                }
            }
            DecodedOpcode::Nop => {}
            _ => registers.fill(None),
        }
    }
    output
}

fn arm64_offset_candidates(bytes: &[u8], offset: u64) -> Vec<usize> {
    use exosphere_cpu_ir::{DecodedOpcode, decode::decode_one};
    bytes.chunks_exact(4).enumerate().filter_map(|(index, word)| {
        matches!(decode_one(u32::from_le_bytes(word.try_into().unwrap())), DecodedOpcode::AddSubImm { imm, sub: false, set_flags: false, is64: true, .. } if imm == offset).then_some(index * 4)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm_words(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[test]
    fn pointer_locations_require_complete_absolute_aligned_words() {
        let target = 0x7d5f4u64;
        let mut bytes = target.to_le_bytes().to_vec();
        bytes.extend_from_slice(&target.to_le_bytes());
        bytes.extend_from_slice(&target.to_le_bytes()[..7]);
        assert_eq!(
            aligned_pointer_locations(&bytes, 0x1000, target),
            [0x1000, 0x1008]
        );
        let mut shifted = vec![0; 7];
        shifted.extend_from_slice(&target.to_le_bytes());
        assert_eq!(
            aligned_pointer_locations(&shifted, 0x1001, target),
            [0x1008]
        );
        assert!(aligned_pointer_locations(&bytes, 0x1001, target).is_empty());
        assert!(aligned_pointer_locations(&bytes, u64::MAX - 7, target).len() == 1);
        assert!(aligned_pointer_locations(&bytes, u64::MAX, target).is_empty());
        assert!(aligned_pointer_locations(&[], 0, target).is_empty());
    }

    #[test]
    fn field_pointer_locations_classify_bases_without_claiming_ownership() {
        use arm64::Expression::{Constant, Input, Load, Offset, Subtract, Unknown};
        let arguments = [
            Offset(Box::new(Subtract(Box::new(Input(31)), 64)), 40),
            Offset(Box::new(Input(29)), 40),
            Offset(Box::new(Constant(4096)), 40),
            Offset(Box::new(Unknown), 40),
            Offset(
                Box::new(Load {
                    base: Box::new(Input(31)),
                    offset: 0,
                    bits: 64,
                }),
                40,
            ),
            Offset(Box::new(Input(31)), 44),
        ];
        assert_eq!(
            field_pointer_locations(arguments.iter(), 40),
            [
                (0, "stack-relative"),
                (1, "register-derived"),
                (2, "constant-derived"),
                (3, "unknown-derived"),
                (4, "register-derived"),
            ]
        );
        assert!(field_pointer_locations(arguments.iter(), 48).is_empty());
    }

    #[test]
    fn indirect_field_pointers_do_not_cross_direct_calls_or_branches() {
        let bytes = arm_words(&[0x9100a261, 0xf9402d09, 0xd63f0120]);
        let calls = field_indirect_output_candidates(&bytes, 40);
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].0, calls[0].1.site), (0, 8));
        assert_eq!(
            calls[0].1.argument1,
            arm64::Expression::Offset(Box::new(arm64::Expression::Input(19)), 40)
        );
        assert!(field_indirect_output_candidates(&bytes, 44).is_empty());
        for middle in [0x94000000, 0x54000040, 0xd61f0120] {
            let bytes = arm_words(&[0x9100a261, middle, 0xd63f0120]);
            assert!(field_indirect_output_candidates(&bytes, 40).is_empty());
        }
        assert!(field_indirect_output_candidates(&[], 40).is_empty());
    }

    #[test]
    fn entry_memory_metadata_distinguishes_read_write_and_stops_at_calls() {
        let bytes = arm_words(&[0xb9400028, 0xb9000048, 0x94000000, 0xb9400028]);
        let metadata = entry_memory_provenance(&bytes, 0);
        assert_eq!(
            metadata,
            [
                (0, true, 0, 32, arm64::Expression::Input(1)),
                (4, false, 0, 32, arm64::Expression::Input(2)),
            ]
        );
        assert!(entry_memory_provenance(&bytes, 100).is_empty());
        assert!(entry_memory_provenance(&[], 0).is_empty());
    }

    #[test]
    fn field_output_pointers_require_matching_argument_and_reachable_call() {
        let bytes = arm_words(&[0x9100a261, 0x94000002, 0xd503201f, 0xd503201f]);
        let calls = field_output_call_candidates(&bytes, 40);
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].0, calls[0].1.site), (0, 4));
        assert_eq!(
            calls[0].1.argument1,
            arm64::Expression::Offset(Box::new(arm64::Expression::Input(19)), 40)
        );
        assert!(field_output_call_candidates(&bytes, 44).is_empty());
        for register in 0..8 {
            let bytes = arm_words(&[0x9100a260 | register, 0x94000002]);
            assert_eq!(field_output_call_candidates(&bytes, 40).len(), 1);
        }
        for register in 0..3 {
            let bytes = arm_words(&[0x9100a260 | register, 0xd63f0120]);
            assert_eq!(field_indirect_output_candidates(&bytes, 40).len(), 1);
        }
        for words in [
            [0x9100a268, 0x94000002, 0xd503201f], // X8 is not an integer argument.
            [0x9100a261, 0x54000040, 0x94000002], // Conditional edge.
            [0x9100a261, 0xd63f0120, 0x94000002], // Indirect call first.
        ] {
            assert!(field_output_call_candidates(&arm_words(&words), 40).is_empty());
        }
        assert!(field_output_call_candidates(&[], 40).is_empty());
    }

    #[test]
    fn callback_candidates_require_complete_nonnull_aligned_text_addresses() {
        let bytes = arm_words(&[0xd2800188, 0xf9002408, 0xd503201f, 0xd503201f]);
        let candidates = callback_candidates(&bytes, 72);
        assert_eq!(candidates.len(), 1);
        assert_eq!((candidates[0].0, candidates[0].1), (4, 12));
        assert_eq!(candidates[0].2.base, arm64::Expression::Input(0));
        for immediate in [0, 10, 20] {
            let bytes = arm_words(&[
                0xd2800008 | (immediate << 5),
                0xf9002408,
                0xd503201f,
                0xd503201f,
            ]);
            assert!(callback_candidates(&bytes, 72).is_empty());
        }
        let bytes = arm_words(&[0xd2800188, 0xb9004808, 0xd503201f, 0xd503201f]);
        assert!(callback_candidates(&bytes, 72).is_empty());
        assert!(callback_candidates(&[], 72).is_empty());
    }

    #[test]
    fn pointer_table_candidates_require_full_aligned_contained_stores() {
        let bytes = arm_words(&[0xd2820008, 0xf9001c28]);
        let candidates = pointer_store_candidates(&bytes, 56, 0x1000, 0x1008);
        assert_eq!(candidates.len(), 1);
        assert_eq!((candidates[0].0, candidates[0].1), (4, 0x1000));
        assert!(pointer_store_candidates(&bytes, 56, 0x1000, 0x1007).is_empty());
        assert!(pointer_store_candidates(&bytes, 48, 0x1000, 0x1008).is_empty());
        for words in [
            vec![0xd2820008, 0xb9003828],
            vec![0xd2820008, 0xf9001fe8],
            vec![0xd2820088, 0xf9001c28],
            vec![0xd2800008, 0xf9001c28],
            vec![0xd2820008, 0x94000000, 0xf9001c28],
        ] {
            assert!(pointer_store_candidates(&arm_words(&words), 56, 0, 0x2000).is_empty());
        }
    }

    #[test]
    fn ioctl_dispatch_requires_complete_register_consistent_equality_chain() {
        let mut words = vec![
            0x52800008 | (0x4701 << 5),
            0x72a00008 | (0x8004 << 5),
            0x6b08001f,
            0x540000a0,
        ];
        words.resize(10, 0);
        assert_eq!(
            arm64_ioctl_dispatch(&arm_words(&words), 0),
            Some((0x80044701, 32))
        );
        let mut va_words = words.clone();
        va_words[0] = 0x52800008 | (0x4108 << 5);
        va_words[1] = 0x72a00008 | (0xc040 << 5);
        assert_eq!(
            arm64_ioctl_dispatch(&arm_words(&va_words), 0),
            Some((0xc0404108, 32))
        );
        va_words[0] = 0x52800008 | (0x4109 << 5);
        va_words[1] = 0x72a00008 | (0x4028 << 5);
        assert_eq!(
            arm64_ioctl_dispatch(&arm_words(&va_words), 0),
            Some((0x40284109, 32))
        );
        va_words[1] = 0x72a00008 | (0xc020 << 5);
        assert!(arm64_ioctl_dispatch(&arm_words(&va_words), 0).is_none());
        assert!(arm64_ioctl_dispatch(&arm_words(&words)[..15], 0).is_none());
        words[1] ^= 1;
        assert!(arm64_ioctl_dispatch(&arm_words(&words), 0).is_none());
        words[1] ^= 1;
        words[3] |= 1;
        assert!(arm64_ioctl_dispatch(&arm_words(&words), 0).is_none());
    }

    #[test]
    fn field_write_probe_bounds_both_slots_of_offset_word_pairs() {
        let bytes = arm_words(&[0x29030440]);
        assert_eq!(arm64::field_writes(&bytes, 0, 4, 24, 32)[0].source, 0);
        assert_eq!(arm64::field_writes(&bytes, 0, 4, 28, 32)[0].source, 1);
        assert!(arm64::field_writes(&bytes, 0, 3, 28, 32).is_empty());
        assert_eq!(
            arm64::field_writes(&bytes, 0, 4, 28, 64)[0].bits_written,
            32
        );
    }

    #[test]
    fn arm64_constant_probe_rejects_memory_and_unknown_values() {
        assert_eq!(
            arm64_constant_return(&arm_words(&[0x52800020, 0xd65f03c0]), 0),
            Some(1)
        );
        assert_eq!(
            arm64_constant_return(&arm_words(&[0x12800000, 0xd65f03c0]), 0),
            Some(u64::from(u32::MAX))
        );
        assert_eq!(
            arm64_constant_return(&arm_words(&[0x72800020, 0xd65f03c0]), 0),
            None
        );
        assert_eq!(
            arm64_constant_return(&arm_words(&[0xb9400020, 0xd65f03c0]), 0),
            None
        );
        assert_eq!(arm64_constant_return(&[], 1), None);
    }

    #[test]
    fn arm64_edges_preserve_condition_and_signed_target() {
        let bytes = arm_words(&[0x54000040, 0x54000041, 0x17fffffe]);
        assert_eq!(
            arm64_edges(&bytes, 0, 12),
            [(0, "equal", 8), (4, "not-equal", 12), (8, "branch", 0)]
        );
        assert!(arm64_edges(&bytes, 99, 4).is_empty());
        assert!(arm64_edges(&bytes[..3], 0, 4).is_empty());
        let branches = arm_words(&[0xb4000040, 0x35ffffe0, 0x36000040, 0xb7ffffa0]);
        assert_eq!(
            arm64_edges(&branches, 0, branches.len()),
            [
                (0, "compare-zero", 8),
                (4, "compare-nonzero", 0),
                (8, "test-bit-clear", 16),
                (12, "test-bit-set", 0),
            ]
        );
        assert!(arm64_edges(&branches, 1, 4).is_empty());
    }

    #[test]
    fn arm64_getter_and_field_stores_do_not_assume_object_identity() {
        assert_eq!(
            arm64_getter(&arm_words(&[0xb9402820, 0xd65f03c0]), 0),
            Some((32, 40, 1))
        );
        assert_eq!(arm64_getter(&arm_words(&[0xb9402820, 0xd503201f]), 0), None);
        let bytes = arm_words(&[0x52800028, 0xb9002828]);
        assert_eq!(arm64_field_stores(&bytes, 40, 32), [(4, 1, 1)]);
        let bytes = arm_words(&[0x52800028, 0x94000000, 0xb9002828]);
        assert!(arm64_field_stores(&bytes, 40, 32).is_empty());
    }

    #[test]
    fn arm64_ioctl_immediates_exclude_unrelated_and_partial_words() {
        let bytes = arm_words(&[
            0x52800000 | (0x4701 << 5),
            0x72800000 | (0x8004 << 5),
            0x52800020,
        ]);
        assert_eq!(
            ioctl_immediate_candidates(&bytes),
            [(0, 0x4701), (4, 0x8004)]
        );
        assert!(ioctl_immediate_candidates(&bytes[..3]).is_empty());
    }

    #[test]
    fn netlist_table_rejects_truncation_and_out_of_bounds_regions() {
        let mut bytes = vec![0; 24];
        bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&4u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&20u32.to_le_bytes());
        assert_eq!(
            netlist_regions(&bytes),
            Some((
                1,
                vec![NetlistRegion {
                    id: 0,
                    offset: 20,
                    size: 4
                }]
            ))
        );
        assert!(netlist_regions(&bytes[..23]).is_none());
        bytes[16..20].copy_from_slice(&19u32.to_le_bytes());
        assert!(netlist_regions(&bytes).is_none());
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(netlist_regions(&bytes).is_none());
    }

    #[test]
    fn filenames_require_safe_terminated_relative_names() {
        assert_eq!(firmware_filenames(b"fecs.bin\0"), [(0, "fecs.bin")]);
        for bytes in [
            b"/abs.bin\0".as_slice(),
            b"../x.bin\0",
            b"x.bin",
            b"bad name.bin\0",
            b"\xff.bin\0",
        ] {
            assert!(firmware_filenames(bytes).is_empty());
        }
    }

    #[test]
    fn netlist_rejects_duplicate_ids_and_overlapping_nonempty_regions() {
        let mut bytes = vec![0; 40];
        bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&4u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&32u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[24..28].copy_from_slice(&4u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&36u32.to_le_bytes());
        assert!(netlist_regions(&bytes).is_some());
        bytes[28..32].copy_from_slice(&35u32.to_le_bytes());
        assert!(netlist_regions(&bytes).is_none());
        bytes[24..28].copy_from_slice(&0u32.to_le_bytes());
        assert!(netlist_regions(&bytes).is_some());
        bytes[20..24].copy_from_slice(&0u32.to_le_bytes());
        assert!(netlist_regions(&bytes).is_none());
    }

    #[test]
    fn asset_descriptors_require_nonempty_contained_payloads() {
        let mut record = [0; 32];
        record[..8].copy_from_slice(&0x120u64.to_le_bytes());
        record[16..24].copy_from_slice(&0x200u64.to_le_bytes());
        record[24..32].copy_from_slice(&8u64.to_le_bytes());
        assert_eq!(asset_candidates(&record, 0x120, 0x200, 8), [(0, 0x200, 8)]);
        assert!(asset_candidates(&record[..31], 0x120, 0x200, 8).is_empty());
        assert!(asset_candidates(&record, 0x121, 0x200, 8).is_empty());
        assert!(asset_candidates(&record, 0x120, 0x201, 8).is_empty());
        assert!(asset_candidates(&record, 0x120, 0x200, 7).is_empty());
        record[24..32].copy_from_slice(&0u64.to_le_bytes());
        assert!(asset_candidates(&record, 0x120, 0x200, 8).is_empty());
        record[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(asset_candidates(&record, 0x120, 0x200, 8).is_empty());
    }

    #[test]
    fn register_metadata_requires_whole_aiv_records() {
        let mut records = [0; 24];
        records[4..8].copy_from_slice(&40u32.to_le_bytes());
        records[16..20].copy_from_slice(&12u32.to_le_bytes());
        assert_eq!(register_list_summary(&records), Some((2, 12, 40)));
        assert_eq!(register_list_summary(&records[..12]), Some((1, 40, 40)));
        assert!(register_list_summary(&records[..23]).is_none());
        assert!(register_list_summary(&[]).is_none());
    }

    #[test]
    fn markers_are_case_insensitive_bounded_and_do_not_require_utf8() {
        assert_eq!(marker_offsets(b"\xffZcUlL\0zcull", b"zcull"), [1, 7]);
        assert!(marker_offsets(b"zcul", b"zcull").is_empty());
        assert!(marker_offsets(b"payload", b"").is_empty());
    }
}
