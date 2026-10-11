// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Runtime orchestration independent from Linux/Windows/macOS details.

use exosphere_horizon::{HorizonKernel, HorizonProfileId};
use exosphere_hypervisor::Hypervisor;
pub mod execution;
pub mod gpu;
pub mod interpreter;
pub mod program;

/// Runtime shell owning host-independent Horizon state plus a selected hypervisor backend.
pub struct Runtime<H: Hypervisor> {
    hypervisor: H,
    horizon: HorizonKernel,
}

impl<H: Hypervisor> Runtime<H> {
    pub fn new(hypervisor: H) -> Self {
        Self::with_horizon_profile(hypervisor, HorizonProfileId("generic".into()))
    }

    pub fn with_horizon_profile(hypervisor: H, profile: HorizonProfileId) -> Self {
        Self {
            hypervisor,
            horizon: HorizonKernel::new(profile),
        }
    }

    pub fn hypervisor_name(&self) -> &'static str {
        self.hypervisor.backend_name()
    }
    pub fn horizon(&self) -> &HorizonKernel {
        &self.horizon
    }
    pub fn horizon_mut(&mut self) -> &mut HorizonKernel {
        &mut self.horizon
    }
}
