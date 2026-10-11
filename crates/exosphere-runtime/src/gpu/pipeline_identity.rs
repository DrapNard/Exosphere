// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable native graphics pipeline identities from complete canonical state.

use exosphere_gpu_ir::GraphicsPipelineDesc;

#[derive(Debug, thiserror::Error)]
#[error("native pipeline identities exhausted")]
pub struct PipelineIdentityExhausted;

/// One native device's graphics-pipeline namespace. Shader IDs must already
/// identify immutable compiled code. Every descriptor field except its assigned
/// ID participates in identity, including fixed state, input and uniform layout.
#[derive(Debug)]
pub struct PipelineIdentityCatalog {
    next_id: u64,
    entries: Vec<GraphicsPipelineDesc>,
}

impl PipelineIdentityCatalog {
    pub fn new(first_id: u64) -> Self {
        Self {
            next_id: first_id,
            entries: Vec::new(),
        }
    }

    pub fn id_for(
        &mut self,
        descriptor: &GraphicsPipelineDesc,
    ) -> Result<u64, PipelineIdentityExhausted> {
        let mut candidate = descriptor.clone();
        for cached in &self.entries {
            candidate.id = cached.id;
            if &candidate == cached {
                return Ok(cached.id);
            }
        }
        let next = self
            .next_id
            .checked_add(1)
            .ok_or(PipelineIdentityExhausted)?;
        let id = self.next_id;
        candidate.id = id;
        self.entries.push(candidate);
        self.next_id = next;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_state_identity_ignores_caller_id_but_not_shader_or_fixed_state() {
        let mut descriptor = exosphere_gpu_ir::GpuModule::triangle_vulkan_smoke_test(32, 32)
            .graphics_pipelines
            .remove(0);
        let mut catalog = PipelineIdentityCatalog::new(7);
        assert_eq!(catalog.id_for(&descriptor).unwrap(), 7);
        descriptor.id = 999;
        assert_eq!(catalog.id_for(&descriptor).unwrap(), 7);
        let mut changed = descriptor.clone();
        changed.vertex_shader += 1;
        assert_eq!(catalog.id_for(&changed).unwrap(), 8);
        changed = descriptor.clone();
        changed.fixed_function.raster.rasterizer_discard = true;
        assert_eq!(catalog.id_for(&changed).unwrap(), 9);
        changed = descriptor.clone();
        changed.primitive_restart = true;
        assert_eq!(catalog.id_for(&changed).unwrap(), 10);
        assert_eq!(catalog.id_for(&descriptor).unwrap(), 7);
        let mut exhausted = PipelineIdentityCatalog::new(u64::MAX - 1);
        assert_eq!(exhausted.id_for(&descriptor).unwrap(), u64::MAX - 1);
        assert!(exhausted.id_for(&changed).is_err());
        assert_eq!(exhausted.id_for(&descriptor).unwrap(), u64::MAX - 1);
    }
}
