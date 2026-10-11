// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional local Falcon decoder bridge. Firmware and disassembly remain in
//! anonymous pipes/memory; only aggregate counts and candidate addresses escape.

use std::{
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub instructions: usize,
    pub undecoded: usize,
    pub zcull_method_compare_candidates: Vec<u32>,
    pub nearby_branch_targets: Vec<(u32, u32)>,
    pub nearby_call_targets: Vec<(u32, u32)>,
    pub nearby_address_candidates: Vec<(u32, u32)>,
    pub branch_conditions: Vec<(u32, &'static str)>,
    pub handler_candidates: Vec<BlockMetadata>,
    pub dispatch_trace: Vec<u32>,
    pub first_constant_io_write: Option<(u32, u32)>,
    pub method_input_io_candidates: Vec<(u32, u32)>,
    pub method_definition_candidates: Vec<(u32, &'static str)>,
    pub method_mask_candidates: Vec<(u32, u32)>,
    pub mask_operand_shapes: Vec<Vec<&'static str>>,
    pub mask_source_definitions: Vec<(u32, &'static str)>,
    pub method_data_read_addresses: Vec<(u32, usize, u32)>,
    pub data_read_base_constants: Vec<(u32, u32)>,
    pub data_read_base_definitions: Vec<(u32, Vec<(u32, &'static str)>)>,
    pub data_read_base_arithmetic: Vec<(u32, u32, Vec<usize>, Vec<u32>)>,
    pub data_read_arithmetic_source_constants: Vec<(u32, usize, u32)>,
    pub data_read_index_transform_constants: Vec<(u32, u32, &'static str, Vec<u32>)>,
    pub index_data_read_candidates: Vec<(u32, u32, usize, u32, Option<u32>)>,
    pub index_base_definition_candidates: Vec<(u32, Vec<(u32, &'static str)>)>,
    pub index_base_control_edges: Vec<(u32, u32, &'static str, Option<u32>)>,
    pub index_base_external_incoming_edges: Vec<(u32, u32, u32)>,
    pub queue_address_materializations: Vec<(u32, usize, u32)>,
    pub queue_reference_windows: Vec<QueueWindow>,
    pub queue_control_store_candidates: Vec<(u32, u32)>,
    pub constant_io_reads: Vec<(u32, usize, u32)>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct QueueWindow {
    pub reference: u32,
    pub io_reads: Vec<(u32, usize, Option<u32>)>,
    pub io_base_definitions: Vec<(u32, Vec<(u32, &'static str)>)>,
    pub io_base_latest_operands: Vec<(u32, u32, Vec<usize>, Vec<u32>)>,
    pub io_index_origins: Vec<IoIndexOrigin>,
    pub stores: Vec<QueueStore>,
}

/// Nearby definitions are candidate origins, not control-flow dominance proof.
#[derive(Debug, PartialEq, Eq)]
pub struct IoIndexOrigin {
    pub site: u32,
    pub register: usize,
    pub scale: u32,
    pub definitions: Vec<(u32, &'static str)>,
    pub latest_operands: Option<(Vec<usize>, Vec<u32>)>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct QueueStore {
    pub site: u32,
    pub base_register: usize,
    pub offset: u32,
    pub source_register: Option<usize>,
    pub base_constant: Option<u32>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlockMetadata {
    pub start: u32,
    pub calls: Vec<u32>,
    pub address_candidates: Vec<u32>,
    pub loads: usize,
    pub stores: usize,
    pub outgoing: Option<u32>,
}

pub fn inspect(decoder: &Path, instructions: &[u8]) -> io::Result<Summary> {
    let mut child = Command::new(decoder)
        .args(["-m", "falcon", "-V", "fuc5", "-i", "-n"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing decoder input"))?;
    // Read output concurrently: a full stdout pipe must not deadlock the input
    // writer. Closing stdin signals EOF without creating a plaintext file.
    let output = std::thread::scope(|scope| {
        let writer = scope.spawn(move || input.write_all(instructions));
        let output = child.wait_with_output();
        writer
            .join()
            .map_err(|_| io::Error::other("decoder writer failed"))??;
        output
    })?;
    if !output.status.success() {
        return Err(io::Error::other("decoder failed"));
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| io::Error::other("invalid decoder output"))?;
    Ok(summarize(text))
}

fn summarize(text: &str) -> Summary {
    let mut summary = Summary {
        constant_io_reads: constant_io_read_candidates(text),
        ..Summary::default()
    };
    let mut candidate_window = 0;
    for line in text.lines() {
        let Some((address, body)) = line.split_once(':') else {
            continue;
        };
        if address.len() != 8 {
            continue;
        }
        let Ok(address) = u32::from_str_radix(address, 16) else {
            continue;
        };
        summary.instructions += 1;
        if body.contains("[unknown") || body.contains("[incomplete]") {
            summary.undecoded += 1;
        }
        // Tegra hw_gr_gk20a.h defines discover_zcull_image_size as method 0x16.
        // A comparison is only a dispatch candidate, not proof of its meaning.
        let fields: Vec<_> = body.split_whitespace().collect();
        if candidate_window != 0 {
            candidate_window -= 1;
            if fields.contains(&"call")
                && let Some(target) = last_hex_operand(&fields)
            {
                summary.nearby_call_targets.push((address, target));
            }
            if fields.contains(&"mov")
                && let Some(target) = last_hex_operand(&fields)
            {
                summary.nearby_address_candidates.push((address, target));
            }
            if fields.contains(&"bra")
                && let Some(target) = fields
                    .iter()
                    .rev()
                    .find_map(|field| u32::from_str_radix(field.strip_prefix("0x")?, 16).ok())
            {
                summary.nearby_branch_targets.push((address, target));
                let branch = fields.iter().position(|field| *field == "bra").unwrap();
                let condition = match fields.get(branch + 1).copied() {
                    Some("e") => "equal",
                    Some("ne") => "not-equal",
                    Some("g") => "greater",
                    Some("l") => "less",
                    Some("ge") => "greater-equal",
                    Some("le") => "less-equal",
                    Some("b" | "c") => "unsigned-below",
                    Some("be" | "na") => "unsigned-below-equal",
                    Some("a") => "unsigned-above",
                    Some("ae" | "nb" | "nc") => "unsigned-above-equal",
                    Some("not") => "negated-predicate",
                    Some(value) if value.starts_with("$p") => "predicate-register",
                    Some(value) if value.starts_with("0x") => "always",
                    _ => "unclassified",
                };
                summary.branch_conditions.push((address, condition));
            }
        }
        if fields.iter().any(|field| field.starts_with("cmp")) && fields.contains(&"0x16") {
            summary.zcull_method_compare_candidates.push(address);
            candidate_window = 8;
        }
    }
    for &(_, target) in &summary.nearby_branch_targets {
        summary
            .handler_candidates
            .push(block_metadata(text, target));
    }
    if let Some(&start) = summary.zcull_method_compare_candidates.first() {
        summary.method_input_io_candidates = method_input_io_candidates(text, start);
        summary.method_definition_candidates = method_definition_candidates(text, start);
        for &(site, kind) in &summary.method_definition_candidates {
            if kind == "data-read"
                && let Some((base, offset)) = data_read_address(text, site)
            {
                summary
                    .method_data_read_addresses
                    .push((site, base, offset));
                let definitions = register_definition_candidates(text, site, &format!("$r{base}"));
                if let Some(&(definition, "arithmetic")) = definitions.last()
                    && let Some((registers, constants)) = definition_operands(text, definition)
                {
                    for &register in &registers {
                        if let Some(value) = preceding_register_constant(text, definition, register)
                        {
                            summary
                                .data_read_arithmetic_source_constants
                                .push((site, register, value));
                        }
                        for &(definition, kind) in register_definition_candidates(
                            text,
                            definition,
                            &format!("$r{register}"),
                        )
                        .iter()
                        .rev()
                        .take(8)
                        {
                            if matches!(kind, "shift" | "mask")
                                && let Some((_, constants)) = definition_operands(text, definition)
                            {
                                summary
                                    .data_read_index_transform_constants
                                    .push((site, definition, kind, constants));
                            }
                            if kind == "data-read"
                                && let Some((base, offset)) = data_read_address(text, definition)
                            {
                                summary.index_data_read_candidates.push((
                                    site,
                                    definition,
                                    base,
                                    offset,
                                    preceding_register_constant(text, definition, base),
                                ));
                                summary.index_base_definition_candidates.push((
                                    definition,
                                    register_definition_candidates(
                                        text,
                                        definition,
                                        &format!("$r{base}"),
                                    )
                                    .into_iter()
                                    .rev()
                                    .take(4)
                                    .collect(),
                                ));
                                let last_definition = register_definition_candidates(
                                    text,
                                    definition,
                                    &format!("$r{base}"),
                                )
                                .last()
                                .map(|(site, _)| *site);
                                if let Some(start) = last_definition {
                                    for (source, target) in
                                        external_incoming_edges(text, start, definition)
                                    {
                                        summary
                                            .index_base_external_incoming_edges
                                            .push((definition, source, target));
                                    }
                                    for (edge, kind, target) in
                                        control_edges_between(text, start, definition)
                                    {
                                        summary
                                            .index_base_control_edges
                                            .push((definition, edge, kind, target));
                                    }
                                }
                            }
                        }
                    }
                    summary
                        .data_read_base_arithmetic
                        .push((site, definition, registers, constants));
                }
                // Keep only the latest bounded candidates in the report.
                summary
                    .data_read_base_definitions
                    .push((site, definitions.into_iter().rev().take(8).collect()));
                if let Some(value) = preceding_register_constant(text, site, base) {
                    summary.data_read_base_constants.push((site, value));
                }
            }
        }
        for &(site, kind) in &summary.method_definition_candidates {
            if kind == "mask"
                && let Some(line) = text
                    .lines()
                    .find(|line| line.starts_with(&format!("{site:08x}:")))
            {
                let fields: Vec<_> = line.split_whitespace().collect();
                if let Some(and) = fields.iter().position(|field| *field == "and") {
                    summary.mask_operand_shapes.push(
                        fields[and + 1..]
                            .iter()
                            .map(|field| {
                                if field.starts_with("$r") {
                                    "register"
                                } else if field.starts_with("0x") {
                                    "hexadecimal"
                                } else if field.starts_with("-0x") {
                                    "negative-hexadecimal"
                                } else if matches!(*field, "b8" | "b16" | "b32") {
                                    "width"
                                } else {
                                    "other"
                                }
                            })
                            .collect(),
                    );
                }
                let mask = last_hex_operand(&fields).or_else(|| {
                    let source = fields.last()?;
                    register_index(source)?;
                    let (address, producer) = text
                        .lines()
                        .filter_map(|line| {
                            let (address, body) = line.split_once(':')?;
                            let address = u32::from_str_radix(address, 16).ok()?;
                            if address >= site || site - address > 1024 {
                                return None;
                            }
                            let fields: Vec<_> = body.split_whitespace().collect();
                            let op = fields.iter().position(|field| {
                                !(field.len() == 2 && u8::from_str_radix(field, 16).is_ok())
                                    && !matches!(*field, "B" | "C")
                            })?;
                            if matches!(
                                fields[op],
                                "cmp" | "cmpu" | "cmps" | "bra" | "lbra" | "st" | "iowr" | "iowrs"
                            ) {
                                return None;
                            }
                            let destination = fields[op + 1..]
                                .iter()
                                .find(|field| !matches!(**field, "b8" | "b16" | "b32"))?;
                            (*destination == *source).then_some((address, fields[op..].to_vec()))
                        })
                        .next_back()?;
                    let kind = match producer[0] {
                        "mov" => "move",
                        "ld" => "data-read",
                        "and" => "mask",
                        "bset" | "bclr" => "bit-change",
                        _ => "unclassified",
                    };
                    summary.mask_source_definitions.push((address, kind));
                    (producer[0] == "mov")
                        .then(|| producer.iter().rev().find_map(|field| parse_word(field)))
                        .flatten()
                });
                if let Some(mask) = mask {
                    summary.method_mask_candidates.push((site, mask));
                }
            }
        }
        summary.dispatch_trace = trace_dispatch(text, start, 0x16);
        if let Some(&target) = summary.dispatch_trace.last() {
            summary.first_constant_io_write = first_constant_io_write(text, target);
            summary
                .handler_candidates
                .push(block_metadata(text, target));
        }
    }
    let controls: Vec<_> = summary
        .index_data_read_candidates
        .iter()
        .filter_map(|(_, _, _, offset, base)| (*base)?.checked_add(*offset))
        .collect();
    summary.queue_control_store_candidates = constant_address_store_candidates(text, &controls);
    let mut queue_addresses = controls;
    queue_addresses.extend(
        summary
            .data_read_arithmetic_source_constants
            .iter()
            .map(|(_, _, value)| *value),
    );
    summary.queue_address_materializations = address_materializations(text, &queue_addresses);
    summary.queue_reference_windows = summary
        .queue_address_materializations
        .iter()
        .map(|(site, _, _)| queue_reference_window(text, *site))
        .collect();
    summary
}

/// Report statically resolved reads throughout the module, including outside
/// the queue-reference windows. These addresses do not establish execution.
fn constant_io_read_candidates(text: &str) -> Vec<(u32, usize, u32)> {
    text.lines()
        .filter_map(|line| {
            let (site, body) = line.split_once(':')?;
            let site = u32::from_str_radix(site, 16).ok()?;
            let fields: Vec<_> = body.split_whitespace().collect();
            let op = fields
                .iter()
                .position(|field| matches!(*field, "iord" | "iords"))?;
            let destination = fields[op + 1..]
                .iter()
                .find_map(|field| register_index(field))?;
            let memory = fields
                .iter()
                .find_map(|field| field.strip_prefix("I[")?.strip_suffix(']'))?;
            let (base, offset) = memory.split_once('+').unwrap_or((memory, "0x0"));
            let base = parse_word(base)
                .or_else(|| preceding_register_constant(text, site, register_index(base)?))?;
            let address = base.checked_add(io_offset_constant(text, site, offset)?)?;
            Some((site, destination, address))
        })
        .collect()
}

/// Candidate memory/I/O metadata around a reference, not an execution trace.
fn queue_reference_window(text: &str, reference: u32) -> QueueWindow {
    let mut window = QueueWindow {
        reference,
        io_reads: vec![],
        io_base_definitions: vec![],
        io_base_latest_operands: vec![],
        io_index_origins: vec![],
        stores: vec![],
    };
    for line in text.lines() {
        let Some((site, body)) = line.split_once(':') else {
            continue;
        };
        let Ok(site) = u32::from_str_radix(site, 16) else {
            continue;
        };
        if site < reference.saturating_sub(96) || site >= reference.saturating_add(160) {
            continue;
        }
        let fields: Vec<_> = body.split_whitespace().collect();
        if let Some(op) = fields
            .iter()
            .position(|field| matches!(*field, "iord" | "iords"))
            && let Some(destination) = fields[op + 1..]
                .iter()
                .find_map(|field| register_index(field))
            && let Some(memory) = fields
                .iter()
                .find_map(|field| field.strip_prefix("I[")?.strip_suffix(']'))
        {
            let (base, offset) = memory.split_once('+').unwrap_or((memory, "0x0"));
            let address = parse_word(base)
                .or_else(|| preceding_register_constant(text, site, register_index(base)?))
                .and_then(|base| base.checked_add(io_offset_constant(text, site, offset)?));
            window.io_reads.push((site, destination, address));
            if let Some((register, scale)) = offset.split_once('*')
                && let Some(register) = register_index(register)
                && let Some(scale) = parse_word(scale)
            {
                let definitions =
                    register_definition_candidates(text, site, &format!("$r{register}"));
                let latest_operands = definitions
                    .last()
                    .and_then(|(definition, _)| definition_operands(text, *definition));
                window.io_index_origins.push(IoIndexOrigin {
                    site,
                    register,
                    scale,
                    definitions: definitions.into_iter().rev().take(4).collect(),
                    latest_operands,
                });
            }
            if let Some(base) = register_index(base) {
                let definitions = register_definition_candidates(text, site, &format!("$r{base}"));
                if let Some(&(definition, _)) = definitions.last()
                    && let Some((registers, constants)) = definition_operands(text, definition)
                {
                    window
                        .io_base_latest_operands
                        .push((site, definition, registers, constants));
                }
                window.io_base_definitions.push((
                    site,
                    register_definition_candidates(text, site, &format!("$r{base}"))
                        .into_iter()
                        .rev()
                        .take(4)
                        .collect(),
                ));
            }
        }
        if fields.contains(&"st")
            && let Some((base, offset)) = data_read_address(text, site)
        {
            let source = fields
                .iter()
                .filter_map(|field| register_index(field))
                .next_back();
            window.stores.push(QueueStore {
                site,
                base_register: base,
                offset,
                source_register: source,
                base_constant: preceding_register_constant(text, site, base),
            });
        }
    }
    window
}

fn address_materializations(text: &str, addresses: &[u32]) -> Vec<(u32, usize, u32)> {
    text.lines()
        .filter_map(|line| {
            let (site, body) = line.split_once(':')?;
            let site = u32::from_str_radix(site, 16).ok()?;
            let fields: Vec<_> = body.split_whitespace().collect();
            let operation = fields.iter().position(|field| *field == "mov")?;
            let operands = &fields[operation + 1..];
            let destination = operands.iter().find_map(|field| register_index(field))?;
            let value = operands.last().and_then(|field| parse_word(field))?;
            addresses
                .contains(&value)
                .then_some((site, destination, value))
        })
        .collect()
}

/// Direct constant-address stores are leads only; indexed stores are omitted.
fn constant_address_store_candidates(text: &str, addresses: &[u32]) -> Vec<(u32, u32)> {
    text.lines()
        .filter_map(|line| {
            if !line.split_whitespace().any(|field| field == "st") {
                return None;
            }
            let (site, _) = line.split_once(':')?;
            let site = u32::from_str_radix(site, 16).ok()?;
            let (base, offset) = data_read_address(text, site)?;
            let address = preceding_register_constant(text, site, base)?.checked_add(offset)?;
            addresses.contains(&address).then_some((site, address))
        })
        .collect()
}

fn control_edges_between(
    text: &str,
    start: u32,
    end: u32,
) -> Vec<(u32, &'static str, Option<u32>)> {
    text.lines()
        .filter_map(|line| {
            let (address, body) = line.split_once(':')?;
            let address = u32::from_str_radix(address, 16).ok()?;
            if address <= start || address >= end {
                return None;
            }
            let fields: Vec<_> = body.split_whitespace().collect();
            let kind = if fields.contains(&"bra") || fields.contains(&"lbra") {
                "branch"
            } else if fields.contains(&"call") || fields.contains(&"lcall") {
                "call"
            } else if fields.contains(&"ret") {
                "return"
            } else {
                return None;
            };
            Some((address, kind, last_hex_operand(&fields)))
        })
        .collect()
}

/// Static direct edges only. An empty result is not a full dominance proof:
/// indirect transfers and entries outside this model still require analysis.
fn external_incoming_edges(text: &str, start: u32, end: u32) -> Vec<(u32, u32)> {
    if start >= end {
        return vec![];
    }
    text.lines()
        .filter_map(|line| {
            let (source, body) = line.split_once(':')?;
            let source = u32::from_str_radix(source, 16).ok()?;
            if (start..=end).contains(&source) {
                return None;
            }
            let fields: Vec<_> = body.split_whitespace().collect();
            if !fields
                .iter()
                .any(|field| matches!(*field, "bra" | "lbra" | "call" | "lcall"))
            {
                return None;
            }
            let target = last_hex_operand(&fields)?;
            (target > start && target <= end).then_some((source, target))
        })
        .collect()
}

fn data_read_address(text: &str, site: u32) -> Option<(usize, u32)> {
    let line = text
        .lines()
        .find(|line| line.starts_with(&format!("{site:08x}:")))?;
    let memory = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("D[")?.strip_suffix(']'))?;
    let (base, offset) = memory.split_once('+').unwrap_or((memory, "0x0"));
    Some((register_index(base)?, parse_word(offset)?))
}

/// Operand metadata only; never returns decoded instruction text or bytes.
fn definition_operands(text: &str, site: u32) -> Option<(Vec<usize>, Vec<u32>)> {
    let line = text
        .lines()
        .find(|line| line.starts_with(&format!("{site:08x}:")))?;
    let fields: Vec<_> = line.split_whitespace().collect();
    let operation = fields.iter().position(|field| {
        matches!(
            *field,
            "mov" | "sethi" | "clear" | "add" | "sub" | "and" | "shl" | "shr"
        )
    })?;
    let operands = &fields[operation + 1..];
    Some((
        operands
            .iter()
            .filter_map(|field| register_index(field))
            .skip(1)
            .collect(),
        operands
            .iter()
            .filter_map(|field| parse_word(field))
            .collect(),
    ))
}

fn preceding_register_constant(text: &str, site: u32, register: usize) -> Option<u32> {
    register_constant_before(text, site, register, site.saturating_sub(128), 8)
}

/// Falcon indexed I/O uses a scaled second register. Resolve it only when
/// both its prior value and the explicit scale are known; never assume zero.
fn io_offset_constant(text: &str, site: u32, operand: &str) -> Option<u32> {
    if let Some(value) = parse_word(operand) {
        return Some(value);
    }
    let (register, scale) = operand.split_once('*')?;
    let value = preceding_register_constant(text, site, register_index(register)?)?;
    value.checked_mul(parse_word(scale)?)
}

fn register_constant_before(
    text: &str,
    site: u32,
    register: usize,
    earliest: u32,
    depth: u8,
) -> Option<u32> {
    if depth == 0 {
        return None;
    }
    let destination = format!("$r{register}");
    for line in text.lines().rev() {
        let Some((address, body)) = line.split_once(':') else {
            continue;
        };
        let Ok(address) = u32::from_str_radix(address, 16) else {
            continue;
        };
        if address >= site || address < earliest {
            continue;
        }
        let fields: Vec<_> = body.split_whitespace().collect();
        let op = fields.iter().position(|field| {
            !(field.len() == 2 && u8::from_str_radix(field, 16).is_ok())
                && !matches!(*field, "B" | "C")
        })?;
        if matches!(fields[op], "bra" | "lbra" | "call" | "lcall" | "ret") {
            return None;
        }
        if matches!(
            fields[op],
            "cmp" | "cmpu" | "cmps" | "st" | "iowr" | "iowrs"
        ) {
            continue;
        }
        let target = fields[op + 1..]
            .iter()
            .find(|field| !matches!(**field, "b8" | "b16" | "b32"))?;
        if **target == destination {
            // Subword operations may preserve upper register bits. Without
            // modeling that merge, no full-word constant may be claimed.
            if fields[op + 1..]
                .iter()
                .any(|field| matches!(*field, "b8" | "b16"))
            {
                return None;
            }
            let operands: Vec<_> = fields[op + 1..]
                .iter()
                .copied()
                .filter(|field| !matches!(*field, "b8" | "b16" | "b32"))
                .collect();
            let resolve = |operand: &str| {
                parse_word(operand).or_else(|| {
                    register_constant_before(
                        text,
                        address,
                        register_index(operand)?,
                        earliest,
                        depth - 1,
                    )
                })
            };
            if fields[op] == "mov" && operands.len() == 2 {
                return resolve(operands[1]);
            }
            // CLEAR writes zero; the subword guard above deliberately excludes
            // partial-register forms whose preserved upper bits are unknown.
            if fields[op] == "clear" && operands.len() == 1 {
                return Some(0);
            }
            // envytools docs/hw/falcon/arith.rst: SETHI preserves the low
            // sixteen bits and replaces the high half with an unsigned I16.
            if fields[op] == "sethi" && operands.len() == 2 {
                let high = parse_word(operands[1])?;
                if high > 0xffff {
                    return None;
                }
                let old = register_constant_before(text, address, register, earliest, depth - 1)?;
                return Some((old & 0xffff) | (high << 16));
            }
            if !matches!(fields[op], "add" | "sub" | "and" | "shl" | "shr") {
                return None;
            }
            let (left, right) = match operands.as_slice() {
                [_, right] => (
                    register_constant_before(text, address, register, earliest, depth - 1)?,
                    resolve(right)?,
                ),
                [_, left, right] => (resolve(left)?, resolve(right)?),
                _ => return None,
            };
            return match fields[op] {
                "add" => Some(left.wrapping_add(right)),
                "sub" => Some(left.wrapping_sub(right)),
                "and" => Some(left & right),
                "shl" => left.checked_shl(right),
                "shr" => left.checked_shr(right),
                _ => None,
            };
        }
    }
    None
}

fn method_definition_candidates(text: &str, start: u32) -> Vec<(u32, &'static str)> {
    let Some(line) = text
        .lines()
        .find(|line| line.starts_with(&format!("{start:08x}:")))
    else {
        return vec![];
    };
    let Some(register) = line
        .split_whitespace()
        .find(|field| field.starts_with("$r"))
    else {
        return vec![];
    };
    register_definition_candidates(text, start, register)
}

/// Nearby definitions are search leads only, not a dominance/path proof.
fn register_definition_candidates(
    text: &str,
    start: u32,
    register: &str,
) -> Vec<(u32, &'static str)> {
    let lines: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (address, body) = line.split_once(':')?;
            Some((
                u32::from_str_radix(address, 16).ok()?,
                body.split_whitespace().collect::<Vec<_>>(),
            ))
        })
        .collect();
    lines
        .iter()
        .filter_map(|(address, fields)| {
            if *address >= start || start - address > 1024 {
                return None;
            }
            let position = fields.iter().position(|field| {
                matches!(
                    *field,
                    "mov"
                        | "sethi"
                        | "clear"
                        | "or"
                        | "xor"
                        | "ld"
                        | "iord"
                        | "iords"
                        | "add"
                        | "sub"
                        | "and"
                        | "shr"
                        | "shl"
                )
            })?;
            if fields[position + 1..]
                .iter()
                .find(|field| field.starts_with("$r"))
                .copied()
                != Some(register)
            {
                return None;
            }
            let kind = match fields[position] {
                "mov" => "move",
                "sethi" => "set-high",
                "clear" => "clear",
                "or" | "xor" => "logical",
                "ld" => "data-read",
                "iord" | "iords" => "io-read",
                "add" | "sub" => "arithmetic",
                "and" => "mask",
                _ => "shift",
            };
            Some((*address, kind))
        })
        .collect()
}

/// Locate nearby direct I/O definitions of the compared register. These are
/// origin candidates only: this probe does not prove dominance across branches.
fn method_input_io_candidates(text: &str, start: u32) -> Vec<(u32, u32)> {
    let lines: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (address, body) = line.split_once(':')?;
            Some((
                u32::from_str_radix(address, 16).ok()?,
                body.split_whitespace().collect::<Vec<_>>(),
            ))
        })
        .collect();
    let Some((_, compared)) = lines.iter().find(|(address, _)| *address == start) else {
        return vec![];
    };
    let Some(register) = compared.iter().find(|field| field.starts_with("$r")) else {
        return vec![];
    };
    let mut candidates = Vec::new();
    for (index, (address, fields)) in lines.iter().enumerate() {
        if *address >= start || start - address > 1024 {
            continue;
        }
        let Some(op) = fields
            .iter()
            .position(|field| matches!(*field, "iord" | "iords"))
        else {
            continue;
        };
        if fields.get(op + 1) != Some(register) {
            continue;
        }
        let Some(base) = fields
            .get(op + 2)
            .and_then(|memory| memory.strip_prefix("I["))
            .and_then(|memory| memory.strip_suffix(']'))
        else {
            continue;
        };
        for (_, previous) in lines[index.saturating_sub(8)..index].iter().rev() {
            let Some(mov) = previous.iter().position(|field| *field == "mov") else {
                continue;
            };
            if previous.get(mov + 1) == Some(&base)
                && let Some(value) = last_hex_operand(previous)
            {
                candidates.push((*address, value));
                break;
            }
        }
    }
    candidates
}

/// A narrow constant propagation probe; unsupported operations stop analysis.
fn first_constant_io_write(text: &str, start: u32) -> Option<(u32, u32)> {
    let lines: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (address, body) = line.split_once(':')?;
            Some((
                u32::from_str_radix(address, 16).ok()?,
                body.split_whitespace().collect::<Vec<_>>(),
            ))
        })
        .collect();
    let mut index = lines.iter().position(|(address, _)| *address == start)?;
    let mut registers = [None; 16];
    for _ in 0..32 {
        let (_, fields) = lines.get(index)?;
        if let Some(op) = fields.iter().position(|field| *field == "mov") {
            let register = register_index(fields.get(op + 1)?)?;
            registers[register] =
                Some(u32::from_str_radix(fields.get(op + 2)?.strip_prefix("0x")?, 16).ok()?);
            index += 1;
        } else if let Some(op) = fields.iter().position(|field| *field == "lbra") {
            let target = last_hex_operand(&fields[op + 1..])?;
            index = lines.iter().position(|(address, _)| *address == target)?;
        } else {
            let op = fields
                .iter()
                .position(|field| matches!(*field, "iowr" | "iowrs"))?;
            let memory = fields.get(op + 1)?.strip_prefix("I[")?.strip_suffix(']')?;
            let (base, offset) = match memory.split_once('+') {
                Some((base, offset)) => (
                    base,
                    u32::from_str_radix(offset.strip_prefix("0x")?, 16).ok()?,
                ),
                None => (memory, 0),
            };
            let address = registers[register_index(base)?]?.checked_add(offset)?;
            let value = registers[register_index(fields.get(op + 2)?)?]?;
            return Some((address, value));
        }
    }
    None
}

fn register_index(field: &str) -> Option<usize> {
    let index = field.strip_prefix("$r")?.parse::<usize>().ok()?;
    (index < 16).then_some(index)
}

fn parse_word(field: &str) -> Option<u32> {
    if let Some(value) = field.strip_prefix("-0x") {
        Some(u32::from_str_radix(value, 16).ok()?.wrapping_neg())
    } else {
        u32::from_str_radix(field.strip_prefix("0x")?, 16).ok()
    }
}

/// Follow only a comparison/branch dispatch tree with one known method register.
/// Stop before any other operation; this is not a Falcon interpreter.
fn trace_dispatch(text: &str, start: u32, method: u32) -> Vec<u32> {
    let lines: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (address, body) = line.split_once(':')?;
            let address = u32::from_str_radix(address, 16).ok()?;
            Some((address, body.split_whitespace().collect::<Vec<_>>()))
        })
        .collect();
    let Some(mut index) = lines.iter().position(|(address, _)| *address == start) else {
        return vec![];
    };
    let Some(register) = lines[index]
        .1
        .iter()
        .find(|field| field.starts_with("$r"))
        .copied()
    else {
        return vec![];
    };
    let mut comparison = None;
    let mut trace = Vec::new();
    for _ in 0..32 {
        let Some((address, fields)) = lines.get(index) else {
            break;
        };
        trace.push(*address);
        if fields
            .iter()
            .any(|field| matches!(*field, "cmpu" | "cmps" | "cmp"))
            && fields.contains(&register)
        {
            comparison = last_hex_operand(fields).map(|value| method.cmp(&value));
            if comparison.is_none() {
                break;
            }
            index += 1;
            continue;
        }
        let Some(branch) = fields
            .iter()
            .position(|field| *field == "bra" || *field == "lbra")
        else {
            break;
        };
        let args = &fields[branch + 1..];
        if args
            .iter()
            .any(|arg| arg.starts_with("$r") && *arg != register)
        {
            break;
        }
        let Some(target) = last_hex_operand(args) else {
            break;
        };
        if args.contains(&register) {
            let constants: Vec<_> = args
                .iter()
                .filter_map(|field| {
                    field
                        .strip_prefix("0x")
                        .and_then(|value| u32::from_str_radix(value, 16).ok())
                })
                .collect();
            if constants.len() != 2 {
                break;
            }
            comparison = Some(method.cmp(&constants[0]));
        }
        let condition = args
            .iter()
            .find(|field| matches!(**field, "e" | "ne" | "a" | "ae" | "b" | "be"));
        let taken = match (condition.copied(), comparison) {
            (Some("e"), Some(order)) => order.is_eq(),
            (Some("ne"), Some(order)) => !order.is_eq(),
            (Some("a"), Some(order)) => order.is_gt(),
            (Some("ae"), Some(order)) => !order.is_lt(),
            (Some("b"), Some(order)) => order.is_lt(),
            (Some("be"), Some(order)) => !order.is_gt(),
            (None, _) if args.first().is_some_and(|arg| arg.starts_with("0x")) => true,
            _ => break,
        };
        if taken {
            let Some(next) = lines.iter().position(|(address, _)| *address == target) else {
                break;
            };
            index = next;
        } else {
            index += 1;
        }
    }
    trace
}

fn block_metadata(text: &str, start: u32) -> BlockMetadata {
    let mut block = BlockMetadata {
        start,
        ..BlockMetadata::default()
    };
    let mut remaining = 16;
    for line in text.lines() {
        let Some((address, body)) = line.split_once(':') else {
            continue;
        };
        let Ok(address) = u32::from_str_radix(address, 16) else {
            continue;
        };
        if address < start {
            continue;
        }
        if remaining == 0 {
            break;
        }
        remaining -= 1;
        let fields: Vec<_> = body.split_whitespace().collect();
        if (fields.contains(&"call") || fields.contains(&"lcall"))
            && let Some(target) = last_hex_operand(&fields)
        {
            block.calls.push(target);
        }
        if fields.contains(&"mov")
            && let Some(target) = last_hex_operand(&fields)
        {
            block.address_candidates.push(target);
        }
        block.loads += usize::from(fields.contains(&"ld"));
        block.stores += usize::from(fields.contains(&"st"));
        if fields.contains(&"bra") || fields.contains(&"lbra") || fields.contains(&"ret") {
            block.outgoing = last_hex_operand(&fields);
            break;
        }
    }
    block
}

fn last_hex_operand(fields: &[&str]) -> Option<u32> {
    fields
        .iter()
        .rev()
        .find_map(|field| u32::from_str_radix(field.strip_prefix("0x")?, 16).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_io_scan_excludes_unknown_offsets_and_writes() {
        let text = "00000000: aa mov $r8 0x20000\n00000004: aa clear b32 $r9\n00000008: aa iord $r1 I[$r8+$r9*0x4]\n0000000c: aa iords $r2 I[0x400]\n00000010: aa iord $r3 I[$r8+$r7*0x4]\n00000014: aa iowr I[0x800] $r1\n";
        assert_eq!(
            constant_io_read_candidates(text),
            [(8, 1, 0x20000), (12, 2, 0x400)]
        );
        assert!(constant_io_read_candidates("").is_empty());
    }

    #[test]
    fn queue_windows_report_io_and_store_operands_without_claiming_execution() {
        let text = "00000000: aa mov $r8 0x20000\n00000004: aa iord $r1 I[$r8]\n00000008: aa mov $r13 0xd8c\n0000000c: aa st b32 D[$r13+0x4] $r1\n00000100: aa st b32 D[$r13] $r2\n";
        let window = queue_reference_window(text, 8);
        assert_eq!(window.io_reads, [(4, 1, Some(0x20000))]);
        assert_eq!(
            window.stores,
            [QueueStore {
                site: 12,
                base_register: 13,
                offset: 4,
                source_register: Some(1),
                base_constant: Some(0xd8c)
            }]
        );
        let direct = queue_reference_window("00000004: aa iords $r2 I[0x400]\n", 4);
        assert_eq!(direct.io_reads, [(4, 2, Some(0x400))]);
        let unknown = queue_reference_window("00000004: aa iord $r2 I[$r8]\n", 4);
        assert_eq!(unknown.io_reads, [(4, 2, None)]);
        assert!(queue_reference_window("", 0).stores.is_empty());
    }

    #[test]
    fn incoming_edges_exclude_local_edges_and_definition_entry() {
        let text = "00000000: aa bra 0xc\n00000004: aa call 0x8\n00000008: aa mov $r13 0xd8c\n0000000c: aa bra 0x10\n00000010: aa ld b32 $r9 D[$r13]\n00000014: aa lbra 0x10\n";
        assert_eq!(external_incoming_edges(text, 8, 16), [(0, 12), (20, 16)]);
        assert!(external_incoming_edges(text, 16, 8).is_empty());
    }

    #[test]
    fn queue_materializations_require_matching_move_immediates() {
        let text = "00000000: aa mov $r13 0xd8c\n00000004: aa mov $r12 $r13\n00000008: aa add $r9 0xd8c\n0000000c: aa mov $r14 0xd4c\n";
        assert_eq!(address_materializations(text, &[0xd8c]), [(0, 13, 0xd8c)]);
        assert_eq!(address_materializations(text, &[0xd4c]), [(12, 14, 0xd4c)]);
        assert!(address_materializations(text, &[]).is_empty());
    }

    #[test]
    fn control_metadata_reports_only_edges_strictly_between_sites() {
        let text = "00000000: aa bra 0x10\n00000004: aa bra e 0x20\n00000008: aa call 0x30\n0000000c: aa ret\n00000010: aa bra 0x0\n";
        assert_eq!(
            control_edges_between(text, 0, 16),
            [
                (4, "branch", Some(0x20)),
                (8, "call", Some(0x30)),
                (12, "return", None)
            ]
        );
        assert!(control_edges_between(text, 16, 0).is_empty());
        assert!(control_edges_between("", 0, 16).is_empty());
    }

    #[test]
    fn constant_addresses_follow_bounded_arithmetic_and_register_moves() {
        let text = "00000000: aa mov $r12 0xd4c\n00000004: aa mov $r9 0xf\n00000008: aa and $r9 0x7\n0000000c: aa shl $r9 0x3\n00000010: aa add $r13 $r12 $r9\n00000014: aa mov $r14 $r13\n00000018: aa sub $r14 0x4\n";
        assert_eq!(preceding_register_constant(text, 24, 14), Some(0xd84));
        assert_eq!(preceding_register_constant(text, 28, 14), Some(0xd80));
        assert!(register_constant_before(text, 28, 14, 0, 1).is_none());
        let subword = "00000000: aa mov $r9 0x100\n00000004: aa add b8 $r9 0x1\n";
        assert!(preceding_register_constant(subword, 8, 9).is_none());
        let shift = "00000000: aa mov $r9 0x1\n00000004: aa shl $r9 0x20\n";
        assert!(preceding_register_constant(shift, 8, 9).is_none());
    }

    #[test]
    fn indexed_io_offsets_require_known_registers_and_checked_scaling() {
        let text = "00000000: aa mov $r9 0x3\n";
        assert_eq!(io_offset_constant(text, 4, "$r9*0x4"), Some(12));
        assert_eq!(io_offset_constant(text, 4, "0x8"), Some(8));
        assert_eq!(io_offset_constant(text, 4, "$r8*0x4"), None);
        assert_eq!(io_offset_constant(text, 4, "$r9*0xffffffff"), None);
        assert_eq!(io_offset_constant(text, 4, "$r9*invalid"), None);
        let clobbered = "00000000: aa mov $r9 0x3\n00000004: aa ld b32 $r9 D[$r0]\n";
        assert_eq!(io_offset_constant(clobbered, 8, "$r9*0x4"), None);
        let cleared = "00000000: aa mov $r8 0x20000\n00000004: aa clear b32 $r9\n00000008: aa iord $r1 I[$r8+$r9*0x4]\n";
        let window = queue_reference_window(cleared, 8);
        assert_eq!(window.io_reads, [(8, 1, Some(0x20000))]);
        assert_eq!(window.io_index_origins[0].register, 9);
        assert_eq!(window.io_index_origins[0].scale, 4);
        assert_eq!(window.io_index_origins[0].definitions, [(4, "clear")]);
        for width in ["b8", "b16"] {
            let partial = format!("00000000: aa clear {width} $r9\n");
            assert_eq!(io_offset_constant(&partial, 4, "$r9*0x4"), None);
        }
    }

    #[test]
    fn sethi_preserves_low_bits_and_requires_known_prior_value() {
        let text = "00000000: aa mov $r9 0x1234\n00000004: aa sethi $r9 0x2\n";
        assert_eq!(preceding_register_constant(text, 8, 9), Some(0x21234));
        let unknown = "00000004: aa sethi $r9 0x2\n";
        assert!(preceding_register_constant(unknown, 8, 9).is_none());
        let invalid = "00000000: aa mov $r9 0x1234\n00000004: aa sethi $r9 0x10000\n";
        assert!(preceding_register_constant(invalid, 8, 9).is_none());
    }

    #[test]
    fn index_reads_and_control_stores_keep_constant_address_bounds() {
        let text = "00000000: aa mov $r13 0xd8c\n00000004: aa ld b32 $r9 D[$r13]\n00000008: aa and $r9 $r9 0x7\n0000000c: aa shl $r9 $r9 0x3\n00000010: aa mov $r12 0xd4c\n00000014: aa add $r9 $r9 $r12\n00000018: aa ld b32 $r1 D[$r9]\n0000001c: aa cmp b32 $r1 0x16\n00000020: aa st b32 D[$r13] $r0\n";
        let summary = summarize(text);
        assert_eq!(
            summary.index_data_read_candidates,
            [(24, 4, 13, 0, Some(0xd8c))]
        );
        assert_eq!(summary.queue_control_store_candidates, [(32, 0xd8c)]);
        assert!(constant_address_store_candidates(text, &[0xd90]).is_empty());
        let branch =
            "00000000: aa mov $r13 0xd8c\n00000004: aa bra 0x8\n00000008: aa st b32 D[$r13] $r0\n";
        assert!(constant_address_store_candidates(branch, &[0xd8c]).is_empty());
        let overflow = "00000000: aa mov $r13 0xffffffff\n00000004: aa st b32 D[$r13+0x4] $r0\n";
        assert!(constant_address_store_candidates(overflow, &[3]).is_empty());
    }

    #[test]
    fn data_read_base_metadata_tracks_index_arithmetic_without_path_claims() {
        let text = "00000000: aa mov $r12 0xd4c\n00000004: aa and $r9 $r9 0x7\n00000008: aa shl $r9 $r9 0x3\n0000000c: aa add $r9 $r9 $r12\n00000010: aa ld b32 $r1 D[$r9]\n00000014: aa cmp b32 $r1 0x16\n";
        let summary = summarize(text);
        assert_eq!(
            summary.data_read_base_arithmetic,
            [(16, 12, vec![9, 12], vec![])]
        );
        assert_eq!(
            summary.data_read_arithmetic_source_constants,
            [(16, 12, 0xd4c)]
        );
        assert_eq!(
            summary.data_read_index_transform_constants,
            [(16, 8, "shift", vec![3]), (16, 4, "mask", vec![7])]
        );
        assert_eq!(definition_operands(text, 8), Some((vec![9], vec![3])));
        assert!(definition_operands(text, 100).is_none());
    }

    #[test]
    fn register_definition_candidates_match_destinations_not_sources() {
        let text =
            "00000000: aa mov $r9 0x4\n00000004: aa mov $r1 $r9\n00000008: aa cmp b32 $r1 0x16\n";
        assert_eq!(
            register_definition_candidates(text, 8, "$r9"),
            [(0, "move")]
        );
        assert!(method_definition_candidates(text, 100).is_empty());
    }

    #[test]
    fn data_read_metadata_bounds_base_constants_at_control_flow() {
        let text = "00000000: aa mov $r9 0xe34\n00000004: aa ld b32 $r1 D[$r9+0x4]\n";
        assert_eq!(data_read_address(text, 4), Some((9, 4)));
        assert_eq!(preceding_register_constant(text, 4, 9), Some(0xe34));
        let text =
            "00000000: aa mov $r9 0xe34\n00000004: aa bra 0x8\n00000008: aa ld b32 $r1 D[$r9]\n";
        assert_eq!(preceding_register_constant(text, 8, 9), None);
    }

    #[test]
    fn signed_mask_metadata_uses_latest_register_definition() {
        assert_eq!(parse_word("-0xf001"), Some(0xffff0fff));
        assert_eq!(parse_word("-0x1"), Some(u32::MAX));
        assert_eq!(parse_word("0x100000000"), None);
        let text = "00000000: aa mov $r2 -0xf001\n00000004: aa and $r1 $r2\n00000008: aa cmp b32 $r1 0x16\n";
        assert_eq!(summarize(text).method_mask_candidates, [(4, 0xffff0fff)]);
        let text = "00000000: aa mov $r2 -0xf001\n00000004: aa add $r2 $r3\n00000008: aa and $r1 $r2\n0000000c: aa cmp b32 $r1 0x16\n";
        assert!(summarize(text).method_mask_candidates.is_empty());
    }

    #[test]
    fn input_origin_reports_candidates_without_claiming_dominance() {
        let text = "00000000: aa mov $r2 0x21c00\n00000004: aa iord $r1 I[$r2]\n00000008: aa and $r1 0xff\n0000000c: aa cmp b32 $r1 0x16\n";
        assert_eq!(method_input_io_candidates(text, 12), [(4, 0x21c00)]);
        assert_eq!(
            method_definition_candidates(text, 12),
            [(4, "io-read"), (8, "mask")]
        );
        assert!(method_input_io_candidates(text, 99).is_empty());
        assert_eq!(summarize(text).method_mask_candidates, [(8, 0xff)]);
    }

    #[test]
    fn dispatch_trace_follows_known_comparisons_and_stops_at_handler() {
        let text = "00000000: aa cmp b32 $r1 0x16\n00000003: aa bra a 0x20\n00000007: aa cmp b32 $r1 0x14\n0000000a: aa bra a 0x30\n00000020: aa mov $r2 0x1\n00000030: aa mov $r2 0x2\n";
        assert_eq!(trace_dispatch(text, 0, 0x16), [0, 3, 7, 10, 0x30]);
        assert_eq!(trace_dispatch(text, 0, 0x17), [0, 3, 0x20]);
        let text = "00000000: aa cmp b32 $r1 0x16\n00000003: aa bra b32 $r2 0x16 e 0x20\n00000020: aa mov $r2 0x1\n";
        assert_eq!(trace_dispatch(text, 0, 0x16), [0, 3]);
    }

    #[test]
    fn constant_io_probe_never_invents_unknown_register_values() {
        let text = "00000000: aa mov $r1 0x7\n00000004: aa mov $r2 0x20000\n00000008: aa lbra 0x20\n00000020: aa iowr I[$r2+0x4] $r1\n";
        assert_eq!(first_constant_io_write(text, 0), Some((0x20004, 7)));
        assert_eq!(first_constant_io_write(text, 0x20), None);
        let text =
            "00000000: aa mov $r1 0x7\n00000004: aa add $r1 $r3\n00000008: aa iowr I[$r2] $r1\n";
        assert_eq!(first_constant_io_write(text, 0), None);
        assert_eq!(register_index("$r16"), None);
    }

    #[test]
    fn decoder_report_retains_only_counts_and_candidate_addresses() {
        let summary = summarize(
            "label:\n00000010: aa cmpu b32 $r1 0x16\n00000014: bb mov $r2 0x16\n00000018: cc [unknown instruction]\nnothex__: cmp 0x16\n",
        );
        assert_eq!(summary.instructions, 3);
        assert_eq!(summary.undecoded, 1);
        assert_eq!(summary.zcull_method_compare_candidates, [0x10]);
        let summary = summarize("00000010: aa cmpu b32 $r1 0x16\n00000014: ff bra e 0x40\n");
        assert_eq!(summary.nearby_branch_targets, [(0x14, 0x40)]);
    }
}
