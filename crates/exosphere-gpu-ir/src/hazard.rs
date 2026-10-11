// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Backend-neutral resource hazard analysis.
//!
//! The pass is intentionally conservative. It identifies RAW/WAR/WAW hazards between canonical
//! commands so Vulkan/Metal backends can materialize barriers without embedding host API concepts in
//! the Game IR.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Command, ResourceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HazardKind {
    ReadAfterWrite,
    WriteAfterRead,
    WriteAfterWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hazard {
    pub resource: ResourceId,
    pub previous_command: usize,
    pub current_command: usize,
    pub kind: HazardKind,
}

#[derive(Debug, Clone, Default)]
pub struct HazardReport {
    pub hazards: Vec<Hazard>,
}

pub fn analyze_hazards(commands: &[Command]) -> HazardReport {
    let mut last_write: BTreeMap<ResourceId, usize> = BTreeMap::new();
    let mut readers: BTreeMap<ResourceId, BTreeSet<usize>> = BTreeMap::new();
    let mut report = HazardReport::default();

    for (index, command) in commands.iter().enumerate() {
        let (reads, writes) = accesses(command);
        for resource in reads {
            if let Some(previous) = last_write.get(&resource).copied() {
                report.hazards.push(Hazard {
                    resource,
                    previous_command: previous,
                    current_command: index,
                    kind: HazardKind::ReadAfterWrite,
                });
            }
            readers.entry(resource).or_default().insert(index);
        }
        for resource in writes {
            if let Some(previous) = last_write.insert(resource, index) {
                report.hazards.push(Hazard {
                    resource,
                    previous_command: previous,
                    current_command: index,
                    kind: HazardKind::WriteAfterWrite,
                });
            }
            if let Some(previous_readers) = readers.remove(&resource) {
                for previous in previous_readers {
                    report.hazards.push(Hazard {
                        resource,
                        previous_command: previous,
                        current_command: index,
                        kind: HazardKind::WriteAfterRead,
                    });
                }
            }
        }
    }
    report
}

fn accesses(command: &Command) -> (Vec<ResourceId>, Vec<ResourceId>) {
    match command {
        Command::Copy {
            source,
            destination,
            ..
        }
        | Command::CopyImage {
            source,
            destination,
        }
        | Command::Blit {
            source,
            destination,
            ..
        }
        | Command::Resolve {
            source,
            destination,
        } => (vec![*source], vec![*destination]),
        Command::Fill { destination, .. }
        | Command::ClearColor {
            target: destination,
            ..
        }
        | Command::ClearDepthStencil {
            target: destination,
            ..
        } => (Vec::new(), vec![*destination]),
        Command::BindVertexBuffer { buffer, .. } | Command::BindIndexBuffer { buffer, .. } => {
            (vec![*buffer], Vec::new())
        }
        Command::BindTexture { texture, .. } | Command::BindImageDescriptor { texture, .. } => {
            (vec![*texture], Vec::new())
        }
        Command::BindBufferDescriptor { buffer, .. } => (vec![*buffer], Vec::new()),
        Command::BindSamplerDescriptor { sampler, .. } => (vec![*sampler], Vec::new()),
        Command::SetRenderTarget { texture, .. } | Command::SetDepthStencilTarget { texture } => {
            (Vec::new(), vec![*texture])
        }
        Command::DrawIndirect { buffer, .. }
        | Command::DispatchIndirect { buffer, .. }
        | Command::BeginConditional { buffer, .. } => (vec![*buffer], Vec::new()),
        Command::ResetQuery { pool, .. }
        | Command::BeginQuery { pool, .. }
        | Command::EndQuery { pool, .. }
        | Command::WriteTimestamp { pool, .. } => (Vec::new(), vec![*pool]),
        Command::CopyQueryResults {
            pool, destination, ..
        } => (vec![*pool], vec![*destination]),
        Command::BeginTransformFeedback { buffers } => {
            (Vec::new(), buffers.iter().map(|(id, _, _)| *id).collect())
        }
        _ => (Vec::new(), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_copy_then_vertex_read_hazard() {
        let src = ResourceId(1);
        let dst = ResourceId(2);
        let commands = vec![
            Command::Copy {
                source: src,
                destination: dst,
                source_offset: 0,
                destination_offset: 0,
                bytes: 16,
            },
            Command::BindVertexBuffer {
                slot: 0,
                buffer: dst,
                offset: 0,
            },
        ];
        let report = analyze_hazards(&commands);
        assert!(
            report
                .hazards
                .iter()
                .any(|h| h.resource == dst && h.kind == HazardKind::ReadAfterWrite)
        );
    }
}
