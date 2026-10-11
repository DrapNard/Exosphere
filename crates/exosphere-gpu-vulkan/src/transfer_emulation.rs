// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Correctness-first host fallback for simple transfer-only Vulkan plans.
//!
//! This is not the fast path. It exists so early Exosphere builds can execute DMA-style buffer
//! operations before every corresponding Vulkan command is wired into the raw backend. Mixed
//! graphics/transfer plans are deliberately not reordered here.

use exosphere_gpu_ir::GpuResourceData;

use crate::{
    VulkanError,
    plan::{VulkanExecutionPlan, VulkanPlanCommand},
    resource_validation,
};

const COPY_CHUNK: usize = 1024 * 1024;

/// Returns true when every command can be executed entirely through [`GpuResourceData`].
pub fn supports_entire_plan(plan: &VulkanExecutionPlan) -> bool {
    plan.commands.iter().all(|command| {
        matches!(
            command,
            VulkanPlanCommand::CopyBuffer { .. }
                | VulkanPlanCommand::FillBuffer { .. }
                | VulkanPlanCommand::PipelineBarrier { .. }
                | VulkanPlanCommand::CacheFlush { .. }
                | VulkanPlanCommand::InvalidateCaches { .. }
        )
    })
}

/// Executes a transfer-only plan while preserving command order.
pub fn execute(
    plan: &VulkanExecutionPlan,
    resources: &mut dyn GpuResourceData,
) -> Result<(), VulkanError> {
    if !supports_entire_plan(plan) {
        return Err(VulkanError::UnsupportedFeature(
            "mixed host-emulated transfer plan",
        ));
    }
    resource_validation::analyze(plan)?;

    for command in &plan.commands {
        match command {
            VulkanPlanCommand::CopyBuffer {
                source,
                destination,
                source_offset,
                destination_offset,
                bytes,
            } => {
                copy_buffer(
                    resources,
                    *source,
                    *destination,
                    *source_offset,
                    *destination_offset,
                    *bytes,
                )?;
            }
            VulkanPlanCommand::FillBuffer {
                destination,
                value,
                bytes,
            } => {
                fill_buffer(resources, *destination, *value, *bytes)?;
            }
            VulkanPlanCommand::PipelineBarrier { .. }
            | VulkanPlanCommand::CacheFlush { .. }
            | VulkanPlanCommand::InvalidateCaches { .. } => {
                // The provider API is already synchronous. These operations are ordering points in
                // the host fallback and therefore require no separate driver primitive.
            }
            _ => unreachable!("supports_entire_plan filtered unsupported commands"),
        }
    }
    Ok(())
}

fn copy_buffer(
    resources: &mut dyn GpuResourceData,
    source: exosphere_gpu_ir::ResourceId,
    destination: exosphere_gpu_ir::ResourceId,
    source_offset: u64,
    destination_offset: u64,
    bytes: u64,
) -> Result<(), VulkanError> {
    // Copy through a temporary chunk so source/destination may alias without borrowing provider
    // storage directly. For exact self-copy there is nothing to do.
    if (source == destination && source_offset == destination_offset) || bytes == 0 {
        return Ok(());
    }

    let mut offset = 0_u64;
    let mut scratch = vec![0_u8; COPY_CHUNK.min(usize::try_from(bytes).unwrap_or(COPY_CHUNK))];
    while offset < bytes {
        let remaining = bytes - offset;
        let chunk_len = usize::try_from(remaining.min(COPY_CHUNK as u64))
            .map_err(|_| VulkanError::ResourceRangeOverflow(source.0))?;
        resources
            .read(source, source_offset + offset, &mut scratch[..chunk_len])
            .map_err(|error| VulkanError::ResourceData(error.to_string()))?;
        resources
            .write(
                destination,
                destination_offset + offset,
                &scratch[..chunk_len],
            )
            .map_err(|error| VulkanError::ResourceData(error.to_string()))?;
        offset = offset
            .checked_add(chunk_len as u64)
            .ok_or(VulkanError::ResourceRangeOverflow(source.0))?;
    }
    resources
        .flush(destination)
        .map_err(|error| VulkanError::ResourceData(error.to_string()))
}

fn fill_buffer(
    resources: &mut dyn GpuResourceData,
    destination: exosphere_gpu_ir::ResourceId,
    value: u32,
    bytes: u64,
) -> Result<(), VulkanError> {
    if bytes == 0 {
        return Ok(());
    }
    let pattern = value.to_le_bytes();
    let chunk_capacity = COPY_CHUNK.min(usize::try_from(bytes).unwrap_or(COPY_CHUNK));
    let mut chunk = vec![0_u8; chunk_capacity];
    for (index, byte) in chunk.iter_mut().enumerate() {
        *byte = pattern[index & 3];
    }

    let mut offset = 0_u64;
    while offset < bytes {
        let remaining = bytes - offset;
        let chunk_len = usize::try_from(remaining.min(COPY_CHUNK as u64))
            .map_err(|_| VulkanError::ResourceRangeOverflow(destination.0))?;
        resources
            .write(destination, offset, &chunk[..chunk_len])
            .map_err(|error| VulkanError::ResourceData(error.to_string()))?;
        offset = offset
            .checked_add(chunk_len as u64)
            .ok_or(VulkanError::ResourceRangeOverflow(destination.0))?;
    }
    resources
        .flush(destination)
        .map_err(|error| VulkanError::ResourceData(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_gpu_ir::{BufferDesc, BufferUsage, InMemoryGpuResources, ResourceId};

    #[test]
    fn copies_and_fills_payloads() {
        let source = ResourceId(1);
        let destination = ResourceId(2);
        let mut resources = InMemoryGpuResources::new();
        resources.insert(source, vec![1, 2, 3, 4, 5, 6, 7, 8], false);
        resources.insert(destination, vec![0; 8], true);
        let plan = VulkanExecutionPlan {
            buffers: vec![
                BufferDesc {
                    id: source,
                    size: 8,
                    usage: BufferUsage::TransferSource,
                },
                BufferDesc {
                    id: destination,
                    size: 8,
                    usage: BufferUsage::TransferDestination,
                },
            ],
            commands: vec![
                VulkanPlanCommand::CopyBuffer {
                    source,
                    destination,
                    source_offset: 0,
                    destination_offset: 0,
                    bytes: 8,
                },
                VulkanPlanCommand::FillBuffer {
                    destination,
                    value: 0xAABB_CCDD,
                    bytes: 4,
                },
            ],
            ..Default::default()
        };
        execute(&plan, &mut resources).unwrap();
        assert_eq!(
            resources.bytes(destination).unwrap(),
            &[0xDD, 0xCC, 0xBB, 0xAA, 5, 6, 7, 8]
        );
    }

    #[test]
    fn lowers_and_executes_subrange_copies_without_touching_surrounding_bytes() {
        use exosphere_gpu_ir::{Command, GpuModule};
        let id = ResourceId(1);
        let module = GpuModule {
            buffers: vec![BufferDesc {
                id,
                size: 16,
                usage: BufferUsage::TransferDestination,
            }],
            commands: vec![Command::Copy {
                source: id,
                destination: id,
                source_offset: 2,
                destination_offset: 10,
                bytes: 4,
            }],
            ..Default::default()
        };
        let mut resources = InMemoryGpuResources::new();
        let original: Vec<u8> = (0..16).collect();
        resources.insert(id, original.clone(), true);
        let mut plan = crate::lower::lower(&module).unwrap();
        execute(&plan, &mut resources).unwrap();
        let mut expected = original;
        expected[10..14].copy_from_slice(&[2, 3, 4, 5]);
        assert_eq!(resources.bytes(id).unwrap(), expected);

        plan.commands = vec![VulkanPlanCommand::CopyBuffer {
            source: id,
            destination: id,
            source_offset: 2,
            destination_offset: 2,
            bytes: 4,
        }];
        execute(&plan, &mut resources).unwrap();
        assert_eq!(resources.bytes(id).unwrap(), expected);

        for (source_offset, destination_offset, bytes) in [
            (2, 3, 4),        // overlapping regions
            (2, 15, 4),       // destination out of bounds
            (15, 2, 4),       // source out of bounds
            (u64::MAX, 2, 4), // arithmetic overflow
        ] {
            plan.commands = vec![VulkanPlanCommand::CopyBuffer {
                source: id,
                destination: id,
                source_offset,
                destination_offset,
                bytes,
            }];
            assert!(execute(&plan, &mut resources).is_err());
            assert_eq!(resources.bytes(id).unwrap(), expected);
        }
    }
}
