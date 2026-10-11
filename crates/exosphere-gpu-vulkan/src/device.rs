// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Raw Vulkan execution boundary.
//!
//! Canonical IR never stores Vulkan handles. `VulkanDevice` is the final backend boundary that
//! consumes a host-specific execution plan. Linux's first real implementation intentionally uses
//! Exosphere's small raw-loader path so the canonical GPU crates do not inherit Vulkan types.

use crate::{
    VulkanError,
    plan::{VulkanExecutionPlan, VulkanPlanCommand},
    raw_instance::RawVulkanInstance,
    resource_validation,
};
use exosphere_gpu_ir::GpuResourceData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureSupport {
    Native,
    Emulated,
    Unsupported,
}

/// Opaque completion point returned by asynchronous Vulkan submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecutionToken {
    pub timeline_value: u64,
}
impl ExecutionToken {
    pub const fn completed() -> Self {
        Self { timeline_value: 0 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VulkanCapabilities {
    pub graphics: FeatureSupport,
    pub compute: FeatureSupport,
    pub transfer: FeatureSupport,
    pub indirect: FeatureSupport,
    pub queries: FeatureSupport,
    pub conditional_rendering: FeatureSupport,
    pub transform_feedback: FeatureSupport,
    pub synchronization: FeatureSupport,
    pub compressed_textures: FeatureSupport,
}
impl VulkanCapabilities {
    pub const fn baseline() -> Self {
        Self {
            graphics: FeatureSupport::Native,
            compute: FeatureSupport::Unsupported,
            transfer: FeatureSupport::Native,
            indirect: FeatureSupport::Unsupported,
            queries: FeatureSupport::Unsupported,
            conditional_rendering: FeatureSupport::Unsupported,
            transform_feedback: FeatureSupport::Unsupported,
            synchronization: FeatureSupport::Emulated,
            compressed_textures: FeatureSupport::Unsupported,
        }
    }
}

pub(crate) fn validate_baseline_plan(plan: &VulkanExecutionPlan) -> Result<(), VulkanError> {
    for command in &plan.commands {
        match command {
            VulkanPlanCommand::BeginRendering { .. }
            | VulkanPlanCommand::BindGraphicsPipeline { .. }
            | VulkanPlanCommand::SetViewport { .. }
            | VulkanPlanCommand::SetScissor { .. }
            | VulkanPlanCommand::Draw { .. }
            | VulkanPlanCommand::EndRendering
            | VulkanPlanCommand::PipelineBarrier { .. }
            | VulkanPlanCommand::CacheFlush { .. }
            | VulkanPlanCommand::InvalidateCaches { .. } => {}
            VulkanPlanCommand::BindVertexBuffer { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "vertex buffers in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::BindIndexBuffer { .. } | VulkanPlanCommand::DrawIndexed { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "indexed draws in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::BindComputePipeline { .. }
            | VulkanPlanCommand::Dispatch { .. }
            | VulkanPlanCommand::DispatchIndirect { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "compute in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::DrawIndirect { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "indirect draws in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::SetRenderTarget { .. }
            | VulkanPlanCommand::SetDepthStencilTarget { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "external attachment binding in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::BindTexture { .. }
            | VulkanPlanCommand::BindSampler { .. }
            | VulkanPlanCommand::BindBufferDescriptor { .. }
            | VulkanPlanCommand::BindImageDescriptor { .. }
            | VulkanPlanCommand::BindSamplerDescriptor { .. }
            | VulkanPlanCommand::PushConstants { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "descriptors/push constants in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::CopyBuffer { .. }
            | VulkanPlanCommand::CopyImage { .. }
            | VulkanPlanCommand::Blit { .. }
            | VulkanPlanCommand::Resolve { .. }
            | VulkanPlanCommand::FillBuffer { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "general transfer operations in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::ClearColor { .. } | VulkanPlanCommand::ClearDepthStencil { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "explicit clear operations in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::SignalSemaphore { .. }
            | VulkanPlanCommand::WaitSemaphore { .. }
            | VulkanPlanCommand::IncrementSyncpoint { .. }
            | VulkanPlanCommand::WaitSyncpoint { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "guest synchronization primitives in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::ResetQuery { .. }
            | VulkanPlanCommand::BeginQuery { .. }
            | VulkanPlanCommand::EndQuery { .. }
            | VulkanPlanCommand::WriteTimestamp { .. }
            | VulkanPlanCommand::CopyQueryResults { .. } => {
                return Err(VulkanError::UnsupportedFeature(
                    "queries in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::BeginConditional { .. } | VulkanPlanCommand::EndConditional => {
                return Err(VulkanError::UnsupportedFeature(
                    "conditional rendering in raw offscreen backend",
                ));
            }
            VulkanPlanCommand::BeginTransformFeedback { .. }
            | VulkanPlanCommand::EndTransformFeedback => {
                return Err(VulkanError::UnsupportedFeature(
                    "transform feedback in raw offscreen backend",
                ));
            }
        }
    }
    Ok(())
}

/// Executes an already-lowered Vulkan plan.
pub trait VulkanDevice: Send {
    fn name(&self) -> &str;
    fn capabilities(&self) -> VulkanCapabilities {
        VulkanCapabilities::baseline()
    }
    fn execute_plan(&mut self, plan: &VulkanExecutionPlan) -> Result<(), VulkanError>;

    /// Executes a plan with access to the current guest/resource payload bytes.
    ///
    /// The default implementation validates resource payloads and then falls back to the legacy
    /// descriptor-only execution path. Backends that upload/download guest memory override this.
    fn execute_plan_with_resources(
        &mut self,
        plan: &VulkanExecutionPlan,
        resources: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        resource_validation::validate_payloads(plan, resources)?;
        self.execute_plan(plan)
    }

    /// Submits work without forcing guest-visible writeback before returning.
    /// Backends without asynchronous execution use the synchronous compatibility path.
    fn submit_plan_with_resources(
        &mut self,
        plan: &VulkanExecutionPlan,
        resources: &mut dyn GpuResourceData,
    ) -> Result<ExecutionToken, VulkanError> {
        self.execute_plan_with_resources(plan, resources)?;
        Ok(ExecutionToken::completed())
    }

    /// Returns whether an asynchronous token has completed.
    fn is_execution_complete(&self, token: ExecutionToken) -> Result<bool, VulkanError> {
        Ok(token.timeline_value == 0)
    }

    /// Completes a token and performs deferred guest-visible writeback.
    fn complete_execution(
        &mut self,
        token: ExecutionToken,
        _resources: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        if token.timeline_value == 0 {
            Ok(())
        } else {
            Err(VulkanError::UnsupportedFeature(
                "asynchronous execution completion",
            ))
        }
    }
}

/// Placeholder backend used by compiler-only tests and hosts without Vulkan.
#[derive(Debug, Default)]
pub struct NullVulkanDevice;
impl VulkanDevice for NullVulkanDevice {
    fn name(&self) -> &str {
        "null-vulkan"
    }
    fn execute_plan(&mut self, _: &VulkanExecutionPlan) -> Result<(), VulkanError> {
        Err(VulkanError::DeviceUnavailable)
    }
}

/// Linux Vulkan backend that owns one real Vulkan instance/device/graphics queue.
///
/// Phase 5 currently renders execution plans into an offscreen color attachment. Swapchain/window
/// presentation stays in the platform layer; this backend already executes real shader modules,
/// graphics pipeline creation, render-pass recording and queue submission.
pub struct RawLinuxVulkanDevice {
    instance: RawVulkanInstance,
}

impl RawLinuxVulkanDevice {
    pub fn create() -> Result<Self, String> {
        Ok(Self {
            instance: RawVulkanInstance::create()?,
        })
    }

    pub fn instance(&self) -> &RawVulkanInstance {
        &self.instance
    }
}

// RawVulkanInstance owns all handles and serializes access through &mut VulkanDevice execution.
unsafe impl Send for RawLinuxVulkanDevice {}

impl VulkanDevice for RawLinuxVulkanDevice {
    fn name(&self) -> &str {
        "raw-linux-vulkan"
    }

    fn execute_plan(&mut self, plan: &VulkanExecutionPlan) -> Result<(), VulkanError> {
        resource_validation::analyze(plan)?;
        validate_baseline_plan(plan)?;
        self.instance
            .execute_offscreen_plan(plan)
            .map(|_| ())
            .map_err(VulkanError::Driver)
    }

    fn execute_plan_with_resources(
        &mut self,
        plan: &VulkanExecutionPlan,
        resources: &mut dyn GpuResourceData,
    ) -> Result<(), VulkanError> {
        resource_validation::validate_payloads(plan, resources)?;
        if crate::transfer_emulation::supports_entire_plan(plan) {
            match self.instance.execute_native_transfer_plan(plan, resources) {
                Ok(()) => return Ok(()),
                Err(_) => {
                    // Native Vulkan is preferred, but early/legacy drivers may lack a convenient
                    // coherent host-visible memory type. Preserve correctness through the existing
                    // synchronous provider fallback rather than failing an otherwise valid guest DMA.
                    return crate::transfer_emulation::execute(plan, resources);
                }
            }
        }
        self.execute_plan(plan)
    }
}
