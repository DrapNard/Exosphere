// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Lightweight backend-neutral GPU dependency graph used for offline optimization and diagnostics.

use crate::{Command, ResourceId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: NodeId,
    pub command_index: usize,
    pub reads: BTreeSet<ResourceId>,
    pub writes: BTreeSet<ResourceId>,
}

#[derive(Debug, Clone, Default)]
pub struct GpuGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: BTreeMap<NodeId, BTreeSet<NodeId>>,
}

impl GpuGraph {
    pub fn from_commands(commands: &[Command]) -> Self {
        let mut graph = GpuGraph::default();
        let mut last_writer: BTreeMap<ResourceId, NodeId> = BTreeMap::new();
        for (i, command) in commands.iter().enumerate() {
            let id = NodeId(i as u32);
            let mut reads = BTreeSet::new();
            let mut writes = BTreeSet::new();
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
                } => {
                    reads.insert(*source);
                    writes.insert(*destination);
                }
                Command::Fill { destination, .. }
                | Command::ClearColor {
                    target: destination,
                    ..
                }
                | Command::ClearDepthStencil {
                    target: destination,
                    ..
                } => {
                    writes.insert(*destination);
                }
                Command::BindVertexBuffer { buffer, .. }
                | Command::BindIndexBuffer { buffer, .. } => {
                    reads.insert(*buffer);
                }
                Command::BindTexture { texture, .. }
                | Command::BindSampler {
                    sampler: texture, ..
                }
                | Command::BindImageDescriptor { texture, .. }
                | Command::BindSamplerDescriptor {
                    sampler: texture, ..
                } => {
                    reads.insert(*texture);
                }
                Command::BindBufferDescriptor { buffer, .. } => {
                    reads.insert(*buffer);
                }
                Command::SetRenderTarget { texture, .. }
                | Command::SetDepthStencilTarget { texture } => {
                    writes.insert(*texture);
                }
                Command::DrawIndirect { buffer, .. }
                | Command::DispatchIndirect { buffer, .. }
                | Command::BeginConditional { buffer, .. } => {
                    reads.insert(*buffer);
                }
                Command::ResetQuery { pool, .. }
                | Command::BeginQuery { pool, .. }
                | Command::EndQuery { pool, .. }
                | Command::WriteTimestamp { pool, .. } => {
                    writes.insert(*pool);
                }
                Command::CopyQueryResults {
                    pool, destination, ..
                } => {
                    reads.insert(*pool);
                    writes.insert(*destination);
                }
                Command::BeginTransformFeedback { buffers } => {
                    for (id, _, _) in buffers {
                        writes.insert(*id);
                    }
                }
                _ => {}
            }
            for r in &reads {
                if let Some(w) = last_writer.get(r) {
                    graph.edges.entry(*w).or_default().insert(id);
                }
            }
            for w in &writes {
                if let Some(prev) = last_writer.insert(*w, id) {
                    graph.edges.entry(prev).or_default().insert(id);
                }
            }
            graph.nodes.push(GraphNode {
                id,
                command_index: i,
                reads,
                writes,
            });
        }
        graph
    }
}
