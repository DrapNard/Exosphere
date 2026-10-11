// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Linux KVM hypervisor backend bootstrap.
//!
//! The implementation intentionally starts with capability probing. VM/vCPU ioctls are added behind
//! the project-owned `Hypervisor` trait so no KVM-specific API leaks into the runtime.

use exosphere_hypervisor::{Hypervisor, HypervisorError};
use std::{fs::OpenOptions, os::fd::AsRawFd};

pub struct KvmHypervisor {
    kvm: std::fs::File,
}

impl KvmHypervisor {
    /// Opens `/dev/kvm` and verifies that the host exposes the KVM API device.
    pub fn open() -> Result<Self, HypervisorError> {
        let kvm = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .map_err(|e| HypervisorError::Host(e.to_string()))?;
        Ok(Self { kvm })
    }

    pub fn raw_fd(&self) -> i32 {
        self.kvm.as_raw_fd()
    }
}

impl Hypervisor for KvmHypervisor {
    fn backend_name(&self) -> &'static str {
        "kvm"
    }

    fn map_memory(&mut self, _guest_addr: u64, _bytes: &mut [u8]) -> Result<(), HypervisorError> {
        // TODO(phase-10): KVM_SET_USER_MEMORY_REGION. Kept here rather than in runtime code so the
        // memory-slot model can later evolve without becoming part of the public runtime ABI.
        Err(HypervisorError::Unsupported)
    }

    fn inject_irq(&mut self, _irq: u32) -> Result<(), HypervisorError> {
        // TODO(phase-10): route to irqfd/GSI or the guest device model selected by the runtime.
        Err(HypervisorError::Unsupported)
    }

    fn run_vcpu(&mut self, _vcpu: u32) -> Result<(), HypervisorError> {
        // TODO(phase-10): KVM_RUN loop + typed exit translation.
        Err(HypervisorError::Unsupported)
    }
}
