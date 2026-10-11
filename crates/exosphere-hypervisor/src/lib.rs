// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Hypervisor abstraction and dynamically-loadable guest-device plugins.

pub mod device;
pub mod loader;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HypervisorError {
    #[error("hypervisor operation is unsupported")]
    Unsupported,
    #[error("host hypervisor error: {0}")]
    Host(String),
}

/// Project-owned hypervisor interface.
///
/// Backends such as KVM/WHPX/Apple implement this trait. Runtime code must depend on this trait and
/// never on KVM-specific file descriptors or ioctls.
pub trait Hypervisor: Send {
    fn backend_name(&self) -> &'static str;
    fn map_memory(&mut self, guest_addr: u64, bytes: &mut [u8]) -> Result<(), HypervisorError>;
    fn inject_irq(&mut self, irq: u32) -> Result<(), HypervisorError>;
    fn run_vcpu(&mut self, vcpu: u32) -> Result<(), HypervisorError>;
}
