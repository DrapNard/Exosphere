// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Vulkan backend for Exosphere.
//!
//! Phase 5 is split in two layers:
//! 1. deterministic lowering into a Vulkan-specific execution plan (implemented here), and
//! 2. raw Vulkan device/swapchain submission (kept behind `VulkanDevice` so it cannot leak into
//!    canonical GPU IR).

pub mod ash_device;
pub mod device;
pub mod loader;
pub mod lower;
pub mod memory_allocator;
pub mod offscreen;
pub mod plan;
pub mod presentation;
pub mod raw_instance;
pub mod resource_validation;
pub mod transfer_emulation;
pub mod vk_convert;

pub use device::{ExecutionToken, FeatureSupport, VulkanCapabilities};

use exosphere_gpu_ir::{GpuModule, GpuResourceData};
use plan::VulkanExecutionPlan;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VulkanError {
    #[error("graphics pipeline {0} does not exist")]
    MissingPipeline(u64),
    #[error("shader {0} does not exist")]
    MissingShader(u64),
    #[error("shader {shader} has stage {actual:?}, expected {expected:?}")]
    ShaderStageMismatch {
        shader: u64,
        expected: exosphere_gpu_ir::ShaderStage,
        actual: exosphere_gpu_ir::ShaderStage,
    },
    #[error("shader still contains opaque Maxwell operations")]
    ShaderNeedsTranslation,
    #[error("GPU module contains an opaque command")]
    OpaqueCommand,
    #[error("raw Vulkan device execution is not available in this build")]
    DeviceUnavailable,
    #[error("GPU resource {0} is not declared by the Vulkan execution plan")]
    MissingResource(u64),
    #[error("GPU resource {0} range arithmetic overflowed")]
    ResourceRangeOverflow(u64),
    #[error(
        "GPU resource {resource} access offset=0x{offset:x} length=0x{length:x} exceeds size=0x{size:x}"
    )]
    ResourceRangeOutOfBounds {
        resource: u64,
        offset: u64,
        length: u64,
        size: u64,
    },
    #[error(
        "GPU resource {resource} payload is too small: required 0x{required:x}, available 0x{available:x}"
    )]
    ResourcePayloadTooSmall {
        resource: u64,
        required: u64,
        available: u64,
    },
    #[error("GPU resource data error: {0}")]
    ResourceData(String),
    #[error("Vulkan backend does not yet execute feature: {0}")]
    UnsupportedFeature(&'static str),
    #[error("Vulkan driver/backend error: {0}")]
    Driver(String),
}

pub struct VulkanBackend {
    device: Box<dyn device::VulkanDevice>,
}

impl Default for VulkanBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl VulkanBackend {
    pub fn new() -> Self {
        Self {
            device: Box::new(device::NullVulkanDevice),
        }
    }

    pub fn with_device(device: Box<dyn device::VulkanDevice>) -> Self {
        Self { device }
    }

    /// Creates the general Vulkan 1.3 backend powered by `ash`.
    ///
    /// The legacy raw-loader device remains available for tiny smoke tests; normal Linux runtime
    /// execution should prefer this constructor.
    pub fn with_ash() -> Result<Self, VulkanError> {
        let device = ash_device::AshVulkanDevice::create().map_err(VulkanError::Driver)?;
        Ok(Self::with_device(Box::new(device)))
    }

    /// Lowers canonical IR without touching the host GPU. Useful for compiler tests and caches.
    pub fn plan(&self, module: &GpuModule) -> Result<VulkanExecutionPlan, VulkanError> {
        lower::lower(module)
    }

    /// Raw device execution is intentionally a second boundary. A future `ash` implementation
    /// can consume exactly the same `VulkanExecutionPlan` without changing Game/GPU IR.
    pub fn execute(&mut self, module: &GpuModule) -> Result<(), VulkanError> {
        let plan = self.plan(module)?;
        self.device.execute_plan(&plan)
    }

    /// Executes canonical GPU IR with access to the current bytes behind its resources.
    ///
    /// Real games should prefer this entry point: vertex/index/indirect buffers, sampled textures
    /// and guest-visible writebacks cannot be implemented correctly from descriptors alone.
    pub fn execute_with_resources(
        &mut self,
        module: &GpuModule,
        resources: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        let plan = self.plan(module)?;
        self.device.execute_plan_with_resources(&plan, resources)
    }
    /// Submits canonical GPU IR asynchronously when supported by the selected backend.
    pub fn submit_with_resources(
        &mut self,
        module: &GpuModule,
        resources: &mut dyn GpuResourceData,
    ) -> Result<ExecutionToken, VulkanError> {
        let plan = self.plan(module)?;
        self.device.submit_plan_with_resources(&plan, resources)
    }

    pub fn is_execution_complete(&self, token: ExecutionToken) -> Result<bool, VulkanError> {
        self.device.is_execution_complete(token)
    }

    /// Waits for completion and applies deferred GPU writes to the guest resource provider.
    pub fn complete_execution(
        &mut self,
        token: ExecutionToken,
        resources: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        self.device.complete_execution(token, resources)
    }
}
