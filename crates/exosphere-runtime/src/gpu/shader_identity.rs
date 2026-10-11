// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable host shader identities, separate from guest program addresses.

use exosphere_gpu_ir::ShaderStage;

#[derive(Debug, thiserror::Error)]
#[error("native shader identities exhausted")]
pub struct ShaderIdentityExhausted;

/// One native device's namespace. Exact stage/code comparison avoids hash
/// collisions; identical code reuses an ID, changed code gets a new immutable
/// ID even when it came from the same guest address. Pipeline identities must
/// also account for their newly selected shader IDs.
#[derive(Debug)]
pub struct ShaderIdentityCatalog {
    next_id: u64,
    entries: Vec<(ShaderStage, Vec<u32>, u64)>,
}

impl ShaderIdentityCatalog {
    pub fn new(first_id: u64) -> Self {
        Self {
            next_id: first_id,
            entries: Vec::new(),
        }
    }

    /// Supply code from a successfully compiled shader, not guest raw bytes.
    pub fn id_for(
        &mut self,
        stage: ShaderStage,
        words: &[u32],
    ) -> Result<u64, ShaderIdentityExhausted> {
        if let Some((_, _, id)) = self
            .entries
            .iter()
            .find(|(known_stage, known_words, _)| *known_stage == stage && known_words == words)
        {
            return Ok(*id);
        }
        let next = self.next_id.checked_add(1).ok_or(ShaderIdentityExhausted)?;
        let id = self.next_id;
        self.entries.push((stage, words.to_vec(), id));
        self.next_id = next;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_stage_and_code_identity_does_not_depend_on_guest_addresses() {
        let mut catalog = ShaderIdentityCatalog::new(7);
        assert_eq!(catalog.id_for(ShaderStage::Vertex, &[1, 2]).unwrap(), 7);
        assert_eq!(catalog.id_for(ShaderStage::Vertex, &[1, 2]).unwrap(), 7);
        assert_eq!(catalog.id_for(ShaderStage::Fragment, &[1, 2]).unwrap(), 8);
        assert_eq!(catalog.id_for(ShaderStage::Vertex, &[1, 3]).unwrap(), 9);
        assert_eq!(catalog.id_for(ShaderStage::Vertex, &[1, 2]).unwrap(), 7);
        let mut exhausted = ShaderIdentityCatalog::new(u64::MAX - 1);
        assert_eq!(
            exhausted.id_for(ShaderStage::Vertex, &[1]).unwrap(),
            u64::MAX - 1
        );
        assert!(exhausted.id_for(ShaderStage::Vertex, &[2]).is_err());
        assert_eq!(
            exhausted.id_for(ShaderStage::Vertex, &[1]).unwrap(),
            u64::MAX - 1
        );
    }
}
