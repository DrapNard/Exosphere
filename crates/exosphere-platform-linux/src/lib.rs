// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Linux platform composition layer.
//!
//! This crate is the only place where Linux-specific backend selection should be orchestrated.

use exosphere_hypervisor_kvm::KvmHypervisor;
use exosphere_runtime::Runtime;
pub mod gpu;
pub mod input;
pub mod surface;
pub mod swapchain;
pub mod window;

/// Build the native Linux driver without opening Vulkan until an actual draw.
/// The supported vertex/fragment path uses graphics constant groups 0/4 and
/// separate host descriptor sets. Other stages and control flow still reject.
pub fn create_native_gpu_backend()
-> Result<exosphere_runtime::gpu::nv_backend::NativeGpuBackend, String> {
    let provider = gpu::DirectDrawProgramProvider::new(
        1024 * 1024,
        exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups {
            vertex_group: 0,
            fragment_group: 4,
            vertex_set: 0,
            fragment_set: 1,
        },
    )
    .map_err(|error| error.to_string())?;
    let renderer = gpu::LinuxDrawRenderer::new(provider, 256).map_err(|error| error.to_string())?;
    exosphere_runtime::gpu::nv_backend::NativeGpuBackend::with_renderer(256, Box::new(renderer))
        .map_err(|error| error.to_string())
}

/// Creates the default Linux runtime using KVM.
pub fn create_default_runtime() -> Result<Runtime<KvmHypervisor>, String> {
    let kvm = KvmHypervisor::open().map_err(|e| e.to_string())?;
    let mut runtime = Runtime::new(kvm);
    runtime
        .horizon_mut()
        .install_gpu_ioctl_backend(Box::new(create_native_gpu_backend()?))
        .map_err(|error| error.to_string())?;
    Ok(runtime)
}

/// Planned Linux Phase-10 backend surface.
pub mod planned {
    /// Primary renderer target.
    pub const GRAPHICS: &str = "Vulkan";
    /// Media fallback/bridge target.
    pub const MEDIA: &str = "FFmpeg + native acceleration";
    /// Guest virtualization backend.
    pub const HYPERVISOR: &str = "KVM";
    /// Input plugins may use evdev/libinput without exposing those APIs to the runtime.
    pub const INPUT: &str = "device plugins (evdev/libinput implementation planned)";
}
