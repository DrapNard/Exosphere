// SPDX-License-Identifier: GPL-3.0-or-later
//! Host implementation boundary for authenticated GPU queue IOCTLs.

use super::{NvGpuChannelIdentity, address_space::NvGpuAddressSpace};
use crate::{
    HorizonResult,
    ipc::{ServiceRequest, ServiceResponse},
    memory::AddressSpace,
};

/// Horizon owns transport and memory; the runtime owns command execution.
/// Implementations must not report completion merely because work was admitted.
pub trait NvGpuIoctlBackend: std::fmt::Debug {
    /// Consume bounded work using current canonical mappings. Zero means idle
    /// or waiting, not successful completion of an outstanding submission.
    fn poll_channel(
        &mut self,
        _identity: &NvGpuChannelIdentity,
        _memory: &mut AddressSpace,
        _gpu: &NvGpuAddressSpace,
        _maximum_entries: usize,
    ) -> HorizonResult<usize> {
        Ok(0)
    }

    /// Release state on descriptor close, last-session close or process exit.
    /// Must be idempotent: handles may subsequently close after process teardown.
    fn close_channel(&mut self, _identity: NvGpuChannelIdentity) -> HorizonResult<()> {
        Ok(())
    }

    fn dispatch(
        &mut self,
        identity: NvGpuChannelIdentity,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse>;
}
