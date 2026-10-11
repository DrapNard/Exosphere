// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Coverage accounting for the GPU translator.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoverageCounter {
    pub total: u64,
    pub semantic: u64,
    pub opaque: u64,
}
impl CoverageCounter {
    pub fn record_known(&mut self) {
        self.total += 1;
        self.semantic += 1;
    }
    pub fn record_opaque(&mut self) {
        self.total += 1;
        self.opaque += 1;
    }
    pub fn permille(self) -> u16 {
        (u128::from(self.semantic) * 1000)
            .checked_div(u128::from(self.total))
            .unwrap_or(1000) as u16
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuCoverage {
    pub pfifo: CoverageCounter,
    pub graphics_methods: CoverageCounter,
    pub compute_methods: CoverageCounter,
    pub dma_methods: CoverageCounter,
    pub inline_methods: CoverageCounter,
    pub twod_methods: CoverageCounter,
    pub shader_instructions: CoverageCounter,
    pub mme_words: CoverageCounter,
}

/// Coverage categories are deliberately separate from architectural feature completeness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SemanticCoverage {
    pub maxwell_registers: CoverageCounter,
    pub sm53_instructions: CoverageCounter,
    pub mme_instructions: CoverageCounter,
    pub texture_descriptors: CoverageCounter,
    pub sampler_descriptors: CoverageCounter,
}

impl SemanticCoverage {
    pub fn overall_permille(self) -> u16 {
        let counters = [
            self.maxwell_registers,
            self.sm53_instructions,
            self.mme_instructions,
            self.texture_descriptors,
            self.sampler_descriptors,
        ];
        // Aggregate in a wider type: valid u64 counters can overflow their
        // sum or the per-mille numerator without exceeding individual limits.
        let total: u128 = counters.iter().map(|c| u128::from(c.total)).sum();
        let semantic: u128 = counters.iter().map(|c| u128::from(c.semantic)).sum();
        (semantic * 1000).checked_div(total).unwrap_or(1000) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coverage_ratios_preserve_empty_policy_and_large_valid_counts() {
        assert_eq!(CoverageCounter::default().permille(), 1000);
        let full = CoverageCounter {
            total: u64::MAX,
            semantic: u64::MAX,
            opaque: 0,
        };
        assert_eq!(full.permille(), 1000);
        let half = CoverageCounter {
            total: u64::MAX,
            semantic: u64::MAX / 2,
            opaque: u64::MAX / 2 + 1,
        };
        assert_eq!(half.permille(), 499);
        let all = SemanticCoverage {
            maxwell_registers: full,
            sm53_instructions: full,
            mme_instructions: full,
            texture_descriptors: full,
            sampler_descriptors: full,
        };
        assert_eq!(all.overall_permille(), 1000);
    }
}
